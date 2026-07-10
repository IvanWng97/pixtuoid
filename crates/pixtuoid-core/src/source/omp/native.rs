//! The `native`-only runtime half of the omp source: `OmpSource`, its
//! `JsonlWatcher` wiring, the first-sight session-ended checker, and the
//! open-write-fd liveness probe. The pure decoder stays in the
//! always-compiled parent module; this whole file sits behind the parent's
//! ONE `#[cfg(feature = "native")] mod native;` gate and is re-exported
//! there, so public paths don't move.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use super::{decode_omp_line, derive_omp_label, omp_agent_dir, omp_id_from_path, SOURCE_NAME};
use crate::source::fd_probe;
use crate::source::jsonl::{JsonlWatcher, ProbeSnapshot};
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
        let mut watcher = JsonlWatcher::new(
            self.sessions_root.clone(),
            SOURCE_NAME.to_string(),
            decode_omp_line,
            derive_omp_label,
            omp_session_ended,
        )
        .with_id_deriver(omp_id_from_path);
        if let Some(root) = omp_probe_root(&self.sessions_root) {
            watcher = watcher
                .with_liveness_probe(std::sync::Arc::new(move || live_omp_session_ids(&root)));
        }
        watcher.run(tx).await
    }
}

/// omp's liveness probe (the Codex fd_probe class): the session ids
/// (`omp_id_from_path` id-space, so they join the watcher's first-sight gate
/// directly) of every transcript under `sessions_root` held OPEN by a running
/// omp process, plus the owning pid per id.
///
/// omp keeps a for-lifetime append fd on its session file (upstream
/// `session-storage.ts` `fs.openSync(fpath, "a")`, closed only on session
/// close), so an open transcript fd IS the first-party liveness signal. omp
/// runs under Bun (its bin stub is `#!/usr/bin/env bun`), so the
/// kernel-truncated process name is `bun`, not `omp` — probe both (a future
/// compiled binary would report `omp`; verified live against omp 16.4.0:
/// comm `bun`, one `O_APPEND` fd on the root transcript). The under-root +
/// `.jsonl` join carries the precision — an unrelated bun process holding an
/// omp session transcript open is the accepted RESIDUAL, not an impossibility:
/// the fd MODE is not checked, so a long-lived bun tool reading an OLD
/// transcript (a log analyzer, a session picker) would vouch it for the scan
/// pass — an intermittent resurrection at worst, reaped again once the fd
/// closes (the negative-vouch ladder). Failure is explicit (#223): `None` ONLY
/// when the proc-table enumeration itself fails (the watcher then changes
/// nothing). An ABSENT sessions root is NOT a failure — omp may simply never
/// have run — so it returns `Some(empty)`: a healthy "nothing alive"
/// observation. Per-pid fd failures stay non-failures.
///
/// Module-private on purpose: the sole consumer is `OmpSource::run` — omp has
/// no focus point-query (registry: `FocusChannel::Unsupported`), so unlike
/// `live_cc_session_ids`/`live_codex_rollout_ids` there is no second consumer
/// to justify a public path (CONTRIBUTING pitfall 5: pub evades dead-code
/// lints).
fn live_omp_session_ids(sessions_root: &Path) -> Option<ProbeSnapshot> {
    // Canonicalize once per probe call: kernel-reported fd paths are fully
    // resolved (e.g. /tmp → /private/tmp on macOS), so the prefix compare
    // must run against the canonical root or every transcript misses.
    let Ok(root) = sessions_root.canonicalize() else {
        tracing::debug!(
            "omp probe: sessions root {} not canonicalizable; nothing alive there",
            sessions_root.display()
        );
        return Some(ProbeSnapshot::default());
    };
    let mut pids = fd_probe::pids_by_name("bun")?;
    pids.extend(fd_probe::pids_by_name("omp")?);
    let pairs = pids.into_iter().flat_map(|pid| {
        fd_probe::open_vnode_paths(pid)
            .into_iter()
            .map(move |path| (pid, path))
    });
    Some(session_ids_from_paths(&root, pairs))
}

/// The pure join half of the probe (unit-testable without FFI): keep the
/// (pid, path) pairs whose path is a `.jsonl` under `root`, mapped through
/// `omp_id_from_path` — the watcher's `IdDeriver`, so probe ids and gate ids
/// can't drift; root transcripts AND nested task children both key on the
/// stem chain, so a live child file vouches for the child id specifically.
/// Each surviving pair also binds id → pid for the snapshot's `pid_of` (the
/// exit-watch half).
fn session_ids_from_paths(
    root: &Path,
    pairs: impl Iterator<Item = (i32, PathBuf)>,
) -> ProbeSnapshot {
    let mut snap = ProbeSnapshot::default();
    for (pid, path) in pairs {
        if !path.starts_with(root) || path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        tracing::debug!("omp probe: pid {pid} holds {} open", path.display());
        // The watcher's id-space is normalize_path_key-folded at the seam
        // (walk.rs `id_path`); fold the kernel-reported path the same way or
        // probe ids miss the first-sight gate on Windows (the CC probe folds
        // identically in `live_cc_session_ids`).
        let id = omp_id_from_path(Path::new(&crate::id::normalize_path_key(
            &path.to_string_lossy(),
        )));
        // Two live processes holding ONE transcript open (a resume overlap)
        // must not bind id→pid by proc-enumeration order — the same
        // determinism rule as the codex probe (#252): larger pid wins,
        // arbitrary but stable for live processes.
        let bound = snap.pid_of.entry(id).or_insert(pid);
        if pid > *bound {
            *bound = pid;
        }
    }
    snap
}

/// Attach the probe ONLY for omp's first-party layout: the standard
/// `~/.omp/agent/sessions` shape (the root's file_name is literally
/// `sessions` AND its parent is `.omp/agent`) or the resolved
/// `omp_agent_dir()/sessions` for THIS environment (a `PI_CODING_AGENT_DIR`
/// user's real sessions root — omp itself writes there, and rejecting it
/// would silently drop the whole liveness ladder for a supported config).
/// Mirrors `codex_probe_root`'s gating: a test/replay root pointed at an
/// arbitrary dir must keep the pure-mtime first-sight gate (the probe is
/// additive-only; a replayed transcript vouched for by a coincidentally
/// running bun process would resurrect as live).
fn omp_probe_root(sessions_root: &Path) -> Option<PathBuf> {
    omp_probe_root_resolved(sessions_root, &omp_agent_dir())
}

/// The injectable core of [`omp_probe_root`] (mirrors
/// `codex_probe_root_resolved`'s testable split): `agent_dir` is the
/// resolved omp agent dir for this environment.
fn omp_probe_root_resolved(sessions_root: &Path, agent_dir: &Path) -> Option<PathBuf> {
    if sessions_root.file_name().and_then(|n| n.to_str()) != Some("sessions") {
        return None;
    }
    let parent = sessions_root.parent();
    // The standard `~/.omp/agent/sessions` shape…
    let parent_is_dot_omp_agent = parent.is_some_and(|p| {
        p.file_name().and_then(|n| n.to_str()) == Some("agent")
            && p.parent()
                .and_then(|g| g.file_name())
                .and_then(|n| n.to_str())
                == Some(".omp")
    });
    // …or a parent that IS the resolved agent dir — the PI_CODING_AGENT_DIR
    // case (`omp_agent_dir()` honors the env var the same way
    // `default_paths` does, one resolution for both).
    let parent_is_resolved_agent_dir = parent.is_some_and(|p| p == agent_dir);
    if !parent_is_dot_omp_agent && !parent_is_resolved_agent_dir {
        return None;
    }
    // Not canonicalized here: the dir may not exist yet at wiring time (omp
    // never run); `live_omp_session_ids` canonicalizes per probe call, which
    // also picks up a root created after startup.
    Some(sessions_root.to_path_buf())
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

    // ---- liveness probe (open-write-fd binding, the Codex fd_probe class) ----

    const STEM: &str = "2026-07-10T18-32-27-539Z_019f4d4d-6c93-7000-af7b-59b47b0e8111";

    fn snap_of(root: &Path, paths: Vec<PathBuf>) -> ProbeSnapshot {
        session_ids_from_paths(root, paths.into_iter().map(|p| (42, p)))
    }

    #[test]
    fn transcript_under_root_yields_its_chain_id_bound_to_its_pid() {
        let root = Path::new("/home/u/.omp/agent/sessions");
        // Real layout nests the per-cwd encoded dir below the root; a task
        // child nests one level deeper — BOTH must key through
        // `omp_id_from_path` so probe ids join the first-sight gate directly.
        let session = root.join(format!("-dev-proj/{STEM}.jsonl"));
        let child = root.join(format!("-dev-proj/{STEM}/Alpha.jsonl"));
        let got = snap_of(root, vec![session, child]);
        let mut ids: Vec<_> = got.ids().cloned().collect();
        ids.sort();
        // Expected ids go through the SAME fold the probe applies (identity
        // on Unix, lowercased on Windows) — a raw-case literal here fails
        // ONLY on windows-test (the path-fold expectation-literal class).
        let stem_key = crate::id::normalize_path_key(STEM);
        assert_eq!(
            ids,
            vec![
                stem_key.clone(),
                crate::id::normalize_path_key(&format!("{STEM}/Alpha"))
            ]
        );
        // The snapshot binds each id to the OWNING pid (the exit-watch half).
        assert_eq!(got.pid_of.get(&stem_key), Some(&42));
    }

    #[test]
    fn shared_transcript_binds_the_larger_pid_regardless_of_enumeration_order() {
        // Two live processes holding ONE transcript (a resume overlap): the
        // binding must be the deterministic tiebreak winner in BOTH
        // presentation orders, never last-writer-wins.
        let root = Path::new("/home/u/.omp/agent/sessions");
        let path = root.join(format!("-dev-proj/{STEM}.jsonl"));
        let stem_key = crate::id::normalize_path_key(STEM);
        for pids in [[100, 200], [200, 100]] {
            let got = session_ids_from_paths(root, pids.into_iter().map(|p| (p, path.clone())));
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
        // A bun process's OTHER open files must never vouch: outside-root
        // jsonl, and non-jsonl files under the root (the sibling artifacts
        // dir holds tool outputs etc.).
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
        // The standard shape attaches.
        assert_eq!(
            omp_probe_root_resolved(Path::new("/home/u/.omp/agent/sessions"), agent_dir),
            Some(PathBuf::from("/home/u/.omp/agent/sessions"))
        );
        // A test/replay root must get NO probe (pure-mtime behavior).
        assert_eq!(
            omp_probe_root_resolved(Path::new("/tmp/fixture"), agent_dir),
            None
        );
        // `sessions` under a parent that is neither `.omp/agent` nor the
        // resolved agent dir is not first-party.
        assert_eq!(
            omp_probe_root_resolved(Path::new("/srv/other/sessions"), agent_dir),
            None
        );
        // A bare relative `sessions` has no parent to check.
        assert_eq!(
            omp_probe_root_resolved(Path::new("sessions"), agent_dir),
            None
        );
    }

    #[test]
    fn probe_root_accepts_resolved_agent_dir_sessions_layout() {
        // A PI_CODING_AGENT_DIR-shaped layout: the resolved agent dir is NOT
        // `.omp/agent`, but its `sessions` child is omp's first-party root
        // for this environment — the probe must attach, or relocated users
        // silently lose the entire liveness ladder.
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
        // canonicalize() fails on a nonexistent dir, but an ABSENT root is
        // not a probe failure — omp may simply never have run (#223: None
        // would freeze the negative-vouch ledger forever on machines
        // without omp).
        let missing = Path::new("/definitely/not/a/real/.omp/agent/sessions");
        let snap = live_omp_session_ids(missing).expect("absent root is not a probe failure");
        assert!(snap.is_empty());
        assert!(snap.pid_of.is_empty());
    }

    #[test]
    fn live_ids_for_unrelated_root_is_empty() {
        // Real FFI smoke: whatever bun/omp processes exist, none hold a
        // transcript open under a fresh tempdir.
        let dir = tempfile::tempdir().unwrap();
        let snap =
            live_omp_session_ids(dir.path()).expect("a healthy system's enumeration must succeed");
        assert!(snap.is_empty());
    }
}
