//! Codex CLI source. Watches the Codex session transcript
//! (`~/.codex/sessions/**/rollout-<ts>-<UUID>.jsonl`) via `JsonlWatcher`.
//! Codex hooks already arrive through the shared hook socket (the shim stamps
//! `source=codex`); this source adds the JSONL lifecycle signals hooks lack —
//! most importantly the post-approval resume (`function_call_output`).
//!
//! Coalescing: hook events key `AgentId` on the hook `session_id`; this source
//! keys on the trailing UUID of the rollout filename. Verified equal
//! (hook.session_id == session_meta.id == filename UUID), so both transports
//! merge onto one sprite.

use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::source::decoder::{cwd_basename_label, make_tool_detail};
use crate::source::jsonl::JsonlWatcher;
use crate::source::{Activity, AgentEvent, Source, TaggedSender};
use crate::AgentId;

pub const SOURCE_NAME: &str = "codex";

/// Trailing canonical UUID (`8-4-4-4-12`) of a `rollout-<ts>-<UUID>.jsonl`
/// filename. Equals the hook payload's `session_id`, so hook and JSONL events
/// coalesce. Falls back to the full stem if no trailing UUID is present.
pub fn codex_id_from_path(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    // `.get()` (not `&stem[..]`) so a non-ASCII filename whose byte split
    // lands mid-codepoint returns None instead of panicking — this runs on
    // every file under the watched tree.
    let tail = stem.get(stem.len().saturating_sub(36)..).unwrap_or("");
    if is_uuid(tail) {
        tail.to_string()
    } else {
        stem.to_string()
    }
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, &c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Decode one transcript line. `tool_use_id` is always `None` so these events
/// are never suppressed by the hook-wins dedup (which keys on `tool_use_id`).
pub fn decode_codex_line(transcript_path: &str, source: &str, v: Value) -> Result<Vec<AgentEvent>> {
    let agent_id = AgentId::from_parts(source, &codex_id_from_path(Path::new(transcript_path)));
    let Some(obj) = v.as_object() else {
        return Ok(vec![]);
    };
    let outer = obj.get("type").and_then(|s| s.as_str()).unwrap_or("");
    let payload = obj.get("payload").and_then(|p| p.as_object());
    let inner = payload
        .and_then(|p| p.get("type"))
        .and_then(|s| s.as_str())
        .unwrap_or("");

    let start = |activity| AgentEvent::ActivityStart {
        agent_id,
        activity,
        tool_use_id: None,
        detail: None,
    };
    let end = || AgentEvent::ActivityEnd {
        agent_id,
        tool_use_id: None,
    };

    let out = match (outer, inner) {
        ("event_msg", "task_started") => vec![start(Activity::Typing)],
        ("response_item", "function_call") => {
            if function_call_needs_approval(payload) {
                vec![AgentEvent::Waiting {
                    agent_id,
                    reason: "permission".to_string(),
                }]
            } else {
                vec![codex_tool_start(agent_id, payload)]
            }
        }
        // Resume signals: a command/patch finished running after (auto-)approval.
        // function_call_output (response_item) is the modern form; exec_command_end
        // and patch_apply_end are the event_msg forms. Each is an ActivityStart so
        // the reducer clears any Waiting set by the permission gate.
        ("response_item", "function_call_output")
        | ("event_msg", "exec_command_end")
        | ("event_msg", "patch_apply_end") => {
            vec![start(Activity::Typing)]
        }
        ("event_msg", "task_complete") | ("event_msg", "turn_aborted") => vec![end()],
        _ => vec![],
    };
    Ok(out)
}

/// A Codex `function_call` requesting escalated sandbox permissions (`arguments`
/// is a JSON string carrying `sandbox_permissions: "require_escalated"`) is an
/// approval gate → Waiting. A bare `justification` is intentionally NOT a signal:
/// Codex can emit it on auto-approved commands too, and the hook `PermissionRequest`
/// is the primary Waiting trigger regardless — keying on it would false-Wait.
fn function_call_needs_approval(payload: Option<&Map<String, Value>>) -> bool {
    let Some(args_str) = payload
        .and_then(|p| p.get("arguments"))
        .and_then(|a| a.as_str())
    else {
        return false;
    };
    let args = match serde_json::from_str::<Value>(args_str) {
        Ok(v) => v,
        Err(e) => {
            // A complete line that parsed as JSON but whose nested `arguments`
            // string doesn't is unusual; log (don't panic) so it's diagnosable.
            tracing::debug!("codex function_call arguments not parseable: {e}");
            return false;
        }
    };
    args.get("sandbox_permissions").and_then(|s| s.as_str()) == Some("require_escalated")
}

fn codex_tool_start(agent_id: AgentId, payload: Option<&Map<String, Value>>) -> AgentEvent {
    let name = payload
        .and_then(|p| p.get("name"))
        .and_then(|s| s.as_str())
        .unwrap_or("tool");
    AgentEvent::ActivityStart {
        agent_id,
        activity: Activity::Typing,
        tool_use_id: None,
        // Codex tool calls are function_calls, never subagent dispatches (those
        // arrive as the SubagentStart hook), so no `subagent_type` to pass.
        detail: Some(make_tool_detail(name, None)),
    }
}

pub fn derive_codex_label(_path: &Path, _source: &str, cwd: &Path) -> String {
    cwd_basename_label("cx", cwd).unwrap_or_else(|| "cx".to_string())
}

/// Codex writes no session-end marker; the reducer's stale-sweep reaps dead
/// sessions. Always false (defer to mtime window + stale-sweep).
fn codex_session_ended(_tail: &[u8]) -> bool {
    false
}

/// Source that watches the Codex session transcript directory.
pub struct CodexSource {
    pub sessions_root: PathBuf,
}

impl CodexSource {
    pub fn default_paths() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        Self {
            sessions_root: PathBuf::from(format!("{home}/.codex/sessions")),
        }
    }
}

#[async_trait]
impl Source for CodexSource {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        let watcher = JsonlWatcher::new(
            self.sessions_root.clone(),
            SOURCE_NAME.to_string(),
            decode_codex_line,
            derive_codex_label,
            codex_session_ended,
        )
        .with_id_deriver(codex_id_from_path);
        watcher.run(tx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(line: Value) -> Vec<AgentEvent> {
        decode_codex_line(
            "/x/rollout-1-019e7762-9ded-7e33-be41-946ecf105bf4.jsonl",
            SOURCE_NAME,
            line,
        )
        .unwrap()
    }

    #[test]
    fn task_started_is_activity_start() {
        let out = ev(json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"t"}}));
        assert!(matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]));
    }

    #[test]
    fn function_call_output_resumes_work() {
        // THE fix: resume signal must be an ActivityStart (clears Waiting in the reducer).
        let out = ev(
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"c","output":"ok"}}),
        );
        assert!(matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]));
    }

    #[test]
    fn patch_apply_end_resumes_work() {
        // A file-edit's resume signal (after patch approval) — mirrors the
        // exec resume so the reducer clears Waiting for patch flows too.
        let out =
            ev(json!({"type":"event_msg","payload":{"type":"patch_apply_end","success":true}}));
        assert!(matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]));
    }

    #[test]
    fn escalated_function_call_is_waiting() {
        let args =
            r#"{"cmd":"date","sandbox_permissions":"require_escalated","justification":"allow?"}"#;
        let out = ev(
            json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":args}}),
        );
        assert!(matches!(out.as_slice(), [AgentEvent::Waiting { .. }]));
    }

    #[test]
    fn plain_function_call_is_activity_start() {
        let args = r#"{"cmd":"ls"}"#;
        let out = ev(
            json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":args}}),
        );
        assert!(matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]));
    }

    #[test]
    fn justification_without_escalation_is_not_waiting() {
        // A bare `justification` (no `require_escalated`) is an auto-approved
        // command, not a permission gate — must decode to working, not Waiting.
        let args = r#"{"cmd":"ls","justification":"because"}"#;
        let out = ev(
            json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":args}}),
        );
        assert!(
            matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]),
            "{out:?}"
        );
    }

    #[test]
    fn malformed_arguments_does_not_panic_and_starts_work() {
        let out = ev(
            json!({"type":"response_item","payload":{"type":"function_call","name":"x","arguments":"{not json"}}),
        );
        assert!(matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]));
    }

    #[test]
    fn task_complete_and_abort_end_activity() {
        for t in ["task_complete", "turn_aborted"] {
            let out = ev(json!({"type":"event_msg","payload":{"type":t,"turn_id":"t"}}));
            assert!(
                matches!(out.as_slice(), [AgentEvent::ActivityEnd { .. }]),
                "{t}"
            );
        }
    }

    #[test]
    fn session_meta_and_unknown_emit_nothing() {
        assert!(ev(json!({"type":"session_meta","payload":{"id":"u","cwd":"/r"}})).is_empty());
        assert!(ev(json!({"type":"event_msg","payload":{"type":"token_count"}})).is_empty());
    }

    #[test]
    fn label_is_cx_basename() {
        assert_eq!(
            derive_codex_label(
                Path::new("/x.jsonl"),
                SOURCE_NAME,
                Path::new("/Users/me/dotfiles")
            ),
            "cx·dotfiles"
        );
        assert_eq!(
            derive_codex_label(Path::new("/x.jsonl"), SOURCE_NAME, Path::new("")),
            "cx"
        );
    }

    #[test]
    fn id_from_rollout_path_is_trailing_uuid() {
        let p = Path::new(
            "/Users/me/.codex/sessions/2026/05/29/rollout-2026-05-29T22-36-52-019e7762-9ded-7e33-be41-946ecf105bf4.jsonl",
        );
        // Must equal the hook session_id for coalescing.
        assert_eq!(
            codex_id_from_path(p),
            "019e7762-9ded-7e33-be41-946ecf105bf4"
        );
    }

    // Coalescing guard: `codex_id_from_path` is invoked in THREE places that must
    // agree — the per-line decode (here), the watcher's `with_id_deriver`
    // (CodexSource::run), and the fixture test above. If the per-line decode ever
    // keys differently from the deriver, one Codex session splits into two
    // sprites. Pin the per-line AgentId to the deriver's output directly.
    #[test]
    fn decode_line_keys_agent_id_on_codex_id_from_path() {
        let path = "/x/rollout-1-019e7762-9ded-7e33-be41-946ecf105bf4.jsonl";
        let events = decode_codex_line(
            path,
            SOURCE_NAME,
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"t"}}),
        )
        .unwrap();
        let expected = AgentId::from_parts(SOURCE_NAME, &codex_id_from_path(Path::new(path)));
        assert_eq!(
            events[0].agent_id(),
            expected,
            "decode_codex_line must key its AgentId on codex_id_from_path (the deriver)"
        );
    }

    #[test]
    fn id_falls_back_to_stem_without_uuid() {
        let p = Path::new("/tmp/notarollout.jsonl");
        assert_eq!(codex_id_from_path(p), "notarollout");
    }

    #[test]
    fn id_handles_non_ascii_filename_without_panic() {
        // The deriver runs on every file under ~/.codex/sessions; a non-ASCII
        // stem whose len-36 byte split lands mid-codepoint must not panic.
        let p = Path::new("/tmp/rollout-日本語のとてもながいファイルめい.jsonl");
        let _ = codex_id_from_path(p);
    }
}
