use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::id::AgentId;

// THE roster of CLI sources this build supports is `registry::REGISTRY` — one
// row per source, all per-source facts (label prefix, decoders, hook keying,
// reducer caps) in that ONE place — projected to just the names by
// `registry::registered_source_names`. (The old hand-maintained
// `REGISTERED_SOURCES` names list + its `registry_bridge_tests` were deleted:
// Stable Rust can't const-project names out of `REGISTRY`, so the two-list sync
// was pure ceremony; every consumer iterates at runtime.) Each row is keyed off
// its module's `SOURCE_NAME` const so a rename is a compile error, not a silent
// two-sprite-ghost, and every source MUST also carry a coalescing fixture
// (`tests/sources/conformance.rs::every_registered_source_has_a_coalescing_fixture`).

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
    /// registry, Codex's & omp's open-FD binding, grok's `active_sessions.json` registry). The reducer ONLY refreshes
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
        /// The agent process's pid (+ recycle marker), from the shim/plugin
        /// `_pid` stamp — the focus-jump channel for hook-only sources.
        /// Transcript-family sources resolve pid via their liveness probes
        /// instead and always carry `None` here (structural:
        /// `patch_identity_pids` skips any source with a `line_decoder`).
        /// Hook-transport Identity recurs ahead of every activity, so the
        /// slot's cached pid stays fresh. serde-skipped so the
        /// conformance/scene goldens don't churn on `None`.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        pid: Option<PidIdentity>,
    },
    /// An LLM model/effort OBSERVATION from a source's wire (the burn-tier
    /// feature): CC assistant lines carry `message.model` per turn, Codex
    /// `turn_context` carries `model` + `effort`, copilot per-tool `model`,
    /// opencode `session.created`, grok `model_changed`. Both fields are RAW wire strings (the
    /// house RAW-store/interpret-at-paint posture — the burn-tier tables in
    /// `pixtuoid-scene` do the reading); CC's periodic ultra-effort marker
    /// carries no wire value, so its arm synthesizes `"ultra"`/`"ultrathink"`.
    /// The reducer updates an EXISTING slot only (unknown id = no-op — a
    /// model line never registers a session) and dedups per field; decoders
    /// may emit per sighting.
    ModelInfo {
        agent_id: AgentId,
        /// `Some` = a model observation (e.g. `"claude-fable-5"`).
        model: Option<String>,
        /// `Some` = an effort observation (Codex `"xhigh"` verbatim; CC's
        /// synthesized `"ultra"`/`"ultrathink"` marker labels).
        effort: Option<String>,
    },
    /// A FRESH-token spend observation from a source's wire (the #632 token
    /// meter): CC assistant lines carry `message.usage` per API call (fresh =
    /// `input_tokens + cache_creation_input_tokens + output_tokens` — cache
    /// READS are re-served context, not new spend), Codex `token_count`
    /// carries a per-turn `last_token_usage`. Each event is that reading's
    /// DELTA; the reducer accumulates onto an EXISTING slot only (unknown id
    /// = no-op — usage never registers a session) and stamps the delta +
    /// arrival time the scene's falling-sheet window reads. JSONL-only (no
    /// hook carries usage), so it never enters hook-wins dedup. Zero-delta
    /// readings are skipped at decode. Interpretation (tier thresholds, the
    /// sheet-fall window) lives in `pixtuoid-scene::token_meter` — the
    /// RAW-store/interpret-at-paint posture `ModelInfo` documents.
    Usage {
        agent_id: AgentId,
        /// Fresh tokens this reading: new input + cache writes + output.
        fresh_tokens: u64,
    },
}

/// A cached agent pid PLUS the kernel start marker read when it was stamped
/// ([`pid_start_marker`]) — together they name ONE process incarnation, so a
/// focus click can refuse a RECYCLED pid (#527): re-read the marker, and a
/// mismatch (or a dead pid) means this is not the process the hook came from.
/// `started: None` = no marker was readable at stamp time (non-unix daemon,
/// EPERM); the click-time guard then skips the identity check (additive, the
/// #220 posture) — so on no-exit-watch platforms a markerless cache retains
/// the pre-#527 recycled-pid residual until the stale sweep (compound-rare;
/// documented, not guarded). Field name `pid` everywhere keeps the
/// `pid: None` construction sites stable across the `Option<i32>` →
/// `Option<PidIdentity>` migration. `non_exhaustive` so a future identity
/// component (a boot id, a Windows session id) is a non-breaking add —
/// cross-crate construction goes through [`PidIdentity::new`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct PidIdentity {
    /// The agent CLI's OS pid (`pid_t`; matches `DaemonPresence.current_pid`).
    pub pid: i32,
    /// Opaque per-OS start marker — equality-only, see [`pid_start_marker`].
    pub started: Option<u64>,
}

impl PidIdentity {
    pub fn new(pid: i32, started: Option<u64>) -> Self {
        Self { pid, started }
    }
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
            AgentEvent::ModelInfo { agent_id, .. } => *agent_id,
            AgentEvent::Usage { agent_id, .. } => *agent_id,
        }
    }
}

/// Focus-jump pid point-queries for the transcript family — the ONE public
/// seam the binary's `focus` module consumes (probe internals stay
/// `pub(crate)`). Point queries against the live registries, never a
/// transcript scan; both ride the recycle-guarded probes (#220), the reason
/// transcript-family pids are NEVER taken from the shim parent. Native/unix
/// backed — a non-unix build resolves nothing (focus silently no-ops).
#[cfg(feature = "native")]
pub fn cc_pid_for_session(projects_root: &std::path::Path, session_id: &str) -> Option<i32> {
    let sessions_dir = cc_probe::cc_sessions_dir(projects_root)?;
    cc_probe::live_cc_session_ids(&sessions_dir)?
        .pid_of
        .get(session_id)
        .copied()
}

/// The CC sessions-registry dir the pid queries consult — the SAME
/// standard-layout gate the probe applies (a `--projects-root /tmp/fixture`
/// replay yields `None`). Exposed so `doctor` can report the focus channel's
/// on-disk state without re-deriving the sibling layout (#526).
#[cfg(feature = "native")]
pub fn cc_registry_dir(projects_root: &std::path::Path) -> Option<std::path::PathBuf> {
    cc_probe::cc_sessions_dir(projects_root)
}

/// Codex twin of [`cc_pid_for_session`], keyed by the rollout UUID (the
/// slot's `session_id`) — NOT the rollout path, which comes back
/// kernel-canonicalized from the fd probe and is deliberately not matched on.
#[cfg(feature = "native")]
pub fn codex_pid_for_session(sessions_root: &std::path::Path, uuid: &str) -> Option<i32> {
    codex::live_codex_rollout_ids(sessions_root)?
        .pid_of
        .get(uuid)
        .copied()
}

/// grok twin, keyed by the session id (== the transcript's parent-dir name)
/// against grok's own `active_sessions.json` registry under `grok_root`
/// (= `grok_home()`, the registry file's parent) — recycle-guarded by the
/// `opened_at` identity check inside the probe.
#[cfg(feature = "native")]
pub fn grok_pid_for_session(grok_root: &std::path::Path, session_id: &str) -> Option<i32> {
    grok::live_grok_session_ids(grok_root)?
        .pid_of
        .get(session_id)
        .copied()
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
pub mod grok;
pub mod hermes;
#[cfg(feature = "native")]
pub mod hook;
#[cfg(feature = "native")]
pub mod jsonl;
pub mod kimi;
#[cfg(feature = "native")]
pub mod manager;
// The async transport seam (tagged channel + `Source`/`DynSource`); the
// re-export keeps the pre-split `source::{Source, TaggedSender, …}` paths.
#[cfg(feature = "native")]
mod native;
#[cfg(feature = "native")]
pub use native::{DynSource, Source, TaggedReceiver, TaggedSender};
pub mod omp;
pub mod openclaw;
pub mod opencode;
// The kernel start-marker read shared by the hook stamp and the binary's
// click-time recycle guard (#527); the fn is the pub seam, the platform
// impls stay private.
#[cfg(feature = "native")]
mod proc_start;
#[cfg(feature = "native")]
pub use proc_start::pid_start_marker;
pub mod reasonix;
// `doc(hidden)`: the registry is an internal fact table, `pub` ONLY so the
// integration-test crates (sources::conformance) can read it. Hiding it keeps it
// off the published API — cargo-semver-checks then lets descriptor/caps
// fields evolve (the most likely change when adding a CLI) without a
// breaking-version bump. Same treatment as `jsonl`'s test-only seam.
#[doc(hidden)]
pub mod registry;

#[cfg(all(test, unix, feature = "native"))]
mod focus_pid_tests {
    // The two focus-jump point-query seams, over real tempdir registries.
    use super::*;

    #[test]
    fn cc_pid_for_session_hits_misses_and_tolerates_garbage() {
        // The seam takes the PROJECTS root and derives the sibling sessions
        // registry (the standard <claude_home>/{projects,sessions} layout).
        let home = tempfile::tempdir().unwrap();
        let projects = home.path().join("projects");
        let sessions = home.path().join("sessions");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::create_dir_all(&sessions).unwrap();
        // A live entry (our own pid is alive by construction) + garbage.
        std::fs::write(
            sessions.join("self.json"),
            serde_json::json!({
                "pid": std::process::id(),
                "sessionId": "focus-sess",
                "status": "idle"
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(sessions.join("junk.json"), "not json {{{").unwrap();

        assert_eq!(
            cc_pid_for_session(&projects, "focus-sess"),
            Some(std::process::id() as i32),
            "hit: the session's live registry pid"
        );
        assert_eq!(
            cc_pid_for_session(&projects, "unknown-sess"),
            None,
            "miss: unknown session resolves nothing"
        );
        // A NON-standard projects root (file_name != "projects") derives no
        // registry — the custom --projects-root replay case resolves nothing.
        assert_eq!(cc_pid_for_session(home.path(), "focus-sess"), None);
    }

    #[test]
    fn codex_pid_for_session_misses_on_unknown_uuid() {
        // No codex processes hold fds under a fresh tempdir → an empty (but
        // healthy) snapshot → any uuid misses. The hit side rides the probe's
        // own fd-matching tests (codex/native.rs).
        let root = tempfile::tempdir().unwrap();
        assert_eq!(codex_pid_for_session(root.path(), "0000-none"), None);
    }
}
