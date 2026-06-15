use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::id::AgentId;

/// CLI sources this build supports. The canonical NAME list the conformance
/// tests iterate; all other per-source facts (label prefix, decoders, hook
/// keying, reducer caps) live in ONE row per source in [`registry::REGISTRY`].
/// Every entry MUST have, enforced by tests so omissions fail CI rather than
/// ship as the silent two-sprite-ghost bug:
///   - a coalescing fixture under `tests/sources/fixtures/<name>/` —
///     `tests/sources/conformance.rs`'s
///     `every_registered_source_has_a_coalescing_fixture` (shape per the row:
///     hook+JSONL for CC/Codex, JSONL-only for antigravity, hook-only for
///     reasonix — `hook-payloads.jsonl` with no transcript), and
///   - a [`registry::SourceDescriptor`] row — pinned below by
///     `registry_covers_exactly_the_registered_sources` (the prefix/decoder
///     shape checks live with the registry's own tests).
///
/// Each entry is keyed off its module's `SOURCE_NAME` const so a rename is a
/// compile error, not a silent two-sprite-ghost. (Stable Rust can't const-
/// project the names out of `REGISTRY`, hence two lists + the bridge test.)
pub const REGISTERED_SOURCES: &[&str] = &[
    claude_code::SOURCE_NAME,
    codex::SOURCE_NAME,
    antigravity::SOURCE_NAME,
    reasonix::SOURCE_NAME,
    codewhale::SOURCE_NAME,
    opencode::SOURCE_NAME,
    copilot::SOURCE_NAME,
    cursor::SOURCE_NAME,
];

#[cfg(test)]
mod registry_bridge_tests {
    use super::*;

    // The names list and the fact table must cover EXACTLY the same sources —
    // a REGISTERED_SOURCES entry without a descriptor row (or vice versa) is
    // the new flavor of the registered-but-not-wired bug class.
    #[test]
    fn registry_covers_exactly_the_registered_sources() {
        for src in REGISTERED_SOURCES {
            assert!(
                registry::descriptor_for(src).is_some(),
                "registered source {src:?} has no SourceDescriptor row — add it to registry::REGISTRY"
            );
        }
        for d in registry::REGISTRY {
            assert!(
                REGISTERED_SOURCES.contains(&d.name),
                "descriptor {:?} is not in REGISTERED_SOURCES — add it there",
                d.name
            );
        }
    }
}

/// Which transport produced an event — used by the reducer for hook-wins
/// dedup. Lives on the source side because every `Source` implementor must
/// tag its own events; the reducer is downstream.
///
/// `#[non_exhaustive]`: this is the tag on the workspace-wide event channel and
/// a published-crate type, and the docs anticipate transport growth (a v2
/// daemon split). Marking it non-exhaustive keeps adding a transport a
/// non-breaking change, matching its channel siblings (`AgentEvent`,
/// `ToolDetail`, `SourceDeath`). All in-crate uses are `==` comparisons, not
/// exhaustive matches, so this is purely a forward-compat guard. (#212 — lands
/// with the 0.7.0 bump so the one-time break rides the version boundary.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Transport {
    Hook,
    Jsonl,
}

/// Structured tool detail. Replaces the free-form `Option<String>` so the
/// reducer can pattern-match (instead of string-scanning) on semantic
/// categories like Task-delegation, which is load-bearing for subagent
/// suppression.
/// `#[non_exhaustive]`: new tool categories (beyond Task/Generic) are
/// expected as more agent semantics get modeled, so downstream `match`es
/// must carry a wildcard arm — adding a variant then stays non-breaking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ToolDetail {
    /// CC `Task` tool — kicks off a subagent. Reducer suppresses
    /// hook-sourced Activity events for the parent until the matching
    /// `ActivityEnd` arrives (subagent leak suppression).
    Task,
    /// Any other tool. `display` is the user-facing label
    /// (e.g. `"Bash: ls"`, `"Edit foo.rs"`) used for the AgentSlot detail.
    Generic { display: String },
}

impl ToolDetail {
    pub fn display(&self) -> &str {
        match self {
            ToolDetail::Task => "Delegating",
            ToolDetail::Generic { display } => display,
        }
    }
    pub fn is_task(&self) -> bool {
        matches!(self, ToolDetail::Task)
    }
}

/// Test-ergonomic conversion by tool NAME. Both subagent-dispatch names map to
/// `Task` — `"Agent"` (current CC) and legacy `"Task"` — so a test written as
/// `Some("Agent".into())` exercises the real `is_task()` path (suppression /
/// Delegating / b1) instead of silently falling to `Generic`. Production code
/// calls `decoder::make_tool_detail`, which additionally detects a dispatch
/// SEMANTICALLY via the `subagent_type` input field (the rename-resilient path);
/// this name-only helper can't see the input, so it keys on the known names.
impl From<&str> for ToolDetail {
    fn from(s: &str) -> Self {
        if s == "Task" || s == "Agent" {
            ToolDetail::Task
        } else {
            ToolDetail::Generic {
                display: s.to_string(),
            }
        }
    }
}

/// `#[non_exhaustive]`: the event vocabulary grows as new agent CLIs and
/// lifecycle signals are modeled (Codex subagent hooks, future
/// permission/compaction events), so external `match`es must carry a
/// wildcard — adding a variant then stays a minor, non-breaking change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AgentEvent {
    SessionStart {
        agent_id: AgentId,
        source: String,
        session_id: String,
        cwd: PathBuf,
        parent_id: Option<AgentId>,
    },
    ActivityStart {
        agent_id: AgentId,
        tool_use_id: Option<String>,
        detail: Option<ToolDetail>,
    },
    ActivityEnd {
        agent_id: AgentId,
        tool_use_id: Option<String>,
    },
    Waiting {
        agent_id: AgentId,
        reason: String,
    },
    /// Late-discovered display name (e.g. CC subagent `attributionAgent`).
    /// Reducer overrides the slot label; noop if the slot doesn't exist.
    Rename {
        agent_id: AgentId,
        label: String,
    },
    SessionEnd {
        agent_id: AgentId,
        /// True ONLY when this end was decoded from a `SubagentStop` hook
        /// (CC #241 / Codex) — the SUBJECT is a CHILD agent ending *as a
        /// child*. The reducer's child ledger (#244/#246) keys on this stamp:
        /// it remembers the child's applied parent and starts the
        /// ended-recently window that blocks a late/reordered parented
        /// re-registration and re-links a parentless revival. EVERY other
        /// constructor — the shared hook `SessionEnd` arm, JSONL terminators,
        /// the watcher's negative-vouch/instant-exit synthesis, Reasonix's
        /// `/new` rotation — stamps `false` and never writes the ledger, so
        /// parentless root resurrects stay untouched by construction.
        ///
        /// Source-trait CONTRACT: only SubagentStop decoders may stamp `true`;
        /// a custom `Source` must stamp `false` on every root end. (Not
        /// enforced structurally on purpose: a misbehaving source already
        /// controls the whole event stream — it can forge `SessionStart`
        /// parent links directly, which is strictly stronger than poisoning
        /// the ledger — so a private constructor would add friction to the
        /// documented extension seam without adding a trust boundary.)
        as_child: bool,
    },
    /// Emitted by a watcher once per liveness-probe refresh for EVERY session
    /// id the probe currently vouches for (CC's `sessions/<pid>.json`
    /// registry, Codex's open-rollout FD binding). The reducer ONLY refreshes
    /// a sweep-exemption timestamp for an existing, non-exiting slot — it must
    /// never create a slot, never touch activity state, and never refresh
    /// `last_event_at` (the Active→Idle debounce and the label/back-fill logic
    /// stay driven by real events). When the live signal disappears the
    /// emissions stop and normal staleness sweeps resume after the TTL.
    ProofOfLife {
        agent_id: AgentId,
    },
    /// Identity context a hook decoder attaches IMMEDIATELY AHEAD of a
    /// tool/permission activity event (#221): hook payloads carry
    /// source/session_id/cwd that the activity variants don't, so without
    /// this a proof-of-life registration for an unknown id starts BLANK
    /// (empty identity, ordinal `#N` label) until the next real
    /// `SessionStart` — for a hook-only source (Reasonix) that's the whole
    /// rest of the turn. The reducer registers-or-back-fills from it on the
    /// Hook transport ONLY (a JSONL `Identity` is a structural no-op —
    /// transcript lines can be historical replays and must never synthesize);
    /// it never touches labels, activity state, or `last_event_at` (the
    /// paired activity event right behind it carries those).
    Identity {
        agent_id: AgentId,
        source: String,
        session_id: String,
        /// `None` when the payload carries no usable cwd (e.g. CC PostToolUse,
        /// Codex PermissionRequest) — the registration is then label-ordinal
        /// but still reap-exempt, exactly like the blank synthesis path.
        cwd: Option<PathBuf>,
    },
}

impl AgentEvent {
    pub fn agent_id(&self) -> AgentId {
        match self {
            AgentEvent::SessionStart { agent_id, .. } => *agent_id,
            AgentEvent::ActivityStart { agent_id, .. } => *agent_id,
            AgentEvent::ActivityEnd { agent_id, .. } => *agent_id,
            AgentEvent::Waiting { agent_id, .. } => *agent_id,
            AgentEvent::Rename { agent_id, .. } => *agent_id,
            AgentEvent::SessionEnd { agent_id, .. } => *agent_id,
            AgentEvent::ProofOfLife { agent_id, .. } => *agent_id,
            AgentEvent::Identity { agent_id, .. } => *agent_id,
        }
    }
}

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

pub mod antigravity;
pub(crate) mod cc_probe;
pub mod claude_code;
pub mod codewhale;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod decoder;
pub mod drift;
pub(crate) mod exit_watch;
pub(crate) mod fd_probe;
pub mod hook;
pub mod jsonl;
pub mod manager;
pub mod opencode;
pub mod reasonix;
// `doc(hidden)`: the registry is an internal fact table, `pub` ONLY so the
// integration-test crates (sources::conformance) can read it. Hiding it keeps it
// off the published API — cargo-semver-checks then lets descriptor/caps
// fields evolve (the most likely change when adding a CLI) without a
// breaking-version bump. Same treatment as `jsonl`'s test-only seam.
#[doc(hidden)]
pub mod registry;
