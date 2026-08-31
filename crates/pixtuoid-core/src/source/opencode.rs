//! opencode source — HOOK-ONLY (no JSONL watcher), via a bundled TS plugin.
//!
//! opencode has NO config-level shell-command hook and stores every session in
//! SQLite with no tailable per-session transcript. Its ONLY external seam is the
//! **plugin** system: a TS plugin gets an `event` hook receiving the SAME
//! EventV2 stream the server's SSE endpoint serves, and pipes the events into
//! the `pixtuoid-hook` shim on stdin. Connecting opencode drops that plugin at
//! `<opencode-config>/plugins/pixtuoid.ts`, which opencode auto-discovers — so
//! there is NO `opencode.jsonc` edit (see `install/opencode.rs`).
//!
//! The forwarded envelope is opencode's own EventV2 shape:
//!
//! ```json
//! {"type":"session.created","properties":{"sessionID":"ses_…","info":{"id":"ses_…","directory":"/repo","parentID":"ses_…?","agent":"build","model":{…}}},"_pid":12345}
//! ```
//!
//! `type` is the BASE event name (the `.N` version suffix is persistence/sync
//! only), so the custom decoder claims every event by `type` and the shared
//! CC-shaped arms are unreachable. The upstream facts the decoding rests on:
//!
//! - The `ses_*` session id is a durable SQLite PRIMARY KEY, identical on every
//!   event of a session, so slots key on it rather than cwd. `info.directory` is
//!   the cwd, canonicalized by opencode (`/tmp` → `/private/tmp`).
//! - Subagents are first-class child SESSIONS: `task` calls
//!   `sessions.create({parentID})`, so the child's `session.created` carries
//!   `info.parentID` and no coalescing trick is needed.
//! - **Waiting** rides the `permission.asked`/`permission.v2.asked` EVENTS — the
//!   `permission.ask` PLUGIN hook is declared but never `.trigger`ed upstream.
//! - An abrupt exit kills the opencode process; the plugin stamps that pid
//!   (`_pid`) and `hook::HookPidWatch` ends every bound sprite when it dies
//!   (Unix only; Windows falls to the stale-sweep).
//!   `server.instance.disposed` carries only a `directory` (no session ids), so
//!   it is NOT decoded — the pid-watch covers instance teardown.

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::source::decoder::{ellipsize, MAX_DECODED_FIELD_CHARS};
use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

/// The opencode CLI source's registry name (its `SourceDescriptor.name`).
pub const SOURCE_NAME: &str = "opencode";

/// opencode's sub-agent dispatch tool — matched by NAME only, see
/// `oc_tool_detail`.
const SUBAGENT_TOOLS: &[&str] = &["task"];

/// Decode one opencode plugin envelope (already identified by
/// `_pixtuoid_source == "opencode"`). `type` is the base EventV2 name; the data
/// is under `properties`.
///
/// An unmapped `type` is a benign skip (`Ok(vec![])`), not an error: the
/// plugin's forward filter lives in JS, so the Rust decoder can't assert 1:1.
/// Upstream drift is caught by `check_upstream_drift.py`, not a bail here.
pub fn decode_oc_hook_payload(v: &Value) -> Result<Vec<AgentEvent>> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("opencode hook payload must be an object"))?;
    let event = obj
        .get("type")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("opencode payload missing type"))?;
    // `properties` is the EventV2 `data`.
    let props_val = obj.get("properties").unwrap_or(&Value::Null);
    let empty = serde_json::Map::new();
    let props = props_val.as_object().unwrap_or(&empty);

    match event {
        SESSION_CREATED => decode_session_lifecycle(props, false),
        SESSION_DELETED => decode_session_lifecycle(props, true),
        MESSAGE_PART_UPDATED => decode_tool_part(props),
        PERMISSION_ASKED | PERMISSION_V2_ASKED => decode_permission(props_val),
        _ => Ok(vec![]),
    }
}

const SESSION_CREATED: &str = "session.created";
const SESSION_DELETED: &str = "session.deleted";
const MESSAGE_PART_UPDATED: &str = "message.part.updated";
const PERMISSION_ASKED: &str = "permission.asked";
const PERMISSION_V2_ASKED: &str = "permission.v2.asked";

const STATUS_RUNNING: &str = "running";
const STATUS_COMPLETED: &str = "completed";
const STATUS_ERROR: &str = "error";

/// The part-state statuses that drive activity. Exported because the dispatch
/// below ends `_ => Ok(vec![])`: rename `running` upstream and every opencode
/// tool activity silently stops. Pinned by the tool-part decode tests.
#[cfg(test)]
pub(crate) const DECODED_PART_STATUSES: &[&str] = &[STATUS_RUNNING, STATUS_COMPLETED, STATUS_ERROR];

/// The hook event types this decoder turns into events — this module's row in
/// the drift surface. Pinned to the arms above by
/// `the_decoded_event_set_is_exactly_what_the_arms_match`.
///
/// Test-gated because the surface emitter is its only reader: the ARMS are what
/// production dispatches on, and a second copy of the vocabulary must not be
/// something the shipped crate can read and drift against.
#[cfg(test)]
pub(crate) const DECODED_EVENTS: &[&str] = &[
    SESSION_CREATED,
    SESSION_DELETED,
    MESSAGE_PART_UPDATED,
    PERMISSION_ASKED,
    PERMISSION_V2_ASKED,
];

/// `session.created` / `session.deleted` → `{sessionID, info: SessionInfo}`,
/// where `info.id` is the stable `ses_*` key, `info.directory` the cwd, and
/// `info.parentID` (only on a `task`-spawned subagent) the parent link.
fn decode_session_lifecycle(
    props: &serde_json::Map<String, Value>,
    deleted: bool,
) -> Result<Vec<AgentEvent>> {
    let info = props
        .get("info")
        .and_then(|i| i.as_object())
        .ok_or_else(|| anyhow!("opencode session event missing info"))?;
    let session_id = info
        .get("id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("opencode session info missing/empty id"))?;
    let agent_id = AgentId::from_parts(SOURCE_NAME, session_id);
    let parent = info
        .get("parentID")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty());

    if deleted {
        return Ok(vec![AgentEvent::SessionEnd {
            agent_id,
            as_child: parent.is_some(),
        }]);
    }

    let cwd = info
        .get("directory")
        .and_then(|s| s.as_str())
        .unwrap_or_default();
    let mut out = vec![AgentEvent::SessionStart {
        agent_id,
        source: SOURCE_NAME.to_string(),
        session_id: session_id.to_string(),
        cwd: cwd.into(),
        parent_id: parent.map(|p| AgentId::from_parts(SOURCE_NAME, p)),
    }];
    // `info.model` is `{id, providerID}`, the id being the raw model slug.
    // session.created is opencode's ONE model carrier — a mid-session switch
    // has no wire signal.
    if let Some(model) = info
        .get("model")
        .and_then(|m| m.get("id"))
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
    {
        out.push(AgentEvent::ModelInfo {
            agent_id,
            model: Some(ellipsize(model, MAX_DECODED_FIELD_CHARS)),
            effort: None,
        });
    }
    Ok(out)
}

/// `message.part.updated` → `{sessionID, part}`. Only `part.type == "tool"`
/// drives activity. `state.status`: `running` → `ActivityStart`,
/// `completed`/`error` → `ActivityEnd`, `pending` → skipped. Keyed on the real
/// `callID` so a future JSONL twin would dedup.
fn decode_tool_part(props: &serde_json::Map<String, Value>) -> Result<Vec<AgentEvent>> {
    let session_id = props
        .get("sessionID")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("opencode message.part.updated missing sessionID"))?;
    let part = match props.get("part").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return Ok(vec![]),
    };
    if part.get("type").and_then(|t| t.as_str()) != Some("tool") {
        return Ok(vec![]);
    }
    let status = part
        .get("state")
        .and_then(|s| s.as_object())
        .and_then(|s| s.get("status"))
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let call_id = part
        .get("callID")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let agent_id = AgentId::from_parts(SOURCE_NAME, session_id);
    let identity = oc_identity(agent_id, session_id);
    match status {
        STATUS_RUNNING => {
            let tool = part
                .get("tool")
                .and_then(|t| t.as_str())
                .unwrap_or_else(|| {
                    crate::source::drift::missing_field(
                        SOURCE_NAME,
                        "message.part.updated",
                        "tool",
                    );
                    "?"
                });
            let input = part.get("state").and_then(|s| s.get("input"));
            Ok(vec![
                identity,
                AgentEvent::ActivityStart {
                    agent_id,
                    tool_use_id: call_id,
                    detail: Some(oc_tool_detail(tool, input)),
                },
            ])
        }
        STATUS_COMPLETED | STATUS_ERROR => Ok(vec![
            identity,
            AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: call_id,
            },
        ]),
        // `pending` = queued, not yet executing.
        _ => Ok(vec![]),
    }
}

/// `permission.asked` / `permission.v2.asked` → `Waiting`. The request fields
/// vary by opencode version, so the key order follows the REAL upstream shapes:
/// `action` is the `permission.v2.asked` verb, `permission` the v1
/// `PermissionRequest` name; the rest are tolerated fallbacks.
fn decode_permission(props: &Value) -> Result<Vec<AgentEvent>> {
    let session_id = props
        .get("sessionID")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("opencode permission event missing sessionID"))?;
    let agent_id = AgentId::from_parts(SOURCE_NAME, session_id);
    const KEYS: &[&str] = &["action", "permission", "title", "pattern", "type", "tool"];
    let reason = crate::source::decoder::first_present_str(props, KEYS)
        .filter(|s| !s.is_empty())
        .map(|s| ellipsize(s, MAX_DECODED_FIELD_CHARS))
        .unwrap_or_else(|| "permission".to_string());
    Ok(vec![
        oc_identity(agent_id, session_id),
        AgentEvent::Waiting {
            agent_id,
            reason,
            tool_use_id: None,
        },
    ])
}

/// The `Identity` prepended ahead of a tool/permission activity event.
/// `cwd: None` — those events carry only `sessionID`, so the reducer back-fills
/// cwd first-wins from the session's `session.created`.
fn oc_identity(agent_id: AgentId, session_id: &str) -> AgentEvent {
    AgentEvent::identity(agent_id, SOURCE_NAME, session_id, None)
}

/// opencode-side tool detail: the `task` dispatch tool (by NAME) →
/// `ToolDetail::Task`; everything else → a `"name: target"` display, the target
/// pulled from the tool `input` record (opencode builtins: bash→`command`,
/// read/edit/write→`filePath`, grep/glob→`pattern`, webfetch→`url`).
fn oc_tool_detail(tool: &str, input: Option<&Value>) -> ToolDetail {
    // NAME-ONLY, deliberately NOT the CC `subagent_type`-presence signal:
    // opencode's tool `ActivityStart` carries a real `callID`, so a
    // model-authored `subagent_type` on an ORDINARY tool would seed the
    // reducer's `active_tasks` and, on drain, cascade the parent's real `ses_*`
    // children out (the lifecycle-authority trap).
    if SUBAGENT_TOOLS.contains(&tool) {
        return ToolDetail::Task;
    }
    const KEYS: &[&str] = &[
        "command",
        "filePath",
        "file_path",
        "path",
        "pattern",
        "url",
        "query",
    ];
    crate::source::decoder::generic_keyed_detail(tool, input, KEYS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exported set IS the arms: every member reaches a real arm and decodes
    /// to at least one event. The arms dispatch on the SAME consts, so a value
    /// can no longer drift; what this catches is a member dropped from the set,
    /// or kept in it after its arm went away.
    #[test]
    fn the_decoded_event_set_is_exactly_what_the_arms_match() {
        let payload = |ev: &str| match ev {
            MESSAGE_PART_UPDATED => serde_json::json!({"type": ev, "properties": {
                "sessionID": "ses_x",
                "part": {"type": "tool", "callID": "c", "tool": "bash",
                         "state": {"status": "running"}}}}),
            PERMISSION_ASKED | PERMISSION_V2_ASKED => {
                serde_json::json!({"type": ev, "properties": {"sessionID": "ses_x"}})
            }
            _ => serde_json::json!({"type": ev, "properties": {
                "sessionID": "ses_m", "info": {"id": "ses_m", "directory": "/repo"}}}),
        };
        for ev in DECODED_EVENTS {
            let got = decode_oc_hook_payload(&payload(ev)).expect("a decoded event decodes");
            assert!(!got.is_empty(), "{ev} must reach a real arm");
        }
        assert!(decode_oc_hook_payload(&payload("session.archived"))
            .expect("an unhandled type is not an error")
            .is_empty());
    }

    #[test]
    fn session_created_surfaces_the_model_slug() {
        let v = serde_json::json!({
            "type": "session.created",
            "properties": {"sessionID": "ses_m", "info": {
                "id": "ses_m", "directory": "/repo",
                "model": {"id": "deepseek-v4-flash-free", "providerID": "opencode"}
            }}
        });
        let evs = decode_oc_hook_payload(&v).unwrap();
        assert!(
            evs.iter().any(|e| matches!(e, AgentEvent::ModelInfo { model: Some(m), effort: None, .. } if m == "deepseek-v4-flash-free")),
            "session.created model must surface, got {evs:?}"
        );
        let v = serde_json::json!({
            "type": "session.created",
            "properties": {"sessionID": "ses_n", "info": {"id": "ses_n", "directory": "/repo"}}
        });
        let evs = decode_oc_hook_payload(&v).unwrap();
        assert!(evs
            .iter()
            .any(|e| matches!(e, AgentEvent::SessionStart { .. })));
        assert!(!evs
            .iter()
            .any(|e| matches!(e, AgentEvent::ModelInfo { .. })));
    }
    use crate::source::decoder::MAX_TOOL_TARGET_CHARS;
    use serde_json::json;

    fn decode_all(v: Value) -> Vec<AgentEvent> {
        decode_oc_hook_payload(&v).expect("decodes")
    }

    /// The payload's MAIN event — the last decoded (activity arms prepend Identity).
    fn decode(v: Value) -> AgentEvent {
        decode_all(v).pop().expect("at least one event")
    }

    /// FIRST event — session.created piggybacks a ModelInfo behind the
    /// SessionStart these lifecycle tests inspect.
    fn decode_first(v: Value) -> AgentEvent {
        decode_all(v)
            .into_iter()
            .next()
            .expect("at least one event")
    }

    #[test]
    fn session_created_keys_on_stable_session_id() {
        let ev = decode_first(json!({
            "type": "session.created",
            "properties": {
                "sessionID": "ses_140762860ffe0d",
                "info": {
                    "id": "ses_140762860ffe0d",
                    "slug": "shiny-canyon",
                    "projectID": "13e2248518abbe",
                    "directory": "/private/tmp/oc-capture/ws",
                    "agent": "build",
                    "model": {"id": "deepseek-v4-flash-free", "providerID": "opencode"}
                }
            },
            "_pid": 13358
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
                    AgentId::from_parts(SOURCE_NAME, "ses_140762860ffe0d")
                );
                assert_eq!(session_id, "ses_140762860ffe0d");
                assert_eq!(cwd, std::path::PathBuf::from("/private/tmp/oc-capture/ws"));
                assert_eq!(parent_id, None);
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn subagent_session_created_links_to_its_parent_session() {
        let ev = decode_first(json!({
            "type": "session.created",
            "properties": { "sessionID": "ses_child", "info": {
                "id": "ses_child", "directory": "/repo", "parentID": "ses_parent"
            }}
        }));
        match ev {
            AgentEvent::SessionStart {
                agent_id,
                parent_id,
                ..
            } => {
                assert_eq!(agent_id, AgentId::from_parts(SOURCE_NAME, "ses_child"));
                assert_eq!(
                    parent_id,
                    Some(AgentId::from_parts(SOURCE_NAME, "ses_parent"))
                );
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn a_subagent_does_not_coalesce_with_its_parent() {
        let parent = decode(json!({"type": "session.created",
            "properties": {"info": {"id": "ses_p", "directory": "/r"}}}));
        let child = decode(json!({"type": "session.created",
            "properties": {"info": {"id": "ses_c", "directory": "/r", "parentID": "ses_p"}}}));
        assert_ne!(parent.agent_id(), child.agent_id());
    }

    #[test]
    fn spoofed_subagent_type_on_an_ordinary_tool_is_not_a_task() {
        let spoof = json!({"command": "ls", "subagent_type": "general"});
        assert!(!oc_tool_detail("bash", Some(&spoof)).is_task());
        assert!(oc_tool_detail("task", Some(&json!({"description": "go"}))).is_task());
    }

    #[test]
    fn running_tool_part_is_activity_start_keyed_on_callid() {
        let events = decode_all(json!({
            "type": "message.part.updated",
            "properties": {
                "sessionID": "ses_x",
                "part": {
                    "id": "prt_1", "sessionID": "ses_x", "messageID": "msg_1",
                    "type": "tool", "callID": "call_abc", "tool": "bash",
                    "state": {"status": "running", "input": {"command": "ls -la"},
                              "time": {"start": 1}}
                }
            }
        }));
        assert_eq!(events.len(), 2, "Identity + ActivityStart");
        assert!(
            matches!(&events[0], AgentEvent::Identity { session_id, cwd, .. }
            if session_id == "ses_x" && cwd.is_none())
        );
        match &events[1] {
            AgentEvent::ActivityStart {
                agent_id,
                tool_use_id,
                detail,
            } => {
                assert_eq!(*agent_id, AgentId::from_parts(SOURCE_NAME, "ses_x"));
                assert_eq!(tool_use_id.as_deref(), Some("call_abc"));
                assert_eq!(detail.as_ref().unwrap().display(), "bash: ls -la");
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn completed_and_error_tool_parts_are_activity_end() {
        for status in ["completed", "error"] {
            let ev = decode(json!({
                "type": "message.part.updated",
                "properties": {"sessionID": "ses_x", "part": {
                    "type": "tool", "callID": "call_abc", "tool": "bash",
                    "state": {"status": status}
                }}
            }));
            assert!(
                matches!(ev, AgentEvent::ActivityEnd { tool_use_id, .. }
                if tool_use_id.as_deref() == Some("call_abc")),
                "{status} must be ActivityEnd"
            );
        }
    }

    #[test]
    fn pending_tool_part_and_non_tool_parts_are_skipped() {
        assert!(decode_all(json!({"type": "message.part.updated", "properties": {
            "sessionID": "ses_x",
            "part": {"type": "tool", "callID": "c", "tool": "bash", "state": {"status": "pending"}}
        }}))
        .is_empty());
        for t in ["text", "reasoning", "step-start", "step-finish"] {
            assert!(
                decode_all(json!({"type": "message.part.updated", "properties": {
                    "sessionID": "ses_x", "part": {"type": t}
                }}))
                .is_empty(),
                "{t} part must be skipped"
            );
        }
    }

    #[test]
    fn task_tool_maps_to_delegating() {
        let ev = decode(json!({
            "type": "message.part.updated", "properties": {"sessionID": "ses_x", "part": {
                "type": "tool", "callID": "c", "tool": "task",
                "state": {"status": "running", "input": {"description": "investigate X"}}
            }}
        }));
        assert!(matches!(&ev, AgentEvent::ActivityStart { detail: Some(d), .. } if d.is_task()));
    }

    #[test]
    fn spoofed_subagent_type_input_does_not_make_a_task() {
        let ev = decode(json!({
            "type": "message.part.updated", "properties": {"sessionID": "ses_x", "part": {
                "type": "tool", "callID": "c", "tool": "spawn",
                "state": {"status": "running", "input": {"subagent_type": "explore"}}
            }}
        }));
        assert!(matches!(&ev, AgentEvent::ActivityStart { detail: Some(d), .. } if !d.is_task()));
    }

    #[test]
    fn permission_asked_maps_to_waiting() {
        for (ty, props, want) in [
            (
                "permission.v2.asked",
                json!({"sessionID": "ses_x", "action": "bash", "resources": ["rm -rf build"]}),
                "bash",
            ),
            (
                "permission.asked",
                json!({"sessionID": "ses_x", "permission": "edit"}),
                "edit",
            ),
        ] {
            let events = decode_all(json!({"type": ty, "properties": props}));
            assert_eq!(events.len(), 2, "{ty}: Identity + Waiting");
            match &events[1] {
                AgentEvent::Waiting {
                    agent_id, reason, ..
                } => {
                    assert_eq!(*agent_id, AgentId::from_parts(SOURCE_NAME, "ses_x"));
                    assert_eq!(reason, want);
                }
                other => panic!("{ty}: expected Waiting, got {other:?}"),
            }
        }
    }

    #[test]
    fn permission_without_a_label_falls_back_to_generic_reason() {
        let ev = decode(json!({"type": "permission.asked", "properties": {"sessionID": "ses_x"}}));
        assert!(matches!(ev, AgentEvent::Waiting { reason, .. } if reason == "permission"));
    }

    #[test]
    fn session_deleted_root_is_a_top_level_end() {
        let ev = decode(json!({"type": "session.deleted",
            "properties": {"sessionID": "ses_x", "info": {"id": "ses_x", "directory": "/r"}}}));
        assert!(matches!(
            ev,
            AgentEvent::SessionEnd {
                as_child: false,
                ..
            }
        ));
    }

    #[test]
    fn session_deleted_child_ends_as_a_child() {
        let ev = decode(json!({"type": "session.deleted",
            "properties": {"info": {"id": "ses_c", "directory": "/r", "parentID": "ses_p"}}}));
        match ev {
            AgentEvent::SessionEnd { agent_id, as_child } => {
                assert_eq!(agent_id, AgentId::from_parts(SOURCE_NAME, "ses_c"));
                assert!(as_child, "a child session delete ends as_child");
            }
            other => panic!("expected SessionEnd, got {other:?}"),
        }
    }

    #[test]
    fn all_events_for_one_session_share_one_agent_id() {
        let events = [
            json!({"type": "session.created", "properties": {"info": {"id": "ses_1", "directory": "/p"}}}),
            json!({"type": "message.part.updated", "properties": {"sessionID": "ses_1", "part": {
                "type": "tool", "callID": "c1", "tool": "read",
                "state": {"status": "running", "input": {"filePath": "x.rs"}}}}}),
            json!({"type": "message.part.updated", "properties": {"sessionID": "ses_1", "part": {
                "type": "tool", "callID": "c1", "tool": "read", "state": {"status": "completed"}}}}),
            json!({"type": "permission.asked", "properties": {"sessionID": "ses_1", "title": "x"}}),
            json!({"type": "session.deleted", "properties": {"info": {"id": "ses_1", "directory": "/p"}}}),
        ];
        let ids: std::collections::BTreeSet<_> = events
            .iter()
            .flat_map(|v| decode_oc_hook_payload(v).unwrap())
            .map(|e| e.agent_id())
            .collect();
        assert_eq!(
            ids.len(),
            1,
            "all events of one session coalesce to one AgentId"
        );
    }

    #[test]
    fn unmapped_event_types_are_skipped_not_errored() {
        for ty in [
            "session.idle",
            "session.updated",
            "message.updated",
            "session.next.step.started",
            "server.instance.disposed",
        ] {
            assert!(
                decode_oc_hook_payload(&json!({"type": ty, "properties": {}}))
                    .unwrap()
                    .is_empty(),
                "{ty} must skip, not error"
            );
        }
    }

    #[test]
    fn malformed_payloads_are_errors_not_panics() {
        assert!(decode_oc_hook_payload(&json!("a string")).is_err());
        assert!(decode_oc_hook_payload(&json!(42)).is_err());
        assert!(
            decode_oc_hook_payload(&json!({"properties": {}})).is_err(),
            "missing type"
        );
        assert!(decode_oc_hook_payload(
            &json!({"type": "session.created", "properties": {"info": {}}})
        )
        .is_err());
        assert!(
            decode_oc_hook_payload(&json!({"type": "session.created", "properties": {}})).is_err()
        );
    }

    #[test]
    fn tool_without_target_or_name_degrades_cleanly() {
        let ev = decode(json!({"type": "message.part.updated", "properties": {
            "sessionID": "ses_x",
            "part": {"type": "tool", "callID": "c", "tool": "bash", "state": {"status": "running"}}
        }}));
        assert!(
            matches!(ev, AgentEvent::ActivityStart { detail: Some(d), .. } if d.display() == "bash")
        );
    }

    #[test]
    fn tool_part_event_without_a_part_object_is_skipped() {
        assert!(
            decode_all(json!({
                "type": "message.part.updated",
                "properties": {"sessionID": "ses_x"}
            }))
            .is_empty(),
            "a part-less message.part.updated must skip, not error"
        );
        assert!(
            decode_oc_hook_payload(&json!({
                "type": "message.part.updated",
                "properties": {"sessionID": "ses_x", "part": 42}
            }))
            .expect("scalar part is a skip, not an error")
            .is_empty(),
            "a scalar `part` must skip, not error"
        );
    }

    #[test]
    fn running_tool_part_without_a_tool_field_degrades_to_question_mark_detail() {
        let events = decode_all(json!({
            "type": "message.part.updated",
            "properties": {
                "sessionID": "ses_x",
                "part": {"type": "tool", "callID": "call_x", "state": {"status": "running"}}
            }
        }));
        assert_eq!(events.len(), 2, "Identity + ActivityStart");
        match &events[1] {
            AgentEvent::ActivityStart {
                tool_use_id,
                detail,
                ..
            } => {
                assert_eq!(tool_use_id.as_deref(), Some("call_x"));
                assert_eq!(
                    detail.as_ref().unwrap().display(),
                    "?",
                    "a tool-less running part degrades to the \"?\" display"
                );
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn long_tool_target_is_truncated_at_the_decode_boundary() {
        let long = "x".repeat(MAX_TOOL_TARGET_CHARS * 3);
        let ev = decode(json!({"type": "message.part.updated", "properties": {
            "sessionID": "ses_x", "part": {"type": "tool", "callID": "c", "tool": "bash",
                "state": {"status": "running", "input": {"command": long}}}
        }}));
        match ev {
            AgentEvent::ActivityStart {
                detail: Some(d), ..
            } => {
                assert!(d.display().starts_with("bash: "));
                assert!(d.display().ends_with('…'));
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }
}
