use std::io::Write;
use std::time::Instant;

use super::health::FailureLatch;
use super::liveness::{emit_session_exit, revouch_gated_files, ProbeLadder, ProbeSnapshot};
use super::unclaim::drain_child_end_unclaims;
use super::walk::{
    detect_parent_id, extract_cwd, park_if_truncated_below_cursor, scan_root, walk_jsonl,
    TASK_SCAN_BYTES,
};
use super::*;
use crate::source::registry::cwd_extractor_for;
use crate::source::{AgentEvent, Transport};
use crate::AgentId;

/// A healthy `ProbeSnapshot` binding each `(id, pid)`. The ladder is a PURE
/// state machine (functional core), so these tests drive it with synthetic time
/// and zero mocks — no `WatchCtx`, no probe, no tokio.
fn snap(pairs: &[(&str, i32)]) -> ProbeSnapshot {
    ProbeSnapshot {
        pid_of: pairs
            .iter()
            .map(|(id, pid)| (id.to_string(), *pid))
            .collect(),
    }
}

// ---- the producer-side fd-probe seam (ProbeSnapshot::bind_pid / from_open_fd_pairs) ----

#[test]
fn bind_pid_keeps_the_larger_pid_in_both_orders() {
    // Two live processes holding one file (a resume overlap) resolve to the
    // deterministic #252 winner — larger pid — regardless of bind order.
    for pids in [[100, 200], [200, 100]] {
        let mut s = ProbeSnapshot::default();
        for pid in pids {
            s.bind_pid("sess".to_string(), pid);
        }
        assert_eq!(
            s.pid_of.get("sess"),
            Some(&200),
            "larger pid wins ({pids:?})"
        );
    }
}

#[test]
fn from_open_fd_pairs_filters_under_root_then_applies_accept() {
    // The shared producer-side join: keep only under-root, accepted paths,
    // bound to the owning pid. `accept` here is a trivial "any .jsonl, id =
    // file stem" so the test pins the MECHANICS (under-root filter + #252
    // bind), not any source's format.
    let root = std::path::Path::new("/root");
    let accept = |p: &std::path::Path| {
        (p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
            .then(|| p.file_stem().unwrap().to_string_lossy().into_owned())
    };
    let pairs = [
        (7, std::path::PathBuf::from("/root/a/keep.jsonl")), // under root, accepted
        (9, std::path::PathBuf::from("/elsewhere/keep.jsonl")), // outside root
        (11, std::path::PathBuf::from("/root/a/skip.txt")),  // under root, rejected
    ];
    let got = ProbeSnapshot::from_open_fd_pairs(root, pairs.into_iter(), accept);
    assert_eq!(
        got.ids().cloned().collect::<Vec<_>>(),
        vec!["keep".to_string()]
    );
    assert_eq!(
        got.pid_of.get("keep"),
        Some(&7),
        "only the under-root .jsonl vouches, bound to its pid"
    );
}

#[test]
fn fold_confirms_an_exit_only_after_a_sustained_miss() {
    let span = Duration::from_secs(60);
    let mut ladder = ProbeLadder::new(span);
    let t0 = Instant::now();
    // Vouch the session, then two healthy misses spanning `min_span`.
    assert!(ladder.fold(&snap(&[("sess", 1)]), t0).exits.is_empty());
    assert!(ladder.fold(&snap(&[]), t0).exits.is_empty()); // window opens
    assert!(ladder.fold(&snap(&[]), t0 + span / 2).exits.is_empty()); // still inside
    assert_eq!(
        ladder.fold(&snap(&[]), t0 + span).exits,
        vec!["sess".to_string()],
        "a miss sustained past min_span confirms the exit"
    );
    // No double-confirm on a later miss.
    assert!(ladder.fold(&snap(&[]), t0 + span * 2).exits.is_empty());
}

#[test]
fn fold_reappearance_cancels_a_pending_miss_window() {
    let span = Duration::from_secs(60);
    let mut ladder = ProbeLadder::new(span);
    let t0 = Instant::now();
    ladder.fold(&snap(&[("sess", 1)]), t0);
    ladder.fold(&snap(&[]), t0); // miss window opens
    ladder.fold(&snap(&[("sess", 1)]), t0 + span / 2); // reappears — cancels it
                                                       // A later miss must re-open a FRESH window, not confirm off the stale one.
    assert!(
        ladder.fold(&snap(&[]), t0 + span).exits.is_empty(),
        "a re-appearing id resets its miss window"
    );
}

#[test]
fn fold_returns_each_new_pid_once_for_the_exit_watch() {
    let mut ladder = ProbeLadder::new(Duration::from_secs(60));
    let t = Instant::now();
    // pid 1 (a+b) and pid 2 (c) each register exactly once.
    let mut watched = ladder
        .fold(&snap(&[("a", 1), ("b", 1), ("c", 2)]), t)
        .newly_watched;
    watched.sort_unstable();
    assert_eq!(watched, vec![1, 2]);
    // Re-seeing the same pids registers nothing new (bindings are additive).
    assert!(ladder
        .fold(&snap(&[("a", 1), ("c", 2)]), t)
        .newly_watched
        .is_empty());
}

/// #223 rebind: `snap.pid_of` is ownership ground truth — an id seen under a
/// NEW pid MIGRATES its binding (so the old pid's later death can't end the live
/// session) while a sibling on the old pid survives the single-id unbind.
/// Observed through the interface: `pid_died` on each pid returns the right ids
/// (an inverted `!=` that left the stale binding in place would return "a" from
/// the old pid too).
#[test]
fn fold_migrates_a_rebound_id_and_keeps_the_old_pids_sibling() {
    let mut ladder = ProbeLadder::new(Duration::from_secs(60));
    let t = Instant::now();
    ladder.fold(&snap(&[("a", 1), ("b", 1)]), t); // a, b under pid 1
                                                  // "a" resumes under pid 2 in another process; "b" stays on pid 1.
    let out = ladder.fold(&snap(&[("a", 2), ("b", 1)]), t);
    assert_eq!(out.newly_watched, vec![2], "only the new pid 2 registers");
    // pid 1's death now ends only "b" (a migrated away); pid 2's ends "a".
    assert_eq!(ladder.pid_died(1), vec!["b".to_string()]);
    assert_eq!(ladder.pid_died(2), vec!["a".to_string()]);
}

/// The instant-exit ↔ negative-vouch handshake: `pid_died` returns the dead ids
/// AND disarms the ledger for each, so a later healthy-snapshot pair can never
/// re-confirm an id whose `SessionEnd` the faster instant-exit rung already
/// emitted (a body-wiped `forget` would leave the ledger armed and emit a
/// duplicate). Also pins the unknown/duplicate-pid no-op.
#[test]
fn pid_died_returns_its_ids_and_disarms_the_negative_vouch() {
    let span = Duration::from_secs(60);
    let mut ladder = ProbeLadder::new(span);
    let t0 = Instant::now();
    ladder.fold(&snap(&[("sess", 2)]), t0); // vouched, bound to pid 2
                                            // The instant-exit rung fires first and returns the id.
    assert_eq!(ladder.pid_died(2), vec!["sess".to_string()]);
    // A duplicate/unknown pid event is a no-op.
    assert!(ladder.pid_died(2).is_empty());
    // The slower negative vouch must NOT re-confirm the id it just forgot,
    // however long it stays missing.
    assert!(ladder.fold(&snap(&[]), t0).exits.is_empty());
    assert!(ladder.fold(&snap(&[]), t0 + span * 2).exits.is_empty());
}

#[test]
fn fold_drops_a_pid_emptied_by_a_confirmed_exit_so_a_reused_pid_re_registers() {
    let span = Duration::from_secs(60);
    let mut ladder = ProbeLadder::new(span);
    let t0 = Instant::now();
    // "sess" is the SOLE id on pid 5 — registered once.
    assert_eq!(
        ladder.fold(&snap(&[("sess", 5)]), t0).newly_watched,
        vec![5]
    );
    // Confirm its exit across the span: the confirm-unbind EMPTIES pid 5, which
    // must be DROPPED (not left as a stale empty binding).
    ladder.fold(&snap(&[]), t0);
    assert_eq!(
        ladder.fold(&snap(&[]), t0 + span).exits,
        vec!["sess".to_string()]
    );
    // A later, unrelated id reusing pid 5 must RE-register with the exit watch —
    // proving the emptied entry was dropped. A leaked `{5: {}}` would read as
    // "not newly seen" and silently skip the instant-exit re-registration.
    assert_eq!(
        ladder.fold(&snap(&[("other", 5)]), t0 + span).newly_watched,
        vec![5]
    );
}

#[test]
fn default_id_from_path_returns_normalized_path_key() {
    // Lowercase literal: identity on every platform (the Windows fold is
    // pinned by id.rs's normalize_path_key unit tests + the backslash
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
fn extract_cwd_dispatches_the_scanned_sources_shape() {
    // CC/AG shape: top-level cwd (also the unregistered-source default).
    let top = br#"{"cwd":"/repo/a"}"#;
    assert_eq!(
        extract_cwd(top, cwd_extractor_for("claude-code")),
        Some(PathBuf::from("/repo/a"))
    );
    assert_eq!(
        extract_cwd(top, cwd_extractor_for("test")),
        Some(PathBuf::from("/repo/a")),
        "an unregistered harness source keeps the shared top-level default"
    );
    // Codex shape: cwd nested under payload (session_meta) — extracted only
    // when codex is the scanned source.
    let nested = br#"{"type":"session_meta","payload":{"cwd":"/repo/b","id":"u"}}"#;
    assert_eq!(
        extract_cwd(nested, cwd_extractor_for("codex")),
        Some(PathBuf::from("/repo/b"))
    );
    // The scan still skips non-JSON / cwd-less prefix lines (never
    // short-circuits on them) before the shape matches.
    let mixed = b"not-json\n{\"type\":\"noise\"}\n{\"cwd\":\"/repo/c\"}\n";
    assert_eq!(
        extract_cwd(mixed, cwd_extractor_for("claude-code")),
        Some(PathBuf::from("/repo/c"))
    );
}

#[test]
fn cc_head_scan_ignores_codex_shaped_payload_cwd() {
    // Design-debt #5: the head scan must dispatch by the SCANNED source, not
    // try every source's shape — a codex-shaped `payload.cwd` inside a CC
    // transcript must NOT label the CC session with the foreign cwd.
    let codex_shaped = br#"{"type":"session_meta","payload":{"cwd":"/foreign/repo","id":"u"}}"#;
    assert_eq!(
        extract_cwd(codex_shaped, cwd_extractor_for("claude-code")),
        None,
        "a foreign source's cwd shape must not extract for a CC transcript"
    );
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
        path_filter: accept_all_paths,
        cwd_derive: no_cwd_from_path,
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

/// The `with_cwd_deriver` seam (grok): a first-sight whose CONTENT head-scan
/// yields no cwd must fall back to the PATH deriver — without it the
/// registration lands empty-cwd and rides the reducer's unknown-cwd short
/// reap. Content, when present, still wins (the deriver is a fallback only).
#[tokio::test]
async fn first_sight_cwd_falls_back_to_the_path_deriver_when_content_has_none() {
    fn derived_cwd(_p: &Path) -> Option<PathBuf> {
        Some(PathBuf::from("/derived/proj"))
    }
    async fn first_sight_cwd(line: &str) -> PathBuf {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("updates.jsonl");
        std::fs::write(&path, format!("{line}\n")).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(32);
        let source: Arc<str> = Arc::from("test");
        let decoders = SourceDecoders {
            decode_line: t_decode,
            derive_label: t_label,
            check_ended: t_ended,
            id_derive: default_id_from_path,
            path_filter: accept_all_paths,
            cwd_derive: derived_cwd,
        };
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));
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
        while let Ok((_, ev)) = rx.try_recv() {
            if let AgentEvent::SessionStart { cwd, .. } = ev {
                return cwd;
            }
        }
        panic!("no SessionStart emitted for {line:?}");
    }

    // No content cwd → the path deriver fills it.
    assert_eq!(
        first_sight_cwd(r#"{"x":1}"#).await,
        PathBuf::from("/derived/proj")
    );
    // A content cwd (the shared top-level extractor's shape for the
    // unregistered "test" source) still WINS over the deriver.
    assert_eq!(
        first_sight_cwd(r#"{"cwd":"/content/proj"}"#).await,
        PathBuf::from("/content/proj")
    );
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
        path_filter: accept_all_paths,
        cwd_derive: no_cwd_from_path,
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
async fn walk_jsonl_honors_the_path_filter() {
    // The per-source PathFilter (Antigravity's `transcript_full.jsonl` skip)
    // drops a file BEFORE any cursor/registration work, so a conversation's
    // duplicate sibling never mints a second path-keyed slot. Observable: the
    // filtered file gets no `cursors` entry, the admitted one does.
    fn skip_full(p: &Path) -> bool {
        p.file_name().and_then(|s| s.to_str()) != Some("transcript_full.jsonl")
    }
    let dir = tempfile::tempdir().unwrap();
    let kept = dir.path().join("transcript.jsonl");
    let dropped = dir.path().join("transcript_full.jsonl");
    tokio::fs::write(&kept, "{\"type\":\"x\"}\n").await.unwrap();
    tokio::fs::write(&dropped, "{\"type\":\"x\"}\n")
        .await
        .unwrap();

    let (tx, _rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(32);
    let source: Arc<str> = Arc::from("test");
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let live = Arc::new(Mutex::new(HashSet::new()));
    let decoders = SourceDecoders {
        decode_line: t_decode,
        derive_label: t_label,
        check_ended: t_ended,
        id_derive: default_id_from_path,
        path_filter: skip_full,
        cwd_derive: no_cwd_from_path,
    };
    let ctx = WatchCtx {
        source: &source,
        cursors: &cursors,
        seen: &seen,
        tx: &tx,
        window: Duration::from_secs(60),
        live: &live,
    };
    // Walk the DIRECTORY so both siblings are visited.
    walk_jsonl(dir.path(), decoders, &ctx).await;

    let cursors = cursors.lock().await;
    assert!(
        cursors.contains_key(&kept),
        "the admitted transcript.jsonl must be processed"
    );
    assert!(
        !cursors.contains_key(&dropped),
        "transcript_full.jsonl must be filtered before any cursor work"
    );
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
        (full.len() - initial.len()) as u64 > super::walk::MAX_PENDING_BYTES,
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
        path_filter: accept_all_paths,
        cwd_derive: no_cwd_from_path,
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
        path_filter: accept_all_paths,
        cwd_derive: no_cwd_from_path,
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
        path_filter: accept_all_paths,
        cwd_derive: no_cwd_from_path,
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
    // The TTL is generous relative to the between-assert wall time: the
    // "inside the TTL" drains below must land before it elapses even on a
    // loaded machine (a 40ms TTL flaked when the scheduler stalled the test
    // past it), and the prune sleep only needs to EXCEED it — load can only
    // stretch the sleep further past, never under.
    let ttl = Duration::from_millis(250);
    let unclaims = ChildEndUnclaims::with_ttl(ttl);
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
    tokio::time::sleep(ttl * 2).await;
    assert!(
        unclaims.take_matching(|_| true).is_empty(),
        "past the TTL the unmatched entry is pruned"
    );
}

/// The release loop is scoped to the MATCHED id's own path: a sibling path
/// this watcher also holds (a different session) must keep its claim when
/// another child's un-claim drains. Pins the loop's skip condition
/// (`*pid != id || !*held`) — an `||`→`&&` flip would walk + release every
/// held sibling on any drain.
#[tokio::test]
async fn child_end_unclaim_releases_only_the_matched_ids_own_path() {
    let dir = tempfile::tempdir().unwrap();
    let ended_child = dir.path().join("ended-child.jsonl");
    let sibling = dir.path().join("live-sibling.jsonl");
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    for p in [&ended_child, &sibling] {
        std::fs::write(p, "{\"type\":\"assistant\"}\n").unwrap();
        walk_once(p, Duration::from_secs(3600), t_ended, &cursors, &seen).await;
        assert_eq!(seen.lock().await.get(p), Some(&true), "fixture: registered");
    }

    let unclaims = ChildEndUnclaims::new();
    unclaims.push(AgentId::from_parts(
        "test",
        &default_id_from_path(&ended_child),
    ));
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
        seen.lock().await.get(&ended_child),
        Some(&false),
        "the ended child's claim is released"
    );
    assert_eq!(
        seen.lock().await.get(&sibling),
        Some(&true),
        "the sibling session's claim must survive another child's un-claim"
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
        len - seeded > super::walk::MAX_PENDING_BYTES,
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
        path_filter: accept_all_paths,
        cwd_derive: no_cwd_from_path,
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
    // G4: a source can carry cwd ONLY on its head line (Codex's session_meta
    // is the motivating case). A file gated at first sight then revived by a
    // small cwd-less append used to register with an EMPTY cwd (downstream:
    // unknown cwd → the short reap), because the revive read cwd only from
    // the appended tail. The revive must fall back to a bounded head read.
    // (This harness runs under the unregistered "test" source → the shared
    // top-level shape; the codex payload-nested dispatch itself is pinned by
    // `extract_cwd_dispatches_the_scanned_sources_shape` + the registry tests.)
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-gated.jsonl");
    let head = "{\"type\":\"meta\",\"cwd\":\"/repo/head\",\"id\":\"u\"}\n";
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
        path_filter: accept_all_paths,
        cwd_derive: no_cwd_from_path,
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

/// The oversized skip is STRICT (`pending > MAX_PENDING_BYTES`): a span of
/// EXACTLY 1 MiB pending still replays through the normal decode path —
/// activity events included. A `>`→`>=` flip would divert the boundary span
/// into the skip branch (registration + Task-seeding only, the backlog's
/// ordinary activity silently dropped).
#[tokio::test]
async fn pending_span_of_exactly_max_bytes_replays_instead_of_skipping() {
    let bash_line = serde_json::json!({
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_bash", "name": "Bash",
                  "input": { "command": "ls" } }
            ]
        }
    })
    .to_string()
        + "\n";
    // walk_jsonl's MAX_PENDING_BYTES (a fn-local const) — the same value the
    // sibling oversized fixtures overshoot with `+ 4096`.
    let target = 1usize << 20;
    let pad_open = "{\"type\":\"assistant\",\"pad\":\"";
    let pad_close = "\"}\n";
    let pad_len = target - CC_HEAD_LINE.len() - bash_line.len() - pad_open.len() - pad_close.len();
    let full = format!(
        "{CC_HEAD_LINE}{pad_open}{}{pad_close}{bash_line}",
        "x".repeat(pad_len)
    );
    assert_eq!(full.len(), target, "fixture: exactly 1 MiB pending");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("boundary.jsonl");
    tokio::fs::write(&path, &full).await.unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let events = walk_oversized_cc(&path, Duration::from_secs(3600), &cursors, &seen).await;
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::ActivityStart { .. })),
        "an exactly-at-the-cap span must REPLAY (strict >) — the ordinary \
         tool activity must come through, got {events:?}"
    );
}

/// Two distinct unmatched dispatches — one buried ~4 KiB before EOF (well
/// past a `256*1024`→`256+1024` mutation's collapsed 1.3 KiB window), one
/// near EOF — must BOTH be seeded, each exactly once, in file order. Pins
/// the scan window's real depth and the pending-set's match-on-same-tuid
/// dedupe (`==`→`!=` would drop every dispatch after the first).
#[tokio::test]
async fn oversized_attach_seeds_dispatches_across_the_full_scan_window() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deleg-deep.jsonl");
    let mut tail = vec![cc_task_dispatch_line("tu_deep")];
    tail.extend(std::iter::repeat_n(FILLER_LINE.to_string(), 200)); // ~4.4 KiB spacing
    tail.push(cc_task_dispatch_line("tu_near"));
    let full = oversized_body(&tail);
    tokio::fs::write(&path, &full).await.unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_oversized_cc(&path, Duration::from_secs(3600), &cursors, &seen).await;
    assert_eq!(
        task_start_tuids(&events),
        vec!["tu_deep".to_string(), "tu_near".to_string()],
        "both in-flight dispatches across the window must be seeded once, in file order"
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

/// R0612-03: symlinked entries under a watched root are refused wholesale —
/// a directory symlink would recurse through a planted loop / walk foreign
/// `.jsonl` trees outside the root, and a file symlink would pull a foreign
/// transcript into this source's id space. Nothing first-party lays out
/// symlinks under a projects/sessions root, so the skip costs no legitimate
/// session; a REAL transcript next to the symlinks must still register.
#[cfg(unix)]
#[tokio::test]
async fn walk_refuses_symlinked_entries() {
    let outside = tempfile::tempdir().unwrap();
    let foreign = outside.path().join("foreign.jsonl");
    std::fs::write(&foreign, "{\"type\":\"assistant\"}\n").unwrap();

    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real.jsonl");
    std::fs::write(&real, "{\"type\":\"assistant\"}\n").unwrap();
    // The three refusal shapes: an out-of-root dir symlink, an out-of-root
    // file symlink, and a self-loop.
    std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
    std::os::unix::fs::symlink(&foreign, root.path().join("link.jsonl")).unwrap();
    std::os::unix::fs::symlink(root.path(), root.path().join("loop")).unwrap();

    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(64);
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
    let mut health = FailureLatch::default();
    scan_root(root.path(), t_decoders(), &ctx, &mut health).await;
    drop(tx);
    let events = drain_events(&mut rx);

    let real_id = AgentId::from_parts("test", &default_id_from_path(&real));
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            AgentEvent::SessionStart { agent_id, .. } if *agent_id == real_id
        )),
        "the real transcript must still register, got {events:?}"
    );
    let foreign_id = AgentId::from_parts("test", &default_id_from_path(&foreign));
    assert!(
        !events.iter().any(|(_, e)| e.agent_id() == foreign_id),
        "a symlinked foreign transcript must emit nothing, got {events:?}"
    );
    assert!(
        cursors
            .lock()
            .await
            .keys()
            .all(|p| p.parent() == Some(root.path()) && !p.is_symlink()),
        "no symlinked or out-of-root path may be tracked: {:?}",
        cursors.lock().await.keys().collect::<Vec<_>>()
    );
}

/// R0612-04 (exit arm): a transcript truncated/recreated BELOW its cursor at
/// the moment the exit drain runs hits `walk_jsonl`'s truncation arm — cursor
/// reset to 0, return WITHOUT draining — leaving exactly the state the #228
/// drain-before-unclaim discipline forbids: existing bytes "pending" on a
/// path whose claim the exit is about to retire. The exit must park the
/// cursor at the NEW EOF instead, so a straggler walk cannot replay the dead
/// session's bytes as a ghost first-sight (every fast rung is already
/// disarmed for it); only a genuinely NEW append revives.
#[tokio::test]
async fn session_exit_parks_truncated_transcript_so_a_straggler_walk_cannot_resurrect() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"assistant\"}\n{\"type\":\"assistant\"}\n{\"type\":\"assistant\"}\n",
    )
    .unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let window = Duration::from_secs(3600);

    // Register normally (recent file → SessionStart, cursor at EOF).
    let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
    assert!(events
        .iter()
        .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })));

    // The transcript is recreated SMALLER before the exit arm runs.
    std::fs::write(&path, "{\"type\":\"assistant\"}\n").unwrap();
    let new_len = std::fs::metadata(&path).unwrap().len();
    assert!(
        *cursors.lock().await.get(&path).unwrap() > new_len,
        "fixture: the file must be truncated below the cursor"
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(32);
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
    let id = default_id_from_path(&path);
    emit_session_exit(&id, t_decoders(), &ctx).await;
    drop(tx);
    let exit_events = drain_events(&mut rx);
    assert!(
        matches!(
            exit_events.last(),
            Some((Transport::Jsonl, AgentEvent::SessionEnd { .. }))
        ),
        "the terminator must be emitted (last), got {exit_events:?}"
    );
    assert!(
        !exit_events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
        "the exit drain must not register anything, got {exit_events:?}"
    );
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(new_len),
        "a truncated transcript must be PARKED at the new EOF, not reset to 0"
    );
    assert!(
        !seen.lock().await.contains_key(&path),
        "seen must be un-claimed so a genuine post-death append revives"
    );

    // The straggler walk: must be a no-op, NOT a ghost first-sight replay.
    let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
    assert!(
        events.is_empty(),
        "a straggler walk after the exit must not resurrect, got {events:?}"
    );

    // A genuinely NEW append still revives — the self-heal contract.
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

/// R0612-04 (the #246 sibling): the child-end un-claim drain has the same
/// truncation corner — its release (`seen` → false) must not leave the
/// truncation arm's cursor-0 reset behind, or the next pass replays the
/// rollout's existing bytes and re-registers the just-ended child.
#[tokio::test]
async fn child_end_unclaim_parks_truncated_transcript_before_release() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("child.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"assistant\"}\n{\"type\":\"assistant\"}\n{\"type\":\"assistant\"}\n",
    )
    .unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let window = Duration::from_secs(3600);

    let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
    assert!(events
        .iter()
        .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })));

    // Truncated/recreated smaller before the drain runs.
    std::fs::write(&path, "{\"type\":\"assistant\"}\n").unwrap();
    let new_len = std::fs::metadata(&path).unwrap().len();
    assert!(
        *cursors.lock().await.get(&path).unwrap() > new_len,
        "fixture: the file must be truncated below the cursor"
    );

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
        "the un-claim emits NOTHING, truncated or not — got {events:?}"
    );
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(new_len),
        "a truncated rollout must be PARKED at the new EOF before release"
    );
    assert_eq!(
        seen.lock().await.get(&path),
        Some(&false),
        "the claim must still be RELEASED (kept known)"
    );

    // Straggler walk after the release: no replay of existing bytes.
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

/// Direct pin of the park primitive (review round, lens-1 DEMONSTRATED: a
/// park-at-0 mutant survived the two lifecycle tests above — the drain's own
/// walk re-reads from 0 and advances the cursor to EOF itself, and the no-op
/// decoder makes the replay invisible). Seed a cursor ABOVE the file's
/// length and assert the park lands EXACTLY at the new EOF, plus the
/// negative branch: a cursor at/below the length is untouched.
#[tokio::test]
async fn park_if_truncated_below_cursor_lands_exactly_at_new_eof() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    std::fs::write(&path, vec![b'x'; 40]).unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::from([(path.clone(), 100u64)])));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let (tx, _rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);
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

    park_if_truncated_below_cursor(&path, &ctx).await;
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(40),
        "the park must land at the NEW EOF, not 0"
    );

    cursors.lock().await.insert(path.clone(), 10);
    park_if_truncated_below_cursor(&path, &ctx).await;
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(10),
        "a cursor at/below the file length must be untouched"
    );
}

/// `scan_root`'s read_dir Err arm: an unreadable/nonexistent watched root
/// latches the failure (`FailureLatch::on_failure()` true → warn once) and
/// discovers no sessions. A mutant that swallowed the Err and read an empty
/// dir would leave the latch quiet AND would never recover-report below.
#[tokio::test]
async fn scan_root_on_unreadable_root_latches_failure_and_emits_nothing() {
    let bad: PathBuf = std::env::temp_dir().join(format!("pixtuoid-no-such-{}", uuid_like()));
    assert!(!bad.exists(), "fixture: the bad root must not exist");
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(32);
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

    let mut health = FailureLatch::default();
    scan_root(&bad, t_decoders(), &ctx, &mut health).await;
    drop(tx);
    let events = drain_events(&mut rx);
    assert!(
        events.is_empty(),
        "an unreadable root discovers no sessions, got {events:?}"
    );
    // The Err arm fired on_failure() — so a FRESH on_failure() is now QUIET
    // (the latch is already in the failed state). A swallow-the-Err mutant
    // would never have set the failed state, so this on_failure() would
    // report true.
    assert!(
        !health.on_failure(),
        "scan_root's Err arm must have already latched the failure"
    );
}

/// `scan_root`'s recovery arm (line 54): read_dir succeeds after a prior
/// failure → `on_success()` true → "readable again". The bad→good
/// composition still registers the real transcript, and the latch's
/// recovery edge is consumed exactly once (a follow-up on_success is quiet).
#[tokio::test]
async fn scan_root_recovers_after_a_failed_root_reports_success_once() {
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(32);
    let source: Arc<str> = Arc::from("test");
    let live = Arc::new(Mutex::new(HashSet::new()));

    let good = tempfile::tempdir().unwrap();
    let real = good.path().join("real.jsonl");
    tokio::fs::write(&real, "{\"type\":\"assistant\",\"cwd\":\"/r\"}\n")
        .await
        .unwrap();

    let ctx = WatchCtx {
        source: &source,
        cursors: &cursors,
        seen: &seen,
        tx: &tx,
        window: Duration::from_secs(3600),
        live: &live,
    };

    let mut health = FailureLatch::default();
    // Pass 1: a bad root latches the failure.
    let bad: PathBuf = std::env::temp_dir().join(format!("pixtuoid-no-such-{}", uuid_like()));
    scan_root(&bad, t_decoders(), &ctx, &mut health).await;
    assert!(rx.try_recv().is_err(), "the bad root emits nothing");

    // Pass 2: the good root recovers — scan_root's Ok arm consumed the
    // latch's recovery edge AND the real transcript registered.
    scan_root(good.path(), t_decoders(), &ctx, &mut health).await;
    drop(tx);
    let events = drain_events(&mut rx);
    let expected = AgentId::from_parts("test", &default_id_from_path(&real));
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            AgentEvent::SessionStart { agent_id, .. } if *agent_id == expected
        )),
        "the recovered root must register the real transcript, got {events:?}"
    );
    // scan_root's Ok arm already called on_success() → a fresh on_success()
    // is now quiet (the recovery edge was consumed inside the good scan).
    assert!(
        !health.on_success(),
        "scan_root's Ok arm must have already reported the recovery"
    );
    // And a NEW failure after that recovery reports again (the latch is back
    // in the clean state, proving the recovery edge truly fired).
    assert!(
        health.on_failure(),
        "a failure after the consumed recovery must report again"
    );
}

/// `walk_jsonl`'s directory-recursion arm (lines 110-116): when the entry is
/// a real directory it read_dirs and Box::pins walk_jsonl over each child, so
/// a nested transcript registers and is tracked. A mutant that `return`ed on
/// is_dir without recursing would emit nothing.
#[tokio::test]
async fn walk_jsonl_recurses_into_a_subdirectory_and_registers_nested_transcripts() {
    let dir = tempfile::tempdir().unwrap();
    let day = dir.path().join("day");
    tokio::fs::create_dir_all(&day).await.unwrap();
    let nested = day.join("ses.jsonl");
    tokio::fs::write(&nested, "{\"type\":\"assistant\",\"cwd\":\"/r\"}\n")
        .await
        .unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    // Pass the DIRECTORY path to walk_jsonl — the recursion descends to the
    // nested transcript.
    let events = walk_once(
        dir.path(),
        Duration::from_secs(3600),
        t_ended,
        &cursors,
        &seen,
    )
    .await;
    let expected = AgentId::from_parts("test", &default_id_from_path(&nested));
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            AgentEvent::SessionStart { agent_id, .. } if *agent_id == expected
        )),
        "the nested transcript must register through the directory recursion, got {events:?}"
    );
    assert!(
        cursors.lock().await.contains_key(&nested),
        "the nested transcript's cursor must be tracked"
    );
}

/// `walk_jsonl`'s extension guard (lines 118-119): a recent, live file whose
/// extension is not `jsonl` is returned without tracking. A mutant dropping
/// the check would register/seed a foreign file.
#[tokio::test]
async fn walk_jsonl_skips_a_non_jsonl_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("foo.txt");
    // A recent, JSON-bearing, live file — only the extension disqualifies it.
    tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/r\"}\n")
        .await
        .unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_once(&path, Duration::from_secs(3600), t_ended, &cursors, &seen).await;
    assert!(
        events.is_empty(),
        "a non-.jsonl file must emit nothing, got {events:?}"
    );
    assert!(
        cursors.lock().await.is_empty(),
        "a non-.jsonl file must never be tracked"
    );
}

/// `walk_jsonl`'s symlink_metadata Err arm (line 102): a path that cannot be
/// stat'd (never created) returns immediately, emitting nothing and tracking
/// nothing. A mutant that unwrapped or proceeded past the stat would panic or
/// register a phantom.
#[tokio::test]
async fn walk_jsonl_on_a_missing_path_is_a_silent_no_op() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ghost.jsonl"); // never written
    assert!(!path.exists(), "fixture: the path must not exist");
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_once(&path, Duration::from_secs(3600), t_ended, &cursors, &seen).await;
    assert!(
        events.is_empty(),
        "a missing path must emit nothing, got {events:?}"
    );
    assert!(
        !cursors.lock().await.contains_key(&path),
        "a missing path must never be tracked"
    );
}

/// `walk_jsonl`'s truncation arm (lines 162-171): a KNOWN file whose stored
/// cursor exceeds the current file_len (a live truncate-rewrite resync) RESETS
/// the cursor to 0 and emits nothing this pass — distinct from the exit-path
/// park, which lands at file_len. A park-at-EOF mutant would leave the cursor
/// at file_len, not 0.
#[tokio::test]
async fn walk_jsonl_resets_cursor_to_zero_when_known_file_truncated_below_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resync.jsonl");
    tokio::fs::write(&path, "{\"type\":\"assistant\"}\n")
        .await
        .unwrap();
    let file_len = tokio::fs::metadata(&path).await.unwrap().len();
    assert!(
        file_len < 999,
        "fixture: the file must be shorter than the seeded cursor"
    );

    // KNOWN (cursor present) at a cursor far above the current file length,
    // and `seen` claimed so it takes the normal-walk truncation arm (the
    // first-sight gate is skipped for a known file).
    let cursors = Arc::new(Mutex::new(HashMap::from([(path.clone(), 999u64)])));
    let seen = Arc::new(Mutex::new(HashMap::from([(path.clone(), true)])));

    let events = walk_once(&path, Duration::from_secs(3600), t_ended, &cursors, &seen).await;
    assert!(
        events.is_empty(),
        "the truncation resync emits nothing this pass, got {events:?}"
    );
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(0),
        "the normal-walk truncation arm must RESET the cursor to 0 (not park at EOF)"
    );
}

/// `walk_jsonl`'s per-line decode-error arm (line 347): a line whose decoder
/// returns Err is warn-logged and skipped — the read continues and the cursor
/// still advances to the safe newline, so the benign line's first-sight
/// registration still fires. A `?`-propagating mutant would abort the whole
/// read, leaving the cursor short and dropping the registration.
#[tokio::test]
async fn walk_jsonl_skips_a_line_whose_decoder_errors_and_advances_cursor() {
    fn err_decode(_t: &str, _s: &str, v: serde_json::Value) -> Result<Vec<AgentEvent>> {
        if v.get("boom").is_some() {
            anyhow::bail!("boom");
        }
        Ok(vec![])
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("badline.jsonl");
    let body = "{\"boom\":1}\n{\"type\":\"assistant\",\"cwd\":\"/r\"}\n";
    tokio::fs::write(&path, body).await.unwrap();
    let file_len = body.len() as u64;
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_once_with(
        &path,
        Duration::from_secs(3600),
        err_decode,
        t_ended,
        &cursors,
        &seen,
    )
    .await;
    let expected = AgentId::from_parts("test", &default_id_from_path(&path));
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            AgentEvent::SessionStart { agent_id, .. } if *agent_id == expected
        )),
        "the erroring line is non-fatal; first-sight registration must still run, got {events:?}"
    );
    assert_eq!(
        cursors.lock().await.get(&path).copied(),
        Some(file_len),
        "the cursor must advance to EOF despite the decode error"
    );
}

/// `oversized_body` variant producing raw BYTES, so a non-UTF8 fragment can be
/// interleaved into the Task-scan window (a String can't hold `\xff`).
fn oversized_body_bytes(tail_chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut full = Vec::from(CC_HEAD_LINE_BYTES);
    while full.len() <= (1usize << 20) + 4096 {
        full.extend_from_slice(FILLER_LINE_BYTES);
    }
    for c in tail_chunks {
        full.extend_from_slice(c);
    }
    full
}

const CC_HEAD_LINE_BYTES: &[u8] = b"{\"type\":\"assistant\",\"cwd\":\"/repo/head\"}\n";
const FILLER_LINE_BYTES: &[u8] = b"{\"type\":\"assistant\"}\n";

/// `scan_pending_tasks`' line loop (lines 557-565): an empty line is skipped
/// and a non-UTF8 line is skipped before decode, so a corrupt fragment in the
/// oversized tail window can't break Task seeding — a complete dispatch after
/// them still seeds. A mutant that decoded the empty/non-utf8 line would panic
/// or drop out of the loop, losing the dispatch.
#[tokio::test]
async fn task_scan_skips_empty_and_non_utf8_lines_and_still_seeds_a_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deleg-garbage.jsonl");
    let full = oversized_body_bytes(&[
        b"\n".to_vec(),                      // empty line (561)
        b"\xff\xfe garbage \xff\n".to_vec(), // non-UTF8 line (564)
        cc_task_dispatch_line("tu_x").into_bytes(),
    ]);
    tokio::fs::write(&path, &full).await.unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_oversized_cc(&path, Duration::from_secs(3600), &cursors, &seen).await;
    assert_eq!(
        task_start_tuids(&events),
        vec!["tu_x".to_string()],
        "the garbage lines must be skipped and the valid dispatch still seed, got {events:?}"
    );
}

/// `scan_pending_tasks`' decode-error arm (lines 566-571): a line that parses
/// as JSON but whose decoder returns Err is debug-logged and skipped
/// (`continue`), not fatal to the rest of the Task scan — a valid dispatch
/// later in the window still seeds. A `?`/unwrap mutant would abort the scan
/// and seed nothing.
#[tokio::test]
async fn task_scan_skips_a_decoder_error_line_and_still_seeds_a_later_dispatch() {
    fn deco(t: &str, s: &str, v: serde_json::Value) -> Result<Vec<AgentEvent>> {
        if v.get("boom").is_some() {
            anyhow::bail!("x");
        }
        crate::source::claude_code::decode_cc_line(t, s, v)
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deleg-decode-err.jsonl");
    let full = oversized_body(&["{\"boom\":1}\n".to_string(), cc_task_dispatch_line("tu_y")]);
    tokio::fs::write(&path, &full).await.unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));

    let events = walk_once_with(
        &path,
        Duration::from_secs(3600),
        deco,
        t_ended,
        &cursors,
        &seen,
    )
    .await;
    assert_eq!(
        task_start_tuids(&events),
        vec!["tu_y".to_string()],
        "the decoder-error line must be skipped and the valid dispatch still seed, got {events:?}"
    );
}

#[tokio::test]
async fn deleted_gated_file_walk_evicts_its_cursor() {
    // A transcript deleted from disk (CC's 30-day cleanup) delivers one last
    // notify event for its path; that walk must retire the cursors entry —
    // otherwise every file ever sighted leaks a map entry for the process
    // lifetime (and stays a permanent re-vouch stat candidate).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gone.jsonl");
    let (cursors, seen) = gated_fixture(&path, "{\"type\":\"assistant\"}\n").await;
    assert!(cursors.lock().await.contains_key(&path));

    tokio::fs::remove_file(&path).await.unwrap();
    let events = walk_once(&path, Duration::from_secs(60), t_ended, &cursors, &seen).await;
    assert!(
        events.is_empty(),
        "a deleted path emits nothing: {events:?}"
    );
    assert!(
        !cursors.lock().await.contains_key(&path),
        "the cursor entry of a deleted file must be evicted"
    );
}

#[tokio::test]
async fn deleted_registered_file_walk_evicts_cursor_and_claim() {
    // Same eviction for a REGISTERED (seen-claimed) file: a recreated
    // same-path file must re-enter through the first-sight gate, not resume
    // a dead claim.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gone-live.jsonl");
    tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/r\"}\n")
        .await
        .unwrap();
    let cursors = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let events = walk_once(&path, Duration::from_secs(60), t_ended, &cursors, &seen).await;
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
        "fixture must register first, got {events:?}"
    );

    tokio::fs::remove_file(&path).await.unwrap();
    walk_once(&path, Duration::from_secs(60), t_ended, &cursors, &seen).await;
    assert!(!cursors.lock().await.contains_key(&path));
    assert!(
        !seen.lock().await.contains_key(&path),
        "the first-sight claim of a deleted file must be evicted"
    );
}

#[tokio::test]
async fn revouch_pass_prunes_deleted_files_from_cursors() {
    // The re-vouch sweep already stats every gated candidate each scan pass;
    // a NotFound stat IS the "this transcript was deleted" observation.
    // Pruning there is the backstop for a lost notify delete event —
    // otherwise the entry stays a permanent candidate (a failed stat per
    // pass, forever).
    let dir = tempfile::tempdir().unwrap();
    let gone = dir.path().join("deleted.jsonl");
    let cursors: Arc<Mutex<HashMap<PathBuf, u64>>> = Arc::new(Mutex::new(HashMap::new()));
    cursors.lock().await.insert(gone.clone(), 42);
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let live: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(
        std::iter::once("someone-alive".to_string()).collect(),
    ));
    let (tx, _rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);
    let source: Arc<str> = Arc::from("test");
    let decoders = SourceDecoders {
        decode_line: t_decode,
        derive_label: t_label,
        check_ended: t_ended,
        id_derive: default_id_from_path,
        path_filter: accept_all_paths,
        cwd_derive: no_cwd_from_path,
    };
    let ctx = WatchCtx {
        source: &source,
        cursors: &cursors,
        seen: &seen,
        tx: &tx,
        window: Duration::from_secs(60),
        live: &live,
    };
    revouch_gated_files(decoders, &ctx).await;
    assert!(
        !cursors.lock().await.contains_key(&gone),
        "a NotFound re-vouch candidate must be pruned from cursors"
    );
}

/// A cheap unique-ish suffix for nonexistent-path fixtures (no uuid dep here).
fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{nanos}-{:p}", &nanos)
}
