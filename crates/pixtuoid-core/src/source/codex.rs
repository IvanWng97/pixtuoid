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

use crate::source::decoder::{cwd_basename_label, make_tool_detail};
#[cfg(feature = "native")]
use crate::source::fd_probe;
#[cfg(feature = "native")]
use crate::source::jsonl::{ChildEndUnclaims, JsonlWatcher, ProbeSnapshot};
use crate::source::AgentEvent;
#[cfg(feature = "native")]
use crate::source::{Source, TaggedSender};
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
        ("event_msg", "task_started") => vec![start()],
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

pub fn derive_codex_label(_path: &Path, _source: &str, cwd: &Path) -> String {
    cwd_basename_label("cx", cwd).unwrap_or_else(|| "cx".to_string())
}

/// Codex writes no session-end marker; the reducer's stale-sweep reaps dead
/// sessions. Always false (defer to mtime window + stale-sweep).
#[cfg(feature = "native")]
fn codex_session_ended(_tail: &[u8]) -> bool {
    false
}

/// Codex's liveness probe: the rollout UUIDs (in `codex_id_from_path`
/// id-space, so they join the watcher's first-sight gate directly) of every
/// rollout under `sessions_root` held OPEN by a running `codex` process,
/// plus the owning pid per id.
///
/// Codex has no session registry (unlike CC's `sessions/<pid>.json`), but a
/// live `codex` process holds its rollout file open in append mode for the
/// whole session (upstream `RolloutRecorder` owns the handle), so an open
/// rollout fd IS the first-party liveness signal: pid → open fd → rollout
/// path → UUID. Failure is explicit (#223): `None` ONLY when the proc-table
/// enumeration itself fails (the watcher then changes nothing). An ABSENT
/// sessions root is NOT a failure — codex may simply never have run — so it
/// returns `Some(empty)`: a healthy "nothing alive" observation. Per-pid fd
/// failures stay non-failures (a pid exiting mid-probe is normal).
#[cfg(feature = "native")]
pub fn live_codex_rollout_ids(sessions_root: &Path) -> Option<ProbeSnapshot> {
    // Canonicalize once per probe call: kernel-reported fd paths are fully
    // resolved (e.g. /tmp → /private/tmp on macOS), so the prefix compare
    // must run against the canonical root or every rollout misses.
    let Ok(root) = sessions_root.canonicalize() else {
        tracing::debug!(
            "codex probe: sessions root {} not canonicalizable; nothing alive there",
            sessions_root.display()
        );
        return Some(ProbeSnapshot::default());
    };
    let pids = fd_probe::pids_by_name("codex")?;
    let pairs = pids.into_iter().flat_map(|pid| {
        fd_probe::open_vnode_paths(pid)
            .into_iter()
            .map(move |path| (pid, path))
    });
    Some(rollout_ids_from_paths(&root, pairs))
}

/// The pure join half of the probe (unit-testable without FFI): keep the
/// (pid, path) pairs whose path is a `rollout-*.jsonl` under `root`, mapped
/// through `codex_id_from_path` — the watcher's `IdDeriver`, so probe ids and
/// gate ids can't drift. Each surviving pair also binds id → pid for the
/// snapshot's `pid_of` (the exit-watch half).
#[cfg(feature = "native")]
fn rollout_ids_from_paths(
    root: &Path,
    pairs: impl Iterator<Item = (i32, PathBuf)>,
) -> ProbeSnapshot {
    let mut snap = ProbeSnapshot::default();
    for (pid, path) in pairs {
        if !path.starts_with(root) || !is_rollout_filename(&path) {
            continue;
        }
        tracing::debug!("codex probe: pid {pid} holds {} open", path.display());
        let id = codex_id_from_path(&path);
        // Two live processes holding ONE rollout open (a resume overlap) must
        // not bind id→pid by proc-enumeration order — the same determinism
        // rule as the CC registry fold's no-startedAt arm (#252): larger pid
        // wins, arbitrary but stable for live processes.
        let bound = snap.pid_of.entry(id.clone()).or_insert(pid);
        if pid > *bound {
            *bound = pid;
        }
        snap.ids.insert(id);
    }
    snap
}

#[cfg(feature = "native")]
fn is_rollout_filename(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("jsonl")
        && path
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.starts_with("rollout-"))
}

/// Attach the probe ONLY for codex's first-party layout: the standard
/// `~/.codex/sessions` shape (the root's file_name is literally `sessions`
/// AND its parent's is `.codex`) or the resolved `codex_home()/sessions` for
/// THIS environment (a `CODEX_HOME` user's real rollout root — codex itself
/// writes there, and rejecting it would silently drop the whole liveness
/// ladder for a supported config). Mirrors `cc_sessions_dir`'s gating: a
/// `--codex-sessions-root /tmp/fixture` replay points at an arbitrary dir,
/// and those runs must keep the pure-mtime first-sight gate (the probe is
/// additive-only; a replayed rollout vouched for by a coincidentally-running
/// codex would resurrect as live).
#[cfg(feature = "native")]
fn codex_probe_root(sessions_root: &Path) -> Option<PathBuf> {
    codex_probe_root_resolved(sessions_root, &codex_home())
}

/// The injectable core of [`codex_probe_root`] (mirrors
/// `platform::resolve_codex_home`'s testable split): `home` is the resolved
/// codex home for this environment.
#[cfg(feature = "native")]
fn codex_probe_root_resolved(sessions_root: &Path, home: &Path) -> Option<PathBuf> {
    if sessions_root.file_name().and_then(|n| n.to_str()) != Some("sessions") {
        return None;
    }
    let parent = sessions_root.parent();
    let parent_is_codex =
        parent.and_then(|p| p.file_name()).and_then(|n| n.to_str()) == Some(".codex");
    // A parent that IS the resolved codex home is first-party even when not
    // named `.codex` — the CODEX_HOME case (`codex_home()` honors the env
    // var the same way `default_paths` does, one resolution for both).
    let parent_is_resolved_home = parent.is_some_and(|p| p == home);
    if !parent_is_codex && !parent_is_resolved_home {
        return None;
    }
    // Not canonicalized here: the dir may not exist yet at wiring time
    // (codex never run); `live_codex_rollout_ids` canonicalizes per probe
    // call, which also picks up a root created after startup.
    Some(sessions_root.to_path_buf())
}

/// The Codex home dir — honors `CODEX_HOME` when it points at an existing dir,
/// else `~/.codex` (codex's own precedence). The public entry the installer
/// routes its `config.toml` path through too, so the watched sessions root and
/// the installed-hook config can never disagree. See `crate::platform::codex_home`.
pub fn codex_home() -> PathBuf {
    crate::platform::codex_home()
}

/// Source that watches the Codex session transcript directory.
#[cfg(feature = "native")]
pub struct CodexSource {
    pub sessions_root: PathBuf,
    /// The #246 child-end un-claim side-channel — Codex is consumer-only:
    /// its `SubagentStop` hooks ride the shared socket the `HookRouter`
    /// owns (whose tee is the producer), and THIS watcher releases the ended
    /// child's rollout claim so a multi-turn child's turn-N+1 append
    /// re-registers (the motivating #246 case). The runtime shares ONE
    /// handle across the router + the CC and Codex watchers; `None` disables
    /// it (bare test construction).
    pub child_end_unclaims: Option<ChildEndUnclaims>,
}

#[cfg(feature = "native")]
impl CodexSource {
    pub fn default_paths() -> Self {
        Self {
            sessions_root: codex_home().join("sessions"),
            child_end_unclaims: None,
        }
    }
}

#[cfg(feature = "native")]
impl Source for CodexSource {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        let mut watcher = JsonlWatcher::new(
            self.sessions_root.clone(),
            SOURCE_NAME.to_string(),
            decode_codex_line,
            derive_codex_label,
            codex_session_ended,
        )
        .with_id_deriver(codex_id_from_path);
        if let Some(root) = codex_probe_root(&self.sessions_root) {
            watcher = watcher
                .with_liveness_probe(std::sync::Arc::new(move || live_codex_rollout_ids(&root)));
        }
        if let Some(unclaims) = &self.child_end_unclaims {
            watcher = watcher.with_child_end_unclaims(unclaims.clone());
        }
        watcher.run(tx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

    #[test]
    fn codex_session_ended_is_always_false() {
        // Codex writes no end marker — the checker always defers to the
        // mtime window + stale-sweep.
        assert!(!codex_session_ended(b"anything"));
        assert!(!codex_session_ended(b""));
    }

    // ---- liveness probe (open-rollout FD binding) ----

    const UUID: &str = "019e7762-9ded-7e33-be41-946ecf105bf4";

    fn snap_of(root: &Path, paths: Vec<PathBuf>) -> ProbeSnapshot {
        rollout_ids_from_paths(root, paths.into_iter().map(|p| (42, p)))
    }

    #[test]
    fn rollout_under_root_yields_its_uuid_bound_to_its_pid() {
        let root = Path::new("/home/u/.codex/sessions");
        // Real layout nests YYYY/MM/DD below the root — starts_with must
        // admit the whole subtree, not only direct children.
        let nested = root.join(format!(
            "2026/06/10/rollout-2026-06-10T08-00-00-{UUID}.jsonl"
        ));
        let got = snap_of(root, vec![nested]);
        assert_eq!(got.ids, std::iter::once(UUID.to_string()).collect());
        // #223: the snapshot binds each id to the OWNING pid (the exit-watch
        // half) — the (42, path) pair above must survive the join intact.
        assert_eq!(got.pid_of.get(UUID), Some(&42));
    }

    #[test]
    fn shared_rollout_binds_the_larger_pid_regardless_of_enumeration_order() {
        // Two live processes holding ONE rollout (a resume overlap, #252's
        // codex sibling): the binding must be the deterministic tiebreak
        // winner in BOTH presentation orders, never last-writer-wins.
        let root = Path::new("/home/u/.codex/sessions");
        let path = root.join(format!(
            "2026/06/10/rollout-2026-06-10T08-00-00-{UUID}.jsonl"
        ));
        for pids in [[100, 200], [200, 100]] {
            let got = rollout_ids_from_paths(root, pids.into_iter().map(|p| (p, path.clone())));
            assert_eq!(got.ids, std::iter::once(UUID.to_string()).collect());
            assert_eq!(
                got.pid_of.get(UUID),
                Some(&200),
                "the larger pid must win in both enumeration orders"
            );
        }
    }

    #[test]
    fn rollout_outside_root_is_excluded() {
        let root = Path::new("/home/u/.codex/sessions");
        let outside = PathBuf::from(format!("/tmp/elsewhere/rollout-1-{UUID}.jsonl"));
        let got = snap_of(root, vec![outside]);
        assert!(got.ids.is_empty());
        assert!(got.pid_of.is_empty());
    }

    #[test]
    fn non_rollout_files_under_root_are_excluded() {
        let root = Path::new("/home/u/.codex/sessions");
        let wrong_stem = root.join("2026/06/10/history.jsonl");
        let wrong_ext = root.join(format!("2026/06/10/rollout-1-{UUID}.log"));
        let no_ext = root.join("2026/06/10/rollout-noext");
        assert!(snap_of(root, vec![wrong_stem, wrong_ext, no_ext])
            .ids
            .is_empty());
    }

    #[test]
    fn probe_root_requires_dot_codex_sessions_layout() {
        assert_eq!(
            codex_probe_root(Path::new("/home/u/.codex/sessions")),
            Some(PathBuf::from("/home/u/.codex/sessions"))
        );
        // A fixture replay root must get NO probe (pure-mtime behavior).
        assert_eq!(codex_probe_root(Path::new("/tmp/fixture")), None);
        // A bare relative `sessions` has no parent to check.
        assert_eq!(codex_probe_root(Path::new("sessions")), None);
    }

    #[test]
    fn probe_root_accepts_resolved_codex_home_sessions_layout() {
        // A CODEX_HOME-shaped layout: the resolved home is NOT named
        // `.codex`, but its `sessions` child is codex's first-party rollout
        // root for this environment — the probe must attach, or CODEX_HOME
        // users silently lose the entire liveness ladder (admission bypass,
        // ProofOfLife, negative vouch, instant exit). The env→home
        // resolution itself is pinned by `platform::resolve_codex_home`'s
        // unit tests; this pins the probe gate against the resolved value.
        let home = tempfile::tempdir().unwrap();
        let sessions = home.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        assert_eq!(
            codex_probe_root_resolved(&sessions, home.path()),
            Some(sessions.clone())
        );
        // Replay roots stay probe-less even with a custom home resolved.
        assert_eq!(
            codex_probe_root_resolved(Path::new("/tmp/fixture"), home.path()),
            None
        );
        // `sessions` under a parent that is neither `.codex` nor the
        // resolved home is not first-party.
        assert_eq!(
            codex_probe_root_resolved(Path::new("/srv/other/sessions"), home.path()),
            None
        );
    }

    #[test]
    fn live_ids_for_missing_root_is_some_empty_not_a_failure() {
        // canonicalize() fails on a nonexistent dir, but an ABSENT root is
        // not a probe failure — codex may simply never have run. Some(empty)
        // is the healthy "nothing alive" observation (#223: None would freeze
        // the negative-vouch ledger forever on machines without codex).
        let missing = Path::new("/definitely/not/a/real/.codex/sessions");
        let snap = live_codex_rollout_ids(missing).expect("absent root is not a probe failure");
        assert!(snap.ids.is_empty());
        assert!(snap.pid_of.is_empty());
    }

    #[test]
    fn live_ids_for_unrelated_root_is_empty() {
        // Real FFI smoke: whatever processes exist, none hold a rollout open
        // under a fresh tempdir.
        let dir = tempfile::tempdir().unwrap();
        let snap = live_codex_rollout_ids(dir.path())
            .expect("a healthy system's enumeration must succeed");
        assert!(snap.ids.is_empty());
    }
}
