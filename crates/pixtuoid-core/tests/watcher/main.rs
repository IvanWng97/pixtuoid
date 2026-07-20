mod attach;
mod first_sight;
mod liveness;
mod sources;
mod tailing;
mod unclaim;

use std::time::{Duration, SystemTime};

use filetime::{set_file_mtime, FileTime};

use pixtuoid_core::source::claude_code::{
    cc_derive_label, cc_id_from_path, cc_session_ended, decode_cc_line,
};
use pixtuoid_core::source::jsonl::{force_polling_backend_for_tests, JsonlWatcher, ProbeSnapshot};

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
    // The live id set IS `pid_of`'s key set; these admission/proof-of-life
    // tests don't exercise the id→pid exit-watch join (that's
    // `vouch_snapshot_with_pid`), so bind each id to this live process's pid —
    // a placeholder that can never spuriously instant-exit.
    let pid = std::process::id() as i32;
    Some(ProbeSnapshot {
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
        cc_session_ended,
    )
    .with_id_deriver(cc_id_from_path)
    .with_label_deriver(cc_derive_label)
}

// ── Shared watcher harness + builders ───────────────────────────────────────
// Fixture writers (`write_lines`, `backdate`), the CC transcript line
// builders, and probe-snapshot helpers — consumed by attach, first_sight,
// liveness, and unclaim alike. The mid-attach suite narrative lives atop
// `attach.rs`.

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
