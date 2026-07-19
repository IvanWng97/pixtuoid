//! Kimi Code CLI source — HOOK-ONLY (no transcript watcher).
//!
//! Moonshot AI's Kimi Code CLI (`kimi`, repo `MoonshotAI/kimi-code` — the
//! successor to the sunsetting `MoonshotAI/kimi-cli`) is a terminal coding agent
//! descended from Claude Code's hook design. Two candidate seams, only one a
//! stable *passive-observer* surface:
//!
//! - **Session transcripts DO exist** — `<KIMI_CODE_HOME>/sessions/<workDirKey>/
//!   <sessionId>/agents/main/wire.jsonl` (default home `~/.kimi-code`) — but the
//!   wire format is EXPLICITLY unstable: it carries a `WIRE_PROTOCOL_VERSION` in a
//!   first-line `metadata` envelope, ships a `wire/migration/` module, the docs
//!   say *"do not manually edit files inside `sessions/`"*, and its message/tool
//!   op `type` strings are undocumented + version-specific (the new `~/.kimi-code`
//!   format already broke ccusage). So pixtuoid does NOT tail it — a future
//!   watcher seam only, gated on the first-line `protocol_version`.
//! - **Hooks** (`<KIMI_CODE_HOME>/config.toml` `[[hooks]]`) — the documented,
//!   supported surface, and Kimi's envelope is CLAUDE-CODE-SHAPED: snake_case
//!   `hook_event_name`/`session_id`/`cwd`/`tool_name`/`tool_input` with PascalCase
//!   event VALUES. THIS is the seam.
//!
//! Because the envelope is CC-shaped, most events ride the SHARED CC-shaped arms
//! (`decoder::decode_hook_payload`) UNCHANGED, keyed on `session_id`
//! (`IdKey::SessionId`, in the registry row): `SessionStart` → register,
//! `PreToolUse`/`PostToolUse` → Identity + Activity, `PermissionRequest` →
//! Waiting, `Stop` → idle, `SessionEnd` → remove. This module's custom `Extend`
//! decoder handles ONLY the two events the shared arms don't know — Kimi's
//! `PostToolUseFailure` / `StopFailure` variants (a failed tool/turn fires the
//! `*Failure` event; it must still close the activity or the sprite lingers
//! Active — the Cursor `postToolUseFailure` lesson) — and DECLINES (`Ok(None)`)
//! for everything else so it falls through.
//!
//! Hook payloads arrive on the shared hook socket stamped
//! `_pixtuoid_source: "kimi"`. **Subagents** (Kimi's "agent swarm") are
//! deliberately NOT modeled: the `SubagentStart`/`SubagentStop` payload shape is
//! uncaptured, so sessions render FLAT (like Cursor/Hermes) — a future byte-real
//! capture can add nesting. The install target registers only the events decoded
//! here (`install/kimi.rs` `KIMI_EVENTS`), pinned by
//! `every_registered_kimi_event_decodes` (registered ⊆ decodable) AND
//! `kimi_events_pins_the_exact_registered_set` (the membership itself — so
//! silently DROPPING a registered event can't ship green past the
//! one-directional drift-watch, which reads the same const).

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde_json::{Map, Value};

use crate::source::AgentEvent;
use crate::AgentId;

pub const SOURCE_NAME: &str = "kimi";

/// Decode the two Kimi hook events the shared CC-shaped arms don't handle, and
/// DECLINE (`Ok(None)`) for the rest so they fall through to
/// `decoder::decode_hook_payload`'s shared arms (Kimi's envelope is Claude-Code-
/// shaped, so `SessionStart`/`PreToolUse`/`PostToolUse`/`PermissionRequest`/
/// `Stop`/`SessionEnd` decode there natively, keyed on `session_id`).
///
/// The two claimed events are Kimi's failure variants (no CC equivalent):
/// - `PostToolUseFailure` — a FAILED tool fires this (upstream lists it beside
///   `PostToolUse`). Close the activity like `PostToolUse` (Identity-backed —
///   proof-of-life for a hook-only source, #221) so the tool's `ActivityStart`
///   doesn't linger Active. Firing BOTH `PostToolUse` and this is harmless: the
///   second `ActivityEnd` no-ops on an already-idle slot.
/// - `StopFailure` — a failed turn end; settle to idle like `Stop`, with NO
///   Identity (an end proves nothing worth registering — mirrors the shared
///   `Stop` arm's boundary).
pub(crate) fn decode_kimi_hook_custom(v: &Value) -> Result<Option<Vec<AgentEvent>>> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("kimi hook payload must be an object"))?;
    match obj.get("hook_event_name").and_then(|s| s.as_str()) {
        Some("PostToolUseFailure") => {
            let session_id = kimi_session_id(obj)?;
            let agent_id = AgentId::from_parts(SOURCE_NAME, &session_id);
            let cwd = obj
                .get("cwd")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
                .map(PathBuf::from);
            let tool_use_id = obj
                .get("tool_use_id")
                .and_then(|s| s.as_str())
                .map(String::from);
            Ok(Some(vec![
                AgentEvent::Identity {
                    agent_id,
                    source: SOURCE_NAME.to_string(),
                    session_id,
                    cwd,
                    pid: None,
                },
                AgentEvent::ActivityEnd {
                    agent_id,
                    tool_use_id,
                },
            ]))
        }
        Some("StopFailure") => {
            let agent_id = AgentId::from_parts(SOURCE_NAME, &kimi_session_id(obj)?);
            Ok(Some(vec![AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: None,
            }]))
        }
        // Every other event rides the shared CC-shaped arms.
        _ => Ok(None),
    }
}

/// The non-empty `session_id` a claimed failure event keys on — the SAME key the
/// shared arms (`IdKey::SessionId`) derive, so a failure event coalesces with the
/// session's other events. `Err` (not `Ok(None)`) on a missing/empty id: we've
/// committed to CLAIMING this event, and falling through would hit the shared
/// arms' "unsupported hook_event_name" bail with a misleading message.
fn kimi_session_id(obj: &Map<String, Value>) -> Result<String> {
    obj.get("session_id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| anyhow!("kimi failure-hook payload missing/empty session_id"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn post_tool_use_failure_closes_activity_with_identity() {
        // A failed tool fires PostToolUseFailure INSTEAD OF / alongside PostToolUse
        // — either way it must close the activity, Identity-backed (proof-of-life
        // for a hook-only source), keyed on session_id.
        let evs = decode_kimi_hook_custom(&json!({
            "hook_event_name": "PostToolUseFailure",
            "session_id": "session_abc",
            "cwd": "/Users/dev/proj",
            "tool_name": "Bash",
            "tool_use_id": "t1"
        }))
        .expect("decodes")
        .expect("claims the event");
        assert_eq!(evs.len(), 2, "Identity + ActivityEnd, got {evs:?}");
        match &evs[0] {
            AgentEvent::Identity {
                agent_id,
                source,
                session_id,
                cwd,
                pid: None,
            } => {
                assert_eq!(*agent_id, AgentId::from_parts(SOURCE_NAME, "session_abc"));
                assert_eq!(source, SOURCE_NAME);
                assert_eq!(session_id, "session_abc");
                assert_eq!(
                    cwd.as_deref(),
                    Some(std::path::Path::new("/Users/dev/proj"))
                );
            }
            other => panic!("expected leading Identity, got {other:?}"),
        }
        match &evs[1] {
            AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id,
            } => {
                assert_eq!(*agent_id, AgentId::from_parts(SOURCE_NAME, "session_abc"));
                assert_eq!(tool_use_id.as_deref(), Some("t1"));
            }
            other => panic!("expected ActivityEnd, got {other:?}"),
        }
    }

    #[test]
    fn post_tool_use_failure_without_cwd_has_none_cwd() {
        let evs = decode_kimi_hook_custom(&json!({
            "hook_event_name": "PostToolUseFailure",
            "session_id": "s",
            "cwd": "",
            "tool_name": "Read"
        }))
        .unwrap()
        .unwrap();
        match &evs[0] {
            AgentEvent::Identity { cwd, .. } => {
                assert_eq!(*cwd, None, "empty cwd must map to None, not Some(\"\")")
            }
            other => panic!("expected Identity, got {other:?}"),
        }
    }

    #[test]
    fn stop_failure_is_activity_end_without_identity() {
        // A failed turn end settles to idle like Stop, with no Identity (an end
        // proves nothing worth registering).
        let evs = decode_kimi_hook_custom(&json!({
            "hook_event_name": "StopFailure",
            "session_id": "s"
        }))
        .unwrap()
        .unwrap();
        assert_eq!(evs.len(), 1);
        assert!(
            matches!(&evs[0], AgentEvent::ActivityEnd { tool_use_id: None, agent_id }
                if *agent_id == AgentId::from_parts(SOURCE_NAME, "s")),
            "got {evs:?}"
        );
    }

    #[test]
    fn non_failure_events_decline_to_the_shared_arms() {
        // The custom decoder claims ONLY the two failure variants; everything else
        // must DECLINE (Ok(None)) so decode_hook_payload falls through to the shared
        // CC-shaped arms. The registered lifecycle events then decode there natively;
        // SubagentStart is included to prove the decoder's narrow scope — it is
        // deliberately UNregistered, so at the shared arms it would bail (not our
        // concern here; the point is the custom decoder doesn't claim it).
        for ev in [
            "SessionStart",
            "PreToolUse",
            "PostToolUse",
            "PermissionRequest",
            "Stop",
            "SessionEnd",
            "UserPromptSubmit",
            "SubagentStart",
        ] {
            let out = decode_kimi_hook_custom(&json!({
                "hook_event_name": ev,
                "session_id": "s",
                "cwd": "/repo"
            }))
            .expect("decodes");
            assert!(
                out.is_none(),
                "{ev} must be declined (Ok(None)) by the custom decoder, got {out:?}"
            );
        }
    }

    #[test]
    fn claimed_failure_event_without_session_id_errs() {
        // Having committed to claiming the event, a missing/empty session_id is a
        // hard error (not a silent Ok(None) fall-through to a misleading bail).
        for ev in ["PostToolUseFailure", "StopFailure"] {
            assert!(
                decode_kimi_hook_custom(&json!({ "hook_event_name": ev, "cwd": "/repo" })).is_err(),
                "{ev} with no session_id must err"
            );
            assert!(
                decode_kimi_hook_custom(&json!({ "hook_event_name": ev, "session_id": "" }))
                    .is_err(),
                "{ev} with empty session_id must err"
            );
        }
    }

    #[test]
    fn non_object_payload_errs() {
        assert!(decode_kimi_hook_custom(&json!("a string")).is_err());
        assert!(decode_kimi_hook_custom(&json!(42)).is_err());
    }

    #[test]
    fn every_event_coalesces_to_one_agent_id_end_to_end() {
        // End-to-end through the shared dispatcher: the failure variant (custom
        // decoder) and the CC-shaped lifecycle events (shared arms) all key on the
        // same session_id-derived AgentId under the "kimi" namespace.
        use crate::source::decoder::decode_hook_payload;
        let sid = "session_abc";
        let payloads = [
            json!({"hook_event_name": "SessionStart", "session_id": sid, "cwd": "/repo", "_pixtuoid_source": "kimi"}),
            json!({"hook_event_name": "PreToolUse", "session_id": sid, "cwd": "/repo", "tool_name": "Bash", "tool_input": {"command": "ls"}, "tool_use_id": "t1", "_pixtuoid_source": "kimi"}),
            json!({"hook_event_name": "PostToolUseFailure", "session_id": sid, "cwd": "/repo", "tool_name": "Bash", "tool_use_id": "t1", "_pixtuoid_source": "kimi"}),
            json!({"hook_event_name": "PermissionRequest", "session_id": sid, "cwd": "/repo", "_pixtuoid_source": "kimi"}),
            json!({"hook_event_name": "SessionEnd", "session_id": sid, "cwd": "/repo", "_pixtuoid_source": "kimi"}),
        ];
        let ids: std::collections::BTreeSet<_> = payloads
            .into_iter()
            .flat_map(|v| decode_hook_payload(v).expect("decodes"))
            .map(|e| e.agent_id())
            .collect();
        assert_eq!(
            ids,
            std::collections::BTreeSet::from([AgentId::from_parts(SOURCE_NAME, sid)]),
            "all kimi events must coalesce to one AgentId"
        );
    }
}
