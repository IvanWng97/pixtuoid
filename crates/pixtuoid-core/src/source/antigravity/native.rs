//! The `native`-only runtime half of the Antigravity source:
//! `AntigravitySource` and its `JsonlWatcher` wiring (label deriver +
//! session-ended checker included — only the watcher reads them). The pure
//! decoder stays in the always-compiled parent module; this whole file sits
//! behind the parent's ONE `#[cfg(feature = "native")] mod native;` gate and
//! is re-exported there, so public paths don't move.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{decode_ag_line, SOURCE_NAME};
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
            ag_session_ended,
        )
        .with_path_filter(skip_transcript_full);
        watcher.run(tx).await
    }
}

fn ag_session_ended(_tail: &[u8]) -> bool {
    false
}

/// Antigravity-cli writes BOTH `transcript.jsonl` (truncated) and
/// `transcript_full.jsonl` (untruncated) per conversation in one
/// `.../logs/` dir, carrying the SAME `step_index` stream — so walking both
/// mints two path-keyed `AgentId`s and double-renders the conversation. Watch
/// only the canonical `transcript.jsonl` (also the shorter one, so less likely
/// to trip the >1 MiB oversized-skip); the decoder ignores content length, so
/// dropping the untruncated copy loses nothing. Narrow by construction — it
/// skips ONLY the known duplicate, never an unrelated `.jsonl`.
///
/// Accepted residual: a dir with ONLY `transcript_full.jsonl` (a brief
/// write-order race before `transcript.jsonl` lands, or a future AG that drops
/// the truncated file) renders nothing and — unlike a step-type rename, which
/// trips `drift::unknown_event` — emits NO drift breadcrumb, because the
/// `fn(&Path) -> bool` filter can't see the sibling to fall back on. It
/// self-heals once `transcript.jsonl` appears, and is strictly better than the
/// every-conversation double-render it replaces.
fn skip_transcript_full(path: &Path) -> bool {
    path.file_name().and_then(|s| s.to_str()) != Some("transcript_full.jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ag_session_ended_is_always_false() {
        // Antigravity writes no end marker — defer to mtime + stale-sweep.
        assert!(!ag_session_ended(b"x"));
        assert!(!ag_session_ended(b""));
    }

    #[test]
    fn skip_transcript_full_drops_only_the_duplicate() {
        // The conversation's canonical transcript is walked; its untruncated
        // twin is skipped so one conversation renders one sprite. Any OTHER
        // `.jsonl` is admitted (the filter is narrow by construction).
        let dir = Path::new("/h/.gemini/antigravity-cli/brain/c1/.system_generated/logs");
        assert!(skip_transcript_full(&dir.join("transcript.jsonl")));
        assert!(!skip_transcript_full(&dir.join("transcript_full.jsonl")));
        assert!(skip_transcript_full(&dir.join("other.jsonl")));
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
