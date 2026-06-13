//! CodeWhale source — HOOK-ONLY (no JSONL watcher).
//!
//! CodeWhale (github.com/Hmbown/CodeWhale, the Rust line, a DeepSeek-V4 agent
//! CLI+TUI) ships a CC-style shell-command hook system (`[[hooks.hooks]]` under
//! a `[hooks]` table in `~/.codewhale/config.toml` / project `.codewhale/
//! hooks.toml`); `pixtuoid install-hooks --target codewhale` registers the shim
//! there. But CodeWhale's hooks DON'T hand the command a CC-shaped JSON payload
//! on stdin — identity travels as `DEEPSEEK_*` ENV VARS (verified against
//! `crates/tui/src/hooks.rs::HookContext::to_env_vars` @0.8.59, and a live
//! capture 2026-06-12), and only `message_submit`/`turn_end`/`subagent_*` pass
//! any stdin at all. So the shim runs in **env-mode** (`pixtuoid-hook --event
//! <name>`): it reads the env vars and synthesizes this envelope, which arrives
//! on the shared hook socket stamped `_pixtuoid_source: "codewhale"`:
//!
//! ```json
//! {"event":"tool_call_before","cwd":"/repo","tool":"exec_shell","tool_args":"{\"command\":\"ls -la\"}"}
//! ```
//!
//! snake_case `event` discriminator (not CC's `hook_event_name`), so the custom
//! decoder claims every event and the shared id-key branch is never reached —
//! the same alien-envelope shape as Reasonix. The load-bearing decision:
//!
//! - **Key on `cwd`, NOT `session_id`.** The live capture proved CodeWhale's
//!   `DEEPSEEK_SESSION_ID` is INCONSISTENT across a single session's events: it
//!   is `sess_<8hex>` on session/turn/tool-after events but a raw turn UUID
//!   (`3074bf2f-…`) on `tool_call_before` — that event is built by a different
//!   code path (the engine turn-loop, also spelled `DEEPSEEK_MODE=Yolo` vs
//!   `YOLO` elsewhere). Keying on it would split every ActivityStart into a
//!   second ghost sprite. `DEEPSEEK_WORKSPACE` (the cwd) is the ONE field
//!   present and identical on EVERY event, so it is the only safe AgentId key —
//!   exactly Reasonix's cwd-keying. Consequences, all deliberate and shared
//!   with Reasonix:
//!     - Two concurrent CodeWhale sessions in ONE workspace render as one
//!       sprite (indistinguishable upstream); one's `session_end` walks the
//!       shared sprite out and it walks back in on the other's next prompt
//!       (`message_submit` → `SessionStart` is the resurrect path).
//!     - `tool_use_id` is always `None`: the reducer's per-call machinery
//!       (hook-wins dedup, `active_tasks`) is bypassed — harmless under a
//!       single transport with no JSONL twin.
//!
//! - **No Waiting/permission state.** CodeWhale exposes NO approval hook to the
//!   TUI shell-command system (its `ApprovalLifecycle` lives in the separate
//!   `codewhale-hooks` app-server sink crate, not the `[[hooks.hooks]]` events).
//!   A tool parked on an approval prompt therefore shows Active (the
//!   `tool_call_before` fired, no `tool_call_after` yet), resolving when the
//!   user approves. This is strictly less than Reasonix (which had
//!   `Notification` → Waiting) — accepted, no signal exists to do better.
//!
//! - **Exit profile is CC-class.** `session_end` fires on a clean quit (Ctrl+C
//!   confirm; verified live) carrying `DEEPSEEK_WORKSPACE`; only SIGKILL /
//!   terminal-close leaves no signal and falls to the generic stale-sweep. So
//!   `has_exit_signal: true` — no Codex-style short-idle carve-out.
//!
//! Why no JSONL transport: CodeWhale's `rollout_path` (a `ThreadMetadata`
//! column in `~/.codewhale/state.db`) is NEVER written in production (set to
//! `Some` only in its own tests); saved sessions are full-snapshot `{id}.json`
//! rewrites (untailable), and headless `codewhale exec` runs with
//! `hook_executor: None` — hooks fire ONLY in the interactive TUI. There is no
//! tailable per-session transcript to watch, so hooks carry everything.

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use crate::source::decoder::{ellipsize, MAX_TOOL_TARGET_CHARS};
use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

pub const SOURCE_NAME: &str = "codewhale";

/// CodeWhale tools that dispatch a sub-agent (`crates/tui/src/tools/subagent`
/// @0.8.59: the tool is `agent_spawn`, with the deprecated alias `spawn_agent`).
/// Mapped to `ToolDetail::Task` so the slot reads "Delegating" while the
/// dispatch tool runs. v1 shows the dispatch only — individual sub-agent
/// sprites would need the `subagent_spawn`/`subagent_complete` observer hooks
/// (which carry an `agent_id`) and are deferred.
const SUBAGENT_TOOLS: &[&str] = &["agent_spawn", "spawn_agent"];

/// Decode one CodeWhale hook envelope (already identified by
/// `_pixtuoid_source == "codewhale"`). The envelope is synthesized by the shim
/// in env-mode (see the module header), keyed on `cwd`:
///
/// - `session_start` / `message_submit` → `SessionStart` (idempotent in the
///   reducer; `message_submit` doubles as the resurrect path after a sweep)
/// - `tool_call_before` → `Identity` + `ActivityStart` (dispatch family → `Task`)
/// - `tool_call_after`  → `Identity` + `ActivityEnd`
/// - `session_end`      → `SessionEnd`
/// - anything else → bail (registered-vs-decoded drift must be loud, not a
///   silent drop — same contract as the CC/Codex/Reasonix arms)
///
/// The activity arms prepend an [`AgentEvent::Identity`] (#221): CodeWhale is
/// HOOK-ONLY, so a slot the reducer's proof-of-life pre-pass synthesizes
/// mid-turn has no JSONL back-fill path; the cwd IS the identity (it is the
/// session key), so `session_id` mirrors the `SessionStart` arm exactly and
/// coalescing holds.
pub fn decode_cw_hook_payload(v: &Value) -> Result<Vec<AgentEvent>> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("codewhale hook payload must be an object"))?;
    let event = obj
        .get("event")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("codewhale payload missing event"))?;
    // `cwd` (DEEPSEEK_WORKSPACE) is the ONLY stable identity — see the module
    // header on why session_id is unusable. An empty one would mint a phantom
    // agent nothing coalesces with (the same empty-key-is-malformed idiom as
    // the Reasonix cwd guard and the shared decoder's session_id guard).
    let cwd = obj
        .get("cwd")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("codewhale payload missing/empty cwd"))?;
    let agent_id = AgentId::from_parts(SOURCE_NAME, cwd);

    let identity = || AgentEvent::Identity {
        agent_id,
        source: SOURCE_NAME.to_string(),
        // Mirrors the SessionStart arm: no usable upstream session id exists;
        // the cwd IS the session key.
        session_id: cwd.to_string(),
        cwd: Some(cwd.into()),
    };

    match event {
        // session_start fires once at TUI launch; message_submit fires on every
        // prompt. Both map to SessionStart: the reducer ignores it when the slot
        // exists, and the message_submit duplicate is the RESURRECT path — a
        // stale-swept session walks back in on its next prompt (CodeWhale has no
        // other re-creation signal; cf. the Codex/Reasonix arms).
        "session_start" | "message_submit" => Ok(vec![AgentEvent::SessionStart {
            agent_id,
            source: SOURCE_NAME.to_string(),
            session_id: cwd.to_string(),
            cwd: cwd.into(),
            parent_id: None,
        }]),
        "tool_call_before" => {
            let tool = obj.get("tool").and_then(|s| s.as_str()).unwrap_or("?");
            Ok(vec![
                identity(),
                AgentEvent::ActivityStart {
                    agent_id,
                    tool_use_id: None,
                    detail: Some(cw_tool_detail(tool, obj.get("tool_args"))),
                },
            ])
        }
        "tool_call_after" => Ok(vec![
            identity(),
            AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: None,
            },
        ]),
        "session_end" => Ok(vec![AgentEvent::SessionEnd {
            agent_id,
            as_child: false,
        }]),
        other => bail!("unsupported codewhale hook event: {other}"),
    }
}

/// The registry's `hook.custom` entry point. CodeWhale's envelope is ALIEN (no
/// `hook_event_name`/`session_id`), so per the `HookDecoding::custom` contract
/// it claims EVERY event reaching it — `.map(Some)`, never `Ok(None)` — and the
/// shared CC-shaped arms are unreachable for `_pixtuoid_source=codewhale`.
pub(crate) fn decode_cw_hook_custom(v: &Value) -> Result<Option<Vec<AgentEvent>>> {
    decode_cw_hook_payload(v).map(Some)
}

/// CodeWhale-side tool detail: the dispatch family is name-keyed (CodeWhale args
/// carry no `subagent_type`, so the shared semantic detection can't see it),
/// everything else gets a `"name: target"` display. `tool_args` arrives as the
/// raw `DEEPSEEK_TOOL_ARGS` JSON STRING (e.g. `{"command":"ls -la","cwd":"…"}`,
/// captured live), so it is parsed here before the target key lookup; an object
/// is also accepted defensively. Target keys, in priority order, match the
/// args CodeWhale's builtin tools emit: `exec_shell`→`command`, the file tools→
/// `file_path`/`path`, search→`pattern`, fetch→`url`.
fn cw_tool_detail(tool: &str, raw_args: Option<&Value>) -> ToolDetail {
    if SUBAGENT_TOOLS.contains(&tool) {
        return ToolDetail::Task;
    }
    // `tool_args` is a JSON string in the shim envelope; parse it. Accept a
    // bare object too (defensive — a future caller might embed it directly).
    let parsed: Option<Value> = match raw_args {
        Some(Value::String(s)) => serde_json::from_str(s).ok(),
        Some(v @ Value::Object(_)) => Some(v.clone()),
        _ => None,
    };
    let target = parsed
        .as_ref()
        .and_then(|a| {
            ["command", "file_path", "path", "pattern", "url"]
                .iter()
                .find_map(|k| a.get(k).and_then(|v| v.as_str()))
        })
        .map(|s| format!(": {}", ellipsize(s, MAX_TOOL_TARGET_CHARS)))
        .unwrap_or_default();
    ToolDetail::Generic {
        display: format!("{}{target}", ellipsize(tool, MAX_TOOL_TARGET_CHARS)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode_all(v: Value) -> Vec<AgentEvent> {
        decode_cw_hook_payload(&v).expect("decodes")
    }

    /// The payload's MAIN event — the lifecycle/activity event the arm maps to,
    /// i.e. the last decoded event (activity arms prepend an `Identity`).
    fn decode(v: Value) -> AgentEvent {
        decode_all(v).pop().expect("at least one event")
    }

    #[test]
    fn session_start_keys_on_cwd() {
        let ev = decode(json!({"event": "session_start", "cwd": "/Users/dev/cwproj"}));
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
                    AgentId::from_parts(SOURCE_NAME, "/Users/dev/cwproj")
                );
                assert_eq!(cwd, std::path::PathBuf::from("/Users/dev/cwproj"));
                assert_eq!(parent_id, None);
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn message_submit_is_the_resurrect_session_start() {
        // After a stale-sweep there is no other re-creation signal; the next
        // prompt must walk the agent back in.
        let ev = decode(json!({
            "event": "message_submit",
            "cwd": "/Users/dev/cwproj"
        }));
        assert!(matches!(ev, AgentEvent::SessionStart { agent_id, .. }
                if agent_id == AgentId::from_parts(SOURCE_NAME, "/Users/dev/cwproj")));
    }

    #[test]
    fn tool_call_before_is_activity_start_with_no_tool_id() {
        // CodeWhale hooks carry no usable tool-call id — tool_use_id must be
        // None, not synthesized. Live-captured exec_shell shape.
        let ev = decode(json!({
            "event": "tool_call_before",
            "cwd": "/repo",
            "tool": "exec_shell",
            "tool_args": "{\"command\":\"ls -la\",\"cwd\":\"/repo\"}"
        }));
        match ev {
            AgentEvent::ActivityStart {
                tool_use_id,
                detail,
                ..
            } => {
                assert_eq!(tool_use_id, None);
                assert_eq!(detail.unwrap().display(), "exec_shell: ls -la");
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn tool_args_object_form_is_also_accepted() {
        // Defensive: a future caller embedding tool_args as an object (not a
        // JSON string) must still resolve the target.
        let ev = decode(json!({
            "event": "tool_call_before",
            "cwd": "/repo",
            "tool": "read_file",
            "tool_args": {"path": "src/main.rs"}
        }));
        assert!(
            matches!(ev, AgentEvent::ActivityStart { detail: Some(d), .. }
            if d.display() == "read_file: src/main.rs")
        );
    }

    #[test]
    fn subagent_dispatch_family_maps_to_task() {
        for tool in ["agent_spawn", "spawn_agent"] {
            let ev = decode(json!({
                "event": "tool_call_before", "cwd": "/r",
                "tool": tool, "tool_args": "{\"prompt\":\"do a thing\"}"
            }));
            assert!(
                matches!(&ev, AgentEvent::ActivityStart { detail: Some(d), .. } if d.is_task()),
                "{tool} must map to ToolDetail::Task"
            );
        }
        // An ordinary tool stays Generic.
        let ev = decode(json!({
            "event": "tool_call_before", "cwd": "/r",
            "tool": "read_file", "tool_args": "{\"path\":\"x.rs\"}"
        }));
        assert!(matches!(&ev, AgentEvent::ActivityStart { detail: Some(d), .. } if !d.is_task()));
    }

    #[test]
    fn tool_call_after_is_activity_end() {
        let ev = decode(json!({
            "event": "tool_call_after", "cwd": "/r", "tool": "exec_shell"
        }));
        assert!(matches!(
            ev,
            AgentEvent::ActivityEnd {
                tool_use_id: None,
                ..
            }
        ));
    }

    #[test]
    fn session_end_maps_to_session_end() {
        let ev = decode(json!({"event": "session_end", "cwd": "/r"}));
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
        // Identity events INCLUDED) keys on the same cwd-derived AgentId — even
        // though CodeWhale's own session_id is inconsistent across events.
        let events = [
            json!({"event": "session_start", "cwd": "/Users/dev/p"}),
            json!({"event": "message_submit", "cwd": "/Users/dev/p"}),
            json!({"event": "tool_call_before", "cwd": "/Users/dev/p", "tool": "exec_shell",
                   "tool_args": "{\"command\":\"ls\"}"}),
            json!({"event": "tool_call_after", "cwd": "/Users/dev/p", "tool": "exec_shell"}),
            json!({"event": "session_end", "cwd": "/Users/dev/p"}),
        ];
        let ids: std::collections::BTreeSet<_> = events
            .iter()
            .flat_map(|v| decode_cw_hook_payload(v).unwrap())
            .map(|e| e.agent_id())
            .collect();
        assert_eq!(ids.len(), 1, "all events must coalesce to one AgentId");
    }

    // #221: hook-only, so a slot synthesized mid-turn has NO JSONL back-fill
    // path — the activity arms must attach the identity the payload carries
    // (cwd = source key = session key) ahead of the activity event, mirroring
    // the SessionStart arm so coalescing holds.
    #[test]
    fn activity_arms_prepend_identity_with_cwd_keyed_session() {
        for payload in [
            json!({"event": "tool_call_before", "cwd": "/Users/dev/p", "tool": "exec_shell",
                   "tool_args": "{\"command\":\"ls\"}"}),
            json!({"event": "tool_call_after", "cwd": "/Users/dev/p", "tool": "exec_shell"}),
        ] {
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
                        "cw hooks always know their workspace"
                    );
                }
                other => panic!("{name}: expected leading Identity, got {other:?}"),
            }
        }
    }

    // Ends/starts don't carry Identity beyond what they already encode:
    // SessionStart/message_submit already carry full identity, and an end for
    // an unknown agent proves nothing worth registering.
    #[test]
    fn session_lifecycle_events_carry_no_separate_identity() {
        for payload in [
            json!({"event": "session_start", "cwd": "/r"}),
            json!({"event": "message_submit", "cwd": "/r"}),
            json!({"event": "session_end", "cwd": "/r"}),
        ] {
            let name = payload["event"].clone();
            let events = decode_all(payload);
            assert_eq!(events.len(), 1, "{name}: exactly one event");
            assert!(
                !matches!(events[0], AgentEvent::Identity { .. }),
                "{name} must not emit a separate Identity"
            );
        }
    }

    #[test]
    fn empty_or_missing_cwd_is_malformed() {
        // An empty cwd would mint a phantom agent nothing coalesces with.
        assert!(decode_cw_hook_payload(&json!({"event": "session_end", "cwd": ""})).is_err());
        assert!(decode_cw_hook_payload(&json!({"event": "session_end"})).is_err());
    }

    #[test]
    fn unknown_event_bails_loudly() {
        // Registered-vs-decoded drift must surface, not silently drop. We
        // deliberately do NOT decode turn_end/mode_change/on_error/subagent_*.
        for ev in [
            "turn_end",
            "mode_change",
            "on_error",
            "subagent_spawn",
            "subagent_complete",
            "shell_env",
            "Bogus",
        ] {
            assert!(
                decode_cw_hook_payload(&json!({"event": ev, "cwd": "/r"})).is_err(),
                "{ev} must bail (not registered, must not decode silently)"
            );
        }
    }

    #[test]
    fn non_object_payload_is_malformed() {
        assert!(decode_cw_hook_payload(&json!("just a string")).is_err());
        assert!(decode_cw_hook_payload(&json!(42)).is_err());
    }

    #[test]
    fn tool_call_before_without_tool_displays_question_mark() {
        let ev = decode(json!({"event": "tool_call_before", "cwd": "/r"}));
        assert!(
            matches!(ev, AgentEvent::ActivityStart { detail: Some(d), .. }
            if d.display() == "?")
        );
    }

    #[test]
    fn malformed_tool_args_string_degrades_to_no_target() {
        // DEEPSEEK_TOOL_ARGS that isn't valid JSON must not panic — just no
        // target suffix.
        let ev = decode(json!({
            "event": "tool_call_before", "cwd": "/r",
            "tool": "exec_shell", "tool_args": "not json {"
        }));
        assert!(
            matches!(ev, AgentEvent::ActivityStart { detail: Some(d), .. }
            if d.display() == "exec_shell")
        );
    }

    #[test]
    fn long_tool_target_is_truncated_at_the_decode_boundary() {
        let long = "x".repeat(MAX_TOOL_TARGET_CHARS * 3);
        let args = format!("{{\"command\":\"{long}\"}}");
        let ev = decode(json!({
            "event": "tool_call_before", "cwd": "/r",
            "tool": "exec_shell", "tool_args": args
        }));
        match ev {
            AgentEvent::ActivityStart {
                detail: Some(d), ..
            } => {
                let display = d.display();
                assert!(display.starts_with("exec_shell: "));
                assert!(display.ends_with('…'));
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }
}
