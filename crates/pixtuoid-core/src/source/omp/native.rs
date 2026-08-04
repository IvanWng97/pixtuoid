use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{decode_omp_line, omp_agent_dir, omp_id_from_path, SOURCE_NAME};
use crate::source::decoder::parsed_tail_lines;
use crate::source::jsonl::{JsonlWatcher, ProbeSnapshot};
use crate::source::{Source, TaggedSender};

/// omp appends a `custom` entry `customType:"session_exit"` on every clean
/// teardown, so a transcript that already ended carries that marker — the
/// first-sight gate uses it to avoid resurrecting a finished session.
/// Structural parse only: tool arguments/results are persisted verbatim in the
/// same file, so a substring scan would let CONTENT (a grep for
/// `session_exit`) end a live session — the CC sharp edge.
fn omp_session_ended(tail: &[u8]) -> bool {
    parsed_tail_lines(tail).any(|v| {
        v.get("type").and_then(|t| t.as_str()) == Some("custom")
            && v.get("customType").and_then(|c| c.as_str()) == Some("session_exit")
    })
}

/// Source that watches the omp sessions directory recursively.
pub struct OmpSource {
    /// The watched omp `sessions` root.
    pub sessions_root: PathBuf,
}

impl OmpSource {
    /// Construct pointed at the default omp `sessions` root.
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
        let mut watcher = JsonlWatcher::new(
            self.sessions_root.clone(),
            SOURCE_NAME.to_string(),
            decode_omp_line,
            omp_session_ended,
        );
        if let Some(root) = omp_probe_root(&self.sessions_root) {
            watcher = watcher
                .with_liveness_probe(std::sync::Arc::new(move || live_omp_session_ids(&root)));
        }
        watcher.run(tx).await
    }
}

/// omp's liveness probe: the session ids (in the `omp_id_from_path` id-space,
/// so they join the watcher's first-sight gate directly) of every transcript
/// under `sessions_root` held OPEN by a running omp process, plus the owning
/// pid per id.
///
/// omp keeps a for-lifetime append fd on its session file, so an open
/// transcript fd IS the first-party liveness signal. omp runs under Bun, so the
/// kernel-truncated process name is `bun`, not `omp` — probe both (a future
/// compiled binary would report `omp`). The fd MODE is not checked, so a
/// long-lived bun tool reading an OLD transcript vouches for it: an accepted
/// intermittent resurrection, reaped again once the fd closes.
fn live_omp_session_ids(sessions_root: &Path) -> Option<ProbeSnapshot> {
    ProbeSnapshot::from_open_fds(
        sessions_root,
        &["bun", "omp"],
        omp_recognize,
        omp_id_from_path,
    )
}

/// The per-source RECOGNIZER: a held-open `.jsonl` under the root vouches. The
/// id-space fold is applied by `from_open_fds`, not here.
fn omp_recognize(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("jsonl")
}

/// Attach the probe ONLY for omp's first-party layout — the standard
/// `~/.omp/agent/sessions` shape or the resolved `omp_agent_dir()/sessions` for
/// THIS environment. A test/replay root pointed at an arbitrary dir must keep
/// the pure-mtime first-sight gate, or a replayed transcript vouched for by a
/// coincidentally running bun process resurrects as live.
fn omp_probe_root(sessions_root: &Path) -> Option<PathBuf> {
    omp_probe_root_resolved(sessions_root, &omp_agent_dir())
}

/// The injectable core of [`omp_probe_root`]: `agent_dir` is the resolved omp
/// agent dir for this environment.
fn omp_probe_root_resolved(sessions_root: &Path, agent_dir: &Path) -> Option<PathBuf> {
    if sessions_root.file_name().and_then(|n| n.to_str()) != Some("sessions") {
        return None;
    }
    let parent = sessions_root.parent();
    let parent_is_dot_omp_agent = parent.is_some_and(|p| {
        p.file_name().and_then(|n| n.to_str()) == Some("agent")
            && p.parent()
                .and_then(|g| g.file_name())
                .and_then(|n| n.to_str())
                == Some(".omp")
    });
    // The PI_CODING_AGENT_DIR case: `omp_agent_dir()` honors the env var the
    // same way `default_paths` does, one resolution for both.
    let parent_is_resolved_agent_dir = parent.is_some_and(|p| p == agent_dir);
    if !parent_is_dot_omp_agent && !parent_is_resolved_agent_dir {
        return None;
    }
    // Not canonicalized here: the dir may not exist yet at wiring time (omp
    // never run). `live_omp_session_ids` canonicalizes per probe call, which
    // also picks up a root created after startup.
    Some(sessions_root.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ended_marker_is_anchored_on_the_structural_fields() {
        assert!(omp_session_ended(
            br#"{"type":"custom","id":"a","parentId":null,"timestamp":"t","customType":"session_exit","data":{"reason":"exit command","kind":"normal","recordedAt":"t"}}"#
        ));
        assert!(!omp_session_ended(
            br#"{"type":"custom","customType":"tool_execution_start","data":{"toolCallId":"t1"}}"#
        ));
        // Marker bytes inside tool CONTENT must not end the session.
        assert!(!omp_session_ended(
            br#"{"type":"message","message":{"role":"toolResult","toolCallId":"t1","content":[{"type":"text","text":"grep hit: \"customType\":\"session_exit\""}]}}"#
        ));
        assert!(!omp_session_ended(br#"{"type":"session","cwd":"/p"}"#));
    }

    #[test]
    fn session_ended_matches_marker_after_a_partial_first_tail_line() {
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

    const STEM: &str = "2026-07-10T18-32-27-539Z_019f4d4d-6c93-7000-af7b-59b47b0e8111";

    fn snap_of(root: &Path, paths: Vec<PathBuf>) -> ProbeSnapshot {
        ProbeSnapshot::from_open_fd_pairs(
            root,
            paths.into_iter().map(|p| (42, p)),
            omp_recognize,
            omp_id_from_path,
        )
    }

    #[test]
    fn transcript_under_root_yields_its_chain_id_bound_to_its_pid() {
        let root = Path::new("/home/u/.omp/agent/sessions");
        let session = root.join(format!("-dev-proj/{STEM}.jsonl"));
        let child = root.join(format!("-dev-proj/{STEM}/Alpha.jsonl"));
        let got = snap_of(root, vec![session, child]);
        let mut ids: Vec<_> = got.ids().cloned().collect();
        ids.sort();
        // Expectations go through the SAME fold the probe applies — a raw-case
        // literal here fails ONLY on windows-test.
        let stem_key = crate::id::normalize_path_key(STEM);
        assert_eq!(
            ids,
            vec![
                stem_key.clone(),
                crate::id::normalize_path_key(&format!("{STEM}/Alpha"))
            ]
        );
        assert_eq!(got.pid_of.get(&stem_key), Some(&42));
    }

    #[test]
    fn shared_transcript_binds_the_larger_pid_regardless_of_enumeration_order() {
        let root = Path::new("/home/u/.omp/agent/sessions");
        let path = root.join(format!("-dev-proj/{STEM}.jsonl"));
        let stem_key = crate::id::normalize_path_key(STEM);
        for pids in [[100, 200], [200, 100]] {
            let got = ProbeSnapshot::from_open_fd_pairs(
                root,
                pids.into_iter().map(|p| (p, path.clone())),
                omp_recognize,
                omp_id_from_path,
            );
            assert_eq!(
                got.ids().cloned().collect::<Vec<_>>(),
                vec![stem_key.clone()]
            );
            assert_eq!(
                got.pid_of.get(&stem_key),
                Some(&200),
                "the larger pid must win in both enumeration orders"
            );
        }
    }

    #[test]
    fn paths_outside_root_and_non_jsonl_files_are_excluded() {
        let root = Path::new("/home/u/.omp/agent/sessions");
        let outside = PathBuf::from(format!("/tmp/elsewhere/{STEM}.jsonl"));
        let wrong_ext = root.join(format!("-dev-proj/{STEM}/notes.txt"));
        let no_ext = root.join("-dev-proj/README");
        let got = snap_of(root, vec![outside, wrong_ext, no_ext]);
        assert!(got.is_empty());
        assert!(got.pid_of.is_empty());
    }

    #[test]
    fn probe_root_requires_first_party_layout() {
        let agent_dir = Path::new("/home/u/.omp/agent");
        assert_eq!(
            omp_probe_root_resolved(Path::new("/home/u/.omp/agent/sessions"), agent_dir),
            Some(PathBuf::from("/home/u/.omp/agent/sessions"))
        );
        assert_eq!(
            omp_probe_root_resolved(Path::new("/tmp/fixture"), agent_dir),
            None
        );
        assert_eq!(
            omp_probe_root_resolved(Path::new("/srv/other/sessions"), agent_dir),
            None
        );
        assert_eq!(
            omp_probe_root_resolved(Path::new("sessions"), agent_dir),
            None
        );
    }

    #[test]
    fn probe_root_accepts_resolved_agent_dir_sessions_layout() {
        let agent_dir = tempfile::tempdir().unwrap();
        let sessions = agent_dir.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        assert_eq!(
            omp_probe_root_resolved(&sessions, agent_dir.path()),
            Some(sessions.clone())
        );
    }

    #[test]
    fn live_ids_for_missing_root_is_some_empty_not_a_failure() {
        // canonicalize() fails on a nonexistent dir, but an ABSENT root is not
        // a probe failure — `None` would freeze the negative-vouch ledger
        // forever on machines that never ran omp.
        let missing = Path::new("/definitely/not/a/real/.omp/agent/sessions");
        let snap = live_omp_session_ids(missing).expect("absent root is not a probe failure");
        assert!(snap.is_empty());
        assert!(snap.pid_of.is_empty());
    }

    #[test]
    fn live_ids_for_unrelated_root_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let snap =
            live_omp_session_ids(dir.path()).expect("a healthy system's enumeration must succeed");
        assert!(snap.is_empty());
    }
}
