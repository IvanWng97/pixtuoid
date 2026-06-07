use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::source::decoder::{cwd_basename_label, make_tool_detail};
use crate::source::hook::HookSocketListener;
use crate::source::jsonl::JsonlWatcher;
use crate::source::{Activity, AgentEvent, Source, TaggedSender};
use crate::AgentId;

pub const SOURCE_NAME: &str = "claude-code";

pub struct ClaudeCodeSource {
    pub socket_path: PathBuf,
    pub projects_root: PathBuf,
}

impl ClaudeCodeSource {
    pub fn default_socket_path() -> PathBuf {
        if let Ok(p) = std::env::var("PIXTUOID_SOCKET") {
            return PathBuf::from(p);
        }
        if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
            return PathBuf::from(format!("{dir}/pixtuoid.sock"));
        }
        // SAFETY: getuid() is a trivial syscall with no pointer args; cannot fail.
        let uid = unsafe { libc::getuid() };
        PathBuf::from(format!("/tmp/pixtuoid-{uid}.sock"))
    }

    pub fn default_paths() -> Self {
        let home = crate::platform::user_home();
        Self {
            socket_path: Self::default_socket_path(),
            projects_root: PathBuf::from(home).join(".claude").join("projects"),
        }
    }
}

#[async_trait]
impl Source for ClaudeCodeSource {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        let socket = HookSocketListener::bind(self.socket_path.clone()).await?;
        let watcher = JsonlWatcher::new(
            self.projects_root.clone(),
            SOURCE_NAME.to_string(),
            decode_cc_line,
            cc_derive_label,
            cc_session_ended,
        );

        let tx_hook = tx.clone();
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
    let agent_id = AgentId::from_parts(source, transcript_path);
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
                    activity: Activity::Typing,
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
        // CC writes no `session_end` line on a clean `/exit`; it logs the
        // slash command as a string-valued user message. Treat `/exit`+`/quit`
        // as a durable SessionEnd so the JSONL transport reaps the session even
        // when the best-effort SessionEnd hook is dropped (the hook races CC's
        // own teardown and has no retry). See `is_exit_command`.
        ("user", Some(Value::String(s))) if is_exit_command(s) => {
            out.push(AgentEvent::SessionEnd { agent_id });
        }
        _ => {}
    }
    Ok(out)
}

/// True if a CC user-message content string is a session-terminating slash
/// command (`/exit` or `/quit`). CC logs slash commands as a `<command-name>`
/// wrapper. Only the two that actually end the session count — `/clear` and
/// `/compact` keep it alive, and prose merely mentioning `/exit` is not wrapped.
fn is_exit_command(content: &str) -> bool {
    content.contains("<command-name>/exit</command-name>")
        || content.contains("<command-name>/quit</command-name>")
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
        // A `/exit` or `/quit` user event ends the session too (CC writes no
        // `session_end` line for it) — without this, a recently-exited session
        // re-ghosts on restart within the mtime window. Same matcher as the
        // live decode path so the two transports agree.
        if v.get("type").and_then(|s| s.as_str()) == Some("user") {
            if let Some(c) = v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
            {
                if is_exit_command(c) {
                    last_is_end = true;
                }
            }
        }
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
    // ONE shared predicate with `detect_parent_id` (both via `SUBAGENTS_SEGMENT`)
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
    #[test]
    fn default_socket_path_env_precedence_and_default_paths() {
        let saved_socket = std::env::var_os("PIXTUOID_SOCKET");
        let saved_xdg = std::env::var_os("XDG_RUNTIME_DIR");

        // PIXTUOID_SOCKET takes precedence (checked first).
        std::env::set_var("PIXTUOID_SOCKET", "/tmp/explicit.sock");
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        assert_eq!(
            ClaudeCodeSource::default_socket_path(),
            PathBuf::from("/tmp/explicit.sock")
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

        // default_paths derives projects_root from HOME.
        let paths = ClaudeCodeSource::default_paths();
        assert!(
            paths.projects_root.ends_with(".claude/projects"),
            "projects_root must end with .claude/projects, got {:?}",
            paths.projects_root
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
}
