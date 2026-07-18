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
//!   panel (`s`) registers the shim in the GLOBAL `~/.cursor/hooks.json`.
//!
//! Hook payloads arrive on the shared hook socket stamped
//! `_pixtuoid_source: "cursor"`; `decoder::decode_hook_payload` dispatches them
//! here (the custom decoder runs FIRST, before the shared CC-shaped field
//! requirements). Cursor's envelope reuses CC's `hook_event_name` field NAME
//! but with **camelCase values** — wire shape verified against a real
//! `cursor-agent -p` capture shape:
//!
//! ```json
//! {"hook_event_name":"preToolUse","session_id":"c7cef226-…","cwd":"",
//!  "workspace_roots":["/repo"],"tool_name":"Shell",
//!  "tool_input":{"command":"ls -la"}}
//! ```
//!
//! Keyed on **`session_id`** (present + CONSISTENT across every CLI event in the
//! capture; == `conversation_id` and the transcript filename stem), so concurrent
//! sessions in one project stay distinct and all of a session's events coalesce.
//! The TOP-LEVEL `cwd` is EMPTY/absent in CLI hooks — `workspace_roots[0]` is the
//! real workspace, used for the label + the SessionStart cwd (a workspace
//! fallback covers the degenerate session_id-less event). Deliberate:
//!
//! - `tool_use_id` is always `None`: the reducer's per-call machinery
//!   (hook-wins dedup, `active_tasks`) is bypassed — harmless on a single
//!   transport. `tool_name` is PascalCase (`Shell`/`Grep`/`Read`); `tool_input`
//!   carries `command`/`pattern`/`file_path`.
//! - **Subagents: rendered FLAT, never nested (parent-link is genuinely
//!   absent).** A parallel-subagent task dispatches via a `Task` tool
//!   (`tool_name:"Task"` + `tool_input.subagent_type` — capture-verified, the
//!   CC semantic) → the PARENT reads "Delegating" (`cursor_tool_detail`). But
//!   each child runs as an INDEPENDENT session (own `session_id`, firing tool
//!   events with NO `sessionStart`/`sessionEnd`), and NOTHING in the stream
//!   links a child to its parent — `subagentStart`/`subagentStop` don't fire
//!   (capture-verified: 0), the `Task` dispatch carries only the PARENT's id,
//!   and child events carry no `parentId`. So children appear as sibling `cu·`
//!   sprites (a "parallel agents" effect), NOT nested, and — getting no
//!   `sessionEnd` — they age out via the idle stale-sweep. The missing link is a
//!   proven upstream constraint (drift-watched for if/when the CLI lands the
//!   subagent hooks).
//! - Exit profile: `sessionEnd` FIRES on clean completion (capture-verified:
//!   `reason:"completed"`) → `has_exit_signal: true` (best-effort, CC/Reasonix
//!   class). `stop` is turn-end and did NOT fire under `-p` (kept mapped for
//!   interactive turns); abrupt exits (no PID exposed) fall to the stale-sweep.
//! - A per-session JSONL transcript DOES exist
//!   (`~/.cursor/projects/<proj>/agent-transcripts/<session-id>/<id>.jsonl`,
//!   `transcript_path` on the payload) — hook-only is complete today, but this is
//!   the seam if a watcher is ever wanted (its stem == our `session_id` key).

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use crate::source::decoder::generic_tool_display;
use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

pub const SOURCE_NAME: &str = "cursor";

/// Decode one Cursor hook payload (already identified by
/// `_pixtuoid_source == "cursor"`). Envelope per `cursor.com/docs/hooks`.
///
/// Event mapping (camelCase `hook_event_name` values), all keyed on `session_id`:
/// - `sessionStart`          → `SessionStart`
/// - `preToolUse`            → `Identity` + `ActivityStart`
/// - `postToolUse`           → `Identity` + `ActivityEnd`
/// - `postToolUseFailure`    → `Identity` + `ActivityEnd` (a FAILED tool fires
///   this INSTEAD OF `postToolUse`; closed the same way — identity-backed, since
///   it carries cwd/tool_name — so the sprite doesn't linger Active under `-p`)
/// - `stop`                  → `ActivityEnd` (turn end → idle debounce; NO
///   Identity — an end for an unknown agent proves nothing worth registering)
/// - `sessionEnd`            → `SessionEnd`
/// - anything else           → bail (registered-vs-decoded drift must be loud)
///
/// The activity arms prepend an [`AgentEvent::Identity`] (#221) because Cursor
/// is HOOK-ONLY: a slot the reducer's proof-of-life pre-pass synthesizes
/// mid-turn has no JSONL back-fill path, so without the attached identity it
/// would stay a blank `#N` ghost. The Identity's `session_id` mirrors the
/// `SessionStart` arm's key exactly — coalescing holds.
pub fn decode_cursor_hook_payload(v: &Value) -> Result<Vec<AgentEvent>> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("cursor hook payload must be an object"))?;
    let event = obj
        .get("hook_event_name")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("cursor payload missing hook_event_name"))?;
    // The workspace path: the top-level `cwd` is EMPTY/absent in CLI hook
    // payloads (from a real capture) — `workspace_roots[0]` is the real
    // one. Used for the label + the SessionStart/Identity cwd, NOT the AgentId key.
    let workspace = obj
        .get("cwd")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            obj.get("workspace_roots")
                .and_then(|r| r.as_array())
                .and_then(|a| a.first())
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
        });
    // Key on `session_id` — present and CONSISTENT across every CLI hook event
    // (capture-verified; == `conversation_id` and the transcript filename stem),
    // so it distinguishes concurrent sessions in one project AND coalesces all of
    // a session's events. Fall back to the workspace path only if a future event
    // ever omits it (keeps coalescing best-effort instead of dropping the event).
    let key = obj
        .get("session_id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .or(workspace)
        .ok_or_else(|| anyhow!("cursor payload has no session_id, cwd, or workspace_roots"))?;
    let agent_id = AgentId::from_parts(SOURCE_NAME, key);
    let cwd = workspace.unwrap_or("");

    let identity = || AgentEvent::Identity {
        agent_id,
        source: SOURCE_NAME.to_string(),
        session_id: key.to_string(),
        cwd: (!cwd.is_empty()).then(|| cwd.into()),
        pid: None,
    };

    match event {
        "sessionStart" => Ok(vec![AgentEvent::SessionStart {
            agent_id,
            source: SOURCE_NAME.to_string(),
            session_id: key.to_string(),
            cwd: cwd.into(),
            parent_id: None,
        }]),
        "preToolUse" => {
            let tool = obj
                .get("tool_name")
                .and_then(|s| s.as_str())
                .unwrap_or_else(|| {
                    crate::source::drift::missing_field(SOURCE_NAME, "preToolUse", "tool_name");
                    "?"
                });
            Ok(vec![
                identity(),
                AgentEvent::ActivityStart {
                    agent_id,
                    tool_use_id: None,
                    detail: Some(cursor_tool_detail(tool, obj.get("tool_input"))),
                },
            ])
        }
        // A FAILED tool fires `postToolUseFailure` (carrying
        // error_message/failure_type) INSTEAD OF `postToolUse`. Close the
        // activity identically — identity-backed, since it carries cwd/tool_name
        // and is proof-of-life. Without this arm a failed tool's ActivityStart
        // never ends under `-p` (where `stop` doesn't fire) and the sprite
        // lingers Active until sessionEnd / the stale sweep.
        "postToolUse" | "postToolUseFailure" => Ok(vec![
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
        other => {
            crate::source::drift::unknown_event(SOURCE_NAME, other);
            bail!("unsupported cursor hook event: {other}")
        }
    }
}

/// Cursor tool detail: `"name: target"` using Cursor's argument vocabulary,
/// looked up `command` > `file_path` > `path` > `pattern` > `url` (the keys its
/// shell/read/edit/grep/web tool inputs carry). Both the tool NAME and the
/// `: target` are capped at the decode boundary (pitfall 3), matching
/// `make_tool_detail`/`rx_tool_detail`. No subagent-dispatch detection — Cursor
/// renders session-only (no in-CLI delegation signal).
fn cursor_tool_detail(tool: &str, args: Option<&Value>) -> ToolDetail {
    // Subagent dispatch: Cursor's `Task` tool carries a `subagent_type`
    // (from a real capture, e.g. "code-explorer") — the SAME stable
    // semantic signal CC's `make_tool_detail` keys on. Show "Delegating" on the
    // parent while the children work. (The children run as INDEPENDENT sessions
    // with no parent-link in the stream — see the module doc — so this is the
    // only delegation signal pixtuoid can render; mirrors CC's `has_subagent_type
    // || known_name`.)
    let has_subagent_type = args.and_then(|a| a.get("subagent_type")).is_some();
    if tool == "Task" || has_subagent_type {
        return ToolDetail::Task;
    }
    // Per-source target vocabulary; the shared scan lives in the decoder, the
    // last-mile assembly (name + `: target` with the matching caps) in
    // `generic_tool_display`.
    const KEYS: &[&str] = &["command", "file_path", "path", "pattern", "url"];
    let target = args.and_then(|a| crate::source::decoder::first_present_str(a, KEYS));
    generic_tool_display(tool, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::decoder::MAX_DECODED_FIELD_CHARS;
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
    fn session_start_keys_on_session_id_label_from_workspace() {
        // Real CLI shape: session_id present, top-level cwd EMPTY, the workspace
        // in workspace_roots[0]. Key = session_id; label cwd = workspace_roots[0].
        let ev = decode(json!({
            "hook_event_name": "sessionStart",
            "session_id": "c7cef226-sess",
            "conversation_id": "c7cef226-sess",
            "cwd": "",
            "workspace_roots": ["/Users/dev/proj"]
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
                assert_eq!(agent_id, AgentId::from_parts(SOURCE_NAME, "c7cef226-sess"));
                assert_eq!(session_id, "c7cef226-sess", "key on session_id, not cwd");
                assert_eq!(
                    cwd,
                    std::path::PathBuf::from("/Users/dev/proj"),
                    "empty top-level cwd → workspace_roots[0] for the label"
                );
                assert_eq!(parent_id, None);
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn session_id_distinguishes_two_sessions_in_one_workspace() {
        // The upgrade's whole point: cwd-keying would merge these into one sprite;
        // session_id keeps them distinct.
        let a = decode(
            json!({"hook_event_name": "sessionStart", "session_id": "sess-A",
                              "workspace_roots": ["/repo"]}),
        );
        let b = decode(
            json!({"hook_event_name": "sessionStart", "session_id": "sess-B",
                              "workspace_roots": ["/repo"]}),
        );
        assert_ne!(
            a.agent_id(),
            b.agent_id(),
            "two sessions in one repo must be distinct"
        );
    }

    #[test]
    fn key_falls_back_to_workspace_when_session_id_absent() {
        // Defensive: a (hypothetical) event with no session_id still keys
        // consistently on the workspace rather than dropping.
        let ev = decode(json!({
            "hook_event_name": "sessionStart",
            "workspace_roots": ["/Users/dev/proj", "/other"]
        }));
        assert!(matches!(ev, AgentEvent::SessionStart { agent_id, .. }
            if agent_id == AgentId::from_parts(SOURCE_NAME, "/Users/dev/proj")));
    }

    #[test]
    fn pre_tool_use_is_activity_start_with_no_tool_id() {
        // Real CLI tool shape: PascalCase tool_name, file_path input, empty cwd.
        let ev = decode(json!({
            "hook_event_name": "preToolUse",
            "session_id": "s",
            "cwd": "",
            "workspace_roots": ["/repo"],
            "tool_name": "Read",
            "tool_input": {"file_path": "/repo/src/main.rs"}
        }));
        match ev {
            AgentEvent::ActivityStart {
                tool_use_id,
                detail,
                ..
            } => {
                assert_eq!(tool_use_id, None);
                assert_eq!(detail.unwrap().display(), "Read: /repo/src/main.rs");
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn task_dispatch_with_subagent_type_is_delegating() {
        // Capture-verified: a parallel-subagent dispatch fires preToolUse with
        // tool_name "Task" + tool_input.subagent_type — the parent must read
        // Delegating (ToolDetail::Task), mirroring CC's semantic detection.
        let ev = decode(json!({
            "hook_event_name": "preToolUse",
            "session_id": "parent",
            "workspace_roots": ["/repo"],
            "tool_name": "Task",
            "tool_input": {"subagent_type": "code-explorer", "description": "investigate the build"}
        }));
        assert!(
            matches!(&ev, AgentEvent::ActivityStart { detail: Some(d), .. } if d.is_task()),
            "Task + subagent_type must map to ToolDetail::Task, got {ev:?}"
        );
        // An ordinary tool stays Generic.
        let read = decode(json!({
            "hook_event_name": "preToolUse", "session_id": "p", "workspace_roots": ["/r"],
            "tool_name": "Read", "tool_input": {"file_path": "/r/x.rs"}
        }));
        assert!(matches!(&read, AgentEvent::ActivityStart { detail: Some(d), .. } if !d.is_task()));
        // EITHER signal alone suffices (the detection is `name OR semantic
        // field`, mirroring CC): an input-less `Task` call, and a renamed
        // dispatch that still carries `subagent_type`.
        let bare_task = decode(json!({
            "hook_event_name": "preToolUse", "session_id": "p", "workspace_roots": ["/r"],
            "tool_name": "Task"
        }));
        assert!(
            matches!(&bare_task, AgentEvent::ActivityStart { detail: Some(d), .. } if d.is_task()),
            "an input-less Task dispatch must still read as Delegating, got {bare_task:?}"
        );
        let renamed = decode(json!({
            "hook_event_name": "preToolUse", "session_id": "p", "workspace_roots": ["/r"],
            "tool_name": "Delegate", "tool_input": {"subagent_type": "code-explorer"}
        }));
        assert!(
            matches!(&renamed, AgentEvent::ActivityStart { detail: Some(d), .. } if d.is_task()),
            "the semantic field must catch a renamed dispatch, got {renamed:?}"
        );
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
        // postToolUseFailure (a failed tool) closes activity like postToolUse.
        for event in ["postToolUse", "postToolUseFailure", "stop"] {
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
    fn all_events_for_one_session_share_one_agent_id() {
        // The coalescing contract: every event of a session keys on the same
        // session_id-derived AgentId — even though the top-level cwd is empty and
        // only workspace_roots carries the path (the real CLI shape).
        let sid = "c7cef226-sess";
        let events = [
            json!({"hook_event_name": "sessionStart", "session_id": sid, "workspace_roots": ["/repo"]}),
            json!({"hook_event_name": "preToolUse", "session_id": sid, "cwd": "", "workspace_roots": ["/repo"],
                   "tool_name": "Shell", "tool_input": {"command": "ls"}}),
            json!({"hook_event_name": "postToolUse", "session_id": sid, "workspace_roots": ["/repo"], "tool_name": "Shell"}),
            json!({"hook_event_name": "stop", "session_id": sid, "workspace_roots": ["/repo"]}),
            json!({"hook_event_name": "sessionEnd", "session_id": sid, "reason": "completed", "workspace_roots": ["/repo"]}),
        ];
        let ids: std::collections::BTreeSet<_> = events
            .iter()
            .flat_map(|v| decode_cursor_hook_payload(v).unwrap())
            .map(|e| e.agent_id())
            .collect();
        assert_eq!(ids.len(), 1, "all events must coalesce to one AgentId");
    }

    #[test]
    fn activity_arms_prepend_identity_keyed_on_session_id() {
        for payload in [
            json!({"hook_event_name": "preToolUse", "session_id": "s", "cwd": "", "workspace_roots": ["/repo"],
                   "tool_name": "Shell", "tool_input": {"command": "ls"}}),
            json!({"hook_event_name": "postToolUse", "session_id": "s", "workspace_roots": ["/repo"], "tool_name": "Shell"}),
            // A failed tool must ALSO back-fill identity (not be identity-less
            // like `stop`) — it carries cwd/tool_name and is proof-of-life.
            json!({"hook_event_name": "postToolUseFailure", "session_id": "s", "workspace_roots": ["/repo"],
                   "tool_name": "Shell", "error_message": "command failed", "failure_type": "error", "is_interrupt": false}),
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
                    pid: None,
                } => {
                    assert_eq!(*agent_id, AgentId::from_parts(SOURCE_NAME, "s"));
                    assert_eq!(source, SOURCE_NAME);
                    assert_eq!(session_id, "s", "key on session_id");
                    assert_eq!(
                        cwd.as_deref(),
                        Some(std::path::Path::new("/repo")),
                        "Identity cwd comes from workspace_roots[0]"
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
    fn no_session_id_cwd_or_workspace_is_malformed_but_session_id_alone_is_ok() {
        // Nothing to key on → Err.
        assert!(decode_cursor_hook_payload(&json!({"hook_event_name": "stop"})).is_err());
        assert!(decode_cursor_hook_payload(
            &json!({"hook_event_name": "stop", "cwd": "", "workspace_roots": []})
        )
        .is_err());
        // session_id alone is enough — cwd/workspace are only for the label.
        assert!(
            decode_cursor_hook_payload(&json!({"hook_event_name": "stop", "session_id": "s"}))
                .is_ok()
        );
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
