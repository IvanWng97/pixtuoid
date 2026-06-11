use std::time::{Duration, SystemTime};

use filetime::{set_file_mtime, FileTime};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use pixtuoid_core::source::antigravity::AntigravitySource;
use pixtuoid_core::source::claude_code::{
    cc_derive_label, cc_id_from_path, cc_session_ended, decode_cc_line, ClaudeCodeSource,
};
use pixtuoid_core::source::codex::CodexSource;
use pixtuoid_core::source::jsonl::{
    force_polling_backend_for_tests, ChildEndUnclaims, JsonlWatcher, ProbeSnapshot,
};
use pixtuoid_core::source::AgentEvent;
use pixtuoid_core::source::Source;
use pixtuoid_core::source::Transport;
use pixtuoid_core::AgentId;

/// Run every watcher test on a fast `PollWatcher` backend instead of the native
/// FSEvents/inotify watcher — events arrive in ~25ms and there's no real
/// FSEvents stream to set up/tear down (that teardown was tens of seconds per
/// test). Idempotent / set-once. Call at the top of any test that drives a real
/// `JsonlWatcher`/`Source::run`.
fn fast_watch() {
    force_polling_backend_for_tests(Duration::from_millis(25));
}

/// A HEALTHY probe snapshot vouching exactly `ids`. No pid binding — these
/// tests fake the probe closure; the id→pid join is unit-tested at the source
/// level (`claude_code::liveness_tests`, `codex::tests`).
fn vouch_snapshot(ids: &[&str]) -> Option<ProbeSnapshot> {
    Some(ProbeSnapshot {
        ids: ids.iter().map(|s| s.to_string()).collect(),
        pid_of: std::collections::HashMap::new(),
    })
}

/// A HEALTHY snapshot vouching `ids` with every id bound to `pid` — the shape
/// the instant-exit (#223 rung 2) tests need: the real probes bind each
/// session id to its owning OS pid.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn vouch_snapshot_with_pid(ids: &[&str], pid: i32) -> Option<ProbeSnapshot> {
    Some(ProbeSnapshot {
        ids: ids.iter().map(|s| s.to_string()).collect(),
        pid_of: ids.iter().map(|s| (s.to_string(), pid)).collect(),
    })
}

fn cc_watcher(root: std::path::PathBuf) -> JsonlWatcher {
    fast_watch();
    // Mirror ClaudeCodeSource::run: CC keys on the session UUID (filename stem),
    // not the raw path, so hook↔JSONL coalesce and the subagent→parent link
    // survives a git-worktree cwd-split.
    JsonlWatcher::new(
        root,
        "claude-code".to_string(),
        decode_cc_line,
        cc_derive_label,
        cc_session_ended,
    )
    .with_id_deriver(cc_id_from_path)
}

#[tokio::test]
async fn watcher_emits_session_start_then_activity_for_tool_use() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-x");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-abc.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    let start_line = serde_json::json!({
        "type": "system",
        "subtype": "session_start",
        "sessionId": "ses-abc",
        "cwd": "/repo"
    });
    f.write_all(format!("{start_line}\n").as_bytes())
        .await
        .unwrap();
    let assistant_line = serde_json::json!({
        "type": "assistant",
        "sessionId": "ses-abc",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_1", "name": "Bash",
                  "input": { "command": "ls" } }
            ]
        }
    });
    f.write_all(format!("{assistant_line}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut start_id = None;
    let mut activity_id = None;
    let mut start_transport = Transport::Hook;
    let mut activity_transport = Transport::Hook;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((t, AgentEvent::SessionStart { agent_id, .. }))) => {
                start_id = Some(agent_id);
                start_transport = t;
            }
            Ok(Some((t, AgentEvent::ActivityStart { agent_id, .. }))) => {
                activity_id = Some(agent_id);
                activity_transport = t;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
        if start_id.is_some() && activity_id.is_some() {
            break;
        }
    }
    let start_id = start_id.expect("expected SessionStart from JSONL watcher");
    let activity_id = activity_id.expect("expected ActivityStart from JSONL watcher");
    // The SessionStart key (id_derive) and the per-line decode key
    // (transcript_path_str) are computed at two different walk_jsonl sites —
    // they must agree or every JSONL event lands on a phantom id (the raw
    // string diverged from the normalized one on the windows runner).
    assert_eq!(
        start_id, activity_id,
        "SessionStart and per-line events must share one AgentId"
    );
    assert_eq!(start_transport, Transport::Jsonl);
    assert_eq!(activity_transport, Transport::Jsonl);
    handle.abort();
}

#[tokio::test]
async fn watcher_does_not_consume_partial_trailing_line() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-x");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-abc.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // First write: a complete line + a partial line (no trailing \n).
    let complete = serde_json::json!({
        "type": "assistant",
        "sessionId": "ses-abc",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_1", "name": "Bash", "input": { "command": "ls" } }
            ]
        }
    });
    let partial_head = r#"{"type":"assistant","sessionId":"ses-abc","cwd":"/repo","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_2","name":"Read","input":{"file_path":"/etc/host"#;
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(format!("{complete}\n{partial_head}").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    // We should see the SessionStart + ActivityStart for tu_1, but NOT for tu_2.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut seen_tool_use_ids: Vec<String> = Vec::new();
    while let Ok(Some((_t, ev))) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
    {
        if let AgentEvent::ActivityStart {
            tool_use_id: Some(id),
            ..
        } = ev
        {
            seen_tool_use_ids.push(id);
        }
    }
    assert!(
        seen_tool_use_ids.contains(&"tu_1".to_string()),
        "expected tu_1 from complete line, got {seen_tool_use_ids:?}"
    );
    assert!(
        !seen_tool_use_ids.contains(&"tu_2".to_string()),
        "tu_2 came from a partial line and must not be emitted yet"
    );

    // Now complete tu_2 by appending the rest of the line. tu_2 should appear.
    let partial_tail = "s\"}}]}}\n";
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(partial_tail.as_bytes()).await.unwrap();
    f.flush().await.unwrap();
    drop(f);

    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut got_tu_2 = false;
    while let Ok(Some((_t, ev))) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
    {
        if let AgentEvent::ActivityStart { tool_use_id, .. } = ev {
            if tool_use_id.as_deref() == Some("tu_2") {
                got_tu_2 = true;
            }
        }
    }
    assert!(
        got_tu_2,
        "tu_2 should appear after partial line is completed"
    );

    handle.abort();
}

/// On startup, the watcher must NOT emit SessionStart for every historical
/// .jsonl on disk. With small `max_desks` this would saturate desks with
/// long-dead sessions and starve the user's currently-active session.
/// Files older than the initial-window are seeded with cursor=file_len and
/// left out of the SessionStart stream until they next get written to.
#[tokio::test]
async fn watcher_skips_session_start_for_stale_files_on_startup() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-stale");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    // Pre-existing stale transcript (mtime backdated 1 hour).
    let stale = project_dir.join("old.jsonl");
    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": "old",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_old", "name": "Bash",
                  "input": { "command": "ls" } }
            ]
        }
    });
    tokio::fs::write(&stale, format!("{line}\n")).await.unwrap();
    let backdated = FileTime::from_system_time(SystemTime::now() - Duration::from_secs(3600));
    set_file_mtime(&stale, backdated).unwrap();

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone()).with_initial_window(Duration::from_secs(60));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    // Give the initial scan a moment to run.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut events = Vec::new();
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
        events.push(ev);
    }
    assert!(
        events.is_empty(),
        "stale file must not produce events on startup, got {events:?}"
    );
    handle.abort();
}

/// T4: a stale-mtime transcript whose session id the first-party liveness
/// probe vouches for (CC's `~/.claude/sessions/<pid>.json` registry) must
/// register on startup — mtime is only a liveness proxy; a long-idle or
/// delegating session writes nothing for hours while its process is alive.
#[tokio::test]
async fn watcher_registers_stale_file_when_probe_says_live() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-idle-live");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let uuid = "01000000-0000-7000-8000-0000000000aa";
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
    let mut start_id = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { agent_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            start_id = Some(agent_id);
            break;
        }
    }
    assert_eq!(
        start_id,
        Some(expected),
        "a probe-live stale transcript must register on startup"
    );
    handle.abort();
}

/// Codex twin of T4: the Codex liveness probe (`live_codex_rollout_ids`)
/// returns ids in `codex_id_from_path` space — the rollout-filename UUID —
/// so a stale rollout the probe vouches for must register through the same
/// `with_liveness_probe` seam. A FAKE probe closure stands in for the real
/// open-FD enumeration (that half is unit-tested in `source::fd_probe` /
/// `source::codex`); this pins the id-space JOIN: probe ids and the watcher's
/// `IdDeriver` agree, or every vouched rollout would stay gated.
#[tokio::test]
async fn codex_watcher_registers_stale_rollout_when_probe_says_live() {
    fast_watch();
    use pixtuoid_core::source::codex::{codex_id_from_path, decode_codex_line, derive_codex_label};

    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    // Real rollout layout: YYYY/MM/DD below the sessions root.
    let day_dir = root.join("2026").join("06").join("10");
    tokio::fs::create_dir_all(&day_dir).await.unwrap();
    let uuid = "019e7762-9ded-7e33-be41-946ecf105bf4";
    let rollout = day_dir.join(format!("rollout-2026-06-10T08-00-00-{uuid}.jsonl"));
    let meta = serde_json::json!({
        "type": "session_meta",
        "payload": { "id": uuid, "cwd": "/Users/me/dotfiles" }
    });
    tokio::fs::write(&rollout, format!("{meta}\n"))
        .await
        .unwrap();
    let backdated = FileTime::from_system_time(SystemTime::now() - Duration::from_secs(3600));
    set_file_mtime(&rollout, backdated).unwrap();

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = JsonlWatcher::new(
        root.clone(),
        "codex".to_string(),
        decode_codex_line,
        derive_codex_label,
        |_t| false,
    )
    .with_id_deriver(codex_id_from_path)
    .with_initial_window(Duration::from_secs(60))
    .with_liveness_probe(std::sync::Arc::new(move || vouch_snapshot(&[uuid])));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    let expected = AgentId::from_parts("codex", uuid);
    let mut start_id = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { agent_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            start_id = Some(agent_id);
            break;
        }
    }
    assert_eq!(
        start_id,
        Some(expected),
        "a probe-live stale rollout must register on startup, UUID-keyed"
    );
    handle.abort();
}

/// First-sight `SessionStart.session_id` must come from the source's
/// `IdDeriver`, NOT the raw file stem: a Codex stem is the full
/// `rollout-<ts>-<uuid>` string while the hook transport carries the bare
/// UUID, so a JSONL-created slot would disagree with its hook-created twin
/// (and `backfill_identity` never heals a non-empty session_id) — the
/// tooltip's same-cwd disambiguator then suffixes the constant `roll` for
/// every JSONL-created Codex slot.
#[tokio::test]
async fn codex_first_sight_session_start_carries_bare_uuid_session_id() {
    fast_watch();
    use pixtuoid_core::source::codex::{codex_id_from_path, decode_codex_line, derive_codex_label};

    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let day_dir = root.join("2026").join("06").join("10");
    tokio::fs::create_dir_all(&day_dir).await.unwrap();
    let uuid = "019e7762-9ded-7e33-be41-946ecf105bf5";
    let rollout = day_dir.join(format!("rollout-2026-06-10T08-00-00-{uuid}.jsonl"));
    let meta = serde_json::json!({
        "type": "session_meta",
        "payload": { "id": uuid, "cwd": "/Users/me/dotfiles" }
    });
    tokio::fs::write(&rollout, format!("{meta}\n"))
        .await
        .unwrap();

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = JsonlWatcher::new(
        root.clone(),
        "codex".to_string(),
        decode_codex_line,
        derive_codex_label,
        |_t| false,
    )
    .with_id_deriver(codex_id_from_path);
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    let mut got = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((
            _,
            AgentEvent::SessionStart {
                agent_id,
                session_id,
                ..
            },
        ))) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            got = Some((agent_id, session_id));
            break;
        }
    }
    let (agent_id, session_id) = got.expect("expected SessionStart from the codex watcher");
    assert_eq!(agent_id, AgentId::from_parts("codex", uuid));
    assert_eq!(
        session_id, uuid,
        "first-sight session_id must be the IdDeriver's bare UUID, not the rollout file stem"
    );
    handle.abort();
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
            Some(ProbeSnapshot {
                ids: probe_view.lock().unwrap().clone(),
                pid_of: std::collections::HashMap::new(),
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
    let (probe_state, mut rx, _transcript, handle) = admitted_with_mutable_probe(
        dir.path().to_path_buf(),
        uuid,
        Duration::from_millis(600),
        vouch_snapshot(&[uuid]),
    )
    .await;
    let expected = AgentId::from_parts("claude-code", uuid);

    // Brief drop: a few healthy-but-empty snapshots open the miss window...
    *probe_state.lock().unwrap() = vouch_snapshot(&[]);
    tokio::time::sleep(Duration::from_millis(250)).await;
    // ...then the vouch re-appears INSIDE the span — the window must cancel.
    *probe_state.lock().unwrap() = vouch_snapshot(&[uuid]);

    assert_no_session_end_within(
        &mut rx,
        expected,
        Duration::from_millis(1500),
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

// ── Child-end un-claim (#246): the carrier for in-flight multi-turn children ─

/// #246 — the watcher half of the un-claim side-channel: a hook SubagentStop
/// ends a child whose transcript stays `seen`-claimed, so turn-N+1 appends
/// used to decode as unknown-id no-ops with no re-registration carrier on
/// either transport (upstream provides none — codex-rs hook_runtime.rs).
/// Pushing the child id into the shared handle (the ClaudeCodeSource tee's
/// job in production, pinned separately) must make the running watcher
/// release the claim on a scan/notify pass, so a later append re-registers
/// the SAME id with a FRESH SessionStart. (The drain-before-release straggler
/// discipline is pinned deterministically in source/jsonl/tests.rs's
/// `child_end_unclaim_drains_stragglers_then_releases_without_session_end`.)
#[tokio::test]
async fn child_end_unclaim_lets_a_turn_n_plus_1_append_re_register() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let parent_uuid = "0f000000-0000-7000-8000-0000000000f1";
    let sub_dir = projects_root
        .join("-Users-me-proj")
        .join(parent_uuid)
        .join("subagents");
    tokio::fs::create_dir_all(&sub_dir).await.unwrap();
    let child_stem = "agent-f0000000000000001";
    let transcript = sub_dir.join(format!("{child_stem}.jsonl"));
    let expected = AgentId::from_parts("claude-code", child_stem);

    let unclaims = ChildEndUnclaims::new();
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let watcher = cc_watcher(projects_root.clone())
        .with_poll_interval(Duration::from_millis(100))
        .with_child_end_unclaims(unclaims.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Turn N: the child's transcript registers (claims `seen`).
    write_lines(
        &transcript,
        &[cc_subagent_line(child_stem, "/repo", "tu_n1")],
    )
    .await;
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
    assert!(registered, "turn N must register the child transcript");

    // Turn N ends: the SubagentStop hook is decoded; the tee pushes the id.
    unclaims.push(expected);

    // Turn N+1: appends arrive. The first notify after the push drains the
    // handle (releasing the claim) — appends keep coming during an active
    // turn, so re-registration converges within a couple of writes.
    let mut revived = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut next_tu = 0u32;
    while tokio::time::Instant::now() < deadline && !revived {
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .await
            .unwrap();
        next_tu += 1;
        f.write_all(
            format!(
                "{}\n",
                cc_subagent_line(child_stem, "/repo", &format!("tu_n2_{next_tu}"))
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        f.flush().await.unwrap();
        drop(f);
        let wait_until = tokio::time::Instant::now() + Duration::from_millis(300);
        while tokio::time::Instant::now() < wait_until {
            if let Ok(Some((_, AgentEvent::SessionStart { agent_id, .. }))) =
                tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
            {
                if agent_id == expected {
                    revived = true;
                    break;
                }
            }
        }
    }
    assert!(
        revived,
        "after the child-end un-claim, a turn-N+1 append must re-register \
         the SAME id with a fresh SessionStart"
    );
    handle.abort();
}

/// THE #246 acceptance test — the full in-flight multi-turn Codex child story
/// across the REAL CodexSource (run()-wiring included) + the reducer:
/// the SubagentStart hook registers the child WITH its parent (the ledger
/// remembers the applied link); the flat rollout is `seen`-claimed by the
/// watcher; the SubagentStop hook (`as_child: true`) ends the slot and — via
/// the tee, simulated here by the push (pinned separately in claude_code.rs)
/// — feeds the un-claim side-channel; the turn-N+1 append then re-emits a
/// parentless SessionStart through the released claim, and the reducer's
/// child ledger RE-LINKS it to the remembered parent: the child re-joins the
/// scope tree mid-flight instead of staying invisible.
#[tokio::test]
async fn in_flight_multi_turn_codex_child_revives_and_relinks_via_unclaim() {
    use pixtuoid_core::source::decoder::decode_hook_payload;
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    use pixtuoid_core::{Reducer, SceneState};

    fast_watch();
    let dir = TempDir::new().unwrap();
    let sessions_root = dir.path().to_path_buf();
    let child_uuid = "0e000000-0000-7000-8000-0000000000e1";
    let rollout = sessions_root.join(format!("rollout-2026-06-11T10-00-00-{child_uuid}.jsonl"));
    let parent = AgentId::from_parts("codex", "parent-sess");
    let child = AgentId::from_parts("codex", child_uuid);

    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let apply_hook = |r: &mut Reducer, scene: &mut SceneState, payload: serde_json::Value| {
        for ev in decode_hook_payload(payload).expect("hook payload decodes") {
            r.apply(scene, ev, SystemTime::now(), Transport::Hook);
        }
    };

    // Thread startup: the parent prompt + the child's SubagentStart hook.
    apply_hook(
        &mut r,
        &mut scene,
        serde_json::json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "parent-sess",
            "_pixtuoid_source": "codex",
            "cwd": "/repo",
        }),
    );
    apply_hook(
        &mut r,
        &mut scene,
        serde_json::json!({
            "hook_event_name": "SubagentStart",
            "session_id": "parent-sess",
            "agent_id": child_uuid,
            "cwd": "/repo",
            "_pixtuoid_source": "codex",
        }),
    );
    assert_eq!(
        scene.agents.get(&child).map(|s| s.parent_id),
        Some(Some(parent)),
        "first life: the child registers with the parent link"
    );

    // The REAL CodexSource (with the shared handle) claims the rollout.
    let unclaims = ChildEndUnclaims::new();
    let src = CodexSource {
        sessions_root,
        child_end_unclaims: Some(unclaims.clone()),
    };
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let watcher_task = tokio::spawn(async move { Box::new(src).run(tx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let meta = serde_json::json!({
        "type": "session_meta",
        "payload": { "id": child_uuid, "cwd": "/repo" }
    });
    let work = serde_json::json!({
        "type": "event_msg",
        "payload": { "type": "task_started", "turn_id": "t1" }
    });
    write_lines(&rollout, &[meta, work.clone()]).await;
    let mut claimed = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline && !claimed {
        if let Ok(Some((transport, ev))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            claimed = matches!(
                &ev,
                AgentEvent::SessionStart { agent_id, .. } if *agent_id == child
            );
            r.apply(&mut scene, ev, SystemTime::now(), transport);
        }
    }
    assert!(claimed, "the watcher must claim the child's flat rollout");

    // Turn N ends: SubagentStop ends the slot; the tee pushes the id.
    apply_hook(
        &mut r,
        &mut scene,
        serde_json::json!({
            "hook_event_name": "SubagentStop",
            "session_id": "parent-sess",
            "agent_id": child_uuid,
            "_pixtuoid_source": "codex",
        }),
    );
    unclaims.push(child);
    r.tick(
        &mut scene,
        SystemTime::now() + EXIT_GRACE_WINDOW + Duration::from_secs(1),
    );
    assert!(
        !scene.agents.contains_key(&child),
        "the child's first life is GC'd after the SubagentStop"
    );

    // Turn N+1 (parent send_input): rollout appends resume. Through the
    // released claim a parentless SessionStart re-registers the child, and
    // the ledger restores the remembered parent.
    let mut relinked = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline && !relinked {
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&rollout)
            .await
            .unwrap();
        f.write_all(format!("{work}\n").as_bytes()).await.unwrap();
        f.flush().await.unwrap();
        drop(f);
        let wait_until = tokio::time::Instant::now() + Duration::from_millis(300);
        while tokio::time::Instant::now() < wait_until {
            if let Ok(Some((transport, ev))) =
                tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
            {
                r.apply(&mut scene, ev, SystemTime::now(), transport);
                relinked = scene.agents.get(&child).map(|s| s.parent_id) == Some(Some(parent));
                if relinked {
                    break;
                }
            }
        }
    }
    assert!(
        scene.agents.contains_key(&child),
        "turn N+1 must re-register the child through the released claim"
    );
    assert_eq!(
        scene.agents.get(&child).map(|s| s.parent_id),
        Some(Some(parent)),
        "the revived child must re-link to the remembered parent (the #249 \
         ledger), not register as an orphan"
    );
    watcher_task.abort();
}

/// Conversely, a transcript whose mtime is *within* the initial-window is
/// treated as live: its SessionStart and any historical content replays so
/// in-flight Task / tool state survives a pixtuoid restart.
#[tokio::test]
async fn watcher_emits_session_start_for_recent_files_on_startup() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-fresh");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let fresh = project_dir.join("fresh.jsonl");
    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": "fresh",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_fresh", "name": "Bash",
                  "input": { "command": "ls" } }
            ]
        }
    });
    std::fs::write(&fresh, format!("{line}\n")).unwrap();
    // fsync the parent directory so the directory entry is guaranteed visible
    // to read_dir — without this, APFS metadata propagation can race with
    // the watcher's initial seed walk under heavy concurrent I/O. Unix-only:
    // Windows can't open a directory as a plain file (and the APFS race
    // doesn't exist there).
    #[cfg(unix)]
    std::fs::File::open(&project_dir)
        .unwrap()
        .sync_all()
        .unwrap();

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone()).with_initial_window(Duration::from_secs(3600));
    let fresh_path = fresh.clone();
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    // Give the watcher task a chance to complete the initial seed scan, then
    // append a no-op newline to trigger a watcher notification as a fallback
    // path in case the initial seed missed the file under heavy I/O contention.
    tokio::time::sleep(Duration::from_millis(500)).await;
    tokio::fs::OpenOptions::new()
        .append(true)
        .open(&fresh_path)
        .await
        .unwrap()
        .sync_all()
        .await
        .unwrap();

    let mut got_start = false;
    let mut got_activity = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            Ok(Some((_, AgentEvent::SessionStart { .. }))) => got_start = true,
            Ok(Some((_, AgentEvent::ActivityStart { .. }))) => got_activity = true,
            _ => {}
        }
        if got_start && got_activity {
            break;
        }
    }
    assert!(got_start, "fresh file should produce SessionStart");
    assert!(got_activity, "fresh file content should be replayed");
    handle.abort();
}

/// First-sight cwd extraction must scan past unparsable prefix lines.
/// `extract_cwd` previously short-circuited via `?` on the first non-JSON
/// (or non-UTF8) line, even if a later line carried the `cwd` field.
#[tokio::test]
async fn first_sight_extracts_cwd_past_non_json_prefix() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-cwd");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-cwd.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    // First line: garbage. Second line: a system line carrying cwd. Watcher
    // should still derive cwd = /real-repo on the SessionStart for first-sight.
    //
    // The watcher emits SessionStart exactly ONCE per file, with cwd taken from
    // whatever bytes are present at first read. Writing the lines incrementally
    // (or even create-then-write) leaves a window where the 250ms poll observes
    // a partial/empty file, latches cwd="" permanently, and fails this test
    // (flaky under load / coverage instrumentation). Stage the complete content
    // in a sibling `.partial` file — excluded by the watcher's `.jsonl`
    // extension filter — then atomically rename it into place so first sight
    // always reads the full content.
    let sys_line = serde_json::json!({
        "type": "system",
        "subtype": "session_start",
        "sessionId": "ses-cwd",
        "cwd": "/real-repo"
    });
    let content = format!("not-json-prefix\n{sys_line}\n");
    let staging = project_dir.join("ses-cwd.jsonl.partial");
    tokio::fs::write(&staging, content.as_bytes())
        .await
        .unwrap();
    tokio::fs::rename(&staging, &transcript).await.unwrap();

    let mut found_cwd = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { cwd, .. }))) =
            tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
        {
            found_cwd = Some(cwd);
            break;
        }
    }
    assert_eq!(
        found_cwd,
        Some(std::path::PathBuf::from("/real-repo")),
        "extract_cwd must scan past non-JSON lines to find cwd"
    );
    handle.abort();
}

/// Stale files become live as soon as CC writes to them — the next notify
/// event must produce a SessionStart, since the file is now active.
#[tokio::test]
async fn stale_file_emits_session_start_when_written_to() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-revive");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let revived = project_dir.join("revive.jsonl");
    tokio::fs::write(&revived, "{}\n").await.unwrap();
    set_file_mtime(
        &revived,
        FileTime::from_system_time(SystemTime::now() - Duration::from_secs(3600)),
    )
    .unwrap();

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone()).with_initial_window(Duration::from_secs(60));
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(150)).await;

    // No SessionStart yet (stale + skipped).
    while tokio::time::timeout(Duration::from_millis(20), rx.recv())
        .await
        .is_ok()
    {}

    // Append a real assistant tool_use line — file is now live.
    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": "revive",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_new", "name": "Bash",
                  "input": { "command": "ls" } }
            ]
        }
    });
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&revived)
        .await
        .unwrap();
    f.write_all(format!("{line}\n").as_bytes()).await.unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_start = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { .. }))) =
            tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
        {
            got_start = true;
            break;
        }
    }
    assert!(got_start, "appending to a stale file should bring it live");
    handle.abort();
}

/// A recent file (within the initial window) that has a session_end marker
/// at its tail must NOT produce a SessionStart on startup — the watcher
/// must detect the ended session and seed the cursor at EOF.
#[tokio::test]
async fn watcher_skips_recent_file_with_session_end_marker() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-ended");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let ended = project_dir.join("ended.jsonl");
    let content = r#"{"type":"system","subtype":"session_start","sessionId":"ended","cwd":"/repo"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_1","name":"Bash","input":{"command":"ls"}}]}}
{"type":"system","subtype":"session_end","sessionId":"ended"}
"#;
    tokio::fs::write(&ended, content).await.unwrap();
    // mtime is "now" — well within the initial window.

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone()).with_initial_window(Duration::from_secs(3600));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut events = Vec::new();
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
        events.push(ev);
    }
    let has_session_start = events
        .iter()
        .any(|(_, ev)| matches!(ev, AgentEvent::SessionStart { .. }));
    assert!(
        !has_session_start,
        "recent file with session_end marker must not produce SessionStart, got {events:?}"
    );
    handle.abort();
}

fn custom_label(_path: &std::path::Path, _source: &str, _cwd: &std::path::Path) -> String {
    "custom-label-ok".to_string()
}

#[tokio::test]
async fn watcher_custom_label_deriver() {
    fast_watch();
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-y");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-xyz.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = JsonlWatcher::new(
        projects_root.clone(),
        "claude-code".to_string(),
        decode_cc_line,
        custom_label,
        cc_session_ended,
    );
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    let start_line = serde_json::json!({
        "type": "system",
        "subtype": "session_start",
        "sessionId": "ses-xyz",
        "cwd": "/repo"
    });
    f.write_all(format!("{start_line}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_custom_rename = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((_, AgentEvent::Rename { label, .. }))) => {
                if label == "custom-label-ok" {
                    got_custom_rename = true;
                    break;
                }
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
    }
    assert!(
        got_custom_rename,
        "expected Rename event with custom label from label deriver fn"
    );
    handle.abort();
}

#[tokio::test]
async fn codex_rollout_yields_uuid_keyed_session_start() {
    fast_watch();
    use pixtuoid_core::source::codex::{codex_id_from_path, decode_codex_line, derive_codex_label};

    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let uuid = "019e7762-9ded-7e33-be41-946ecf105bf4";
    let transcript = root.join(format!("rollout-2026-05-29T22-36-52-{uuid}.jsonl"));

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = JsonlWatcher::new(
        root.clone(),
        "codex".to_string(),
        decode_codex_line,
        derive_codex_label,
        |_t| false,
    )
    .with_id_deriver(codex_id_from_path);
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    let meta = serde_json::json!({
        "type": "session_meta",
        "payload": { "id": uuid, "cwd": "/Users/me/dotfiles" }
    });
    f.write_all(format!("{meta}\n").as_bytes()).await.unwrap();
    let task_started = serde_json::json!({
        "type": "event_msg",
        "payload": { "type": "task_started", "turn_id": "t" }
    });
    f.write_all(format!("{task_started}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let expected = AgentId::from_parts("codex", uuid);
    let mut saw_session_start = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((_t, AgentEvent::SessionStart { agent_id, .. }))) => {
                assert_eq!(agent_id, expected, "Codex SessionStart must be UUID-keyed");
                saw_session_start = true;
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
    }
    assert!(saw_session_start, "expected a SessionStart event");
    handle.abort();
}

#[tokio::test]
async fn default_id_deriver_stays_path_keyed() {
    // Pin the IdDeriver DEFAULT: a watcher built WITHOUT `.with_id_deriver`
    // (e.g. Antigravity) must key on the file path. CC + Codex override it
    // (`.with_id_deriver`) to key on the session UUID; this guards the
    // un-overridden default so the path-keyed sources keep coalescing.
    fast_watch();
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let project_dir = root.join("proj-y");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("abc.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    // No `.with_id_deriver` → the default path-keyed deriver is exercised.
    let watcher = JsonlWatcher::new(
        root.clone(),
        "antigravity".to_string(),
        decode_cc_line,
        cc_derive_label,
        cc_session_ended,
    );
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    let start_line = serde_json::json!({
        "type": "system",
        "subtype": "session_start",
        "sessionId": "abc",
        "cwd": "/repo"
    });
    f.write_all(format!("{start_line}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    // A bare watcher (no `.with_id_deriver`) uses the DEFAULT deriver, which
    // keys on the file PATH (`default_id_from_path` = `normalize_path_key(path)`),
    // NOT a UUID/stem — the keying Antigravity relies on; the real
    // ClaudeCodeSource overrides it with `cc_id_from_path`. Assert the emitted id
    // is NOT the stem-keyed id (the regression a stem-keyed default deriver would
    // introduce); this holds on every platform since the path string is never
    // "abc". The EXACT value (`from_parts(source, normalize_path_key(path))`) is
    // platform-dependent and `normalize_path_key` is `pub(crate)` (unreachable
    // here), so it's pinned at the UNIT level instead —
    // `jsonl/tests.rs::default_id_from_path_returns_normalized_path_key` + `decoder.rs`'s
    // `normalize_path_key` tests — not re-derived in this integration test.
    let stem_keyed = AgentId::from_parts("antigravity", "abc");
    let mut ok = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((_t, AgentEvent::SessionStart { agent_id, .. }))) => {
                assert_ne!(
                    agent_id, stem_keyed,
                    "default deriver must be path-keyed, not stem-keyed"
                );
                ok = true;
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
    }
    assert!(ok, "expected a path-keyed SessionStart");
    handle.abort();
}

// CodexSource::run is just `JsonlWatcher::new(...).run(tx)` — drive the real
// Source impl against a TempDir sessions_root so its run()-glue is exercised
// (not only the watcher internals). A rollout file with a task_started line must
// surface an ActivityStart through the source.
#[tokio::test]
async fn codex_source_run_emits_events_from_rollout() {
    fast_watch();
    let dir = TempDir::new().unwrap();
    let sessions_root = dir.path().to_path_buf();
    let uuid = "019e7762-9ded-7e33-be41-946ecf105bf4";
    let transcript = sessions_root.join(format!("rollout-2026-05-29T22-36-52-{uuid}.jsonl"));

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let src = CodexSource {
        sessions_root,
        child_end_unclaims: None,
    };
    let handle = tokio::spawn(async move { Box::new(src).run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let meta = serde_json::json!({
        "type": "session_meta",
        "payload": { "id": uuid, "cwd": "/repo" }
    });
    let task_started = serde_json::json!({
        "type": "event_msg",
        "payload": { "type": "task_started", "turn_id": "t" }
    });
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(format!("{meta}\n{task_started}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_activity = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::ActivityStart { .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            got_activity = true;
            break;
        }
    }
    assert!(
        got_activity,
        "CodexSource::run should surface ActivityStart"
    );
    handle.abort();
}

// AntigravitySource::run mirrors CodexSource::run — drive the real Source impl
// against a TempDir brain_root.
#[tokio::test]
async fn antigravity_source_run_emits_events_from_transcript() {
    fast_watch();
    let dir = TempDir::new().unwrap();
    let brain_root = dir.path().to_path_buf();
    let project_dir = brain_root.join("sess");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("transcript.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let src = AntigravitySource { brain_root };
    let handle = tokio::spawn(async move { Box::new(src).run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let planner = serde_json::json!({
        "step_index": 1,
        "cwd": "/repo",
        "type": "PLANNER_RESPONSE",
        "tool_calls": [ { "name": "list_dir", "args": { "DirectoryPath": "\"/repo/src\"" } } ]
    });
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(format!("{planner}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_activity = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::ActivityStart { .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            got_activity = true;
            break;
        }
    }
    assert!(
        got_activity,
        "AntigravitySource::run should surface ActivityStart"
    );
    handle.abort();
}

// ClaudeCodeSource::run binds the hook socket, spawns the watcher, and enters
// the select! — drive the real Source impl so the bind + spawn + select-entry
// glue is exercised (only the select abort/warn arms stay structurally
// unreachable: both inner tasks loop forever). A CC transcript written under
// the projects_root must surface a SessionStart through the JSONL leg.
#[tokio::test]
async fn claude_code_source_run_binds_socket_and_emits_events() {
    fast_watch();
    let dir = TempDir::new().unwrap();
    // The hook endpoint must be platform-shaped: a filesystem path is an
    // invalid pipe name on Windows and would fail run()'s bind before the
    // JSONL leg (the thing under test) ever starts.
    #[cfg(unix)]
    let socket_path = dir.path().join("pixtuoid-test.sock");
    #[cfg(windows)]
    let socket_path = std::path::PathBuf::from(format!(
        r"\\.\pipe\pixtuoid-test-jsonlw-{}",
        std::process::id()
    ));
    let projects_root = dir.path().join("projects");
    let project_dir = projects_root.join("proj-cc");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-cc.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let src = ClaudeCodeSource {
        socket_path,
        projects_root,
        child_end_unclaims: None,
    };
    let handle = tokio::spawn(async move { Box::new(src).run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": "ses-cc",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_1", "name": "Bash", "input": { "command": "ls" } }
            ]
        }
    });
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(format!("{line}\n").as_bytes()).await.unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_start = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            got_start = true;
            break;
        }
    }
    assert!(
        got_start,
        "ClaudeCodeSource::run should surface SessionStart from the JSONL leg"
    );
    handle.abort();
}

// Cursor-safety guard: a transcript truncated below the watcher's stored cursor
// must reset the cursor (not stay stuck) so newly-appended content re-decodes.
#[tokio::test]
async fn watcher_resets_cursor_on_truncation_below_cursor() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-trunc");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("trunc.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let tool_line = |id: &str| {
        serde_json::json!({
            "type": "assistant",
            "sessionId": "trunc",
            "cwd": "/repo",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": id, "name": "Bash", "input": { "command": "ls" } }
                ]
            }
        })
        .to_string()
    };

    // Write a long first line so the cursor advances well past a later short one.
    let long = tool_line("tu_long") + &" ".repeat(400);
    tokio::fs::write(&transcript, format!("{long}\n"))
        .await
        .unwrap();

    // Let the watcher advance its cursor to EOF.
    let mut saw_long = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::ActivityStart { tool_use_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if tool_use_id.as_deref() == Some("tu_long") {
                saw_long = true;
                break;
            }
        }
    }
    assert!(saw_long, "expected the first long line to decode");

    // Truncate the file far below the stored cursor, then append a fresh line.
    let fresh = tool_line("tu_fresh");
    tokio::fs::write(&transcript, format!("{fresh}\n"))
        .await
        .unwrap();

    // The cursor (set past the long line) now exceeds file_len → reset to 0 →
    // the fresh line re-decodes on the next scan.
    let mut saw_fresh = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::ActivityStart { tool_use_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if tool_use_id.as_deref() == Some("tu_fresh") {
                saw_fresh = true;
                break;
            }
        }
    }
    assert!(
        saw_fresh,
        "after truncation the cursor must reset so the fresh line decodes"
    );
    handle.abort();
}

// Cursor-safety guard: a > 1 MiB first-sight pending tail with no newline (no
// recoverable cwd in its head) must skip its BACKLOG to EOF (not buffer it),
// yet still REGISTER the agent (#204) — a SessionStart + a project-dir-fallback
// Rename — and a later newline-terminated valid line still decodes.
#[tokio::test]
async fn watcher_skips_oversized_pending_tail() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-big");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("big.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Write > 1 MiB of junk with NO newline — file_len - cursor exceeds
    // MAX_PENDING_BYTES, so the watcher seeks the cursor to EOF (skipping the
    // backlog) but still registers the agent on first-sight.
    let junk = vec![b'x'; (1 << 20) + 1024];
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(&junk).await.unwrap();
    f.flush().await.unwrap();
    drop(f);

    // Give the watcher a scan; collect the first-sight registration. The junk
    // head has no complete line → no cwd → empty-cwd SessionStart, and the
    // Rename falls back to the project-dir basename. No ActivityStart: a
    // no-newline blob has no decodable line, and the backlog isn't replayed.
    let mut got_start = false;
    let mut got_rename = None;
    let mut activity_before = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((_, AgentEvent::SessionStart { cwd, .. }))) => {
                got_start = true;
                assert_eq!(
                    cwd,
                    std::path::PathBuf::from(""),
                    "a no-newline head yields an empty-cwd SessionStart"
                );
            }
            Ok(Some((_, AgentEvent::Rename { label, .. }))) => got_rename = Some(label),
            Ok(Some((_, AgentEvent::ActivityStart { .. }))) => activity_before += 1,
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                if got_start && got_rename.is_some() {
                    break;
                }
            }
        }
    }
    assert!(
        got_start,
        "a first-sight oversized transcript must register an agent (#204), not stay invisible"
    );
    assert_eq!(
        got_rename.as_deref(),
        Some("cc·big"),
        "empty-cwd Rename falls back to the project-dir basename"
    );
    assert_eq!(
        activity_before, 0,
        "the oversized backlog must not be replayed (got {activity_before} ActivityStart)"
    );

    // Append a newline (closing the junk line) plus a valid line. The junk line
    // is past the EOF-seeked cursor, so only the valid line decodes.
    let valid = serde_json::json!({
        "type": "assistant",
        "sessionId": "big",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_after_junk", "name": "Bash", "input": { "command": "ls" } }
            ]
        }
    });
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(format!("\n{valid}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_after = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::ActivityStart { tool_use_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if tool_use_id.as_deref() == Some("tu_after_junk") {
                got_after = true;
                break;
            }
        }
    }
    assert!(
        got_after,
        "the post-skip valid line must decode after the oversized tail is skipped"
    );
    handle.abort();
}

// #204: a RECENT, valid, multi-line transcript larger than MAX_PENDING_BYTES
// (e.g. a 7.4 MB main session) must REGISTER its agent on first-sight — a
// SessionStart + Rename derived from a bounded head read — instead of being
// silently skipped to EOF and staying invisible until its next small append.
// The giant backlog is still NOT replayed (no flood of historical events).
#[tokio::test]
async fn watcher_registers_large_transcript_on_first_sight() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("-Users-me-bigrepo");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("big-session.jsonl");

    // Build a valid, newline-terminated transcript that exceeds 1 MiB. The
    // FIRST line carries `cwd` (CC always writes it on the first line), so a
    // bounded head read recovers it without touching the whole file. The rest
    // are valid tool_use lines — the backlog that must NOT be replayed.
    let first = serde_json::json!({
        "type": "system",
        "subtype": "session_start",
        "sessionId": "big-session",
        "cwd": "/Users/me/work/bigrepo"
    });
    let mut contents = format!("{first}\n");
    let backlog_line = serde_json::json!({
        "type": "assistant",
        "sessionId": "big-session",
        "cwd": "/Users/me/work/bigrepo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_backlog", "name": "Bash", "input": { "command": "ls" } }
            ]
        }
    });
    let backlog_line = format!("{backlog_line}\n");
    while contents.len() <= (1usize << 20) + 4096 {
        contents.push_str(&backlog_line);
    }
    assert!(
        contents.len() > (1usize << 20),
        "test transcript must exceed MAX_PENDING_BYTES"
    );

    // Write the whole file BEFORE the watcher first sees it, so the entire body
    // is one oversized first-sight pending tail (cursor 0 → file_len > 1 MiB).
    tokio::fs::write(&transcript, contents.as_bytes())
        .await
        .unwrap();
    // Keep it inside the recency window (write() above already set a fresh
    // mtime; assert it isn't gated as historical by should_seed_at_eof).

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(256);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    let mut start_id = None;
    let mut start_cwd = None;
    let mut label = None;
    let mut activity_count = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((_, AgentEvent::SessionStart { agent_id, cwd, .. }))) => {
                start_id = Some(agent_id);
                start_cwd = Some(cwd);
            }
            Ok(Some((_, AgentEvent::Rename { label: l, .. }))) => {
                label = Some(l);
            }
            Ok(Some((_, AgentEvent::ActivityStart { .. }))) => {
                activity_count += 1;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
        // Once we have the registration pair, drain briefly to confirm the
        // backlog isn't pouring in, then stop.
        if start_id.is_some() && label.is_some() {
            // Short settle: if the backlog were replayed we'd accumulate
            // hundreds of ActivityStart here.
            tokio::time::sleep(Duration::from_millis(200)).await;
            while let Ok(Some((_, ev))) =
                tokio::time::timeout(Duration::from_millis(20), rx.recv()).await
            {
                if matches!(ev, AgentEvent::ActivityStart { .. }) {
                    activity_count += 1;
                }
            }
            break;
        }
    }

    let _start_id = start_id.expect("expected SessionStart for the large first-sight transcript");
    let start_cwd = start_cwd.expect("SessionStart should carry the head-derived cwd");
    let label = label.expect("expected a Rename label for the large transcript");
    assert_eq!(
        start_cwd,
        std::path::PathBuf::from("/Users/me/work/bigrepo"),
        "cwd must come from the bounded head read of the first line"
    );
    assert_eq!(
        label, "cc·bigrepo",
        "label must derive from the head-read cwd basename"
    );
    // The backlog is skipped to EOF: registration fires, but the thousands of
    // historical tool_use lines are NOT replayed. Allow a small margin (0) but
    // assert it's nowhere near the backlog count (hundreds of lines).
    assert!(
        activity_count < 5,
        "the giant backlog must not be replayed wholesale (got {activity_count} ActivityStart)"
    );
    handle.abort();
}

// The per-line non-UTF8 guard in walk_jsonl: a raw invalid-UTF8 byte line is
// warn-and-skipped, and a following valid JSON line still decodes (the bad line
// is not fatal to the rest of the read).
#[tokio::test]
async fn watcher_skips_non_utf8_line_and_keeps_going() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-nonutf8");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("nonutf8.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let valid = serde_json::json!({
        "type": "assistant",
        "sessionId": "nonutf8",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_valid", "name": "Bash", "input": { "command": "ls" } }
            ]
        }
    });
    // Invalid-UTF8 bytes + newline, then a valid JSON line + newline. The bytes
    // can't go through serde_json (JSON is UTF-8) — write them raw.
    let mut bytes: Vec<u8> = vec![0xff, 0xfe, b'\n'];
    bytes.extend_from_slice(format!("{valid}\n").as_bytes());
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(&bytes).await.unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_valid = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::ActivityStart { tool_use_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if tool_use_id.as_deref() == Some("tu_valid") {
                got_valid = true;
                break;
            }
        }
    }
    assert!(
        got_valid,
        "a non-UTF8 line must be skipped, not block the following valid line"
    );
    handle.abort();
}

// Drives detect_parent_id through the REAL watcher recursion: a subagent
// transcript at <root>/proj/parent/subagents/agent-1.jsonl must emit a
// SessionStart whose parent_id derives the parent from the grandparent dir.
#[tokio::test]
async fn watcher_derives_parent_id_for_subagent_path() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let subagent_dir = projects_root.join("proj").join("parent").join("subagents");
    tokio::fs::create_dir_all(&subagent_dir).await.unwrap();
    let transcript = subagent_dir.join("agent-1.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": "agent-1",
        "cwd": "/repo",
        "attributionAgent": "feature-dev:code-explorer",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_1", "name": "Read", "input": { "file_path": "/x" } }
            ]
        }
    });
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(format!("{line}\n").as_bytes()).await.unwrap();
    f.flush().await.unwrap();
    drop(f);

    // The parent link keys on the `<parent-uuid>` dir component ("parent"),
    // which is cwd-independent — so there is no raw-vs-canonical ambiguity here
    // (the project-dir prefix is intentionally not part of the key).
    let expected = AgentId::from_parts("claude-code", "parent");

    let mut found_parent = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((
            _,
            AgentEvent::SessionStart {
                parent_id: Some(pid),
                ..
            },
        ))) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            found_parent = Some(pid);
            break;
        }
    }
    let found = found_parent.expect("expected a SessionStart carrying parent_id");
    assert_eq!(
        found, expected,
        "parent_id must key on the <parent-uuid> dir component; got {found:?}"
    );
    handle.abort();
}

// THE cwd-split bug: a git-worktree splits the parent transcript and the
// subagent transcript into DIFFERENT `~/.claude/projects/<project-dir>/` trees
// (project-dir is a pure function of cwd). The link must still resolve because
// the `<parent-uuid>` component is cwd-independent and equals the parent's own
// session UUID. Drives the REAL watcher: the subagent's emitted parent_id must
// equal the parent's emitted SessionStart agent_id even though they live under
// different project dirs.
#[tokio::test]
async fn watcher_links_subagent_across_project_dirs() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();

    let parent_uuid = "abc123def456";
    // Parent transcript under project-dir A.
    let project_a = projects_root.join("-Users-me-PROJECT-A");
    tokio::fs::create_dir_all(&project_a).await.unwrap();
    let parent_transcript = project_a.join(format!("{parent_uuid}.jsonl"));
    // Subagent transcript under a DIFFERENT project-dir B, sharing the same
    // `<parent-uuid>/subagents/` component.
    let subagent_dir = projects_root
        .join("-Users-me-PROJECT-B")
        .join(parent_uuid)
        .join("subagents");
    tokio::fs::create_dir_all(&subagent_dir).await.unwrap();
    let sub_transcript = subagent_dir.join("agent-1.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let parent_line = serde_json::json!({
        "type": "system",
        "subtype": "session_start",
        "sessionId": parent_uuid,
        "cwd": "/Users/me/PROJECT-A"
    });
    let mut pf = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&parent_transcript)
        .await
        .unwrap();
    pf.write_all(format!("{parent_line}\n").as_bytes())
        .await
        .unwrap();
    pf.flush().await.unwrap();
    drop(pf);

    let sub_line = serde_json::json!({
        "type": "assistant",
        "sessionId": "agent-1",
        "cwd": "/Users/me/PROJECT-B",
        "attributionAgent": "feature-dev:code-explorer",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_1", "name": "Read", "input": { "file_path": "/x" } }
            ]
        }
    });
    let mut sf = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&sub_transcript)
        .await
        .unwrap();
    sf.write_all(format!("{sub_line}\n").as_bytes())
        .await
        .unwrap();
    sf.flush().await.unwrap();
    drop(sf);

    // Collect the parent's SessionStart agent_id (no parent_id) and the
    // subagent's SessionStart parent_id; they must be equal.
    let mut parent_agent_id: Option<AgentId> = None;
    let mut sub_parent_id: Option<AgentId> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((
                _,
                AgentEvent::SessionStart {
                    agent_id,
                    parent_id,
                    ..
                },
            ))) => match parent_id {
                Some(pid) => sub_parent_id = Some(pid),
                None => parent_agent_id = Some(agent_id),
            },
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
        if parent_agent_id.is_some() && sub_parent_id.is_some() {
            break;
        }
    }
    let parent_agent_id = parent_agent_id.expect("expected the parent's SessionStart");
    let sub_parent_id = sub_parent_id.expect("expected the subagent's SessionStart with parent_id");
    assert_eq!(
        sub_parent_id, parent_agent_id,
        "subagent parent_id must equal the parent's agent_id across a cwd-split (different project dirs)"
    );
    handle.abort();
}

// ── Mid-attach scenario suite ────────────────────────────────────────────────
// The acceptance criterion: opening pixtuoid at ANY moment must show all
// active running agent CLIs correctly. These drive JsonlWatcher::run over a
// pre-populated projects-root — the attach moment — and assert the emitted
// live set, exercising the first-sight gate, the liveness-probe bypass, the
// ended check, and the subagent parent link TOGETHER.

/// Write a complete transcript (each line + trailing newline) in one shot, so
/// the watcher's first sight always reads the full fixture content.
async fn write_lines(path: &std::path::Path, lines: &[serde_json::Value]) {
    let mut content = String::new();
    for l in lines {
        content.push_str(&l.to_string());
        content.push('\n');
    }
    tokio::fs::write(path, content).await.unwrap();
}

fn backdate(path: &std::path::Path, secs: u64) {
    set_file_mtime(
        path,
        FileTime::from_system_time(SystemTime::now() - Duration::from_secs(secs)),
    )
    .unwrap();
}

fn cc_session_start_line(uuid: &str, cwd: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "system",
        "subtype": "session_start",
        "sessionId": uuid,
        "cwd": cwd
    })
}

fn cc_tool_use_line(
    uuid: &str,
    cwd: &str,
    tool_use_id: &str,
    name: &str,
    input: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "type": "assistant",
        "sessionId": uuid,
        "cwd": cwd,
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": tool_use_id, "name": name, "input": input }
            ]
        }
    })
}

fn cc_subagent_line(stem: &str, cwd: &str, tool_use_id: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "assistant",
        "sessionId": stem,
        "cwd": cwd,
        "attributionAgent": "feature-dev:code-explorer",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": tool_use_id, "name": "Read",
                  "input": { "file_path": "/x" } }
            ]
        }
    })
}

/// S2 + S6 — THE acceptance test for "attach shows exactly the live set".
/// ONE watcher run over a projects-root holding, side by side:
///   (a) a recent live transcript                 → registers (fresh mtime)
///   (b) a stale transcript the probe vouches for → registers (probe bypass)
///   (c) a stale transcript NOT in the probe      → stays hidden
///   (d) a recent transcript ending in a structural session_end → stays hidden
///   (e) a fresh subagent transcript under (b)    → registers, linked to (b)
/// Exactly {a, b, e} emit SessionStart — and each exactly ONCE across the
/// initial seed, the 250ms rescan, and the poll cycles (S6: emit_first_sight
/// idempotence at the suite level — re-scans must not duplicate registrations).
#[tokio::test]
async fn attach_matrix_registers_exactly_the_live_set() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let proj = projects_root.join("-Users-me-mixed");
    let live_a = "aa000000-0000-7000-8000-00000000000a";
    let idle_b = "bb000000-0000-7000-8000-00000000000b";
    let dead_c = "cc000000-0000-7000-8000-00000000000c";
    let ended_d = "dd000000-0000-7000-8000-00000000000d";
    let sub_dir = proj.join(idle_b).join("subagents");
    tokio::fs::create_dir_all(&sub_dir).await.unwrap();

    // (a) recent live: fresh mtime, no end marker.
    write_lines(
        &proj.join(format!("{live_a}.jsonl")),
        &[
            cc_session_start_line(live_a, "/Users/me/proj-a"),
            cc_tool_use_line(
                live_a,
                "/Users/me/proj-a",
                "tu_a",
                "Bash",
                serde_json::json!({"command": "ls"}),
            ),
        ],
    )
    .await;
    // (b) long-idle but probe-live: stale mtime, in the probe's live set.
    let b_path = proj.join(format!("{idle_b}.jsonl"));
    write_lines(
        &b_path,
        &[cc_session_start_line(idle_b, "/Users/me/proj-b")],
    )
    .await;
    backdate(&b_path, 7200);
    // (c) genuinely dead: stale mtime, NOT in the probe.
    let c_path = proj.join(format!("{dead_c}.jsonl"));
    write_lines(
        &c_path,
        &[cc_session_start_line(dead_c, "/Users/me/proj-c")],
    )
    .await;
    backdate(&c_path, 7200);
    // (d) recent but ENDED: fresh mtime, structural session_end at the tail.
    write_lines(
        &proj.join(format!("{ended_d}.jsonl")),
        &[
            cc_session_start_line(ended_d, "/Users/me/proj-d"),
            serde_json::json!({
                "type": "system", "subtype": "session_end", "sessionId": ended_d
            }),
        ],
    )
    .await;
    // (e) fresh subagent under the probe-live parent (b).
    write_lines(
        &sub_dir.join("agent-e1.jsonl"),
        &[cc_subagent_line("agent-e1", "/Users/me/proj-b", "tu_e")],
    )
    .await;

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let watcher = cc_watcher(projects_root.clone())
        .with_initial_window(Duration::from_secs(60))
        .with_liveness_probe(std::sync::Arc::new(move || vouch_snapshot(&[idle_b])));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    let id_a = AgentId::from_parts("claude-code", live_a);
    let id_b = AgentId::from_parts("claude-code", idle_b);
    let id_e = AgentId::from_parts("claude-code", "agent-e1");

    let mut starts: std::collections::HashMap<AgentId, usize> = Default::default();
    let mut sub_parent: Option<Option<AgentId>> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((
            _,
            AgentEvent::SessionStart {
                agent_id,
                parent_id,
                ..
            },
        ))) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
        {
            *starts.entry(agent_id).or_default() += 1;
            if agent_id == id_e {
                sub_parent = Some(parent_id);
            }
        }
        if starts.contains_key(&id_a) && starts.contains_key(&id_b) && starts.contains_key(&id_e) {
            break;
        }
    }
    // S6: settle past the 250ms post-startup rescan (plus several poll
    // cycles), then drain — re-scans of the SAME root must not re-emit
    // SessionStart for any fixture.
    tokio::time::sleep(Duration::from_millis(700)).await;
    while let Ok(Some((_, ev))) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
        if let AgentEvent::SessionStart {
            agent_id,
            parent_id,
            ..
        } = ev
        {
            *starts.entry(agent_id).or_default() += 1;
            if agent_id == id_e {
                sub_parent = Some(parent_id);
            }
        }
    }

    let expected: std::collections::HashMap<AgentId, usize> =
        [(id_a, 1), (id_b, 1), (id_e, 1)].into_iter().collect();
    assert_eq!(
        starts, expected,
        "attach must register EXACTLY the live set, once each — a (recent), b (probe-live), e (fresh subagent); c (stale dead) and d (ended) stay hidden"
    );
    assert_eq!(
        sub_parent.expect("the subagent's SessionStart was seen"),
        Some(id_b),
        "the fresh subagent must attach linked to its probe-live parent"
    );
    handle.abort();
}

/// S3 — the delegating-parent attach: the parent transcript is STALE (it has
/// been silently waiting on its subagent for longer than the window), its tail
/// is a pending Agent dispatch (tool_use with no tool_result), and its UUID is
/// in the probe's live set; the subagent transcript is fresh. One attach scan
/// must register BOTH, link the subagent to the parent, and replay the pending
/// dispatch as a Task ActivityStart (so the reducer's active_tasks suppression
/// picks the in-flight delegation back up).
#[tokio::test]
async fn delegating_parent_attach_registers_parent_and_links_subagent() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let proj = projects_root.join("-Users-me-deleg");
    let parent_uuid = "ee000000-0000-7000-8000-00000000000e";
    let sub_dir = proj.join(parent_uuid).join("subagents");
    tokio::fs::create_dir_all(&sub_dir).await.unwrap();

    let parent_path = proj.join(format!("{parent_uuid}.jsonl"));
    write_lines(
        &parent_path,
        &[
            cc_session_start_line(parent_uuid, "/Users/me/deleg"),
            cc_tool_use_line(
                parent_uuid,
                "/Users/me/deleg",
                "tu_task",
                "Agent",
                serde_json::json!({
                    "description": "explore", "subagent_type": "code-explorer", "prompt": "go"
                }),
            ),
        ],
    )
    .await;
    backdate(&parent_path, 7200);
    write_lines(
        &sub_dir.join("agent-x1.jsonl"),
        &[cc_subagent_line("agent-x1", "/Users/me/deleg", "tu_x")],
    )
    .await;

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let watcher = cc_watcher(projects_root.clone())
        .with_initial_window(Duration::from_secs(60))
        .with_liveness_probe(std::sync::Arc::new(move || vouch_snapshot(&[parent_uuid])));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    let expected_parent = AgentId::from_parts("claude-code", parent_uuid);
    let mut parent_started = false;
    let mut sub_parent_id: Option<AgentId> = None;
    let mut dispatch_is_task: Option<bool> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            Ok(Some((
                _,
                AgentEvent::SessionStart {
                    agent_id,
                    parent_id,
                    ..
                },
            ))) => match parent_id {
                Some(pid) => sub_parent_id = Some(pid),
                None => parent_started |= agent_id == expected_parent,
            },
            Ok(Some((
                _,
                AgentEvent::ActivityStart {
                    agent_id,
                    tool_use_id,
                    detail,
                },
            ))) if agent_id == expected_parent && tool_use_id.as_deref() == Some("tu_task") => {
                dispatch_is_task = Some(detail.as_ref().is_some_and(|d| d.is_task()));
            }
            _ => {}
        }
        if parent_started && sub_parent_id.is_some() && dispatch_is_task.is_some() {
            break;
        }
    }
    assert!(
        parent_started,
        "the stale probe-live delegating parent must register at attach"
    );
    assert_eq!(
        sub_parent_id,
        Some(expected_parent),
        "the subagent must attach linked to the parent, not as an orphan"
    );
    assert_eq!(
        dispatch_is_task,
        Some(true),
        "the replayed pending Agent dispatch must decode as a Task ActivityStart"
    );
    handle.abort();
}

/// #222 — the oversized delegating-parent attach: like S3, but the parent
/// transcript has > MAX_PENDING_BYTES pending at attach, so the backlog is
/// skipped to EOF instead of replayed. The in-flight Agent dispatch sits in
/// the last 256 KiB (TASK_SCAN_BYTES) with no tool_result — the tail scan
/// must re-emit it as a Task ActivityStart, EXACTLY ONCE across the initial
/// seed + rescan + poll cycles, while the completed Bash backlog stays
/// un-replayed. The parent registers via the bounded head read (#204) and the
/// fresh subagent links to it.
#[tokio::test]
async fn oversized_delegating_parent_attach_replays_pending_dispatch() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let proj = projects_root.join("-Users-me-bigdeleg");
    let parent_uuid = "ab000000-0000-7000-8000-0000000000ab";
    let sub_dir = proj.join(parent_uuid).join("subagents");
    tokio::fs::create_dir_all(&sub_dir).await.unwrap();

    // Parent: head session_start (cwd on line 1), > 1 MiB of completed-tool
    // backlog, then the pending Agent dispatch at the tail.
    let parent_path = proj.join(format!("{parent_uuid}.jsonl"));
    let mut contents = format!(
        "{}\n",
        cc_session_start_line(parent_uuid, "/Users/me/bigdeleg")
    );
    let backlog_line = format!(
        "{}\n",
        cc_tool_use_line(
            parent_uuid,
            "/Users/me/bigdeleg",
            "tu_backlog",
            "Bash",
            serde_json::json!({"command": "ls"}),
        )
    );
    while contents.len() <= (1usize << 20) + 4096 {
        contents.push_str(&backlog_line);
    }
    contents.push_str(&format!(
        "{}\n",
        cc_tool_use_line(
            parent_uuid,
            "/Users/me/bigdeleg",
            "tu_task",
            "Agent",
            serde_json::json!({
                "description": "explore", "subagent_type": "code-explorer", "prompt": "go"
            }),
        )
    ));
    tokio::fs::write(&parent_path, contents.as_bytes())
        .await
        .unwrap();
    write_lines(
        &sub_dir.join("agent-b1.jsonl"),
        &[cc_subagent_line("agent-b1", "/Users/me/bigdeleg", "tu_s")],
    )
    .await;

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(256);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    let expected_parent = AgentId::from_parts("claude-code", parent_uuid);
    let is_parent_start = |e: &AgentEvent| {
        matches!(e, AgentEvent::SessionStart { agent_id, parent_id: None, .. }
            if *agent_id == expected_parent)
    };
    let is_sub_start = |e: &AgentEvent| {
        matches!(
            e,
            AgentEvent::SessionStart {
                parent_id: Some(_),
                ..
            }
        )
    };
    let is_task_start = |e: &AgentEvent| {
        matches!(e, AgentEvent::ActivityStart { agent_id, tool_use_id, detail }
            if *agent_id == expected_parent
                && tool_use_id.as_deref() == Some("tu_task")
                && detail.as_ref().is_some_and(|d| d.is_task()))
    };

    let mut events: Vec<(Transport, AgentEvent)> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(pair)) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            events.push(pair);
        }
        if events.iter().any(|(_, e)| is_parent_start(e))
            && events.iter().any(|(_, e)| is_sub_start(e))
            && events.iter().any(|(_, e)| is_task_start(e))
        {
            break;
        }
    }
    // Settle past the 250ms rescan + several poll cycles, then drain: the
    // scan runs only on the oversized-skip pass, so re-scans (cursor parked
    // at EOF) must not re-emit the dispatch.
    tokio::time::sleep(Duration::from_millis(700)).await;
    while let Ok(Some(pair)) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
        events.push(pair);
    }

    assert!(
        events.iter().any(|(_, e)| is_parent_start(e)),
        "the oversized delegating parent must register at attach (#204 head read), got {events:?}"
    );
    let sub_parents: Vec<Option<AgentId>> = events
        .iter()
        .filter_map(|(_, e)| match e {
            AgentEvent::SessionStart {
                parent_id: Some(pid),
                ..
            } => Some(Some(*pid)),
            _ => None,
        })
        .collect();
    assert_eq!(
        sub_parents,
        vec![Some(expected_parent)],
        "the fresh subagent must attach linked to the parent, exactly once"
    );
    let task_starts: Vec<Transport> = events
        .iter()
        .filter(|(_, e)| is_task_start(e))
        .map(|(t, _)| *t)
        .collect();
    assert_eq!(
        task_starts,
        vec![Transport::Jsonl],
        "the in-flight dispatch must be re-emitted exactly once, Jsonl-tagged (it passes the hook-wins dedup at mid-attach)"
    );
    let backlog_replays = events
        .iter()
        .filter(|(_, e)| {
            matches!(e, AgentEvent::ActivityStart { tool_use_id, .. }
                if tool_use_id.as_deref() == Some("tu_backlog"))
        })
        .count();
    assert_eq!(
        backlog_replays, 0,
        "the completed Bash backlog must not be replayed — this is a Task-seeding scan, not a replay"
    );
    handle.abort();
}

/// S4 — the #203 identity property pinned for the ATTACH path: a worktree
/// cwd-split puts the parent transcript and the subagent transcript under
/// DIFFERENT project dirs. At attach time the parent is stale-but-probe-live
/// and the subagent is fresh; both must register, and the `<parent-uuid>`
/// join must hold across the project dirs.
#[tokio::test]
async fn cwd_split_attach_links_subagent_to_probe_live_stale_parent() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let parent_uuid = "ff000000-0000-7000-8000-00000000000f";

    let proj_a = projects_root.join("-Users-me-wt-main");
    tokio::fs::create_dir_all(&proj_a).await.unwrap();
    let parent_path = proj_a.join(format!("{parent_uuid}.jsonl"));
    write_lines(
        &parent_path,
        &[cc_session_start_line(parent_uuid, "/Users/me/wt-main")],
    )
    .await;
    backdate(&parent_path, 7200);

    let sub_dir = projects_root
        .join("-Users-me-wt-feature")
        .join(parent_uuid)
        .join("subagents");
    tokio::fs::create_dir_all(&sub_dir).await.unwrap();
    write_lines(
        &sub_dir.join("agent-w1.jsonl"),
        &[cc_subagent_line("agent-w1", "/Users/me/wt-feature", "tu_w")],
    )
    .await;

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let watcher = cc_watcher(projects_root.clone())
        .with_initial_window(Duration::from_secs(60))
        .with_liveness_probe(std::sync::Arc::new(move || vouch_snapshot(&[parent_uuid])));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    let mut parent_agent_id: Option<AgentId> = None;
    let mut sub_parent_id: Option<AgentId> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((
            _,
            AgentEvent::SessionStart {
                agent_id,
                parent_id,
                ..
            },
        ))) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
        {
            match parent_id {
                Some(pid) => sub_parent_id = Some(pid),
                None => parent_agent_id = Some(agent_id),
            }
        }
        if parent_agent_id.is_some() && sub_parent_id.is_some() {
            break;
        }
    }
    let parent_agent_id =
        parent_agent_id.expect("the stale probe-live parent must register at attach");
    let sub_parent_id = sub_parent_id.expect("the fresh subagent must register at attach");
    assert_eq!(
        parent_agent_id,
        AgentId::from_parts("claude-code", parent_uuid),
        "the parent registers on its session UUID"
    );
    assert_eq!(
        sub_parent_id, parent_agent_id,
        "the UUID join must link the subagent to the parent across project dirs (a worktree cwd-split) on the attach path"
    );
    handle.abort();
}
