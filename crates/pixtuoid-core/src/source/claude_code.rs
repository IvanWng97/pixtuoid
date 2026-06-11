use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use crate::source::cc_probe::cc_sessions_dir;
// The registry-probe machinery lives in `source/cc_probe.rs`; the public path
// `claude_code::live_cc_session_ids` is preserved via this re-export.
pub use crate::source::cc_probe::live_cc_session_ids;
use crate::source::decoder::{cwd_basename_label, make_tool_detail};
use crate::source::hook::HookSocketListener;
use crate::source::jsonl::{ChildEndUnclaims, JsonlWatcher};
use crate::source::{AgentEvent, Source, TaggedReceiver, TaggedSender, Transport};
use crate::AgentId;

pub const SOURCE_NAME: &str = "claude-code";

/// CC's session/agent id = the transcript filename stem, which is
/// cwd-independent (the cwd-derived project-dir is the *parent* dir, not the
/// stem): `<uuid>.jsonl` → `<uuid>` for a root, `agent-<id>.jsonl` →
/// `agent-<id>` for a subagent. Mirrors `codex_id_from_path`. CC session UUIDs
/// and agent-ids are lowercase, so the Windows path fold is inert here.
pub fn cc_id_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

/// CC's source-specific hook arms — `SubagentStart`/`SubagentStop` (#241).
/// Like the Codex twin (`codex::decode_codex_hook_custom`) they change the
/// event's SUBJECT to the child's AgentId, which the shared session-keyed arms
/// cannot express; every other CC hook event falls through (`Ok(None)`) to
/// those shared arms. Dispatched via `registry::HookDecoding::custom`.
///
/// Why CC needs these at all when its subagents already register via JSONL:
/// a Workflow-tool fleet spawns subagents with NO per-agent `Agent` tool_use
/// in the parent transcript (b1 Task-drain structurally can't fire) and the
/// subagent transcripts carry NO end marker — without `SubagentStop`, finished
/// fleet agents idle until the 10/30-min stale sweeps batch-reap them, holding
/// desks and starving the next wave. Wire facts (captured live, CC v2.1.170,
/// pinned in `tests/sources/claude/fixtures/hook-payloads.jsonl`): the
/// payload's `agent_id` is BARE hex (no `agent-` prefix) while the transcript
/// filename stem — the JSONL watcher's id space (`cc_id_from_path`) — is
/// `agent-<id>`; `SubagentStop` additionally carries `agent_transcript_path`
/// (the subagent's own transcript, incl. the deeper `subagents/workflows/
/// wf_*/` nesting), which is the authoritative key.
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
    // Per the registry's custom-decoder contract: claim our two events FULLY
    // (Err on malformed instances), never fall through. An empty `session_id`
    // or `agent_id` would mint a phantom that never coalesces with the real
    // subagent transcript — reject rather than decode.
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
    // The wire id is bare hex; prefix it into the transcript-stem id space.
    // Tolerate an already-prefixed form (the CC docs' example shows one even
    // though the live wire sends bare) without double-prefixing.
    let prefixed = if wire_agent_id.starts_with("agent-") {
        wire_agent_id.to_string()
    } else {
        format!("agent-{wire_agent_id}")
    };
    if event == "SubagentStart" {
        // Instant registration with the parent link — the JSONL watcher's
        // later SessionStart for the same transcript coalesces (duplicate
        // SessionStart = enrichment no-op in the reducer). Mirrors the Codex
        // arm field-by-field so the reducer treats both identically.
        let cwd = obj.get("cwd").and_then(|s| s.as_str()).unwrap_or("").into();
        Ok(Some(vec![AgentEvent::SessionStart {
            agent_id: AgentId::from_parts(SOURCE_NAME, &prefixed),
            source: SOURCE_NAME.to_string(),
            session_id: prefixed,
            cwd,
            parent_id: Some(AgentId::from_parts(SOURCE_NAME, session_id)),
        }]))
    } else {
        // SubagentStop: end the CHILD promptly (else its transcript lingers
        // to the 10/30-min stale sweeps). Best-effort, mirroring the Codex
        // twin: losing the race against the child's slot creation leaves a
        // harmless no-op + the stale-sweep fallback. The authoritative key is
        // the subagent transcript's filename stem (`cc_id_from_path` on
        // `agent_transcript_path` — EXACT parity with the watcher's id
        // deriver, immune to a prefix-scheme drift); the prefixed wire id is
        // the fallback when the path is absent.
        let path_key = obj
            .get("agent_transcript_path")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .map(|p| cc_id_from_path(Path::new(&crate::source::decoder::normalize_path_key(p))))
            .filter(|s| !s.is_empty());
        if let Some(ref k) = path_key {
            if *k != prefixed {
                // Drift alarm: the stem and the prefixed wire id disagree —
                // upstream changed the filename scheme or the prefix. The
                // stem keeps THIS exit keyed to the watcher, but hook-FIRST
                // registrations (Start keys on the prefixed form) would
                // become sweep-cleared phantoms — genuinely actionable, so
                // warn (it reaches the warn-floor file log), unlike the
                // per-dispatch tool-name breadcrumbs.
                tracing::warn!(
                    stem = %k,
                    wire = %prefixed,
                    "SubagentStop transcript stem != prefixed agent_id; keying on the stem"
                );
            }
        }
        Ok(Some(vec![AgentEvent::SessionEnd {
            agent_id: AgentId::from_parts(SOURCE_NAME, &path_key.unwrap_or(prefixed)),
            as_child: true,
        }]))
    }
}

pub struct ClaudeCodeSource {
    pub socket_path: PathBuf,
    pub projects_root: PathBuf,
    /// The #246 child-end un-claim side-channel. This source is BOTH the
    /// producer (its hook tee observes every decoded `SubagentStop` — all
    /// sources' hooks ride the one shared socket it owns) and a consumer
    /// (its own watcher releases CC child-transcript claims, the rare CC
    /// blocked-stop continuation). The runtime shares ONE handle with
    /// `CodexSource` (whose multi-turn children are the motivating case);
    /// `None` disables the side-channel (bare test construction).
    pub child_end_unclaims: Option<ChildEndUnclaims>,
}

/// Resolve `CLAUDE_CONFIG_DIR` (an empty value is treated as unset). `pub` +
/// `#[doc(hidden)]` so the `pixtuoid` install crate's settings.json resolver
/// shares this one definition — the two CC path sites must not drift. Internal
/// cross-crate helper, not a stable API.
#[doc(hidden)]
pub fn claude_config_dir() -> Option<PathBuf> {
    std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

impl ClaudeCodeSource {
    pub fn default_socket_path() -> PathBuf {
        if let Ok(p) = std::env::var("PIXTUOID_SOCKET") {
            // Set-but-empty/whitespace = unset (the #172 RUST_LOG policy):
            // honored verbatim, "" makes bind() fail fatally — killing the
            // whole CC source — for a trivially recoverable misconfiguration.
            // Same shape as pixtuoid-hook's paths.rs (parity-pinned by
            // tests/socket_path_parity.rs).
            if !p.trim().is_empty() {
                return PathBuf::from(p);
            }
        }
        #[cfg(unix)]
        {
            if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
                return PathBuf::from(format!("{dir}/pixtuoid.sock"));
            }
            // SAFETY: getuid() is a trivial syscall with no pointer args; cannot fail.
            let uid = unsafe { libc::getuid() };
            PathBuf::from(format!("/tmp/pixtuoid-{uid}.sock"))
        }
        #[cfg(windows)]
        {
            // Mirrors pixtuoid-hook/src/paths.rs — parity-pinned by
            // tests/socket_path_parity.rs, not shared (no dep edge between
            // shim and core).
            let user = std::env::var("USERNAME")
                .unwrap_or_else(|_| "default".into())
                .replace('\\', "-");
            PathBuf::from(format!(r"\\.\pipe\pixtuoid-{user}"))
        }
    }

    pub fn default_paths() -> Self {
        let projects_root = claude_config_dir()
            .unwrap_or_else(|| PathBuf::from(crate::platform::user_home()).join(".claude"))
            .join("projects");
        Self {
            socket_path: Self::default_socket_path(),
            projects_root,
            child_end_unclaims: None,
        }
    }
}

/// Producer half of the #246 child-end un-claim side-channel (see
/// `ChildEndUnclaims` for the WHY). Interposed between the hook listener and
/// the real channel inside [`ClaudeCodeSource::run`] — the listener's API
/// stays source-agnostic; this is source-local plumbing at the ONE seam every
/// decoded `SubagentStop` (CC and Codex alike — all sources' hooks ride this
/// source's shared socket) passes through. Every event is forwarded UNCHANGED,
/// transport tag included (invariant #2: the producer's tag flows through).
/// The push happens BEFORE the forward — the order is irrelevant for
/// correctness (the watcher drains on its own scan cadence), but push-first
/// means the un-claim is already pending by the time the reducer applies the
/// end, which keeps tests deterministic. Exits when either side closes
/// (listener gone → `recv` None; reducer gone → send Err).
async fn tee_child_end_unclaims(
    mut rx: TaggedReceiver,
    tx: TaggedSender,
    unclaims: ChildEndUnclaims,
) {
    while let Some((transport, ev)) = rx.recv().await {
        if transport == Transport::Hook {
            if let AgentEvent::SessionEnd {
                agent_id,
                as_child: true,
            } = &ev
            {
                unclaims.push(*agent_id);
            }
        }
        if tx.send((transport, ev)).await.is_err() {
            return;
        }
    }
}

impl Source for ClaudeCodeSource {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        // SocketBusy (another live instance owns the endpoint) must not take
        // the whole source down: the JSONL watcher works fine concurrently,
        // so this instance degrades to transcript-only — hooks (and the
        // hook-only Reasonix source riding the same socket) stay with the
        // owning instance. Every other bind error is fatal as before.
        let socket = match HookSocketListener::bind(self.socket_path.clone()).await {
            Ok(s) => Some(s),
            Err(e) if e.downcast_ref::<super::hook::SocketBusy>().is_some() => {
                tracing::warn!(
                    "{e:#}; continuing transcript-only — hook-borne signals \
                     (permission Waiting, instant lifecycle) belong to the owning instance"
                );
                None
            }
            Err(e) => return Err(e),
        };
        let mut watcher = JsonlWatcher::new(
            self.projects_root.clone(),
            SOURCE_NAME.to_string(),
            decode_cc_line,
            cc_derive_label,
            cc_session_ended,
        )
        .with_id_deriver(cc_id_from_path);
        if let Some(sessions_dir) = cc_sessions_dir(&self.projects_root) {
            watcher = watcher.with_liveness_probe(std::sync::Arc::new(move || {
                live_cc_session_ids(&sessions_dir)
            }));
        }
        if let Some(unclaims) = &self.child_end_unclaims {
            watcher = watcher.with_child_end_unclaims(unclaims.clone());
        }

        let Some(socket) = socket else {
            return watcher.run(tx).await;
        };
        // #246: route hook events through the un-claim tee when the
        // side-channel is wired (the runtime always wires it; `None` is bare
        // test construction). The tee task is a passive pipe — not part of
        // the select! below — and dies with the listener (its sender drops).
        let tx_hook = match &self.child_end_unclaims {
            Some(unclaims) => {
                // Same capacity as the runtime's event channel: the tee adds
                // a stage, not a different backpressure policy.
                let (tee_tx, tee_rx) = tokio::sync::mpsc::channel(256);
                tokio::spawn(tee_child_end_unclaims(tee_rx, tx.clone(), unclaims.clone()));
                tee_tx
            }
            None => tx.clone(),
        };
        let tx_jsonl = tx.clone();
        let hook_task = tokio::spawn(async move { socket.run(tx_hook).await });
        let jsonl_task = tokio::spawn(async move { watcher.run(tx_jsonl).await });

        let hook_abort = hook_task.abort_handle();
        let jsonl_abort = jsonl_task.abort_handle();

        let inner: Result<()> = tokio::select! {
            r = hook_task => {
                tracing::warn!("hook listener exited first; aborting jsonl watcher");
                jsonl_abort.abort();
                r?
            }
            r = jsonl_task => {
                tracing::warn!("jsonl watcher exited first; aborting hook listener");
                hook_abort.abort();
                r?
            }
        };
        inner
    }
}

/// Decode one CC JSONL transcript line into 0..N AgentEvents.
pub fn decode_cc_line(transcript_path: &str, source: &str, v: Value) -> Result<Vec<AgentEvent>> {
    // Key on the session UUID (filename stem), NOT the raw path — matches the
    // hook decoder's `IdKey::SessionId` and the watcher's `cc_id_from_path`
    // deriver, so all four CC keying sites coalesce (mirrors Codex).
    let agent_id = AgentId::from_parts(source, &cc_id_from_path(Path::new(transcript_path)));
    let Some(obj) = v.as_object() else {
        return Ok(vec![]);
    };

    let mut out = Vec::new();
    let ty = obj.get("type").and_then(|s| s.as_str()).unwrap_or("");

    // `.filter(non-empty)`: an empty `attributionAgent` would emit `Rename {
    // label: "" }`, blanking a good hook-derived label with no recovery until the
    // next Rename — same empty-string guard as the decoder's id fields.
    if let Some(name) = obj
        .get("attributionAgent")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        let label = name.rsplit(':').next().unwrap_or(name).to_string();
        out.push(AgentEvent::Rename { agent_id, label });
    }

    let Some(message) = obj.get("message").and_then(|m| m.as_object()) else {
        return Ok(out);
    };
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
                let name = bobj.get("name").and_then(|s| s.as_str()).unwrap_or("?");
                out.push(AgentEvent::ActivityStart {
                    agent_id,
                    tool_use_id: id,
                    detail: Some(make_tool_detail(name, bobj.get("input"))),
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
        // No content arm: user-message content is user-controllable and must
        // never drive session lifecycle (a message QUOTING the slash-command
        // wrapper would false-positive), and modern CC persists no /exit
        // marker in the transcript anyway. Lifecycle = the SessionEnd hook +
        // the idle sweep.
        _ => {}
    }
    Ok(out)
}

/// CC session-end checker: parses lines as JSON and checks for
/// session lifecycle markers structurally (not byte scan).
pub fn cc_session_ended(tail: &[u8]) -> bool {
    let mut last_is_end = false;
    for line in tail.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(s) = std::str::from_utf8(line) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(s) else {
            continue;
        };
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
        // Only STRUCTURAL markers count. Message content is user-controllable
        // and must never drive lifecycle (quoting the slash-command wrapper
        // would false-positive), and modern CC persists no /exit marker at
        // all — a session whose end hook dropped is reaped by the idle sweep.
    }
    last_is_end
}

/// CC label: subagent paths → "subagent", otherwise "cc·" + cwd basename.
///
/// When `cwd` is unknown (a seed line that carries no `cwd` — the JSONL Rename
/// can fire on such a line), fall back to the CC **project dir** instead of a
/// bare "cc": the project dir name encodes the cwd path with '/'→'-', so its
/// last segment is the project basename. Without this, an empty-cwd Rename
/// silently degrades a good hook-derived `cc·dotfiles` back to `cc`.
pub fn cc_derive_label(path: &Path, _source: &str, cwd: &Path) -> String {
    // ONE shared predicate with `detect_parent_id` (both via the `SUBAGENTS_DIR` component)
    // so the two can't diverge — a loose `"subagents"` substring once mislabeled a
    // `subagents-paper` repo's parent transcript "subagent" with parent_id=None
    // (bug_004); the slash-bounded predicate fixes that at a single source.
    if crate::source::jsonl::is_subagent_path(path) {
        return "subagent".to_string();
    }
    if let Some(label) = cwd_basename_label("cc", cwd) {
        return label;
    }
    if let Some(base) = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .and_then(|proj| proj.rsplit('-').find(|s| !s.is_empty()))
    {
        return format!("cc·{base}");
    }
    "cc".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    /// The #246 tee contract: a Hook-transport `SessionEnd { as_child: true }`
    /// flowing through the forwarding task lands its id in the shared handle,
    /// AND every event — that one included — reaches the downstream channel
    /// UNCHANGED, in order, transport tag intact (invariant #2). Jsonl-tagged
    /// child ends and root (`as_child: false`) hook ends must NOT be pushed:
    /// only the SubagentStop decode shape feeds the un-claim.
    #[tokio::test]
    async fn tee_pushes_hook_child_ends_and_forwards_every_event_unchanged() {
        let unclaims = ChildEndUnclaims::new();
        let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
        let tee = tokio::spawn(tee_child_end_unclaims(in_rx, out_tx, unclaims.clone()));

        let child = AgentId::from_parts("codex", "child-uuid");
        let root = AgentId::from_parts(SOURCE_NAME, "root-uuid");
        let events: Vec<(Transport, AgentEvent)> = vec![
            (
                Transport::Hook,
                AgentEvent::ActivityStart {
                    agent_id: root,
                    tool_use_id: Some("tu_1".into()),
                    detail: None,
                },
            ),
            // A JSONL-tagged child end must not feed the handle (nothing
            // in-tree emits one; the guard IS the boundary).
            (
                Transport::Jsonl,
                AgentEvent::SessionEnd {
                    agent_id: root,
                    as_child: true,
                },
            ),
            // A root hook end is not a SubagentStop — not pushed.
            (
                Transport::Hook,
                AgentEvent::SessionEnd {
                    agent_id: root,
                    as_child: false,
                },
            ),
            // THE shape: the decoded SubagentStop.
            (
                Transport::Hook,
                AgentEvent::SessionEnd {
                    agent_id: child,
                    as_child: true,
                },
            ),
        ];
        for ev in &events {
            in_tx.send(ev.clone()).await.unwrap();
        }
        for expected in &events {
            let got = tokio::time::timeout(Duration::from_secs(5), out_rx.recv())
                .await
                .expect("tee must forward promptly")
                .expect("tee must not drop the channel");
            assert_eq!(
                &got, expected,
                "event parity: forwarded unchanged, in order"
            );
        }
        assert_eq!(
            unclaims.take_matching(|_| true),
            vec![child],
            "exactly the Hook-transport as_child end lands in the handle"
        );
        drop(in_tx);
        tokio::time::timeout(Duration::from_secs(5), tee)
            .await
            .expect("tee exits when the listener side closes")
            .unwrap();
    }

    // The custom-decoder contract (mirrors the codex twin): claim our two
    // events FULLY — a malformed instance must be Err, never Ok(None) (which
    // would silently fall through to the shared session-keyed arms). Happy
    // paths are pinned end-to-end in tests/sources/decode/mod.rs and against
    // the captured fixture in tests/sources/claude/mod.rs.
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
    fn non_subagent_events_fall_through_to_shared_arms() {
        let start = json!({"hook_event_name": "SessionStart", "session_id": "s"});
        assert!(matches!(decode_cc_hook_custom(&start), Ok(None)));
        // Non-object payload: defensive fall-through — the dispatcher
        // pre-validates object-ness, so the shared path owns the error.
        assert!(matches!(decode_cc_hook_custom(&json!("nope")), Ok(None)));
    }

    // A present agent_transcript_path whose stem is EMPTY (degenerate path)
    // must fall back to the prefixed wire id, never mint AgentId("").
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
        // Regression: an empty-cwd Rename must not degrade `cc·dotfiles` to `cc`.
        let path = Path::new("/Users/me/.claude/projects/-Users-me-dotfiles/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("")),
            "cc·dotfiles"
        );
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
        // A parent transcript for a repo named `subagents-paper` encodes to a
        // project dir containing the substring "subagents" but no `/subagents/`
        // segment — it must NOT be mislabeled "subagent".
        let path = Path::new("/Users/me/.claude/projects/-Users-me-subagents-paper/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("/Users/me/subagents-paper")),
            "cc·subagents-paper"
        );
    }

    #[test]
    fn label_uses_project_dir_when_cwd_is_root() {
        // cwd = "/" fails the non-empty/non-root guard → falls to the project-dir
        // branch rather than the cwd basename.
        let path = Path::new("/Users/me/.claude/projects/-Users-me-dotfiles/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("/")),
            "cc·dotfiles"
        );
    }

    #[test]
    fn label_uses_project_dir_when_cwd_has_no_basename() {
        // A non-empty, non-root cwd whose file_name() is None (e.g. "..") enters
        // the cwd block but can't return → falls through to the project-dir branch.
        let path = Path::new("/Users/me/.claude/projects/-Users-me-dotfiles/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("..")),
            "cc·dotfiles"
        );
    }

    #[test]
    fn label_final_fallback_to_cc_when_no_project_dir() {
        // Degenerate path with no parent dir to decode AND empty cwd → bare "cc".
        assert_eq!(
            cc_derive_label(Path::new("abc.jsonl"), "claude-code", Path::new("")),
            "cc"
        );
    }

    // The socket-path and default-paths env precedence. All three socket
    // branches are checked in ONE test because the env vars are process-global —
    // splitting across tests would race under the default multi-thread runner.
    // Unix-specific branches (XDG_RUNTIME_DIR + getuid fallback) can only be
    // asserted on Unix; the platform-neutral default_paths check is split into
    // a separate test so it compiles + runs on all platforms.
    #[cfg(unix)]
    #[test]
    fn default_socket_path_env_precedence_and_default_paths() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved_socket = std::env::var_os("PIXTUOID_SOCKET");
        let saved_xdg = std::env::var_os("XDG_RUNTIME_DIR");

        // PIXTUOID_SOCKET takes precedence (checked first).
        std::env::set_var("PIXTUOID_SOCKET", "/tmp/explicit.sock");
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        assert_eq!(
            ClaudeCodeSource::default_socket_path(),
            PathBuf::from("/tmp/explicit.sock")
        );

        // Set-but-empty/whitespace PIXTUOID_SOCKET = unset (the #172 RUST_LOG
        // policy): falls through to XDG instead of binding an empty path.
        std::env::set_var("PIXTUOID_SOCKET", "");
        assert_eq!(
            ClaudeCodeSource::default_socket_path(),
            PathBuf::from("/run/user/1000/pixtuoid.sock")
        );
        std::env::set_var("PIXTUOID_SOCKET", "   ");
        assert_eq!(
            ClaudeCodeSource::default_socket_path(),
            PathBuf::from("/run/user/1000/pixtuoid.sock")
        );

        // Without PIXTUOID_SOCKET, XDG_RUNTIME_DIR drives the path.
        std::env::remove_var("PIXTUOID_SOCKET");
        assert_eq!(
            ClaudeCodeSource::default_socket_path(),
            PathBuf::from("/run/user/1000/pixtuoid.sock")
        );

        // With neither set, fall back to the uid-suffixed /tmp socket.
        std::env::remove_var("XDG_RUNTIME_DIR");
        // SAFETY: getuid() is a trivial argless syscall.
        let uid = unsafe { libc::getuid() };
        assert_eq!(
            ClaudeCodeSource::default_socket_path(),
            PathBuf::from(format!("/tmp/pixtuoid-{uid}.sock"))
        );

        // Restore prior env so a later env-reading test in this binary isn't
        // poisoned by the cleared state.
        match saved_socket {
            Some(v) => std::env::set_var("PIXTUOID_SOCKET", v),
            None => std::env::remove_var("PIXTUOID_SOCKET"),
        }
        match saved_xdg {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
    }

    #[test]
    fn default_paths_projects_root_honors_claude_config_dir() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved_config = std::env::var_os("CLAUDE_CONFIG_DIR");
        let fallback_suffix = PathBuf::from(".claude").join("projects");

        std::env::remove_var("CLAUDE_CONFIG_DIR");
        let unset_paths = ClaudeCodeSource::default_paths();
        assert!(
            unset_paths.projects_root.ends_with(&fallback_suffix),
            "projects_root must end with .claude/projects, got {:?}",
            unset_paths.projects_root
        );

        let custom_dir = std::env::temp_dir().join("pixtuoid-claude-config-dir");
        std::env::set_var("CLAUDE_CONFIG_DIR", &custom_dir);
        assert_eq!(
            ClaudeCodeSource::default_paths().projects_root,
            custom_dir.join("projects")
        );

        std::env::set_var("CLAUDE_CONFIG_DIR", "");
        let empty_paths = ClaudeCodeSource::default_paths();
        assert!(
            empty_paths.projects_root.ends_with(&fallback_suffix),
            "empty CLAUDE_CONFIG_DIR must fall back to .claude/projects, got {:?}",
            empty_paths.projects_root
        );

        match saved_config {
            Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
            None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
        }
    }

    // CC on Windows slugs an absolute path like `C:\Users\foo\bar` into a project
    // dir name using `[^a-zA-Z0-9]→'-'` (regex from upstream CC source, drive
    // letter kept, no leading dash): `C--Users-foo-bar`. The fallback path in
    // `cc_derive_label` (empty cwd → rsplit on '-' → last non-empty segment)
    // must extract the project-basename `bar` and produce `cc·bar`. Verified
    // against upstream CC; real hook-payload fixture lands post-tester (PR 5).
    #[test]
    fn label_falls_back_to_project_dir_for_windows_slug() {
        // Windows slug: C:\Users\foo\bar  →  C--Users-foo-bar
        let path = Path::new("/Users/me/.claude/projects/C--Users-foo-bar/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("")),
            "cc·bar"
        );
    }

    // CC writes `message.content` as a plain STRING (not a block array) for
    // simple text turns — 4709 such lines in a local 2379-session / 822 MB
    // corpus. The tool-event match only fires on `Value::Array`, so a
    // string-content turn must decode to NOTHING (no events, no panic) — even
    // a slash-command wrapper line: content never drives lifecycle. A fuzz of
    // all 291k real lines through decode_cc_line confirmed zero panics; this
    // pins the common string-content shape the array-only fixtures never
    // exercise.
    // Coalescing guard: `cc_id_from_path` is invoked in multiple places that
    // must agree — the per-line decode (here), the watcher's `with_id_deriver`
    // (ClaudeCodeSource::run), and the hook decoder's session-id key. If the
    // per-line decode ever keys differently from the deriver, one CC session
    // splits into two sprites. Mirrors codex's
    // `decode_line_keys_agent_id_on_codex_id_from_path`.
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

    // Lifecycle must never read chat content: a user message QUOTING the CC
    // slash-command wrapper mid-prose (common in sessions discussing CC
    // internals) is user-controllable text, not a lifecycle signal. Neither
    // the live decode nor the tail scan may treat it as a session end.
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
        // Even an exact slash-command wrapper decodes to nothing — the old
        // content-based /exit → SessionEnd matcher is gone (zero true
        // positives in a 135-transcript corpus; lifecycle is hooks + sweep).
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
        // The first-sight deriver gets a raw &Path; the per-line decoder gets the
        // normalize_path_key'd string (lowercased + forward-slashed on Windows).
        // Both must yield the SAME stem for a lowercase-hex CC id, or one session
        // splits into two sprites (same assumption codex_id_from_path relies on).
        let raw =
            Path::new("/Users/me/.claude/projects/p/01000000-0000-7000-8000-0000000000cc.jsonl");
        let normalized =
            Path::new("/users/me/.claude/projects/p/01000000-0000-7000-8000-0000000000cc.jsonl");
        assert_eq!(cc_id_from_path(raw), cc_id_from_path(normalized));
    }
}
