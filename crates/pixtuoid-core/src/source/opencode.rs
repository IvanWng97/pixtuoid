//! opencode source — HOOK-ONLY (no JSONL watcher), via a bundled TS plugin.
//!
//! opencode (github.com/anomalyco/opencode, a Bun/TS agent CLI+TUI) has NO
//! config-level shell-command hook (unlike CC/CodeWhale/Reasonix) and stores
//! every session in SQLite (`~/.local/share/opencode/opencode.db`) with no
//! tailable per-session transcript. Its ONLY external seam is the **plugin**
//! system: a TS plugin loaded via the config `plugin` array gets an `event`
//! hook that receives the SAME EventV2 stream the server's SSE endpoint serves,
//! dir-scoped to the session's directory, and fires on the default `opencode`
//! TUI/`run` (an in-process worker server — no `opencode serve` needed). The
//! plugin gets `Bun.$`, so it pipes the events pixtuoid maps into the existing
//! `pixtuoid-hook` shim on stdin (plain mode, no `--event`, like CodeWhale's
//! subagent hooks). Connecting opencode in the in-TUI Sources panel (`s`) DROPS
//! that bundled plugin at `<opencode-config>/plugins/pixtuoid.ts` — opencode
//! auto-discovers `<config>/plugins/*.{ts,js}`, so there is NO `opencode.jsonc`
//! edit (see `install/opencode.rs`).
//!
//! So this is the FIRST integration whose install target ships a CODE artifact
//! (a `.ts` file) rather than a declarative config block — the plugin IS the
//! shim caller. The envelope the plugin forwards (stamped `_pixtuoid_source:
//! "opencode"` by the shim) is opencode's own EventV2 shape:
//!
//! ```json
//! {"type":"session.created","properties":{"sessionID":"ses_…","info":{"id":"ses_…","directory":"/repo","parentID":"ses_…?","agent":"build","model":{…}}},"_pid":12345}
//! ```
//!
//! `type` is the BASE event name (the `.N` version suffix is persistence/sync
//! only — `event-v2-bridge.ts` delivers `event.type` to listeners), so the
//! custom decoder claims every event by `type` (alien envelope, no CC
//! `hook_event_name`) and the shared CC-shaped arms are unreachable. Load-bearing
//! decisions, contrasted with CodeWhale:
//!
//! - **Key on the stable `ses_*` session id, NOT cwd.** opencode's session id is
//!   a durable SQLite PRIMARY KEY, identical on every event of a session (no
//!   CodeWhale-style inconsistency), so `AgentId::from_parts("opencode",
//!   session_id)` is the safe key. `info.directory` is the cwd (canonicalized by
//!   opencode — `/tmp` → `/private/tmp`), used for the label only.
//! - **Subagents are first-class child SESSIONS.** opencode's `task` tool calls
//!   `sessions.create({parentID})`, so a child's `session.created` carries
//!   `info.parentID` (the parent's session id). The child is keyed on its OWN
//!   `ses_*` and parent-linked to `parentID` — both distinct sessions, so no
//!   coalescing trick is needed (unlike CC/CodeWhale where the child shares the
//!   parent's transcript/workspace). The parent ALSO flashes "Delegating" via
//!   its `task` tool part → `ToolDetail::Task`.
//! - **Waiting** rides `permission.asked`/`permission.v2.asked` (the
//!   `permission.ask` PLUGIN hook is declared but never `.trigger`ed upstream —
//!   the EVENT is the signal). Resolved by the gated tool's end / SessionEnd via
//!   the existing reducer machinery.
//! - **Tool activity** rides `message.part.updated` for `part.type == "tool"`:
//!   `state.status == "running"` → `ActivityStart` (keyed on the real `callID`),
//!   `completed`/`error` → `ActivityEnd`, `pending` → skipped. The plugin filters
//!   OUT the chatty text/reasoning/step parts so the socket sees ~one
//!   connection per tool-state change, not per token.
//! - **Exit profile.** A clean per-session close fires `session.deleted` →
//!   `SessionEnd`. An abrupt exit / TUI quit kills the opencode process; the
//!   plugin stamps that pid (`_pid`, `process.pid` — the in-process worker shares
//!   the CLI's pid) and the daemon's `hook::HookPidWatch` ends every bound sprite
//!   when it dies (Unix only, like CodeWhale; Windows falls to the stale-sweep).
//!   `server.instance.disposed` carries only a `directory` (no session ids), so
//!   it is NOT decoded — the pid-watch covers instance teardown.

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::source::decoder::{ellipsize, MAX_DECODED_FIELD_CHARS};
use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

pub const SOURCE_NAME: &str = "opencode";

/// opencode's sub-agent dispatch tool. opencode's `task` tool calls
/// `sessions.create({parentID})` to spawn a child session; the parent's `task`
/// tool part maps to `ToolDetail::Task` so the parent reads "Delegating" while
/// it runs. The CHILD gets its own sprite via its `session.created` (which
/// carries `info.parentID`). Detection is NAME-ONLY — `task` is opencode's
/// stable builtin with no CC-style rename to survive, and (unlike CC) opencode's
/// `ActivityStart` carries a real `callID`, so a `subagent_type`-presence signal
/// would let a model-authored key on an ordinary tool seed `active_tasks` and
/// cascade real children out (the lifecycle-authority trap — see `oc_tool_detail`).
const SUBAGENT_TOOLS: &[&str] = &["task"];

/// Decode one opencode plugin envelope (already identified by
/// `_pixtuoid_source == "opencode"`). `type` is the base EventV2 name; the data
/// is under `properties`. Mapped events:
///
/// - `session.created` → `SessionStart` (keyed on `info.id`; `info.parentID` ⇒
///   a child sprite; Identity rides the SessionStart)
/// - `message.part.updated` (tool part) → `Identity` + `ActivityStart`/`End`
///   (keyed on `callID`; `running` starts, `completed`/`error` ends, `pending`
///   skipped)
/// - `permission.asked` / `permission.v2.asked` → `Identity` + `Waiting`
/// - `session.deleted` → `SessionEnd` (`as_child` iff `info.parentID` present)
/// - any other `type` → skipped (`Ok(vec![])`): the plugin forwards a filtered
///   set, but its filter lives in JS, so the Rust decoder can't assert 1:1 —
///   an unmapped-but-forwarded event (a `pending` tool part, a future type) is
///   a benign skip, not a hard error. Upstream drift is caught by
///   `check_upstream_drift.py` + the plugin filter, not a bail here.
pub fn decode_oc_hook_payload(v: &Value) -> Result<Vec<AgentEvent>> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("opencode hook payload must be an object"))?;
    let event = obj
        .get("type")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("opencode payload missing type"))?;
    // `properties` is the EventV2 `data`. Derive it ONCE from a single lookup so
    // the two views can't drift: `props_val` is the raw value (fed to
    // `decode_permission`, which scans it via `first_present_str`), `props` its
    // object view with an empty-object fallback so a payload-shape surprise
    // degrades to "missing field" rather than panicking.
    let props_val = obj.get("properties").unwrap_or(&Value::Null);
    let empty = serde_json::Map::new();
    let props = props_val.as_object().unwrap_or(&empty);

    match event {
        "session.created" => decode_session_lifecycle(props, false),
        "session.deleted" => decode_session_lifecycle(props, true),
        "message.part.updated" => decode_tool_part(props),
        "permission.asked" | "permission.v2.asked" => decode_permission(props_val),
        // The plugin only forwards the mapped set + tool parts; anything else
        // (a pending tool part, a not-yet-mapped type) is skipped, not bailed.
        _ => Ok(vec![]),
    }
}

/// `session.created` / `session.deleted` → `{sessionID, info: SessionInfo}`.
/// `info.id` is the stable `ses_*` key; `info.directory` the cwd;
/// `info.parentID` (present only for a `task`-spawned subagent) the parent link.
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
        // A child session delete (`parentID` present) ends `as_child: true` so
        // the reducer's child ledger / scope cascade handle the parent link;
        // a root delete ends `as_child: false`.
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
    // Burn-tier observation: `info.model` is `{id, providerID}` — the id is
    // the raw model slug (e.g. "deepseek-v4-flash-free"). session.created is
    // opencode's ONE model carrier (fires once per session; a mid-session
    // switch has no wire signal — accepted, last-seen-wins downstream).
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
/// drives activity (the plugin should pre-filter, but we re-check defensively).
/// `state.status`: `running` → `ActivityStart`, `completed`/`error` →
/// `ActivityEnd`, `pending` → skipped (the running transition is the real
/// start). Keyed on the real `callID` so a future JSONL twin would dedup; under
/// the single hook transport it's harmless.
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
    // Non-tool parts (text/reasoning/step-*) carry no activity — skip.
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
        "running" => {
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
        "completed" | "error" => Ok(vec![
            identity,
            AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: call_id,
            },
        ]),
        // `pending` (queued, not yet executing) and any unknown status: skip.
        _ => Ok(vec![]),
    }
}

/// `permission.asked` / `permission.v2.asked` → `Waiting`. The request fields
/// vary by opencode version; derive a short human reason defensively. The keys
/// are ordered by the REAL upstream shapes: `action` is the `permission.v2.asked`
/// verb (`Request.fields` = `{sessionID, action, resources, …}`, permission.ts),
/// `permission` the v1 `PermissionRequest` name; `title`/`pattern`/`type`/`tool`
/// are tolerated fallbacks. Else a generic label. cwd is unknown here (the
/// request carries only `sessionID`), so the prepended `Identity` registers
/// ordinal-labeled if the session is unknown — back-filled when its
/// `session.created` arrives (#221).
fn decode_permission(props: &Value) -> Result<Vec<AgentEvent>> {
    let session_id = props
        .get("sessionID")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("opencode permission event missing sessionID"))?;
    let agent_id = AgentId::from_parts(SOURCE_NAME, session_id);
    // Per-source reason vocabulary; the shared scan lives in the decoder. Keep
    // the non-empty filter + ellipsize cap chained here.
    const KEYS: &[&str] = &["action", "permission", "title", "pattern", "type", "tool"];
    let reason = crate::source::decoder::first_present_str(props, KEYS)
        .filter(|s| !s.is_empty())
        .map(|s| ellipsize(s, MAX_DECODED_FIELD_CHARS))
        .unwrap_or_else(|| "permission".to_string());
    Ok(vec![
        oc_identity(agent_id, session_id),
        AgentEvent::Waiting { agent_id, reason },
    ])
}

/// The `Identity` prepended ahead of a tool/permission activity event (#221):
/// hook-only, so a slot the proof-of-life pre-pass synthesizes mid-turn has no
/// JSONL back-fill. `cwd: None` — tool/permission events carry only `sessionID`,
/// not `directory`; the reducer back-fills cwd first-wins from the session's
/// `session.created`.
fn oc_identity(agent_id: AgentId, session_id: &str) -> AgentEvent {
    AgentEvent::Identity {
        agent_id,
        source: SOURCE_NAME.to_string(),
        session_id: session_id.to_string(),
        cwd: None,
        pid: None,
    }
}

/// opencode-side tool detail: the `task` dispatch tool (by NAME) →
/// `ToolDetail::Task` (parent reads "Delegating"); everything else → a
/// `"name: target"` display, the target pulled from the tool `input` record
/// (opencode builtins: bash→`command`, read/edit/write→`filePath`,
/// grep/glob→`pattern`, webfetch→`url`).
fn oc_tool_detail(tool: &str, input: Option<&Value>) -> ToolDetail {
    // NAME-ONLY subagent detection: opencode's dispatch is the stably-named
    // `task` tool. Deliberately NOT the CC `subagent_type`-presence signal —
    // opencode's tool `ActivityStart` carries a real `callID`, so a model-authored
    // `subagent_type` on an ORDINARY tool's input would seed the reducer's
    // `active_tasks` and, on drain, cascade the parent's real `ses_*` children out
    // (the lifecycle-authority trap). opencode subagents are first-class SESSIONS
    // (`session.created` `parentID`), never this Task path, and `task` has no
    // CC-style rename to survive — so presence detection would buy only the spoof.
    if SUBAGENT_TOOLS.contains(&tool) {
        return ToolDetail::Task;
    }
    // Per-source target vocabulary; the shared scan lives in the decoder, the
    // last-mile assembly (name + `: target` with the matching caps) in
    // `generic_tool_display`.
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

    // ---- burn-tier observations (ModelInfo) ----

    #[test]
    fn session_created_surfaces_the_model_slug() {
        // Byte-real shape: info.model = {id, providerID}.
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
        // Model-less create still registers, no phantom ModelInfo.
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
    /// SessionStart these lifecycle tests inspect (tool parts keep `decode`:
    /// their Identity rides FIRST, activity last).
    fn decode_first(v: Value) -> AgentEvent {
        decode_all(v)
            .into_iter()
            .next()
            .expect("at least one event")
    }

    // Real shape, captured from opencode.db's event table:
    // session.created.1 → {sessionID, info:{id, slug, projectID, directory, …}}.
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
        // opencode's task tool spawns sessions.create({parentID}); the child's
        // session.created carries info.parentID. Child keyed on its OWN ses_*,
        // parent-linked to the parent session — both distinct sprites.
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
        // lifecycle-authority: a model-authored `subagent_type` on an ORDINARY
        // tool must NOT read as a Task dispatch — opencode emits a real callID, so
        // a Task here would seed active_tasks and cascade real ses_* children out.
        // Only the `task` tool NAME dispatches (opencode has no CC-style rename).
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
        // pending = queued, not executing; running is the real start.
        assert!(decode_all(json!({"type": "message.part.updated", "properties": {
            "sessionID": "ses_x",
            "part": {"type": "tool", "callID": "c", "tool": "bash", "state": {"status": "pending"}}
        }}))
        .is_empty());
        // text/reasoning/step parts carry no activity (the plugin filters them,
        // but the decoder re-checks).
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
        // Regression (inverts the old CC-lesson test): opencode's dispatch is the
        // stably-named `task` tool and its ActivityStart carries a real callID, so
        // a model-authored `subagent_type` on an ORDINARY tool must NOT read as a
        // Task — else it seeds `active_tasks` and cascades real `ses_*` children
        // out (the lifecycle-authority trap). NAME-ONLY detection, by design.
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
        // Real upstream shapes: permission.v2.asked carries `action` (the verb);
        // the v1 permission.asked carries `permission` (the name).
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
                AgentEvent::Waiting { agent_id, reason } => {
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
        // A subagent session delete drives the scope cascade.
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
        // The plugin forwards a filtered set, but its filter is in JS — an
        // unmapped-but-forwarded type is a benign skip (drift is caught by the
        // upstream-drift script + the plugin filter, not a bail here).
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
        // session.created without a usable id is malformed (can't key it).
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
        // A message.part.updated carrying a sessionID but NO `part` (or a scalar
        // part) is a benign skip — not the missing-sessionID error path, and not
        // an activity event. Distinguishes the part-missing skip (line 180) from
        // the missing-sessionID bail.
        assert!(
            decode_all(json!({
                "type": "message.part.updated",
                "properties": {"sessionID": "ses_x"}
            }))
            .is_empty(),
            "a part-less message.part.updated must skip, not error"
        );
        // A non-object part (`part.as_object()` is None) takes the same branch.
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
        // status==running but the part has NO `tool` key: the unwrap_or_else
        // drift fallback substitutes "?" as the tool name and still builds a
        // real ActivityStart (Identity + ActivityStart), keyed on the callID.
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
