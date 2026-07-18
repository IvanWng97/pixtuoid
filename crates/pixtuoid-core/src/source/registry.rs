//! The per-source fact table — ONE row per agent CLI for every cross-source
//! registry core needs (label prefix, JSONL decoder, hook keying, reducer
//! capability flags). Before this existed those facts were scattered across
//! `reducer::source_label_prefix`, `decoder::decode_hook_payload`'s id-key
//! branch, and `conformance::decoder_for` — each individually
//! test-enforced, but adding a CLI meant restating "this source exists" in
//! 5+ files. Now it's this table + the source's own module.
//!
//! Deliberately NOT in the table (the scatter there is load-bearing):
//! - The `Source` trait impls and their JsonlWatcher wiring (label derivers,
//!   session-end checkers, id derivers): each source's `run()` uses its own
//!   module's fns directly — that's per-source code in a per-source file,
//!   not a cross-source registry, and mirroring it here would only add
//!   dead-data drift risk.
//! - `_pixtuoid_source` attribution + the shared CC-shaped hook arms: they
//!   stay in `decoder.rs` at the read site, pinned by their regression tests.
//! - The binary crate's `install::Target` registry (this table's design
//!   precedent) and `runtime/driver.rs` source spawning.

use anyhow::Result;
use serde_json::Value;

use crate::source::decoder::{extract_top_level_cwd, CwdExtractor, LineDecoder};
use crate::source::{
    antigravity, claude_code, codewhale, codex, copilot, cursor, grok, hermes, omp, openclaw,
    opencode, reasonix, AgentEvent,
};

/// How the shared hook decoder derives the AgentId for this source. Moot for
/// an alien-envelope source whose `custom` decoder claims every event (the
/// shared id-key branch is then never reached) — pick
/// `TranscriptPathThenSessionId` with a `// inert` comment and let the custom
/// fn construct its own AgentIds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IdKey {
    /// `transcript_path` when present and non-empty, else `session_id`.
    /// Correct for Antigravity and the unknown-source default (path-keyed
    /// sources whose hook and JSONL both carry the transcript path, so they
    /// coalesce on it). NOT CC — see `SessionId`.
    TranscriptPathThenSessionId,
    /// Always `session_id`, ignoring any `transcript_path`. Correct for CC and
    /// Codex: their hook `session_id` IS the session UUID, which equals the
    /// transcript filename stem the JSONL watcher derives (`cc_id_from_path` /
    /// `codex_id_from_path`), so hook and JSONL events coalesce on it. CC's
    /// transcript path is cwd-derived (different project dirs after a
    /// git-worktree split → keying on the path rebuilds the wrong parent);
    /// Codex's `transcript_path` is `string | null` — keying on the path would
    /// split hook and JSONL events into two sprites for either.
    SessionId,
}

/// A source's own hook-payload decoder, dispatched ahead of the shared
/// CC-shaped arms — TYPED by whether it may decline. The two-variant split
/// makes the old "an alien-envelope source must NEVER return `Ok(None)`"
/// caller-contract unrepresentable: a [`Self::ClaimsAll`] fn has no `Option`
/// to get wrong.
#[derive(Clone, Copy)]
pub enum HookCustom {
    /// EXTENDS the shared arms: tried first; `Ok(Some(events))` short-circuits
    /// (the decoded sequence — usually one event, two when an
    /// [`AgentEvent::Identity`] is attached ahead of an activity event, #221),
    /// `Ok(None)` DECLINES and falls through to the shared arms, `Err`
    /// propagates. CC/Codex: they decode only `SubagentStart`/`SubagentStop`
    /// (which change the event's SUBJECT to the child) and defer everything
    /// else to the shared session-keyed arms.
    Extend(fn(&Value) -> Result<Option<Vec<AgentEvent>>>),
    /// CLAIMS every event: an alien-envelope source (no shared
    /// `hook_event_name`/`session_id` for the shared arms to key on, so they'd
    /// only mis-serve it) whose decoder handles EVERYTHING and constructs its
    /// own AgentIds. Returns `Result<Vec>` — structurally it can NOT decline,
    /// so the payload can never silently fall through to the shared arms.
    /// reasonix/codewhale/opencode/cursor/hermes/grok.
    ClaimsAll(fn(&Value) -> Result<Vec<AgentEvent>>),
}

/// Per-source hook decoding behaviour beyond the shared CC-shaped arms.
pub struct HookDecoding {
    /// The per-session AgentId key strategy. Read by the shared arms only —
    /// MOOT for a [`HookCustom::ClaimsAll`] source (its decoder builds its own
    /// AgentIds and the shared branch is never reached), pick
    /// `TranscriptPathThenSessionId` with an `// inert` comment there.
    pub id_key: IdKey,
    /// The source's own decoder, dispatched FIRST (immediately after
    /// `_pixtuoid_source` attribution, BEFORE any shared field requirement) —
    /// so an alien envelope (no `session_id` at all) can still decode. The fn
    /// knows its own `SOURCE_NAME` (no source parameter). `None` = ride the
    /// shared arms only (CC/Codex before #241 did). See [`HookCustom`] for the
    /// Extend-vs-ClaimsAll contract.
    pub custom: Option<HookCustom>,
}

/// Reducer-facing capability flags — stable facts about the source's wire
/// protocol, NOT policy names, so a future CLI picks values truthfully and
/// the policy falls out. `Copy` (three bools) so `SourceDescriptor::caps()`
/// can hand back a value (a `Daemon` row has no stored caps to borrow).
#[derive(Clone, Copy)]
pub struct SourceCaps {
    /// Does a CLEAN exit leave any end signal at all (a SessionEnd hook
    /// and/or a JSONL end marker — best-effort counts; "none of any kind" is
    /// the bar for `false`)? When false, the stale-sweep is the ONLY reaper a
    /// closed session ever gets. CC: true (the best-effort SessionEnd hook —
    /// no durable transcript marker exists; content-based /exit detection was
    /// removed because chat content must never drive lifecycle). Codex: false
    /// (no SessionEnd hook, no PID, ShutdownComplete unpersisted — all
    /// verified upstream). Antigravity: false (its session-end checker is
    /// always-false; no hook transport).
    pub has_exit_signal: bool,
    /// Does a live-but-swept session WALK BACK IN on the user's next prompt
    /// (a `UserPromptSubmit`-class event re-emitting `SessionStart`)? This is
    /// the safety precondition for the short idle reaper: its only false
    /// positive (a live session idle past the window) must self-heal. Codex:
    /// true. Antigravity: false — its JSONL watcher emits the synthetic
    /// SessionStart once per first-sight path, so a swept session never
    /// returns; that is WHY it keeps the long idle window despite having no
    /// exit signal.
    pub resurrects_on_prompt: bool,
    /// Are subagent delegations invisible on this source's event stream
    /// (in-process subagents that fire no hooks)? When true, a Delegating
    /// slot's `last_event_at` freezes for the whole delegation, so the
    /// reducer gives it the Waiting-class stale window instead of sweeping a
    /// long delegation mid-turn. False for every JSONL/CC-class source: CC's
    /// subagent hooks (misattributed to the parent) drive `refresh_lineage`.
    pub delegations_are_hook_silent: bool,
}

impl SourceCaps {
    /// All-false caps for a `Daemon` source: it creates no `AgentSlot`s, so the
    /// AgentSlot-reaping caps never apply — `short_idle_reap()` is false.
    pub const INERT_DAEMON: SourceCaps = SourceCaps {
        has_exit_signal: false,
        resurrects_on_prompt: false,
        delegations_are_hook_silent: false,
    };

    /// The short-idle-reaper policy, derived: only safe when the sweep is the
    /// sole reaper (`!has_exit_signal`) AND the false positive self-heals
    /// (`resurrects_on_prompt`). See `reducer::STALE_SHORT_IDLE_TIMEOUT`'s
    /// rationale — this encodes that argument as data.
    pub fn short_idle_reap(&self) -> bool {
        !self.has_exit_signal && self.resurrects_on_prompt
    }
}

/// One agent CLI's cross-source facts. `const` data with fn pointers — the
/// same pattern as the binary's `install::target::Target` registry.
pub struct SourceDescriptor {
    /// Stable lowercase id — MUST equal the module's `SOURCE_NAME` (pinned by
    /// `descriptor_names_match_module_source_name_consts`).
    pub name: &'static str,
    /// Exactly 2 chars (pinned by `every_descriptor_has_two_char_label_prefix`);
    /// applied at `SessionStart` and reinforced idempotently by the JSONL
    /// label derivers.
    pub label_prefix: &'static str,
    /// The CLI version this build's decoder + fixtures were last verified against
    /// (a byte-real capture — the #294 pattern). `"unknown"` (NOT `""`, pinned by
    /// `every_descriptor_has_a_verified_version`) where we have no fixed anchor;
    /// `pixtuoid doctor` only flags version SKEW when this parses to a version.
    /// Maintainers bump it when they re-capture against a newer CLI.
    pub verified_version: &'static str,
    /// argv to probe the installed CLI version, e.g. `&["claude", "--version"]`.
    /// `None` = no probe (no stable CLI binary). `doctor` runs it best-effort; a
    /// missing binary / parse failure degrades to "version: unknown".
    pub version_probe: Option<&'static [&'static str]>,
    /// What KIND of source this is — the typed discriminator that replaced a
    /// `presence_only: bool` over `Option`-soup fields. Consumers read through
    /// the accessors (`line_decoder()`/`hook()`/`caps()`/`is_daemon()`/
    /// `presence_decoder()`) so the enum shape stays an internal detail.
    pub kind: SourceKind,
}

/// The transcript half of an `Agent` row. A watchable transcript needs BOTH a
/// line decoder AND a first-sight cwd extractor — bundling them makes the
/// Some↔Some pairing structural (the pre-bundle paired `Option`s' iff
/// invariant was only test-pinned): a row is either transcript-bearing (both
/// fns) or hook-only (`transcript: None`), and a half-populated row is
/// unrepresentable.
pub struct Transcript {
    /// JSONL line decoder.
    pub line_decoder: LineDecoder,
    /// First-sight cwd extractor for the walker's transcript head scan. The fn
    /// lives in the source's own module (invariant #3: per-source
    /// transcript-format knowledge stays per-source); the shared
    /// `decoder::extract_top_level_cwd` covers the top-level `cwd` shape (CC;
    /// Antigravity, whose lines carry no cwd at all). The walker dispatches by
    /// the SCANNED source, so one source's shape is never tried against
    /// another's transcript.
    pub cwd_extractor: CwdExtractor,
}

/// How focus-jump resolves this source's OS pid — a DATA-only capability
/// (this const table compiles to wasm via `default-features = false`, so a
/// native-only probe FN POINTER can never live here; the transcript probes
/// stay in the BINARY's `focus::resolve_pid`, pinned to this enum by its
/// lockstep test). ONE source of truth for the three consumers that used to
/// hand-agree: the hook stamp gate (`patch_identity_pids`), the click-time
/// probe dispatch (`focus::resolve_pid`), and the doctor report bucketing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusChannel {
    /// The shim stamps its `getppid` into `_pid` — trustworthy because
    /// `sh -c` EXECs the hook so the parent IS the CLI; absent on Windows
    /// (`cmd /C` transient parent, the documented `parent_pid` trap).
    ShimStamp,
    /// The source's own runtime stamps `_pid` (opencode's `process.pid`) —
    /// cross-platform, survives even where the shim sends nothing.
    PluginStamp,
    /// Pid resolves through a recycle-guarded liveness probe at CLICK time;
    /// hook stamps are never trusted (the shim's getppid is the hook-command
    /// parent, not the CLI). The probe fn itself lives in the binary.
    TranscriptProbe,
    /// No pid channel — a focus click silently no-ops (the ONE failure rule).
    Unsupported,
}

impl FocusChannel {
    /// Whether a hook-envelope `_pid` stamp is a trustworthy pid for this
    /// source — the `patch_identity_pids` gate.
    pub fn accepts_stamp(self) -> bool {
        matches!(self, FocusChannel::ShimStamp | FocusChannel::PluginStamp)
    }
}

/// The two source classes, type-isolated. Adding a daemon (a 2nd one is "one
/// `Daemon` row + one binary mascot arm + one badge arm") needs no `handle_conn`
/// edit and no new reducer arm — the registry-driven demux + the
/// `daemon_sources()` sweep loop dispatch on this.
pub enum SourceKind {
    /// Produces `AgentEvent`s → `SceneState::agents` → a desk sprite.
    Agent {
        /// `None` = a HOOK-ONLY agent (no watchable transcript): the fixture
        /// harness then accepts a transcript-less, hook-payloads-only
        /// scenario for it — and ONLY for it.
        transcript: Option<Transcript>,
        /// `None` = a TRANSCRIPT-ONLY agent with no hook transport (copilot,
        /// omp — no install target, the shim never fires for them). Symmetric
        /// with `transcript: None`: absent capability is TYPED, not an inert
        /// stub. A non-daemon row must carry a transcript OR a hook (both `None`
        /// would decode nothing — pinned by `agent_has_a_decode_path`).
        hook: Option<HookDecoding>,
        caps: SourceCaps,
        /// The focus-jump pid channel. Lives INSIDE `Agent` so a daemon can't
        /// carry one (structurally unrepresentable — a mascot isn't clickable).
        focus: FocusChannel,
    },
    /// Produces `DaemonPresenceUpdate`s → `SceneState::daemons` → a wandering
    /// mascot (the OpenClaw gateway is instance #1). Its `presence_decoder` maps
    /// the daemon's wire envelope to presence deltas; it emits ZERO `AgentEvent`s
    /// (the `HookRouter` demux routes its payloads to the sibling channel, and
    /// `decode_hook_payload` short-circuits `is_daemon()` → empty).
    Daemon { presence_decoder: PresenceDecoder },
}

impl SourceDescriptor {
    /// A `Daemon`-kind row renders a mascot, not a desk sprite — the conformance
    /// harness treats its empty `AgentEvent` output as by-design.
    pub fn is_daemon(&self) -> bool {
        matches!(self.kind, SourceKind::Daemon { .. })
    }

    /// The transcript bundle (`None` for a hook-only agent AND every daemon —
    /// neither has a transcript for the walker to watch/head-scan).
    fn transcript(&self) -> Option<&Transcript> {
        match &self.kind {
            SourceKind::Agent { transcript, .. } => transcript.as_ref(),
            SourceKind::Daemon { .. } => None,
        }
    }

    /// The JSONL line decoder (`None` for a hook-only agent AND every daemon).
    pub fn line_decoder(&self) -> Option<LineDecoder> {
        self.transcript().map(|t| t.line_decoder)
    }

    /// The first-sight cwd extractor (`None` for a hook-only agent AND every
    /// daemon — neither has a transcript for the walker to head-scan).
    pub fn cwd_extractor(&self) -> Option<CwdExtractor> {
        self.transcript().map(|t| t.cwd_extractor)
    }

    /// The focus-jump pid channel (`Unsupported` for a daemon — a mascot has
    /// no terminal to jump to).
    pub fn focus_channel(&self) -> FocusChannel {
        match &self.kind {
            SourceKind::Agent { focus, .. } => *focus,
            SourceKind::Daemon { .. } => FocusChannel::Unsupported,
        }
    }

    /// The hook-decoding spec (`None` for a daemon — its payloads never reach
    /// the shared agent arms — AND for a transcript-only agent with no hook
    /// transport, e.g. copilot/omp). `decode_hook_payload` already defaults a
    /// missing spec (id_key → `TranscriptPathThenSessionId`, no custom arm).
    pub fn hook(&self) -> Option<&HookDecoding> {
        match &self.kind {
            SourceKind::Agent { hook, .. } => hook.as_ref(),
            SourceKind::Daemon { .. } => None,
        }
    }

    /// Reducer capability flags — an INERT all-false default for a daemon (it
    /// creates no `AgentSlot`s, so `short_idle_reap()` is false).
    pub fn caps(&self) -> SourceCaps {
        match &self.kind {
            SourceKind::Agent { caps, .. } => *caps,
            SourceKind::Daemon { .. } => SourceCaps::INERT_DAEMON,
        }
    }

    /// The daemon's presence decoder (`None` for an agent source).
    pub fn presence_decoder(&self) -> Option<PresenceDecoder> {
        match &self.kind {
            SourceKind::Daemon { presence_decoder } => Some(*presence_decoder),
            SourceKind::Agent { .. } => None,
        }
    }
}

pub const REGISTRY: &[SourceDescriptor] = &[
    CLAUDE_CODE,
    CODEX,
    ANTIGRAVITY,
    REASONIX,
    CODEWHALE,
    OPENCODE,
    COPILOT,
    CURSOR,
    HERMES,
    OMP,
    OPENCLAW,
    GROK,
];

/// Linear scan — at most a handful of entries, called on slot creation and
/// the per-tick sweep; a map would cost more in ceremony than it saves.
pub fn descriptor_for(name: &str) -> Option<&'static SourceDescriptor> {
    REGISTRY.iter().find(|d| d.name == name)
}

/// The first-sight cwd extractor for the source being scanned. Registry-driven
/// (invariant #3): the JSONL walker calls this with the source it is scanning,
/// so one source's head shape is never tried against another source's
/// transcript (the pre-registry shared if-chain gave a foreign-shaped line —
/// e.g. a codex-style `payload.cwd` inside a CC transcript — a cross-source
/// false-positive window on the identity-bearing cwd). Falls back to the
/// shared top-level `cwd` shape for a source with no row or no extractor
/// (test-harness source names).
pub fn cwd_extractor_for(source: &str) -> CwdExtractor {
    descriptor_for(source)
        .and_then(|d| d.cwd_extractor())
        .unwrap_or(extract_top_level_cwd)
}

/// A daemon source's wire decoder: its envelope → presence deltas. The pointer
/// type the registry hands the `HookRouter` demux so each daemon routes to its
/// OWN decoder (a 2nd daemon needs no `handle_conn` edit).
pub type PresenceDecoder = fn(&Value) -> Result<Vec<crate::source::daemon::DaemonPresenceUpdate>>;

/// The presence decoder for a daemon source, or `None` for an agent source.
/// Registry-DRIVEN — the demux never names a source, so a 2nd daemon needs no
/// `handle_conn` edit.
pub fn presence_decoder_for(source: &str) -> Option<PresenceDecoder> {
    descriptor_for(source).and_then(|d| d.presence_decoder())
}

/// Every registered daemon source paired with its presence-decay profile — the
/// reducer iterates this for the per-daemon presence sweep + disconnect
/// reconcile, so each daemon decays on its own TTL with no hardcoded name.
pub fn daemon_sources() -> impl Iterator<Item = (&'static str, crate::source::daemon::PresenceTtl)>
{
    REGISTRY
        .iter()
        .filter(|d| d.is_daemon())
        .map(|d| (d.name, crate::source::daemon::PresenceTtl::DEFAULT))
}

const CLAUDE_CODE: SourceDescriptor = SourceDescriptor {
    name: claude_code::SOURCE_NAME,
    label_prefix: "cc",
    verified_version: "unknown",
    version_probe: Some(&["claude", "--version"]),
    kind: SourceKind::Agent {
        transcript: Some(Transcript {
            line_decoder: claude_code::decode_cc_line,
            // CC writes a top-level `cwd` on transcript lines — the shared shape.
            cwd_extractor: extract_top_level_cwd,
        }),
        hook: Some(HookDecoding {
            // CC keys on the session UUID (== the transcript filename stem
            // `cc_id_from_path` derives), NOT the cwd-derived transcript path, so a
            // subagent→parent link survives a git-worktree cwd-split. Mirrors Codex.
            id_key: IdKey::SessionId,
            // SubagentStart/Stop change the event's SUBJECT (child AgentId ≠
            // session AgentId) — inexpressible in the shared arms. The Stop is the
            // ONLY end signal a Workflow-fleet subagent gets (#241).
            custom: Some(HookCustom::Extend(claude_code::decode_cc_hook_custom)),
        }),
        caps: SourceCaps {
            has_exit_signal: true,
            // CC has no UserPromptSubmit-class resurrect path (its JSONL
            // SessionStart is first-sight-only, so a swept slot would NOT walk
            // back in) — but the flag is moot: with a real exit signal the short
            // reaper never applies (see short_idle_reap).
            resurrects_on_prompt: false,
            delegations_are_hook_silent: false,
        },
        focus: FocusChannel::TranscriptProbe,
    },
};

const CODEX: SourceDescriptor = SourceDescriptor {
    name: codex::SOURCE_NAME,
    label_prefix: "cx",
    verified_version: "unknown",
    version_probe: Some(&["codex", "--version"]),
    kind: SourceKind::Agent {
        transcript: Some(Transcript {
            line_decoder: codex::decode_codex_line,
            // Rollouts carry cwd ONLY on the head session_meta line, under `payload`.
            cwd_extractor: codex::extract_codex_cwd,
        }),
        hook: Some(HookDecoding {
            id_key: IdKey::SessionId,
            // SubagentStart/Stop change the event's SUBJECT (child AgentId ≠
            // session AgentId) — inexpressible in the shared arms.
            custom: Some(HookCustom::Extend(codex::decode_codex_hook_custom)),
        }),
        caps: SourceCaps {
            has_exit_signal: false,
            resurrects_on_prompt: true,
            delegations_are_hook_silent: false,
        },
        focus: FocusChannel::TranscriptProbe,
    },
};

const ANTIGRAVITY: SourceDescriptor = SourceDescriptor {
    name: antigravity::SOURCE_NAME,
    label_prefix: "ag",
    verified_version: "unknown",
    version_probe: Some(&["agy", "--version"]),
    kind: SourceKind::Agent {
        transcript: Some(Transcript {
            line_decoder: antigravity::decode_ag_line,
            // AG step lines carry no cwd field at all — the shared top-level shape
            // never matches and the label falls back to the bare `ag` prefix,
            // exactly the pre-dispatch behavior.
            cwd_extractor: extract_top_level_cwd,
        }),
        // Keeps a real hook spec (NOT `None` like copilot/omp) even with no
        // install target: an antigravity hook payload decodes via the shared
        // arms on its REAL `TranscriptPathThenSessionId` key (the one the enum
        // doc calls "Correct for Antigravity"), a path the hook-decode tests
        // exercise. copilot/omp were `None`'d in #10 because they are
        // TS-extension-only with NO hook path — their spec was an inert stub;
        // antigravity's is not.
        hook: Some(HookDecoding {
            id_key: IdKey::TranscriptPathThenSessionId,
            custom: None,
        }),
        caps: SourceCaps {
            has_exit_signal: false,
            resurrects_on_prompt: false,
            delegations_are_hook_silent: false,
        },
        focus: FocusChannel::Unsupported,
    },
};

/// HOOK-ONLY: Reasonix v2 session files are full-rewritten per turn
/// (untailable — no `Source` impl, no runtime wiring) and its hook envelope is
/// ALIEN (camelCase, `event` discriminator, `cwd` as the only identity), so
/// the custom decoder claims every event and the shared id-key branch is
/// never reached.
const REASONIX: SourceDescriptor = SourceDescriptor {
    name: reasonix::SOURCE_NAME,
    label_prefix: "rx",
    verified_version: "unknown",
    version_probe: Some(&["reasonix", "--version"]),
    kind: SourceKind::Agent {
        transcript: None, // hook-only: no transcript for the walker to watch
        hook: Some(HookDecoding {
            id_key: IdKey::TranscriptPathThenSessionId, // inert: custom claims all
            custom: Some(HookCustom::ClaimsAll(reasonix::decode_rx_hook_payload)),
        }),
        caps: SourceCaps {
            // SessionEnd hook fires on clean exit (verified upstream @v1.2.0,
            // internal/hook/hook.go run sites) — best-effort counts.
            has_exit_signal: true,
            // UserPromptSubmit re-emits SessionStart (the decode maps it so) —
            // a swept-but-live session walks back in on the next prompt.
            resurrects_on_prompt: true,
            // Subagents run in-process with hooks disabled upstream
            // (internal/agent/task.go) — a Delegating slot emits NOTHING until
            // the dispatch tool's PostToolUse, so it gets the Waiting-class
            // stale window (see `stale_threshold_with_caps`).
            delegations_are_hook_silent: true,
        },
        focus: FocusChannel::ShimStamp,
    },
};

/// HOOK-ONLY: CodeWhale has NO tailable transcript (`rollout_path` is an unused
/// `state.db` column; saved sessions are full-snapshot rewrites; headless
/// `codewhale exec` runs hooks-off — only the TUI fires hooks). Its hook
/// envelope is ALIEN (snake_case `event` discriminator, identity via
/// `DEEPSEEK_*` env vars the shim folds into `cwd`/`tool`/`tool_args`), so the
/// custom decoder claims every event and the shared id-key branch is never
/// reached. Keyed on cwd because `session_id` is inconsistent across events
/// — see `source/codewhale.rs`.
const CODEWHALE: SourceDescriptor = SourceDescriptor {
    name: codewhale::SOURCE_NAME,
    label_prefix: "cw",
    verified_version: "unknown",
    version_probe: Some(&["codewhale", "--version"]),
    kind: SourceKind::Agent {
        transcript: None, // hook-only: no transcript for the walker to watch
        hook: Some(HookDecoding {
            id_key: IdKey::TranscriptPathThenSessionId, // inert: custom claims all
            custom: Some(HookCustom::ClaimsAll(codewhale::decode_cw_hook_payload)),
        }),
        caps: SourceCaps {
            // session_end fires on a clean TUI quit carrying DEEPSEEK_WORKSPACE
            // — best-effort counts.
            has_exit_signal: true,
            // message_submit re-emits SessionStart (the decode maps it so) —
            // a swept-but-live session walks back in on the next prompt.
            resurrects_on_prompt: true,
            // Conservative ASSUMPTION (the live exec_shell capture did not exercise
            // a dispatch): if the dispatch tool (`agent_spawn`) blocks hook-silently
            // until its tool_call_after, a Delegating slot must get the Waiting-class
            // stale window rather than be swept mid-delegation. `true` is the safe
            // default — it can only over-retain a dead Delegating slot, never reap a
            // live one; matches Reasonix. (Individual sub-agents ALSO get their own
            // child sprites via the subagent_spawn/complete observer hooks —
            // `codewhale::decode_cw_subagent`.)
            delegations_are_hook_silent: true,
        },
        focus: FocusChannel::ShimStamp,
    },
};

const OPENCODE: SourceDescriptor = SourceDescriptor {
    name: opencode::SOURCE_NAME,
    label_prefix: "oc",
    verified_version: "unknown",
    version_probe: Some(&["opencode", "--version"]),
    kind: SourceKind::Agent {
        transcript: None, // hook-only: no transcript for the walker to watch
        hook: Some(HookDecoding {
            id_key: IdKey::TranscriptPathThenSessionId, // inert: custom claims all
            custom: Some(HookCustom::ClaimsAll(opencode::decode_oc_hook_payload)),
        }),
        caps: SourceCaps {
            // A clean per-session close fires `session.deleted` → SessionEnd, and an
            // abrupt exit / TUI quit kills the opencode process → `hook::HookPidWatch`
            // ends every bound sprite (the plugin stamps `_pid`). So there IS an exit
            // signal — no Codex-style short-idle carve-out.
            has_exit_signal: true,
            // opencode sessions are persistent SQLite rows; a follow-up prompt
            // continues the SAME session and emits NO new `session.created`. So a
            // stale-swept session does NOT walk back in on the next prompt (unlike
            // CodeWhale/Reasonix/Codex) — combined with `has_exit_signal` this keeps
            // the normal long idle timeout, not the short-idle reaper.
            resurrects_on_prompt: false,
            // The `task` dispatch tool emits BOTH a `running` and a `completed`/`error`
            // tool part (→ ActivityStart + ActivityEnd), so a delegation is NOT
            // hook-silent; liveness also flows UP from the child session (its own
            // sprite, parent-linked via `info.parentID`). No Waiting-class retention
            // needed.
            delegations_are_hook_silent: false,
        },
        focus: FocusChannel::PluginStamp,
    },
};

/// The DAEMON row: OpenClaw is one always-on gateway DAEMON, not a per-session
/// coding agent. Its backend `claude-cli` sessions are already shown by `cc·`,
/// so OpenClaw renders ONE presence-gated wandering lobster mascot. The
/// `presence_decoder` maps its alien `{type:…}` envelope to presence deltas on
/// the sibling channel; it emits ZERO `AgentEvent`s (the `HookRouter` demux
/// routes its payloads via this decoder, and `decode_hook_payload` short-circuits
/// `is_daemon()` → empty). `line_decoder()`/`hook()`/`caps()` are inert by
/// construction (a daemon creates no `AgentSlot`s — `short_idle_reap()` is false).
const OPENCLAW: SourceDescriptor = SourceDescriptor {
    name: openclaw::SOURCE_NAME,
    label_prefix: "ok",
    // Byte-real capture anchor: `openclaw 2026.6.6`.
    verified_version: "2026.6.6",
    version_probe: Some(&["openclaw", "--version"]),
    kind: SourceKind::Daemon {
        presence_decoder: openclaw::decode_openclaw_hook_payload,
    },
};

/// GitHub Copilot CLI (`@github/copilot`). TRANSCRIPT-ONLY (Antigravity/Codex-class):
/// the whole lifecycle is persisted to `<copilot_home>/session-state/<id>/events.jsonl`
/// (permission + sub-agent events included — richer than Codex), so it needs NO hook
/// install target and NO custom hook decoder. Sub-agents interleave in the root file,
/// keyed on the envelope `agentId`; the session id is the parent-dir UUID
/// (`copilot::copilot_id_from_path`).
const COPILOT: SourceDescriptor = SourceDescriptor {
    name: copilot::SOURCE_NAME,
    label_prefix: "cp",
    verified_version: "1.0.62",
    version_probe: Some(&["copilot", "--version"]),
    kind: SourceKind::Agent {
        transcript: Some(Transcript {
            line_decoder: copilot::decode_copilot_line,
            // `session.start` nests the cwd at `data.context.cwd`.
            cwd_extractor: copilot::extract_copilot_cwd,
        }),
        // No hook transport: copilot is transcript-only (no install target, the
        // shim never fires for it), so absent-capability is TYPED as `None`,
        // symmetric with a hook-only source's `transcript: None` — not a stub.
        hook: None,
        caps: SourceCaps {
            // `session.shutdown` is a real persisted exit marker → no short-idle reaper.
            has_exit_signal: true,
            // Sessions are stable + resumable (sessionId constant across --resume); a
            // stale-swept session does not silently walk back in on the next prompt.
            resurrects_on_prompt: false,
            // The `task` dispatch emits a `tool.execution_start`/`complete` pair AND explicit
            // `subagent.started`/`completed` events, so a delegation is not hook-silent.
            delegations_are_hook_silent: false,
        },
        focus: FocusChannel::Unsupported,
    },
};

/// HOOK-ONLY: Cursor CLI (`cursor-agent`) has no passively-observable transcript
/// — its `--output-format stream-json` NDJSON is per-invocation stdout (pixtuoid
/// never spawns the agent) and its on-disk sessions are SQLite, not a tailable
/// JSONL. The reachable seam is Cursor Hooks (`~/.cursor/hooks.json`). The hook
/// envelope reuses CC's `hook_event_name` field NAME but with camelCase values,
/// so the custom decoder claims every event and keys on `session_id`
/// (capture-verified present + consistent; `workspace_roots[0]` is the fallback
/// label/cwd — `source/cursor.rs`). Subagents render FLAT, not nested: a `Task`
/// dispatch makes the parent Delegating, but children run as independent
/// sessions with no parent-link in the stream (a proven upstream absence;
/// drift-watched).
const CURSOR: SourceDescriptor = SourceDescriptor {
    name: cursor::SOURCE_NAME,
    label_prefix: "cu",
    verified_version: "unknown",
    version_probe: Some(&["cursor-agent", "--version"]),
    kind: SourceKind::Agent {
        transcript: None, // hook-only: no transcript for the walker to watch
        hook: Some(HookDecoding {
            id_key: IdKey::TranscriptPathThenSessionId, // inert: custom claims all
            custom: Some(HookCustom::ClaimsAll(cursor::decode_cursor_hook_payload)),
        }),
        caps: SourceCaps {
            // `sessionEnd` FIRES on clean completion (`reason:"completed"`) —
            // best-effort counts, CC/Reasonix class. Abrupt
            // exits (no PID exposed) fall to the generic stale-sweep.
            has_exit_signal: true,
            // Each `cursor-agent` invocation is a NEW session_id, so a stale-swept
            // session does NOT walk back in on a later prompt — but moot: with an
            // exit signal the short reaper never applies (short_idle_reap == false).
            resurrects_on_prompt: false,
            // Cursor's `Task` dispatch (capture-verified) makes the parent Delegating,
            // and it gets NO `postToolUse` for the Task — the parent is hook-silent
            // through the delegation (the children run as separate, unlinkable
            // sessions), so the Delegating slot needs the Waiting-class stale window
            // rather than a mid-delegation sweep (matches Reasonix/CodeWhale; safe —
            // it can only over-retain a dead Delegating slot, the parent's own
            // `sessionEnd` reaps it cleanly in the normal case).
            delegations_are_hook_silent: true,
        },
        focus: FocusChannel::ShimStamp,
    },
};

/// HOOK-ONLY: Hermes Agent (Nous Research) has no passively-observable transcript
/// — its output is per-invocation and its sessions aren't a tailable JSONL. The
/// reachable seam is Hermes shell hooks (`~/.hermes/config.yaml`, or
/// `$HERMES_HOME/config.yaml`). The hook envelope reuses CC's `hook_event_name`
/// field NAME but with snake_case values, so the custom decoder claims every
/// event and keys on `session_id` (present + consistent; a real `cwd` is the
/// label — `source/hermes.rs`). A user may run several instances, so session_id
/// keeps concurrent sessions distinct rather than merging them by workspace.
const HERMES: SourceDescriptor = SourceDescriptor {
    name: hermes::SOURCE_NAME,
    label_prefix: "hm",
    verified_version: "0.18.0", // captured banner: `Hermes Agent v0.18.0 (2026.7.1)`
    version_probe: Some(&["hermes", "--version"]),
    kind: SourceKind::Agent {
        transcript: None, // hook-only: no transcript for the walker to watch
        hook: Some(HookDecoding {
            id_key: IdKey::TranscriptPathThenSessionId, // inert: custom claims all
            custom: Some(HookCustom::ClaimsAll(hermes::decode_hermes_hook_payload)),
        }),
        caps: SourceCaps {
            // `on_session_end` FIRES on clean completion (best-effort counts,
            // CC/Cursor class). Abrupt exits (no PID in the payload) fall to the
            // generic stale-sweep.
            has_exit_signal: true,
            // A stale-swept session does not walk back in on a later prompt — but
            // moot with an exit signal (short_idle_reap == false).
            resurrects_on_prompt: false,
            // No subagent nesting on the wire — sessions render flat, so there is
            // no hook-silent delegation window to hold.
            delegations_are_hook_silent: false,
        },
        focus: FocusChannel::ShimStamp,
    },
};

/// Grok Build (`grok`, xai-org/grok-build). TRANSCRIPT-BEARING **with** a hook
/// target — the CC/Codex class, and the FIRST row combining a transcript with
/// a claims-all custom hook decoder (grok's envelope is camelCase-keyed
/// `hookEventName`, alien to the shared arms). Coalescing rests on
/// `sessionId == the transcript's parent-dir name` (upstream joins the id into
/// `session_dir`), mirrored by `grok_id_from_path`. Transcript =
/// `{grok_home}/sessions/<enc-cwd>/<session-id>/updates.jsonl`, append-only;
/// content carries NO cwd (path-derived via the watcher's cwd deriver — see
/// `extract_grok_cwd`). Wire facts repo-derived @ 0.1.220-alpha.4 c68e39f6,
/// then BYTE-REAL anchored @ 0.2.102 (ab5ebf69acec, live capture 2026-07-16,
/// #637): every envelope field, both method namespaces, the
/// toolUseId==toolCallId join, and the subagent_end finish spelling held
/// across the version jump. 0.2.x deltas absorbed: a NEW `events.jsonl`
/// sibling (the updates.jsonl path filter already excludes it), children now
/// fire their own `session_end` (a harmless earlier exit — the ledger still
/// rides the parent's `subagent_end`), the `turn_completed` xAI tag is now
/// decoded (→ turn-end; the variant predates the sync, we hadn't mapped it),
/// and the transcript `rawInput` carries CLIENT-form
/// keys (`background`) — the both-keys `spawn_is_blocking` read covers it.
const GROK: SourceDescriptor = SourceDescriptor {
    name: grok::SOURCE_NAME,
    label_prefix: "gk",
    // Byte-real capture anchor: `grok 0.2.102 (ab5ebf69acec) [stable]`.
    verified_version: "0.2.102",
    version_probe: Some(&["grok", "--version"]),
    kind: SourceKind::Agent {
        transcript: Some(Transcript {
            line_decoder: grok::decode_grok_line,
            // Always-None: grok lines carry no cwd — the PATH carries it
            // (URL-encoded group dir), applied via the watcher's cwd deriver.
            cwd_extractor: grok::extract_grok_cwd,
        }),
        hook: Some(HookDecoding {
            id_key: IdKey::SessionId, // inert: custom claims all
            custom: Some(HookCustom::ClaimsAll(grok::decode_grok_hook_payload)),
        }),
        caps: SourceCaps {
            // grok's `session_end` hook does NOT fire on a plain TUI quit (the
            // event loop breaks without draining the session actor — verified
            // vs run_loop/dispatch @ c68e39f6), i.e. the DOMINANT exit path is
            // signal-less; the hook_execution transcript marker is equally
            // best-effort. The reliable exit authority is the liveness ladder
            // over grok's own `active_sessions.json` (native half), which
            // synthesizes SessionEnd — a probe, not a wire signal.
            has_exit_signal: false,
            // Every prompt fires `user_prompt_submit` (decoded → SessionStart,
            // the Codex-class carrier), so a stale-swept LIVE session walks
            // back in on its next prompt. Together with !has_exit_signal this
            // opts grok into the short idle reap — correct for its untracked
            // one-shot headless sessions, while live TUI sessions are
            // vouch-exempt via the probe.
            resurrects_on_prompt: true,
            // A BLOCKING spawn's Task detail gets its post_tool_use at
            // completion (background spawns never mint Task — the b1 note in
            // `grok_tool_detail`), so delegations are not hook-silent.
            delegations_are_hook_silent: false,
        },
        // First-party pid registry (`active_sessions.json`: session_id → pid,
        // recycle-guarded by `opened_at`) — the probe answers focus clicks;
        // a shim stamp would shadow it with an unguarded getppid (#527).
        focus: FocusChannel::TranscriptProbe,
    },
};

/// Oh My Pi (`omp`, omp.sh). TRANSCRIPT-ONLY (Copilot/Antigravity-class): the
/// whole lifecycle is persisted to
/// `<omp_agent_dir>/sessions/<encoded-cwd>/<ts>_<uuid>.jsonl` (assistant
/// toolCall blocks + toolResult messages + the `session_exit` custom entry),
/// and omp has NO shell-hook seam (its hooks are in-process TS extensions), so
/// it needs NO install target and NO custom hook decoder. Subagents persist as
/// SEPARATE nested files (`<parent-stem>/<taskId>.jsonl`) — parent-linked by
/// path (`omp::omp_id_from_path` keys the stem chain).
const OMP: SourceDescriptor = SourceDescriptor {
    name: omp::SOURCE_NAME,
    label_prefix: "om",
    // Byte-real capture anchor: `omp/16.4.0` (`omp --version`);
    // the omp fixtures are sanitized captures from that install.
    verified_version: "16.4.0",
    version_probe: Some(&["omp", "--version"]),
    kind: SourceKind::Agent {
        transcript: Some(Transcript {
            line_decoder: omp::decode_omp_line,
            // The `type:"session"` header carries a top-level `cwd` — the
            // shared shape.
            cwd_extractor: extract_top_level_cwd,
        }),
        // No hook transport: omp is transcript-only (no install target, only
        // in-process TS extensions), so absent-capability is TYPED as `None`,
        // symmetric with a hook-only source's `transcript: None` — not a stub.
        hook: None,
        caps: SourceCaps {
            // The `session_exit` custom entry is appended + flushed on every
            // clean teardown incl. SIGINT/SIGTERM (best-effort counts;
            // SIGKILL falls to the stale-sweep) → no short-idle reaper.
            has_exit_signal: true,
            // The header `SessionStart` decodes once per transcript life (and
            // first-sight is once per path), so a stale-swept session does not
            // walk back in on the next prompt — moot anyway with a real exit
            // signal (short_idle_reap == false).
            resurrects_on_prompt: false,
            // The `task` dispatch emits a toolCall/toolResult pair on the
            // parent AND the child persists its own parent-linked transcript
            // (liveness flows up), so a delegation is not hook-silent.
            delegations_are_hook_silent: false,
        },
        // Transcript-only: no shim to stamp a pid, and no click-time
        // transcript probe wired in the binary (the #518 fd probe binds
        // pid_of for LIVENESS; promoting it to a focus point-query is a
        // separate seam). Matches Copilot/Antigravity.
        focus: FocusChannel::Unsupported,
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    /// `daemon_sources()` drives the per-daemon presence sweep + disconnect
    /// reconcile — an empty iterator silently freezes every mascot's decay.
    /// Pin that it yields exactly the registry's Daemon rows (openclaw
    /// today), each with the default decay profile.
    #[test]
    fn daemon_sources_yields_every_daemon_row() {
        let daemons: Vec<&'static str> = daemon_sources()
            .map(|(name, ttl)| {
                assert_eq!(
                    ttl.presence_ttl_ms,
                    crate::source::daemon::PresenceTtl::DEFAULT.presence_ttl_ms,
                    "{name} must carry the default decay profile"
                );
                name
            })
            .collect();
        assert_eq!(daemons, vec![crate::source::openclaw::SOURCE_NAME]);
    }

    // Registry-local shape check. The reducer KEEPS its own end-to-end
    // `every_registered_source_has_two_char_label_prefix` (through the real
    // `source_label_prefix`, lookup included) — this one exists so a bad row
    // fails HERE with a row-shaped message, not three modules away.
    #[test]
    fn every_descriptor_has_two_char_label_prefix() {
        for d in REGISTRY {
            assert_eq!(
                d.label_prefix.chars().count(),
                2,
                "source {:?} label_prefix {:?} must be exactly 2 chars",
                d.name,
                d.label_prefix
            );
        }
    }

    // `verified_version` must be non-empty — `"unknown"` is the sentinel, NOT
    // `""` (an empty string would parse as no-version AND read as a blank column;
    // `pixtuoid doctor` relies on the distinction). A new row must make a
    // conscious choice rather than defaulting to "".
    #[test]
    fn every_descriptor_has_a_verified_version() {
        for d in REGISTRY {
            assert!(
                !d.verified_version.is_empty(),
                "source {:?} verified_version is empty — use \"unknown\", not \"\"",
                d.name
            );
        }
    }

    // Every label_prefix must be globally UNIQUE: the per-row length check above
    // catches a malformed prefix, but two rows sharing the same 2-char prefix
    // (e.g. two `cc`s) would render two distinct CLIs as indistinguishable
    // sprites with no compile or test error. Pin uniqueness across the table.
    #[test]
    fn all_label_prefixes_are_unique() {
        use std::collections::HashSet;
        let set: HashSet<&str> = REGISTRY.iter().map(|d| d.label_prefix).collect();
        assert_eq!(
            set.len(),
            REGISTRY.len(),
            "duplicate label_prefix across sources — two CLIs would share one sprite prefix"
        );
    }

    // Guards literal-drift: `name` is initialized FROM the module const (so a
    // rename is already a compile error at the init site); this catches the
    // init being replaced with a string literal that later drifts.
    #[test]
    fn descriptor_names_match_module_source_name_consts() {
        assert_eq!(CLAUDE_CODE.name, claude_code::SOURCE_NAME);
        assert_eq!(CODEX.name, codex::SOURCE_NAME);
        assert_eq!(ANTIGRAVITY.name, antigravity::SOURCE_NAME);
        assert_eq!(REASONIX.name, reasonix::SOURCE_NAME);
        assert_eq!(CODEWHALE.name, codewhale::SOURCE_NAME);
        assert_eq!(OPENCODE.name, opencode::SOURCE_NAME);
        assert_eq!(COPILOT.name, copilot::SOURCE_NAME);
        assert_eq!(CURSOR.name, cursor::SOURCE_NAME);
        assert_eq!(HERMES.name, hermes::SOURCE_NAME);
        assert_eq!(OMP.name, omp::SOURCE_NAME);
        assert_eq!(OPENCLAW.name, openclaw::SOURCE_NAME);
        assert_eq!(GROK.name, grok::SOURCE_NAME);
        // Hand-enumerated above — the len pin turns "forgot the new row's
        // assert" from a silent gap into a loud failure.
        assert_eq!(REGISTRY.len(), 12, "new row? add its name-pin assert above");
    }

    #[test]
    fn descriptor_for_resolves_known_and_rejects_unknown() {
        assert_eq!(descriptor_for("codex").unwrap().label_prefix, "cx");
        assert!(descriptor_for("not-a-source").is_none());
    }

    // The walker's head scan needs a cwd extractor for EXACTLY the
    // transcript-bearing sources — since the `Transcript` bundle this is
    // structural (`cwd_extractor_present_iff_transcript_bearing` retired: a
    // row can no longer carry one fn without the other).

    // Dispatch resolution: each source's extractor reads ITS OWN shape and
    // ignores the others' — the walker never tries a foreign shape against a
    // transcript. Unregistered (harness) sources fall back to the shared
    // top-level shape.
    #[test]
    fn cwd_extractor_for_dispatches_per_source_shapes() {
        use std::path::PathBuf;
        let top = serde_json::json!({ "cwd": "/repo/top" });
        let codex_shaped =
            serde_json::json!({ "type": "session_meta", "payload": { "cwd": "/repo/cx" } });
        let copilot_shaped = serde_json::json!({ "type": "session.start", "data": { "context": { "cwd": "/repo/cp" } } });

        let cc = cwd_extractor_for(claude_code::SOURCE_NAME);
        assert_eq!(cc(&top), Some(PathBuf::from("/repo/top")));
        assert_eq!(cc(&codex_shaped), None, "CC must ignore the codex shape");
        assert_eq!(
            cc(&copilot_shaped),
            None,
            "CC must ignore the copilot shape"
        );

        let cx = cwd_extractor_for(codex::SOURCE_NAME);
        assert_eq!(cx(&codex_shaped), Some(PathBuf::from("/repo/cx")));
        assert_eq!(cx(&top), None, "codex must ignore a top-level cwd");

        let cp = cwd_extractor_for(copilot::SOURCE_NAME);
        assert_eq!(cp(&copilot_shaped), Some(PathBuf::from("/repo/cp")));
        assert_eq!(
            cp(&codex_shaped),
            None,
            "copilot must ignore the codex shape"
        );

        let unknown = cwd_extractor_for("not-a-source");
        assert_eq!(
            unknown(&top),
            Some(PathBuf::from("/repo/top")),
            "unregistered sources keep the shared top-level default"
        );
    }

    // The short-idle policy must fire for EXACTLY the sources that both lack
    // an exit signal AND self-heal on the next prompt: Codex (the original
    // carve-out) and grok (session_end doesn't fire on TUI quit; every prompt's
    // `user_prompt_submit` re-registers — and grok's probe vouch additionally
    // exempts live TUI sessions, so the reap effectively owns only the
    // untracked headless one-shots and probe blind spots). Antigravity lacks
    // the signal but cannot resurrect — sweeping it short would make a
    // live-but-idle ag session vanish permanently.
    #[test]
    fn short_idle_reap_fires_for_codex_and_grok_only() {
        for d in REGISTRY {
            assert_eq!(
                d.caps().short_idle_reap(),
                d.name == codex::SOURCE_NAME || d.name == grok::SOURCE_NAME,
                "short_idle_reap mismatch for {:?}",
                d.name
            );
        }
    }

    // OpenClaw is the ONLY daemon today. The Agent/Daemon partition must be exact:
    // a 2nd daemon updates this list (and gets a mascot arm + badge arm). Pins the
    // `SourceKind` discriminant so an Agent row never silently becomes daemon-shaped.
    #[test]
    fn openclaw_is_the_only_daemon() {
        let daemons: Vec<&str> = REGISTRY
            .iter()
            .filter(|d| d.is_daemon())
            .map(|d| d.name)
            .collect();
        assert_eq!(daemons, vec![openclaw::SOURCE_NAME]);
    }

    // A `Daemon` row's agent-facing accessors are INERT by construction: no JSONL
    // watcher, no hook spec (its payloads never reach the shared agent arms),
    // all-false caps (so `short_idle_reap()` is false), AND a presence decoder.
    #[test]
    fn daemon_accessors_are_inert() {
        let d = descriptor_for(openclaw::SOURCE_NAME).unwrap();
        assert!(d.is_daemon());
        assert!(d.line_decoder().is_none(), "a daemon has no JSONL watcher");
        assert!(
            d.hook().is_none(),
            "a daemon never reaches the shared agent arms"
        );
        assert!(
            !d.caps().short_idle_reap(),
            "INERT caps — a daemon reaps no AgentSlot"
        );
        assert!(
            d.presence_decoder().is_some(),
            "a daemon MUST carry a presence decoder"
        );
    }

    // The complement: every Agent has a DECODE PATH (a transcript, a hook, or
    // both) and NO presence decoder, so the registry-driven demux never routes
    // an agent payload to the presence channel (and a daemon's payload never
    // decodes as an agent — pinned above). Both-absent would be a wholly inert
    // row (decodes nothing) — now unrepresentable-by-review since `hook` went
    // `Option` alongside `transcript` (#10: absent capability is typed, not stubbed).
    #[test]
    fn agent_has_a_decode_path_and_no_presence_decoder() {
        for d in REGISTRY.iter().filter(|d| !d.is_daemon()) {
            assert!(
                d.line_decoder().is_some() || d.hook().is_some(),
                "agent {:?} has no decode path (neither a transcript nor a hook)",
                d.name
            );
            assert!(
                d.presence_decoder().is_none(),
                "agent {:?} must have no presence decoder",
                d.name
            );
        }
    }

    // A `Daemon` row can't ALSO be a transcript source: it carries a presence
    // decoder and NO line decoder (the demux + conformance harness rely on this).
    #[test]
    fn every_daemon_source_has_a_presence_decoder_and_no_line_decoder() {
        for d in REGISTRY.iter().filter(|d| d.is_daemon()) {
            assert!(
                d.presence_decoder().is_some(),
                "daemon {:?} needs a presence decoder",
                d.name
            );
            assert!(
                d.line_decoder().is_none(),
                "daemon {:?} must not also be a transcript source",
                d.name
            );
        }
    }
}
