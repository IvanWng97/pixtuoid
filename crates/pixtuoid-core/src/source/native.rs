//! The `native`-only async transport seam: the `Transport`-tagged tokio
//! channel aliases + the `Source`/`DynSource` traits. A `--no-default-features`
//! (wasm) build has no tokio, so none of this exists there — the whole module
//! sits behind ONE `#[cfg(feature = "native")]` gate in `source/mod.rs` and is
//! re-exported from `crate::source`, so public paths don't move.

use tokio::sync::mpsc;

use super::{AgentEvent, Transport};

/// Events sent on a tagged channel so the reducer knows which transport produced them.
pub type TaggedSender = mpsc::Sender<(Transport, AgentEvent)>;
pub type TaggedReceiver = mpsc::Receiver<(Transport, AgentEvent)>;

/// A `Source` produces `AgentEvent`s from one agent CLI flavor (Claude Code,
/// Codex, Cursor, Gemini, Copilot, etc.) and sends them on a `Transport`-
/// tagged channel.
///
/// ## Implementor contract
///
/// 1. **`name()`** — returns a stable, lowercase identifier for this source
///    (e.g. `"claude-code"`, `"codex"`, `"cursor"`). Used both as the
///    `AgentSlot.source` field and as the first argument to
///    [`AgentId::from_parts`] so two sources with the same opaque session
///    id never collide.
///
/// 2. **`AgentId` derivation** — every `AgentEvent::SessionStart` MUST carry
///    an `agent_id` constructed via [`AgentId::from_parts(self.name(),
///    opaque_id)`][`AgentId::from_parts`]. `opaque_id` is whatever your source uses to uniquely
///    identify a session: a JSONL transcript path for CC, a session UUID
///    for SDK-based sources, the socket path for hook-based sources.
///    Constructing `AgentId`s any other way risks cross-source collisions.
///
/// 3. **Transport tagging** — every event you send must be tagged with the
///    appropriate [`Transport`] enum variant. The reducer relies on this
///    tag for hook-vs-JSONL dedup; sending the wrong tag silently breaks
///    that logic.
///
/// 4. **Never panic** — sources run inside a tokio task that doesn't
///    propagate panics cleanly. Log + continue on malformed input rather
///    than `unwrap`.
///
/// [`AgentId::from_parts`]: crate::AgentId::from_parts
pub trait Source: Send + 'static {
    fn name(&self) -> &str;
    fn run(
        self: Box<Self>,
        tx: TaggedSender,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;
}

/// Object-safety twin of [`Source`] — the type `SourceManager` actually
/// boxes (`Box<dyn DynSource>`). It exists ONLY because [`Source`]'s native
/// `-> impl Future + Send` return (RPITIT, how the `+ Send` bound is
/// expressed without `async-trait`) is not dyn-compatible, so `dyn Source`
/// cannot exist. Don't merge the two traits or make `Source` `dyn` again —
/// that's the un-simplifiable WHY of the split. Source authors never name
/// this trait: the blanket impl below + unsize coercion let
/// `with_source(Box::new(my_source))` work directly; implement [`Source`]
/// only.
pub trait DynSource: Send + 'static {
    fn name(&self) -> &str;
    fn run(
        self: Box<Self>,
        tx: TaggedSender,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>;
}

/// The bridge: every [`Source`] is a [`DynSource`] whose future is boxed at
/// the erasure boundary — the same one box per `run` that `async-trait` used
/// to add, now paid only where dynamic dispatch genuinely needs it. The
/// inner `self.name()`/`self.run(tx)` calls resolve to `<T as Source>` (the
/// where-clause candidate), not recursively to this impl.
impl<T: Source> DynSource for T {
    fn name(&self) -> &str {
        self.name()
    }

    fn run(
        self: Box<Self>,
        tx: TaggedSender,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>> {
        Box::pin(self.run(tx))
    }
}
