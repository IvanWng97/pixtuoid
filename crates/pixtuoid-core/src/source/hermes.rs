//! Hermes Agent (Nous Research) source — HOOK-ONLY: pixtuoid never spawns the user's
//! agent and Hermes's on-disk sessions are not tailable JSONL, so the shell hooks in
//! `~/.hermes/config.yaml` (or `$HERMES_HOME/config.yaml`) are the only seam a
//! passive observer can reach.
//!
//! Keyed on `session_id`, not the workspace: a user may run several Hermes sessions
//! in ONE project and cwd-keying would merge them (the Cursor lesson).
//!
//! The envelope reuses CC's `hook_event_name` field NAME but with snake_case VALUES
//! alien to the shared CC-shaped arms, so per the `HookDecoding::custom` contract the
//! decoder claims EVERY event (`.map(Some)`, never `Ok(None)`).
//!
//! `subagent_stop` is deliberately absent from `HERMES_EVENTS`: the SHELL-hook payload
//! carries a parent id but NO child session/agent id, so a decode could only end a
//! child that was never started. Hermes's Python PLUGIN API does define a
//! `subagent_start` with a child id, but plugin hooks are in-process callbacks that
//! never reach a shell command's stdin — don't "fix" this by modelling them.

use std::path::PathBuf;

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

/// The Hermes CLI source's registry name (its `SourceDescriptor.name`).
pub const SOURCE_NAME: &str = "hermes";

/// The Hermes home dir (`config.yaml` lives directly in it), mirroring Hermes's own
/// resolution: a non-empty `HERMES_HOME` is taken VERBATIM even when the dir does not
/// exist — unlike Codex's exists-check — else `<user_home>/.hermes`.
pub fn hermes_home() -> Option<PathBuf> {
    resolve_hermes_home(
        std::env::var("HERMES_HOME").ok(),
        crate::platform::user_home_opt(),
    )
}

fn resolve_hermes_home(
    hermes_home_env: Option<String>,
    user_home: Option<String>,
) -> Option<PathBuf> {
    if let Some(h) = crate::platform::nonempty(hermes_home_env) {
        return Some(PathBuf::from(h));
    }
    user_home.map(|h| PathBuf::from(h).join(".hermes"))
}

/// Decode one Hermes hook payload (already identified by
/// `_pixtuoid_source == "hermes"`); an unregistered event bails so
/// registered-vs-decoded drift is loud.
///
/// The activity arms prepend an [`AgentEvent::Identity`] because Hermes is HOOK-ONLY:
/// a slot the reducer synthesizes mid-turn has no transcript back-fill path, so
/// without the attached identity it would stay a blank `#N` ghost.
pub fn decode_hermes_hook_payload(v: &Value) -> Result<Vec<AgentEvent>> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("hermes hook payload must be an object"))?;
    let event = obj
        .get("hook_event_name")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("hermes payload missing hook_event_name"))?;
    let cwd = obj
        .get("cwd")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty());
    // The cwd fallback is only for a future event that omits `session_id` — it keeps
    // coalescing best-effort instead of dropping the event.
    let key = obj
        .get("session_id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .or(cwd)
        .ok_or_else(|| anyhow!("hermes payload has no session_id or cwd"))?;
    let agent_id = AgentId::from_parts(SOURCE_NAME, key);
    let cwd = cwd.unwrap_or("");

    let identity = || {
        AgentEvent::identity(
            agent_id,
            SOURCE_NAME,
            key,
            (!cwd.is_empty()).then(|| cwd.into()),
        )
    };

    match event {
        "on_session_start" => Ok(vec![AgentEvent::SessionStart {
            agent_id,
            source: SOURCE_NAME.to_string(),
            session_id: key.to_string(),
            cwd: cwd.into(),
            parent_id: None,
        }]),
        "pre_tool_call" => {
            let tool = obj
                .get("tool_name")
                .and_then(|s| s.as_str())
                .unwrap_or_else(|| {
                    crate::source::drift::missing_field(SOURCE_NAME, "pre_tool_call", "tool_name");
                    "?"
                });
            Ok(vec![
                identity(),
                AgentEvent::ActivityStart {
                    agent_id,
                    tool_use_id: None,
                    detail: Some(hermes_tool_detail(tool, obj.get("tool_input"))),
                },
            ])
        }
        "post_tool_call" => Ok(vec![
            identity(),
            AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: None,
            },
        ]),
        "on_session_end" => Ok(vec![AgentEvent::SessionEnd {
            agent_id,
            as_child: false,
        }]),
        other => {
            crate::source::drift::unknown_event(SOURCE_NAME, other);
            bail!(
                "unsupported hermes hook event: {}",
                crate::source::decoder::display_safe(other)
            )
        }
    }
}

fn hermes_tool_detail(tool: &str, args: Option<&Value>) -> ToolDetail {
    const KEYS: &[&str] = &["command", "file_path", "path", "pattern", "url"];
    crate::source::decoder::generic_keyed_detail(tool, args, KEYS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode_all(v: Value) -> Vec<AgentEvent> {
        decode_hermes_hook_payload(&v).expect("decodes")
    }

    fn decode(v: Value) -> AgentEvent {
        decode_all(v).pop().expect("at least one event")
    }

    #[test]
    fn session_start_keys_on_session_id_with_real_cwd() {
        let ev = decode(json!({
            "hook_event_name": "on_session_start",
            "tool_name": null, "tool_input": null,
            "session_id": "sess-1", "cwd": "/Users/dev/proj", "extra": {}
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
                assert_eq!(agent_id, AgentId::from_parts(SOURCE_NAME, "sess-1"));
                assert_eq!(session_id, "sess-1", "key on session_id, not cwd");
                assert_eq!(cwd, std::path::PathBuf::from("/Users/dev/proj"));
                assert_eq!(parent_id, None);
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn session_id_distinguishes_two_sessions_in_one_workspace() {
        let a = decode(
            json!({"hook_event_name": "on_session_start", "session_id": "A", "cwd": "/repo"}),
        );
        let b = decode(
            json!({"hook_event_name": "on_session_start", "session_id": "B", "cwd": "/repo"}),
        );
        assert_ne!(
            a.agent_id(),
            b.agent_id(),
            "two sessions in one repo must be distinct"
        );
    }

    #[test]
    fn key_falls_back_to_cwd_when_session_id_absent() {
        let ev = decode(json!({
            "hook_event_name": "on_session_start", "cwd": "/Users/dev/proj"
        }));
        assert!(matches!(ev, AgentEvent::SessionStart { agent_id, .. }
            if agent_id == AgentId::from_parts(SOURCE_NAME, "/Users/dev/proj")));
    }

    #[test]
    fn pre_tool_call_is_activity_start_with_no_tool_id() {
        let ev = decode(json!({
            "hook_event_name": "pre_tool_call",
            "session_id": "s", "cwd": "/repo",
            "tool_name": "terminal", "tool_input": {"command": "echo hello"},
            "extra": {"task_id": "t", "tool_call_id": "c"}
        }));
        match ev {
            AgentEvent::ActivityStart {
                tool_use_id,
                detail,
                ..
            } => {
                assert_eq!(tool_use_id, None);
                assert_eq!(detail.unwrap().display(), "terminal: echo hello");
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn post_tool_call_is_activity_end() {
        let ev = decode(json!({
            "hook_event_name": "post_tool_call", "session_id": "s", "cwd": "/repo",
            "tool_name": "terminal",
            "extra": {"result": "{\"output\":\"hi\"}", "duration_ms": 42}
        }));
        assert!(matches!(
            &ev,
            AgentEvent::ActivityEnd {
                tool_use_id: None,
                ..
            }
        ));
    }

    #[test]
    fn on_session_end_maps_to_session_end() {
        let ev =
            decode(json!({"hook_event_name": "on_session_end", "session_id": "s", "cwd": "/r"}));
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
        let sid = "sess-1";
        let events = [
            json!({"hook_event_name": "on_session_start", "session_id": sid, "cwd": "/repo"}),
            json!({"hook_event_name": "pre_tool_call", "session_id": sid, "cwd": "/repo",
                   "tool_name": "terminal", "tool_input": {"command": "ls"}}),
            json!({"hook_event_name": "post_tool_call", "session_id": sid, "cwd": "/repo", "tool_name": "terminal"}),
            json!({"hook_event_name": "on_session_end", "session_id": sid, "cwd": "/repo"}),
        ];
        let ids: std::collections::BTreeSet<_> = events
            .iter()
            .flat_map(|v| decode_hermes_hook_payload(v).unwrap())
            .map(|e| e.agent_id())
            .collect();
        assert_eq!(ids.len(), 1, "all events must coalesce to one AgentId");
    }

    #[test]
    fn activity_arms_prepend_identity_keyed_on_session_id() {
        for payload in [
            json!({"hook_event_name": "pre_tool_call", "session_id": "s", "cwd": "/repo",
                   "tool_name": "terminal", "tool_input": {"command": "ls"}}),
            json!({"hook_event_name": "post_tool_call", "session_id": "s", "cwd": "/repo", "tool_name": "terminal"}),
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
                    assert_eq!(session_id, "s");
                    assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/repo")));
                }
                other => panic!("{name}: expected leading Identity, got {other:?}"),
            }
        }
    }

    #[test]
    fn session_events_carry_no_identity() {
        for payload in [
            json!({"hook_event_name": "on_session_start", "session_id": "s", "cwd": "/r"}),
            json!({"hook_event_name": "on_session_end", "session_id": "s", "cwd": "/r"}),
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
    fn unknown_event_bails_loudly() {
        for ev in ["subagent_stop", "pre_message", "on_error", "Bogus"] {
            assert!(
                decode_hermes_hook_payload(&json!({"hook_event_name": ev, "cwd": "/r"})).is_err(),
                "{ev} must bail (not registered, must not decode silently)"
            );
        }
    }

    #[test]
    fn malformed_payloads_are_errors() {
        assert!(decode_hermes_hook_payload(&json!("just a string")).is_err());
        assert!(decode_hermes_hook_payload(&json!(42)).is_err());
        assert!(decode_hermes_hook_payload(&json!({"hook_event_name": "on_session_end"})).is_err());
    }

    #[test]
    fn hermes_home_prefers_verbatim_env_then_dot_hermes() {
        assert_eq!(
            resolve_hermes_home(Some("/custom/hm".into()), Some("/home/u".into())),
            Some(PathBuf::from("/custom/hm"))
        );
        assert_eq!(
            resolve_hermes_home(None, Some("/home/u".into())),
            Some(PathBuf::from("/home/u").join(".hermes"))
        );
        assert_eq!(
            resolve_hermes_home(Some("   ".into()), Some("/home/u".into())),
            Some(PathBuf::from("/home/u").join(".hermes"))
        );
        assert_eq!(resolve_hermes_home(None, None), None);
    }
}
