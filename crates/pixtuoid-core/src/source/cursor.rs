//! Cursor CLI source — HOOK-ONLY (no JSONL watcher).
//!
//! Only one of Cursor's three seams is reachable by a *passive observer*:
//! `--output-format stream-json` NDJSON is stdout of an invocation we would have
//! to launch ourselves, and on-disk sessions are SQLite
//! (`~/.cursor/chats/.../store.db`, hex-encoded blobs). So **Cursor Hooks**
//! (`~/.cursor/hooks.json`, registered by the Connection panel) is the seam.
//!
//! Payloads arrive on the shared hook socket stamped
//! `_pixtuoid_source: "cursor"`. The envelope reuses CC's `hook_event_name`
//! field NAME but with **camelCase values**; `tool_name` is PascalCase
//! (`Shell`/`Grep`/`Read`) and `tool_input` carries
//! `command`/`pattern`/`file_path`.
//!
//! Keyed on **`session_id`** (present + CONSISTENT across every CLI event; ==
//! `conversation_id` and the transcript filename stem), so concurrent sessions
//! in one project stay distinct. The TOP-LEVEL `cwd` is EMPTY/absent in CLI
//! hooks — `workspace_roots[0]` is the real workspace.
//!
//! - **`tool_use_id` IS on the wire and DROPPED deliberately — do not "fix"
//!   that.** Every `preToolUse` pairs with its `postToolUse` /
//!   `postToolUseFailure` on the same id, and tools INTERLEAVE (a Read and a
//!   Shell overlap), so the id looks like exactly what the reducer's per-call
//!   machinery wants. The trap is `Task`: it fires `preToolUse` and NEVER a
//!   post (capture-verified — every unpaired id in a delegating run is a `Task`,
//!   while `subagentStart`/`subagentStop` stay silent). Passing the id through would
//!   satisfy `track_active_tasks`' `Some(tuid)` + `is_task()` arm, insert a
//!   tuid nothing ever drains, and strand the parent Delegating for the rest of
//!   the session — `apply_activity_end` re-enters Delegating on every later
//!   tool end while `any_active_task` holds. The machinery the id would
//!   otherwise buy is inert here anyway: hook-wins dedup needs a second
//!   transport, and precise wait-resolution needs permission events this source
//!   never emits. Pinned by `tool_use_id_is_dropped_even_though_the_wire_has_one`.
//! - **cursor runs its hook command 4-6x per event, and only ONE of those keeps
//!   the `PIXTUOID_SOURCE=` env prefix** its install writes — the others arrive
//!   with no argv and no stamp (counted against a wrapper on the shim, no
//!   recorder in the loop; an unrelated hook entry in the same config ran once).
//!   Unstamped, they fall to the claude-code default and bail on these camelCase
//!   event values, so `decoder`'s cross-fire guard drops them on `cursor_version`.
//!   Lossless: the one stamped copy carries the whole arc, which
//!   `cursor/tool-failure` pins byte for byte — 29 payloads, 8 of them stamped.
//! - **Subagents render FLAT, never nested — the parent-link is genuinely
//!   absent.** Each child runs as an INDEPENDENT session and NOTHING links it to
//!   its parent: `subagentStart`/`subagentStop` don't fire (capture-verified:
//!   0), the `Task` dispatch carries only the PARENT's id, and child events
//!   carry no `parentId`. Getting no `sessionEnd`, children age out via the
//!   idle stale-sweep.
//! - Exit profile: `sessionEnd` FIRES on clean completion → `has_exit_signal:
//!   true`. `stop` is turn-end and did NOT fire under `-p` (kept mapped for
//!   interactive turns). An abrupt exit rides the shim's `_pid` BEST-EFFORT:
//!   the walk skips the `$SHELL -c` wrapper Cursor `eval`s every hook inside,
//!   but some hooks arrive through a different non-shell ancestor, so the pid is
//!   not stable across every event and corroboration often withholds the arm
//!   (#896) — see this crate's `SHARP-EDGES.md`.
//! - A per-session JSONL transcript DOES exist and the envelope now CARRIES its
//!   path (`transcript_path`: `null` on `sessionStart`, set from the first tool
//!   event on) — the seam if a watcher is ever wanted, without reconstructing
//!   `~/.cursor/projects/<proj>/agent-transcripts/<session-id>/<id>.jsonl`
//!   (its stem == our `session_id` key).

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use crate::source::decoder::{ellipsize, MAX_DECODED_FIELD_CHARS};
use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

/// The Cursor CLI source's registry name (its `SourceDescriptor.name`).
pub const SOURCE_NAME: &str = "cursor";

/// Decode one Cursor hook payload (already identified by
/// `_pixtuoid_source == "cursor"`). Envelope per `cursor.com/docs/hooks`; an
/// unregistered event bails, so registered-vs-decoded drift is loud.
///
/// The activity arms prepend an [`AgentEvent::Identity`] (#221) because Cursor
/// is HOOK-ONLY: a slot the reducer's proof-of-life pre-pass synthesizes
/// mid-turn has no JSONL back-fill path, so without the attached identity it
/// would stay a blank `#N` ghost.
pub fn decode_cursor_hook_payload(v: &Value) -> Result<Vec<AgentEvent>> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("cursor hook payload must be an object"))?;
    let event = obj
        .get("hook_event_name")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("cursor payload missing hook_event_name"))?;
    // The top-level `cwd` is EMPTY/absent in CLI hook payloads —
    // `workspace_roots[0]` is the real one. Label/cwd only, NOT the AgentId key.
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
    // Key on `session_id` — present and CONSISTENT across every CLI hook event,
    // so it distinguishes concurrent sessions in one project AND coalesces all of
    // a session's events. Fall back to the workspace path only if a future event
    // ever omits it, rather than dropping it.
    let key = obj
        .get("session_id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .or(workspace)
        .ok_or_else(|| anyhow!("cursor payload has no session_id, cwd, or workspace_roots"))?;
    let agent_id = AgentId::from_parts(SOURCE_NAME, key);
    let cwd = workspace.unwrap_or("");

    let identity = || {
        AgentEvent::identity(
            agent_id,
            SOURCE_NAME,
            key,
            (!cwd.is_empty()).then(|| cwd.into()),
        )
    };

    let decoded: Result<Vec<AgentEvent>> = match event {
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
        // A FAILED tool fires `postToolUseFailure` INSTEAD OF `postToolUse`.
        // Without this arm a failed tool's ActivityStart never ends under `-p`
        // (where `stop` doesn't fire) and the sprite lingers Active.
        "postToolUse" | "postToolUseFailure" => Ok(vec![
            identity(),
            AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: None,
            },
        ]),
        // Turn end — deliberately Identity-LESS: an end doesn't prove a session
        // worth registering.
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
            bail!(
                "unsupported cursor hook event: {}",
                crate::source::decoder::display_safe(other)
            )
        }
    };
    let mut evs = decoded?;
    // Cursor stamps the model on EVERY event, and it changes within one session
    // (a `sessionStart` on `composer-2.5-fast` whose tool calls run on
    // `composer-2.5`), so reading it once at the start would show the wrong one
    // for the rest of the turn.
    if let Some(model) = obj
        .get("model")
        .and_then(|m| m.as_str())
        .filter(|s| !s.is_empty())
    {
        evs.push(AgentEvent::ModelInfo {
            agent_id,
            model: Some(ellipsize(model, MAX_DECODED_FIELD_CHARS)),
            effort: None,
        });
    }
    Ok(evs)
}

fn cursor_tool_detail(tool: &str, args: Option<&Value>) -> ToolDetail {
    // Cursor's `Task` tool carries a `subagent_type` (capture-verified) — the
    // SAME stable semantic signal CC's `make_tool_detail` keys on. The children
    // run as INDEPENDENT sessions with no parent-link in the stream (module
    // doc), so this is the only delegation signal pixtuoid can render.
    let has_subagent_type = args.and_then(|a| a.get("subagent_type")).is_some();
    if tool == "Task" || has_subagent_type {
        return ToolDetail::Task;
    }
    const KEYS: &[&str] = &["command", "file_path", "path", "pattern", "url"];
    crate::source::decoder::generic_keyed_detail(tool, args, KEYS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::decoder::MAX_DECODED_FIELD_CHARS;
    use serde_json::json;

    fn decode_all(v: Value) -> Vec<AgentEvent> {
        decode_cursor_hook_payload(&v).expect("decodes")
    }

    /// The payload's MAIN event — the LAST one (activity arms prepend an Identity).
    fn decode(v: Value) -> AgentEvent {
        decode_all(v).pop().expect("at least one event")
    }

    #[test]
    fn every_event_carries_the_model_and_it_can_change_mid_session() {
        // Both shapes from fixtures/cursor/tool-run: a session opens on the
        // `-fast` composer and the tool events run on the full one.
        let start = decode_all(json!({
            "hook_event_name": "sessionStart", "session_id": "s1",
            "workspace_roots": ["/repo"], "model": "composer-2.5-fast"
        }));
        let tool = decode_all(json!({
            "hook_event_name": "preToolUse", "session_id": "s1",
            "workspace_roots": ["/repo"], "tool_name": "read", "model": "composer-2.5"
        }));
        let model_of = |evs: &[AgentEvent]| {
            evs.iter().find_map(|e| match e {
                AgentEvent::ModelInfo { model, .. } => model.clone(),
                _ => None,
            })
        };
        assert_eq!(model_of(&start).as_deref(), Some("composer-2.5-fast"));
        assert_eq!(model_of(&tool).as_deref(), Some("composer-2.5"));
    }

    #[test]
    fn session_start_keys_on_session_id_label_from_workspace() {
        // Real CLI shape: session_id present, top-level cwd EMPTY, the workspace
        // in workspace_roots[0].
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

    /// The wire HAS a `tool_use_id` and it pairs; we drop it on purpose. Wiring
    /// it would let `track_active_tasks` claim cursor's `Task` — which never
    /// gets a `postToolUse` — and strand the parent Delegating. See the module
    /// doc before changing this.
    #[test]
    fn tool_use_id_is_dropped_even_though_the_wire_has_one() {
        for event in ["preToolUse", "postToolUse", "postToolUseFailure"] {
            let ev = decode(json!({
                "hook_event_name": event, "session_id": "s", "workspace_roots": ["/repo"],
                "tool_name": "Shell", "tool_input": {"command": "ls"},
                "tool_use_id": "tool_b3a89c28-dbe4-487e-8c70-ea05a83083a"
            }));
            let tuid = match &ev {
                AgentEvent::ActivityStart { tool_use_id, .. }
                | AgentEvent::ActivityEnd { tool_use_id, .. } => tool_use_id.clone(),
                other => panic!("{event}: expected an activity event, got {other:?}"),
            };
            assert_eq!(
                tuid, None,
                "{event}: the id must stay dropped — passing it through strands \
                 a Task-delegating parent (module doc)"
            );
        }
    }

    #[test]
    fn task_dispatch_with_subagent_type_is_delegating() {
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
        let read = decode(json!({
            "hook_event_name": "preToolUse", "session_id": "p", "workspace_roots": ["/r"],
            "tool_name": "Read", "tool_input": {"file_path": "/r/x.rs"}
        }));
        assert!(matches!(&read, AgentEvent::ActivityStart { detail: Some(d), .. } if !d.is_task()));
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
        let shell = decode(json!({
            "hook_event_name": "preToolUse", "cwd": "/r",
            "tool_name": "shell", "tool_input": {"command": "cargo test"}
        }));
        assert!(
            matches!(shell, AgentEvent::ActivityStart { detail: Some(d), .. }
            if d.display() == "shell: cargo test")
        );
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
        assert!(decode_cursor_hook_payload(&json!({"hook_event_name": "stop"})).is_err());
        assert!(decode_cursor_hook_payload(
            &json!({"hook_event_name": "stop", "cwd": "", "workspace_roots": []})
        )
        .is_err());
        assert!(
            decode_cursor_hook_payload(&json!({"hook_event_name": "stop", "session_id": "s"}))
                .is_ok()
        );
    }

    #[test]
    fn unknown_event_bails_loudly() {
        // subagentStart/Stop are deliberately unregistered — they do not fire.
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
