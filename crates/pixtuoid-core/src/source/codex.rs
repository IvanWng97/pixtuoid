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
use serde_json::{Map, Value};

use crate::source::decoder::{ellipsize, make_tool_detail, MAX_DECODED_FIELD_CHARS};
use crate::source::AgentEvent;
use crate::AgentId;

// The runtime half (liveness probe + `CodexSource` watcher wiring) — ONE gate
// for the whole `native` layer of this source; the re-export keeps the
// pre-split `source::codex::{CodexSource, live_codex_rollout_ids}` paths.
#[cfg(feature = "native")]
mod native;
#[cfg(feature = "native")]
pub use native::{live_codex_rollout_ids, CodexSource};

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

/// Codex's source-specific hook arms — `SubagentStart`/`SubagentStop`. These
/// change the event's SUBJECT (the child's AgentId, not the session's), which
/// the shared CC-shaped arms in `decoder::decode_hook_payload` cannot express;
/// every other Codex hook event falls through (`Ok(None)`) to those shared
/// arms. Dispatched via `registry::HookDecoding::custom`. The parent link
/// carried here is the ONLY one a flat Codex rollout gets — see the module
/// doc and the wire capture pinned in `tests/sources/codex/mod.rs`.
pub(crate) fn decode_codex_hook_custom(v: &Value) -> Result<Option<Vec<AgentEvent>>> {
    use anyhow::anyhow;
    let Some(obj) = v.as_object() else {
        return Ok(None); // shared path reports the malformed payload
    };
    let event = obj
        .get("hook_event_name")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    // Per the registry's custom-decoder contract: claim our two events FULLY
    // (Err on malformed instances), Ok(None) for everything else. An empty
    // `session_id` or `agent_id` would mint a phantom that never coalesces
    // with the real rollout — reject rather than decode.
    let guards = |obj: &Map<String, Value>| -> Result<(String, Option<String>)> {
        let session_id = obj
            .get("session_id")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("missing/empty session_id"))?
            .to_string();
        let child = obj
            .get("agent_id")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        Ok((session_id, child))
    };
    match event {
        // The subagent owns a SEPARATE rollout (filename UUID == this
        // payload's `agent_id`), so the JSONL watcher already renders it —
        // but orphaned (a flat rollout path has no `/subagents/` for
        // `detect_parent_id`). Key the CHILD on `agent_id` (coalescing with
        // that rollout) and link it to the parent `session_id`, joining the
        // same scope tree (cascade / liveness / readiness) as a CC subagent.
        "SubagentStart" => {
            let (session_id, child) = guards(obj)?;
            let child = child.ok_or_else(|| anyhow!("SubagentStart missing/empty agent_id"))?;
            let cwd = obj.get("cwd").and_then(|s| s.as_str()).unwrap_or("").into();
            Ok(Some(vec![AgentEvent::SessionStart {
                agent_id: AgentId::from_parts(SOURCE_NAME, &child),
                source: SOURCE_NAME.to_string(),
                session_id: child,
                cwd,
                parent_id: Some(AgentId::from_parts(SOURCE_NAME, &session_id)),
            }]))
        }
        // End the CHILD promptly (else its rollout lingers to the 30-min
        // stale-sweep). Best-effort: losing the race against the child's slot
        // creation leaves a harmless no-op + the stale-sweep fallback.
        "SubagentStop" => {
            let (_session_id, child) = guards(obj)?;
            let child = child.ok_or_else(|| anyhow!("SubagentStop missing/empty agent_id"))?;
            Ok(Some(vec![AgentEvent::SessionEnd {
                agent_id: AgentId::from_parts(SOURCE_NAME, &child),
                as_child: true,
            }]))
        }
        _ => Ok(None),
    }
}

/// First-sight cwd extractor (the walker's head scan, dispatched via the
/// registry row — invariant #3): Codex rollouts carry the cwd ONLY on the head
/// `session_meta` line, nested under `payload`.
pub(crate) fn extract_codex_cwd(v: &Value) -> Option<PathBuf> {
    v.get("payload")?.get("cwd")?.as_str().map(PathBuf::from)
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

    let start = || AgentEvent::ActivityStart {
        agent_id,
        tool_use_id: None,
        detail: None,
    };
    let end = || AgentEvent::ActivityEnd {
        agent_id,
        tool_use_id: None,
    };

    let out = match (outer, inner) {
        // `task_started`/`task_complete` are what codex serializes TODAY; the v2
        // `turn_started`/`turn_complete` are upstream's OWN serde aliases
        // (`#[serde(rename="task_started", alias="turn_started")]` in codex-rs
        // protocol.rs) — accepted here too so a future serializer flip to the
        // alias form still drives Active/Idle. Mirrors upstream by construction.
        ("event_msg", "task_started") | ("event_msg", "turn_started") => vec![start()],
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
            vec![start()]
        }
        // Web/tool search are turn-INTERNAL work pulses — the agent is actively
        // searching, not idle — so they keep it Active (→ ActivityStart), the
        // same as every other intra-turn step above; only task_complete /
        // turn_aborted end the turn. `web_search_{begin,end}` are EventMsg
        // lifecycle events (codex-rs `protocol.rs` `EventMsg::WebSearch{Begin,
        // End}`); `web_search_call` + `tool_search_{call,output}` are raw OpenAI
        // Responses items (response_item) — both forms appear in real rollouts
        // (verified, codex-cli 0.137). No approval gate: searching is never
        // permission-prompted, so unlike function_call there's no Waiting branch.
        ("response_item", "web_search_call")
        | ("event_msg", "web_search_begin")
        | ("event_msg", "web_search_end")
        | ("response_item", "tool_search_call")
        | ("response_item", "tool_search_output") => vec![start()],
        ("event_msg", "task_complete")
        | ("event_msg", "turn_complete")
        | ("event_msg", "turn_aborted") => vec![end()],
        // Token-meter usage observation (#632): `token_count` fires per turn
        // with `info.last_token_usage` (that turn's reading — the cumulative
        // twin `total_token_usage` is deliberately NOT read: the reducer
        // accumulates deltas, and summing a running total would double-count).
        // FRESH spend only: codex's `input_tokens` INCLUDES the cached share
        // (`cached_input_tokens` ⊂ input, live-rollout-verified: total =
        // input + output), so fresh input = input − cached (saturating —
        // upstream reporting quirks must not wrap). `reasoning_output_tokens`
        // is additive alongside `output_tokens` in codex's accounting.
        // A `token_count` without `info` (rate-limit-only pings) emits
        // nothing, as does a zero reading.
        ("event_msg", "token_count") => {
            let last = payload
                .and_then(|p| p.get("info"))
                .and_then(|i| i.get("last_token_usage"))
                .and_then(|u| u.as_object());
            let fresh = last.map_or(0, |u| {
                let field = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
                field("input_tokens")
                    .saturating_sub(field("cached_input_tokens"))
                    .saturating_add(field("output_tokens"))
                    .saturating_add(field("reasoning_output_tokens"))
            });
            if fresh > 0 {
                vec![AgentEvent::Usage {
                    agent_id,
                    fresh_tokens: fresh,
                }]
            } else {
                vec![]
            }
        }
        // Burn-tier observation: `turn_context` opens every turn carrying the
        // model + (on reasoning turns only) the effort — both RAW verbatim,
        // last-seen-wins downstream, so a mid-session model/effort switch
        // tracks. Absent effort ≠ downgrade: the reducer only refreshes on
        // Some. Source-verified vs openai/codex protocol + live rollouts.
        ("turn_context", _) => {
            let model = payload
                .and_then(|p| p.get("model"))
                .and_then(|m| m.as_str())
                .filter(|m| !m.is_empty());
            let effort = payload
                .and_then(|p| p.get("effort"))
                .and_then(|e| e.as_str())
                .filter(|e| !e.is_empty());
            if model.is_some() || effort.is_some() {
                vec![AgentEvent::ModelInfo {
                    agent_id,
                    model: model.map(|m| ellipsize(m, MAX_DECODED_FIELD_CHARS)),
                    effort: effort.map(|e| ellipsize(e, MAX_DECODED_FIELD_CHARS)),
                }]
            } else {
                vec![]
            }
        }
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
            // string doesn't is a consumed-shape drift — surfaced the same
            // structured way as the sibling missing-`name` case (a plain
            // debug! was invisible to the warn-floor log / `pixtuoid doctor`).
            crate::source::drift::shape_drift(
                SOURCE_NAME,
                &format!("function_call arguments not parseable: {e}"),
            );
            return false;
        }
    };
    args.get("sandbox_permissions").and_then(|s| s.as_str()) == Some("require_escalated")
}

fn codex_tool_start(agent_id: AgentId, payload: Option<&Map<String, Value>>) -> AgentEvent {
    let name = payload
        .and_then(|p| p.get("name"))
        .and_then(|s| s.as_str())
        .unwrap_or_else(|| {
            crate::source::drift::missing_field(SOURCE_NAME, "function_call", "name");
            "tool"
        });
    AgentEvent::ActivityStart {
        agent_id,
        tool_use_id: None,
        // Codex tool calls are function_calls, never subagent dispatches (those
        // arrive as the SubagentStart hook), so no `subagent_type` to pass.
        detail: Some(make_tool_detail(SOURCE_NAME, name, None)),
    }
}

/// The Codex home dir — honors `CODEX_HOME` when it points at an existing dir,
/// else `~/.codex` (codex's own precedence). The public entry the installer
/// routes its `config.toml` path through too, so the watched sessions root and
/// the installed-hook config can never disagree. See `crate::platform::codex_home`.
pub fn codex_home() -> PathBuf {
    crate::platform::codex_home()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- burn-tier observations (ModelInfo) ----

    #[test]
    fn turn_context_surfaces_model_and_effort_verbatim() {
        // Shape sanitized from a LIVE rollout; effort appears
        // only on reasoning turns.
        let v = serde_json::json!({
            "type": "turn_context",
            "payload": {"turn_id": "t1", "cwd": "/r", "model": "gpt-5.6-sol", "effort": "xhigh"}
        });
        let evs = decode_codex_line("/p/rollout-2026-07-10-abc.jsonl", "codex", v).unwrap();
        assert!(
            evs.iter().any(
                |e| matches!(e, AgentEvent::ModelInfo { model: Some(m), effort: Some(f), .. }
                if m == "gpt-5.6-sol" && f == "xhigh")
            ),
            "turn_context must surface model+effort, got {evs:?}"
        );
        // Effort-less turn: model only, no phantom effort.
        let v = serde_json::json!({
            "type": "turn_context",
            "payload": {"turn_id": "t2", "cwd": "/r", "model": "gpt-5.5"}
        });
        let evs = decode_codex_line("/p/rollout-2026-07-10-abc.jsonl", "codex", v).unwrap();
        assert!(
            evs.iter().any(|e| matches!(e, AgentEvent::ModelInfo { model: Some(m), effort: None, .. } if m == "gpt-5.5")),
            "got {evs:?}"
        );
        // Neither field → no event (not an empty ModelInfo).
        let v = serde_json::json!({"type": "turn_context", "payload": {"turn_id": "t3"}});
        assert!(
            decode_codex_line("/p/rollout-2026-07-10-abc.jsonl", "codex", v)
                .unwrap()
                .is_empty()
        );
    }

    /// `is_uuid` is the rollout-stem shape gate: 36 chars, dashes at the
    /// canonical positions, hex everywhere else — each axis pinned by a
    /// counterexample so an always-true / OR-collapsed mutant can't admit
    /// non-UUID stems into the id space.
    #[test]
    fn is_uuid_requires_length_dashes_and_hex() {
        assert!(is_uuid("0196fdb2-99d1-7db2-9ded-93a4a0d4a90e"));
        for bad in [
            "",
            "abc",                                   // wrong length
            "0196fdb299d17db29ded93a4a0d4a90e",      // 32 hex, no dashes
            "0196fdb2-99d1-7db2-9ded-93a4a0d4a90ez", // 37 chars
            "0196fdb2x99d1-7db2-9ded-93a4a0d4a90e",  // dash position wrong
            "0196fdb2-99d1-7db2-9ded-93a4a0d4a90g",  // non-hex digit
        ] {
            assert!(!is_uuid(bad), "{bad:?} must not read as a UUID");
        }
    }

    // The custom-decoder contract: claim our two events FULLY — a malformed
    // instance must be Err, never Ok(None) (which would silently fall through
    // to the shared session-keyed arms). These pin the guards directly; the
    // happy paths are pinned end-to-end in tests/sources/decode/mod.rs.
    #[test]
    fn subagent_hooks_with_empty_ids_are_err_not_fallthrough() {
        for event in ["SubagentStart", "SubagentStop"] {
            let no_session = json!({"hook_event_name": event, "agent_id": "child"});
            assert!(
                decode_codex_hook_custom(&no_session).is_err(),
                "{event} without session_id must Err (claim-fully), not fall through"
            );
            let empty_child = json!({"hook_event_name": event, "session_id": "s", "agent_id": ""});
            assert!(
                decode_codex_hook_custom(&empty_child).is_err(),
                "{event} with empty agent_id must Err — a phantom child never coalesces"
            );
        }
    }

    #[test]
    fn non_subagent_events_fall_through_to_shared_arms() {
        let stop = json!({"hook_event_name": "Stop", "session_id": "s"});
        assert!(matches!(decode_codex_hook_custom(&stop), Ok(None)));
        // Non-object payload: defensive fall-through — the dispatcher
        // pre-validates object-ness, so the shared path owns the error.
        assert!(matches!(decode_codex_hook_custom(&json!("nope")), Ok(None)));
    }

    #[test]
    fn session_end_hook_decodes_to_a_clean_session_end() {
        // #710: upstream's SessionEnd hook (stdin: session_id / cwd /
        // transcript_path / reason — schema.rs SessionEndCommandInput) rides
        // the SHARED SessionEnd arm via the fall-through above, keyed on the
        // same session_id the rollout filename carries — the immediate clean
        // exit the FD-probe ladder previously had to time out toward.
        let payload = json!({
            "hook_event_name": "SessionEnd",
            "session_id": "019e7762-9ded-7e33-be41-946ecf105bf4",
            "cwd": "/repo",
            "reason": "other",
            "_pixtuoid_source": SOURCE_NAME,
        });
        assert!(matches!(decode_codex_hook_custom(&payload), Ok(None)));
        let events = crate::source::decoder::decode_hook_payload(payload).unwrap();
        let expected = AgentId::from_parts(SOURCE_NAME, "019e7762-9ded-7e33-be41-946ecf105bf4");
        assert!(
            matches!(
                events.as_slice(),
                [AgentEvent::SessionEnd { agent_id, as_child: false }] if *agent_id == expected
            ),
            "SessionEnd must decode to exactly one clean SessionEnd for the \
             rollout-coalesced id, got {events:?}"
        );
    }

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
        // task_started (serialized today) + turn_started (upstream's v2 serde alias).
        for t in ["task_started", "turn_started"] {
            let out = ev(json!({"type":"event_msg","payload":{"type":t,"turn_id":"t"}}));
            assert!(
                matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]),
                "{t}"
            );
        }
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

    // Web/tool search are turn-internal work — the agent must read as Active,
    // not idle, while it searches. Payload shapes are the real ones captured
    // from local codex-cli 0.137 rollouts (web_search_call is a response_item;
    // web_search_end is an event_msg; tool_search_call/output are response_items).
    #[test]
    fn web_and_tool_search_keep_the_agent_active() {
        for line in [
            json!({"type":"response_item","payload":{"type":"web_search_call","status":"completed","action":{}}}),
            json!({"type":"event_msg","payload":{"type":"web_search_begin","call_id":"c"}}),
            json!({"type":"event_msg","payload":{"type":"web_search_end","call_id":"c","query":"q","action":{}}}),
            json!({"type":"response_item","payload":{"type":"tool_search_call","call_id":"c","status":"in_progress","arguments":"{}"}}),
            json!({"type":"response_item","payload":{"type":"tool_search_output","call_id":"c","status":"completed","tools":[]}}),
        ] {
            let out = ev(line.clone());
            assert!(
                matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]),
                "search event {line} must keep the agent Active"
            );
        }
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
        // task_complete (serialized today) + turn_complete (v2 serde alias) + turn_aborted.
        for t in ["task_complete", "turn_complete", "turn_aborted"] {
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
        // A token_count without `info` (rate-limit-only ping) stays silent.
        assert!(ev(json!({"type":"event_msg","payload":{"type":"token_count"}})).is_empty());
    }

    #[test]
    fn token_count_emits_fresh_usage_from_last_reading() {
        // Live-rollout shape (codex 0.x, 2026-06): input INCLUDES the cached
        // share, reasoning is additive. fresh = (11480−9088) + 87 + 15 = 2494.
        let out = ev(
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
            "total_token_usage":{"input_tokens":999999,"cached_input_tokens":0,"output_tokens":999999},
            "last_token_usage":{"input_tokens":11480,"cached_input_tokens":9088,
                                 "output_tokens":87,"reasoning_output_tokens":15}}}}),
        );
        assert!(
            matches!(
                out.as_slice(),
                [AgentEvent::Usage {
                    fresh_tokens: 2494,
                    ..
                }]
            ),
            "expected fresh=2494 from last_token_usage (never the totals), got {out:?}"
        );
    }

    #[test]
    fn token_count_saturates_and_skips_zero() {
        // cached > input (upstream reporting quirk) saturates to 0 input, not a wrap.
        let out = ev(
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
            "last_token_usage":{"input_tokens":10,"cached_input_tokens":50,"output_tokens":7}}}}),
        );
        assert!(
            matches!(
                out.as_slice(),
                [AgentEvent::Usage {
                    fresh_tokens: 7,
                    ..
                }]
            ),
            "got {out:?}"
        );
        // An all-zero reading emits nothing (no slot churn).
        let out = ev(
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
            "last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0}}}}),
        );
        assert!(out.is_empty(), "zero reading must be silent, got {out:?}");
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

    #[test]
    fn non_object_line_emits_nothing() {
        // A bare string / number / array transcript line is not an object →
        // decode early-returns empty (the `v.as_object()` else-guard).
        assert!(ev(json!("just a string")).is_empty());
        assert!(ev(json!(42)).is_empty());
        assert!(ev(json!([1, 2, 3])).is_empty());
    }

    #[test]
    fn function_call_without_arguments_starts_work_not_waiting() {
        // No `arguments` field → `function_call_needs_approval` hits its
        // None-arm (false) → falls to codex_tool_start → ActivityStart, never
        // Waiting (the absence of escalation args is not a permission gate).
        let out = ev(json!({
            "type": "response_item",
            "payload": { "type": "function_call", "name": "x" }
        }));
        assert!(
            matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]),
            "{out:?}"
        );
    }

    #[test]
    fn function_call_without_name_falls_back_to_tool_label() {
        // A `function_call` with non-escalated `arguments` but NO `name` key
        // exercises codex_tool_start's unwrap_or_else fallback (lines 212-214):
        // it substitutes the literal "tool", which make_tool_detail turns into
        // ToolDetail::Generic { display: "tool" }. Every other function_call
        // test/fixture supplies a name, so this is the only coverage of the
        // missing-`name` branch. The `arguments` keep needs_approval false (no
        // escalation) so routing reaches codex_tool_start, not the Waiting arm.
        use crate::source::ToolDetail;
        let out = ev(json!({
            "type": "response_item",
            "payload": { "type": "function_call", "arguments": r#"{"cmd":"ls"}"# }
        }));
        match out.as_slice() {
            [AgentEvent::ActivityStart {
                detail: Some(ToolDetail::Generic { display }),
                ..
            }] => assert_eq!(display, "tool"),
            other => panic!("expected one Generic-detail ActivityStart, got {other:?}"),
        }
    }

    // The `codex_session_ended` + liveness-probe tests live with the runtime
    // half in `native.rs`.
}
