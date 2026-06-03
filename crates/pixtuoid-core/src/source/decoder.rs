//! Shared decoder utilities used by per-source decoders (CC, Antigravity).
//! Hook payload decoding lives here because the hook socket is shared.

use std::path::Path;

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use crate::source::{Activity, AgentEvent, ToolDetail};
use crate::AgentId;

/// `"{prefix}·{basename}"` from a working directory, or `None` when `cwd` is
/// empty / the filesystem root / has no final component. The cwd-basename label
/// rule, shared by the per-source derivers (cc / cx / ag) so it lives once; each
/// source supplies its 2-char prefix and its own fallback for the `None` case
/// (CC falls back to its project dir, codex/antigravity to a bare prefix).
pub(crate) fn cwd_basename_label(prefix: &str, cwd: &Path) -> Option<String> {
    if cwd == Path::new("") || cwd == Path::new("/") {
        return None;
    }
    let base = cwd.file_name().and_then(|n| n.to_str())?;
    Some(format!("{prefix}·{base}"))
}

pub fn decode_hook_payload(v: Value) -> Result<AgentEvent> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("hook payload must be an object"))?;
    let event = obj
        .get("hook_event_name")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("missing hook_event_name"))?;

    // `.filter(non-empty)`: an empty session_id passes `as_str` but, for Codex
    // (which keys the AgentId on session_id), would mint a phantom agent that
    // never coalesces with any rollout — reject it as malformed (same idiom as
    // the SubagentStart agent_id guard).
    let session_id = obj
        .get("session_id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("missing/empty session_id"))?
        .to_string();
    // CLI attribution comes ONLY from the shim-owned `_pixtuoid_source` (the
    // shim stamps it from `PIXTUOID_SOURCE`). We must NOT read the public
    // `source` field: CC's SessionStart payload uses `source` for the start
    // *reason* (startup/resume/clear/compact), which would namespace the agent
    // under "startup" and split it from the claude-code-keyed tool/JSONL/
    // SessionEnd events (an un-reapable ghost). Absent the private key (bare
    // `pixtuoid-hook` with no env, i.e. CC), default to claude-code.
    let source = obj
        .get("_pixtuoid_source")
        .and_then(|s| s.as_str())
        .unwrap_or(crate::source::claude_code::SOURCE_NAME);
    // `transcript_path` is the preferred stable per-session key for CC (its hook
    // and JSONL both carry the same transcript path, so they coalesce on it).
    // Codex is different: its hooks send `transcript_path` as `string | null`,
    // and its JSONL source keys on the rollout-filename UUID (== `session_id`).
    // So Codex MUST key on `session_id` regardless of any `transcript_path`, or
    // hook and JSONL events would hash to different AgentIds (two sprites).
    let id_key = if source == crate::source::codex::SOURCE_NAME {
        session_id.as_str()
    } else {
        obj.get("transcript_path")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(session_id.as_str())
    };
    let agent_id = AgentId::from_parts(source, id_key);

    match event {
        "SessionStart" => {
            let cwd = obj.get("cwd").and_then(|s| s.as_str()).unwrap_or("").into();
            let source = source.to_string();
            Ok(AgentEvent::SessionStart {
                agent_id,
                source,
                session_id,
                cwd,
                parent_id: None,
            })
        }
        "PreToolUse" => {
            let tool_name = obj.get("tool_name").and_then(|s| s.as_str()).unwrap_or("?");
            let tool_use_id = obj
                .get("tool_use_id")
                .and_then(|s| s.as_str())
                .map(String::from);
            Ok(AgentEvent::ActivityStart {
                agent_id,
                activity: Activity::Typing,
                tool_use_id,
                detail: Some(make_tool_detail(tool_name, obj.get("tool_input"))),
            })
        }
        "PostToolUse" => {
            let tool_use_id = obj
                .get("tool_use_id")
                .and_then(|s| s.as_str())
                .map(String::from);
            Ok(AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id,
            })
        }
        "Notification" => {
            let msg = obj
                .get("message")
                .and_then(|s| s.as_str())
                .unwrap_or("waiting");
            Ok(AgentEvent::Waiting {
                agent_id,
                reason: msg.into(),
            })
        }
        // Codex's permission prompt is a "waiting on the human" signal — maps to
        // the same Waiting state as Claude's Notification.
        "PermissionRequest" => Ok(AgentEvent::Waiting {
            agent_id,
            reason: "permission".into(),
        }),
        // Codex turn lifecycle. Verified live (Codex 0.135): the ONLY hook events
        // that fire are UserPromptSubmit + Stop — SessionStart and PreToolUse do
        // NOT fire. So UserPromptSubmit is our agent-creation signal: emit
        // SessionStart from its cwd (idempotent in the reducer — ignored if the
        // agent already exists). The fresh `last_event_at` makes the cx· agent
        // show seated-thinking, so it reads as "working" right after a prompt.
        "UserPromptSubmit" => {
            let cwd = obj.get("cwd").and_then(|s| s.as_str()).unwrap_or("").into();
            Ok(AgentEvent::SessionStart {
                agent_id,
                source: source.to_string(),
                session_id,
                cwd,
                parent_id: None,
            })
        }
        // Turn end — Codex fires no SessionEnd, so keep the slot; just settle to
        // idle (harmless no-op if the agent is already idle).
        "Stop" => Ok(AgentEvent::ActivityEnd {
            agent_id,
            tool_use_id: None,
        }),
        "SessionEnd" => Ok(AgentEvent::SessionEnd { agent_id }),
        // Codex subagents (`spawn_agent`): these two hooks are the ONLY carrier
        // of the parent<->child link. The subagent owns a SEPARATE rollout file
        // (filename UUID == this payload's `agent_id`), so the JSONL watcher
        // already renders it — but orphaned: a flat `~/.codex/sessions/.../
        // rollout-*.jsonl` path has no `/subagents/` for `detect_parent_id`. So
        // key the CHILD on `agent_id` (coalesces with that rollout) and link it
        // to the parent `session_id`. It then joins the same scope tree
        // (cascade / liveness / readiness) as a CC subagent — source-agnostic.
        // Wire format captured live (Codex 0.135, gpt-5.5).
        "SubagentStart" => {
            // `.filter(non-empty)`: an empty `agent_id` string passes `as_str`
            // but would key a phantom child that never coalesces with the real
            // rollout — reject it as malformed instead.
            let child = obj
                .get("agent_id")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow!("SubagentStart missing/empty agent_id"))?;
            let cwd = obj.get("cwd").and_then(|s| s.as_str()).unwrap_or("").into();
            Ok(AgentEvent::SessionStart {
                agent_id: AgentId::from_parts(source, child),
                source: source.to_string(),
                session_id: child.to_string(),
                cwd,
                parent_id: Some(AgentId::from_parts(source, session_id.as_str())),
            })
        }
        // Subagent done — end the CHILD promptly (else its rollout lingers to
        // the 30-min stale-sweep). Keyed on `agent_id`; the parent keeps running.
        // Best-effort: if this hook wins the race against the child's own slot
        // creation, the `SessionEnd` is a harmless no-op (no slot yet) and the
        // later-created orphan falls back to the stale-sweep — Codex has no
        // durable subagent-stop marker, so "prompt" is not guaranteed.
        "SubagentStop" => {
            let child = obj
                .get("agent_id")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow!("SubagentStop missing/empty agent_id"))?;
            Ok(AgentEvent::SessionEnd {
                agent_id: AgentId::from_parts(source, child),
            })
        }
        other => bail!("unsupported hook_event_name: {other}"),
    }
}

pub(crate) fn make_tool_detail(tool_name: &str, input: Option<&Value>) -> ToolDetail {
    // Detect the subagent-dispatch tool SEMANTICALLY, by the PRESENCE of a
    // `subagent_type` input field. The dispatch tool was renamed `Task` →
    // `Agent` (CC v2.1.63, undocumented) and upstream can rename it again, but
    // the field is stable. Key on presence (not value): a renamed tool emitting
    // `subagent_type: null` is still caught AND surfaces the drift breadcrumb —
    // the one drift we most need to see. Known names are the fallback for the
    // rare input-less call. The reducer keys subagent-leak suppression
    // (`active_tasks`) and b1 Task-drain completion on `is_task()`, so a missed
    // dispatch silently disables both for real subagents.
    let has_subagent_type = input.and_then(|v| v.get("subagent_type")).is_some();
    let known_name = tool_name == "Task" || tool_name == "Agent";
    if has_subagent_type || known_name {
        // Drift breadcrumb: a dispatch under a name we don't recognise means
        // upstream renamed the tool again. Semantic detection keeps us working;
        // this surfaces the new name so the known set / docs can be updated.
        if has_subagent_type && !known_name {
            tracing::debug!(
                tool = %tool_name,
                "subagent-dispatch tool has an unrecognized name (handled via subagent_type); upstream may have renamed it"
            );
        }
        ToolDetail::Task
    } else {
        // `target` (the file/cmd descriptor) is only meaningful on the Generic
        // branch, so derive it here lazily — no wasted alloc on the dispatch
        // path, and callers can't pass a `target` computed from a different
        // `input` than the one used for detection.
        ToolDetail::Generic {
            display: format!("{tool_name}{}", describe_tool_target(tool_name, input)),
        }
    }
}

pub(crate) fn describe_tool_target(tool: &str, input: Option<&Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    let key = match tool {
        "Write" | "Edit" | "MultiEdit" | "Read" => "file_path",
        "Bash" => "command",
        "Grep" | "Glob" => "pattern",
        _ => "",
    };
    if key.is_empty() {
        return String::new();
    }
    let Some(s) = input.get(key).and_then(|v| v.as_str()) else {
        return String::new();
    };
    let total_chars = s.chars().count();
    let mut s: String = s.chars().take(40).collect();
    if total_chars > 40 {
        s.push('…');
    }
    format!(": {s}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_session_start_without_transcript_path_uses_session_id() {
        // Codex sends transcript_path as string|null; decode must still work,
        // namespacing the AgentId under the explicit "codex" source.
        let ev = decode_hook_payload(json!({
            "hook_event_name": "SessionStart",
            "session_id": "codex-sess-1",
            "_pixtuoid_source": "codex",
            "cwd": "/Users/me/work/myrepo"
        }))
        .expect("decodes without transcript_path");
        match ev {
            AgentEvent::SessionStart {
                agent_id,
                source,
                cwd,
                ..
            } => {
                assert_eq!(source, "codex");
                assert_eq!(agent_id, AgentId::from_parts("codex", "codex-sess-1"));
                assert_eq!(cwd, std::path::PathBuf::from("/Users/me/work/myrepo"));
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn codex_permission_request_maps_to_waiting() {
        let ev = decode_hook_payload(json!({
            "hook_event_name": "PermissionRequest",
            "session_id": "s",
            "_pixtuoid_source": "codex"
        }))
        .expect("decodes");
        assert!(matches!(ev, AgentEvent::Waiting { .. }));
    }

    #[test]
    fn codex_user_prompt_submit_creates_agent_via_session_start() {
        // Codex 0.135 fires NO SessionStart/PreToolUse — only UserPromptSubmit +
        // Stop (verified live). So UserPromptSubmit is the agent-creation signal:
        // it carries source + cwd and decodes to a SessionStart the reducer turns
        // into a cx· agent.
        let ev = decode_hook_payload(json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "codex-sess",
            "_pixtuoid_source": "codex",
            "cwd": "/Users/me/work/myrepo",
            "transcript_path": "/Users/me/.codex/sessions/x.jsonl"
        }))
        .expect("decodes");
        match ev {
            AgentEvent::SessionStart {
                agent_id,
                source,
                cwd,
                ..
            } => {
                assert_eq!(source, "codex");
                assert_eq!(cwd, std::path::PathBuf::from("/Users/me/work/myrepo"));
                // Coalescing contract: Codex keys on session_id, NOT the
                // (here non-null) transcript_path — so hook events and the
                // JSONL source (which keys on the rollout-filename UUID ==
                // session_id) hash to the SAME AgentId. Keying on the path
                // would produce two sprites for one session.
                assert_eq!(agent_id, AgentId::from_parts("codex", "codex-sess"));
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn codex_stop_maps_to_activity_end() {
        let ev = decode_hook_payload(json!({
            "hook_event_name": "Stop",
            "session_id": "s",
            "_pixtuoid_source": "codex"
        }))
        .expect("decodes");
        assert!(matches!(ev, AgentEvent::ActivityEnd { .. }));
    }

    // Regression: CC's SessionStart hook payload carries `source: "startup"`
    // (the start *reason* — startup/resume/clear/compact), which is NOT a CLI
    // name. Reading it as the CLI source namespaced the agent under "startup",
    // splitting it from the claude-code-keyed tool/JSONL/SessionEnd events — an
    // un-reapable `startup·…` ghost. The public `source` field must never drive
    // CLI attribution; only the shim-owned `_pixtuoid_source` does.
    #[test]
    fn cc_session_start_reason_source_does_not_hijack_cli_source() {
        let tp = "/Users/me/.claude/projects/x/ses-abc.jsonl";
        let ev = decode_hook_payload(json!({
            "hook_event_name": "SessionStart",
            "session_id": "ses-abc",
            "transcript_path": tp,
            "cwd": "/repo",
            "source": "startup"
        }))
        .expect("decodes");
        match ev {
            AgentEvent::SessionStart {
                agent_id, source, ..
            } => {
                assert_eq!(source, crate::source::claude_code::SOURCE_NAME);
                assert_eq!(
                    agent_id,
                    AgentId::from_parts(crate::source::claude_code::SOURCE_NAME, tp),
                    "must coalesce with tool/JSONL/SessionEnd events on the claude-code id"
                );
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn pixtuoid_source_private_key_drives_cli_attribution() {
        // The shim stamps the trusted CLI source under `_pixtuoid_source`.
        let ev = decode_hook_payload(json!({
            "hook_event_name": "Stop",
            "session_id": "codex-sess",
            "_pixtuoid_source": "codex"
        }))
        .expect("decodes");
        assert_eq!(
            ev.agent_id(),
            AgentId::from_parts("codex", "codex-sess"),
            "Codex Stop keys on session_id under the codex namespace"
        );
    }

    #[test]
    fn absent_source_still_defaults_to_claude() {
        // A payload with no `source` (legacy / un-stamped) must remain CC.
        let ev = decode_hook_payload(json!({
            "hook_event_name": "SessionStart",
            "session_id": "s",
            "transcript_path": "/p/a.jsonl",
            "cwd": "/repo"
        }))
        .expect("decodes");
        match ev {
            AgentEvent::SessionStart { source, .. } => {
                assert_eq!(source, crate::source::claude_code::SOURCE_NAME)
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }
}
