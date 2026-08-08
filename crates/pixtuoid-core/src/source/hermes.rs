//! Hermes Agent (Nous Research) source — HOOK-ONLY: pixtuoid never spawns the user's
//! agent and Hermes's on-disk sessions are not tailable JSONL, so the shell hooks in
//! the `config.yaml` under [`hermes_home`] are the only seam a passive observer can
//! reach. That home is `$HERMES_HOME` when set, else `~/.hermes` — but
//! `%LOCALAPPDATA%\hermes` on Windows, which is why every caller resolves it through
//! that fn rather than joining `.hermes` itself.
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

/// The Hermes home dir (`config.yaml` lives directly in it), mirroring hermes's
/// own `hermes_constants._hermes_home_from_env` (2026.8.3): `HERMES_HOME`
/// STRIPPED and taken verbatim when what's left is non-empty — even for a dir
/// that does not exist, unlike Codex's exists-check — else the PLATFORM-NATIVE
/// default, which is `%LOCALAPPDATA%\hermes` on Windows and only elsewhere
/// `<user_home>/.hermes`.
///
/// Hermes PROFILES are applied out-of-band and are deliberately not mirrored —
/// see this crate's `CLAUDE.md` "per-CLI home resolvers" sharp edge.
pub fn hermes_home() -> Option<PathBuf> {
    resolve_hermes_home(
        std::env::var("HERMES_HOME").ok(),
        std::env::var("LOCALAPPDATA").ok(),
        cfg!(windows),
        crate::platform::user_home_opt(),
    )
    .map(|d| crate::platform::warn_if_relative_override("HERMES_HOME", d))
}

/// Pure precedence core, separated so the Windows arm is unit-testable on any
/// host. `user_home` mirrors `Path.home()`, whose Windows arm is
/// `USERPROFILE`-first (`ntpath.expanduser`) exactly like [`user_home_opt`].
///
/// [`user_home_opt`]: crate::platform::user_home_opt
fn resolve_hermes_home(
    hermes_home_env: Option<String>,
    local_appdata: Option<String>,
    windows: bool,
    user_home: Option<String>,
) -> Option<PathBuf> {
    use crate::platform::nonempty_trimmed;

    if let Some(h) = nonempty_trimmed(hermes_home_env) {
        return Some(PathBuf::from(h));
    }
    if !windows {
        return user_home.map(|h| PathBuf::from(h).join(".hermes"));
    }
    let base = match nonempty_trimmed(local_appdata) {
        Some(l) => PathBuf::from(l),
        None => PathBuf::from(user_home?).join("AppData").join("Local"),
    };
    Some(base.join("hermes"))
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

    /// Pinned against a live probe of hermes 2026.8.3, the STRIP included.
    #[test]
    fn hermes_home_strips_the_env_value_then_falls_back_to_dot_hermes() {
        let unix = |env: Option<&str>| {
            resolve_hermes_home(
                env.map(str::to_string),
                None,
                false,
                Some("/home/u".to_string()),
            )
        };
        let default = Some(PathBuf::from("/home/u").join(".hermes"));

        assert_eq!(unix(Some("/custom/hm")), Some(PathBuf::from("/custom/hm")));
        assert_eq!(
            unix(Some("  /custom/hm  ")),
            Some(PathBuf::from("/custom/hm")),
            "upstream reads HERMES_HOME through `.strip()` — the value is trimmed, not just tested"
        );
        assert_eq!(unix(None), default);
        assert_eq!(unix(Some("")), default);
        assert_eq!(unix(Some("   ")), default);
        assert_eq!(resolve_hermes_home(None, None, false, None), None);
    }

    /// `%LOCALAPPDATA%\hermes`, NOT `<home>/.hermes` — the generic answer wrote
    /// hooks into a config.yaml hermes never reads (#880).
    #[test]
    fn hermes_home_on_windows_is_local_appdata_not_dot_hermes() {
        let win = |env: Option<&str>, appdata: Option<&str>| {
            resolve_hermes_home(
                env.map(str::to_string),
                appdata.map(str::to_string),
                true,
                Some(r"C:\Users\ada".to_string()),
            )
        };

        assert_eq!(
            win(None, Some(r"C:\Users\ada\AppData\Local")),
            Some(PathBuf::from(r"C:\Users\ada\AppData\Local").join("hermes"))
        );
        // LOCALAPPDATA is `.strip()`ed upstream too, so a padded or blank one
        // falls through to the same relative default.
        let relative_default = Some(
            PathBuf::from(r"C:\Users\ada")
                .join("AppData")
                .join("Local")
                .join("hermes"),
        );
        assert_eq!(win(None, None), relative_default);
        assert_eq!(win(None, Some("   ")), relative_default);
        assert_eq!(
            win(None, Some(r"  C:\pad  ")),
            Some(PathBuf::from(r"C:\pad").join("hermes"))
        );
        assert_eq!(
            win(
                Some(r"D:\profiles\work"),
                Some(r"C:\Users\ada\AppData\Local")
            ),
            Some(PathBuf::from(r"D:\profiles\work")),
            "HERMES_HOME still outranks the platform default"
        );
        assert_eq!(resolve_hermes_home(None, None, true, None), None);
    }

    /// The arms MUST disagree — the assertion that would have caught the bug.
    #[test]
    fn the_windows_and_unix_defaults_diverge() {
        let home = Some(r"C:\Users\ada".to_string());
        assert_ne!(
            resolve_hermes_home(None, None, true, home.clone()),
            resolve_hermes_home(None, None, false, home),
        );
    }
}
