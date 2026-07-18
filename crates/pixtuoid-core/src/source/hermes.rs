//! Hermes Agent source — HOOK-ONLY (no transcript watcher).
//!
//! Hermes Agent (Nous Research) is a standalone autonomous coding agent. A user
//! may run SEVERAL instances at once — multiple terminals/projects, or concurrent
//! sessions in ONE project — so the identity must distinguish concurrent sessions,
//! not merge them by workspace. Three candidate seams, only one reachable by a
//! passive observer:
//!
//! - The agent's own stdout/transcript is per-invocation and pixtuoid never
//!   spawns the user's agent — structurally unreachable.
//! - Sessions on disk are not a tailable append-only JSONL.
//! - **Hermes shell hooks** (`~/.hermes/config.yaml`, or `$HERMES_HOME/config.yaml`)
//!   — shell commands fired on lifecycle/tool events, JSON on stdin. THIS is the
//!   seam: connecting Hermes in the Connection panel (`s`) registers the shim under
//!   the `hooks:` block. Wire shape verified against a real capture:
//!
//! ```json
//! {"hook_event_name":"pre_tool_call","tool_name":"terminal",
//!  "tool_input":{"command":"echo hi"},"session_id":"…","cwd":"/repo",
//!  "extra":{"task_id":"…","tool_call_id":"…"}}
//! ```
//!
//! Keyed on **`session_id`** (present on every event in the capture), so two
//! Hermes sessions in one repo stay distinct AND all of a session's events
//! coalesce — cwd-keying would wrongly merge concurrent sessions (the Cursor
//! lesson). Unlike Cursor, the top-level `cwd` IS populated, so it is the label /
//! SessionStart cwd directly (with a cwd fallback for the key if a future event
//! ever omits `session_id`).
//!
//! Hook payloads arrive on the shared hook socket stamped
//! `_pixtuoid_source: "hermes"`. The envelope reuses CC's `hook_event_name` field
//! NAME but with **snake_case values** (`on_session_start` / `pre_tool_call` /
//! `post_tool_call` / `on_session_end`) alien to the shared CC-shaped arms, so per
//! the `HookDecoding::custom` contract the custom decoder claims EVERY event
//! (`.map(Some)`, never `Ok(None)`).
//!
//! Deliberate scope: `tool_use_id` is always `None` (single-transport, no
//! hook-wins dedup needed); sessions render FLAT. The SHELL-hook set pixtuoid
//! consumes (`_DEFAULT_PAYLOADS` in `hermes_cli/hooks.py` — the `hermes hooks
//! test`/doctor fixtures, and our drift-watch's source-of-truth) has a stop-only
//! `subagent_stop` carrying `parent_session_id` + `child_summary`/`child_status`
//! but NO child session/agent id, and no `subagent_start`. (Hermes's Python
//! PLUGIN API in `hooks.md` DOES define a `subagent_start` with a
//! `child_subagent_id`, but plugin hooks are in-process callbacks that never
//! reach a shell command's stdin — pixtuoid, a passive SHELL-hook observer,
//! can't see them; don't "fix" this by trying to model them.) With no observable
//! child key, a decoded `subagent_stop` would be a `SessionEnd` for a child that
//! was never `SessionStart`ed (a reducer no-op), so it is left out of
//! `HERMES_EVENTS` (the shim never delivers it) and, should one ever arrive,
//! bails via the `unknown_event` drift arm (pinned by `unknown_event_bails_loudly`)
//! — the same deliberate-omit treatment as Reasonix's stop-only `SubagentStop`
//! (`REASONIX_KNOWN_OMITTED`).
//! `on_session_end` FIRES on clean completion → `has_exit_signal: true`; abrupt
//! exits fall to the stale-sweep (no PID exposed in the payload).

use std::path::PathBuf;

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use crate::source::decoder::generic_tool_display;
use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

pub const SOURCE_NAME: &str = "hermes";

/// The Hermes home dir, mirroring Hermes's OWN resolution (verified via
/// `hermes config path`): `HERMES_HOME` VERBATIM when set to a non-empty value —
/// Hermes uses it even when the dir does not yet exist, UNLIKE Codex's
/// exists-check (`hermes config path` reported a non-existent `HERMES_HOME`
/// verbatim) — else `<user_home>/.hermes`. `config.yaml` lives directly in it.
/// Consumed by the installer's `default_config_path`; `None` when neither
/// `HERMES_HOME` nor a home dir resolves (installer surfaces "pass --config").
pub fn hermes_home() -> Option<PathBuf> {
    resolve_hermes_home(
        std::env::var("HERMES_HOME").ok(),
        crate::platform::user_home_opt(),
    )
}

/// Pure precedence core for [`hermes_home`] — env + home injected so every arm
/// unit-tests on any host without mutating process env.
fn resolve_hermes_home(
    hermes_home_env: Option<String>,
    user_home: Option<String>,
) -> Option<PathBuf> {
    if let Some(h) = hermes_home_env.filter(|s| !s.trim().is_empty()) {
        return Some(PathBuf::from(h));
    }
    user_home.map(|h| PathBuf::from(h).join(".hermes"))
}

/// Decode one Hermes hook payload (already identified by
/// `_pixtuoid_source == "hermes"`). Envelope per `~/.hermes/config.yaml` hooks.
///
/// Event mapping (snake_case `hook_event_name` values), all keyed on `session_id`:
/// - `on_session_start` → `SessionStart`
/// - `pre_tool_call`    → `Identity` + `ActivityStart`
/// - `post_tool_call`   → `Identity` + `ActivityEnd`
/// - `on_session_end`   → `SessionEnd`
/// - anything else      → bail (registered-vs-decoded drift must be loud)
///
/// The activity arms prepend an [`AgentEvent::Identity`] (#221) because Hermes is
/// HOOK-ONLY: a slot the reducer synthesizes mid-turn has no transcript back-fill
/// path, so without the attached identity it would stay a blank `#N` ghost. The
/// Identity's `session_id` mirrors the `SessionStart` arm's key exactly.
pub fn decode_hermes_hook_payload(v: &Value) -> Result<Vec<AgentEvent>> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("hermes hook payload must be an object"))?;
    let event = obj
        .get("hook_event_name")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("hermes payload missing hook_event_name"))?;
    // The workspace path — Hermes populates the top-level `cwd` (unlike Cursor).
    let cwd = obj
        .get("cwd")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty());
    // Key on `session_id` — present + consistent across a session's events, so it
    // distinguishes concurrent sessions in one project AND coalesces a session.
    // Fall back to the workspace only if a future event ever omits it (keeps
    // coalescing best-effort instead of dropping the event).
    let key = obj
        .get("session_id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .or(cwd)
        .ok_or_else(|| anyhow!("hermes payload has no session_id or cwd"))?;
    let agent_id = AgentId::from_parts(SOURCE_NAME, key);
    let cwd = cwd.unwrap_or("");

    let identity = || AgentEvent::Identity {
        agent_id,
        source: SOURCE_NAME.to_string(),
        session_id: key.to_string(),
        cwd: (!cwd.is_empty()).then(|| cwd.into()),
        pid: None,
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
            bail!("unsupported hermes hook event: {other}")
        }
    }
}

/// Hermes tool detail: `"name: target"` using Hermes's argument vocabulary,
/// looked up `command` > `file_path` > `path` > `pattern` > `url` (`terminal`
/// carries `command`; file/search tools carry the rest). Both the tool NAME and
/// the `: target` are capped at the decode boundary via `generic_tool_display`.
fn hermes_tool_detail(tool: &str, args: Option<&Value>) -> ToolDetail {
    const KEYS: &[&str] = &["command", "file_path", "path", "pattern", "url"];
    let target = args.and_then(|a| crate::source::decoder::first_present_str(a, KEYS));
    generic_tool_display(tool, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode_all(v: Value) -> Vec<AgentEvent> {
        decode_hermes_hook_payload(&v).expect("decodes")
    }

    /// The payload's MAIN event — the last decoded event (activity arms prepend
    /// an `Identity`).
    fn decode(v: Value) -> AgentEvent {
        decode_all(v).pop().expect("at least one event")
    }

    #[test]
    fn session_start_keys_on_session_id_with_real_cwd() {
        // Real capture shape: session_id present, top-level cwd populated.
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
        // The multi-instance point: cwd-keying would merge these; session_id keeps
        // two concurrent Hermes sessions in one repo distinct.
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
        // Real capture tool shape: tool_name "terminal", tool_input.command.
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
        // `subagent_stop` is a REAL upstream event (hermes_cli/hooks.py
        // _DEFAULT_PAYLOADS) deliberately left out of HERMES_EVENTS — it is
        // stop-only with no child key to model (see the module doc). It must bail
        // like any unregistered event, not decode silently.
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
        // Nothing to key on.
        assert!(decode_hermes_hook_payload(&json!({"hook_event_name": "on_session_end"})).is_err());
    }

    #[test]
    fn hermes_home_prefers_verbatim_env_then_dot_hermes() {
        // HERMES_HOME wins VERBATIM (even a not-yet-existing dir — mirrors Hermes,
        // unlike Codex's exists-check); home/`.hermes` never consulted.
        assert_eq!(
            resolve_hermes_home(Some("/custom/hm".into()), Some("/home/u".into())),
            Some(PathBuf::from("/custom/hm"))
        );
        // Unset (or whitespace-only) → <home>/.hermes.
        assert_eq!(
            resolve_hermes_home(None, Some("/home/u".into())),
            Some(PathBuf::from("/home/u").join(".hermes"))
        );
        assert_eq!(
            resolve_hermes_home(Some("   ".into()), Some("/home/u".into())),
            Some(PathBuf::from("/home/u").join(".hermes"))
        );
        // No home + no override → None (installer surfaces "pass --config").
        assert_eq!(resolve_hermes_home(None, None), None);
    }
}
