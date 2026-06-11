use std::io::Write;

use super::health::FailureLatch;
use super::liveness::{emit_session_exit, unbind_session};
use super::unclaim::drain_child_end_unclaims;
use super::walk::{detect_parent_id, extract_cwd, scan_root, walk_jsonl, TASK_SCAN_BYTES};
use super::*;
use crate::source::Transport;
use crate::AgentId;

#[test]
fn unbind_session_drops_only_emptied_pid_entries() {
    let mut bindings: HashMap<i32, HashSet<String>> = HashMap::new();
    bindings
        .entry(100)
        .or_default()
        .extend(["a".to_string(), "b".to_string()]);
    bindings.entry(200).or_default().insert("a".to_string());
    unbind_session(&mut bindings, "a");
    // pid 200 emptied → dropped; pid 100 keeps its other session.
    assert!(!bindings.contains_key(&200));
    assert_eq!(
        bindings.get(&100).map(|ids| ids.len()),
        Some(1),
        "the sibling id on a shared pid must survive the unbind"
    );
}

#[test]
fn default_id_from_path_returns_normalized_path_key() {
    // Lowercase literal: identity on every platform (the Windows fold is
    // pinned by decoder.rs's normalize_path_key unit tests + the backslash
    // test below).
    let p = Path::new("/users/me/.claude/projects/x/abc.jsonl");
    assert_eq!(
        default_id_from_path(p),
        "/users/me/.claude/projects/x/abc.jsonl"
    );
}

// These call the REAL detect_parent_id/is_subagent_path (they're private —
// an integration test can't reach them; an old decoder.rs test re-simulated
// the algorithm inline and silently pinned the superseded string-scan).
#[test]
fn detect_parent_id_derives_grandparent_transcript_key() {
    // THE contract: the derived parent_id keys on the `<parent-uuid>`
    // component (the dir immediately before `subagents`), which equals the
    // parent's own id (`cc_id_from_path` of `<parent-uuid>.jsonl`). The
    // project-dir prefix is cwd-derived and intentionally NOT part of the
    // key, so the link survives a git-worktree cwd-split.
    let parent: PathBuf = ["projects", "x", "abc123"].iter().collect();
    let p = parent.join("subagents").join("agent-1.jsonl");
    let expected = AgentId::from_parts("claude-code", "abc123");
    assert_eq!(detect_parent_id(&p, "claude-code"), Some(expected));
    assert!(is_subagent_path(&p));
}

#[test]
fn detect_parent_id_none_for_regular_and_lookalike_paths() {
    assert_eq!(
        detect_parent_id(
            Path::new("/Users/me/.claude/projects/x/ses.jsonl"),
            "claude-code"
        ),
        None
    );
    // Component matching: a dir merely CONTAINING the word never matches.
    let lookalike = Path::new("/Users/me/.claude/projects/subagents-paper/ses.jsonl");
    assert_eq!(detect_parent_id(lookalike, "claude-code"), None);
    assert!(!is_subagent_path(lookalike));
    // A bare relative path starting AT `subagents` has no parent to derive.
    assert_eq!(
        detect_parent_id(Path::new("subagents/agent-1.jsonl"), "claude-code"),
        None
    );
}

#[test]
fn detect_parent_id_keys_on_parent_uuid_component() {
    let sub = Path::new("/Users/me/.claude/projects/-Users-me-proj/abc123/subagents/agent-1.jsonl");
    let expected = AgentId::from_parts("claude-code", "abc123");
    assert_eq!(detect_parent_id(sub, "claude-code"), Some(expected));
}

#[test]
fn detect_parent_id_survives_cwd_split() {
    // THE bug: parent + subagent under DIFFERENT project dirs (a worktree
    // cwd-split). Only the project-dir component differs; the <parent-uuid>
    // component is identical, so BOTH must resolve to the same parent link.
    let under_a = Path::new("/Users/me/.claude/projects/-PROJECT-A/abc123/subagents/agent-1.jsonl");
    let under_b = Path::new("/Users/me/.claude/projects/-PROJECT-B/abc123/subagents/agent-1.jsonl");
    let expected = AgentId::from_parts("claude-code", "abc123");
    assert_eq!(detect_parent_id(under_a, "claude-code"), Some(expected));
    assert_eq!(detect_parent_id(under_b, "claude-code"), Some(expected));
    assert_eq!(
        detect_parent_id(under_a, "claude-code"),
        detect_parent_id(under_b, "claude-code"),
        "same <parent-uuid> under different project dirs resolves to the same parent"
    );
}

#[test]
fn detect_parent_id_handles_workflow_nesting() {
    let sub =
        Path::new("/Users/me/.claude/projects/p/abc123/subagents/workflows/wf_0d/agent-y.jsonl");
    let expected = AgentId::from_parts("claude-code", "abc123");
    assert_eq!(detect_parent_id(sub, "claude-code"), Some(expected));
}

// Only RUNS on the windows-test CI job (backslashes are ordinary filename
// bytes on Unix, so this shape is only meaningful there) — pins the
// components rewrite's whole reason to exist.
#[cfg(windows)]
#[test]
fn detect_parent_id_handles_backslash_paths() {
    // Backslash separators are split into ordinary components on Windows, so
    // the `<parent-uuid>` component (`abc123`) before `subagents` is
    // extracted just as it is on Unix — pins the component-walk reason to
    // exist. CC session UUIDs are lowercase, so no casefold is needed on the
    // key (mirrors `cc_id_from_path`).
    let p = Path::new(r"C:\Users\Me\.claude\projects\x\abc123\subagents\agent-1.jsonl");
    let expected = AgentId::from_parts("claude-code", "abc123");
    assert_eq!(detect_parent_id(p, "claude-code"), Some(expected));
    assert!(is_subagent_path(p));
}

#[test]
fn extract_cwd_reads_top_level_and_nested_payload() {
    // CC/AG shape: top-level cwd.
    let top = br#"{"cwd":"/repo/a"}"#;
    assert_eq!(extract_cwd(top), Some(PathBuf::from("/repo/a")));
    // Codex shape: cwd nested under payload (session_meta).
    let nested = br#"{"type":"session_meta","payload":{"cwd":"/repo/b","id":"u"}}"#;
    assert_eq!(extract_cwd(nested), Some(PathBuf::from("/repo/b")));
}

fn t_decode(_t: &str, _s: &str, _v: serde_json::Value) -> Result<Vec<AgentEvent>> {
    Ok(vec![])
}
/// Minimal lifecycle decoder: a structural `session_end` line decodes to
/// `SessionEnd` keyed exactly like the harness's default `id_derive`
/// (`transcript_path` == `default_id_from_path(path)` here), mirroring how
/// the real CC pair (`decode_cc_line` + `cc_id_from_path`) agrees.
fn t_decode_lifecycle(t: &str, s: &str, v: serde_json::Value) -> Result<Vec<AgentEvent>> {
    if v.get("subtype").and_then(|x| x.as_str()) == Some("session_end") {
        return Ok(vec![AgentEvent::SessionEnd {
            agent_id: AgentId::from_parts(s, t),
            as_child: false,
        }]);
    }
    Ok(vec![])
}
fn t_label(_p: &Path, _s: &str, _c: &Path) -> String {
    "t".to_string()
}
fn t_ended(buf: &[u8]) -> bool {
    std::str::from_utf8(buf).is_ok_and(|s| s.contains("session_end"))
}

/// Drive `walk_jsonl` once over `path` against caller-owned cursor/seen
/// maps, so multi-pass scenarios (gate → append → revive) share state the
/// way the real watch loop does. Returns the emitted events.
async fn walk_once_with(
    path: &Path,
    window: Duration,
    decode_line: LineDecoder,
    check_ended: SessionEndChecker,
    cursors: &Arc<Mutex<HashMap<PathBuf, u64>>>,
    seen: &Arc<Mutex<HashMap<PathBuf, bool>>>,
) -> Vec<(Transport, AgentEvent)> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(32);
    let source: Arc<str> = Arc::from("test");
    let decoders = SourceDecoders {
        decode_line,
        derive_label: t_label,
        check_ended,
        id_derive: default_id_from_path,
    };
    let live = Arc::new(Mutex::new(HashSet::new()));
    let ctx = WatchCtx {
        source: &source,
        cursors,
        seen,
        tx: &tx,
        window,
        live: &live,
    };
    walk_jsonl(path, decoders, &ctx).await;
    drop(tx);
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    events
}

/// `walk_once` against a NON-EMPTY liveness snapshot, using the CC stem
/// deriver (`cc_id_from_path`) — the id-space the real probe joins on
/// (the registry carries session UUIDs; transcripts are `<uuid>.jsonl`).
async fn walk_once_live(
    path: &Path,
    window: Duration,
    live_ids: &[&str],
    cursors: &Arc<Mutex<HashMap<PathBuf, u64>>>,
    seen: &Arc<Mutex<HashMap<PathBuf, bool>>>,
) -> Vec<(Transport, AgentEvent)> {
    let live: Arc<Mutex<HashSet<String>>> =
        Arc::new(Mutex::new(live_ids.iter().map(|s| s.to_string()).collect()));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(32);
    let source: Arc<str> = Arc::from("test");
    let decoders = SourceDecoders {
        decode_line: t_decode,
        derive_label: t_label,
        check_ended: t_ended,
        id_derive: crate::source::claude_code::cc_id_from_path,
    };
    let ctx = WatchCtx {
        source: &source,
        cursors,
        seen,
        tx: &tx,
        window,
        live: &live,
    };
    walk_jsonl(path, decoders, &ctx).await;
    drop(tx);
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    events
}

fn backdate_one_hour(path: &Path) {
    filetime::set_file_mtime(
        path,
        filetime::FileTime::from_system_time(
            std::time::SystemTime::now() - Duration::from_secs(3600),
        ),
    )
    .unwrap();
}

/// `walk_once_with` with the no-op decoder — the common case for tests
/// that exercise the gate / cursor / registration paths, not decoding.
async fn walk_once(
    path: &Path,
    window: Duration,
    check_ended: SessionEndChecker,
    cursors: &Arc<Mutex<HashMap<PathBuf, u64>>>,
    seen: &Arc<Mutex<HashMap<PathBuf, bool>>>,
) -> Vec<(Transport, AgentEvent)> {
    walk_once_with(path, window, t_decode, check_ended, cursors, seen).await
}

/// Drive `walk_jsonl` once over a fresh (never-seeded) file — the
/// deterministic, timing-free repro of the #85 race. When the watcher's
/// `walk_jsonl` (rescan / 60s poll / notify) is the FIRST to see a file,
/// does it gate (ended/stale) or resurrect it? Returns the emitted events +
/// the cursor it left.
async fn first_sight_walk(
    path: &Path,
    window: Duration,
    check_ended: SessionEndChecker,
) -> (Vec<(Transport, AgentEvent)>, Option<u64>) {
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let events = walk_once(path, window, check_ended, &cursors, &seen).await;
    let cursor = cursors.lock().await.get(path).copied();
    (events, cursor)
}

/// Build the G2 fixture: a file GATED at first sight (old mtime → cursor
/// seeded at EOF, `seen` unclaimed), returning the shared maps for the
/// follow-up walk.
async fn gated_fixture(
    path: &Path,
    initial: &str,
) -> (
    Arc<Mutex<HashMap<PathBuf, u64>>>,
    Arc<Mutex<HashMap<PathBuf, bool>>>,
) {
    tokio::fs::write(path, initial).await.unwrap();
    backdate_one_hour(path);
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let gated = walk_once(path, Duration::from_secs(60), t_ended, &cursors, &seen).await;
    assert!(
        gated.is_empty(),
        "stale first sight must gate silently, got {gated:?}"
    );
    assert!(
        !seen.lock().await.contains_key(path),
        "a gated file must not claim `seen`"
    );
    (cursors, seen)
}

#[tokio::test]
async fn gated_file_registers_on_oversized_first_append() {
    // G2: a file gated at first sight (cursor at EOF, never registered)
    // then appends > MAX_PENDING_BYTES in one burst. The oversized branch
    // used to key registration on `!known`, but a gated file IS known —
    // the agent stayed invisible until a later ≤1 MiB append.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gated-big.jsonl");
    let initial = "{\"type\":\"assistant\",\"cwd\":\"/repo/head\"}\n";
    let (cursors, seen) = gated_fixture(&path, initial).await;

    let mut full = String::from(initial);
    full.push_str(&"{\"type\":\"assistant\"}\n".repeat(60_000));
    tokio::fs::write(&path, &full).await.unwrap();
    assert!(
        (full.len() - initial.len()) as u64 > (1 << 20),
        "the appended span must exceed MAX_PENDING_BYTES"
    );

    let events = walk_once(&path, Duration::from_secs(60), t_ended, &cursors, &seen).await;
    let expected = AgentId::from_parts("test", &default_id_from_path(&path));
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            AgentEvent::SessionStart { agent_id, .. } if *agent_id == expected
        )),
        "a gated file's oversized first append must register the agent, got {events:?}"
    );
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(full.len() as u64),
        "cursor must advance to EOF"
    );
}

#[tokio::test]
async fn gated_file_oversized_ended_append_stays_unregistered() {
    // Same shape as above, but the burst ENDS the session: registering
    // would emit SessionStart AFTER the buried SessionEnd and resurrect a
    // ghost slot. The terminator must still be emitted; registration must
    // not.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gated-big-ended.jsonl");
    let initial = "{\"type\":\"assistant\",\"cwd\":\"/repo/head\"}\n";
    let (cursors, seen) = gated_fixture(&path, initial).await;

    let mut full = String::from(initial);
    full.push_str(&"{\"type\":\"assistant\"}\n".repeat(60_000));
    full.push_str("{\"type\":\"system\",\"subtype\":\"session_end\"}\n");
    tokio::fs::write(&path, &full).await.unwrap();

    let events = walk_once(&path, Duration::from_secs(60), t_ended, &cursors, &seen).await;
    let expected = AgentId::from_parts("test", &default_id_from_path(&path));
    assert!(
        events.iter().any(
            |(_, e)| matches!(e, AgentEvent::SessionEnd { agent_id, as_child: false } if *agent_id == expected)
        ),
        "the buried terminator must still emit SessionEnd, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
        "an ended oversized span must not register a ghost, got {events:?}"
    );
}

#[tokio::test]
async fn session_end_unclaims_seen_so_a_later_append_re_registers() {
    // Self-heal layer: once a decoded line yields SessionEnd for this
    // path's agent, the path must be UN-claimed from `seen` so a LATER
    // append re-registers through the documented emit_first_sight revive.
    // Today `seen` stays claimed forever — the agent can never re-register
    // without a watcher restart.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resumed.jsonl");
    tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/repo\"}\n")
        .await
        .unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    // Pass 1: first-sight registration.
    let window = Duration::from_secs(3600);
    let events = walk_once_with(&path, window, t_decode_lifecycle, t_ended, &cursors, &seen).await;
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
        "live first sight must register, got {events:?}"
    );

    // Pass 2: a structural session_end line decodes to SessionEnd.
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .await
        .unwrap();
    tokio::io::AsyncWriteExt::write_all(
        &mut f,
        b"{\"type\":\"system\",\"subtype\":\"session_end\"}\n",
    )
    .await
    .unwrap();
    tokio::io::AsyncWriteExt::flush(&mut f).await.unwrap();
    drop(f);
    let events = walk_once_with(&path, window, t_decode_lifecycle, t_ended, &cursors, &seen).await;
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionEnd { .. })),
        "the structural end must decode to SessionEnd, got {events:?}"
    );
    assert!(
        !seen.lock().await.contains_key(&path),
        "SessionEnd must un-claim `seen` so a revival can re-register"
    );

    // Pass 3: the session resumes (normal lines again) — a SECOND
    // SessionStart must be emitted via the revive path.
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .await
        .unwrap();
    tokio::io::AsyncWriteExt::write_all(&mut f, b"{\"type\":\"assistant\"}\n")
        .await
        .unwrap();
    tokio::io::AsyncWriteExt::flush(&mut f).await.unwrap();
    drop(f);
    let events = walk_once_with(&path, window, t_decode_lifecycle, t_ended, &cursors, &seen).await;
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
        "a post-end append must re-register the agent, got {events:?}"
    );
}

/// The instant-exit ↔ pre-death-write race (#223 review finding): a write
/// landing just before the process dies can have its notify event delivered
/// AFTER the exit arm runs. `emit_session_exit` must drain those pending
/// bytes (cursor → EOF) BEFORE un-claiming `seen`, or the straggler walk
/// re-enters as a first-sight and resurrects the dead session as a ghost —
/// with every fast rung already disarmed for it.
#[tokio::test]
async fn session_exit_drains_pending_bytes_so_a_straggler_walk_cannot_resurrect() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    std::fs::write(&path, "{\"type\":\"assistant\"}\n").unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let window = Duration::from_secs(3600);

    // Register normally (recent file → SessionStart, cursor at EOF).
    let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
    assert!(events
        .iter()
        .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })));

    // The pre-death write: appended, but its notify walk has NOT run.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"type\":\"assistant\"}\n")
        .unwrap();
    let pre_exit_cursor = *cursors.lock().await.get(&path).unwrap();
    let file_len = std::fs::metadata(&path).unwrap().len();
    assert!(pre_exit_cursor < file_len, "fixture: bytes must be pending");

    // The instant exit fires (process died) before the notify event lands.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(32);
    let source: Arc<str> = Arc::from("test");
    let decoders = SourceDecoders {
        decode_line: t_decode,
        derive_label: t_label,
        check_ended: t_ended,
        id_derive: default_id_from_path,
    };
    let live = Arc::new(Mutex::new(HashSet::new()));
    let ctx = WatchCtx {
        source: &source,
        cursors: &cursors,
        seen: &seen,
        tx: &tx,
        window,
        live: &live,
    };
    let id = default_id_from_path(&path);
    emit_session_exit(&id, decoders, &ctx).await;
    drop(tx);
    let mut exit_events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        exit_events.push(ev);
    }
    assert!(
        matches!(
            exit_events.last(),
            Some((Transport::Jsonl, AgentEvent::SessionEnd { .. }))
        ),
        "the terminator must be emitted (last), got {exit_events:?}"
    );
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(file_len),
        "the exit must drain pending bytes to EOF before un-claiming"
    );
    assert!(
        !seen.lock().await.contains_key(&path),
        "seen must be un-claimed so a genuine post-death append revives"
    );

    // The straggler notify walk: must be a no-op, NOT a ghost first-sight.
    let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
    assert!(
        events.is_empty(),
        "a straggler walk after the exit must not resurrect, got {events:?}"
    );

    // A genuinely post-death append still revives — the self-heal contract.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"type\":\"assistant\"}\n")
        .unwrap();
    let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
        "a post-exit append must re-register, got {events:?}"
    );
}

#[tokio::test]
async fn session_exit_purges_live_so_a_probe_failure_pass_cannot_revouch() {
    // Instant-exit ghost: `live` is only rewritten by a HEALTHY probe
    // refresh, so after an instant exit a probe-FAILURE pass keeps the
    // stale snapshot vouching the dead id — `revouch_gated_files` would
    // re-admit the parked file (cursor reset to 0 → full replay → a
    // phantom SessionStart for the session whose SessionEnd just
    // emitted, with every fast rung already disarmed for it).
    // `emit_session_exit` must purge the id from the admission set.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    std::fs::write(&path, "{\"type\":\"assistant\"}\n").unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let window = Duration::from_secs(3600);

    let id = default_id_from_path(&path);
    // The last healthy snapshot vouched the id (that is how its pid got
    // bound in the first place).
    let live: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::from([id.clone()])));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(64);
    let source: Arc<str> = Arc::from("test");
    let decoders = SourceDecoders {
        decode_line: t_decode,
        derive_label: t_label,
        check_ended: t_ended,
        id_derive: default_id_from_path,
    };
    let ctx = WatchCtx {
        source: &source,
        cursors: &cursors,
        seen: &seen,
        tx: &tx,
        window,
        live: &live,
    };

    // Register normally, then the bound process dies (instant exit).
    walk_jsonl(&path, decoders, &ctx).await;
    emit_session_exit(&id, decoders, &ctx).await;
    assert!(
        !live.lock().await.contains(&id),
        "the exit must purge the dead id from the admission set"
    );
    while rx.try_recv().is_ok() {} // drain the registration + exit events

    // The next scan pass runs under a FAILING probe: `live` was NOT
    // refreshed. The re-vouch sweep + walk must not resurrect the
    // session the watcher itself just declared dead.
    let mut health = FailureLatch::default();
    scan_root(dir.path(), decoders, &ctx, &mut health).await;
    drop(tx);
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
        "a probe-failure pass after the instant exit must not mint a phantom SessionStart, got {events:?}"
    );
}

#[test]
fn failure_latch_fires_once_per_state_change() {
    let mut latch = FailureLatch::default();
    assert!(latch.on_failure(), "first failure after a success reports");
    assert!(!latch.on_failure(), "a persistent failure stays quiet");
    assert!(latch.on_success(), "recovery reports once");
    assert!(!latch.on_success(), "steady success stays quiet");
    assert!(
        latch.on_failure(),
        "a NEW failure after recovery reports again"
    );
}

/// The shared fixture pieces for the child-end un-claim tests (mirrors
/// the `emit_session_exit` harness shape): default-derived decoders + a
/// fresh tagged channel.
fn t_decoders() -> SourceDecoders {
    SourceDecoders {
        decode_line: t_decode,
        derive_label: t_label,
        check_ended: t_ended,
        id_derive: default_id_from_path,
    }
}

fn drain_events(
    rx: &mut tokio::sync::mpsc::Receiver<(Transport, AgentEvent)>,
) -> Vec<(Transport, AgentEvent)> {
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    events
}

/// #246: the child-end un-claim must run the SAME drain-before-unclaim
/// discipline as `emit_session_exit` (#228) — a pre-stop straggler's
/// pending bytes are walked to EOF BEFORE the claim is released, so the
/// straggler cannot re-register the just-ended child — and it must emit
/// NOTHING: no SessionEnd (the reducer already ended the slot from the
/// hook SubagentStop) and no registration from the drained bytes. A
/// genuinely NEW append afterwards re-registers — the revival the
/// side-channel exists for.
#[tokio::test]
async fn child_end_unclaim_drains_stragglers_then_releases_without_session_end() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("child.jsonl");
    std::fs::write(&path, "{\"type\":\"assistant\"}\n").unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let window = Duration::from_secs(3600);

    // Register normally (recent file → SessionStart, cursor at EOF).
    let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
    assert!(events
        .iter()
        .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })));

    // The pre-stop straggler: appended, but its notify walk has NOT run.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"type\":\"assistant\"}\n")
        .unwrap();
    let file_len = std::fs::metadata(&path).unwrap().len();
    assert!(
        *cursors.lock().await.get(&path).unwrap() < file_len,
        "fixture: bytes must be pending"
    );

    // The hook SubagentStop was decoded — the tee pushed the child id.
    let unclaims = ChildEndUnclaims::new();
    let id = AgentId::from_parts("test", &default_id_from_path(&path));
    unclaims.push(id);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(64);
    let source: Arc<str> = Arc::from("test");
    let live = Arc::new(Mutex::new(HashSet::new()));
    let ctx = WatchCtx {
        source: &source,
        cursors: &cursors,
        seen: &seen,
        tx: &tx,
        window,
        live: &live,
    };
    drain_child_end_unclaims(Some(&unclaims), t_decoders(), &ctx).await;
    let events = drain_events(&mut rx);
    assert!(
        events.is_empty(),
        "the un-claim emits NOTHING — no SessionEnd (the hook already \
         ended the slot), no straggler registration — got {events:?}"
    );
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(file_len),
        "stragglers must be drained to EOF BEFORE the release (#228)"
    );
    assert_eq!(
        seen.lock().await.get(&path),
        Some(&false),
        "the claim must be RELEASED (kept known, so the re-vouch sweep \
         cannot replay it)"
    );

    // The straggler notify walk: a no-op, not a ghost first-sight.
    let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
    assert!(
        events.is_empty(),
        "a straggler walk after the release must not resurrect, got {events:?}"
    );

    // Turn N+1: a fresh append re-registers the SAME id.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"type\":\"assistant\"}\n")
        .unwrap();
    let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            AgentEvent::SessionStart { agent_id, .. } if *agent_id == id
        )),
        "the turn-N+1 append must re-register the child, got {events:?}"
    );
}

/// The release keeps the path KNOWN (`seen` → false) instead of removing
/// it: a live multi-turn child's rollout stays OPEN in its codex process,
/// so the FD probe keeps vouching the id — with the claim fully removed,
/// `revouch_gated_files` would reset the cursor to 0 and the same pass
/// would replay the WHOLE rollout (a stale-activity burst + an instant
/// re-registration that negates the SubagentStop end). A released path
/// must be skipped by the re-vouch sweep; only fresh bytes revive it.
#[tokio::test]
async fn released_claim_is_not_revouched_into_a_full_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("child.jsonl");
    std::fs::write(&path, "{\"type\":\"assistant\"}\n").unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let id = default_id_from_path(&path);
    let agent_id = AgentId::from_parts("test", &id);
    let file_len = std::fs::metadata(&path).unwrap().len();

    // Register, then the hook end releases the claim.
    let events = walk_once(&path, Duration::from_secs(3600), t_ended, &cursors, &seen).await;
    assert!(events
        .iter()
        .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })));
    let unclaims = ChildEndUnclaims::new();
    unclaims.push(agent_id);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(64);
    let source: Arc<str> = Arc::from("test");
    // The FD probe still vouches the open rollout of the live child.
    let live: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::from([id.clone()])));
    let ctx = WatchCtx {
        source: &source,
        cursors: &cursors,
        seen: &seen,
        tx: &tx,
        window: Duration::from_secs(3600),
        live: &live,
    };
    drain_child_end_unclaims(Some(&unclaims), t_decoders(), &ctx).await;
    let events = drain_events(&mut rx);
    assert!(events.is_empty(), "release emits nothing, got {events:?}");
    assert_eq!(
        seen.lock().await.get(&path),
        Some(&false),
        "the claim must be RELEASED (false), not removed — removal is \
         exactly what would expose the path to the re-vouch replay below"
    );

    // The next scan pass runs while the probe STILL vouches the open
    // rollout: the re-vouch sweep must not reset the cursor / replay.
    let mut health = FailureLatch::default();
    scan_root(dir.path(), t_decoders(), &ctx, &mut health).await;
    let events = drain_events(&mut rx);
    assert!(
        events.is_empty(),
        "a re-vouch sweep over a RELEASED claim must not replay/re-register, got {events:?}"
    );
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(file_len),
        "the released path's cursor must stay parked at EOF (no reset-to-0 replay)"
    );
}

/// Cross-source isolation: an id claimed by NO path in this watcher must
/// STAY pending — `AgentId` is source-namespaced, so another source's
/// watcher is its owner and a later drain there must still find it.
/// This watcher's own claims stay untouched by the foreign id.
#[tokio::test]
async fn unclaim_for_foreign_id_stays_pending_and_leaves_local_claims_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("local.jsonl");
    std::fs::write(&path, "{\"type\":\"assistant\"}\n").unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    walk_once(&path, Duration::from_secs(3600), t_ended, &cursors, &seen).await;
    assert_eq!(seen.lock().await.get(&path), Some(&true));

    let unclaims = ChildEndUnclaims::new();
    let foreign = AgentId::from_parts("codex", "not-claimed-here");
    unclaims.push(foreign);
    let (tx, _rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(64);
    let source: Arc<str> = Arc::from("test");
    let live = Arc::new(Mutex::new(HashSet::new()));
    let ctx = WatchCtx {
        source: &source,
        cursors: &cursors,
        seen: &seen,
        tx: &tx,
        window: Duration::from_secs(3600),
        live: &live,
    };
    drain_child_end_unclaims(Some(&unclaims), t_decoders(), &ctx).await;
    assert_eq!(
        seen.lock().await.get(&path),
        Some(&true),
        "a foreign id must not release this watcher's claims"
    );
    assert_eq!(
        unclaims.take_matching(|x| *x == foreign),
        vec![foreign],
        "the foreign id must survive the non-matching drain for its owning watcher"
    );
}

/// The TTL prune pin: an entry no watcher ever matches is pruned after
/// the TTL (bounded growth), and a non-matching drain never consumes it
/// early.
#[tokio::test]
async fn child_end_unclaims_ttl_prunes_unmatched_entries() {
    let unclaims = ChildEndUnclaims::with_ttl(Duration::from_millis(40));
    let id = AgentId::from_parts("codex", "orphaned-entry");
    unclaims.push(id);
    assert!(
        unclaims.take_matching(|_| false).is_empty(),
        "a non-matching drain must not consume the entry"
    );
    assert_eq!(
        unclaims.take_matching(|x| *x == id),
        vec![id],
        "inside the TTL a later drain still finds it"
    );
    unclaims.push(id);
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        unclaims.take_matching(|_| true).is_empty(),
        "past the TTL the unmatched entry is pruned"
    );
}

#[tokio::test]
async fn oversized_ended_skip_unclaims_seen_so_a_later_append_re_registers() {
    // Same self-heal for the oversized branch: a REGISTERED file whose
    // > MAX_PENDING_BYTES skipped span buries a session_end emits the
    // terminator AND un-claims `seen`, so a later small append revives the
    // agent with a fresh SessionStart.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big-resumed.jsonl");
    let initial = "{\"type\":\"assistant\",\"cwd\":\"/repo\"}\n";
    tokio::fs::write(&path, initial).await.unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    // Pass 1: first-sight registration (file is small + live).
    let window = Duration::from_secs(3600);
    let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
        "live first sight must register, got {events:?}"
    );

    // Pass 2: an oversized span ending in session_end → terminator + skip.
    let mut full = String::from(initial);
    full.push_str(&"{\"type\":\"assistant\"}\n".repeat(60_000));
    full.push_str("{\"type\":\"system\",\"subtype\":\"session_end\"}\n");
    tokio::fs::write(&path, &full).await.unwrap();
    let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionEnd { .. })),
        "the buried terminator must emit SessionEnd, got {events:?}"
    );
    assert!(
        !seen.lock().await.contains_key(&path),
        "the oversized-ended skip must un-claim `seen`"
    );

    // Pass 3: a small live append revives the agent.
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .await
        .unwrap();
    tokio::io::AsyncWriteExt::write_all(&mut f, b"{\"type\":\"assistant\"}\n")
        .await
        .unwrap();
    tokio::io::AsyncWriteExt::flush(&mut f).await.unwrap();
    drop(f);
    let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
        "a post-end append must re-register the agent, got {events:?}"
    );
}

#[tokio::test]
async fn walk_jsonl_gates_a_first_sight_ended_file() {
    // #85: an ENDED session the initial read_dir missed must NOT be
    // resurrected when the rescan's walk_jsonl is the first to see it.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ended.jsonl");
    let content = "{\"type\":\"system\",\"subtype\":\"session_start\"}\n\
                   {\"type\":\"system\",\"subtype\":\"session_end\"}\n";
    tokio::fs::write(&path, content).await.unwrap();
    let len = tokio::fs::metadata(&path).await.unwrap().len();

    let (events, cursor) = first_sight_walk(&path, Duration::from_secs(3600), t_ended).await;
    assert!(
        events.is_empty(),
        "a never-seeded ENDED file must not emit SessionStart, got {events:?}"
    );
    assert_eq!(cursor, Some(len), "ended file must be seeded at EOF");
}

#[tokio::test]
async fn walk_jsonl_gates_a_first_sight_stale_file() {
    // The stale-on-startup flake's root: an OLD file the initial read_dir
    // missed must be seeded at EOF by the rescan, not read from the top.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.jsonl");
    tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/r\"}\n")
        .await
        .unwrap();
    backdate_one_hour(&path);
    let len = tokio::fs::metadata(&path).await.unwrap().len();

    let (events, cursor) = first_sight_walk(&path, Duration::from_secs(60), t_ended).await;
    assert!(
        events.is_empty(),
        "a never-seeded STALE file must not emit SessionStart, got {events:?}"
    );
    assert_eq!(cursor, Some(len), "stale file must be seeded at EOF");
}

#[tokio::test]
async fn known_oversized_tail_emits_session_end_if_the_skipped_span_ended() {
    // A tracked file grows by > MAX_PENDING_BYTES between passes, and that
    // skipped span buries a structural session_end marker. The watcher
    // must still emit SessionEnd before skipping to EOF — otherwise the
    // terminator is lost and the slot reaps only via the slow stale-sweep.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.jsonl");
    let initial = "{\"type\":\"assistant\",\"cwd\":\"/r\"}\n";
    tokio::fs::write(&path, initial).await.unwrap();
    let seeded = initial.len() as u64;

    // Overwrite with the same prefix + > 1 MiB of filler + a trailing
    // session_end line (lands in the tail-scan window).
    let mut full = String::from(initial);
    full.push_str(&"{\"type\":\"assistant\"}\n".repeat(60_000));
    full.push_str("{\"type\":\"system\",\"subtype\":\"session_end\"}\n");
    tokio::fs::write(&path, &full).await.unwrap();
    let len = full.len() as u64;
    assert!(
        len - seeded > (1 << 20),
        "the appended span must exceed MAX_PENDING_BYTES"
    );

    // Pre-seed the cursor so the file is KNOWN at `seeded`.
    let cursors = Arc::new(Mutex::new(HashMap::from([(path.clone(), seeded)])));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(32);
    let source: Arc<str> = Arc::from("test");
    let decoders = SourceDecoders {
        decode_line: t_decode,
        derive_label: t_label,
        check_ended: t_ended,
        id_derive: default_id_from_path,
    };
    let live = Arc::new(Mutex::new(HashSet::new()));
    let ctx = WatchCtx {
        source: &source,
        cursors: &cursors,
        seen: &seen,
        tx: &tx,
        window: Duration::from_secs(3600),
        live: &live,
    };
    walk_jsonl(&path, decoders, &ctx).await;
    drop(tx);

    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    let expected = AgentId::from_parts("test", &default_id_from_path(&path));
    assert!(
        events.iter().any(
            |(_, e)| matches!(e, AgentEvent::SessionEnd { agent_id, as_child: false } if *agent_id == expected)
        ),
        "a buried session_end in the skipped span must still emit SessionEnd, got {events:?}"
    );
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(len),
        "cursor must advance to EOF"
    );
}

#[tokio::test]
async fn gated_revive_falls_back_to_head_cwd_when_tail_has_none() {
    // G4: Codex rollouts carry cwd ONLY on the head session_meta line. A
    // file gated at first sight then revived by a small cwd-less append
    // used to register with an EMPTY cwd (downstream: unknown cwd → the
    // short reap), because the revive read cwd only from the appended
    // tail. The revive must fall back to a bounded head read.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-gated.jsonl");
    let head = "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/repo/head\",\"id\":\"u\"}}\n";
    let (cursors, seen) = gated_fixture(&path, head).await;

    let mut full = String::from(head);
    full.push_str("{\"type\":\"assistant\"}\n");
    tokio::fs::write(&path, &full).await.unwrap();

    let events = walk_once(&path, Duration::from_secs(60), t_ended, &cursors, &seen).await;
    let cwds: Vec<PathBuf> = events
        .iter()
        .filter_map(|(_, e)| match e {
            AgentEvent::SessionStart { cwd, .. } => Some(cwd.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        cwds,
        vec![PathBuf::from("/repo/head")],
        "the revive SessionStart must carry the head cwd, got {events:?}"
    );
}

#[tokio::test]
async fn gated_file_revives_on_small_append_with_tail_cwd() {
    // S1 (the audit's never-pinned plain case): a file GATED at first sight
    // (stale mtime → cursor seeded at EOF, no SessionStart) then revived by
    // a SMALL newline-terminated append must register — SessionStart +
    // Rename — and the registration carries the APPEND's cwd (the tail
    // read wins; the head read is only the G4 fallback when the tail
    // carries none, pinned by the head-vs-tail value split below).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gated-small.jsonl");
    let head = "{\"type\":\"assistant\",\"cwd\":\"/repo/head\"}\n";
    let (cursors, seen) = gated_fixture(&path, head).await;

    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .await
        .unwrap();
    tokio::io::AsyncWriteExt::write_all(
        &mut f,
        b"{\"type\":\"assistant\",\"cwd\":\"/repo/tail\"}\n",
    )
    .await
    .unwrap();
    tokio::io::AsyncWriteExt::flush(&mut f).await.unwrap();
    drop(f);

    let events = walk_once(&path, Duration::from_secs(60), t_ended, &cursors, &seen).await;
    let expected = AgentId::from_parts("test", &default_id_from_path(&path));
    let starts: Vec<(AgentId, PathBuf)> = events
        .iter()
        .filter_map(|(_, e)| match e {
            AgentEvent::SessionStart { agent_id, cwd, .. } => Some((*agent_id, cwd.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        starts,
        vec![(expected, PathBuf::from("/repo/tail"))],
        "the small-append revive must register exactly once, carrying the APPEND's cwd, got {events:?}"
    );
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            AgentEvent::Rename { agent_id, .. } if *agent_id == expected
        )),
        "the revive must emit the Rename half of the registration pair, got {events:?}"
    );
}

#[tokio::test]
async fn walk_jsonl_emits_for_a_first_sight_recent_live_file() {
    // The gate must NOT over-suppress: a recent, not-ended file seen first by
    // any path is a live session and must still get its SessionStart.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("live.jsonl");
    tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/r\"}\n")
        .await
        .unwrap();

    let (events, _cursor) = first_sight_walk(&path, Duration::from_secs(3600), t_ended).await;
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
        "a recent, not-ended file seen first must still emit SessionStart, got {events:?}"
    );
}

const LIVE_UUID: &str = "01000000-0000-7000-8000-0000000000aa";

#[tokio::test]
async fn probe_live_stale_file_registers_at_first_sight() {
    // T4: pixtuoid starts AFTER a long-idle live session. mtime says
    // historical (outside the window), but the first-party liveness probe
    // says the owning process is ALIVE — the gate must not hide it.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{LIVE_UUID}.jsonl"));
    tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/repo\"}\n")
        .await
        .unwrap();
    backdate_one_hour(&path);
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_once_live(
        &path,
        Duration::from_secs(60),
        &[LIVE_UUID],
        &cursors,
        &seen,
    )
    .await;
    let expected = AgentId::from_parts("test", LIVE_UUID);
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            AgentEvent::SessionStart { agent_id, .. } if *agent_id == expected
        )),
        "a probe-live stale transcript must register at first sight, got {events:?}"
    );
}

#[tokio::test]
async fn probe_miss_keeps_the_stale_gate() {
    // A non-empty live set that does NOT contain this transcript's id
    // changes nothing: the recency gate applies as today.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{LIVE_UUID}.jsonl"));
    tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/repo\"}\n")
        .await
        .unwrap();
    backdate_one_hour(&path);
    let len = tokio::fs::metadata(&path).await.unwrap().len();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_once_live(
        &path,
        Duration::from_secs(60),
        &["99999999-9999-7999-8999-999999999999"],
        &cursors,
        &seen,
    )
    .await;
    assert!(
        events.is_empty(),
        "a stale transcript the probe does not vouch for must stay gated, got {events:?}"
    );
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(len),
        "gated file must be seeded at EOF"
    );
}

#[tokio::test]
async fn probe_never_gates_a_recent_file() {
    // ADDITIVE-ONLY: a recent file absent from a non-empty live set still
    // registers — the probe can only admit, never hide what mtime admits.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{LIVE_UUID}.jsonl"));
    tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/repo\"}\n")
        .await
        .unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_once_live(
        &path,
        Duration::from_secs(3600),
        &["99999999-9999-7999-8999-999999999999"],
        &cursors,
        &seen,
    )
    .await;
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
        "a recent file must register regardless of the probe, got {events:?}"
    );
}

#[tokio::test]
async fn probe_live_oversized_stale_file_registers_via_head_read() {
    // A probe-live stale transcript whose whole body exceeds
    // MAX_PENDING_BYTES at first sight skips the gate and lands in the
    // #204 oversized first-sight branch: registered from a bounded head
    // read (cwd off line 1), backlog skipped to EOF.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{LIVE_UUID}.jsonl"));
    let mut full = String::from("{\"type\":\"assistant\",\"cwd\":\"/repo/head\"}\n");
    full.push_str(&"{\"type\":\"assistant\"}\n".repeat(60_000));
    assert!(full.len() as u64 > (1 << 20), "body must exceed 1 MiB");
    tokio::fs::write(&path, &full).await.unwrap();
    backdate_one_hour(&path);
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_once_live(
        &path,
        Duration::from_secs(60),
        &[LIVE_UUID],
        &cursors,
        &seen,
    )
    .await;
    let cwds: Vec<PathBuf> = events
        .iter()
        .filter_map(|(_, e)| match e {
            AgentEvent::SessionStart { cwd, .. } => Some(cwd.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        cwds,
        vec![PathBuf::from("/repo/head")],
        "the oversized probe-live first sight must register with the head cwd, got {events:?}"
    );
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(full.len() as u64),
        "backlog must be skipped to EOF, not replayed"
    );
}

#[tokio::test]
async fn scan_pass_re_vouches_a_transiently_gated_live_file() {
    // F1: a transient probe miss at first sight (registry file
    // mid-rewrite, a read race) gates a LIVE session — and without a
    // re-check every later pass exits at cursor == file_len and never
    // asks the probe again, hiding the session permanently. Each SCAN
    // pass (whose probe snapshot was just refreshed) must re-ask about
    // gated-but-never-registered files and replay a vouched one.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{LIVE_UUID}.jsonl"));
    tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/repo\"}\n")
        .await
        .unwrap();
    backdate_one_hour(&path);
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let live: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(32);
    let source: Arc<str> = Arc::from("test");
    let decoders = SourceDecoders {
        decode_line: t_decode,
        derive_label: t_label,
        check_ended: t_ended,
        id_derive: crate::source::claude_code::cc_id_from_path,
    };
    let ctx = WatchCtx {
        source: &source,
        cursors: &cursors,
        seen: &seen,
        tx: &tx,
        window: Duration::from_secs(60),
        live: &live,
    };

    let mut health = FailureLatch::default();
    // Pass 1: empty probe snapshot (the transient miss) → gated.
    scan_root(dir.path(), decoders, &ctx, &mut health).await;
    assert!(rx.try_recv().is_err(), "pass 1 must gate silently");
    assert!(
        !seen.lock().await.contains_key(&path),
        "gated, not registered"
    );

    // The next probe refresh sees the session — simulate it by mutating
    // the shared snapshot the way the run loop's refresh arms do.
    live.lock().await.insert(LIVE_UUID.to_string());

    // Pass 2: the scan must re-vouch the gated file and register it.
    scan_root(dir.path(), decoders, &ctx, &mut health).await;
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    let expected = AgentId::from_parts("test", LIVE_UUID);
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            AgentEvent::SessionStart { agent_id, .. } if *agent_id == expected
        )),
        "a re-vouched scan pass must register the gated live session, got {events:?}"
    );

    // Pass 3 (loop guard): the file registered → claimed `seen` → out of
    // the candidate set; nothing is re-emitted while the probe vouches.
    scan_root(dir.path(), decoders, &ctx, &mut health).await;
    assert!(
        rx.try_recv().is_err(),
        "a registered file must not be re-vouched/replayed again"
    );
}

#[tokio::test]
async fn probe_live_oversized_ended_first_sight_stays_unregistered() {
    // M1: the probe bypasses the first-sight gate — INCLUDING its ended
    // tail-scan — so a probe-admitted !known >1MiB ENDED transcript
    // reaches the oversized branch. Its ended check used to be gated on
    // `known` (assuming should_seed_at_eof had already filtered !known
    // ended files, which the probe bypass breaks): the terminator was
    // never emitted AND the #204 path registered a ghost for a session
    // that is over.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{LIVE_UUID}.jsonl"));
    let mut full = String::from("{\"type\":\"assistant\",\"cwd\":\"/repo/head\"}\n");
    full.push_str(&"{\"type\":\"assistant\"}\n".repeat(60_000));
    full.push_str("{\"type\":\"system\",\"subtype\":\"session_end\"}\n");
    assert!(full.len() as u64 > (1 << 20), "body must exceed 1 MiB");
    tokio::fs::write(&path, &full).await.unwrap();
    backdate_one_hour(&path);
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_once_live(
        &path,
        Duration::from_secs(60),
        &[LIVE_UUID],
        &cursors,
        &seen,
    )
    .await;
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
        "an ended oversized probe-admitted first sight must not register a ghost, got {events:?}"
    );
    let expected = AgentId::from_parts("test", LIVE_UUID);
    assert!(
        events.iter().any(
            |(_, e)| matches!(e, AgentEvent::SessionEnd { agent_id, as_child: false } if *agent_id == expected)
        ),
        "the buried terminator must still emit SessionEnd (a reducer no-op for an unknown id), got {events:?}"
    );
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(full.len() as u64),
        "backlog must be skipped to EOF, not replayed"
    );
}

#[tokio::test]
async fn probe_parent_uuid_does_not_admit_subagent_transcript() {
    // Subagent transcripts (<parent-uuid>/subagents/agent-*.jsonl) are NOT
    // in the registry; the join key is the file STEM (an agent id, not a
    // session UUID), so the parent's registry entry must not admit them —
    // they keep today's mtime gate.
    let dir = tempfile::tempdir().unwrap();
    let sub_dir = dir.path().join(LIVE_UUID).join("subagents");
    tokio::fs::create_dir_all(&sub_dir).await.unwrap();
    let path = sub_dir.join("agent-deadbeef.jsonl");
    tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/repo\"}\n")
        .await
        .unwrap();
    backdate_one_hour(&path);
    let len = tokio::fs::metadata(&path).await.unwrap().len();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_once_live(
        &path,
        Duration::from_secs(60),
        &[LIVE_UUID],
        &cursors,
        &seen,
    )
    .await;
    assert!(
        events.is_empty(),
        "a stale subagent transcript must stay gated even when its parent is probe-live, got {events:?}"
    );
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(len),
        "gated subagent transcript must be seeded at EOF"
    );
}

// ── #222: oversized-skip Task scan ──────────────────────────────────────
// Mid-attach to a delegating session with > MAX_PENDING_BYTES pending
// skips the backlog, losing the in-flight Agent dispatch (its PreToolUse
// hook predates attach too) — active_tasks stays empty, so subagent-leak
// suppression is off and b1 never arms. The oversized branch must
// tail-scan the last TASK_SCAN_BYTES and re-emit exactly the UNMATCHED
// Task ActivityStarts. These drive the REAL decode_cc_line so the line
// shapes (Agent tool_use with subagent_type / tool_result) are wire-true.

const FILLER_LINE: &str = "{\"type\":\"assistant\"}\n";
const CC_HEAD_LINE: &str = "{\"type\":\"assistant\",\"cwd\":\"/repo/head\"}\n";

fn cc_task_dispatch_line(tuid: &str) -> String {
    serde_json::json!({
        "type": "assistant",
        "cwd": "/repo/head",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": tuid, "name": "Agent",
                  "input": { "description": "explore",
                             "subagent_type": "code-explorer",
                             "prompt": "go" } }
            ]
        }
    })
    .to_string()
        + "\n"
}

fn cc_task_result_line(tuid: &str) -> String {
    serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [
                { "type": "tool_result", "tool_use_id": tuid, "content": "done" }
            ]
        }
    })
    .to_string()
        + "\n"
}

/// `CC_HEAD_LINE` + filler past MAX_PENDING_BYTES + the given tail lines —
/// the whole body is one oversized first-sight pending span.
fn oversized_body(tail_lines: &[String]) -> String {
    let mut full = String::from(CC_HEAD_LINE);
    while full.len() <= (1usize << 20) + 4096 {
        full.push_str(FILLER_LINE);
    }
    for l in tail_lines {
        full.push_str(l);
    }
    full
}

/// The Jsonl-tagged Task ActivityStarts among `events`, as tuids.
fn task_start_tuids(events: &[(Transport, AgentEvent)]) -> Vec<String> {
    events
        .iter()
        .filter_map(|(t, e)| match e {
            AgentEvent::ActivityStart {
                tool_use_id: Some(tuid),
                detail: Some(d),
                ..
            } if d.is_task() => {
                assert_eq!(*t, Transport::Jsonl, "synthesized starts are Jsonl-tagged");
                Some(tuid.clone())
            }
            _ => None,
        })
        .collect()
}

async fn walk_oversized_cc(
    path: &Path,
    window: Duration,
    cursors: &Arc<Mutex<HashMap<PathBuf, u64>>>,
    seen: &Arc<Mutex<HashMap<PathBuf, bool>>>,
) -> Vec<(Transport, AgentEvent)> {
    walk_once_with(
        path,
        window,
        crate::source::claude_code::decode_cc_line,
        t_ended,
        cursors,
        seen,
    )
    .await
}

#[tokio::test]
async fn oversized_attach_seeds_unmatched_task_dispatch() {
    // The headline #222 case: a recent > 1 MiB transcript whose tail holds
    // an Agent dispatch with NO matching tool_result — the walk must
    // register the agent AND re-emit that dispatch as a Task
    // ActivityStart (after the SessionStart, so the reducer has a slot).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deleg-big.jsonl");
    let full = oversized_body(&[cc_task_dispatch_line("tu_task")]);
    tokio::fs::write(&path, &full).await.unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_oversized_cc(&path, Duration::from_secs(3600), &cursors, &seen).await;
    let start_pos = events
        .iter()
        .position(|(_, e)| matches!(e, AgentEvent::SessionStart { .. }))
        .expect("the oversized first sight must register the agent (#204)");
    assert_eq!(
        task_start_tuids(&events),
        vec!["tu_task".to_string()],
        "the unmatched in-flight dispatch must be re-emitted, got {events:?}"
    );
    let task_pos = events
        .iter()
        .position(|(_, e)| matches!(e, AgentEvent::ActivityStart { .. }))
        .expect("checked above");
    assert!(
        start_pos < task_pos,
        "registration must precede the synthesized Task start (a JSONL event for an unknown id is a reducer no-op)"
    );
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(full.len() as u64),
        "cursor must land at EOF — the scan seeds tasks, it must not replay the backlog"
    );
}

#[tokio::test]
async fn oversized_attach_matched_task_is_not_seeded() {
    // A dispatch whose tool_result also sits in the window has RETURNED —
    // re-emitting it would pin the parent Delegating forever (no further
    // completion is coming). Window geometry makes this exact: a
    // completion is always later in the file than its start.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deleg-done.jsonl");
    let full = oversized_body(&[
        cc_task_dispatch_line("tu_task"),
        cc_task_result_line("tu_task"),
    ]);
    tokio::fs::write(&path, &full).await.unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_oversized_cc(&path, Duration::from_secs(3600), &cursors, &seen).await;
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
        "registration still fires, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::ActivityStart { .. })),
        "a matched (returned) dispatch must not be seeded — and no other backlog event may leak from the scan, got {events:?}"
    );
}

#[tokio::test]
async fn oversized_attach_ended_session_skips_task_scan() {
    // Ended wins: the buried terminator just emitted SessionEnd, so
    // seeding a Task afterwards would animate a ghost delegation. The
    // file is pre-seeded KNOWN at the head — a recent ENDED file at FIRST
    // sight is gated by should_seed_at_eof and never reaches the
    // oversized branch at all.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deleg-ended.jsonl");
    let full = oversized_body(&[
        cc_task_dispatch_line("tu_task"),
        "{\"type\":\"system\",\"subtype\":\"session_end\"}\n".to_string(),
    ]);
    tokio::fs::write(&path, &full).await.unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::from([(
        path.clone(),
        CC_HEAD_LINE.len() as u64,
    )])));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_oversized_cc(&path, Duration::from_secs(3600), &cursors, &seen).await;
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionEnd { .. })),
        "the buried terminator must still emit SessionEnd, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::ActivityStart { .. })),
        "an ended span must not seed Task starts, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
        "an ended span must not register either (ghost), got {events:?}"
    );
}

#[tokio::test]
async fn oversized_attach_unregistered_skips_task_scan() {
    // A stale, probe-less oversized file is gated unregistered (no slot)
    // — JSONL events for an unknown id are reducer no-ops, so the scan
    // must not run (no wasted decode, no orphan Task events).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deleg-stale.jsonl");
    let full = oversized_body(&[cc_task_dispatch_line("tu_task")]);
    tokio::fs::write(&path, &full).await.unwrap();
    backdate_one_hour(&path);
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_oversized_cc(&path, Duration::from_secs(60), &cursors, &seen).await;
    assert!(
        events.is_empty(),
        "a gated unregistered oversized file must emit NOTHING — no Task seeding without a slot, got {events:?}"
    );
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(full.len() as u64),
        "gated file must still be seeded at EOF"
    );
}

#[tokio::test]
async fn oversized_attach_dispatch_outside_window_is_missed() {
    // THE documented residual: a dispatch buried deeper than
    // TASK_SCAN_BYTES of subsequent traffic keeps the pre-#222 behavior
    // (skipped — the parent re-enters Delegating only via live signals).
    // Pinned explicitly so a window-size change is a conscious decision.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deleg-buried.jsonl");
    let mut full = String::from(CC_HEAD_LINE);
    let dispatch = cc_task_dispatch_line("tu_buried");
    full.push_str(&dispatch);
    let dispatch_end = full.len();
    while full.len() <= (1usize << 20) + 4096 {
        full.push_str(FILLER_LINE);
    }
    assert!(
        (full.len() - dispatch_end) as u64 > TASK_SCAN_BYTES,
        "the dispatch must sit deeper than the scan window"
    );
    tokio::fs::write(&path, &full).await.unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_oversized_cc(&path, Duration::from_secs(3600), &cursors, &seen).await;
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
        "registration still fires, got {events:?}"
    );
    assert!(
        task_start_tuids(&events).is_empty(),
        "a dispatch outside the tail window is consciously missed (bounded residual), got {events:?}"
    );
}

#[tokio::test]
async fn task_scan_handles_partial_first_line() {
    // The window boundary (file_len - TASK_SCAN_BYTES) almost never lands
    // on a line boundary. Engineer it to split a Task dispatch mid-JSON:
    // the straddled fragment must be skipped (not decoded, no panic) and
    // a complete dispatch inside the window still seeds.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deleg-straddle.jsonl");
    let task_a = cc_task_dispatch_line("tu_straddle");
    let task_b = cc_task_dispatch_line("tu_inside");

    let mut full = String::from(CC_HEAD_LINE);
    // Deep enough that suffix + window keeps the total > MAX_PENDING_BYTES.
    while full.len() < (1usize << 20) {
        full.push_str(FILLER_LINE);
    }
    let offset_a = full.len();
    full.push_str(&task_a);
    full.push_str(&task_b);
    // Pad the tail so the boundary lands strictly inside task_a's bytes.
    let delta = task_a.len() / 2;
    let target_len = offset_a + delta + TASK_SCAN_BYTES as usize;
    let pad = target_len - full.len();
    assert!(pad > FILLER_LINE.len(), "padding must fit one JSON line");
    full.push_str("{\"type\":\"assistant\"}");
    full.push_str(&" ".repeat(pad - FILLER_LINE.len()));
    full.push('\n');
    assert_eq!(full.len(), target_len);
    let boundary = full.len() - TASK_SCAN_BYTES as usize;
    assert!(
        boundary > offset_a && boundary < offset_a + task_a.len(),
        "the window boundary must split the straddled dispatch mid-line"
    );
    tokio::fs::write(&path, &full).await.unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_oversized_cc(&path, Duration::from_secs(3600), &cursors, &seen).await;
    assert_eq!(
        task_start_tuids(&events),
        vec!["tu_inside".to_string()],
        "the complete in-window dispatch seeds; the straddled fragment is skipped, got {events:?}"
    );
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(full.len() as u64),
        "cursor must land at EOF"
    );
}
