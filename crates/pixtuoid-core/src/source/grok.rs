//! Grok Build source (`grok`, xai-org/grok-build) — TRANSCRIPT-BEARING with a
//! hook install target (the CC/Codex class: both transports).
//!
//! Wire facts are repo-derived from the open-source sync of the grok monorepo
//! (v0.1.220-alpha.4 @ c68e39f6, 2026-07-16) — `verified_version` stays
//! `"unknown"` until a byte-real capture anchors it:
//!
//! - **Transcript**: `{grok_home}/sessions/<enc-cwd>/<session-id>/updates.jsonl`
//!   is append-ONLY for the session's whole life (even `/rewind` appends a
//!   `rewind_marker` instead of truncating; O_APPEND + flush per record, torn
//!   tail healed upstream). The SIBLING `chat_history.jsonl` is REWRITTEN via
//!   temp+rename on resume/compaction/rewind — never tail it (the watcher
//!   path-filters to `updates.jsonl`). Line shape:
//!   `{"timestamp":<unix-secs>,"method":"session/update"|"_x.ai/session/update",
//!     "params":{"sessionId":"…","update":{"sessionUpdate":"<tag>",…}}}`.
//! - **Hooks**: 14 lifecycle events, JSON envelope on stdin with **camelCase
//!   field names and snake_case event values** (`hookEventName`,`sessionId`,
//!   `cwd`,`workspaceRoot`,`toolName`,`toolUseId`,`toolInput`,…) — alien to the
//!   shared CC-shaped arms (`hook_event_name`), hence the claims-all custom
//!   decoder below. Only PreToolUse can block; everything else is observe-only
//!   fail-open with a 5s default timeout, dispatched SEQUENTIALLY inline on the
//!   session actor — the shim's 200ms bound matters here.
//! - **Keying**: `sessionId` — consistent across every event of a session, ==
//!   the transcript's parent-DIR name (grok-generated ids are UUIDv7), == a
//!   subagent's `subagentId` (upstream: `child_session_id = subagent_id`,
//!   handle_request.rs). Hook and watcher keys therefore coalesce by the same
//!   string, and a child's tool hooks (which carry the CHILD's `sessionId` +
//!   a `subagentType` marker) attribute to the child sprite with NO CC-style
//!   `active_tasks` suppression needed.
//! - **Subagents**: in-process child sessions persisted as FLAT siblings in the
//!   normal sessions tree (only `meta.json` nests under the parent dir). The
//!   parent linkage carriers are the `subagent_start`/`subagent_stop` hooks
//!   (parent-keyed envelope + `subagentId` payload) and the parent transcript's
//!   `subagent_spawned`/`subagent_finished` xAI lines. Children fire NO
//!   `session_start` hook of their own — the hook `subagent_start` (or the
//!   parent-transcript line) is the child's registration carrier, and the
//!   child's own flat transcript first-sight coalesces/enriches.
//! - **Exit profile**: `session_end` fires on `SessionCommand::Shutdown` and
//!   channel-closed teardown but NOT on a plain TUI quit (the event loop breaks
//!   without draining the actor — verified against run_loop/dispatch), and not
//!   on kill. The reliable exit signal is the liveness ladder over grok's own
//!   crash-recovery registry `{grok_home}/active_sessions.json`
//!   (`{session_id,pid,cwd,opened_at}`, removed on clean quit, left on crash) —
//!   see the native half. No open-FD probe is possible: every append opens and
//!   drops the file handle (unlike Codex's for-lifetime rollout fd).

use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

use crate::source::decoder::{ellipsize, generic_tool_display, MAX_DECODED_FIELD_CHARS};
use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

pub const SOURCE_NAME: &str = "grok";

/// Decode one grok hook payload (already identified by
/// `_pixtuoid_source == "grok"`). Envelope per xai-grok-hooks `event.rs`
/// (camelCase serde on `HookEventEnvelope`, snake_case `hookEventName` values).
///
/// Event mapping, all keyed on `sessionId`:
/// - `session_start`        → `SessionStart` (+ `ModelInfo` when `modelId` is
///   offered — the fire site passes None today, decode-if-present)
/// - `user_prompt_submit`   → `SessionStart` — the resurrect carrier: grok's
///   `session_end` is unreliable (TUI quit fires none) so a stale-swept LIVE
///   session must walk back in on its next prompt, the same reasoning as the
///   shared Codex `UserPromptSubmit` arm (idempotent when the slot exists)
/// - `pre_tool_use`         → `Identity` + `ActivityStart{toolUseId}`
/// - `post_tool_use`(+`_failure`) → `Identity` + `ActivityEnd{toolUseId}`
/// - `permission_denied`    → `Identity` + `ActivityEnd{toolUseId}` — a denied
///   tool never reaches `post_tool_use`, and the End both closes the activity
///   and resolves the reducer's `gated_before_waiting` entry for that tool
/// - `notification`         → `Waiting` for `permission_prompt` /
///   `elicitation_dialog`; `idle_prompt` (the 60s-idle nudge — the session is
///   merely idle, not blocked) and unknown types decode to NOTHING (unknown
///   additionally drops a drift breadcrumb)
/// - `stop` / `stop_failure` → `ActivityEnd` (turn end → idle debounce; NO
///   Identity — an end for an unknown agent proves nothing worth registering)
/// - `subagent_start`       → child `SessionStart{parent_id}` + `Rename`
/// - `subagent_stop` / `subagent_end` → child `SessionEnd{as_child: true}` —
///   BOTH spellings: the docs name `SubagentStop`, but upstream's finish-site
///   file-hook dispatch keys `SubagentEnd` (updates.rs) and the envelope
///   serializes `"subagent_end"`; registration writes both (drift-watched)
/// - `session_end`          → `SessionEnd` (best-effort — see module doc)
/// - anything else          → bail (registered-vs-decoded drift must be loud;
///   `pre_compact`/`post_compact` are deliberately unregistered)
pub fn decode_grok_hook_payload(v: &Value) -> Result<Vec<AgentEvent>> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("grok hook payload must be an object"))?;
    let event = obj
        .get("hookEventName")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("grok payload missing hookEventName"))?;
    // `cwd` is a non-optional envelope field upstream; `workspaceRoot` is the
    // defensive fallback (both are the workspace path for top-level sessions).
    let cwd = obj
        .get("cwd")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            obj.get("workspaceRoot")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
        });
    // Key on `sessionId` — consistent across every event of a session and equal
    // to the transcript's parent-dir name, so hook and watcher coalesce. The
    // cwd fallback only guards a hypothetical future event that omits it.
    let key = obj
        .get("sessionId")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .or(cwd)
        .ok_or_else(|| anyhow!("grok payload has no sessionId, cwd, or workspaceRoot"))?;
    let agent_id = AgentId::from_parts(SOURCE_NAME, key);
    let cwd_path = || cwd.map(PathBuf::from);

    let identity = || AgentEvent::Identity {
        agent_id,
        source: SOURCE_NAME.to_string(),
        session_id: key.to_string(),
        cwd: cwd_path(),
        pid: None,
    };
    let tool_use_id = || {
        obj.get("toolUseId")
            .and_then(|s| s.as_str())
            .map(String::from)
    };

    match event {
        "session_start" => {
            // NOTE: grok's `session_start` payload ALSO carries a public
            // `source` field ("new"/"load" — the start REASON, exactly CC's
            // overload). Attribution comes ONLY from `_pixtuoid_source`; never
            // read that field (the un-reapable-ghost lesson at
            // `decode_hook_payload`).
            let mut evs = vec![AgentEvent::SessionStart {
                agent_id,
                source: SOURCE_NAME.to_string(),
                session_id: key.to_string(),
                cwd: cwd.unwrap_or("").into(),
                parent_id: None,
            }];
            // The type carries `modelId` but the current fire site passes
            // None (run_loop.rs) — take it when a future build offers it.
            if let Some(model) = obj
                .get("modelId")
                .and_then(|m| m.as_str())
                .filter(|m| !m.is_empty())
            {
                evs.push(AgentEvent::ModelInfo {
                    agent_id,
                    model: Some(ellipsize(model, MAX_DECODED_FIELD_CHARS)),
                    effort: None,
                });
            }
            Ok(evs)
        }
        "user_prompt_submit" => Ok(vec![AgentEvent::SessionStart {
            agent_id,
            source: SOURCE_NAME.to_string(),
            session_id: key.to_string(),
            cwd: cwd.unwrap_or("").into(),
            parent_id: None,
        }]),
        "pre_tool_use" => {
            let tool = obj
                .get("toolName")
                .and_then(|s| s.as_str())
                .unwrap_or_else(|| {
                    crate::source::drift::missing_field(SOURCE_NAME, "pre_tool_use", "toolName");
                    "?"
                });
            Ok(vec![
                identity(),
                AgentEvent::ActivityStart {
                    agent_id,
                    tool_use_id: tool_use_id(),
                    detail: Some(grok_tool_detail(tool, obj.get("toolInput"))),
                },
            ])
        }
        // A FAILED tool fires `post_tool_use_failure` INSTEAD OF
        // `post_tool_use`; a DENIED tool fires `permission_denied` and never
        // runs at all. All three close the activity the same way — the End
        // also resolves a pending permission `Waiting` gated on this tool
        // (`gated_before_waiting`), which is exactly right for the denied
        // case: the prompt is answered, the sprite must not stay Waiting.
        "post_tool_use" | "post_tool_use_failure" | "permission_denied" => Ok(vec![
            identity(),
            AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: tool_use_id(),
            },
        ]),
        "notification" => {
            let kind = obj
                .get("notificationType")
                .and_then(|s| s.as_str())
                .unwrap_or_else(|| {
                    crate::source::drift::missing_field(
                        SOURCE_NAME,
                        "notification",
                        "notificationType",
                    );
                    "?"
                });
            match kind {
                // Fires BEFORE the permission/question prompt shows
                // (tool_calls.rs / idle_prompt.rs) — a genuine blocked-on-the-
                // human state. Resolution: approval fires NO hook (the tool
                // proceeds → its post_tool_use End clears the gate); denial
                // fires permission_denied (same End, above).
                "permission_prompt" | "elicitation_dialog" => {
                    let msg = obj
                        .get("message")
                        .and_then(|s| s.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or(kind);
                    Ok(vec![
                        identity(),
                        AgentEvent::Waiting {
                            agent_id,
                            reason: ellipsize(msg, MAX_DECODED_FIELD_CHARS),
                        },
                    ])
                }
                // The 60s-idle nudge: the session is idle, not blocked — a
                // Waiting here would misrender every lunch break as a
                // permission prompt.
                "idle_prompt" => Ok(vec![]),
                other => {
                    // Sub-type drift breadcrumb (composed name — the event
                    // itself is known, the TYPE vocabulary drifted).
                    crate::source::drift::unknown_event(
                        SOURCE_NAME,
                        &format!("notification:{other}"),
                    );
                    Ok(vec![])
                }
            }
        }
        // Turn end (`reason`: end_turn/cancelled/error; `stop_failure` is the
        // API-error twin). Identity-LESS like the shared Stop arm.
        "stop" | "stop_failure" => Ok(vec![AgentEvent::ActivityEnd {
            agent_id,
            tool_use_id: None,
        }]),
        "subagent_start" => {
            let child = subagent_child_id(obj, event)?;
            let mut evs = vec![AgentEvent::SessionStart {
                agent_id: child,
                source: SOURCE_NAME.to_string(),
                session_id: child_key(obj).unwrap_or_default(),
                // The envelope cwd is the PARENT's — correct for the default
                // (inherited-cwd) child. A worktree-ISOLATED child actually
                // runs elsewhere; its label is still fixed by the Rename below
                // and only the outfit-palette cwd key stays parent-tinted
                // (first-wins backfill) — accepted residual.
                cwd: cwd.unwrap_or("").into(),
                parent_id: Some(agent_id),
            }];
            // grok's own label precedence leads with the spawn `description`
            // (3–5 words); `subagentType` is the fallback.
            if let Some(label) = obj
                .get("description")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
                .or_else(|| obj.get("subagentType").and_then(|s| s.as_str()))
                .filter(|s| !s.is_empty())
            {
                evs.push(AgentEvent::Rename {
                    agent_id: child,
                    label: ellipsize(label, MAX_DECODED_FIELD_CHARS),
                });
            }
            Ok(evs)
        }
        "subagent_stop" | "subagent_end" => Ok(vec![AgentEvent::SessionEnd {
            agent_id: subagent_child_id(obj, event)?,
            as_child: true,
        }]),
        "session_end" => Ok(vec![AgentEvent::SessionEnd {
            agent_id,
            as_child: false,
        }]),
        other => {
            crate::source::drift::unknown_event(SOURCE_NAME, other);
            bail!("unsupported grok hook event: {other}")
        }
    }
}

/// The registry's `hook.custom` entry point: grok's envelope is ALIEN to the
/// shared CC-shaped arms (`hookEventName` vs `hook_event_name`), so it claims
/// EVERY event reaching it — `.map(Some)`, never `Ok(None)`.
pub(crate) fn decode_grok_hook_custom(v: &Value) -> Result<Option<Vec<AgentEvent>>> {
    decode_grok_hook_payload(v).map(Some)
}

/// The child's `subagentId` — upstream sets `child_session_id = subagent_id`
/// (handle_request.rs), so this key coalesces with the child's own tool hooks
/// (which carry the child's `sessionId`) AND its flat transcript dir name.
fn child_key(obj: &serde_json::Map<String, Value>) -> Option<String> {
    obj.get("subagentId")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn subagent_child_id(obj: &serde_json::Map<String, Value>, event: &str) -> Result<AgentId> {
    match child_key(obj) {
        Some(id) => Ok(AgentId::from_parts(SOURCE_NAME, &id)),
        None => {
            crate::source::drift::missing_field(SOURCE_NAME, event, "subagentId");
            bail!("grok {event} payload missing subagentId")
        }
    }
}

/// Grok tool detail: `"name: target"` over grok's snake_case tool vocabulary
/// (`run_terminal_command`/`read_file`/`search_replace`/`spawn_subagent`, …).
///
/// **`spawn_subagent` maps to `ToolDetail::Task` ONLY for an explicit
/// `background: false` (blocking) dispatch — NOT on the CC-style semantic
/// `subagent_type` detection.** Deliberate, and load-bearing: grok's spawn
/// defaults to background=TRUE (xai-tool-types task.rs `default_true`), where
/// `post_tool_use` fires at SPAWN time, not completion. A Task-detail Start
/// whose End arrives immediately would drain `active_tasks` while the child
/// is alive → the reducer's b1 drain-cascade (`B1_CASCADE_GRACE`, 2.5s) would
/// `cascade_exit` the LIVE child subtree, unrecoverably. Blocking spawns are
/// the one shape where End == completion, i.e. where CC Task semantics hold.
/// Skipping Task detail for background spawns loses nothing structural: grok
/// children are FIRST-CLASS (child-keyed tool hooks — no parent
/// misattribution to suppress) and their ends are wire-carried
/// (`subagent_stop` hook / `subagent_finished` transcript line), so neither
/// of the two jobs `active_tasks` exists for applies.
fn grok_tool_detail(tool: &str, args: Option<&Value>) -> ToolDetail {
    let is_spawn = tool == "spawn_subagent" || args.and_then(|a| a.get("subagent_type")).is_some();
    if is_spawn {
        let blocking = args
            .and_then(|a| a.get("background"))
            .and_then(Value::as_bool)
            == Some(false);
        if blocking {
            return ToolDetail::Task;
        }
        // Background (or default) spawn: fall through to a generic display —
        // the `description` key below gives it a human-readable target.
    }
    // Per-source target vocabulary; assembly + caps live in
    // `generic_tool_display` (the chokepoint — pitfall 3).
    const KEYS: &[&str] = &[
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "description",
    ];
    let target = args.and_then(|a| crate::source::decoder::first_present_str(a, KEYS));
    generic_tool_display(tool, target)
}

// ---------------------------------------------------------------------------
// Path derivers (the watcher's id/cwd come from the PATH, not line content)
// ---------------------------------------------------------------------------

/// Session id from a transcript path: the PARENT-DIR name (the filename stem
/// is the constant `updates`) — `…/sessions/<enc-cwd>/<session-id>/updates.jsonl`.
/// Same shape as copilot's parent-dir UUID. Equal to every hook event's
/// `sessionId`, so the two transports coalesce.
pub fn grok_id_from_path(path: &Path) -> String {
    path.parent()
        .and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// The session's cwd from a transcript path — updates.jsonl lines carry NO cwd
/// anywhere, so it lives one level up: the GRANDPARENT dir name is grok's
/// `encode_cwd_dirname(cwd)`. Mirrors upstream `decode_cwd_from_dirname`
/// exactly: URL-decode the name and accept it only when it looks like an
/// absolute path (Unix `/…`, Windows drive letter); otherwise it's the
/// `{slug}-{blake3_hex16}` long-path form, whose original cwd upstream records
/// in a sibling `.cwd` file (trimmed).
pub fn grok_cwd_from_path(path: &Path) -> Option<PathBuf> {
    let group = path.parent()?.parent()?;
    let name = group.file_name()?.to_str()?;
    if let Some(decoded) = percent_decode(name) {
        // Upstream's own absolute-path test distinguishes the two encodings.
        if decoded.starts_with('/') || (cfg!(windows) && decoded.chars().nth(1) == Some(':')) {
            return Some(PathBuf::from(decoded));
        }
    }
    // Long-path fallback: the `.cwd` metadata file (small by construction —
    // it holds one filesystem path; the bounded read guards a planted file).
    let raw = read_bounded(&group.join(".cwd"), MAX_CWD_FILE_BYTES)?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

/// A `.cwd` file holds one path (< 4 KiB by construction: upstream only writes
/// it when the URL-encoded cwd exceeds 255 bytes, and PATH_MAX-scale inputs
/// are ~1–4 KiB); the cap only guards a planted oversized file.
const MAX_CWD_FILE_BYTES: u64 = 4096;

fn read_bounded(path: &Path, cap: u64) -> Option<String> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut buf = String::new();
    f.take(cap).read_to_string(&mut buf).ok()?;
    Some(buf)
}

/// Pure `%XX` percent-decoding (the inverse of upstream's
/// `urlencoding::encode`, which never emits `+` for spaces — so `+` passes
/// through literally, matching `urlencoding::decode`). Returns `None` on
/// malformed escapes or non-UTF-8 decoded bytes.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hi = (hex[0] as char).to_digit(16)?;
            let lo = (hex[1] as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The grok home dir — `$GROK_HOME` UNCONDITIONALLY when set (grok takes the
/// env var without an exists-check and `create_dir_all`s it, unlike codex's
/// gate), else `<home>/.grok`. The public entry BOTH the watcher's
/// `default_paths()` and the installer's `default_config_path()` (and the
/// liveness probe) route through, so the watched root, the installed hooks
/// file, and the probed registry can never disagree. See
/// `crate::platform::grok_home`.
pub fn grok_home() -> PathBuf {
    crate::platform::grok_home()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode_all(v: Value) -> Vec<AgentEvent> {
        decode_grok_hook_payload(&v).expect("decodes")
    }

    /// The payload's MAIN event — the last decoded event (activity arms
    /// prepend an `Identity`, subagent_start appends a `Rename`).
    fn decode(v: Value) -> AgentEvent {
        decode_all(v).pop().expect("at least one event")
    }

    fn envelope(event: &str) -> Value {
        json!({
            "hookEventName": event,
            "sessionId": "0197fa30-sess",
            "cwd": "/Users/dev/proj",
            "workspaceRoot": "/Users/dev/proj",
            "timestamp": "2026-07-16T12:00:00Z"
        })
    }

    // ---- keying + lifecycle arms ----

    #[test]
    fn session_start_keys_on_session_id() {
        let mut v = envelope("session_start");
        v["source"] = json!("new");
        let ev = decode(v);
        match ev {
            AgentEvent::SessionStart {
                agent_id,
                source,
                session_id,
                cwd,
                parent_id,
            } => {
                assert_eq!(source, SOURCE_NAME);
                assert_eq!(agent_id, AgentId::from_parts(SOURCE_NAME, "0197fa30-sess"));
                assert_eq!(session_id, "0197fa30-sess");
                assert_eq!(cwd, PathBuf::from("/Users/dev/proj"));
                assert_eq!(parent_id, None);
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn session_start_public_source_field_never_drives_attribution() {
        // grok reuses `source` for the start REASON ("new"/"load") — the CC
        // overload that once split agents into un-reapable ghosts. The decoded
        // source must be OURS regardless of the field's value.
        for reason in ["new", "load"] {
            let mut v = envelope("session_start");
            v["source"] = json!(reason);
            match decode(v) {
                AgentEvent::SessionStart { source, .. } => assert_eq!(source, SOURCE_NAME),
                other => panic!("expected SessionStart, got {other:?}"),
            }
        }
    }

    #[test]
    fn session_start_takes_model_id_when_offered() {
        // The fire site passes None today; the type has the field — decode it
        // when a future build fills it.
        let mut v = envelope("session_start");
        v["modelId"] = json!("grok-4-code");
        let evs = decode_all(v);
        assert_eq!(evs.len(), 2);
        assert!(
            matches!(&evs[1], AgentEvent::ModelInfo { model: Some(m), effort: None, .. }
            if m == "grok-4-code")
        );
        // Absent → exactly one event.
        assert_eq!(decode_all(envelope("session_start")).len(), 1);
    }

    #[test]
    fn user_prompt_submit_is_the_resurrect_carrier() {
        // Maps to SessionStart (idempotent for a live slot) so a stale-swept
        // LIVE session re-registers on its next prompt — grok's session_end
        // does not fire on TUI quit, making this the Codex-class carrier.
        let ev = decode(envelope("user_prompt_submit"));
        assert!(matches!(ev, AgentEvent::SessionStart { agent_id, .. }
            if agent_id == AgentId::from_parts(SOURCE_NAME, "0197fa30-sess")));
    }

    #[test]
    fn session_end_maps_to_root_session_end() {
        let mut v = envelope("session_end");
        v["reason"] = json!("shutdown");
        assert!(matches!(
            decode(v),
            AgentEvent::SessionEnd {
                as_child: false,
                ..
            }
        ));
    }

    // ---- tool activity ----

    #[test]
    fn pre_tool_use_is_identity_plus_activity_start_with_tool_id() {
        let mut v = envelope("pre_tool_use");
        v["toolName"] = json!("run_terminal_command");
        v["toolUseId"] = json!("call_42");
        v["toolInput"] = json!({"command": "cargo test"});
        v["toolInputTruncated"] = json!(false);
        let evs = decode_all(v);
        assert_eq!(evs.len(), 2);
        match &evs[0] {
            AgentEvent::Identity {
                session_id,
                cwd,
                pid: None,
                ..
            } => {
                assert_eq!(session_id, "0197fa30-sess");
                assert_eq!(cwd.as_deref(), Some(Path::new("/Users/dev/proj")));
            }
            other => panic!("expected leading Identity, got {other:?}"),
        }
        match &evs[1] {
            AgentEvent::ActivityStart {
                tool_use_id,
                detail,
                ..
            } => {
                assert_eq!(tool_use_id.as_deref(), Some("call_42"));
                assert_eq!(
                    detail.as_ref().unwrap().display(),
                    "run_terminal_command: cargo test"
                );
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn post_tool_use_variants_and_denial_close_the_activity() {
        // post_tool_use / post_tool_use_failure / permission_denied all end
        // the SAME tool id — the denial arm is what resolves a Waiting gated
        // on the denied tool (no post_tool_use will ever come).
        for event in [
            "post_tool_use",
            "post_tool_use_failure",
            "permission_denied",
        ] {
            let mut v = envelope(event);
            v["toolName"] = json!("run_terminal_command");
            v["toolUseId"] = json!("call_42");
            let evs = decode_all(v);
            assert_eq!(evs.len(), 2, "{event}: Identity + End");
            assert!(
                matches!(&evs[1], AgentEvent::ActivityEnd { tool_use_id: Some(id), .. }
                    if id == "call_42"),
                "{event} must end tool call_42"
            );
        }
    }

    #[test]
    fn stop_and_stop_failure_are_identityless_turn_ends() {
        for event in ["stop", "stop_failure"] {
            let evs = decode_all(envelope(event));
            assert_eq!(evs.len(), 1, "{event}: exactly one event");
            assert!(
                matches!(
                    &evs[0],
                    AgentEvent::ActivityEnd {
                        tool_use_id: None,
                        ..
                    }
                ),
                "{event} must decode to a bare ActivityEnd"
            );
        }
    }

    // ---- the b1 trap: spawn_subagent Task detail ----

    #[test]
    fn blocking_spawn_is_task_background_and_default_are_not() {
        // background:false (blocking) → PostToolUse == completion → CC Task
        // semantics hold → ToolDetail::Task.
        let blocking = grok_tool_detail(
            "spawn_subagent",
            Some(&json!({"subagent_type": "explore", "background": false})),
        );
        assert!(blocking.is_task(), "blocking spawn must read Delegating");

        // background:true AND the absent-field DEFAULT (upstream default_true)
        // → PostToolUse fires at SPAWN → Task detail would b1-cascade the LIVE
        // child. Both must stay generic.
        for input in [
            json!({"subagent_type": "explore", "background": true}),
            json!({"subagent_type": "explore", "description": "map the build"}),
        ] {
            let detail = grok_tool_detail("spawn_subagent", Some(&input));
            assert!(
                !detail.is_task(),
                "background/default spawn must NOT be Task (b1 would evict the live child): {input}"
            );
        }
        // Input-less spawn (degenerate) — default is background → generic.
        assert!(!grok_tool_detail("spawn_subagent", None).is_task());
        // The semantic field alone (renamed dispatch) follows the same rule.
        let renamed = grok_tool_detail(
            "task",
            Some(&json!({"subagent_type": "explore", "background": false})),
        );
        assert!(
            renamed.is_task(),
            "semantic detection still applies when blocking"
        );
    }

    #[test]
    fn background_spawn_displays_description_as_target() {
        let detail = grok_tool_detail(
            "spawn_subagent",
            Some(&json!({"subagent_type": "explore", "description": "map the build"})),
        );
        assert_eq!(detail.display(), "spawn_subagent: map the build");
    }

    #[test]
    fn tool_target_uses_grok_arg_vocabulary() {
        let mut v = envelope("pre_tool_use");
        v["toolName"] = json!("read_file");
        v["toolInput"] = json!({"path": "src/lib.rs"});
        assert!(
            matches!(decode(v), AgentEvent::ActivityStart { detail: Some(d), .. }
            if d.display() == "read_file: src/lib.rs")
        );
    }

    #[test]
    fn long_targets_are_truncated_at_the_decode_boundary() {
        let mut v = envelope("pre_tool_use");
        v["toolName"] = json!("run_terminal_command");
        v["toolInput"] = json!({"command": "x".repeat(300)});
        match decode(v) {
            AgentEvent::ActivityStart {
                detail: Some(d), ..
            } => {
                let display = d.display();
                assert!(display.ends_with('…'), "must be ellipsized: {display}");
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    // ---- waiting (notification) ----

    #[test]
    fn permission_and_elicitation_notifications_are_waiting() {
        for kind in ["permission_prompt", "elicitation_dialog"] {
            let mut v = envelope("notification");
            v["notificationType"] = json!(kind);
            v["message"] = json!("Tool permission requested");
            let evs = decode_all(v);
            assert_eq!(evs.len(), 2, "{kind}: Identity + Waiting");
            assert!(
                matches!(&evs[1], AgentEvent::Waiting { reason, .. }
                    if reason == "Tool permission requested"),
                "{kind} must decode to Waiting"
            );
        }
    }

    #[test]
    fn waiting_reason_falls_back_to_the_notification_type() {
        let mut v = envelope("notification");
        v["notificationType"] = json!("permission_prompt");
        assert!(matches!(decode(v), AgentEvent::Waiting { reason, .. }
            if reason == "permission_prompt"));
    }

    #[test]
    fn idle_prompt_and_unknown_notification_types_decode_to_nothing() {
        // idle_prompt = the 60s-idle nudge, NOT a blocked state; an unknown
        // type must not invent a Waiting either (drift breadcrumb only).
        for kind in ["idle_prompt", "some_future_nudge"] {
            let mut v = envelope("notification");
            v["notificationType"] = json!(kind);
            assert!(
                decode_all(v).is_empty(),
                "{kind} must decode to zero events"
            );
        }
    }

    // ---- subagents ----

    #[test]
    fn subagent_start_registers_the_child_under_the_parent() {
        let mut v = envelope("subagent_start");
        v["subagentId"] = json!("0197fa31-child");
        v["subagentType"] = json!("explore");
        v["description"] = json!("map the build");
        let evs = decode_all(v);
        assert_eq!(evs.len(), 2);
        match &evs[0] {
            AgentEvent::SessionStart {
                agent_id,
                session_id,
                parent_id,
                ..
            } => {
                assert_eq!(
                    *agent_id,
                    AgentId::from_parts(SOURCE_NAME, "0197fa31-child"),
                    "child keys on subagentId (== child session id)"
                );
                assert_eq!(session_id, "0197fa31-child");
                assert_eq!(
                    *parent_id,
                    Some(AgentId::from_parts(SOURCE_NAME, "0197fa30-sess")),
                    "parent link from the envelope's (parent) sessionId"
                );
            }
            other => panic!("expected child SessionStart, got {other:?}"),
        }
        assert!(
            matches!(&evs[1], AgentEvent::Rename { agent_id, label }
                if *agent_id == AgentId::from_parts(SOURCE_NAME, "0197fa31-child")
                    && label == "map the build"),
            "description is the primary label (grok's own precedence)"
        );
    }

    #[test]
    fn subagent_rename_falls_back_to_type_when_description_absent() {
        let mut v = envelope("subagent_start");
        v["subagentId"] = json!("c");
        v["subagentType"] = json!("explore");
        let evs = decode_all(v);
        assert!(matches!(&evs[1], AgentEvent::Rename { label, .. } if label == "explore"));
    }

    #[test]
    fn both_subagent_stop_spellings_end_the_child_as_child() {
        // Docs say SubagentStop; upstream's finish site fires SubagentEnd
        // (serialized "subagent_end"). BOTH must decode — whichever spelling a
        // given build emits, the child ends promptly.
        for event in ["subagent_stop", "subagent_end"] {
            let mut v = envelope(event);
            v["subagentId"] = json!("0197fa31-child");
            v["subagentType"] = json!("explore");
            v["exitCode"] = json!(0);
            let evs = decode_all(v);
            assert_eq!(evs.len(), 1);
            assert!(
                matches!(&evs[0], AgentEvent::SessionEnd { agent_id, as_child: true }
                    if *agent_id == AgentId::from_parts(SOURCE_NAME, "0197fa31-child")),
                "{event} must end the CHILD with the as_child stamp"
            );
        }
    }

    #[test]
    fn subagent_events_without_subagent_id_are_malformed() {
        for event in ["subagent_start", "subagent_stop", "subagent_end"] {
            assert!(
                decode_grok_hook_payload(&envelope(event)).is_err(),
                "{event} without subagentId must bail"
            );
        }
    }

    // ---- coalescing + malformed ----

    #[test]
    fn all_events_for_one_session_share_one_agent_id() {
        let sid = "0197fa30-sess";
        let mut pre = envelope("pre_tool_use");
        pre["toolName"] = json!("read_file");
        pre["toolUseId"] = json!("c1");
        let mut post = envelope("post_tool_use");
        post["toolUseId"] = json!("c1");
        let mut note = envelope("notification");
        note["notificationType"] = json!("permission_prompt");
        let events = [
            envelope("session_start"),
            envelope("user_prompt_submit"),
            pre,
            note,
            post,
            envelope("stop"),
            envelope("session_end"),
        ];
        let ids: std::collections::BTreeSet<_> = events
            .iter()
            .flat_map(|v| decode_grok_hook_payload(v).unwrap())
            .map(|e| e.agent_id())
            .collect();
        assert_eq!(ids.len(), 1, "all root events must coalesce to one AgentId");
        assert!(ids.contains(&AgentId::from_parts(SOURCE_NAME, sid)));
    }

    #[test]
    fn key_falls_back_to_cwd_when_session_id_absent() {
        let ev = decode(json!({
            "hookEventName": "stop",
            "cwd": "/Users/dev/proj"
        }));
        assert!(matches!(ev, AgentEvent::ActivityEnd { agent_id, .. }
            if agent_id == AgentId::from_parts(SOURCE_NAME, "/Users/dev/proj")));
    }

    #[test]
    fn nothing_to_key_on_is_malformed() {
        assert!(decode_grok_hook_payload(&json!({"hookEventName": "stop"})).is_err());
        assert!(decode_grok_hook_payload(
            &json!({"hookEventName": "stop", "cwd": "", "workspaceRoot": ""})
        )
        .is_err());
        assert!(decode_grok_hook_payload(&json!("just a string")).is_err());
        assert!(decode_grok_hook_payload(&json!({"sessionId": "s"})).is_err());
    }

    #[test]
    fn unregistered_events_bail_loudly() {
        // pre/post_compact are deliberately unregistered; a PascalCase or
        // CC-style value reaching this decoder is drift and must be loud.
        for ev in ["pre_compact", "post_compact", "PreToolUse", "bogus"] {
            assert!(
                decode_grok_hook_payload(&envelope(ev)).is_err(),
                "{ev} must bail"
            );
        }
    }

    // ---- path derivers ----

    #[test]
    fn id_is_the_parent_dir_name() {
        let p = Path::new("/home/u/.grok/sessions/%2Fhome%2Fu%2Fproj/0197fa30-sess/updates.jsonl");
        assert_eq!(grok_id_from_path(p), "0197fa30-sess");
    }

    #[test]
    fn cwd_decodes_from_the_urlencoded_group_dir() {
        let p = Path::new(
            "/home/u/.grok/sessions/%2FUsers%2Fdev%2Fmy%20proj/0197fa30-sess/updates.jsonl",
        );
        assert_eq!(
            grok_cwd_from_path(p),
            Some(PathBuf::from("/Users/dev/my proj"))
        );
    }

    #[test]
    fn cwd_slug_form_reads_the_dot_cwd_file() {
        // The >255-byte encoded form is `{slug}-{blake3_hex16}` — never
        // absolute after decoding — and upstream records the real cwd in a
        // sibling `.cwd` file.
        let tmp = std::env::temp_dir().join(format!("pixtuoid-grok-cwd-{}", std::process::id()));
        let group = tmp.join("sessions").join("deep-project-a1b2c3d4e5f60718");
        let session = group.join("0197fa30-sess");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(group.join(".cwd"), "/very/deep/project\n").unwrap();
        let p = session.join("updates.jsonl");
        assert_eq!(
            grok_cwd_from_path(&p),
            Some(PathBuf::from("/very/deep/project"))
        );
        // No `.cwd` file → None (never guess from a slug).
        std::fs::remove_file(group.join(".cwd")).unwrap();
        assert_eq!(grok_cwd_from_path(&p), None);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn percent_decode_handles_escapes_and_rejects_malformed() {
        assert_eq!(percent_decode("%2Fa%20b"), Some("/a b".into()));
        assert_eq!(percent_decode("plain"), Some("plain".into()));
        // `+` passes through literally (urlencoding never emits it for space).
        assert_eq!(percent_decode("a+b"), Some("a+b".into()));
        assert_eq!(percent_decode("%2"), None, "truncated escape");
        assert_eq!(percent_decode("%zz"), None, "non-hex escape");
        assert_eq!(percent_decode("%FF"), None, "invalid UTF-8 byte");
    }
}
