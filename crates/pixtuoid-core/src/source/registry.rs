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

use crate::source::jsonl::LineDecoder;
use crate::source::{
    antigravity, claude_code, codewhale, codex, copilot, opencode, reasonix, AgentEvent,
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

/// A source's own hook-payload decoder, dispatched ahead of the shared arms.
/// `Ok(Some(events))` short-circuits (the decoded sequence for this payload —
/// usually one event, two when an [`AgentEvent::Identity`] is attached ahead
/// of an activity event, #221); `Ok(None)` means "not my event" and falls
/// through to the shared arms; `Err` propagates.
pub type HookCustomDecoder = fn(&Value) -> Result<Option<Vec<AgentEvent>>>;

/// Per-source hook decoding behaviour beyond the shared CC-shaped arms.
pub struct HookDecoding {
    pub id_key: IdKey,
    /// Tried FIRST, immediately after `_pixtuoid_source` attribution and
    /// BEFORE any shared field requirement (`hook_event_name`, `session_id`)
    /// — so a source with a completely alien envelope (no `session_id` at
    /// all) can still decode. The fn knows its own `SOURCE_NAME` — no source
    /// parameter needed. CONTRACT: a custom fn that claims an event name must
    /// claim it FULLY — return `Err` on a malformed instance of its own
    /// event, never `Ok(None)` — or the payload silently falls through and
    /// decodes under the shared session-keyed semantics (divergent AgentId
    /// instead of an error). An ALIEN-envelope source (payloads without
    /// `hook_event_name`/`session_id` at all) must claim EVERY event — its
    /// simplest correct shape is `decode_x(v).map(Some)`, never `Ok(None)` —
    /// since the shared arms can only mis-serve it.
    pub custom: Option<HookCustomDecoder>,
}

/// Reducer-facing capability flags — stable facts about the source's wire
/// protocol, NOT policy names, so a future CLI picks values truthfully and
/// the policy falls out.
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
    /// JSONL line decoder. `None` = a HOOK-ONLY source (no watchable
    /// transcript): the fixture harness then accepts a transcript-less,
    /// hook-payloads-only scenario for it — and ONLY for it.
    pub line_decoder: Option<LineDecoder>,
    pub hook: HookDecoding,
    pub caps: SourceCaps,
}

pub const REGISTRY: &[SourceDescriptor] = &[
    CLAUDE_CODE,
    CODEX,
    ANTIGRAVITY,
    REASONIX,
    CODEWHALE,
    OPENCODE,
    COPILOT,
];

/// Linear scan — at most a handful of entries, called on slot creation and
/// the per-tick sweep; a map would cost more in ceremony than it saves.
pub fn descriptor_for(name: &str) -> Option<&'static SourceDescriptor> {
    REGISTRY.iter().find(|d| d.name == name)
}

const CLAUDE_CODE: SourceDescriptor = SourceDescriptor {
    name: claude_code::SOURCE_NAME,
    label_prefix: "cc",
    line_decoder: Some(claude_code::decode_cc_line),
    hook: HookDecoding {
        // CC keys on the session UUID (== the transcript filename stem
        // `cc_id_from_path` derives), NOT the cwd-derived transcript path, so a
        // subagent→parent link survives a git-worktree cwd-split. Mirrors Codex.
        id_key: IdKey::SessionId,
        // SubagentStart/Stop change the event's SUBJECT (child AgentId ≠
        // session AgentId) — inexpressible in the shared arms. The Stop is the
        // ONLY end signal a Workflow-fleet subagent gets (#241).
        custom: Some(claude_code::decode_cc_hook_custom),
    },
    caps: SourceCaps {
        has_exit_signal: true,
        // CC has no UserPromptSubmit-class resurrect path (its JSONL
        // SessionStart is first-sight-only, so a swept slot would NOT walk
        // back in) — but the flag is moot: with a real exit signal the short
        // reaper never applies (see short_idle_reap).
        resurrects_on_prompt: false,
        delegations_are_hook_silent: false,
    },
};

const CODEX: SourceDescriptor = SourceDescriptor {
    name: codex::SOURCE_NAME,
    label_prefix: "cx",
    line_decoder: Some(codex::decode_codex_line),
    hook: HookDecoding {
        id_key: IdKey::SessionId,
        // SubagentStart/Stop change the event's SUBJECT (child AgentId ≠
        // session AgentId) — inexpressible in the shared arms.
        custom: Some(codex::decode_codex_hook_custom),
    },
    caps: SourceCaps {
        has_exit_signal: false,
        resurrects_on_prompt: true,
        delegations_are_hook_silent: false,
    },
};

const ANTIGRAVITY: SourceDescriptor = SourceDescriptor {
    name: antigravity::SOURCE_NAME,
    label_prefix: "ag",
    line_decoder: Some(antigravity::decode_ag_line),
    hook: HookDecoding {
        id_key: IdKey::TranscriptPathThenSessionId,
        custom: None,
    },
    caps: SourceCaps {
        has_exit_signal: false,
        resurrects_on_prompt: false,
        delegations_are_hook_silent: false,
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
    line_decoder: None,
    hook: HookDecoding {
        id_key: IdKey::TranscriptPathThenSessionId, // inert: custom claims all
        custom: Some(reasonix::decode_rx_hook_custom),
    },
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
};

/// HOOK-ONLY: CodeWhale has NO tailable transcript (`rollout_path` is an unused
/// `state.db` column; saved sessions are full-snapshot rewrites; headless
/// `codewhale exec` runs hooks-off — only the TUI fires hooks). Its hook
/// envelope is ALIEN (snake_case `event` discriminator, identity via
/// `DEEPSEEK_*` env vars the shim folds into `cwd`/`tool`/`tool_args`), so the
/// custom decoder claims every event and the shared id-key branch is never
/// reached. Keyed on cwd because `session_id` is inconsistent across events
/// (live-verified 2026-06-12) — see `source/codewhale.rs`.
const CODEWHALE: SourceDescriptor = SourceDescriptor {
    name: codewhale::SOURCE_NAME,
    label_prefix: "cw",
    line_decoder: None,
    hook: HookDecoding {
        id_key: IdKey::TranscriptPathThenSessionId, // inert: custom claims all
        custom: Some(codewhale::decode_cw_hook_custom),
    },
    caps: SourceCaps {
        // session_end fires on a clean TUI quit carrying DEEPSEEK_WORKSPACE
        // (verified live 2026-06-12) — best-effort counts.
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
};

const OPENCODE: SourceDescriptor = SourceDescriptor {
    name: opencode::SOURCE_NAME,
    label_prefix: "oc",
    line_decoder: None,
    hook: HookDecoding {
        id_key: IdKey::TranscriptPathThenSessionId, // inert: custom claims all
        custom: Some(opencode::decode_oc_hook_custom),
    },
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
    line_decoder: Some(copilot::decode_copilot_line),
    hook: HookDecoding {
        id_key: IdKey::TranscriptPathThenSessionId, // inert: no hook transport for this source
        custom: None,
    },
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
};

#[cfg(test)]
mod tests {
    use super::*;

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
        // Hand-enumerated above — the len pin turns "forgot the new row's
        // assert" from a silent gap into a loud failure.
        assert_eq!(REGISTRY.len(), 7, "new row? add its name-pin assert above");
    }

    #[test]
    fn descriptor_for_resolves_known_and_rejects_unknown() {
        assert_eq!(descriptor_for("codex").unwrap().label_prefix, "cx");
        assert!(descriptor_for("not-a-source").is_none());
    }

    // The short-idle policy must fire for Codex ONLY: it is the one source
    // that both lacks an exit signal AND self-heals on the next prompt.
    // Antigravity lacks the signal but cannot resurrect — sweeping it short
    // would make a live-but-idle ag session vanish permanently.
    #[test]
    fn short_idle_reap_fires_for_codex_only() {
        for d in REGISTRY {
            assert_eq!(
                d.caps.short_idle_reap(),
                d.name == codex::SOURCE_NAME,
                "short_idle_reap mismatch for {:?}",
                d.name
            );
        }
    }
}
