use std::time::{Duration, SystemTime};

use filetime::{set_file_mtime, FileTime};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use pixtuoid_core::source::jsonl::ProbeSnapshot;
use pixtuoid_core::source::AgentEvent;
use pixtuoid_core::source::Transport;
use pixtuoid_core::AgentId;

use crate::{
    backdate, cc_session_start_line, cc_tool_use_line, cc_watcher, vouch_snapshot, write_lines,
};

/// A HEALTHY snapshot vouching `ids` with every id bound to `pid` — the shape
/// the instant-exit (#223 rung 2) tests need: the real probes bind each
/// session id to its owning OS pid.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn vouch_snapshot_with_pid(ids: &[&str], pid: i32) -> Option<ProbeSnapshot> {
    Some(ProbeSnapshot {
        pid_of: ids.iter().map(|s| (s.to_string(), pid)).collect(),
    })
}

/// #220: the probe is ONGOING liveness, not just admission — after each probe
/// refresh (the initial seed makes this fast; the 60s poll repeats it) the
/// watcher emits a `ProofOfLife` for every vouched id so the reducer can keep
/// the slot exempt from staleness sweeps while the process lives.
#[tokio::test]
async fn watcher_emits_proof_of_life_for_probe_live_ids() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-pol");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let uuid = "01000000-0000-7000-8000-0000000000ab";
    let stale = project_dir.join(format!("{uuid}.jsonl"));
    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": uuid,
        "cwd": "/repo",
        "message": { "role": "assistant", "content": [] }
    });
    tokio::fs::write(&stale, format!("{line}\n")).await.unwrap();
    let backdated = FileTime::from_system_time(SystemTime::now() - Duration::from_secs(3600));
    set_file_mtime(&stale, backdated).unwrap();

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone())
        .with_initial_window(Duration::from_secs(60))
        .with_liveness_probe(std::sync::Arc::new(move || vouch_snapshot(&[uuid])));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    let expected = AgentId::from_parts("claude-code", uuid);
    let mut pol = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((t, AgentEvent::ProofOfLife { agent_id }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            pol = Some((t, agent_id));
            break;
        }
    }
    assert_eq!(
        pol,
        Some((Transport::Jsonl, expected)),
        "each probe refresh must emit a ProofOfLife per vouched id"
    );
    handle.abort();
}

/// #220 follow-up: the 60s-poll arm must REFRESH the probe snapshot
/// (`*live = probe()`) and RE-EMIT `ProofOfLife` — not only the initial seed.
/// The probe starts EMPTY so the initial seed + 250ms rescan gate the stale
/// file; the session id becomes probe-live only afterwards, so the SessionStart
/// can ONLY come from a poll-arm snapshot refresh (re-vouch sweep), and the
/// ProofOfLife only from the poll-arm emission. Uses the `with_poll_interval`
/// test seam — the production 60s cadence makes the poll arm untestable.
#[tokio::test]
async fn poll_arm_refreshes_probe_snapshot_and_reemits_proof_of_life() {
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-poll-pol");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let uuid = "01000000-0000-7000-8000-0000000000ac";
    let stale = project_dir.join(format!("{uuid}.jsonl"));
    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": uuid,
        "cwd": "/repo",
        "message": { "role": "assistant", "content": [] }
    });
    tokio::fs::write(&stale, format!("{line}\n")).await.unwrap();
    let backdated = FileTime::from_system_time(SystemTime::now() - Duration::from_secs(3600));
    set_file_mtime(&stale, backdated).unwrap();

    let live: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let probe_view = live.clone();
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone())
        .with_initial_window(Duration::from_secs(60))
        .with_poll_interval(Duration::from_millis(100))
        .with_liveness_probe(Arc::new(move || {
            // The live id set IS `pid_of`'s keys; this test only exercises
            // admission + proof-of-life, so bind each vouched id to this live
            // process's pid (a placeholder that never instant-exits).
            let pid = std::process::id() as i32;
            Some(ProbeSnapshot {
                pid_of: probe_view
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|s| (s.clone(), pid))
                    .collect(),
            })
        }));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    // While the snapshot is empty (initial seed, rescan, first polls), the
    // first-sight gate must hold: no SessionStart, no ProofOfLife.
    let quiet_until = tokio::time::Instant::now() + Duration::from_millis(300);
    while tokio::time::Instant::now() < quiet_until {
        if let Ok(Some((_, ev))) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
        {
            assert!(
                !matches!(
                    ev,
                    AgentEvent::SessionStart { .. } | AgentEvent::ProofOfLife { .. }
                ),
                "the gate must hold while the probe snapshot is empty, got {ev:?}"
            );
        }
    }

    // The session becomes probe-live AFTER startup (e.g. pixtuoid attached
    // before CC's registry entry landed). Only a poll-arm refresh can see it.
    live.lock().unwrap().insert(uuid.to_string());

    let expected = AgentId::from_parts("claude-code", uuid);
    let mut got_start = false;
    let mut got_pol = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline && !(got_start && got_pol) {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((_, AgentEvent::SessionStart { agent_id, .. }))) if agent_id == expected => {
                got_start = true;
            }
            Ok(Some((Transport::Jsonl, AgentEvent::ProofOfLife { agent_id })))
                if agent_id == expected =>
            {
                got_pol = true;
            }
            _ => {}
        }
    }
    assert!(
        got_start,
        "the poll-arm snapshot refresh must re-vouch and admit the gated file"
    );
    assert!(
        got_pol,
        "the poll arm must re-emit ProofOfLife for every vouched id"
    );
    handle.abort();
}

// ── Negative vouch (#223): vouched-then-gone is a high-confidence exit ──────

/// Shared fixture for the negative-vouch + instant-exit tests: a stale
/// transcript admitted via a MUTABLE probe (the test flips its return value
/// mid-run; `initial` is the snapshot the probe starts on — with or without a
/// pid binding), driven by a running watcher with fast poll + a test-tuned
/// confirmation span. Returns the probe handle, the event receiver, the
/// transcript path, and the watcher task — after asserting the probe-vouched
/// admission already happened.
async fn admitted_with_mutable_probe(
    projects_root: std::path::PathBuf,
    uuid: &'static str,
    min_span: Duration,
    initial: Option<ProbeSnapshot>,
) -> (
    std::sync::Arc<std::sync::Mutex<Option<ProbeSnapshot>>>,
    mpsc::Receiver<(Transport, AgentEvent)>,
    std::path::PathBuf,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let project_dir = projects_root.join("proj-nvouch");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let stale = project_dir.join(format!("{uuid}.jsonl"));
    write_lines(&stale, &[cc_session_start_line(uuid, "/repo")]).await;
    backdate(&stale, 7200);

    let probe_state: std::sync::Arc<std::sync::Mutex<Option<ProbeSnapshot>>> =
        std::sync::Arc::new(std::sync::Mutex::new(initial));
    let probe_view = probe_state.clone();
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let watcher = cc_watcher(projects_root)
        .with_initial_window(Duration::from_secs(60))
        .with_poll_interval(Duration::from_millis(100))
        .with_negative_vouch_min_span(min_span)
        .with_liveness_probe(std::sync::Arc::new(move || {
            probe_view.lock().unwrap().clone()
        }));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    let expected = AgentId::from_parts("claude-code", uuid);
    let mut registered = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { agent_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if agent_id == expected {
                registered = true;
                break;
            }
        }
    }
    assert!(
        registered,
        "the probe-vouched stale transcript must register"
    );
    (probe_state, rx, stale, handle)
}

/// Drain `rx` for `window`, panicking if a `SessionEnd` for `expected`
/// arrives — the quiet-window assertion the no-exit tests share.
async fn assert_no_session_end_within(
    rx: &mut mpsc::Receiver<(Transport, AgentEvent)>,
    expected: AgentId,
    window: Duration,
    why: &str,
) {
    let quiet_until = tokio::time::Instant::now() + window;
    while tokio::time::Instant::now() < quiet_until {
        if let Ok(Some((_, AgentEvent::SessionEnd { agent_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
        {
            assert_ne!(agent_id, expected, "{why}");
        }
    }
}

/// #223 rung 2 — THE negative-vouch exit: a previously-vouched session id
/// missing from two healthy snapshots ≥ the confirmation span apart gets the
/// `SessionEnd` its CLI never wrote (Codex has no exit signal at all; CC's
/// hook is best-effort), instead of ghosting until the 10–30 min stale-sweep.
/// Also pins the self-heal: the confirmation un-claims `seen`, so a LATER
/// append (a resumed session) re-registers through `emit_first_sight`.
#[tokio::test]
async fn negative_vouch_emits_session_end_after_sustained_disappearance() {
    let dir = TempDir::new().unwrap();
    let uuid = "01000000-0000-7000-8000-0000000000ad";
    let (probe_state, mut rx, transcript, handle) = admitted_with_mutable_probe(
        dir.path().to_path_buf(),
        uuid,
        Duration::from_millis(300),
        vouch_snapshot(&[uuid]),
    )
    .await;
    let expected = AgentId::from_parts("claude-code", uuid);

    // The owning process exits: the probe stays HEALTHY but stops vouching.
    *probe_state.lock().unwrap() = vouch_snapshot(&[]);

    let mut ended = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((
            Transport::Jsonl,
            AgentEvent::SessionEnd {
                agent_id,
                as_child: false,
            },
        ))) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if agent_id == expected {
                ended = true;
                break;
            }
        }
    }
    assert!(
        ended,
        "two healthy snapshots ≥ the span apart without the vouch must emit SessionEnd"
    );

    // Self-heal: the un-claimed `seen` lets a resumed session re-register on
    // its next append (mirrors the decoded-SessionEnd revive).
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(
        format!(
            "{}\n",
            cc_tool_use_line(uuid, "/repo", "tu_resume", "Bash", serde_json::json!({}))
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut restarted = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { agent_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if agent_id == expected {
                restarted = true;
                break;
            }
        }
    }
    assert!(
        restarted,
        "an append after the negative-vouch exit must re-register the session (seen un-claim)"
    );
    handle.abort();
}

/// One missed snapshot is NOT an exit: Codex briefly drops and reopens its
/// rollout fd on a write failure, so a vouch that disappears and re-appears
/// within the confirmation span must cancel the pending miss window — no
/// SessionEnd, even well past the span.
#[tokio::test]
async fn one_missed_snapshot_does_not_end_the_session() {
    let dir = TempDir::new().unwrap();
    let uuid = "01000000-0000-7000-8000-0000000000ae";
    // The confirmation span dwarfs the 250ms drop below (via the
    // with_negative_vouch_min_span seam, not a bigger sleep): the re-vouch
    // must land INSIDE the span even when load stretches the sleep — a 600ms
    // span left only ~350ms of real-time slack and could flake into a
    // confirmed exit on a busy machine.
    let span = Duration::from_millis(2500);
    let (probe_state, mut rx, _transcript, handle) = admitted_with_mutable_probe(
        dir.path().to_path_buf(),
        uuid,
        span,
        vouch_snapshot(&[uuid]),
    )
    .await;
    let expected = AgentId::from_parts("claude-code", uuid);

    // Brief drop: a few healthy-but-empty snapshots open the miss window...
    *probe_state.lock().unwrap() = vouch_snapshot(&[]);
    tokio::time::sleep(Duration::from_millis(250)).await;
    // ...then the vouch re-appears INSIDE the span — the window must cancel.
    *probe_state.lock().unwrap() = vouch_snapshot(&[uuid]);

    // The quiet window reaches PAST the span (+1s): a merely not-yet-expired
    // (uncancelled) miss window would confirm inside it, so this proves the
    // cancellation, not just that the span hasn't elapsed.
    assert_no_session_end_within(
        &mut rx,
        expected,
        span + Duration::from_millis(1000),
        "a vouch re-appearing within the span must cancel the miss window — no SessionEnd",
    )
    .await;
    handle.abort();
}

/// A probe FAILURE (`None`) is not an observation: it must neither open nor
/// age a miss window (no SessionEnd ever — the span here is tiny, so treating
/// None as an empty snapshot WOULD confirm within the quiet window), and it
/// must not disturb admission state (the previous snapshot keeps vouching; a
/// fresh append still walks normally).
#[tokio::test]
async fn probe_failure_changes_nothing() {
    let dir = TempDir::new().unwrap();
    let uuid = "01000000-0000-7000-8000-0000000000af";
    let (probe_state, mut rx, transcript, handle) = admitted_with_mutable_probe(
        dir.path().to_path_buf(),
        uuid,
        Duration::from_millis(200),
        vouch_snapshot(&[uuid]),
    )
    .await;
    let expected = AgentId::from_parts("claude-code", uuid);

    // The probe itself breaks (registry dir unreadable, proc table down).
    *probe_state.lock().unwrap() = None;

    assert_no_session_end_within(
        &mut rx,
        expected,
        Duration::from_millis(1500),
        "a probe failure must never confirm an exit — None is not an observation",
    )
    .await;

    // Admission state intact: the session is still registered, so a fresh
    // append decodes through the normal walk (no re-gate, no resurrection).
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(
        format!(
            "{}\n",
            cc_tool_use_line(
                uuid,
                "/repo",
                "tu_after_failure",
                "Bash",
                serde_json::json!({})
            )
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_activity = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::ActivityStart { tool_use_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if tool_use_id.as_deref() == Some("tu_after_failure") {
                got_activity = true;
                break;
            }
        }
    }
    assert!(
        got_activity,
        "a fresh append must still walk normally while the probe is failing"
    );
    handle.abort();
}

/// A probe that PANICS (not merely returns `None`) must not crash the watcher:
/// `spawn_blocking`'s `JoinError` folds into the same fail-safe as a `None` —
/// the previous snapshot keeps vouching and no exit is confirmed. Covers the
/// `Err(join_err) => warn!` arm that the `None` path never reaches.
#[tokio::test]
async fn probe_panic_changes_nothing() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = TempDir::new().unwrap();
    let uuid = "01000000-0000-7000-8000-0000000000b1";
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-panic");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let stale = project_dir.join(format!("{uuid}.jsonl"));
    write_lines(&stale, &[cc_session_start_line(uuid, "/repo")]).await;
    backdate(&stale, 7200);

    // Healthy until `boom` flips; then every probe call panics inside the
    // blocking task.
    let boom = std::sync::Arc::new(AtomicBool::new(false));
    let probe_boom = boom.clone();
    let snap = vouch_snapshot(&[uuid]);
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let watcher = cc_watcher(projects_root)
        .with_initial_window(Duration::from_secs(60))
        .with_poll_interval(Duration::from_millis(100))
        .with_liveness_probe(std::sync::Arc::new(move || {
            if probe_boom.load(Ordering::SeqCst) {
                panic!("probe boom");
            }
            snap.clone()
        }));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    let expected = AgentId::from_parts("claude-code", uuid);
    let mut registered = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { agent_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if agent_id == expected {
                registered = true;
                break;
            }
        }
    }
    assert!(
        registered,
        "the probe-vouched stale transcript must register while the probe is healthy"
    );

    // The probe now panics on every poll — the watcher must survive and never
    // confirm an exit (a panic is not an observation, just a harsher failure).
    boom.store(true, Ordering::SeqCst);
    assert_no_session_end_within(
        &mut rx,
        expected,
        Duration::from_millis(1500),
        "a probe panic must fold into the fail-safe, never confirm an exit",
    )
    .await;
    assert!(
        !handle.is_finished(),
        "a probe panic must not crash the watcher task"
    );
    handle.abort();
}

// ── Instant exit (#223 rung 2): a bound pid dying IS the session's end ──────

/// THE instant-exit pin: a probe snapshot binds the vouched session id to its
/// owning OS pid (`ProbeSnapshot::pid_of`); when that process dies, the
/// kernel watch (kqueue NOTE_EXIT / pidfd+poll) emits the SessionEnd within
/// milliseconds. The negative-vouch span stays at its production 60s DEFAULT,
/// so a SessionEnd inside the 5s window can ONLY have come from the
/// instant-exit path. Also pins the shared exit path's `seen` un-claim: a
/// later append re-registers the session.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn instant_exit_emits_session_end_when_bound_pid_dies() {
    let dir = TempDir::new().unwrap();
    let uuid = "01000000-0000-7000-8000-0000000000b0";
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    let (probe_state, mut rx, transcript, handle) = admitted_with_mutable_probe(
        dir.path().to_path_buf(),
        uuid,
        Duration::from_secs(60), // production default — the fast exit must not come from the negative vouch
        vouch_snapshot_with_pid(&[uuid], pid),
    )
    .await;
    let expected = AgentId::from_parts("claude-code", uuid);

    // The process is about to die — flip the probe FIRST (a real probe stops
    // vouching a dead pid) so the re-vouch sweep can't re-admit the ended
    // session behind the test's back. With the 60s span, this flip alone
    // cannot produce a SessionEnd inside the assertion window.
    *probe_state.lock().unwrap() = vouch_snapshot(&[]);
    let _ = child.kill();
    let _ = child.wait();

    let mut ended = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((
            Transport::Jsonl,
            AgentEvent::SessionEnd {
                agent_id,
                as_child: false,
            },
        ))) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if agent_id == expected {
                ended = true;
                break;
            }
        }
    }
    assert!(
        ended,
        "a bound pid dying must SessionEnd within seconds (instant exit), \
         not wait out the 60s negative vouch"
    );

    // Shared exit path (emit_session_exit): the `seen` un-claim lets a later
    // append re-register — identical self-heal to the negative-vouch exit.
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(
        format!(
            "{}\n",
            cc_tool_use_line(uuid, "/repo", "tu_resume_2", "Bash", serde_json::json!({}))
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut restarted = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { agent_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if agent_id == expected {
                restarted = true;
                break;
            }
        }
    }
    assert!(
        restarted,
        "an append after the instant exit must re-register the session (seen un-claim)"
    );
    handle.abort();
}

/// The pid binding must be UNBOUND when the negative vouch confirms an id:
/// a codex-style process owns many rollouts, so a session can end (rollout
/// fd closed → vouch gone) while its OS process lives on — when that process
/// finally dies, the exit event must find no binding and emit NOTHING (no
/// second SessionEnd for the already-ended id).
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn negative_vouch_confirm_unbinds_pid_so_a_later_exit_is_quiet() {
    let dir = TempDir::new().unwrap();
    let uuid = "01000000-0000-7000-8000-0000000000b1";
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    let (probe_state, mut rx, _transcript, handle) = admitted_with_mutable_probe(
        dir.path().to_path_buf(),
        uuid,
        Duration::from_millis(300),
        vouch_snapshot_with_pid(&[uuid], pid),
    )
    .await;
    let expected = AgentId::from_parts("claude-code", uuid);

    // The session ends while its process lives: the probe stays healthy but
    // stops vouching → the negative vouch confirms (tiny span).
    *probe_state.lock().unwrap() = vouch_snapshot(&[]);
    let mut ended = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((
            Transport::Jsonl,
            AgentEvent::SessionEnd {
                agent_id,
                as_child: false,
            },
        ))) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if agent_id == expected {
                ended = true;
                break;
            }
        }
    }
    assert!(ended, "the negative vouch must confirm the first exit");

    // NOW the process dies. Its exit event must find no binding (the confirm
    // unbound the id) — quiet, no panic.
    let _ = child.kill();
    let _ = child.wait();
    assert_no_session_end_within(
        &mut rx,
        expected,
        Duration::from_millis(1500),
        "a process exit after the id was negative-vouch-confirmed must not re-emit SessionEnd",
    )
    .await;
    handle.abort();
}

/// The instant-exit arm must purge the dead id from the shared admission set:
/// `live` is only rewritten by a HEALTHY probe refresh, so without the purge
/// a probe FAILURE (`None`) pass right after the exit keeps the stale
/// snapshot vouching the dead id — the re-vouch sweep re-admits the parked
/// transcript (cursor reset to 0) and replays it into a phantom SessionStart
/// for the session whose SessionEnd just emitted, unreachable by every fast
/// rung (pid unbound, vouch forgotten).
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn instant_exit_under_probe_failure_does_not_resurrect_the_session() {
    let dir = TempDir::new().unwrap();
    let uuid = "01000000-0000-7000-8000-0000000000b2";
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    let (probe_state, mut rx, _transcript, handle) = admitted_with_mutable_probe(
        dir.path().to_path_buf(),
        uuid,
        Duration::from_secs(60), // production span — the negative vouch stays out of the picture
        vouch_snapshot_with_pid(&[uuid], pid),
    )
    .await;
    let expected = AgentId::from_parts("claude-code", uuid);

    // The probe breaks FIRST (`None` = enumeration failure: `live` keeps the
    // last healthy snapshot, which still vouches the id)...
    *probe_state.lock().unwrap() = None;
    // ...then the bound process dies → the instant exit emits SessionEnd.
    let _ = child.kill();
    let _ = child.wait();

    let mut ended = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((
            Transport::Jsonl,
            AgentEvent::SessionEnd {
                agent_id,
                as_child: false,
            },
        ))) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if agent_id == expected {
                ended = true;
                break;
            }
        }
    }
    assert!(ended, "the instant exit must emit the SessionEnd");

    // Scan passes keep running against the failing probe (100ms poll). The
    // stale snapshot must NOT let the re-vouch sweep resurrect the session.
    let quiet_until = tokio::time::Instant::now() + Duration::from_millis(1500);
    while tokio::time::Instant::now() < quiet_until {
        if let Ok(Some((_, AgentEvent::SessionStart { agent_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
        {
            assert_ne!(
                agent_id, expected,
                "a probe-failure pass after the instant exit must not mint a phantom SessionStart"
            );
        }
    }
    handle.abort();
}

/// An id that REBINDS to a new pid (a codex `resume` of the same rollout in
/// process B while process A still lives) must MIGRATE between pid sets:
/// the OLD pid's later death must not instant-exit the session now alive
/// under the new pid, and the NEW pid's death must (the binding moved, it
/// wasn't dropped).
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn rebound_session_survives_old_pid_death_and_follows_the_new_pid() {
    let dir = TempDir::new().unwrap();
    let uuid = "01000000-0000-7000-8000-0000000000b3";
    let mut old = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let mut new = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let old_pid = old.id() as i32;
    let new_pid = new.id() as i32;
    let (probe_state, mut rx, _transcript, handle) = admitted_with_mutable_probe(
        dir.path().to_path_buf(),
        uuid,
        Duration::from_secs(60), // production span — only the instant-exit rung is in play
        vouch_snapshot_with_pid(&[uuid], old_pid),
    )
    .await;
    let expected = AgentId::from_parts("claude-code", uuid);

    // The session rebinds: later healthy snapshots observe the id under the
    // NEW pid. Give the 100ms poll a few passes to fold the migration.
    *probe_state.lock().unwrap() = vouch_snapshot_with_pid(&[uuid], new_pid);
    tokio::time::sleep(Duration::from_millis(700)).await;

    // The OLD process dies — its stale binding must not end the live session.
    let _ = old.kill();
    let _ = old.wait();
    assert_no_session_end_within(
        &mut rx,
        expected,
        Duration::from_millis(1500),
        "the old pid's death must not end a session that rebound to a new pid",
    )
    .await;

    // The NEW process dying IS the session's end.
    let _ = new.kill();
    let _ = new.wait();
    let mut ended = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((
            Transport::Jsonl,
            AgentEvent::SessionEnd {
                agent_id,
                as_child: false,
            },
        ))) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if agent_id == expected {
                ended = true;
                break;
            }
        }
    }
    assert!(
        ended,
        "the migrated binding must follow the new pid — its death is the session's instant exit"
    );
    handle.abort();
}
