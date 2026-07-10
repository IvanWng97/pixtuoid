use std::time::Duration;

use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use pixtuoid_core::source::antigravity::AntigravitySource;
use pixtuoid_core::source::claude_code::ClaudeCodeSource;
use pixtuoid_core::source::codex::CodexSource;
use pixtuoid_core::source::copilot::CopilotSource;
use pixtuoid_core::source::AgentEvent;
use pixtuoid_core::source::Source;
use pixtuoid_core::source::Transport;

use crate::fast_watch;

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

// CC is now a PURE transcript watcher (the hook socket lifted to `HookRouter`) —
// drive the real Source impl so the watcher spawn + run glue is exercised. A CC
// transcript written under the projects_root must surface a SessionStart through
// the JSONL leg, with NO hook socket involved at all (platform-neutral now).
#[tokio::test]
async fn claude_code_source_run_emits_session_start_from_jsonl() {
    fast_watch();
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().join("projects");
    let project_dir = projects_root.join("proj-cc");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-cc.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let mut src = ClaudeCodeSource::default_paths();
    src.projects_root = projects_root;
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

// CopilotSource::run drives a JsonlWatcher over <sessions_root>/<id>/events.jsonl
// with the parent-dir id-deriver. The e2e gap that unit tests can't cover: does
// the real watcher RECURSE into the <sessionId>/ dir, pick up the constant-named
// `events.jsonl`, derive the id from the PARENT DIR (not the "events" stem), and
// surface a copilot SessionStart? Driven against real-shaped bytes.
#[tokio::test]
async fn copilot_source_run_emits_session_start_from_events_jsonl() {
    fast_watch();
    let dir = TempDir::new().unwrap();
    let sessions_root = dir.path().to_path_buf();
    let session_dir = sessions_root.join("65f8cef9-7dd8-46fa-9f6a-78cc95f68ab3");
    tokio::fs::create_dir_all(&session_dir).await.unwrap();
    let transcript = session_dir.join("events.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let src = CopilotSource { sessions_root };
    let handle = tokio::spawn(async move { Box::new(src).run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Real session.start shape + a tool round (single agent → one AgentId).
    let start = serde_json::json!({
        "type": "session.start",
        "data": {"sessionId": "65f8cef9-7dd8-46fa-9f6a-78cc95f68ab3", "version": 1,
                 "producer": "copilot-agent", "context": {"cwd": "/repo"}},
        "id": "a", "timestamp": "2026-05-22T05:59:45.488Z", "parentId": serde_json::Value::Null
    });
    let tool = serde_json::json!({
        "type": "tool.execution_start",
        "data": {"toolCallId": "tooluse_1", "toolName": "view", "arguments": {"path": "/repo"}},
        "id": "b", "timestamp": "2026-05-22T06:00:14.298Z", "parentId": "a"
    });
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(format!("{start}\n{tool}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut session_id = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((
            _,
            AgentEvent::SessionStart {
                source,
                session_id: sid,
                ..
            },
        ))) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            assert_eq!(source, "copilot");
            session_id = Some(sid);
            break;
        }
    }
    assert_eq!(
        session_id.as_deref(),
        Some("65f8cef9-7dd8-46fa-9f6a-78cc95f68ab3"),
        "CopilotSource::run should surface a copilot SessionStart from events.jsonl"
    );
    handle.abort();
}

// OmpSource::run drives a JsonlWatcher over the DEEPEST nesting yet:
// <sessions_root>/<encoded-cwd>/<ts>_<uuid>.jsonl with a subagent child at
// <sessions_root>/<encoded-cwd>/<ts>_<uuid>/<taskId>.jsonl. The e2e gap unit
// tests can't cover: does the real watcher recurse BOTH levels, key the root
// on its stem and the child on the stem CHAIN, and surface the child's header
// SessionStart parent-linked to the root? Driven against real-shaped bytes.
#[tokio::test]
async fn omp_source_run_links_a_nested_subagent_to_its_root() {
    use pixtuoid_core::source::omp::OmpSource;
    use pixtuoid_core::AgentId;

    fast_watch();
    let dir = TempDir::new().unwrap();
    let sessions_root = dir.path().to_path_buf();
    const ROOT_STEM: &str = "2026-07-09T08-00-00-000Z_0197f0aa-0000-7000-8000-000000000001";
    let cwd_dir = sessions_root.join("-dev-proj");
    let child_dir = cwd_dir.join(ROOT_STEM);
    tokio::fs::create_dir_all(&child_dir).await.unwrap();
    let root_transcript = cwd_dir.join(format!("{ROOT_STEM}.jsonl"));
    let child_transcript = child_dir.join("Alpha.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let src = OmpSource { sessions_root };
    let handle = tokio::spawn(async move { Box::new(src).run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let header = |id: &str| {
        serde_json::json!({
            "type": "session", "version": 3, "id": id,
            "timestamp": "2026-07-09T08:00:00.000Z", "cwd": "/dev/proj"
        })
    };
    for (path, id) in [
        (&root_transcript, "0197f0aa-0000-7000-8000-000000000001"),
        (&child_transcript, "0197f0cc-0000-7000-8000-000000000003"),
    ] {
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .unwrap();
        f.write_all(format!("{}\n", header(id)).as_bytes())
            .await
            .unwrap();
        f.flush().await.unwrap();
    }

    // Collect until the CHILD's parented SessionStart shows (root + child both
    // also arrive via first-sight; only the header-decoded child carries the
    // parent link). Expected ids go through the SAME seam fold + deriver the
    // watcher uses (walk.rs `id_path` → `omp_id_from_path`) — a raw-case
    // literal here passes on Unix and fails ONLY on windows-test, where the
    // id-space is normalize_path_key-folded (the path-fold
    // expectation-literal class).
    use pixtuoid_core::source::omp::omp_id_from_path;
    let watcher_key = |p: &std::path::Path| {
        omp_id_from_path(std::path::Path::new(
            &pixtuoid_core::id::normalize_path_key(&p.to_string_lossy()),
        ))
    };
    let root_id = AgentId::from_parts("omp", &watcher_key(&root_transcript));
    let child_id = AgentId::from_parts("omp", &watcher_key(&child_transcript));
    let mut child_linked = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline && !child_linked {
        if let Ok(Some((
            _,
            AgentEvent::SessionStart {
                agent_id,
                source,
                parent_id: Some(parent),
                ..
            },
        ))) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            assert_eq!(source, "omp");
            assert_eq!(agent_id, child_id, "the parented start must be the child");
            assert_eq!(parent, root_id, "child must link to the root stem's id");
            child_linked = true;
        }
    }
    assert!(
        child_linked,
        "OmpSource::run should surface the nested child's parent-linked SessionStart"
    );
    handle.abort();
}

// The session-ended checker's WIRING into OmpSource's watcher (native.rs) —
// the checker fn is unit-tested, but swapping it for `|_| false` in the
// JsonlWatcher::new call would keep every unit test green while finished
// sessions resurrect at first sight. Paired: the same recent transcript
// seeds a SessionStart WITHOUT the session_exit marker and is gated WITH it,
// so the assert can only pass through the wired checker.
#[tokio::test]
async fn omp_ended_transcript_is_gated_at_first_sight_and_a_live_one_seeds() {
    use pixtuoid_core::source::omp::OmpSource;

    fast_watch();
    let header = r#"{"type":"session","version":3,"id":"0197","timestamp":"2026-07-09T08:00:00.000Z","cwd":"/dev/proj"}"#;
    let exit = r#"{"type":"custom","id":"e1","parentId":null,"timestamp":"2026-07-09T08:10:00.000Z","customType":"session_exit","data":{"reason":"exit command","kind":"normal","recordedAt":"2026-07-09T08:10:00.000Z"}}"#;

    for (name, content, expect_start) in [
        ("live", format!("{header}\n"), true),
        ("ended", format!("{header}\n{exit}\n"), false),
    ] {
        let dir = TempDir::new().unwrap();
        let sessions_root = dir.path().to_path_buf();
        let cwd_dir = sessions_root.join("-dev-proj");
        tokio::fs::create_dir_all(&cwd_dir).await.unwrap();
        // Pre-existing, mtime "now" — inside the default initial window, so
        // ONLY the ended-marker gate can suppress the seed.
        std::fs::write(
            cwd_dir.join("2026-07-09T08-00-00-000Z_0197f0aa-0000-7000-8000-000000000001.jsonl"),
            &content,
        )
        .unwrap();

        let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
        let src = OmpSource { sessions_root };
        let handle = tokio::spawn(async move { Box::new(src).run(tx).await });

        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(if expect_start { 15 } else { 1 });
        let mut saw_start = false;
        while tokio::time::Instant::now() < deadline && !saw_start {
            if let Ok(Some((_, AgentEvent::SessionStart { .. }))) =
                tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
            {
                saw_start = true;
            }
        }
        assert_eq!(
            saw_start, expect_start,
            "{name}: expected SessionStart={expect_start} for a recent pre-existing transcript"
        );
        handle.abort();
    }
}
