//! OpenCode CLI source. Hook-only (no JSONL watcher) — events arrive through the
//! shared `pixtuoid-hook` shim, dispatched by the JS plugin. The custom hook
//! decoder claims ALL events (alien envelope) since we control the entire event
//! shape (no shared-arm fallback).

use std::future;
use std::path::Path;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::Value;

use crate::source::decoder::{cwd_basename_label, make_tool_detail};
use crate::source::{Activity, AgentEvent, Source, TaggedSender};
use crate::AgentId;

pub const SOURCE_NAME: &str = "opencode";
pub const LABEL_PREFIX: &str = "oc";

/// OpenCode source. Hook-only: the shared `HookSocketListener` (bound by
/// ClaudeCodeSource) receives all hook events; the decoder dispatches to
/// [`decode_opencode_hook_custom`] by matching `_pixtuoid_source == "opencode"`.
/// No JSONL watcher needed — just a parked task so the SourceManager handle
/// stays alive.
pub struct OpenCodeSource;

#[async_trait]
impl Source for OpenCodeSource {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn run(self: Box<Self>, _tx: TaggedSender) -> Result<()> {
        // Hook-only: the shared socket listener handles all I/O.
        // Park forever so the SourceManager's task stays alive.
        future::pending::<()>().await;
        Ok(())
    }
}

/// Custom hook decoder — claims ALL events (alien envelope).
///
/// The JS plugin sends standard-shaped hook payloads with `_pixtuoid_source:
/// "opencode"`. We decode every known `hook_event_name`:
///
/// | hook_event_name   | AgentEvent                    |
/// |-------------------|-------------------------------|
/// | `SessionStart`    | `SessionStart { cwd }`        |
/// | `PreToolUse`      | `ActivityStart { Typing }`    |
/// | `PostToolUse`     | `ActivityEnd`                 |
/// | `PermissionRequest` | `Waiting { permission }`    |
/// | `SessionEnd`      | `SessionEnd`                  |
///
/// Unknown events return `Ok(None)` — they fall through to the shared CC-shaped
/// arms which will likely bail with "missing hook_event_name" or similar.
pub fn decode_opencode_hook_custom(v: &Value) -> Result<Option<AgentEvent>> {
    let Some(obj) = v.as_object() else {
        return Ok(None);
    };

    let event = match obj.get("hook_event_name").and_then(|s| s.as_str()) {
        Some(e) => e,
        None => return Ok(None),
    };

    let session_id = obj
        .get("session_id")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();

    let source = SOURCE_NAME;
    let cwd: std::path::PathBuf = obj.get("cwd").and_then(|s| s.as_str()).unwrap_or("").into();

    match event {
        "SessionStart" => {
            if session_id.is_empty() {
                return Err(anyhow!("SessionStart without session_id"));
            }
            Ok(Some(AgentEvent::SessionStart {
                agent_id: AgentId::from_parts(source, &session_id),
                source: source.to_string(),
                session_id,
                cwd,
                parent_id: None,
            }))
        }
        "PreToolUse" => {
            let tool_use_id = obj
                .get("tool_use_id")
                .and_then(|s| s.as_str())
                .map(String::from);
            let tool_name = obj
                .get("tool_name")
                .and_then(|s| s.as_str())
                .unwrap_or("tool");
            // Prefer `tool_input` when the plugin supplies it; fall back to None
            let tool_input = obj.get("tool_input");
            Ok(Some(AgentEvent::ActivityStart {
                agent_id: AgentId::from_parts(source, &session_id),
                activity: Activity::Typing,
                tool_use_id,
                detail: Some(make_tool_detail(tool_name, tool_input)),
            }))
        }
        "PostToolUse" => {
            let tool_use_id = obj
                .get("tool_use_id")
                .and_then(|s| s.as_str())
                .map(String::from);
            Ok(Some(AgentEvent::ActivityEnd {
                agent_id: AgentId::from_parts(source, &session_id),
                tool_use_id,
            }))
        }
        "PermissionRequest" => Ok(Some(AgentEvent::Waiting {
            agent_id: AgentId::from_parts(source, &session_id),
            reason: "permission".into(),
        })),
        "SessionEnd" => {
            if session_id.is_empty() {
                return Err(anyhow!("SessionEnd without session_id"));
            }
            Ok(Some(AgentEvent::SessionEnd {
                agent_id: AgentId::from_parts(source, &session_id),
            }))
        }
        _ => {
            tracing::trace!("unhandled opencode hook event: {event}");
            Ok(None)
        }
    }
}

/// Label deriver: `"oc·{basename}"` or bare `"oc"` when cwd has no basename.
pub fn derive_opencode_label(_path: &Path, _source: &str, cwd: &Path) -> String {
    cwd_basename_label(LABEL_PREFIX, cwd).unwrap_or_else(|| LABEL_PREFIX.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode(v: Value) -> Result<Option<AgentEvent>> {
        decode_opencode_hook_custom(&v)
    }

    #[test]
    fn decode_session_start() {
        let v = json!({
            "_pixtuoid_source": "opencode",
            "hook_event_name": "SessionStart",
            "session_id": "ses_abc",
            "cwd": "/repo"
        });
        let ev = decode(v).unwrap().unwrap();
        match &ev {
            AgentEvent::SessionStart {
                agent_id,
                source,
                session_id,
                cwd,
                parent_id,
            } => {
                assert_eq!(*agent_id, AgentId::from_parts("opencode", "ses_abc"));
                assert_eq!(source, "opencode");
                assert_eq!(session_id, "ses_abc");
                assert_eq!(cwd, &std::path::PathBuf::from("/repo"));
                assert!(parent_id.is_none());
            }
            _ => panic!("expected SessionStart, got {ev:?}"),
        }
    }

    #[test]
    fn session_start_without_id_errors() {
        let v = json!({
            "_pixtuoid_source": "opencode",
            "hook_event_name": "SessionStart",
            "session_id": ""
        });
        assert!(decode(v).is_err());
    }

    #[test]
    fn pre_tool_use_is_activity_start() {
        let v = json!({
            "_pixtuoid_source": "opencode",
            "hook_event_name": "PreToolUse",
            "session_id": "ses_abc",
            "tool_use_id": "call_1",
            "tool_name": "Bash"
        });
        let ev = decode(v).unwrap().unwrap();
        assert!(matches!(
            ev,
            AgentEvent::ActivityStart {
                activity: Activity::Typing,
                ..
            }
        ));
    }

    #[test]
    fn post_tool_use_is_activity_end() {
        let v = json!({
            "_pixtuoid_source": "opencode",
            "hook_event_name": "PostToolUse",
            "session_id": "ses_abc",
            "tool_use_id": "call_1"
        });
        let ev = decode(v).unwrap().unwrap();
        assert!(matches!(ev, AgentEvent::ActivityEnd { .. }));
    }

    #[test]
    fn permission_request_is_waiting() {
        let v = json!({
            "_pixtuoid_source": "opencode",
            "hook_event_name": "PermissionRequest",
            "session_id": "ses_abc"
        });
        let ev = decode(v).unwrap().unwrap();
        assert!(matches!(ev, AgentEvent::Waiting { .. }));
    }

    #[test]
    fn session_end() {
        let v = json!({
            "_pixtuoid_source": "opencode",
            "hook_event_name": "SessionEnd",
            "session_id": "ses_abc"
        });
        let ev = decode(v).unwrap().unwrap();
        assert!(matches!(ev, AgentEvent::SessionEnd { .. }));
    }

    #[test]
    fn session_end_without_id_errors() {
        let v = json!({
            "_pixtuoid_source": "opencode",
            "hook_event_name": "SessionEnd",
            "session_id": ""
        });
        assert!(decode(v).is_err());
    }

    #[test]
    fn unknown_events_fall_through() {
        let v = json!({
            "_pixtuoid_source": "opencode",
            "hook_event_name": "UnknownEvent",
            "session_id": "ses_abc"
        });
        assert!(decode(v).unwrap().is_none());
    }

    #[test]
    fn non_object_payload_falls_through() {
        assert!(decode(json!("nope")).unwrap().is_none());
    }

    #[test]
    fn label_is_oc_basename() {
        assert_eq!(
            derive_opencode_label(
                Path::new("/ignored.jsonl"),
                SOURCE_NAME,
                Path::new("/Users/me/my-project")
            ),
            "oc·my-project"
        );
        assert_eq!(
            derive_opencode_label(Path::new("/ignored.jsonl"), SOURCE_NAME, Path::new("")),
            "oc"
        );
    }

    #[test]
    fn pre_tool_use_without_tool_use_id_still_decodes() {
        let v = json!({
            "_pixtuoid_source": "opencode",
            "hook_event_name": "PreToolUse",
            "session_id": "ses_abc"
        });
        let ev = decode(v).unwrap().unwrap();
        assert!(matches!(
            ev,
            AgentEvent::ActivityStart {
                tool_use_id: None,
                ..
            }
        ));
    }

    #[test]
    fn session_start_without_cwd_defaults_to_empty() {
        let v = json!({
            "_pixtuoid_source": "opencode",
            "hook_event_name": "SessionStart",
            "session_id": "ses_abc"
        });
        let ev = decode(v).unwrap().unwrap();
        match &ev {
            AgentEvent::SessionStart { cwd, .. } => {
                assert_eq!(cwd, &std::path::PathBuf::from(""));
            }
            _ => panic!("expected SessionStart, got {ev:?}"),
        }
    }
}
