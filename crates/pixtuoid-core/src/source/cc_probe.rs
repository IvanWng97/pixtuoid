//! The CC sessions/<pid>.json registry probe — the `~/.claude/sessions`
//! machinery behind `claude_code::live_cc_session_ids` (re-exported there;
//! this module is the implementation home).

use std::path::{Path, PathBuf};

use crate::source::jsonl::ProbeSnapshot;

/// CC's first-party live-process registry: `<claude_home>/sessions/<pid>.json`,
/// one tiny JSON file per running CC process (`{pid, sessionId, cwd, status,
/// startedAt, procStart, ...}` — undocumented; check_upstream_drift.py only
/// watches for the registry APPEARING in the docs, so shape drift (#247) is
/// detected HERE: an entry that parses as JSON but lacks a consumed key warns
/// once per process run and yields no vouch — graceful mtime-only degradation
/// with a breadcrumb instead of silence. Returns the session UUIDs (+ owning
/// pid each) of entries whose pid is still ALIVE — a registry file can outlive
/// a crashed CC, so each entry is verified with kill(pid, 0).
///
/// Failure is explicit (#223): an UNREADABLE-or-MISSING registry dir is
/// `None` — on a machine where CC runs the dir's absence is ambiguous (older
/// CC without the registry, a permissions problem), so the probe declares
/// failure and the watcher changes nothing. An EMPTY but readable dir is
/// `Some(empty)`: a healthy "no CC running" observation that the negative
/// vouch may act on.
///
/// PID-reuse guard (#220): kill(0) only proves SOME process owns the pid. When
/// the entry carries `startedAt` (ms epoch, stamped by CC at startup) AND the
/// kernel can report the process start time ([`pid_start_time_secs`] — macOS
/// only today), the two must agree within [`PID_START_TOLERANCE_SECS`] or the
/// pid was recycled by an unrelated process and the entry is skipped. Either
/// side missing falls back to pid-alive-only (the previous behavior — the
/// check is additive). This matters more now that the probe is ONGOING
/// liveness (a recycled pid would hold a dead session's sweep exemption open,
/// not just admit one transient sprite).
pub fn live_cc_session_ids(sessions_dir: &Path) -> Option<ProbeSnapshot> {
    #[cfg(unix)]
    {
        // session_id → winning entry. The fold (not direct inserts) exists for
        // the pathological duplicate-id case (#252): two live entries claiming
        // one sessionId would otherwise bind id→pid by unspecified read_dir
        // order, flapping across refreshes — each flap churns the exit-watch
        // rebind, and the losing pid dying first emits a spurious SessionEnd
        // for the live session.
        let mut winners: std::collections::HashMap<String, RegistryEntry> =
            std::collections::HashMap::new();
        let Ok(entries) = std::fs::read_dir(sessions_dir) else {
            tracing::debug!(
                "CC session registry {} unreadable or missing; probe pass failed",
                sessions_dir.display()
            );
            return None;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            // Regular files only, decided WITHOUT following symlinks (so a
            // symlink-to-FIFO is rejected too): reading a writer-less FIFO
            // named `x.json` blocks forever, hanging the probe and silently
            // killing the whole CC watcher task it runs inside.
            if !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            // An unreadable file is usually a CC exiting and removing its own
            // entry mid-scan — not format drift; skip silently.
            let Ok(bytes) = read_registry_entry_bounded(&path) else {
                continue;
            };
            let reg = match parse_registry_entry(&bytes) {
                RegistryParse::Entry(reg) => reg,
                // Not JSON / corrupt value — likely a half-written file
                // mid-write or junk, transient: skip silently, not drift.
                RegistryParse::Skip => continue,
                RegistryParse::ShapeDrift(key) => {
                    // The registry is the one undocumented upstream surface
                    // with no fetchable text to drift-diff (#247): a silent
                    // key rename would degrade the probe to mtime-only gating
                    // with zero signal. Warn ONCE per process run, never per
                    // 250ms scan pass (FailureLatch spirit; no recovery
                    // logging — drift doesn't un-happen mid-run).
                    static SHAPE_DRIFT_WARNED: std::sync::Once = std::sync::Once::new();
                    SHAPE_DRIFT_WARNED.call_once(|| {
                        crate::source::drift::shape_drift(
                            crate::source::claude_code::SOURCE_NAME,
                            &format!(
                                "sessions-registry entry {} parses as JSON but `{key}` is \
                                 missing or mistyped — the registry shape changed upstream; \
                                 mid-attach liveness degraded to mtime gating",
                                path.display()
                            ),
                        );
                    });
                    continue;
                }
            };
            if !pid_alive(reg.pid) {
                continue;
            }
            if let (Some(claimed_ms), Some(actual_secs)) =
                (reg.started_at_ms, pid_start_time_secs(reg.pid))
            {
                if (claimed_ms / 1000).abs_diff(actual_secs) > PID_START_TOLERANCE_SECS {
                    tracing::debug!(
                        pid = reg.pid,
                        claimed_secs = claimed_ms / 1000,
                        actual_secs,
                        "pid recycled — registry startedAt does not match process start; skipping"
                    );
                    continue;
                }
            }
            match winners.entry(reg.session_id.clone()) {
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(reg);
                }
                std::collections::hash_map::Entry::Occupied(mut slot) => {
                    // Real registries are one-file-per-pid with unique ids —
                    // a duplicate is upstream junk worth ONE breadcrumb per
                    // process run (SHAPE_DRIFT_WARNED spirit), not a per-scan
                    // log storm.
                    static DUPLICATE_ID_WARNED: std::sync::Once = std::sync::Once::new();
                    DUPLICATE_ID_WARNED.call_once(|| {
                        tracing::warn!(
                            session_id = %reg.session_id,
                            pids = ?(slot.get().pid, reg.pid),
                            "two live CC registry entries claim the same sessionId — \
                             keeping a deterministic winner"
                        );
                    });
                    if prefer_candidate(slot.get(), &reg) {
                        slot.insert(reg);
                    }
                }
            }
        }
        let mut live = ProbeSnapshot::default();
        for (session_id, entry) in winners {
            live.pid_of.insert(session_id.clone(), entry.pid);
            live.ids.insert(session_id);
        }
        Some(live)
    }
    #[cfg(not(unix))]
    {
        // Windows has no kill(0); pid liveness there needs OpenProcess +
        // exit-code semantics we haven't validated against CC-on-Windows.
        // The probe CANNOT enumerate → None (explicit failure): the watcher
        // changes nothing — admission keeps today's pure-mtime behavior and
        // the negative vouch never arms.
        let _ = sessions_dir;
        None
    }
}

/// Deterministic winner between two LIVE entries claiming one sessionId
/// (#252). Scan-order independence is the whole point — read_dir order is
/// unspecified and may differ between refreshes, and the binding must not
/// flap. Newest `startedAt` wins (the genuinely newer CC owns a reused id);
/// a `startedAt`-carrying entry beats one without (it also passed the pid
/// identity check, so it's the better-attested binding); both absent or equal
/// → larger pid — arbitrary, but stable for live processes, which is all
/// stability needs.
#[cfg(unix)]
fn prefer_candidate(incumbent: &RegistryEntry, candidate: &RegistryEntry) -> bool {
    match (candidate.started_at_ms, incumbent.started_at_ms) {
        (Some(c), Some(i)) if c != i => c > i,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        _ => candidate.pid > incumbent.pid,
    }
}

/// Read one registry entry, bounded to 64 KiB. Real entries are <1 KiB; the
/// bound keeps junk dropped into the registry dir from ballooning a read that
/// runs on every scan pass (truncated bytes just fail the JSON parse and are
/// skipped silently).
#[cfg(unix)]
fn read_registry_entry_bounded(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    const MAX_REGISTRY_ENTRY_BYTES: u64 = 64 * 1024;
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_REGISTRY_ENTRY_BYTES)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// One parsed registry entry — the fields the liveness join needs.
#[cfg(unix)]
#[derive(Debug)]
struct RegistryEntry {
    pid: i32,
    session_id: String,
    /// `startedAt` — ms-epoch CC stamps at startup (~1.2s after process start,
    /// measured live). Optional: an older CC without the field still probes
    /// pid-alive-only.
    started_at_ms: Option<u64>,
}

/// One registry-file parse outcome — the seam the #247 drift warn keys on.
/// `ShapeDrift` carries WHICH consumed key vanished/changed type, so the
/// warn-once can name it. Routing rule for future keys: a REQUIRED consumed
/// key goes to `ShapeDrift("<key>")`; an additive key stays `Option` on the
/// entry and its absence is never a parse fail nor drift (the `startedAt`
/// precedent).
#[cfg(unix)]
#[derive(Debug)]
enum RegistryParse {
    Entry(RegistryEntry),
    /// Skip silently: not JSON at all (a half-written file mid-write —
    /// transient) or a value-level corruption (pid <= 0, empty sessionId) —
    /// the keys are shaped right, so it's not format drift. A PERSISTENT
    /// wholesale format replacement (registry no longer JSON at all) also
    /// lands here, deliberately silent: indistinguishable from torn reads
    /// per-file, and warning on it would let one startup transient consume
    /// the once-per-run breadcrumb that a real key rename deserves.
    Skip,
    /// Parses as JSON but a consumed key is missing or mistyped — the
    /// undocumented upstream shape changed (#247); the consumer warns once.
    ShapeDrift(&'static str),
}

/// Extract `{pid, sessionId, startedAt}` from one registry file.
/// `serde_json::Value` on purpose — the format is undocumented, so we read
/// only the fields the join needs and tolerate everything else changing.
/// `startedAt` is optional (missing/malformed → `None`, never a parse fail
/// nor drift): it only powers the additive PID-reuse identity check.
#[cfg(unix)]
fn parse_registry_entry(bytes: &[u8]) -> RegistryParse {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return RegistryParse::Skip;
    };
    let Some(pid) = v.get("pid").and_then(|p| p.as_i64()) else {
        return RegistryParse::ShapeDrift("pid");
    };
    // pid <= 0 is never a single process (kill(0)/kill(-n) target process
    // GROUPS — a corrupt entry must not probe our own group as "alive").
    let Some(pid) = i32::try_from(pid).ok().filter(|p| *p > 0) else {
        return RegistryParse::Skip;
    };
    let Some(session_id) = v.get("sessionId").and_then(|s| s.as_str()) else {
        return RegistryParse::ShapeDrift("sessionId");
    };
    if session_id.is_empty() {
        return RegistryParse::Skip;
    }
    let started_at_ms = v.get("startedAt").and_then(|s| s.as_u64());
    RegistryParse::Entry(RegistryEntry {
        pid,
        session_id: session_id.to_string(),
        started_at_ms,
    })
}

/// Tolerance for the `startedAt` ↔ kernel-start-time identity check. Measured
/// live: CC writes `startedAt` ≈ 1.2s after `pbi_start_tvsec` (Node boot +
/// module load before the stamp), so 10s is ~8× margin while still being far
/// below any plausible pid-recycling interval.
#[cfg(unix)]
const PID_START_TOLERANCE_SECS: u64 = 10;

/// Kernel-reported process start time in epoch seconds — the identity half of
/// the PID-reuse guard. macOS: `proc_pidinfo(PROC_PIDTBSDINFO)` →
/// `pbi_start_tvsec` (same libproc family as `fd_probe.rs`). Linux:
/// `/proc/<pid>/stat` field 22 is clock ticks since BOOT — epoch conversion
/// needs boot time + ticks-per-sec, so the identity check is macOS-only for
/// now and Linux returns `None` (pid-alive-only, today's behavior). `None` on
/// failure (pid gone mid-probe, EPERM) — never an error: the check is additive.
#[cfg(target_os = "macos")]
fn pid_start_time_secs(pid: i32) -> Option<u64> {
    // SAFETY: all-zero bytes are a valid value for this repr(C) plain-old-data
    // struct (integers + byte arrays only).
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the buffer is exactly `size` bytes of a repr(C) struct matching
    // the macOS SDK's proc_bsdinfo layout (proc_info.h, ABI-stable since
    // 10.5), so the kernel fills only memory we own. PROC_PIDTBSDINFO returns
    // the full struct or <= 0 on failure.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut std::ffi::c_void,
            size,
        )
    };
    if n != size {
        return None;
    }
    Some(info.pbi_start_tvsec)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn pid_start_time_secs(_pid: i32) -> Option<u64> {
    None
}

/// kill(pid, 0) liveness: rc 0 = alive and signalable; EPERM = alive but owned
/// by another user; ESRCH (or anything else) = no such process.
#[cfg(unix)]
fn pid_alive(pid: i32) -> bool {
    // SAFETY: kill with signal 0 performs only the existence/permission check —
    // no signal is delivered, no memory is touched, no pointer args.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// The sessions registry is a SIBLING of the projects root
/// (`<claude_home>/sessions` vs `<claude_home>/projects`). Derive it only when
/// the parent layout matches (the root's file_name is literally `projects`) —
/// a custom `--projects-root /tmp/fixture` replay points at an arbitrary dir
/// whose parent could hold an unrelated `sessions/`, so those runs get no
/// probe and keep the pure-mtime gate.
pub(super) fn cc_sessions_dir(projects_root: &Path) -> Option<PathBuf> {
    if projects_root.file_name().and_then(|n| n.to_str()) != Some("projects") {
        return None;
    }
    projects_root
        .parent()
        // A bare relative `projects` has an EMPTY parent — not a claude home.
        .filter(|home| !home.as_os_str().is_empty())
        .map(|home| home.join("sessions"))
}

#[cfg(test)]
mod liveness_tests {
    use super::*;
    #[cfg(unix)]
    use std::collections::HashSet;

    // Only the cfg(unix) tests write registry entries (the Windows impl never
    // reads them) — keep the helper gated too or it's dead code there.
    #[cfg(unix)]
    fn write_entry(dir: &Path, name: &str, pid: i64, session_id: &str) {
        std::fs::write(
            dir.join(name),
            serde_json::json!({ "pid": pid, "sessionId": session_id, "status": "idle" })
                .to_string(),
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn keeps_entry_whose_pid_is_alive_and_binds_its_pid() {
        let dir = tempfile::tempdir().unwrap();
        // Our own pid is alive by construction.
        write_entry(
            dir.path(),
            "self.json",
            std::process::id() as i64,
            "alive-session",
        );
        let live = live_cc_session_ids(dir.path()).expect("readable dir is a healthy probe");
        assert!(
            live.ids.contains("alive-session"),
            "an entry with a live pid must be kept, got {live:?}"
        );
        // #223: the snapshot binds each vouched id to its owning OS pid (the
        // exit-watch half).
        assert_eq!(
            live.pid_of.get("alive-session"),
            Some(&(std::process::id() as i32)),
            "the vouched id must bind to the registry entry's pid"
        );
    }

    #[cfg(unix)]
    #[test]
    fn drops_entry_whose_pid_is_dead() {
        // Spawn-and-reap a real child: its pid is guaranteed dead once wait()
        // returns (modulo an astronomically unlikely instant reuse — the
        // accepted PID-reuse caveat, see live_cc_session_ids).
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead_pid = child.id() as i64;
        child.wait().unwrap();

        let dir = tempfile::tempdir().unwrap();
        write_entry(dir.path(), "dead.json", dead_pid, "dead-session");
        write_entry(
            dir.path(),
            "alive.json",
            std::process::id() as i64,
            "alive-session",
        );
        let live = live_cc_session_ids(dir.path()).expect("readable dir is a healthy probe");
        assert!(
            !live.ids.contains("dead-session"),
            "a crashed CC's leftover registry file must not count as live, got {live:?}"
        );
        assert!(
            live.ids.contains("alive-session"),
            "the live sibling must survive the dead entry, got {live:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn malformed_and_incomplete_entries_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let own_pid = std::process::id() as i64;
        std::fs::write(dir.path().join("garbage.json"), "not json {{{").unwrap();
        // Missing sessionId.
        std::fs::write(
            dir.path().join("nosid.json"),
            serde_json::json!({ "pid": own_pid }).to_string(),
        )
        .unwrap();
        // pid <= 0 would kill(0) our own process GROUP — must be rejected.
        write_entry(dir.path(), "pid0.json", 0, "group-session");
        // pid as a string (format drift) — not silently coerced.
        std::fs::write(
            dir.path().join("strpid.json"),
            serde_json::json!({ "pid": own_pid.to_string(), "sessionId": "str-pid" }).to_string(),
        )
        .unwrap();
        // Non-.json files are not registry entries.
        write_entry(dir.path(), "notes.txt", own_pid, "txt-session");
        write_entry(dir.path(), "valid.json", own_pid, "valid-session");

        let live = live_cc_session_ids(dir.path()).expect("readable dir is a healthy probe");
        assert_eq!(
            live.ids,
            HashSet::from(["valid-session".to_string()]),
            "only the well-formed live entry may survive"
        );
    }

    #[cfg(unix)]
    #[test]
    fn oversized_junk_json_entry_is_ignored() {
        // Real registry entries are <1 KiB; a 256 KiB blob is junk (or drift
        // we couldn't parse anyway). The bounded read keeps the per-scan
        // probe cost flat — the truncated bytes just fail the JSON parse and
        // land in the silent skip.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("huge.json"), vec![b'x'; 256 * 1024]).unwrap();
        write_entry(
            dir.path(),
            "valid.json",
            std::process::id() as i64,
            "valid-session",
        );
        let live = live_cc_session_ids(dir.path()).expect("readable dir is a healthy probe");
        assert_eq!(
            live.ids,
            HashSet::from(["valid-session".to_string()]),
            "an oversized junk entry must be ignored, not break the probe"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fifo_named_json_does_not_hang_the_probe() {
        // A FIFO named like a registry entry must be skipped BEFORE any read:
        // reading a writer-less FIFO blocks forever, which would hang the
        // probe and silently kill the whole CC watcher task. The file_type()
        // filter (which doesn't follow symlinks) rejects it without touching
        // its contents.
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo.json");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: mkfifo only reads the NUL-terminated path; 0o600 owner-only.
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "mkfifo failed: {}", std::io::Error::last_os_error());
        write_entry(
            dir.path(),
            "valid.json",
            std::process::id() as i64,
            "valid-session",
        );
        let live = live_cc_session_ids(dir.path()).expect("readable dir is a healthy probe");
        assert_eq!(
            live.ids,
            HashSet::from(["valid-session".to_string()]),
            "a FIFO entry must be skipped; the live sibling must survive"
        );
    }

    #[test]
    fn missing_dir_is_probe_failure_none() {
        // #223: a MISSING registry dir on a machine where CC runs is
        // ambiguous (older CC, permissions) — explicit failure, the watcher
        // changes nothing. (On Windows the probe can't enumerate at all, so
        // None holds there too.)
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert!(live_cc_session_ids(&missing).is_none());
    }

    #[test]
    fn unreadable_dir_is_probe_failure_none() {
        // A FILE where the sessions dir should be makes read_dir fail
        // deterministically (no chmod games) — same explicit-failure path.
        let dir = tempfile::tempdir().unwrap();
        let file_not_dir = dir.path().join("sessions");
        std::fs::write(&file_not_dir, b"not a dir").unwrap();
        assert!(live_cc_session_ids(&file_not_dir).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn empty_readable_dir_is_some_empty() {
        // An empty but READABLE registry is a healthy observation: no CC is
        // running (meaningful — the negative vouch may act on it), NOT a
        // probe failure.
        let dir = tempfile::tempdir().unwrap();
        let snap = live_cc_session_ids(dir.path()).expect("empty readable dir is healthy");
        assert!(snap.ids.is_empty());
        assert!(snap.pid_of.is_empty());
    }

    // --- PID-reuse identity check (#220) -----------------------------------

    #[cfg(unix)]
    fn expect_entry(bytes: &[u8]) -> RegistryEntry {
        match parse_registry_entry(bytes) {
            RegistryParse::Entry(e) => e,
            other => panic!("expected a parsed entry, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn parse_registry_entry_extracts_started_at_and_tolerates_absence() {
        let with = serde_json::json!({
            "pid": 64924, "sessionId": "s", "startedAt": 1_781_109_422_174_u64
        })
        .to_string();
        let entry = expect_entry(with.as_bytes());
        assert_eq!(entry.started_at_ms, Some(1_781_109_422_174));

        // Older CC without the field — still a valid entry (pid-alive-only).
        let without = serde_json::json!({ "pid": 64924, "sessionId": "s" }).to_string();
        let entry = expect_entry(without.as_bytes());
        assert_eq!(entry.started_at_ms, None);

        // Malformed startedAt (string / negative) degrades to None, never a
        // parse failure — the identity check is additive.
        let junk =
            serde_json::json!({ "pid": 64924, "sessionId": "s", "startedAt": "soon" }).to_string();
        let entry = expect_entry(junk.as_bytes());
        assert_eq!(entry.started_at_ms, None);
    }

    // --- registry shape pin (#247) ------------------------------------------

    /// Pin of the CURRENT live `<claude_home>/sessions/<pid>.json` shape
    /// (synthesized values, same keys as a real 2026-06 entry). The format is
    /// undocumented with no upstream text to diff, so this fixture is the
    /// shape's regression detector: if a consumed key is renamed upstream,
    /// the drift tests below are what a maintainer updates against reality.
    #[cfg(unix)]
    fn live_shape_entry(pid: i64, session_id: &str) -> String {
        serde_json::json!({
            "pid": pid,
            "sessionId": session_id,
            "startedAt": 1_781_109_422_174_u64,
            "procStart": 1_781_109_420_000_u64,
            "cwd": "/Users/someone/project",
            "version": "2.1.0",
            "peerProtocol": 1,
            "kind": "repl",
            "entrypoint": "cli",
            "status": "idle",
            "updatedAt": 1_781_109_500_000_u64,
            "name": "project",
            "bridgeSessionId": "00000000-0000-7000-8000-00000000bb1d"
        })
        .to_string()
    }

    #[cfg(unix)]
    #[test]
    fn registry_shape_pin_parses_the_current_live_format() {
        let entry = expect_entry(live_shape_entry(64924, "pinned-session").as_bytes());
        assert_eq!(entry.pid, 64924);
        assert_eq!(entry.session_id, "pinned-session");
        assert_eq!(entry.started_at_ms, Some(1_781_109_422_174));
    }

    #[cfg(unix)]
    #[test]
    fn renamed_or_mistyped_required_key_is_shape_drift() {
        // The warn predicate (#247), tested directly — the warn site itself is
        // gated by a process-global Once, so the classification is the seam.
        let renamed_sid = serde_json::json!({ "pid": 64924, "session_id": "s" }).to_string();
        assert!(matches!(
            parse_registry_entry(renamed_sid.as_bytes()),
            RegistryParse::ShapeDrift("sessionId")
        ));
        let nonstring_sid = serde_json::json!({ "pid": 64924, "sessionId": 7 }).to_string();
        assert!(matches!(
            parse_registry_entry(nonstring_sid.as_bytes()),
            RegistryParse::ShapeDrift("sessionId")
        ));
        let renamed_pid = serde_json::json!({ "processId": 64924, "sessionId": "s" }).to_string();
        assert!(matches!(
            parse_registry_entry(renamed_pid.as_bytes()),
            RegistryParse::ShapeDrift("pid")
        ));
        let string_pid = serde_json::json!({ "pid": "64924", "sessionId": "s" }).to_string();
        assert!(matches!(
            parse_registry_entry(string_pid.as_bytes()),
            RegistryParse::ShapeDrift("pid")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn non_json_and_corrupt_values_are_silent_skips_not_drift() {
        // Half-written mid-write file — transient, never a drift warn.
        assert!(matches!(
            parse_registry_entry(b"{\"pid\": 64924, \"sessionI"),
            RegistryParse::Skip
        ));
        // Value-level corruption (the keys exist with the right types): pid
        // <= 0 targets a process GROUP, an empty sessionId vouches nothing.
        let pid_zero = serde_json::json!({ "pid": 0, "sessionId": "s" }).to_string();
        assert!(matches!(
            parse_registry_entry(pid_zero.as_bytes()),
            RegistryParse::Skip
        ));
        let empty_sid = serde_json::json!({ "pid": 64924, "sessionId": "" }).to_string();
        assert!(matches!(
            parse_registry_entry(empty_sid.as_bytes()),
            RegistryParse::Skip
        ));
    }

    #[cfg(unix)]
    #[test]
    fn shape_drifted_entry_yields_no_vouch() {
        // End-to-end through the probe: a key-renamed entry for a LIVE pid
        // must not vouch (the degraded-to-mtime path #247 warns about).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("drifted.json"),
            serde_json::json!({
                "pid": std::process::id(), "session_id": "drifted-session"
            })
            .to_string(),
        )
        .unwrap();
        write_entry(
            dir.path(),
            "valid.json",
            std::process::id() as i64,
            "valid-session",
        );
        let live = live_cc_session_ids(dir.path()).expect("readable dir is a healthy probe");
        assert_eq!(
            live.ids,
            HashSet::from(["valid-session".to_string()]),
            "a shape-drifted entry must not vouch; the well-formed sibling survives"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pid_start_time_secs_reports_a_fresh_child_as_just_started() {
        // Hermetic: a child spawned NOW must have a kernel start time within a
        // few seconds of the current wall clock (proves both the FFI call and
        // the epoch-seconds unit — a ticks-since-boot misread would be off by
        // decades).
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let started = pid_start_time_secs(child.id() as i32);
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let _ = child.kill();
        let _ = child.wait();
        let started = started.expect("proc_pidinfo must report a live child's start time");
        assert!(
            started.abs_diff(now_secs) <= 5,
            "child start time {started} should be within 5s of now {now_secs}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pid_start_time_secs_none_for_dead_pid() {
        assert_eq!(pid_start_time_secs(999_999_999), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn recycled_pid_entry_is_dropped_and_matching_one_kept() {
        let own_pid = std::process::id() as i32;
        let own_start = pid_start_time_secs(own_pid).expect("own start time");
        let dir = tempfile::tempdir().unwrap();
        // Plausible recycling: the registry claims a start an hour before the
        // process actually started — a dead CC whose pid was reused.
        std::fs::write(
            dir.path().join("recycled.json"),
            serde_json::json!({
                "pid": own_pid,
                "sessionId": "recycled-session",
                "startedAt": (own_start - 3600) * 1000
            })
            .to_string(),
        )
        .unwrap();
        // Live measurement: CC stamps startedAt ~1.2s after process start —
        // inside the 10s tolerance.
        std::fs::write(
            dir.path().join("genuine.json"),
            serde_json::json!({
                "pid": own_pid,
                "sessionId": "genuine-session",
                "startedAt": own_start * 1000 + 1200
            })
            .to_string(),
        )
        .unwrap();
        let live = live_cc_session_ids(dir.path()).expect("readable dir is a healthy probe");
        assert_eq!(
            live.ids,
            HashSet::from(["genuine-session".to_string()]),
            "a recycled-pid entry must be dropped; the identity-matching one kept"
        );
    }

    #[cfg(unix)]
    #[test]
    fn entry_without_started_at_keeps_pid_alive_only_behavior() {
        // Additive fallback: no startedAt → no identity check → the live pid
        // alone vouches (exactly the pre-#220 behavior).
        let dir = tempfile::tempdir().unwrap();
        write_entry(
            dir.path(),
            "legacy.json",
            std::process::id() as i64,
            "legacy-session",
        );
        assert!(live_cc_session_ids(dir.path())
            .expect("readable dir is a healthy probe")
            .ids
            .contains("legacy-session"));
    }

    // --- duplicate sessionId across live entries (#252) ---------------------

    #[cfg(unix)]
    fn entry(pid: i32, started_at_ms: Option<u64>) -> RegistryEntry {
        RegistryEntry {
            pid,
            session_id: "dup".into(),
            started_at_ms,
        }
    }

    #[cfg(unix)]
    #[test]
    fn duplicate_winner_rule_is_symmetric_and_total() {
        // Newest startedAt wins, in BOTH presentation orders (the symmetry IS
        // the scan-order independence).
        assert!(prefer_candidate(&entry(1, Some(100)), &entry(2, Some(200))));
        assert!(!prefer_candidate(
            &entry(2, Some(200)),
            &entry(1, Some(100))
        ));
        // A startedAt-carrying entry beats one without, both orders.
        assert!(prefer_candidate(&entry(9, None), &entry(1, Some(100))));
        assert!(!prefer_candidate(&entry(1, Some(100)), &entry(9, None)));
        // Both absent (or equal): larger pid wins, both orders.
        assert!(prefer_candidate(&entry(1, None), &entry(2, None)));
        assert!(!prefer_candidate(&entry(2, None), &entry(1, None)));
        assert!(prefer_candidate(&entry(1, Some(100)), &entry(2, Some(100))));
        assert!(!prefer_candidate(
            &entry(2, Some(100)),
            &entry(1, Some(100))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn duplicate_session_id_binds_a_stable_pid_regardless_of_scan_order() {
        // Two LIVE pids claiming one sessionId (no startedAt → no identity
        // check on either platform → the pid tiebreak decides). The same pair
        // is presented under file names sorting in OPPOSITE orders; whatever
        // order read_dir yields, the binding must come out identical — the
        // flap in #252 was exactly this binding following dir order.
        let mut child_a = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let mut child_b = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let (pid_a, pid_b) = (child_a.id() as i64, child_b.id() as i64);
        let expected = pid_a.max(pid_b) as i32;

        // Probe inside the loop, assert only after the children are reaped —
        // a panicking assert here would leak two sleep-30s (the sibling
        // pid_start_time test's kill-before-assert discipline).
        let mut snapshots = Vec::new();
        for (first, second) in [(pid_a, pid_b), (pid_b, pid_a)] {
            let dir = tempfile::tempdir().unwrap();
            write_entry(dir.path(), "aaa.json", first, "dup");
            write_entry(dir.path(), "zzz.json", second, "dup");
            snapshots.push(live_cc_session_ids(dir.path()));
        }

        let _ = child_a.kill();
        let _ = child_a.wait();
        let _ = child_b.kill();
        let _ = child_b.wait();

        for snapshot in snapshots {
            let live = snapshot.expect("readable dir is a healthy probe");
            assert_eq!(
                live.ids,
                HashSet::from(["dup".to_string()]),
                "a duplicate id must yield ONE vouch, not two"
            );
            assert_eq!(
                live.pid_of.get("dup"),
                Some(&expected),
                "the winning pid must be the deterministic tiebreak winner in both name orders"
            );
        }
    }

    #[test]
    fn sessions_dir_derives_only_from_the_standard_layout() {
        // <claude_home>/projects → the sessions sibling.
        assert_eq!(
            cc_sessions_dir(Path::new("/home/u/.claude/projects")),
            Some(PathBuf::from("/home/u/.claude/sessions"))
        );
        // A fixture replay (--projects-root /tmp/fixture) has no registry
        // sibling — no probe, pure-mtime gate.
        assert_eq!(cc_sessions_dir(Path::new("/tmp/fixture")), None);
        assert_eq!(cc_sessions_dir(Path::new("projects")), None);
    }
}
