use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::source::{DynSource, TaggedSender};

/// A source's fatal exit, published on the health channel so the binary can
/// surface it (#157). Plain data — no terminal deps (workspace invariant #1);
/// how it reaches the user (TUI footer, stderr) is the consumer's call.
/// `non_exhaustive` + [`SourceDeath::new`] keep a future field (timestamp,
/// exit kind) a minor bump instead of a `constructible_struct_adds_field`
/// major at the CI semver gate (the #131 class).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SourceDeath {
    /// [`Source::name`] of the source that died (e.g. "claude-code").
    pub source: String,
    /// Display rendering of the fatal error.
    pub error: String,
}

impl SourceDeath {
    pub fn new(source: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            error: error.into(),
        }
    }
}

/// Owns a set of `Source` implementations and spawns each as its own tokio
/// task, multiplexing their events onto a single `TaggedSender`. The single-
/// source case is just `SourceManager::new().with_source(Box::new(src)).spawn(tx)`.
/// Adding a second CLI (Codex, Cursor, Gemini, …) is a one-line addition.
#[derive(Default)]
pub struct SourceManager {
    sources: Vec<Box<dyn DynSource>>,
}

impl SourceManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one more `Source`. Builder-style — chain to add several.
    /// Named `with_source` (not `add`) to avoid clippy's
    /// `should_implement_trait` confusing it with `std::ops::Add`.
    pub fn with_source(mut self, source: Box<dyn DynSource>) -> Self {
        self.sources.push(source);
        self
    }

    /// Spawn one tokio task per source. Each task gets its own clone of `tx`,
    /// so the channel stays open as long as any source is alive. Errors from
    /// individual sources are logged via `tracing` and do not abort siblings.
    pub fn spawn(self, tx: TaggedSender) -> Vec<JoinHandle<()>> {
        // Health channel with no listener: send_modify works without
        // receivers, so the no-health path shares one implementation.
        let (deaths, _) = watch::channel(Vec::new());
        self.spawn_with_health(tx, deaths)
    }

    /// Like [`SourceManager::spawn`], additionally APPENDING each source's
    /// fatal exit onto the `deaths` watch channel (#157). A dead source means
    /// its agents silently freeze and stale-sweep out over 10–60 min — the
    /// worst class of partial failure — so the death must reach a surface the
    /// user actually watches, not only `tracing` (which has no subscriber in
    /// default TUI mode beyond the warn-level file log).
    pub fn spawn_with_health(
        self,
        tx: TaggedSender,
        deaths: watch::Sender<Vec<SourceDeath>>,
    ) -> Vec<JoinHandle<()>> {
        self.sources
            .into_iter()
            .map(|src| {
                let tx = tx.clone();
                let deaths = deaths.clone();
                // `run(self: Box<Self>)` consumes the source — capture the
                // name first so the report (and log line) can attribute it.
                let name = src.name().to_string();
                tokio::spawn(async move {
                    if let Err(e) = src.run(tx).await {
                        tracing::error!(source = %name, "source died: {e:#}");
                        deaths.send_modify(|v| v.push(SourceDeath::new(name, format!("{e:#}"))));
                    }
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{AgentEvent, Source, Transport};

    struct DyingSource;

    impl Source for DyingSource {
        fn name(&self) -> &str {
            "dying-test-source"
        }
        async fn run(self: Box<Self>, _tx: TaggedSender) -> anyhow::Result<()> {
            anyhow::bail!("listener exploded")
        }
    }

    struct HealthySource;

    impl Source for HealthySource {
        fn name(&self) -> &str {
            "healthy-test-source"
        }
        async fn run(self: Box<Self>, _tx: TaggedSender) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn fatal_source_exit_is_published_on_the_health_channel() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);
        let (deaths_tx, deaths_rx) = watch::channel(Vec::new());
        let handles = SourceManager::new()
            .with_source(Box::new(DyingSource))
            .with_source(Box::new(HealthySource))
            .spawn_with_health(tx, deaths_tx);
        for h in handles {
            h.await.unwrap();
        }
        let deaths = deaths_rx.borrow().clone();
        assert_eq!(
            deaths,
            vec![SourceDeath::new("dying-test-source", "listener exploded")],
            "a fatal source exit must be attributed and published; a clean exit must not"
        );
    }

    #[tokio::test]
    async fn spawn_without_health_listener_does_not_panic_on_death() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);
        for h in SourceManager::new()
            .with_source(Box::new(DyingSource))
            .spawn(tx)
        {
            h.await.unwrap();
        }
    }
}
