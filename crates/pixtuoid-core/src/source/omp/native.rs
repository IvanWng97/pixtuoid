//! The `native`-only runtime half of the omp source: `OmpSource`, its
//! `JsonlWatcher` wiring, and the first-sight session-ended checker. The pure
//! decoder stays in the always-compiled parent module; this whole file sits
//! behind the parent's ONE `#[cfg(feature = "native")] mod native;` gate and
//! is re-exported there, so public paths don't move.

use std::path::PathBuf;

use anyhow::Result;
use serde_json::Value;

use super::{decode_omp_line, derive_omp_label, omp_agent_dir, omp_id_from_path, SOURCE_NAME};
use crate::source::jsonl::JsonlWatcher;
use crate::source::{Source, TaggedSender};

/// omp appends a `custom` entry `customType:"session_exit"` on every clean
/// teardown (incl. SIGINT/SIGTERM — upstream `agent-session.ts::
/// #recordSessionExit`), so a transcript that already ended carries that
/// marker — the first-sight gate uses it to avoid resurrecting a finished
/// session. Structural parse only (top-level `type` + `customType`): tool
/// arguments/results are persisted verbatim in the same file, so a substring
/// scan would let CONTENT (e.g. a grep for `session_exit`) end a live session
/// — the CC sharp edge.
fn omp_session_ended(tail: &[u8]) -> bool {
    tail.split(|b| *b == b'\n').any(|line| {
        if line.is_empty() {
            return false;
        }
        let Ok(s) = std::str::from_utf8(line) else {
            return false;
        };
        let Ok(v) = serde_json::from_str::<Value>(s) else {
            return false;
        };
        v.get("type").and_then(|t| t.as_str()) == Some("custom")
            && v.get("customType").and_then(|c| c.as_str()) == Some("session_exit")
    })
}

/// Source that watches the omp sessions directory (recursively — root
/// transcripts sit under per-cwd encoded dirs, subagent transcripts nest one
/// level deeper per delegation).
pub struct OmpSource {
    pub sessions_root: PathBuf,
}

impl OmpSource {
    pub fn default_paths() -> Self {
        Self {
            sessions_root: omp_agent_dir().join("sessions"),
        }
    }
}

impl Source for OmpSource {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        JsonlWatcher::new(
            self.sessions_root.clone(),
            SOURCE_NAME.to_string(),
            decode_omp_line,
            derive_omp_label,
            omp_session_ended,
        )
        .with_id_deriver(omp_id_from_path)
        .run(tx)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ended_marker_is_anchored_on_the_structural_fields() {
        // Real on-disk shape → ended.
        assert!(omp_session_ended(
            br#"{"type":"custom","id":"a","parentId":null,"timestamp":"t","customType":"session_exit","data":{"reason":"exit command","kind":"normal","recordedAt":"t"}}"#
        ));
        // A DIFFERENT customType must not end the session.
        assert!(!omp_session_ended(
            br#"{"type":"custom","customType":"tool_execution_start","data":{"toolCallId":"t1"}}"#
        ));
        // Marker bytes inside tool CONTENT must not end the session (content
        // must never drive lifecycle).
        assert!(!omp_session_ended(
            br#"{"type":"message","message":{"role":"toolResult","toolCallId":"t1","content":[{"type":"text","text":"grep hit: \"customType\":\"session_exit\""}]}}"#
        ));
        assert!(!omp_session_ended(br#"{"type":"session","cwd":"/p"}"#));
    }

    #[test]
    fn session_ended_matches_marker_after_a_partial_first_tail_line() {
        // The tail window usually opens mid-line; the leading fragment must be
        // skipped without defeating the real marker on a later line.
        assert!(omp_session_ended(
            b"...tail-fragment\"}\n{\"type\":\"custom\",\"customType\":\"session_exit\",\"data\":{}}\n"
        ));
    }

    #[test]
    fn default_paths_points_at_the_agent_sessions_dir() {
        let src = OmpSource::default_paths();
        assert!(
            src.sessions_root.ends_with("sessions"),
            "got {:?}",
            src.sessions_root
        );
    }
}
