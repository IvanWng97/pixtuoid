use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use crate::source::decoder::{
    cwd_basename_label, ellipsize, make_tool_detail, parsed_tail_lines, TailActivity,
    MAX_DECODED_FIELD_CHARS,
};

use crate::source::AgentEvent;
use crate::AgentId;

#[cfg(feature = "native")]
mod native;
#[cfg(feature = "native")]
pub use native::{cc_watcher, live_cc_session_ids, ClaudeCodeSource};

/// homebrew-core contract: their formula's `test do` asserts this exact id, so
/// renaming it breaks Homebrew's CI on the next autobump. Coordinate a core PR.
pub const SOURCE_NAME: &str = "claude-code";

/// The label the attachment decoder synthesizes for the `ultra_effort_exit`
/// marker — deliberately NOT a real effort word, so last-seen-wins kills the
/// flame instantly, and suppressed by `pixtuoid_scene::burn::fresh_effort` so
/// the sentinel never reaches the dossier. In-workspace decoder↔painter
/// contract token, not a stable API.
#[doc(hidden)]
pub const ULTRA_EXIT_LABEL: &str = "ultra_exit";

/// CC's session/agent id = the transcript filename stem, which is
/// cwd-independent (the cwd-derived project-dir is the *parent* dir, not the
/// stem): `<uuid>.jsonl` → `<uuid>` for a root, `agent-<id>.jsonl` →
/// `agent-<id>` for a subagent. CC session UUIDs and agent-ids are lowercase,
/// so the Windows path fold is inert here.
pub fn cc_id_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

/// CC's source-specific hook arms — `SubagentStart`/`SubagentStop` — which
/// change the event's SUBJECT to the child's AgentId, something the shared
/// session-keyed arms cannot express; every other CC hook event falls through
/// (`Ok(None)`) to those shared arms.
///
/// Needed even though CC subagents also register via JSONL: a Workflow-tool
/// fleet spawns subagents with NO per-agent `Agent` tool_use in the parent
/// transcript and no end marker in theirs, so without `SubagentStop` finished
/// fleet agents hold desks until the stale sweeps batch-reap them.
///
/// Wire facts: the payload's `agent_id` is BARE hex (no `agent-` prefix) while
/// the transcript filename stem — the JSONL watcher's id space
/// (`cc_id_from_path`) — is `agent-<id>`; `SubagentStop` additionally carries
/// `agent_transcript_path`, which is the authoritative key.
pub(crate) fn decode_cc_hook_custom(v: &Value) -> Result<Option<Vec<AgentEvent>>> {
    use anyhow::anyhow;
    let Some(obj) = v.as_object() else {
        return Ok(None); // shared path reports the malformed payload
    };
    let event = obj
        .get("hook_event_name")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    if event != "SubagentStart" && event != "SubagentStop" {
        return Ok(None);
    }
    // Registry contract: claim our two events FULLY (Err on malformed), never
    // fall through. An empty `session_id` or `agent_id` would mint a phantom
    // that never coalesces with the real subagent transcript.
    let session_id = obj
        .get("session_id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("{event} missing/empty session_id"))?;
    let wire_agent_id = obj
        .get("agent_id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("{event} missing/empty agent_id"))?;
    // Tolerate an already-prefixed wire id (the CC docs' example shows one even
    // though the live wire sends bare) without double-prefixing.
    let prefixed = if wire_agent_id.starts_with("agent-") {
        wire_agent_id.to_string()
    } else {
        format!("agent-{wire_agent_id}")
    };
    if event == "SubagentStart" {
        // The watcher's later SessionStart for the same transcript coalesces
        // (a duplicate SessionStart is an enrichment no-op in the reducer).
        let cwd = obj.get("cwd").and_then(|s| s.as_str()).unwrap_or("").into();
        Ok(Some(vec![AgentEvent::SessionStart {
            agent_id: AgentId::from_parts(SOURCE_NAME, &prefixed),
            source: SOURCE_NAME.to_string(),
            session_id: prefixed,
            cwd,
            parent_id: Some(AgentId::from_parts(SOURCE_NAME, session_id)),
        }]))
    } else {
        // The authoritative key is the subagent transcript's filename stem
        // (`cc_id_from_path` on `agent_transcript_path` — exact parity with
        // the watcher's deriver, immune to a prefix-scheme drift); the
        // prefixed wire id serves only when the path is absent.
        let path_key = obj
            .get("agent_transcript_path")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .map(|p| cc_id_from_path(Path::new(&crate::id::normalize_path_key(p))))
            .filter(|s| !s.is_empty());
        if let Some(ref k) = path_key {
            if *k != prefixed {
                // Upstream changed the filename scheme or the prefix. The stem
                // keeps THIS exit keyed to the watcher, but hook-FIRST
                // registrations (Start keys on the prefixed form) become
                // sweep-cleared phantoms — actionable, hence a warn rather
                // than a per-dispatch breadcrumb.
                crate::source::drift::shape_drift(
                    SOURCE_NAME,
                    &format!(
                        "SubagentStop transcript stem `{k}` != prefixed agent_id \
                         `{prefixed}`; keying on the stem"
                    ),
                );
            }
        }
        Ok(Some(vec![AgentEvent::SessionEnd {
            agent_id: AgentId::from_parts(SOURCE_NAME, &path_key.unwrap_or(prefixed)),
            as_child: true,
        }]))
    }
}

/// Resolve `CLAUDE_CONFIG_DIR`; an empty OR whitespace-only value is treated
/// as unset (a `"  "` value otherwise resolved hooks/watch to a relative
/// `"  /…"`). Internal cross-crate helper, not a stable API.
#[doc(hidden)]
pub fn claude_config_dir() -> Option<PathBuf> {
    crate::platform::nonempty(std::env::var("CLAUDE_CONFIG_DIR").ok()).map(PathBuf::from)
}

/// The COMPLETE set of CC transcript top-level `type` discriminators, swept
/// empirically from a live `~/.claude/projects` corpus. CC is a CLOSED binary
/// with no fetchable transcript schema, so this axis gets no CI drift-watch and
/// the runtime breadcrumb in `decode_cc_line` is the SOLE defense — an
/// unlisted type breadcrumbs until it is named here. Latest format only (legacy
/// `summary`/`progress` are intentionally absent — a replayed legacy line
/// breadcrumbs at session-frequency, not a flood).
const KNOWN_TYPES: &[&str] = &[
    "agent-name",
    "ai-title",
    "assistant",
    "attachment",
    "bridge-session",
    "custom-title",
    "file-history-delta",
    "file-history-snapshot",
    "frame-link",
    "last-prompt",
    "mode",
    "permission-mode",
    "pr-link",
    "queue-operation",
    "relocated",
    "system",
    "user",
    "worktree-state",
];

/// Decode one CC JSONL transcript line into 0..N AgentEvents.
pub fn decode_cc_line(transcript_path: &str, source: &str, v: Value) -> Result<Vec<AgentEvent>> {
    // Key on the filename stem, NOT the raw path — matches the hook decoder's
    // `IdKey::SessionId` and the watcher's deriver, so all CC keying sites
    // coalesce onto one sprite.
    let agent_id = AgentId::from_parts(source, &cc_id_from_path(Path::new(transcript_path)));
    let Some(obj) = v.as_object() else {
        return Ok(vec![]);
    };

    let mut out = Vec::new();
    let ty = obj.get("type").and_then(|s| s.as_str()).unwrap_or("");

    // Guarded on the LOW-cardinality top-level `type` (session-frequency),
    // never the HIGH-cardinality inner `attachment.type`, which would flood.
    if !ty.is_empty() && !KNOWN_TYPES.contains(&ty) {
        crate::source::drift::unknown_event(source, ty);
    }

    // An empty `attributionAgent` would emit `Rename { label: "" }`, blanking a
    // good hook-derived label with no recovery until the next Rename.
    if let Some(name) = obj
        .get("attributionAgent")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        // A trailing colon ("ns:") splits to an EMPTY segment, which the
        // pre-split guard above cannot catch.
        let seg = name.rsplit_once(':').map_or(name, |(_, tail)| tail);
        if !seg.is_empty() {
            // Capped at decode: transcript content that persists in slot state.
            let label = ellipsize(seg, MAX_DECODED_FIELD_CHARS);
            out.push(AgentEvent::Rename { agent_id, label });
        }
    }

    // CC stamps a periodic reminder attachment while ultra-class effort is
    // active, plus an EXIT marker on leaving it. The wire carries no effort
    // VALUE, so the arm synthesizes each marker's own label; the `/effort`
    // picker's chosen level is not derivable at all (empty command args).
    if ty == "attachment" {
        if let Some(kind) = obj
            .get("attachment")
            .and_then(|a| a.get("type"))
            .and_then(|t| t.as_str())
        {
            let effort = match kind {
                "ultra_effort_enter" => Some("ultra"),
                "ultrathink_effort" => Some("ultrathink"),
                // A label NOT in the scene's MAX_EFFORTS set, so last-seen-wins
                // drops the boost immediately instead of waiting out the TTL.
                "ultra_effort_exit" => Some(ULTRA_EXIT_LABEL),
                _ => None,
            };
            if let Some(effort) = effort {
                out.push(AgentEvent::ModelInfo {
                    agent_id,
                    model: None,
                    effort: Some(effort.to_string()),
                });
            }
        }
    }

    let Some(message) = obj.get("message").and_then(|m| m.as_object()) else {
        return Ok(out);
    };
    // Every assistant line carries the model that produced that turn.
    // `<synthetic>` is CC's marker for tool-generated/error turns, not a model.
    if ty == "assistant" {
        if let Some(model) = message
            .get("model")
            .and_then(|m| m.as_str())
            .filter(|m| !m.is_empty() && *m != "<synthetic>")
        {
            out.push(AgentEvent::ModelInfo {
                agent_id,
                model: Some(ellipsize(model, MAX_DECODED_FIELD_CHARS)),
                effort: None,
            });
        }
        // FRESH spend only — new input + cache WRITES + output;
        // `cache_read_input_tokens` is re-served context, not new spend, and
        // dwarfs the rest. Sidechain lines carry usage too and key to the same
        // session id, so the meter is the SESSION's burn, subagents included.
        if let Some(usage) = message.get("usage").and_then(|u| u.as_object()) {
            let field = |k: &str| usage.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
            let fresh = field("input_tokens")
                .saturating_add(field("cache_creation_input_tokens"))
                .saturating_add(field("output_tokens"));
            if fresh > 0 {
                out.push(AgentEvent::Usage {
                    agent_id,
                    fresh_tokens: fresh,
                });
            }
        }
    }
    let content = message.get("content");
    match (ty, content) {
        ("assistant", Some(Value::Array(blocks))) => {
            for block in blocks {
                let Some(bobj) = block.as_object() else {
                    continue;
                };
                let btype = bobj.get("type").and_then(|s| s.as_str()).unwrap_or("");
                if btype != "tool_use" {
                    continue;
                }
                let id = bobj.get("id").and_then(|s| s.as_str()).map(String::from);
                let name = bobj
                    .get("name")
                    .and_then(|s| s.as_str())
                    .unwrap_or_else(|| {
                        crate::source::drift::missing_field(SOURCE_NAME, "tool_use", "name");
                        "?"
                    });
                out.push(AgentEvent::ActivityStart {
                    agent_id,
                    tool_use_id: id,
                    detail: Some(make_tool_detail(SOURCE_NAME, name, bobj.get("input"))),
                });
            }
        }
        ("user", Some(Value::Array(blocks))) => {
            for block in blocks {
                let Some(bobj) = block.as_object() else {
                    continue;
                };
                let btype = bobj.get("type").and_then(|s| s.as_str()).unwrap_or("");
                if btype != "tool_result" {
                    continue;
                }
                let id = bobj
                    .get("tool_use_id")
                    .and_then(|s| s.as_str())
                    .map(String::from);
                out.push(AgentEvent::ActivityEnd {
                    agent_id,
                    tool_use_id: id,
                });
            }
        }
        // Deliberately no content arm: user-message content is
        // user-controllable and must never drive session lifecycle (a message
        // QUOTING the slash-command wrapper would false-positive). Lifecycle is
        // the SessionEnd hook + the idle sweep.
        _ => {}
    }
    Ok(out)
}

/// The [`KNOWN_TYPES`] subset whose lines are AGENT TURN activity. Everything
/// else in that list is a session-metadata SIDECAR line, which CC appends to a
/// transcript for reasons unrelated to the owning process being alive: a
/// STARTING session writes a metadata run (`bridge-session`, `pr-link`, …) into
/// an OTHER, long-dead session's transcript, bumping an mtime the first-sight
/// gate trusted as its liveness proxy.
///
/// Membership is by TURN-AUTHORSHIP, not by whether we decode the type:
/// `system` carries no `AgentEvent` but IS the session speaking (it closes live
/// tails), while `pr-link` carries a timestamp and is NOT.
const ACTIVITY_TYPES: &[&str] = &[
    "assistant",
    "attachment",
    "file-history-delta",
    "queue-operation",
    "system",
    "user",
];

/// The workflow/skills orchestrator writes a `journal.jsonl` sidecar under
/// `<uuid>/subagents/workflows/wf_*/` — a FOREIGN schema (top-level
/// `type:"started"`/`"result"`, not CC transcript lines) living in the SAME
/// projects tree the CC watcher walks, whose every line would fire the
/// undeduplicated unknown-`type` breadcrumb into the warn-floor.
///
/// A denylist, not an allowlist: the real subagent transcripts (including the
/// ones nested under `workflows/wf_*/`) stay admitted, because misfiltering one
/// costs a vanished sprite while a future foreign sidecar merely breadcrumbs.
pub(crate) fn admits_transcript(path: &Path) -> bool {
    path.file_name().and_then(|s| s.to_str()) != Some("journal.jsonl")
}

/// When this transcript's SESSION last wrote, read from its tail — the honest
/// TIGHTENING of the file's mtime in the first-sight recency gate. See the
/// `ACTIVITY_TYPES` docs for the write that made mtime lie. Widening the tail
/// window is NOT an alternative: these lines carry file contents, so no fixed
/// window bounds them.
///
/// An UNRECOGNIZED type degrades to [`TailActivity::Unknown`], never to sidecar
/// evidence: the day CC renames a turn type, reading it as "not a turn" would
/// gate every live session at once. The `unknown_event` breadcrumb in
/// `decode_cc_line` is what drives the fix instead.
///
/// Accepted residual: an unvouched live session whose tail is momentarily all
/// sidecar stays invisible until its next turn. Unvouched covers headless
/// `claude -p`, a non-standard projects root, and EVERY CC session on Windows,
/// where `live_cc_session_ids` returns `None` by construction; a session with
/// working hooks registers through the hook plane regardless.
pub fn cc_activity_recency(tail: &[u8]) -> TailActivity {
    let mut newest: Option<u64> = None;
    let mut saw_turn = false;
    let mut saw_classified = false;
    let mut saw_unclassifiable = false;
    for v in parsed_tail_lines(tail) {
        // A typeless line, or one whose type we have never seen, classifies as
        // NOTHING — it is not evidence the recent bytes were metadata.
        let Some(ty) = v.get("type").and_then(|t| t.as_str()) else {
            saw_unclassifiable = true;
            continue;
        };
        if !KNOWN_TYPES.contains(&ty) {
            saw_unclassifiable = true;
            continue;
        }
        saw_classified = true;
        if !ACTIVITY_TYPES.contains(&ty) {
            continue;
        }
        saw_turn = true;
        if let Some(secs) = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(crate::source::decoder::rfc3339_to_epoch_secs)
        {
            newest = Some(newest.map_or(secs, |n: u64| n.max(secs)));
        }
    }
    match newest {
        Some(secs) => TailActivity::At(secs),
        // A stampless turn means the session DID write here and we merely cannot
        // date it — the opposite of SidecarOnly, so it must not gate.
        None if saw_classified && !saw_turn && !saw_unclassifiable => TailActivity::SidecarOnly,
        None => TailActivity::Unknown,
    }
}

/// CC session-end checker: structural lifecycle markers only, never a byte
/// scan of message content.
pub fn cc_session_ended(tail: &[u8]) -> bool {
    // Last-marker-wins: a later `session_start` resets a `session_end` earlier
    // in the window, so this can't reduce to a simple `.any(predicate)`.
    let mut last_is_end = false;
    for v in parsed_tail_lines(tail) {
        let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
        let hook = v
            .get("hook_event_name")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        if subtype == "session_start" {
            last_is_end = false;
        }
        if subtype == "session_end" || hook == "SessionEnd" {
            last_is_end = true;
        }
    }
    last_is_end
}

/// CC label: subagent paths → "subagent", otherwise "cc·" + cwd basename.
///
/// When `cwd` is unknown (a seed line carrying no `cwd` can still fire the
/// JSONL Rename), fall back to the CC **project dir** instead of a bare "cc":
/// its name encodes the cwd path with '/'→'-', so the last segment is the
/// project basename. Without this an empty-cwd Rename silently degrades a good
/// hook-derived `cc·dotfiles` back to `cc`.
pub fn cc_derive_label(path: &Path, source: &str, cwd: &Path) -> String {
    // Slash-bounded, and shared with `detect_parent_id`: a loose `"subagents"`
    // substring mislabels a `subagents-paper` repo's parent transcript.
    if crate::source::decoder::is_subagent_path(path) {
        return "subagent".to_string();
    }
    // The `cc` prefix is a registry fact, not a literal (invariant #3).
    let prefix = crate::source::decoder::label_prefix_for(source);
    if let Some(label) = cwd_basename_label(prefix, cwd) {
        return label;
    }
    if let Some(base) = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .and_then(|proj| proj.rsplit('-').find(|s| !s.is_empty()))
    {
        // Same cap as the `cwd_basename_label` branch, for the reason that
        // chokepoint exists: an uncapped `AgentSlot.label` reaches the painter
        // and the headless summary. This branch bypasses it — CC encodes the
        // whole cwd into ONE project-dir component ('/'→'-'), so a deep path
        // yields an arbitrarily long segment even though, unlike `cwd`, nothing
        // here is wire content.
        return format!(
            "{prefix}·{}",
            crate::source::decoder::ellipsize(
                base,
                crate::source::decoder::MAX_DECODED_FIELD_CHARS
            )
        );
    }
    prefix.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_activity_type_is_a_known_type() {
        for t in ACTIVITY_TYPES {
            assert!(KNOWN_TYPES.contains(t), "{t} missing from KNOWN_TYPES");
        }
    }

    #[test]
    fn a_metadata_only_tail_does_not_refresh_the_activity_clock() {
        // A session whose last real turn was days ago, then a run of sidecar
        // lines a DIFFERENT live session appended.
        let tail = concat!(
            r#"{"type":"assistant","timestamp":"2026-07-29T05:46:24.525Z"}"#,
            "\n",
            r#"{"type":"bridge-session","sessionId":"s"}"#,
            "\n",
            r#"{"type":"custom-title","sessionId":"s"}"#,
            "\n",
            r#"{"type":"pr-link","timestamp":"2026-08-02T05:56:43.894Z"}"#,
            "\n",
        );
        assert_eq!(
            cc_activity_recency(tail.as_bytes()),
            TailActivity::At(1_785_303_984),
            "the July turn is the newest ACTIVITY, whatever the sidecars stamp"
        );
    }

    #[test]
    fn a_live_tail_reports_its_newest_turn_and_a_stampless_tail_reports_nothing() {
        let live = concat!(
            r#"{"type":"user","timestamp":"2026-08-02T06:04:54.650Z"}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-08-02T06:05:26.261Z"}"#,
            "\n",
            r#"{"type":"system","timestamp":"2026-08-02T06:05:26.613Z"}"#,
            "\n",
        );
        assert_eq!(
            cc_activity_recency(live.as_bytes()),
            TailActivity::At(1_785_650_726)
        );
        assert_eq!(
            cc_activity_recency(br#"{"type":"pr-link","timestamp":"2026-08-02T05:56:43.894Z"}"#),
            TailActivity::SidecarOnly
        );
        for blind in [
            &b""[..],
            br#"not json at all"#,
            br#"{"type":"assistant"}"#,
            br#"{"type":"assistant","timestamp":"not-a-date"}"#,
        ] {
            assert_eq!(
                cc_activity_recency(blind),
                TailActivity::Unknown,
                "{blind:?}"
            );
        }
    }

    #[test]
    fn assistant_line_model_becomes_a_model_info_observation() {
        let v = json!({
            "type": "assistant",
            "message": {"model": "claude-fable-5", "content": []}
        });
        let evs = decode_cc_line("/p/ses-1.jsonl", "claude-code", v).unwrap();
        assert!(
            evs.iter().any(|e| matches!(e, AgentEvent::ModelInfo { model: Some(m), effort: None, .. } if m == "claude-fable-5")),
            "assistant model must surface, got {evs:?}"
        );
    }

    #[test]
    fn assistant_usage_becomes_a_fresh_token_observation() {
        let v = json!({
            "type": "assistant",
            "message": {"model": "claude-fable-5", "content": [], "usage": {
                "input_tokens": 1200, "cache_creation_input_tokens": 50000,
                "cache_read_input_tokens": 940000, "output_tokens": 5300}}
        });
        let evs = decode_cc_line("/p/ses-1.jsonl", "claude-code", v).unwrap();
        assert!(
            evs.iter().any(|e| matches!(
                e,
                AgentEvent::Usage {
                    fresh_tokens: 56500,
                    ..
                }
            )),
            "expected fresh=1200+50000+5300 (cache reads excluded), got {evs:?}"
        );
    }

    #[test]
    fn usage_free_and_zero_usage_assistant_lines_stay_silent() {
        let v = json!({"type": "assistant", "message": {"content": []}});
        let evs = decode_cc_line("/p/ses-1.jsonl", "claude-code", v).unwrap();
        assert!(!evs.iter().any(|e| matches!(e, AgentEvent::Usage { .. })));
        let v = json!({
            "type": "assistant",
            "message": {"content": [], "usage": {
                "input_tokens": 0, "cache_read_input_tokens": 123456, "output_tokens": 0}}
        });
        let evs = decode_cc_line("/p/ses-1.jsonl", "claude-code", v).unwrap();
        assert!(
            !evs.iter().any(|e| matches!(e, AgentEvent::Usage { .. })),
            "cache-read-only reading must be silent, got {evs:?}"
        );
    }

    #[test]
    fn synthetic_and_empty_models_are_not_observations() {
        for model in ["<synthetic>", ""] {
            let v = json!({
                "type": "assistant",
                "message": {"model": model, "content": []}
            });
            let evs = decode_cc_line("/p/ses-1.jsonl", "claude-code", v).unwrap();
            assert!(
                !evs.iter()
                    .any(|e| matches!(e, AgentEvent::ModelInfo { .. })),
                "{model:?} must not emit ModelInfo, got {evs:?}"
            );
        }
    }

    #[test]
    fn ultra_effort_attachments_become_effort_observations() {
        for (kind, label) in [
            ("ultra_effort_enter", "ultra"),
            ("ultrathink_effort", "ultrathink"),
            ("ultra_effort_exit", "ultra_exit"),
        ] {
            let v = json!({
                "type": "attachment",
                "attachment": {"type": kind}
            });
            let evs = decode_cc_line("/p/ses-1.jsonl", "claude-code", v).unwrap();
            assert!(
                evs.iter().any(|e| matches!(e, AgentEvent::ModelInfo { model: None, effort: Some(f), .. } if f == label)),
                "{kind} must synthesize effort {label:?}, got {evs:?}"
            );
        }
        let v = json!({"type": "attachment", "attachment": {"type": "task_reminder"}});
        let evs = decode_cc_line("/p/ses-1.jsonl", "claude-code", v).unwrap();
        assert!(
            evs.is_empty(),
            "unrelated attachments are inert, got {evs:?}"
        );
    }

    #[test]
    fn subagent_hooks_with_empty_ids_are_err_not_fallthrough() {
        for event in ["SubagentStart", "SubagentStop"] {
            let no_session = json!({"hook_event_name": event, "agent_id": "abc"});
            assert!(
                decode_cc_hook_custom(&no_session).is_err(),
                "{event} without session_id must Err (claim-fully), not fall through"
            );
            let empty_child = json!({"hook_event_name": event, "session_id": "s", "agent_id": ""});
            assert!(
                decode_cc_hook_custom(&empty_child).is_err(),
                "{event} with empty agent_id must Err — a phantom child never coalesces"
            );
        }
    }

    #[test]
    fn subagent_stop_warns_only_when_stem_and_wire_id_disagree() {
        let capture = |payload: serde_json::Value| {
            crate::test_capture::capture_logs(|| {
                decode_cc_hook_custom(&payload)
                    .expect("decodes")
                    .expect("claimed");
            })
        };
        let matched = capture(json!({
            "hook_event_name": "SubagentStop",
            "session_id": "s",
            "agent_id": "abc123",
            "agent_transcript_path": "/p/parent/subagents/agent-abc123.jsonl"
        }));
        assert!(
            !matched.contains("shape_drift"),
            "an agreeing stem must stay silent, got:\n{matched}"
        );
        let drifted = capture(json!({
            "hook_event_name": "SubagentStop",
            "session_id": "s",
            "agent_id": "abc123",
            "agent_transcript_path": "/p/parent/subagents/agent-zzz999.jsonl"
        }));
        assert!(
            drifted.contains("shape_drift") && drifted.contains("agent-zzz999"),
            "a disagreeing stem must fire the drift alarm, got:\n{drifted}"
        );
    }

    #[test]
    fn non_subagent_events_fall_through_to_shared_arms() {
        let start = json!({"hook_event_name": "SessionStart", "session_id": "s"});
        assert!(matches!(decode_cc_hook_custom(&start), Ok(None)));
        assert!(matches!(decode_cc_hook_custom(&json!("nope")), Ok(None)));
    }

    #[test]
    fn subagent_stop_keys_on_stem_when_it_disagrees_with_prefixed_wire_id() {
        let evs = decode_cc_hook_custom(&json!({
            "hook_event_name": "SubagentStop",
            "session_id": "s",
            "agent_id": "abc",
            "agent_transcript_path": "/p/q/01-deadbeef/subagents/agent-zzz.jsonl"
        }))
        .unwrap()
        .unwrap();
        assert_eq!(evs.len(), 1, "SubagentStop emits exactly one event");
        match &evs[0] {
            AgentEvent::SessionEnd { agent_id, as_child } => {
                assert!(*as_child, "a SubagentStop end is stamped as_child");
                assert_eq!(
                    *agent_id,
                    AgentId::from_parts(SOURCE_NAME, "agent-zzz"),
                    "the transcript STEM agent-zzz must win over the prefixed wire id agent-abc"
                );
                assert_ne!(
                    *agent_id,
                    AgentId::from_parts(SOURCE_NAME, "agent-abc"),
                    "the prefixed wire id must NOT be the key when the stem differs"
                );
            }
            other => panic!("expected SessionEnd, got {other:?}"),
        }
    }

    #[test]
    fn subagent_stop_with_stemless_path_falls_back_to_prefixed_id() {
        let evs = decode_cc_hook_custom(&json!({
            "hook_event_name": "SubagentStop",
            "session_id": "s",
            "agent_id": "abc",
            "agent_transcript_path": "/"
        }))
        .unwrap()
        .unwrap();
        assert_eq!(
            evs[0].agent_id(),
            crate::AgentId::from_parts(SOURCE_NAME, "agent-abc")
        );
    }

    #[test]
    fn label_prefers_cwd_basename_when_present() {
        let path = Path::new("/x/.claude/projects/-Users-me-repo/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("/Users/me/work/myrepo")),
            "cc·myrepo"
        );
    }

    #[test]
    fn label_falls_back_to_project_dir_when_cwd_empty() {
        let path = Path::new("/Users/me/.claude/projects/-Users-me-dotfiles/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("")),
            "cc·dotfiles"
        );
    }

    /// The project-dir fallback BYPASSES the `cwd_basename_label` chokepoint,
    /// so it needs the cap applied at its own site — an uncapped label persists
    /// in `AgentSlot.label` and reaches the painter.
    #[test]
    fn label_project_dir_fallback_is_capped_like_the_cwd_branch() {
        let long = "z".repeat(MAX_DECODED_FIELD_CHARS * 2);
        let path = PathBuf::from(format!("/Users/me/.claude/projects/-{long}/abc.jsonl"));
        let label = cc_derive_label(&path, "claude-code", Path::new(""));
        assert!(
            label.chars().count() < long.chars().count(),
            "project-dir fallback label must be capped, got {} chars: {label:?}",
            label.chars().count()
        );
        // The binding assertion: exact parity with the sibling branch, which
        // caps via the `cwd_basename_label` chokepoint. Comparing to the
        // sibling rather than to a hand-computed width keeps this honest if
        // `ellipsize`'s accounting ever changes.
        let via_cwd = cc_derive_label(
            Path::new("/x/.claude/projects/-p/abc.jsonl"),
            "claude-code",
            &PathBuf::from(format!("/{long}")),
        );
        assert_eq!(label, via_cwd, "both label branches must cap identically");
    }

    #[test]
    fn label_marks_subagent_paths() {
        let path = Path::new("/x/projects/proj/subagents/agent-1.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("/repo")),
            "subagent"
        );
    }

    #[test]
    fn label_does_not_false_positive_on_subagents_in_project_name() {
        let path = Path::new("/Users/me/.claude/projects/-Users-me-subagents-paper/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("/Users/me/subagents-paper")),
            "cc·subagents-paper"
        );
    }

    #[test]
    fn label_uses_project_dir_when_cwd_is_root() {
        let path = Path::new("/Users/me/.claude/projects/-Users-me-dotfiles/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("/")),
            "cc·dotfiles"
        );
    }

    #[test]
    fn label_uses_project_dir_when_cwd_has_no_basename() {
        // ".." is non-empty and non-root but has no `file_name()`.
        let path = Path::new("/Users/me/.claude/projects/-Users-me-dotfiles/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("..")),
            "cc·dotfiles"
        );
    }

    #[test]
    fn label_final_fallback_to_cc_when_no_project_dir() {
        assert_eq!(
            cc_derive_label(Path::new("abc.jsonl"), "claude-code", Path::new("")),
            "cc"
        );
    }

    // CC on Windows slugs `C:\Users\foo\bar` into the project dir name
    // `C--Users-foo-bar` (upstream's `[^a-zA-Z0-9]→'-'`, drive letter kept).
    #[test]
    fn label_falls_back_to_project_dir_for_windows_slug() {
        let path = Path::new("/Users/me/.claude/projects/C--Users-foo-bar/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("")),
            "cc·bar"
        );
    }

    // If the per-line decode ever keys differently from the registry row's
    // `id_from_path` deriver, one CC session splits into two sprites.
    #[test]
    fn decode_cc_line_keys_agent_id_on_cc_id_from_path() {
        let path = "/Users/me/.claude/projects/p/01000000-0000-7000-8000-0000000000cc.jsonl";
        let events = decode_cc_line(
            path,
            "claude-code",
            serde_json::json!({"type":"assistant","attributionAgent":"explorer","message":{"content":[]}}),
        )
        .unwrap();
        let expected =
            AgentId::from_parts("claude-code", &cc_id_from_path(std::path::Path::new(path)));
        assert_eq!(
            events[0].agent_id(),
            expected,
            "decode_cc_line must key its AgentId on cc_id_from_path (the deriver)"
        );
    }

    #[test]
    fn quoted_exit_wrapper_in_user_content_never_ends_the_session() {
        let prose =
            "the transcript shows <command-name>/exit</command-name> as a wrapped line — why?";
        let v = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": prose }
        });
        let events = decode_cc_line("/x/.claude/projects/p/s.jsonl", "claude-code", v).unwrap();
        assert!(
            events.is_empty(),
            "quoting the wrapper must not emit SessionEnd: {events:?}"
        );

        let tail = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": prose }
        })
        .to_string();
        assert!(
            !cc_session_ended(tail.as_bytes()),
            "tail scan must not end a session on quoted wrapper text"
        );
    }

    #[test]
    fn string_content_turns_emit_no_tool_events() {
        for ty in ["assistant", "user"] {
            let v = serde_json::json!({
                "type": ty,
                "message": { "role": ty, "content": "just some prose, no tool blocks" }
            });
            let out = decode_cc_line("/x/.claude/projects/p/s.jsonl", "claude-code", v).unwrap();
            assert!(
                out.is_empty(),
                "{ty} turn with string content must emit no events"
            );
        }
        let exit = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": "<command-name>/exit</command-name>" }
        });
        let out = decode_cc_line("/x/.claude/projects/p/s.jsonl", "claude-code", exit).unwrap();
        assert!(
            out.is_empty(),
            "slash-command content must not emit lifecycle events: {out:?}"
        );
    }

    #[test]
    fn decode_cc_line_non_object_value_decodes_to_nothing() {
        let path = "/x/.claude/projects/p/s.jsonl";
        assert!(
            decode_cc_line(path, "claude-code", serde_json::json!([1, 2, 3]))
                .unwrap()
                .is_empty(),
            "a bare array line must emit no events"
        );
        assert!(
            decode_cc_line(path, "claude-code", serde_json::json!("raw string line"))
                .unwrap()
                .is_empty(),
            "a bare string line must emit no events"
        );
    }

    #[test]
    fn unknown_top_level_type_breadcrumbs_but_known_and_inner_attachment_stay_silent() {
        let path = "/x/.claude/projects/p/s.jsonl";
        let logs = crate::test_capture::capture_logs(|| {
            let out = decode_cc_line(
                path,
                "claude-code",
                json!({"type": "quantum-line", "foo": 1}),
            )
            .unwrap();
            assert!(
                out.is_empty(),
                "an unknown top-level type decodes to no events"
            );
        });
        assert!(
            logs.contains("unknown_event") && logs.contains("quantum-line"),
            "a brand-new CC top-level type must fire the drift breadcrumb, got:\n{logs}"
        );

        // The last case is a real `attachment` line whose INNER
        // attachment.type is brand-new: only the top-level `type` is guarded.
        let quiet = crate::test_capture::capture_logs(|| {
            for v in [
                json!({"type": "mode", "mode": "acceptEdits"}),
                json!({"type": "last-prompt", "prompt": "hi"}),
                json!({"type": "worktree-state"}),
                json!({"type": "frame-link", "url": "x"}),
                json!({"type": "pr-link"}),
                json!({"type": "bridge-session"}),
                json!({"type": "system", "subtype": "brand_new_subtype_2027"}),
                json!({"type": "attachment", "attachment": {"type": "brand_new_kind_2027"}}),
            ] {
                decode_cc_line(path, "claude-code", v).unwrap();
            }
        });
        assert!(
            !quiet.contains("unknown_event"),
            "known top-level types + a novel inner attachment.type must NOT breadcrumb, got:\n{quiet}"
        );
    }

    #[test]
    fn tool_use_without_name_emits_activity_start_with_question_mark_detail() {
        let out = decode_cc_line(
            "/x/.claude/projects/p/s.jsonl",
            "claude-code",
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu1"}]}}),
        )
        .unwrap();
        assert_eq!(out.len(), 1, "one tool_use block → one ActivityStart");
        match &out[0] {
            AgentEvent::ActivityStart {
                tool_use_id,
                detail,
                ..
            } => {
                assert_eq!(tool_use_id.as_deref(), Some("tu1"));
                assert_eq!(
                    detail.as_ref(),
                    Some(&make_tool_detail(SOURCE_NAME, "?", None)),
                    "a name-less tool_use substitutes the \"?\" fallback name"
                );
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn assistant_content_skips_non_object_and_non_tool_use_blocks() {
        let out = decode_cc_line(
            "/x/.claude/projects/p/s.jsonl",
            "claude-code",
            json!({"type":"assistant","message":{"content":[
                42,
                {"type":"text","text":"hi"},
                {"type":"tool_use","id":"tu","name":"Read","input":{"file_path":"/a"}}
            ]}}),
        )
        .unwrap();
        assert_eq!(
            out.len(),
            1,
            "only the real tool_use block decodes; the int + text block are skipped: {out:?}"
        );
        match &out[0] {
            AgentEvent::ActivityStart {
                tool_use_id,
                detail,
                ..
            } => {
                assert_eq!(tool_use_id.as_deref(), Some("tu"));
                assert_eq!(
                    detail.as_ref(),
                    Some(&make_tool_detail(
                        SOURCE_NAME,
                        "Read",
                        Some(&json!({"file_path":"/a"}))
                    )),
                    "the surviving block is the Read tool_use"
                );
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn user_content_skips_non_object_and_non_tool_result_blocks() {
        let out = decode_cc_line(
            "/x/.claude/projects/p/s.jsonl",
            "claude-code",
            json!({"type":"user","message":{"content":[
                "str",
                {"type":"text","text":"hi"},
                {"type":"tool_result","tool_use_id":"tu"}
            ]}}),
        )
        .unwrap();
        assert_eq!(
            out.len(),
            1,
            "only the real tool_result block decodes; the string + text block are skipped: {out:?}"
        );
        match &out[0] {
            AgentEvent::ActivityEnd { tool_use_id, .. } => {
                assert_eq!(tool_use_id.as_deref(), Some("tu"));
            }
            other => panic!("expected ActivityEnd, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod cc_id_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn cc_id_from_path_root_is_filename_uuid() {
        let p = Path::new(
            "/Users/me/.claude/projects/-Users-me-proj/01000000-0000-7000-8000-0000000000cc.jsonl",
        );
        assert_eq!(cc_id_from_path(p), "01000000-0000-7000-8000-0000000000cc");
    }

    #[test]
    fn cc_id_from_path_subagent_is_agent_stem() {
        let p = Path::new("/Users/me/.claude/projects/-Users-me-proj/01000000-0000-7000-8000-0000000000cc/subagents/agent-a0a7dc28dd772bd0d.jsonl");
        assert_eq!(cc_id_from_path(p), "agent-a0a7dc28dd772bd0d");
    }

    #[test]
    fn cc_id_from_path_empty_for_no_stem() {
        assert_eq!(cc_id_from_path(Path::new("")), "");
    }

    #[test]
    fn cc_id_from_path_is_stable_across_path_separators() {
        // The first-sight deriver gets a raw &Path while the per-line decoder
        // gets the `normalize_path_key`'d string — lowercased on Windows.
        let raw =
            Path::new("/Users/me/.claude/projects/p/01000000-0000-7000-8000-0000000000cc.jsonl");
        let normalized =
            Path::new("/users/me/.claude/projects/p/01000000-0000-7000-8000-0000000000cc.jsonl");
        assert_eq!(cc_id_from_path(raw), cc_id_from_path(normalized));
    }

    #[test]
    fn attribution_agent_label_is_capped_at_the_decode_boundary() {
        let path = "/p/x/s.jsonl";
        let long = "é".repeat(MAX_DECODED_FIELD_CHARS * 10);
        let events = decode_cc_line(
            path,
            "claude-code",
            serde_json::json!({"type":"assistant","attributionAgent": long, "message":{"content":[]}}),
        )
        .unwrap();
        match &events[0] {
            AgentEvent::Rename { label, .. } => {
                assert_eq!(label.chars().count(), MAX_DECODED_FIELD_CHARS + 1);
                assert!(label.ends_with('…'));
            }
            other => panic!("expected Rename, got {other:?}"),
        }

        let events = decode_cc_line(
            path,
            "claude-code",
            serde_json::json!({"type":"assistant","attributionAgent":"explorer","message":{"content":[]}}),
        )
        .unwrap();
        assert!(
            matches!(&events[0], AgentEvent::Rename { label, .. } if label == "explorer"),
            "a short label must pass through unchanged, got {events:?}"
        );
    }

    #[test]
    fn admits_every_transcript_except_the_orchestrator_journal() {
        let wf = Path::new("/h/.claude/projects/proj/uuid/subagents/workflows/wf_abc");
        assert!(!admits_transcript(&wf.join("journal.jsonl")));
        assert!(admits_transcript(&wf.join("agent-xyz.jsonl")));
        assert!(admits_transcript(Path::new(
            "/h/.claude/projects/proj/uuid.jsonl"
        )));
        assert!(admits_transcript(Path::new(
            "/h/.claude/projects/proj/uuid/subagents/agent-xyz.jsonl"
        )));
    }
}
