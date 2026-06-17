//! Reasonix source — HOOK-ONLY (no JSONL watcher).
//!
//! Reasonix (github.com/esengine/DeepSeek-Reasonix, the Go line, releases
//! v1.0.0+) ships a CC-style hook system; connecting Reasonix in the in-TUI
//! Sources panel (`s`) registers the shim in the GLOBAL `~/.reasonix/settings.json`
//! (project-scope hooks are trust-gated and would silently not fire). Hook
//! payloads arrive on the shared hook socket stamped
//! `_pixtuoid_source: "reasonix"`; `decoder::decode_hook_payload` dispatches
//! them here because the envelope is Reasonix's own, NOT the CC shape:
//!
//! ```json
//! {"event":"PreToolUse","cwd":"/repo","toolName":"bash","toolArgs":{"command":"ls"}}
//! ```
//!
//! camelCase fields, `event` discriminator (not `hook_event_name`), and — the
//! load-bearing difference — **no session id, no transcript path, no tool-call
//! id anywhere** (verified against `internal/hook/hook.go` Payload @v1.2.0).
//! The only situating field is `cwd` (the workspace root), so the AgentId is
//! keyed on `cwd`. Consequences, all deliberate:
//!
//! - Two concurrent Reasonix sessions in ONE project render as one sprite
//!   (indistinguishable upstream; same accepted blur as CC's parent-keyed
//!   subagent hooks). The blur cuts both ways: one session's `SessionEnd`
//!   (or `/new` rotation) walks the shared sprite out even if the other
//!   session is still live — it walks back in on that session's next prompt
//!   (`UserPromptSubmit` → `SessionStart` is the resurrect path).
//! - `tool_use_id` is always `None`: the reducer's per-call machinery
//!   (hook-wins dedup, `active_tasks`) is bypassed — harmless, single
//!   transport and in-process subagents mean there is nothing to dedup or
//!   suppress. One real staleness interaction remains: a foreground
//!   delegation is hook-SILENT until its `PostToolUse`, so the reducer gives
//!   a Delegating rx slot the Waiting-class stale window (see
//!   `stale_threshold`) instead of sweeping a long `research`/`review` run
//!   mid-turn.
//! - A turn-end `Stop` decodes to `ActivityEnd { tool_use_id: None }`, which
//!   the reducer also treats as resolving a stale `Waiting` (an approval
//!   prompt BLOCKS the Reasonix turn, so Waiting-at-Stop can only be a denied
//!   prompt that already resolved). This holds for BOTH Waiting producers —
//!   `Notification` and `PermissionRequest` (#302) — they're the same gate.
//! - Exit profile is CC-class: clean exits and `/new` rotations fire
//!   `SessionEnd`; only SIGKILL/SIGTERM/terminal-close leave no signal and
//!   fall to the generic stale-sweep (30-min idle). No Codex-style short
//!   carve-out is needed — Codex got one because it has NO exit signal at
//!   all; Reasonix does.
//!
//! Why no JSONL transport: v2 session files are FULL-REWRITTEN (tmp+rename)
//! once per turn — zero mid-turn writes, so a tail watcher shows nothing while
//! the agent works and re-reads the whole file every turn; headless
//! `reasonix run` writes no session file at all; and with no shared key
//! between the file name and hook payloads, a JSONL agent could never coalesce
//! with the hook agent (guaranteed two-sprites). Hooks carry everything the
//! office needs, including `SessionEnd` (which even Codex lacks).

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use crate::source::decoder::{ellipsize, generic_tool_display, MAX_DECODED_FIELD_CHARS};
use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

pub const SOURCE_NAME: &str = "reasonix";

/// Reasonix tools that dispatch an in-process subagent (`internal/agent/task.go`
/// plus the skill wrappers in `internal/skill/tools.go` @v1.2.0). Mapped to
/// `ToolDetail::Task` so the slot reads "Delegating" while the (hook-invisible)
/// subagent works. `run_skill` is excluded: it is only sometimes a subagent and
/// the args don't say which.
const SUBAGENT_TOOLS: &[&str] = &["task", "explore", "research", "review", "security_review"];

/// Decode one Reasonix hook payload (already identified by
/// `_pixtuoid_source == "reasonix"`). Envelope verified against
/// `internal/hook/hook.go:227-240` @v1.2.0.
///
/// Event mapping:
/// - `SessionStart` / `UserPromptSubmit` → `SessionStart` (idempotent in the
///   reducer; `UserPromptSubmit` doubles as the resurrect path after a sweep)
/// - `PreToolUse`  → `Identity` + `ActivityStart` (dispatch family → `Task`)
/// - `PostToolUse` → `Identity` + `ActivityEnd`
/// - `Stop`        → `ActivityEnd` (turn end → idle debounce; NO Identity —
///   an end for an unknown agent proves nothing worth registering)
/// - `Notification`→ `Identity` + `Waiting` (a "needs attention" approval ping)
/// - `PermissionRequest` → `Identity` + `Waiting` (the STRUCTURED approval gate
///   — fires before the prompt with `toolName`+`subject`; fires alongside
///   `Notification` for the same approval, both→Waiting is idempotent)
/// - `SessionEnd`  → `SessionEnd`
/// - anything else → bail (registered-vs-decoded drift must be loud, not a
///   silent drop — same contract as the CC/Codex arms)
///
/// The activity arms prepend an [`AgentEvent::Identity`] (#221) because
/// Reasonix is HOOK-ONLY: a slot the reducer's proof-of-life pre-pass
/// synthesizes mid-turn has no JSONL back-fill path, so without the attached
/// identity it would stay a blank `#N` ghost until the next
/// `UserPromptSubmit`. The cwd is the whole identity (it IS the session key),
/// so `session_id` mirrors the `SessionStart` arm exactly — coalescing holds.
pub fn decode_rx_hook_payload(v: &Value) -> Result<Vec<AgentEvent>> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("reasonix hook payload must be an object"))?;
    let event = obj
        .get("event")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("reasonix payload missing event"))?;
    // `cwd` is the ONLY identity a Reasonix hook carries — an empty one would
    // mint a phantom agent that nothing else coalesces with (the same
    // empty-key-is-malformed idiom as the session_id / SubagentStart guards).
    let cwd = obj
        .get("cwd")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("reasonix payload missing/empty cwd"))?;
    let agent_id = AgentId::from_parts(SOURCE_NAME, cwd);

    let identity = || AgentEvent::Identity {
        agent_id,
        source: SOURCE_NAME.to_string(),
        // Mirrors the SessionStart arm: no upstream session id exists; the
        // cwd IS the session key.
        session_id: cwd.to_string(),
        cwd: Some(cwd.into()),
    };

    match event {
        // SessionStart fires lazily on the first turn; UserPromptSubmit fires
        // on every prompt. Both map to SessionStart: the reducer ignores it
        // when the slot exists, and the UserPromptSubmit duplicate is the
        // RESURRECT path — a stale-swept session walks back in on its next
        // prompt (Reasonix has no other re-creation signal; cf. the Codex arm).
        "SessionStart" | "UserPromptSubmit" => Ok(vec![AgentEvent::SessionStart {
            agent_id,
            source: SOURCE_NAME.to_string(),
            // No upstream session id exists; the cwd IS the session key.
            session_id: cwd.to_string(),
            cwd: cwd.into(),
            parent_id: None,
        }]),
        "PreToolUse" => {
            let tool = obj
                .get("toolName")
                .and_then(|s| s.as_str())
                .unwrap_or_else(|| {
                    crate::source::drift::missing_field(SOURCE_NAME, "PreToolUse", "toolName");
                    "?"
                });
            Ok(vec![
                identity(),
                AgentEvent::ActivityStart {
                    agent_id,
                    tool_use_id: None,
                    detail: Some(rx_tool_detail(tool, obj.get("toolArgs"))),
                },
            ])
        }
        "PostToolUse" => Ok(vec![
            identity(),
            AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: None,
            },
        ]),
        // Turn end — same decoded event as PostToolUse but deliberately
        // Identity-LESS: ends don't prove a session worth registering (the
        // shared Stop arm makes the same call), so an unknown id here rides
        // the reducer's blank-synthesis fallback.
        "Stop" => Ok(vec![AgentEvent::ActivityEnd {
            agent_id,
            tool_use_id: None,
        }]),
        "Notification" => {
            // Upstream's only Notification producer is the approval gate
            // (`controller.go:2065-2071`): "approval needed: <tool> <subject>".
            let msg = obj
                .get("message")
                .and_then(|s| s.as_str())
                .unwrap_or("waiting");
            Ok(vec![
                identity(),
                AgentEvent::Waiting {
                    agent_id,
                    // Capped at decode like CC's Notification arm (pitfall 3).
                    reason: ellipsize(msg, MAX_DECODED_FIELD_CHARS),
                },
            ])
        }
        // The STRUCTURED approval gate (`internal/hook/runner.go`:
        // `PermissionRequest(name, subject, args)`) — fires BEFORE the prompt is
        // shown, carrying the tool + subject, and fires ALONGSIDE `Notification`
        // for the same approval (`internal/control/controller.go`). Both →
        // Waiting is idempotent (the gated tool's PostToolUse/Stop resolves it,
        // exactly like Notification). Richer than Notification's free-form
        // `message`, so the reason is built from `toolName` + `subject`. NOT
        // live-capturable: the gate never fires in headless `reasonix run`
        // (auto-approve), the same reason Notification doesn't — decoded
        // generously from source (the capture would refine the firing set, not
        // the shape).
        "PermissionRequest" => {
            let tool = obj
                .get("toolName")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty());
            if tool.is_none() {
                crate::source::drift::missing_field(SOURCE_NAME, "PermissionRequest", "toolName");
            }
            let subject = obj
                .get("subject")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty());
            // Pre-cap each component before concatenating (matching
            // `rx_tool_detail` in this file) so an oversized `subject` can't
            // force a large intermediate allocation; the outer `ellipsize` on
            // the combined reason stays the authoritative cap.
            let reason = match (tool, subject) {
                (Some(t), Some(s)) => format!(
                    "approval: {} {}",
                    ellipsize(t, MAX_DECODED_FIELD_CHARS),
                    ellipsize(s, MAX_DECODED_FIELD_CHARS),
                ),
                (Some(t), None) => format!("approval: {}", ellipsize(t, MAX_DECODED_FIELD_CHARS)),
                (None, _) => "approval required".to_string(),
            };
            Ok(vec![
                identity(),
                AgentEvent::Waiting {
                    agent_id,
                    reason: ellipsize(&reason, MAX_DECODED_FIELD_CHARS),
                },
            ])
        }
        "SessionEnd" => Ok(vec![AgentEvent::SessionEnd {
            agent_id,
            as_child: false,
        }]),
        other => {
            crate::source::drift::unknown_event(SOURCE_NAME, other);
            bail!("unsupported reasonix hook event: {other}")
        }
    }
}

/// The registry's `hook.custom` entry point. Reasonix's envelope is ALIEN (no
/// `hook_event_name`/`session_id`), so per the `HookDecoding::custom` contract
/// it claims EVERY event reaching it — `.map(Some)`, never `Ok(None)` — and
/// the shared CC-shaped arms are unreachable for `_pixtuoid_source=reasonix`.
pub(crate) fn decode_rx_hook_custom(v: &Value) -> Result<Option<Vec<AgentEvent>>> {
    decode_rx_hook_payload(v).map(Some)
}

/// Reasonix-side tool detail: the dispatch family is name-keyed (Reasonix args
/// carry no `subagent_type`, so the shared semantic detection can't see it),
/// everything else gets a `"name: target"` display using Reasonix's own
/// argument vocabulary, looked up `command` > `path` > `pattern` > `url` —
/// the keys the v1.2.0 builtin tools actually emit (`path` NOT CC's
/// `file_path`: no builtin uses `file_path`; `url` is `web_fetch`). Close to,
/// but not identical to, upstream's permission `subjectKeys` (which carries a
/// defensive `file_path` for external tools and no `url`).
fn rx_tool_detail(tool: &str, args: Option<&Value>) -> ToolDetail {
    if SUBAGENT_TOOLS.contains(&tool) {
        return ToolDetail::Task;
    }
    let target = args.and_then(|a| {
        ["command", "path", "pattern", "url"]
            .iter()
            .find_map(|k| a.get(k).and_then(|v| v.as_str()))
    });
    // Per-source target keys above; the shared last-mile assembly (name +
    // `: target`, each capped at the decode boundary — pitfall 3) lives in
    // `generic_tool_display`.
    generic_tool_display(tool, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode_all(v: Value) -> Vec<AgentEvent> {
        decode_rx_hook_payload(&v).expect("decodes")
    }

    /// The payload's MAIN event — the lifecycle/activity event the arm maps
    /// to, i.e. the last decoded event (activity arms prepend an `Identity`).
    fn decode(v: Value) -> AgentEvent {
        decode_all(v).pop().expect("at least one event")
    }

    #[test]
    fn session_start_keys_on_cwd() {
        let ev = decode(json!({
            "event": "SessionStart",
            "cwd": "/Users/dev/zirs"
        }));
        match ev {
            AgentEvent::SessionStart {
                agent_id,
                source,
                cwd,
                parent_id,
                ..
            } => {
                assert_eq!(source, SOURCE_NAME);
                assert_eq!(
                    agent_id,
                    AgentId::from_parts(SOURCE_NAME, "/Users/dev/zirs")
                );
                assert_eq!(cwd, std::path::PathBuf::from("/Users/dev/zirs"));
                assert_eq!(parent_id, None);
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn user_prompt_submit_is_the_resurrect_session_start() {
        // After a stale-sweep there is no other re-creation signal; the next
        // prompt must walk the agent back in.
        let ev = decode(json!({
            "event": "UserPromptSubmit",
            "cwd": "/Users/dev/zirs",
            "prompt": "add a feature"
        }));
        assert!(matches!(ev, AgentEvent::SessionStart { agent_id, .. }
                if agent_id == AgentId::from_parts(SOURCE_NAME, "/Users/dev/zirs")));
    }

    #[test]
    fn pre_tool_use_is_activity_start_with_no_tool_id() {
        // Reasonix hook payloads carry NO tool-call id (verified @v1.2.0) —
        // tool_use_id must be None, not synthesized.
        let ev = decode(json!({
            "event": "PreToolUse",
            "cwd": "/repo",
            "toolName": "read_file",
            "toolArgs": {"path": "src/main.go"}
        }));
        match ev {
            AgentEvent::ActivityStart {
                tool_use_id,
                detail,
                ..
            } => {
                assert_eq!(tool_use_id, None);
                assert_eq!(detail.unwrap().display(), "read_file: src/main.go");
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn tool_target_uses_reasonix_arg_vocabulary() {
        // `path`, not CC's `file_path`; `command` wins the priority order.
        let bash = decode(json!({
            "event": "PreToolUse", "cwd": "/r",
            "toolName": "bash", "toolArgs": {"command": "go test ./..."}
        }));
        assert!(
            matches!(bash, AgentEvent::ActivityStart { detail: Some(d), .. }
            if d.display() == "bash: go test ./...")
        );
        // grep sends both `pattern` and `path` — `path` outranks `pattern`,
        // matching upstream's own subject-extractor order.
        let grep = decode(json!({
            "event": "PreToolUse", "cwd": "/r",
            "toolName": "grep", "toolArgs": {"pattern": "TODO", "path": "src"}
        }));
        assert!(
            matches!(grep, AgentEvent::ActivityStart { detail: Some(d), .. }
            if d.display() == "grep: src")
        );
    }

    #[test]
    fn long_targets_are_truncated() {
        let long = "x".repeat(60);
        let ev = decode(json!({
            "event": "PreToolUse", "cwd": "/r",
            "toolName": "bash", "toolArgs": {"command": long}
        }));
        match ev {
            AgentEvent::ActivityStart {
                detail: Some(d), ..
            } => {
                let display = d.display();
                assert!(display.starts_with("bash: "));
                assert!(display.ends_with('…'));
                assert_eq!(display.chars().count(), "bash: ".chars().count() + 41);
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn long_tool_name_is_truncated_at_the_decode_boundary() {
        // The tool NAME is wire content — capped at the FIELD width like
        // `make_tool_detail` (d01: it used to be emitted uncapped).
        let long = "T".repeat(MAX_DECODED_FIELD_CHARS * 3);
        let ev = decode(json!({
            "event": "PreToolUse", "cwd": "/r",
            "toolName": long, "toolArgs": {}
        }));
        match ev {
            AgentEvent::ActivityStart {
                detail: Some(d), ..
            } => {
                let display = d.display();
                assert!(
                    display.ends_with('…'),
                    "name should be ellipsized: {display}"
                );
                assert_eq!(display.chars().count(), MAX_DECODED_FIELD_CHARS + 1);
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn subagent_dispatch_family_maps_to_task() {
        // In-process subagents fire no hooks of their own; the parent's
        // dispatch tool is the only signal — show "Delegating".
        for tool in ["task", "explore", "research", "review", "security_review"] {
            let ev = decode(json!({
                "event": "PreToolUse", "cwd": "/r",
                "toolName": tool, "toolArgs": {"prompt": "do a thing"}
            }));
            assert!(
                matches!(&ev, AgentEvent::ActivityStart { detail: Some(d), .. } if d.is_task()),
                "{tool} must map to ToolDetail::Task"
            );
        }
        // run_skill is deliberately NOT in the family (only sometimes a subagent).
        let ev = decode(json!({
            "event": "PreToolUse", "cwd": "/r",
            "toolName": "run_skill", "toolArgs": {"name": "lint"}
        }));
        assert!(matches!(&ev, AgentEvent::ActivityStart { detail: Some(d), .. } if !d.is_task()));
    }

    #[test]
    fn post_tool_use_and_stop_are_activity_end() {
        for event in ["PostToolUse", "Stop"] {
            let ev = decode(json!({"event": event, "cwd": "/r"}));
            assert!(
                matches!(
                    &ev,
                    AgentEvent::ActivityEnd {
                        tool_use_id: None,
                        ..
                    }
                ),
                "{event} must decode to ActivityEnd with no tool id"
            );
        }
    }

    #[test]
    fn notification_maps_to_waiting_with_message() {
        let ev = decode(json!({
            "event": "Notification",
            "cwd": "/r",
            "message": "approval needed: bash rm -rf ./build"
        }));
        assert!(matches!(ev, AgentEvent::Waiting { reason, .. }
            if reason == "approval needed: bash rm -rf ./build"));
    }

    #[test]
    fn session_end_maps_to_session_end() {
        let ev = decode(json!({"event": "SessionEnd", "cwd": "/r"}));
        assert!(matches!(ev, AgentEvent::SessionEnd { .. }));
    }

    // #302: the structured approval gate → Waiting, reason built from the
    // tool + subject the hook carries (runner.go: PermissionRequest(name,
    // subject, args)).
    #[test]
    fn permission_request_maps_to_waiting_with_tool_and_subject() {
        let ev = decode(json!({
            "event": "PermissionRequest",
            "cwd": "/r",
            "toolName": "bash",
            "subject": "rm -rf ./build",
            "toolArgs": {"command": "rm -rf ./build"}
        }));
        assert!(matches!(ev, AgentEvent::Waiting { reason, .. }
            if reason == "approval: bash rm -rf ./build"));
    }

    #[test]
    fn permission_request_without_subject_uses_tool_only() {
        let ev = decode(json!({
            "event": "PermissionRequest", "cwd": "/r", "toolName": "write_file"
        }));
        assert!(matches!(ev, AgentEvent::Waiting { reason, .. }
            if reason == "approval: write_file"));
    }

    #[test]
    fn permission_request_without_tool_falls_back() {
        // Degenerate (toolName is always set upstream) — graceful fallback, and
        // a `missing_field` drift breadcrumb fires (verified by the count test
        // below, not asserted here without a subscriber).
        let ev = decode(json!({"event": "PermissionRequest", "cwd": "/r"}));
        assert!(matches!(ev, AgentEvent::Waiting { reason, .. }
            if reason == "approval required"));
    }

    #[test]
    fn permission_request_reason_is_capped_at_the_decode_boundary() {
        let long = "é".repeat(MAX_DECODED_FIELD_CHARS * 10);
        let ev = decode(json!({
            "event": "PermissionRequest", "cwd": "/r",
            "toolName": "bash", "subject": long
        }));
        match &ev {
            AgentEvent::Waiting { reason, .. } => {
                assert_eq!(reason.chars().count(), MAX_DECODED_FIELD_CHARS + 1);
                assert!(reason.ends_with('…'));
            }
            other => panic!("expected Waiting, got {other:?}"),
        }
    }

    #[test]
    fn all_events_for_one_cwd_share_one_agent_id() {
        // The coalescing contract, hook-only flavor: every event of a session
        // (the prepended Identity events INCLUDED) keys on the same
        // cwd-derived AgentId.
        let events = [
            json!({"event": "SessionStart", "cwd": "/Users/dev/p"}),
            json!({"event": "UserPromptSubmit", "cwd": "/Users/dev/p"}),
            json!({"event": "PreToolUse", "cwd": "/Users/dev/p", "toolName": "bash",
                   "toolArgs": {"command": "ls"}}),
            json!({"event": "PostToolUse", "cwd": "/Users/dev/p", "toolName": "bash"}),
            json!({"event": "PermissionRequest", "cwd": "/Users/dev/p", "toolName": "bash", "subject": "ls"}),
            json!({"event": "Notification", "cwd": "/Users/dev/p", "message": "approval needed: bash ls"}),
            json!({"event": "Stop", "cwd": "/Users/dev/p"}),
            json!({"event": "SessionEnd", "cwd": "/Users/dev/p"}),
        ];
        let ids: std::collections::BTreeSet<_> = events
            .iter()
            .flat_map(|v| decode_rx_hook_payload(v).unwrap())
            .map(|e| e.agent_id())
            .collect();
        assert_eq!(ids.len(), 1, "all events must coalesce to one AgentId");
    }

    // #221: Reasonix is hook-only, so a slot synthesized mid-turn has NO JSONL
    // back-fill path — the activity arms must attach the identity the payload
    // carries (cwd = source key = session key) ahead of the activity event,
    // mirroring the SessionStart arm exactly so coalescing holds.
    #[test]
    fn activity_arms_prepend_identity_with_cwd_keyed_session() {
        let payloads = [
            json!({"event": "PreToolUse", "cwd": "/Users/dev/p", "toolName": "bash",
                   "toolArgs": {"command": "ls"}}),
            json!({"event": "PostToolUse", "cwd": "/Users/dev/p", "toolName": "bash"}),
            json!({"event": "Notification", "cwd": "/Users/dev/p", "message": "approval needed"}),
            json!({"event": "PermissionRequest", "cwd": "/Users/dev/p", "toolName": "bash", "subject": "ls"}),
        ];
        for payload in payloads {
            let name = payload["event"].clone();
            let events = decode_all(payload);
            assert_eq!(events.len(), 2, "{name}: Identity + activity");
            match &events[0] {
                AgentEvent::Identity {
                    agent_id,
                    source,
                    session_id,
                    cwd,
                } => {
                    assert_eq!(*agent_id, AgentId::from_parts(SOURCE_NAME, "/Users/dev/p"));
                    assert_eq!(source, SOURCE_NAME);
                    assert_eq!(session_id, "/Users/dev/p", "cwd IS the session key");
                    assert_eq!(
                        cwd.as_deref(),
                        Some(std::path::Path::new("/Users/dev/p")),
                        "rx hooks always know their cwd"
                    );
                }
                other => panic!("{name}: expected leading Identity, got {other:?}"),
            }
        }
    }

    // Ends don't prove a session worth registering (same call as the shared
    // Stop arm) — and SessionStart/UserPromptSubmit already carry identity.
    #[test]
    fn stop_session_events_and_session_end_carry_no_identity() {
        for payload in [
            json!({"event": "Stop", "cwd": "/r"}),
            json!({"event": "SessionStart", "cwd": "/r"}),
            json!({"event": "UserPromptSubmit", "cwd": "/r"}),
            json!({"event": "SessionEnd", "cwd": "/r"}),
        ] {
            let name = payload["event"].clone();
            let events = decode_all(payload);
            assert_eq!(events.len(), 1, "{name}: exactly one event");
            assert!(
                !matches!(events[0], AgentEvent::Identity { .. }),
                "{name} must not emit Identity"
            );
        }
    }

    #[test]
    fn empty_or_missing_cwd_is_malformed() {
        // An empty cwd would mint a phantom agent nothing coalesces with —
        // reject, same idiom as the decoder's session_id guard.
        assert!(decode_rx_hook_payload(&json!({"event": "Stop", "cwd": ""})).is_err());
        assert!(decode_rx_hook_payload(&json!({"event": "Stop"})).is_err());
    }

    #[test]
    fn unknown_event_bails_loudly() {
        // Registered-vs-decoded drift must surface, not silently drop —
        // PostLLMCall/PreCompact/SubagentStop are deliberately unregistered.
        for ev in ["PostLLMCall", "PreCompact", "SubagentStop", "Bogus"] {
            assert!(
                decode_rx_hook_payload(&json!({"event": ev, "cwd": "/r"})).is_err(),
                "{ev} must bail (not registered, must not decode silently)"
            );
        }
    }

    #[test]
    fn non_object_payload_is_malformed() {
        assert!(decode_rx_hook_payload(&json!("just a string")).is_err());
        assert!(decode_rx_hook_payload(&json!(42)).is_err());
    }

    #[test]
    fn notification_without_message_falls_back_to_waiting() {
        let ev = decode(json!({"event": "Notification", "cwd": "/r"}));
        assert!(matches!(ev, AgentEvent::Waiting { reason, .. } if reason == "waiting"));
    }

    #[test]
    fn pre_tool_use_without_tool_name_displays_question_mark() {
        let ev = decode(json!({"event": "PreToolUse", "cwd": "/r"}));
        assert!(
            matches!(ev, AgentEvent::ActivityStart { detail: Some(d), .. }
            if d.display() == "?")
        );
    }

    #[test]
    fn tool_args_spoofed_subagent_type_does_not_make_task() {
        // toolArgs are model-authored (prompt-injectable). The shared
        // `make_tool_detail` keys Task on `subagent_type` PRESENCE — Reasonix
        // has no such field, so its detail fn must not consult it: a crafted
        // arg must not flip an ordinary tool into "Delegating" (which would
        // pollute the slot display).
        let ev = decode(json!({
            "event": "PreToolUse", "cwd": "/r",
            "toolName": "read_file",
            "toolArgs": {"path": "x.go", "subagent_type": null}
        }));
        assert!(
            matches!(&ev, AgentEvent::ActivityStart { detail: Some(d), .. } if !d.is_task()),
            "spoofed subagent_type must stay Generic"
        );
    }

    // conf-35 (#262 item 5): the in-diff sibling of CC's Notification cap
    // (pitfall 2) — Reasonix's `message` is the same content-derived field.
    #[test]
    fn notification_reason_is_capped_at_the_decode_boundary() {
        let long = "é".repeat(MAX_DECODED_FIELD_CHARS * 10);
        let ev = decode(json!({
            "event": "Notification",
            "cwd": "/r",
            "message": long
        }));
        match &ev {
            AgentEvent::Waiting { reason, .. } => {
                assert_eq!(reason.chars().count(), MAX_DECODED_FIELD_CHARS + 1);
                assert!(reason.ends_with('…'));
            }
            other => panic!("expected Waiting, got {other:?}"),
        }
        // The short-message pass-through is pinned by
        // notification_maps_to_waiting_with_message above.
    }
}
