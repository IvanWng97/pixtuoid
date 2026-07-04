use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
    hermes::SOURCE_NAME,
    openclaw::SOURCE_NAME,
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

/// Backpressure bound for the workspace-wide `(Transport, AgentEvent)` event
/// channel — the ONE place this capacity is defined. The runtime reducer feed
/// (`runtime::driver` + `floating`) and the hook tee (`hook::router`) all size
/// their channels from this so the tee adds a stage, not a different
/// backpressure policy; keeping one constant stops the three from drifting.
pub const EVENT_CHANNEL_CAPACITY: usize = 256;

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

/// Test-ergonomic conversion by tool NAME. `"Agent"` (current CC's dispatch
/// tool) maps to `Task`, so a test written as `Some("Agent".into())` exercises
/// the real `is_task()` path (suppression / Delegating / b1) instead of
/// silently falling to `Generic`. The legacy `"Task"` name was dropped here in
/// 0.12.0, in lockstep with `decoder::make_tool_detail`'s known-name set — a
/// test-only alias for a name production no longer recognizes would give false
/// confidence. Production code calls `make_tool_detail`, which additionally
/// detects a dispatch SEMANTICALLY via the `subagent_type` input field (the
/// rename-resilient path, THE mechanism); this name-only helper can't see the
/// input, so it keys on the known name.
impl From<&str> for ToolDetail {
    fn from(s: &str) -> Self {
        if s == "Agent" {
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
        /// True ONLY when this end's SUBJECT is a CHILD agent ending *as a
        /// child* — stamped by the subagent-END decoders: the CC/Codex
        /// `SubagentStop` hooks (#241), CodeWhale `subagent_complete`,
        /// opencode's child `session.deleted` (all Hook transport), and
        /// copilot's `subagent.completed`/`subagent.failed` transcript lines
        /// (the one JSONL-transported `true` constructor — which is why the
        /// #246 tee's un-claim guard keys on Hook TRANSPORT as well as this
        /// stamp). The reducer's child ledger (#244/#246) keys on the stamp:
        /// it remembers the child's applied parent and starts the
        /// ended-recently window that blocks a late/reordered parented
        /// re-registration and re-links a parentless revival. EVERY other
        /// constructor — the shared hook `SessionEnd` arm, the
        /// watcher-synthesized JSONL terminators (oversized-skip,
        /// negative-vouch, instant-exit), Reasonix's `/new` rotation — stamps
        /// `false` and never writes the ledger, so parentless root resurrects
        /// stay untouched by construction.
        ///
        /// Source-trait CONTRACT: only subagent-END decoders may stamp `true`;
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

pub mod antigravity;
// The async runtime + watcher + liveness-probe layer: gated out of a wasm
// (`--no-default-features`) build. These modules own all the tokio/notify/libc
// FFI in the crate; the per-source modules below stay compiled because their
// pure DECODERS feed the registry — each mixed module's runtime half (its
// `impl Source`, probes, watcher wiring) lives in a once-gated `native`
// sub-module (`source/<cli>/native.rs`), re-exported from the parent so the
// public paths don't move.
#[cfg(feature = "native")]
pub(crate) mod cc_probe;
pub mod claude_code;
pub mod codewhale;
pub mod codex;
pub mod copilot;
pub mod cursor;
/// The shared, daemon-agnostic presence layer (state machine + lifecycle for
/// every daemon-style source; OpenClaw is instance #1). Per-daemon wire decode
/// stays in the daemon's own module.
pub mod daemon;
pub mod decoder;
pub mod drift;
#[cfg(feature = "native")]
pub(crate) mod exit_watch;
#[cfg(feature = "native")]
pub(crate) mod fd_probe;
pub mod hermes;
#[cfg(feature = "native")]
pub mod hook;
#[cfg(feature = "native")]
pub mod jsonl;
#[cfg(feature = "native")]
pub mod manager;
// The async transport seam (tagged channel + `Source`/`DynSource`); the
// re-export keeps the pre-split `source::{Source, TaggedSender, …}` paths.
#[cfg(feature = "native")]
mod native;
#[cfg(feature = "native")]
pub use native::{DynSource, Source, TaggedReceiver, TaggedSender};
pub mod openclaw;
pub mod opencode;
pub mod reasonix;
// `doc(hidden)`: the registry is an internal fact table, `pub` ONLY so the
// integration-test crates (sources::conformance) can read it. Hiding it keeps it
// off the published API — cargo-semver-checks then lets descriptor/caps
// fields evolve (the most likely change when adding a CLI) without a
// breaking-version bump. Same treatment as `jsonl`'s test-only seam.
#[doc(hidden)]
pub mod registry;
