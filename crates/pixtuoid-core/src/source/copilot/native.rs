//! The `native`-only runtime half of the Copilot source: `CopilotSource`, its
//! `JsonlWatcher` wiring, and the first-sight session-ended checker (only the
//! watcher's gate reads it). The pure decoder stays in the always-compiled
//! parent module; this whole file sits behind the parent's ONE
//! `#[cfg(feature = "native")] mod native;` gate and is re-exported there, so
//! public paths don't move.

use std::path::PathBuf;

use anyhow::Result;

use super::{
    copilot_home, copilot_id_from_path, decode_copilot_line, derive_copilot_label, SOURCE_NAME,
};
use crate::source::decoder::parsed_tail_lines;
use crate::source::jsonl::JsonlWatcher;
use crate::source::{Source, TaggedSender};

/// Copilot persists a real `session.shutdown` event, so a transcript that has
/// already ended carries that marker — the first-sight gate uses it to avoid
/// resurrecting a finished session.
fn copilot_session_ended(tail: &[u8]) -> bool {
    // Structural top-level `type` per parsed line (via the shared
    // `parsed_tail_lines` scaffold). A substring scan is falsifiable by
    // CONTENT: copilot persists tool `arguments` structurally in events.jsonl,
    // so a grep for `session_end` would land the quoted marker bytes verbatim —
    // content must never drive lifecycle (the CC sharp edge). `session.shutdown`
    // is the real on-disk marker (drift-watched); `session_end` stays a
    // defensive alias, anchored on the structural field.
    parsed_tail_lines(tail).any(|v| {
        matches!(
            v.get("type").and_then(|t| t.as_str()),
            Some("session.shutdown" | "session_end")
        )
    })
}

/// Source that watches the Copilot session-state directory.
pub struct CopilotSource {
    pub sessions_root: PathBuf,
}

impl CopilotSource {
    pub fn default_paths() -> Self {
        Self {
            sessions_root: copilot_home().join("session-state"),
        }
    }
}

impl Source for CopilotSource {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        JsonlWatcher::new(
            self.sessions_root.clone(),
            SOURCE_NAME.to_string(),
            decode_copilot_line,
            derive_copilot_label,
            copilot_session_ended,
        )
        .with_id_deriver(copilot_id_from_path)
        .run(tx)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ended_marker_is_anchored_on_the_type_field() {
        // Real compact on-disk shape → ended.
        assert!(copilot_session_ended(
            br#"{"type":"session.shutdown","data":{}}"#
        ));
        // A tool result that merely MENTIONS the string must NOT end the session
        // (content must never drive lifecycle — the CC sharp edge).
        assert!(!copilot_session_ended(
            br#"{"type":"tool.execution_complete","data":{"result":{"content":"run session.shutdown the cluster"}}}"#
        ));
        assert!(!copilot_session_ended(
            br#"{"type":"tool.execution_start"}"#
        ));
    }

    #[test]
    fn session_ended_matches_marker_after_a_partial_first_tail_line() {
        // The 8 KiB tail window usually opens mid-line; the leading fragment
        // must be skipped without defeating the real marker on a later line.
        assert!(copilot_session_ended(
            b"...tail-fragment\"}\n{\"type\":\"session.shutdown\",\"data\":{}}\n"
        ));
    }

    #[test]
    fn session_ended_ignores_marker_bytes_inside_tool_arguments() {
        // Copilot persists tool `arguments` STRUCTURALLY in events.jsonl, so a
        // grep/glob run with pattern `session_end` puts the quoted marker
        // bytes in the tail verbatim (`"pattern":"session_end"`). Only the
        // structural top-level `type` field may end the session — content
        // must never drive lifecycle.
        assert!(!copilot_session_ended(
            br#"{"type":"tool.execution_start","data":{"toolName":"grep","arguments":{"pattern":"session_end"}}}"#
        ));
        // Even the fully-anchored needle as argument CONTENT stays inert (the
        // JSON string form escapes its quotes, so a structural parse can't
        // confuse it with a real `type` field).
        assert!(!copilot_session_ended(
            br#"{"type":"tool.execution_start","data":{"arguments":{"pattern":"\"type\":\"session.shutdown\""}}}"#
        ));
        // Nested `type` keys deeper in the object are not the top-level field.
        assert!(!copilot_session_ended(
            br#"{"type":"tool.execution_complete","data":{"result":{"type":"session_end"}}}"#
        ));
    }
}
