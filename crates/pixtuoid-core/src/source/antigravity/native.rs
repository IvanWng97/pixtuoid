//! The `native`-only runtime half of the Antigravity source:
//! `AntigravitySource` and its `JsonlWatcher` wiring (label deriver +
//! session-ended checker included — only the watcher reads them). The pure
//! decoder stays in the always-compiled parent module; this whole file sits
//! behind the parent's ONE `#[cfg(feature = "native")] mod native;` gate and
//! is re-exported there, so public paths don't move.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{decode_ag_line, SOURCE_NAME};
use crate::source::decoder::cwd_basename_label;
use crate::source::jsonl::JsonlWatcher;
use crate::source::{Source, TaggedSender};

/// Source that watches Antigravity CLI conversation log directories.
/// Uses JsonlWatcher with a custom decoder for the Antigravity JSONL
/// format (step_index/PLANNER_RESPONSE/tool_calls schema).
pub struct AntigravitySource {
    pub brain_root: PathBuf,
}

impl AntigravitySource {
    /// The Antigravity **CLI** (`agy`) brain dir, home-rooted on every platform:
    /// `<home>/.gemini/antigravity-cli/brain` (Windows: `%USERPROFILE%\.gemini\…`
    /// via `user_home()` — the brain is NOT under `%APPDATA%`/`%LOCALAPPDATA%`;
    /// only the IDE's editor settings and the `agy.exe` binary live there).
    /// Note `antigravity-cli` (the CLI), NOT `antigravity` (the IDE's brain at
    /// `~/.gemini/antigravity/brain`) — don't "fix" this to the IDE path.
    pub fn default_paths() -> Self {
        let home = crate::platform::user_home();
        Self {
            brain_root: PathBuf::from(home)
                .join(".gemini")
                .join("antigravity-cli")
                .join("brain"),
        }
    }
}

impl Source for AntigravitySource {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        let watcher = JsonlWatcher::new(
            self.brain_root.clone(),
            SOURCE_NAME.to_string(),
            decode_ag_line,
            derive_ag_label,
            ag_session_ended,
        );
        watcher.run(tx).await
    }
}

fn ag_session_ended(_tail: &[u8]) -> bool {
    false
}

fn derive_ag_label(_path: &Path, _source: &str, cwd: &Path) -> String {
    cwd_basename_label("ag", cwd).unwrap_or_else(|| "ag".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_is_ag_basename_or_bare_prefix() {
        assert_eq!(
            derive_ag_label(
                Path::new("/x"),
                SOURCE_NAME,
                Path::new("/Users/me/dotfiles")
            ),
            "ag·dotfiles"
        );
        // Empty / root cwd fall back to the bare prefix.
        assert_eq!(
            derive_ag_label(Path::new("/x"), SOURCE_NAME, Path::new("")),
            "ag"
        );
        assert_eq!(
            derive_ag_label(Path::new("/x"), SOURCE_NAME, Path::new("/")),
            "ag"
        );
    }

    #[test]
    fn ag_session_ended_is_always_false() {
        // Antigravity writes no end marker — defer to mtime + stale-sweep.
        assert!(!ag_session_ended(b"x"));
        assert!(!ag_session_ended(b""));
    }

    // The brain dir is the CLI's (`antigravity-cli`), home-rooted, on every OS —
    // the suffix is separator-agnostic so this pins it on Unix AND Windows. The
    // USERPROFILE-vs-HOME rooting itself is covered by platform::user_home tests.
    #[test]
    fn brain_root_is_the_cli_brain_under_dot_gemini() {
        let p = AntigravitySource::default_paths().brain_root;
        assert!(
            p.ends_with(
                PathBuf::from(".gemini")
                    .join("antigravity-cli")
                    .join("brain")
            ),
            "brain_root must be <home>/.gemini/antigravity-cli/brain, got {p:?}"
        );
    }
}
