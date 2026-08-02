//! The `native`-only runtime half of the Claude Code source:
//! `ClaudeCodeSource` (the pure `~/.claude/projects` JSONL watcher), the
//! shim↔daemon socket-path anchor, and the liveness-probe re-export. The pure
//! decoder stays in the always-compiled parent module; this whole file sits
//! behind the parent's ONE `#[cfg(feature = "native")] mod native;` gate and
//! is re-exported there, so public paths don't move.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{
    cc_derive_label, cc_id_from_path, cc_session_ended, claude_config_dir, decode_cc_line,
    SOURCE_NAME,
};
use crate::source::cc_probe::cc_sessions_dir;
// The registry-probe machinery lives in `source/cc_probe.rs`; the public path
// `claude_code::live_cc_session_ids` is preserved via this re-export chain.
pub use crate::source::cc_probe::live_cc_session_ids;
use crate::source::jsonl::{ChildEndUnclaims, JsonlWatcher};
use crate::source::{Source, TaggedSender};

/// A pure CC transcript watcher. The shared hook socket it used to host moved to
/// [`crate::source::hook::HookRouter`] (its honest owner — every source's hooks
/// ride that one socket), so this is now ONLY the `~/.claude/projects` JSONL
/// watcher: no socket bind, no presence/pid plumbing, no dual-task `select!`.
pub struct ClaudeCodeSource {
    /// The watched `~/.claude/projects` transcript root.
    pub projects_root: PathBuf,
    /// The #246 child-end un-claim side-channel — CONSUMER only now (its watcher
    /// releases CC child-transcript claims on the rare blocked-stop continuation;
    /// the PRODUCER tee lives on the `HookRouter`). The runtime shares ONE handle
    /// across the router (producer) + the CC and Codex watchers (consumers);
    /// `None` disables it (bare test construction).
    pub child_end_unclaims: Option<ChildEndUnclaims>,
}

impl ClaudeCodeSource {
    // The resolved hook-socket path. It stays on `ClaudeCodeSource` (CC no longer
    // binds the socket — the `HookRouter` does — but this is the shim↔daemon
    // parity bridge anchor, pinned by tests/socket_path_parity.rs). The driver
    // resolves it here and hands it to `HookRouter::new`.
    /// Resolve the hook rendezvous path — a Unix socket (or Windows named pipe) the shim connects to and the daemon binds.
    pub fn default_socket_path() -> PathBuf {
        if let Ok(p) = std::env::var("PIXTUOID_SOCKET") {
            // Set-but-empty/whitespace = unset (the #172 RUST_LOG policy): an
            // empty value falls through to the default below rather than being
            // bound verbatim. Same shape as pixtuoid-hook's paths.rs.
            if !p.trim().is_empty() {
                return PathBuf::from(p);
            }
        }
        #[cfg(unix)]
        {
            // XDG spec: absolute-only. Empty → `/pixtuoid.sock` (fatal bind); relative →
            // shim/daemon cwd mis-rendezvous → treated as unset. Parity with paths.rs.
            if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
                if !dir.trim().is_empty() && std::path::Path::new(&dir).is_absolute() {
                    return PathBuf::from(format!("{dir}/pixtuoid.sock"));
                }
            }
            // No XDG_RUNTIME_DIR (macOS, bare Linux): a per-user SUBDIR the bind
            // creates 0700-owned-by-us (`hook::unix::ensure_owned_socket_dir`),
            // NOT a flat predictable `pixtuoid-{uid}.sock` a co-located user
            // could squat/lock to silently kill the hook plane (#485). Built
            // from the same `owned_socket_dir()` that guard compares against,
            // so the two cannot drift apart. Parity-pinned to the shim's
            // branch 3 by tests/socket_path_parity.rs.
            crate::source::hook::owned_socket_dir().join(crate::source::hook::SOCKET_FILE_NAME)
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

    /// Construct pointed at the default `~/.claude/projects` root.
    pub fn default_paths() -> Self {
        let projects_root = claude_config_dir()
            .unwrap_or_else(|| PathBuf::from(crate::platform::user_home()).join(".claude"))
            .join("projects");
        Self {
            projects_root,
            child_end_unclaims: None,
        }
    }
}

impl Source for ClaudeCodeSource {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        // Pure transcript watcher: the hook socket (and its presence/pid/tee
        // plumbing) lives on the `HookRouter` now. CC keeps only the
        // `child_end_unclaims` CONSUMER (the #246 blocked-stop continuation).
        let mut watcher = JsonlWatcher::new(
            self.projects_root.clone(),
            SOURCE_NAME.to_string(),
            decode_cc_line,
            cc_session_ended,
        )
        .with_id_deriver(cc_id_from_path)
        .with_label_deriver(cc_derive_label)
        .with_activity_recency(super::cc_activity_recency)
        .with_path_filter(skip_workflow_journal);
        if let Some(sessions_dir) = cc_sessions_dir(&self.projects_root) {
            watcher = watcher.with_liveness_probe(std::sync::Arc::new(move || {
                live_cc_session_ids(&sessions_dir)
            }));
        }
        if let Some(unclaims) = &self.child_end_unclaims {
            watcher = watcher.with_child_end_unclaims(unclaims.clone());
        }
        watcher.run(tx).await
    }
}

/// The workflow/skills orchestrator writes a `journal.jsonl` sidecar under
/// `<uuid>/subagents/workflows/wf_*/` — a FOREIGN schema (top-level
/// `type:"started"`/`"result"`, not CC transcript lines) living in the SAME
/// projects tree the CC watcher walks. The watcher has no default path_filter and
/// `walk_jsonl` recurses into every `.jsonl`, so without this skip a recent
/// journal (read from the top — it is not liveness-probed nor `cc_session_ended`)
/// feeds each line to `decode_cc_line`, and the #763 unknown-`type` breadcrumb
/// would FLOOD the warn-floor (avg ~14 `started`+`result` per file, `unknown_event`
/// has no dedup). Skips ONLY `journal.jsonl` — the real subagent transcripts
/// (`agent-<id>.jsonl`, INCLUDING the ones nested under `workflows/wf_*/`) are
/// admitted, so subagent attribution is untouched. A denylist, not an allowlist:
/// misfiltering a real transcript (a vanished sprite) is worse than a future
/// foreign sidecar breadcrumbing (visible, self-heals by adding it here).
fn skip_workflow_journal(path: &Path) -> bool {
    path.file_name().and_then(|s| s.to_str()) != Some("journal.jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_workflow_journal_drops_only_the_orchestrator_journal() {
        // The workflow orchestrator's FOREIGN-schema journal is skipped so its
        // `started`/`result` lines never reach decode_cc_line (the #763 flood).
        let wf = Path::new("/h/.claude/projects/proj/uuid/subagents/workflows/wf_abc");
        assert!(!skip_workflow_journal(&wf.join("journal.jsonl")));
        // The real subagent transcripts nested ALONGSIDE it stay admitted, as do
        // the main + top-level subagent transcripts — subagent attribution is
        // unaffected (a denylist, narrow by construction).
        assert!(skip_workflow_journal(&wf.join("agent-xyz.jsonl")));
        assert!(skip_workflow_journal(Path::new(
            "/h/.claude/projects/proj/uuid.jsonl"
        )));
        assert!(skip_workflow_journal(Path::new(
            "/h/.claude/projects/proj/uuid/subagents/agent-xyz.jsonl"
        )));
    }

    // The socket-path and default-paths env precedence. Every socket branch
    // (incl. the invalid-XDG fallback) is checked in ONE test because the env
    // vars are process-global — splitting would race under the multi-thread runner.
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

        // Invalid (empty/whitespace/relative) XDG_RUNTIME_DIR is unset per the
        // XDG absolute-only spec -> /tmp subdir (parity with paths.rs / the shim).
        let uid = rustix::process::getuid().as_raw();
        let tmp_fallback = PathBuf::from(format!("/tmp/pixtuoid-{uid}/pixtuoid.sock"));
        for invalid in ["", "   ", "relative/run"] {
            std::env::set_var("XDG_RUNTIME_DIR", invalid);
            assert_eq!(ClaudeCodeSource::default_socket_path(), tmp_fallback);
        }

        // With neither set, fall back to the uid-scoped /tmp SUBDIR socket
        // (#485: the bind creates `/tmp/pixtuoid-{uid}/` 0700-owned-by-us).
        std::env::remove_var("XDG_RUNTIME_DIR");
        assert_eq!(
            ClaudeCodeSource::default_socket_path(),
            PathBuf::from(format!("/tmp/pixtuoid-{uid}/pixtuoid.sock"))
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
}
