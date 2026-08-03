//! The `native`-only runtime half of the grok source: the liveness probe over
//! grok's own crash-recovery registry (`active_sessions.json`) + `GrokSource`
//! and its `JsonlWatcher` wiring. The pure decoders stay in the always-compiled
//! parent module; this whole file sits behind the parent's ONE
//! `#[cfg(feature = "native")] mod native;` gate and is re-exported there.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{decode_grok_line, grok_cwd_from_path, grok_home, grok_session_ended, SOURCE_NAME};
use crate::source::jsonl::{ChildEndUnclaims, JsonlWatcher, ProbeSnapshot};
use crate::source::{Source, TaggedSender};

/// grok's liveness probe: the session ids of every entry in
/// `{grok_home}/active_sessions.json` whose pid is alive (bound to the LEADER's
/// pid instead of the entry's when one is live — see below), in
/// `grok_id_from_path` id-space (the registry stores the bare session id ==
/// the transcript's parent-dir name, so probe ids join the first-sight gate
/// directly), plus the owning pid per id for the instant-exit watch.
///
/// The registry is grok's OWN crash-recovery design (active_sessions.rs):
/// registered per TUI session with `std::process::id()`, removed on clean
/// quit, left behind on crash — so pid-liveness over it is first-party, not
/// heuristic. grok keeps NO long-lived fd on its session files (every append
/// opens and drops the handle), so a Codex-style open-FD probe is impossible;
/// this registry is the substitute. Headless (`-p`) sessions are NOT
/// registered (only under the debug env `GROK_TRACK_HEADLESS`) — they are
/// never vouched, and since the negative vouch only ends PREVIOUSLY-vouched
/// ids, headless one-shots ride the mtime gate + short-idle reap instead.
///
/// Failure semantics (#223): an ABSENT registry file is `Some(empty)` — a
/// healthy "nothing alive" observation (grok not running / never run; also
/// the state after every session exits cleanly). An unreadable or unparseable
/// file is `None` — the enumeration itself failed, the watcher changes
/// nothing (grok rewrites the file atomically via temp+rename, so a torn read
/// is not expected; a parse failure means format drift → one `shape_drift`
/// breadcrumb per process run, the #247 non-fetchable-surface pattern).
/// Windows: `None` — no validated pid liveness (CC-probe precedent; the
/// ExitWatch backend is absent there anyway).
///
/// **Leader mode (`--leader`) reassigns ownership (#826).** There the agent
/// runs in a shared LEADER process while this registry still records each TUI
/// CLIENT's pid — so when a live leader is detected (an exclusive `flock` held
/// on `leader{suffix}.lock`, whose contents are its pid), every listed session
/// binds to the LEADER instead. Binding the client was the defect #638 set out
/// to fix and did not: that binding is what reaches the `ExitWatch`, so a
/// client disconnect ended a session the leader was still driving, in
/// milliseconds. Accepted residual: a client that quits CLEANLY unregisters
/// its entry, so a leader-kept session then leaves the snapshot and the
/// negative vouch ends it in ~60-120s — the leader's session map lives only in
/// its memory, so no disk artifact can close that. See the grok LEADER MODE
/// entry in this crate's `CLAUDE.md`.
#[cfg(unix)]
pub fn live_grok_session_ids(grok_root: &Path) -> Option<ProbeSnapshot> {
    let path = grok_root.join("active_sessions.json");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        // Absent registry = healthy "no TUI clients".
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(ProbeSnapshot::default()),
        Err(_) => return None,
    };
    let leader = live_leader_pid(&leader_lock_candidates(
        grok_root,
        std::env::var_os(LEADER_SOCKET_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
    ));
    match grok_ids_from_registry(
        &bytes,
        crate::source::cc_probe::pid_alive,
        crate::source::cc_probe::pid_start_time_secs,
        leader,
    ) {
        Some(snap) => Some(snap),
        None => {
            // The registry is an undocumented first-party surface with no
            // fetchable upstream text to drift-diff — the consumer is the
            // drift detector (#247). Warn ONCE per process run.
            static SHAPE_DRIFT_WARNED: std::sync::Once = std::sync::Once::new();
            SHAPE_DRIFT_WARNED.call_once(|| {
                crate::source::drift::shape_drift(
                    SOURCE_NAME,
                    &format!(
                        "active_sessions.json at {} does not parse as the expected \
                         [{{session_id,pid,cwd,opened_at}}] array — the registry shape \
                         changed upstream; liveness degraded to mtime gating",
                        path.display()
                    ),
                );
            });
            None
        }
    }
}

/// The CLIENT pid owning `session_id` — the TUI process whose WINDOW a focus
/// click must raise.
///
/// Deliberately NOT [`live_grok_session_ids`]: that answers "whose death ends
/// this session", which under a live leader is the LEADER, whose window is not
/// the one the user is typing in (and in a headless-into-leader setup has no
/// window at all). Two consumers, two questions — the same map cannot serve
/// both, and grok's `FocusChannel::TranscriptProbe` never stamps a pid, so this
/// probe is the ONLY answer the click has.
#[cfg(unix)]
pub(crate) fn grok_client_pid_for_session(grok_root: &Path, session_id: &str) -> Option<i32> {
    let bytes = std::fs::read(grok_root.join("active_sessions.json")).ok()?;
    grok_ids_from_registry(
        &bytes,
        crate::source::cc_probe::pid_alive,
        crate::source::cc_probe::pid_start_time_secs,
        None,
    )?
    .pid_of
    .get(session_id)
    .copied()
}

/// Non-Unix stub — no validated pid liveness, so a focus click has no answer.
#[cfg(not(unix))]
pub(crate) fn grok_client_pid_for_session(_grok_root: &Path, _session_id: &str) -> Option<i32> {
    None
}

/// Non-Unix stub — grok's `active_sessions.json` liveness probe is Unix-only
/// (the pid-liveness + kernel-start recycle check it needs), so on Windows it
/// always returns `None` and grok liveness degrades to pure mtime gating.
#[cfg(not(unix))]
pub fn live_grok_session_ids(_grok_root: &Path) -> Option<ProbeSnapshot> {
    None
}

/// The env var upstream honors to relocate the leader socket (and, by
/// extension, its sibling lock). Set by `--leader-socket` or exported directly;
/// when set it BYPASSES the WS-URL-derived name entirely, so a resolver that
/// only globs `{grok_home}` would miss a sandboxed leader.
#[cfg(unix)]
const LEADER_SOCKET_ENV: &str = "GROK_LEADER_SOCKET";

/// The lock files that could belong to a leader for this root, most specific
/// first. Upstream names them `leader{suffix}.lock`, where `suffix` is a hash
/// of the WS URL (empty for the production relay) — computed with
/// `DefaultHasher`, whose output std explicitly does NOT guarantee stable, so
/// we ENUMERATE rather than recompute it. `override_socket` is passed in (not
/// read from the environment here) to keep the resolution unit-testable.
#[cfg(unix)]
fn leader_lock_candidates(grok_root: &Path, override_socket: Option<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // An explicit relocation is the most specific answer, but it does NOT
    // replace the enumeration: the var is read from OUR environment, not the
    // leader's, so a shell exporting it for an unrelated sandbox would
    // otherwise blank leader detection entirely.
    if let Some(sock) = override_socket {
        out.push(sock.with_extension("lock"));
    }
    let Ok(entries) = std::fs::read_dir(grok_root) else {
        return out;
    };
    let mut globbed: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("leader") && n.ends_with(".lock"))
        })
        .collect();
    // `read_dir` order is arbitrary and unstable across directory mutation;
    // with two live leaders (a dev relay beside prod) it would decide which one
    // owns every session, and flip between passes.
    globbed.sort();
    out.extend(globbed);
    out
}

/// The pid of a LIVE grok leader, read from upstream's own first-party
/// artifact: `run_leader` acquires an exclusive `flock` on
/// `leader{suffix}.lock` and writes its pid into the file as decimal text,
/// holding the lock for its entire lifetime (`LeaderLock::try_acquire` +
/// `write_pid`, xai-grok-shell). So a failed SHARED lock IS the liveness
/// proof — the same advisory-lock arbitration the hook socket already uses —
/// and the contents are the identity. No process enumeration, no socket
/// connect, no per-fd syscalls.
///
/// The LOCK proves life, but it does not prove IDENTITY: upstream's own
/// `live_grok_lock_holder` gates the file's pid on `is_process_alive` because a
/// client that wins the flock while spawning a leader holds it across the
/// handoff without rewriting the file, leaving the previous — dead — leader's
/// pid in place. Hence the liveness filter on the parsed pid.
#[cfg(unix)]
fn live_leader_pid(candidates: &[PathBuf]) -> Option<i32> {
    use std::os::unix::fs::OpenOptionsExt;
    for path in candidates {
        // O_NOFOLLOW mirrors the hook socket's lock open: a symlink planted at
        // the lock path must fail rather than have us probe an arbitrary file.
        let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
        else {
            continue;
        };
        match file.try_lock_shared() {
            // Acquired ⇒ nobody holds it exclusively ⇒ no live leader; this is
            // residue from one that died. Dropping `file` releases ours.
            Ok(()) => continue,
            Err(std::fs::TryLockError::WouldBlock) => {
                // Read through the handle we already opened, not the path: a
                // second path-based open would follow a symlink and undo the
                // O_NOFOLLOW above.
                let mut text = String::new();
                let pid = std::io::Read::read_to_string(&mut &file, &mut text)
                    .ok()
                    .and_then(|_| text.trim().parse::<i64>().ok())
                    .and_then(crate::source::decoder::checked_pid)
                    // The holder and the CONTENTS are two facts, and upstream
                    // documents them diverging: a client that wins the flock
                    // while spawning a leader holds it across the handoff
                    // WITHOUT rewriting the file, so it still names the
                    // previous — dead — leader (their own `live_grok_lock_holder`
                    // gates on `is_process_alive` for exactly this). Binding a
                    // dead pid would register it with the ExitWatch, which
                    // synthesizes an immediate exit on ESRCH and would end every
                    // grok session at once.
                    .filter(|p| crate::source::cc_probe::pid_alive(*p));
                if pid.is_some() {
                    return pid;
                }
            }
            Err(std::fs::TryLockError::Error(_)) => continue,
        }
    }
    None
}

/// The pure join half of the probe (unit-testable with injected liveness
/// fns): parse the registry array, keep entries whose pid is alive (binding
/// them to `leader` when one is live) AND — when
/// BOTH sides are available — whose `opened_at` matches the kernel-reported
/// process start within [`cc_probe::PID_START_TOLERANCE_SECS`] (the #220
/// pid-recycle identity check; either side missing → pid-alive-only, the
/// check is additive). Returns `None` only when the DOCUMENT doesn't parse as
/// an array of entries (format drift); junk VALUES inside an entry (pid <= 0)
/// skip that entry silently, mirroring the CC registry's value-vs-shape
/// distinction.
///
/// `leader` (#826) reassigns OWNERSHIP: `ProbeLadder::fold` hands the bound pid
/// to the `ExitWatch`, so a live leader must be that pid. `alive` still gates
/// ADMISSION — it is this registry's only filter against crash residue — but
/// the pid-recycle check is skipped on the leader path, because it identifies a
/// CLIENT process the binding no longer depends on.
#[cfg(unix)]
fn grok_ids_from_registry(
    bytes: &[u8],
    alive: fn(i32) -> bool,
    start_time: impl Fn(i32) -> Option<u64>,
    leader: Option<i32>,
) -> Option<ProbeSnapshot> {
    #[derive(serde::Deserialize)]
    struct Entry {
        session_id: String,
        pid: i32,
        #[serde(default)]
        opened_at: Option<String>,
    }
    let entries: Vec<Entry> = serde_json::from_slice(bytes).ok()?;
    let mut snap = ProbeSnapshot::default();
    for e in entries {
        if let Some(leader_pid) = leader {
            // `alive` still gates ADMISSION — it is the only filter this
            // registry has against crash residue (entries are "removed on clean
            // quit, left behind on crash"), and a vouched id bypasses the
            // first-sight gate, emits ProofOfLife, and is exempt from both
            // sweeps, so a leaked entry would render an immortal ghost desk for
            // as long as any leader runs. Only the BINDING moves to the leader:
            // that is the pid whose death should end the session, so the
            // client's death now decays through the negative vouch (~60-120s)
            // instead of the ExitWatch's milliseconds.
            if e.pid > 0 && !e.session_id.is_empty() && alive(e.pid) {
                snap.bind_pid(e.session_id, leader_pid);
            }
            continue;
        }
        if e.pid <= 0 || e.session_id.is_empty() || !alive(e.pid) {
            continue;
        }
        if let (Some(claimed_secs), Some(actual_secs)) = (
            e.opened_at
                .as_deref()
                .and_then(crate::source::decoder::rfc3339_to_epoch_secs),
            start_time(e.pid),
        ) {
            // `opened_at` is stamped at session OPEN, which can lag process
            // start by however long the user sat on the welcome screen — the
            // tolerance only needs to catch RECYCLING (a pid reborn hours or
            // days later), so it accepts claimed >= actual generously and
            // rejects only a claim EARLIER than the process start beyond the
            // shared tolerance (a session can't have opened before its
            // process existed — that pid was recycled).
            if claimed_secs + crate::source::cc_probe::PID_START_TOLERANCE_SECS < actual_secs {
                tracing::debug!(
                    pid = e.pid,
                    claimed_secs,
                    actual_secs,
                    "pid recycled — active_sessions opened_at predates process start; skipping"
                );
                continue;
            }
        }
        // Duplicate session_id across entries is upstream junk — keep the
        // deterministic tiebreak winner (larger pid, the shared #252 rule).
        snap.bind_pid(e.session_id, e.pid);
    }
    Some(snap)
}

/// Attach the probe ONLY for grok's first-party layout: the standard
/// `~/.grok/sessions` shape (file_name `sessions` AND parent `.grok`) or the
/// resolved `grok_home()/sessions` for THIS environment (a `GROK_HOME` user's
/// real root). The registry file is a SIBLING of the sessions root
/// (`{grok_home}/active_sessions.json`), so the probe root is the sessions
/// root's PARENT. A `--grok-sessions-root /tmp/fixture` replay keeps the
/// pure-mtime first-sight gate (codex_probe_root's rationale).
fn grok_probe_root(sessions_root: &Path) -> Option<PathBuf> {
    grok_probe_root_resolved(sessions_root, &grok_home())
}

/// The injectable core of [`grok_probe_root`] (mirrors
/// `codex_probe_root_resolved`'s testable split).
fn grok_probe_root_resolved(sessions_root: &Path, home: &Path) -> Option<PathBuf> {
    if sessions_root.file_name().and_then(|n| n.to_str()) != Some("sessions") {
        return None;
    }
    let parent = sessions_root.parent()?;
    let parent_is_grok = parent.file_name().and_then(|n| n.to_str()) == Some(".grok");
    let parent_is_resolved_home = parent == home;
    if !parent_is_grok && !parent_is_resolved_home {
        return None;
    }
    Some(parent.to_path_buf())
}

/// Source that watches the grok session transcript tree.
pub struct GrokSource {
    /// The watched grok session-transcript root; per-session `updates.jsonl` lives under it.
    pub sessions_root: PathBuf,
    /// The #246 child-end un-claim side-channel — grok is consumer-only like
    /// Codex: its `subagent_stop`/`subagent_end` hooks decode to Hook-transport
    /// `SessionEnd{as_child:true}` (the tee's producer trigger), and THIS
    /// watcher releases the ended child's flat-sibling transcript claim so a
    /// `resume_from` / late-append revival re-registers cleanly. The runtime
    /// shares ONE handle across the router + the CC/Codex/grok watchers;
    /// `None` disables it (bare test construction).
    pub child_end_unclaims: Option<ChildEndUnclaims>,
}

impl GrokSource {
    /// Construct pointed at the default grok `sessions` root.
    pub fn default_paths() -> Self {
        Self {
            sessions_root: grok_home().join("sessions"),
            child_end_unclaims: None,
        }
    }
}

impl Source for GrokSource {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        let mut watcher = JsonlWatcher::new(
            self.sessions_root.clone(),
            SOURCE_NAME.to_string(),
            decode_grok_line,
            grok_session_ended,
        )
        .with_cwd_deriver(grok_cwd_from_path);
        if let Some(root) = grok_probe_root(&self.sessions_root) {
            watcher = watcher
                .with_liveness_probe(std::sync::Arc::new(move || live_grok_session_ids(&root)));
        }
        if let Some(unclaims) = &self.child_end_unclaims {
            watcher = watcher.with_child_end_unclaims(unclaims.clone());
        }
        watcher.run(tx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_root_accepts_first_party_layouts_only() {
        let home = Path::new("/custom/grok-home");
        // Standard dot-dir layout.
        assert_eq!(
            grok_probe_root_resolved(Path::new("/Users/u/.grok/sessions"), home),
            Some(PathBuf::from("/Users/u/.grok"))
        );
        // Resolved GROK_HOME layout (parent == home even though not `.grok`).
        assert_eq!(
            grok_probe_root_resolved(&home.join("sessions"), home),
            Some(home.to_path_buf())
        );
        // Replay/fixture roots keep pure-mtime gating.
        assert_eq!(
            grok_probe_root_resolved(Path::new("/tmp/fixture"), home),
            None
        );
        assert_eq!(
            grok_probe_root_resolved(Path::new("/tmp/other/sessions"), home),
            None
        );
    }

    #[cfg(unix)]
    mod registry_join {
        use super::*;

        fn alive_all(_pid: i32) -> bool {
            true
        }
        fn alive_none(_pid: i32) -> bool {
            false
        }

        const REG: &str = r#"[
            {"session_id":"0197-a","pid":100,"cwd":"/r/a","opened_at":"2026-07-16T12:00:05Z"},
            {"session_id":"0197-b","pid":200,"cwd":"/r/b","opened_at":"2026-07-16T12:01:00+00:00"}
        ]"#;

        // --- the OS BINDER (`live_grok_session_ids`) ---
        // The pure join above had six tests; the binder that decides
        // `Some(empty)` vs `None` had ZERO, while BOTH sibling probes cover
        // theirs (`live_omp_session_ids` has the same pair; `live_cc_session_ids`
        // has ~14). The arms are load-bearing in opposite directions:
        // `ProbeLadder::fold` is never called on a probe FAILURE ("failure
        // changes nothing"), whereas a healthy `Some(empty)` puts every
        // previously-vouched id into the miss window and confirms a
        // `SessionEnd` two healthy misses later. Reading one as the other
        // either freezes the ladder forever or ends live sessions.

        /// No `active_sessions.json` = no TUI clients, a real observation — NOT
        /// an enumeration failure. `None` here would freeze the negative-vouch
        /// ledger on every machine that never ran grok.
        #[test]
        fn binder_absent_registry_is_a_healthy_empty_not_a_failure() {
            let dir = tempfile::tempdir().unwrap();
            let snap =
                live_grok_session_ids(dir.path()).expect("an absent registry is not a failure");
            assert!(snap.pid_of.is_empty());
        }

        /// A registry that exists but cannot be READ is the enumeration failing
        /// — the watcher must change nothing.
        #[test]
        fn binder_unreadable_registry_is_a_failure_not_an_empty() {
            let dir = tempfile::tempdir().unwrap();
            // A directory in its place makes `std::fs::read` fail with EISDIR on
            // both platforms (chmod 000 would not fail for root, which CI sometimes is).
            std::fs::create_dir(dir.path().join("active_sessions.json")).unwrap();
            assert!(
                live_grok_session_ids(dir.path()).is_none(),
                "an unreadable registry must be None, never a healthy empty"
            );
        }

        /// End-to-end through the real file read + the real liveness check: our
        /// own pid is unquestionably alive, so it must bind. Falsifiable — a
        /// binder stubbed to `Some(default)` returns an empty snapshot.
        #[test]
        fn binder_reads_a_real_registry_and_binds_our_own_live_pid() {
            let dir = tempfile::tempdir().unwrap();
            let me = std::process::id();
            std::fs::write(
                dir.path().join("active_sessions.json"),
                format!(r#"[{{"session_id":"live-1","pid":{me},"cwd":"/r"}}]"#),
            )
            .unwrap();
            let snap = live_grok_session_ids(dir.path()).expect("a readable registry is healthy");
            assert_eq!(
                snap.pid_of.get("live-1"),
                Some(&(me as i32)),
                "a live registry entry must bind its id to its pid"
            );
        }

        #[test]
        fn live_entries_bind_ids_to_pids() {
            let snap = grok_ids_from_registry(REG.as_bytes(), alive_all, |_| None, None).unwrap();
            assert_eq!(snap.pid_of.get("0197-a"), Some(&100));
            assert_eq!(snap.pid_of.get("0197-b"), Some(&200));
        }

        #[test]
        fn dead_pids_and_junk_values_are_skipped_not_failures() {
            let snap = grok_ids_from_registry(REG.as_bytes(), alive_none, |_| None, None).unwrap();
            assert!(snap.pid_of.is_empty(), "dead pids yield a healthy empty");
            let junk = r#"[{"session_id":"","pid":100,"cwd":"/r","opened_at":"x"},
                           {"session_id":"s","pid":0,"cwd":"/r","opened_at":"x"}]"#;
            let snap = grok_ids_from_registry(junk.as_bytes(), alive_all, |_| None, None).unwrap();
            assert!(snap.pid_of.is_empty(), "junk VALUES skip entries silently");
        }

        #[test]
        fn unparseable_document_is_format_drift_none() {
            assert!(grok_ids_from_registry(b"not json", alive_all, |_| None, None).is_none());
            assert!(
                grok_ids_from_registry(br#"{"an":"object"}"#, alive_all, |_| None, None).is_none()
            );
        }

        #[test]
        fn recycled_pid_is_rejected_when_both_sides_agree_it_is() {
            // opened_at 12:00:05Z = epoch 1784203205. A process started LATER
            // than the claim + tolerance ⇒ the original process died and the
            // pid was recycled — the entry must be skipped.
            let opened =
                crate::source::decoder::rfc3339_to_epoch_secs("2026-07-16T12:00:05Z").unwrap();
            let tolerance = crate::source::cc_probe::PID_START_TOLERANCE_SECS;
            let recycled_start = opened + tolerance + 1;
            let snap =
                grok_ids_from_registry(REG.as_bytes(), alive_all, |_| Some(recycled_start), None);
            assert_eq!(snap.unwrap().pid_of.get("0197-a"), None);
            // At exactly claim + tolerance the entry SURVIVES (boundary
            // derived from the shared const, both sides pinned).
            let boundary_start = opened + tolerance;
            let snap =
                grok_ids_from_registry(REG.as_bytes(), alive_all, |_| Some(boundary_start), None);
            assert_eq!(snap.unwrap().pid_of.get("0197-a"), Some(&100));
            // A process started BEFORE the claim is the NORMAL welcome-screen
            // lag (session opened after process start) — never rejected.
            let snap =
                grok_ids_from_registry(REG.as_bytes(), alive_all, |_| Some(opened - 3600), None);
            assert_eq!(snap.unwrap().pid_of.get("0197-a"), Some(&100));
        }

        #[test]
        fn duplicate_session_id_keeps_the_larger_pid_in_both_orders() {
            for reg in [
                r#"[{"session_id":"s","pid":100,"cwd":"/r","opened_at":"2026-01-01T00:00:00Z"},
                    {"session_id":"s","pid":200,"cwd":"/r","opened_at":"2026-01-01T00:00:00Z"}]"#,
                r#"[{"session_id":"s","pid":200,"cwd":"/r","opened_at":"2026-01-01T00:00:00Z"},
                    {"session_id":"s","pid":100,"cwd":"/r","opened_at":"2026-01-01T00:00:00Z"}]"#,
            ] {
                let snap =
                    grok_ids_from_registry(reg.as_bytes(), alive_all, |_| None, None).unwrap();
                assert_eq!(snap.pid_of.get("s"), Some(&200));
            }
        }

        /// A pid that is definitively dead: spawn a child and reap it. Beats a
        /// large literal — 999_999 sits above macOS's PID_MAX but well inside
        /// Linux's default `pid_max` (4194304), which would make the assertion
        /// environment-dependent.
        fn reaped_pid() -> i32 {
            let mut child = std::process::Command::new("true").spawn().unwrap();
            let pid = child.id() as i32;
            child.wait().unwrap();
            pid
        }

        /// Hold the leader's own exclusive flock the way `run_leader` does, with
        /// `contents` as its `write_pid` payload. `flock` is per open-file
        /// DESCRIPTION, so the detector's separate `File` contends with ours
        /// inside one process — no leader to spawn.
        fn with_held_lock<T>(path: &Path, contents: &str, f: impl FnOnce() -> T) -> T {
            std::fs::write(path, contents).unwrap();
            let held = std::fs::File::open(path).unwrap();
            held.lock().expect("take the leader's exclusive lock");
            let out = f();
            drop(held);
            out
        }

        fn registry_with(dir: &Path, session_id: &str, pid: i32) {
            std::fs::write(
                dir.join("active_sessions.json"),
                format!(r#"[{{"session_id":"{session_id}","pid":{pid},"cwd":"/r"}}]"#),
            )
            .unwrap();
        }

        /// A live leader OWNS the sessions the registry lists, so they bind to
        /// ITS pid — not the TUI client's (#826). The client binding is what
        /// reaches the `ExitWatch`, so binding it was the whole defect: the
        /// client's death ended a session the leader was still driving.
        ///
        /// The leader pid here is a spawned CHILD we keep alive, so it differs
        /// from the client's and the assertion can tell which one won.
        #[test]
        fn a_live_leader_owns_the_registrys_sessions_instead_of_the_client() {
            let tmp = tempfile::tempdir().unwrap();
            let me = std::process::id() as i32;
            registry_with(tmp.path(), "kept-1", me);
            let lock = tmp.path().join("leader.lock");

            let mut leader = std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .unwrap();
            let leader_pid = leader.id() as i32;

            // An UNHELD lock file naming that same live process is residue from
            // a dead leader, not a vouch — the client keeps its own binding.
            std::fs::write(&lock, format!("{leader_pid}")).unwrap();
            let snap = live_grok_session_ids(tmp.path()).expect("healthy");
            assert_eq!(
                snap.pid_of.get("kept-1"),
                Some(&me),
                "an unheld lock must not move ownership"
            );

            // Held → ownership moves, though the client pid is alive and unchanged.
            let snap = with_held_lock(&lock, &format!("{leader_pid}"), || {
                live_grok_session_ids(tmp.path()).expect("healthy")
            });
            let _ = leader.kill();
            let _ = leader.wait();
            assert_eq!(
                snap.pid_of.get("kept-1"),
                Some(&leader_pid),
                "a leader-kept session binds to the LEADER's pid, not the client's"
            );
        }

        /// The registry keeps CRASH residue — entries are "removed on clean
        /// quit, left behind on crash" — and `alive` is the only filter against
        /// it. A vouched id bypasses the first-sight recency gate, emits
        /// ProofOfLife, and is exempt from BOTH sweeps, so binding residue
        /// unconditionally would render an immortal ghost desk for as long as
        /// any leader runs. The leader moves the BINDING, never admission.
        #[test]
        fn a_live_leader_does_not_vouch_crash_residue_from_a_dead_client() {
            let tmp = tempfile::tempdir().unwrap();
            registry_with(tmp.path(), "leaked-1", reaped_pid());
            let me = std::process::id().to_string();
            let snap = with_held_lock(&tmp.path().join("leader.lock"), &me, || {
                live_grok_session_ids(tmp.path()).expect("healthy")
            });
            assert_eq!(
                snap.pid_of.get("leaked-1"),
                None,
                "a crashed client's leaked entry must not become an immortal ghost"
            );
        }

        /// The flock proves a leader is ALIVE; the contents are a SEPARATE fact,
        /// and upstream documents them diverging — a client that wins the flock
        /// while spawning a leader holds it across the handoff WITHOUT rewriting
        /// the file, so it still names the previous, dead leader. Binding that
        /// pid would register it with the `ExitWatch`, which synthesizes an
        /// immediate exit on ESRCH, ending every grok session at once.
        #[test]
        fn a_held_lock_naming_a_dead_process_vouches_nothing() {
            let tmp = tempfile::tempdir().unwrap();
            let me = std::process::id() as i32;
            registry_with(tmp.path(), "s", me);
            let dead = reaped_pid().to_string();
            let snap = with_held_lock(&tmp.path().join("leader.lock"), &dead, || {
                live_grok_session_ids(tmp.path()).expect("healthy")
            });
            assert_eq!(
                snap.pid_of.get("s"),
                Some(&me),
                "a held lock naming a corpse is not a leader — fall back to the client"
            );
        }

        /// A focus click must raise the window the user TYPES in. Under a live
        /// leader the liveness probe binds to the leader (whose death ends the
        /// session), so the two consumers need different answers off one
        /// registry — and grok stamps no pid on the hook path, so this probe is
        /// the click's only source.
        #[test]
        fn focus_resolves_the_client_pid_even_while_a_leader_owns_the_session() {
            let tmp = tempfile::tempdir().unwrap();
            let me = std::process::id() as i32;
            registry_with(tmp.path(), "s", me);
            let mut leader = std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .unwrap();
            let leader_pid = leader.id() as i32;
            let (owner, focus) = with_held_lock(
                &tmp.path().join("leader.lock"),
                &format!("{leader_pid}"),
                || {
                    (
                        live_grok_session_ids(tmp.path())
                            .unwrap()
                            .pid_of
                            .get("s")
                            .copied(),
                        // The CONSUMER's entry point, not the helper it
                        // delegates to: a test on the helper survives
                        // re-pointing `grok_pid_for_session` at the leader map.
                        crate::source::grok_pid_for_session(tmp.path(), "s"),
                    )
                },
            );
            let _ = leader.kill();
            let _ = leader.wait();
            assert_eq!(owner, Some(leader_pid), "the ExitWatch follows the leader");
            assert_eq!(focus, Some(me), "the click follows the client");
            assert_eq!(
                crate::source::grok_pid_for_session(tmp.path(), "nope"),
                None
            );
        }

        /// The lock name is `leader{suffix}.lock`, where suffix is a hash of the
        /// WS URL — `DefaultHasher`, whose output std does not guarantee stable,
        /// so the resolver enumerates instead of recomputing. Order is
        /// load-bearing: `live_leader_pid` takes the FIRST held candidate, so
        /// `read_dir` order must not decide which of two live leaders owns every
        /// session. The env override is additive and first, never a replacement
        /// — it is read from OUR environment, not the leader's.
        #[test]
        fn leader_lock_resolution_is_ordered_and_the_env_override_is_additive() {
            let tmp = tempfile::tempdir().unwrap();
            for n in [
                "leader.lock",
                "leader-9f2a1c04.lock",
                "active_sessions.lock",
            ] {
                std::fs::write(tmp.path().join(n), "1").unwrap();
            }
            let names = |v: &[PathBuf]| -> Vec<String> {
                v.iter()
                    .filter_map(|p| p.file_name()?.to_str().map(str::to_owned))
                    .collect()
            };
            assert_eq!(
                names(&leader_lock_candidates(tmp.path(), None)),
                vec!["leader-9f2a1c04.lock", "leader.lock"],
                "both leader forms, deterministically ordered; the sibling \
                 active_sessions.lock is not a candidate"
            );

            let sock = PathBuf::from("/sandbox/run/leader-dev.sock");
            let with_override = leader_lock_candidates(tmp.path(), Some(sock));
            assert_eq!(
                names(&with_override),
                vec!["leader-dev.lock", "leader-9f2a1c04.lock", "leader.lock"],
                "the override is most-specific-first AND extends the enumeration \
                 — a stray override must not blank leader detection"
            );
        }

        /// Negative control for the detector: no candidates, an unopenable one,
        /// and a held lock whose contents are not a usable pid are all "no
        /// leader" rather than a panic or a false vouch.
        #[test]
        fn leader_detection_is_quiet_when_there_is_nothing_to_find() {
            assert_eq!(live_leader_pid(&[]), None);
            assert_eq!(
                live_leader_pid(&[PathBuf::from("/nonexistent/leader.lock")]),
                None
            );
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("leader.lock");
            // "0" and "-5" REACH `checked_pid` (unlike "not-a-pid", which dies
            // at parse), so they pin the narrowing rather than the parser.
            for junk in ["not-a-pid", "0", "-5", ""] {
                let got =
                    with_held_lock(&path, junk, || live_leader_pid(std::slice::from_ref(&path)));
                assert_eq!(got, None, "a held lock reading {junk:?} vouches nothing");
            }
        }
    }
}
