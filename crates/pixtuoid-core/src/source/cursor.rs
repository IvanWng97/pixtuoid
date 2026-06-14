//! Cursor CLI source — HOOK-ONLY (no JSONL watcher).
//!
//! Cursor ships a standalone agent CLI (`cursor-agent`, installed via
//! `curl https://cursor.com/install`). Three candidate seams, only one
//! reachable by a *passive observer* like pixtuoid:
//!
//! - `--output-format stream-json` NDJSON is **stdout of an invocation we'd
//!   have to launch ourselves** — pixtuoid never spawns the user's agent, so it
//!   is structurally unreachable.
//! - On-disk sessions are **SQLite** (`~/.cursor/chats/.../store.db`,
//!   hex-encoded blobs) — not a tailable JSONL (the opencode situation).
//! - **Cursor Hooks** (`~/.cursor/hooks.json`) — shell commands fired on
//!   lifecycle/tool events, JSON on stdin, staff-confirmed firing in the
//!   standalone CLI. THIS is the seam: connecting Cursor in the Connection
//!   panel (`c`) registers the shim in the GLOBAL `~/.cursor/hooks.json`.
//!
//! Hook payloads arrive on the shared hook socket stamped
//! `_pixtuoid_source: "cursor"`; `decoder::decode_hook_payload` dispatches them
//! here (the custom decoder runs FIRST, before the shared CC-shaped field
//! requirements). Cursor's envelope reuses CC's `hook_event_name` field NAME
//! but with **camelCase values** and **no usable session id in CLI mode**:
//!
//! ```json
//! {"hook_event_name":"preToolUse","cwd":"/repo","tool_name":"shell",
//!  "tool_input":{"command":"ls"}}
//! ```
//!
//! Keyed on **cwd** (the CodeWhale/Reasonix precedent), NOT `conversation_id`:
//! the field is IDE-documented but UNVERIFIED in the CLI payload, while
//! `cwd`/`workspace_roots` is always present — so cwd-keying guarantees
//! coalescing. Consequences, all deliberate and matching Reasonix:
//!
//! - Two concurrent Cursor sessions in ONE project render as one sprite
//!   (indistinguishable on a cwd key; same accepted blur as Reasonix/CodeWhale).
//! - `tool_use_id` is always `None`: the reducer's per-call machinery
//!   (hook-wins dedup, `active_tasks`) is bypassed — harmless on a single
//!   transport with no rendered delegation.
//! - **Session-only.** Cursor's subagent linkage lives ONLY in
//!   `subagentStart`/`subagentStop` hooks (not firing in the CLI) and is absent
//!   from every other surface — a proven upstream absence, so no child sprites
//!   (tracked by the upstream-drift watch for if/when the CLI lands them).
//! - Exit profile: `sessionEnd` is NOT confirmed firing in the CLI and `stop`
//!   is turn-end, not session-end, and no PID is exposed — so a closed session
//!   falls to the generic stale-sweep (the Antigravity profile;
//!   `has_exit_signal: false`, `resurrects_on_prompt: false` → the LONG idle
//!   window, never the short-idle reaper).
//!
//! The exact firing set + `conversation_id` presence are pinned by a one-shot
//! live capture before this source flips to "supported"; the decoder is written
//! generously so the capture refines the firing set, not the architecture.

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use crate::source::decoder::{ellipsize, MAX_DECODED_FIELD_CHARS, MAX_TOOL_TARGET_CHARS};
use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

pub const SOURCE_NAME: &str = "cursor";

/// Decode one Cursor hook payload (already identified by
/// `_pixtuoid_source == "cursor"`). Envelope per `cursor.com/docs/hooks`.
///
/// Event mapping (camelCase `hook_event_name` values):
/// - `sessionStart`          → `SessionStart` (cwd-keyed)
/// - `preToolUse`            → `Identity` + `ActivityStart`
/// - `postToolUse`           → `Identity` + `ActivityEnd`
/// - `stop`                  → `ActivityEnd` (turn end → idle debounce; NO
///   Identity — an end for an unknown agent proves nothing worth registering)
/// - `sessionEnd`            → `SessionEnd` (harmless if it never fires in CLI)
/// - anything else           → bail (registered-vs-decoded drift must be loud)
///
/// The activity arms prepend an [`AgentEvent::Identity`] (#221) because Cursor
/// is HOOK-ONLY: a slot the reducer's proof-of-life pre-pass synthesizes
/// mid-turn has no JSONL back-fill path, so without the attached identity it
/// would stay a blank `#N` ghost. The cwd IS the session key, so `session_id`
/// mirrors the `SessionStart` arm exactly — coalescing holds.
pub fn decode_cursor_hook_payload(v: &Value) -> Result<Vec<AgentEvent>> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("cursor hook payload must be an object"))?;
    let event = obj
        .get("hook_event_name")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("cursor payload missing hook_event_name"))?;
    // cwd is the ONLY usable identity a Cursor CLI hook carries — the `cwd`
    // field, falling back to the first `workspace_roots` entry. An empty one
    // would mint a phantom agent nothing coalesces with (the Reasonix idiom).
    let cwd = obj
        .get("cwd")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            obj.get("workspace_roots")
                .and_then(|r| r.as_array())
                .and_then(|a| a.first())
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
        })
        .ok_or_else(|| anyhow!("cursor payload missing/empty cwd and workspace_roots"))?;
    let agent_id = AgentId::from_parts(SOURCE_NAME, cwd);

    let identity = || AgentEvent::Identity {
        agent_id,
        source: SOURCE_NAME.to_string(),
        // Mirrors the SessionStart arm: no usable session id in CLI mode; the
        // cwd IS the session key.
        session_id: cwd.to_string(),
        cwd: Some(cwd.into()),
    };

    match event {
        "sessionStart" => Ok(vec![AgentEvent::SessionStart {
            agent_id,
            source: SOURCE_NAME.to_string(),
            session_id: cwd.to_string(),
            cwd: cwd.into(),
            parent_id: None,
        }]),
        "preToolUse" => {
            let tool = obj.get("tool_name").and_then(|s| s.as_str()).unwrap_or("?");
            Ok(vec![
                identity(),
                AgentEvent::ActivityStart {
                    agent_id,
                    tool_use_id: None,
                    detail: Some(cursor_tool_detail(tool, obj.get("tool_input"))),
                },
            ])
        }
        "postToolUse" => Ok(vec![
            identity(),
            AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: None,
            },
        ]),
        // Turn end — deliberately Identity-LESS (the shared Stop arm makes the
        // same call): an end doesn't prove a session worth registering.
        "stop" => Ok(vec![AgentEvent::ActivityEnd {
            agent_id,
            tool_use_id: None,
        }]),
        "sessionEnd" => Ok(vec![AgentEvent::SessionEnd {
            agent_id,
            as_child: false,
        }]),
        other => bail!("unsupported cursor hook event: {other}"),
    }
}

/// The registry's `hook.custom` entry point. Cursor's envelope is ALIEN to the
/// shared CC-shaped arms (camelCase event values, cwd-only identity), so per
/// the `HookDecoding::custom` contract it claims EVERY event reaching it —
/// `.map(Some)`, never `Ok(None)`.
pub(crate) fn decode_cursor_hook_custom(v: &Value) -> Result<Option<Vec<AgentEvent>>> {
    decode_cursor_hook_payload(v).map(Some)
}

/// Cursor tool detail: `"name: target"` using Cursor's argument vocabulary,
/// looked up `command` > `file_path` > `path` > `pattern` > `url` (the keys its
/// shell/read/edit/grep/web tool inputs carry). Both the tool NAME and the
/// `: target` are capped at the decode boundary (pitfall 3), matching
/// `make_tool_detail`/`rx_tool_detail`. No subagent-dispatch detection — Cursor
/// renders session-only (no in-CLI delegation signal).
fn cursor_tool_detail(tool: &str, args: Option<&Value>) -> ToolDetail {
    let target = args
        .and_then(|a| {
            ["command", "file_path", "path", "pattern", "url"]
                .iter()
                .find_map(|k| a.get(k).and_then(|v| v.as_str()))
        })
        .map(|s| format!(": {}", ellipsize(s, MAX_TOOL_TARGET_CHARS)))
        .unwrap_or_default();
    ToolDetail::Generic {
        display: format!("{}{target}", ellipsize(tool, MAX_DECODED_FIELD_CHARS)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode_all(v: Value) -> Vec<AgentEvent> {
        decode_cursor_hook_payload(&v).expect("decodes")
    }

    /// The payload's MAIN event — the last decoded event (activity arms prepend
    /// an `Identity`).
    fn decode(v: Value) -> AgentEvent {
        decode_all(v).pop().expect("at least one event")
    }

    #[test]
    fn session_start_keys_on_cwd() {
        let ev = decode(json!({
            "hook_event_name": "sessionStart",
            "cwd": "/Users/dev/proj",
            "conversation_id": "ignored-in-cli"
        }));
        match ev {
            AgentEvent::SessionStart {
                agent_id,
                source,
                session_id,
                cwd,
                parent_id,
            } => {
                assert_eq!(source, SOURCE_NAME);
                assert_eq!(
                    agent_id,
                    AgentId::from_parts(SOURCE_NAME, "/Users/dev/proj")
                );
                assert_eq!(session_id, "/Users/dev/proj", "cwd IS the session key");
                assert_eq!(cwd, std::path::PathBuf::from("/Users/dev/proj"));
                assert_eq!(parent_id, None);
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn cwd_falls_back_to_workspace_roots() {
        // When `cwd` is absent the first workspace root is the identity.
        let ev = decode(json!({
            "hook_event_name": "sessionStart",
            "workspace_roots": ["/Users/dev/proj", "/other"]
        }));
        assert!(matches!(ev, AgentEvent::SessionStart { agent_id, .. }
            if agent_id == AgentId::from_parts(SOURCE_NAME, "/Users/dev/proj")));
    }

    #[test]
    fn pre_tool_use_is_activity_start_with_no_tool_id() {
        let ev = decode(json!({
            "hook_event_name": "preToolUse",
            "cwd": "/repo",
            "tool_name": "readToolCall",
            "tool_input": {"path": "src/main.rs"}
        }));
        match ev {
            AgentEvent::ActivityStart {
                tool_use_id,
                detail,
                ..
            } => {
                assert_eq!(tool_use_id, None);
                assert_eq!(detail.unwrap().display(), "readToolCall: src/main.rs");
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn tool_target_uses_cursor_arg_vocabulary() {
        // `command` wins the priority order.
        let shell = decode(json!({
            "hook_event_name": "preToolUse", "cwd": "/r",
            "tool_name": "shell", "tool_input": {"command": "cargo test"}
        }));
        assert!(
            matches!(shell, AgentEvent::ActivityStart { detail: Some(d), .. }
            if d.display() == "shell: cargo test")
        );
        // `file_path` (edit/write tools) is recognized.
        let edit = decode(json!({
            "hook_event_name": "preToolUse", "cwd": "/r",
            "tool_name": "edit", "tool_input": {"file_path": "src/lib.rs"}
        }));
        assert!(
            matches!(edit, AgentEvent::ActivityStart { detail: Some(d), .. }
            if d.display() == "edit: src/lib.rs")
        );
    }

    #[test]
    fn long_targets_are_truncated() {
        let long = "x".repeat(60);
        let ev = decode(json!({
            "hook_event_name": "preToolUse", "cwd": "/r",
            "tool_name": "shell", "tool_input": {"command": long}
        }));
        match ev {
            AgentEvent::ActivityStart {
                detail: Some(d), ..
            } => {
                let display = d.display();
                assert!(display.starts_with("shell: "));
                assert!(display.ends_with('…'));
                assert_eq!(display.chars().count(), "shell: ".chars().count() + 41);
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn long_tool_name_is_truncated_at_the_decode_boundary() {
        let long = "T".repeat(MAX_DECODED_FIELD_CHARS * 3);
        let ev = decode(json!({
            "hook_event_name": "preToolUse", "cwd": "/r",
            "tool_name": long, "tool_input": {}
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
    fn post_tool_use_and_stop_are_activity_end() {
        for event in ["postToolUse", "stop"] {
            let ev = decode(json!({"hook_event_name": event, "cwd": "/r"}));
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
    fn session_end_maps_to_session_end() {
        let ev = decode(json!({"hook_event_name": "sessionEnd", "cwd": "/r"}));
        assert!(matches!(
            ev,
            AgentEvent::SessionEnd {
                as_child: false,
                ..
            }
        ));
    }

    #[test]
    fn all_events_for_one_cwd_share_one_agent_id() {
        // The coalescing contract: every event of a session (the prepended
        // Identity events INCLUDED) keys on the same cwd-derived AgentId.
        let events = [
            json!({"hook_event_name": "sessionStart", "cwd": "/Users/dev/p"}),
            json!({"hook_event_name": "preToolUse", "cwd": "/Users/dev/p", "tool_name": "shell",
                   "tool_input": {"command": "ls"}}),
            json!({"hook_event_name": "postToolUse", "cwd": "/Users/dev/p", "tool_name": "shell"}),
            json!({"hook_event_name": "stop", "cwd": "/Users/dev/p"}),
            json!({"hook_event_name": "sessionEnd", "cwd": "/Users/dev/p"}),
        ];
        let ids: std::collections::BTreeSet<_> = events
            .iter()
            .flat_map(|v| decode_cursor_hook_payload(v).unwrap())
            .map(|e| e.agent_id())
            .collect();
        assert_eq!(ids.len(), 1, "all events must coalesce to one AgentId");
    }

    #[test]
    fn activity_arms_prepend_identity_with_cwd_keyed_session() {
        for payload in [
            json!({"hook_event_name": "preToolUse", "cwd": "/Users/dev/p", "tool_name": "shell",
                   "tool_input": {"command": "ls"}}),
            json!({"hook_event_name": "postToolUse", "cwd": "/Users/dev/p", "tool_name": "shell"}),
        ] {
            let name = payload["hook_event_name"].clone();
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
                        "cursor hooks always know their cwd"
                    );
                }
                other => panic!("{name}: expected leading Identity, got {other:?}"),
            }
        }
    }

    #[test]
    fn stop_session_events_and_session_end_carry_no_identity() {
        for payload in [
            json!({"hook_event_name": "stop", "cwd": "/r"}),
            json!({"hook_event_name": "sessionStart", "cwd": "/r"}),
            json!({"hook_event_name": "sessionEnd", "cwd": "/r"}),
        ] {
            let name = payload["hook_event_name"].clone();
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
        assert!(
            decode_cursor_hook_payload(&json!({"hook_event_name": "stop", "cwd": ""})).is_err()
        );
        assert!(decode_cursor_hook_payload(&json!({"hook_event_name": "stop"})).is_err());
        // An empty workspace_roots array doesn't rescue an absent cwd.
        assert!(decode_cursor_hook_payload(
            &json!({"hook_event_name": "stop", "workspace_roots": []})
        )
        .is_err());
    }

    #[test]
    fn unknown_event_bails_loudly() {
        // Registered-vs-decoded drift must surface. subagentStart/Stop are
        // deliberately unregistered (not firing in CLI / session-only).
        for ev in [
            "subagentStart",
            "subagentStop",
            "beforeShellExecution",
            "Bogus",
        ] {
            assert!(
                decode_cursor_hook_payload(&json!({"hook_event_name": ev, "cwd": "/r"})).is_err(),
                "{ev} must bail (not registered, must not decode silently)"
            );
        }
    }

    #[test]
    fn non_object_payload_is_malformed() {
        assert!(decode_cursor_hook_payload(&json!("just a string")).is_err());
        assert!(decode_cursor_hook_payload(&json!(42)).is_err());
    }

    #[test]
    fn pre_tool_use_without_tool_name_displays_question_mark() {
        let ev = decode(json!({"hook_event_name": "preToolUse", "cwd": "/r"}));
        assert!(
            matches!(ev, AgentEvent::ActivityStart { detail: Some(d), .. }
            if d.display() == "?")
        );
    }
}
