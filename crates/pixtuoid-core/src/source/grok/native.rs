//! The `native`-only runtime half of the grok source: the liveness probe over
//! grok's own crash-recovery registry (`active_sessions.json`) + `GrokSource`
//! and its `JsonlWatcher` wiring. The pure decoders stay in the always-compiled
//! parent module; this whole file sits behind the parent's ONE
//! `#[cfg(feature = "native")] mod native;` gate and is re-exported there.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{
    decode_grok_line, grok_cwd_from_path, grok_home, grok_id_from_path, grok_session_ended,
    SOURCE_NAME,
};
use crate::source::jsonl::{ChildEndUnclaims, JsonlWatcher, ProbeSnapshot};
use crate::source::{Source, TaggedSender};

/// Only `updates.jsonl` is the tailable transcript. Every session dir carries
/// SIBLING `.jsonl` files that must never be walked: `chat_history.jsonl` and
/// `rewind_points.jsonl` are REWRITTEN via temp+rename (a tail would replay
/// whole files as fresh events and mint a second path-keyed sprite), and
/// `feedback.jsonl`/`btw_history.jsonl`/the group-level `prompt_history.jsonl`
/// are not session streams at all.
fn is_updates_jsonl(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()) == Some("updates.jsonl")
}

/// grok's liveness probe: the session ids of every entry in
/// `{grok_home}/active_sessions.json` whose pid is alive, in
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
#[cfg(unix)]
pub fn live_grok_session_ids(grok_root: &Path) -> Option<ProbeSnapshot> {
    let path = grok_root.join("active_sessions.json");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        // Absent registry = healthy "no TUI clients"; the leader arm may
        // still vouch (a headless-into-leader setup writes no registry
        // entries at all — exactly the #638 gap).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Some(augment_with_leader_vouch(
                ProbeSnapshot::default(),
                grok_root,
            ))
        }
        Err(_) => return None,
    };
    match grok_ids_from_registry(&bytes, crate::source::cc_probe::pid_alive, |pid| {
        crate::source::cc_probe::pid_start_time_secs(pid)
    }) {
        Some(snap) => Some(augment_with_leader_vouch(snap, grok_root)),
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

#[cfg(not(unix))]
pub fn live_grok_session_ids(_grok_root: &Path) -> Option<ProbeSnapshot> {
    None
}

/// A leader-mode session's transcript counts as fresh for this long after its
/// last append — 2.5× the watcher's 60s poll (two missed polls + slack), the
/// same derivation as the reducer's `PROOF_OF_LIFE_TTL` (a DISTINCT semantic —
/// leader-session freshness vs vouch-emission TTL — that shares the rationale,
/// hence a separate named const rather than a cross-layer reuse).
#[cfg(unix)]
const LEADER_SESSION_FRESH_SECS: u64 = 150;

/// The #638 leader-mode secondary vouch. In opt-in `--leader` mode the AGENT
/// (session-file writes + hook firing) runs in a shared LEADER process while
/// `active_sessions.json` records only each TUI CLIENT's pid — so a client
/// disconnect used to read as the session's exit even though the leader keeps
/// it alive. When the leader's rendezvous socket (`{grok_home}/leader.sock`)
/// has a LIVE owner (a grok process holding it open — the same open-fd
/// evidence the Codex probe rides), every session whose `updates.jsonl` was
/// appended within [`LEADER_SESSION_FRESH_SECS`] is vouched, bound to the
/// LEADER's pid (leader death ⇒ its sessions exit via the ExitWatch —
/// correct, the leader owns them). A registry (client-pid) binding wins where
/// both exist. mtime is a weaker signal than the pid registry, bounded both
/// ways: an idle-in-leader session's vouch lapses after the window (falls to
/// the short-idle reap + prompt resurrect — the pre-#638 behavior), and a
/// just-ended leader session stays vouched ≤ the window (the negative vouch
/// ends it ~2 min later). Non-leader setups pay one `Path::exists` per probe
/// refresh (no socket → no proc enumeration).
#[cfg(unix)]
fn augment_with_leader_vouch(snap: ProbeSnapshot, grok_root: &Path) -> ProbeSnapshot {
    let sock = grok_root.join("leader.sock");
    if !sock.exists() {
        return snap;
    }
    let Some(leader_pid) = leader_socket_owner(&sock) else {
        // Socket file exists but no live process holds it open — residue from
        // a killed leader; the stale-file case, not a live leader.
        return snap;
    };
    augment_with_fresh_sessions(
        snap,
        &grok_root.join("sessions"),
        leader_pid,
        std::time::SystemTime::now(),
    )
}

/// The pid holding `leader.sock` open, via the shared fd probe (macOS libproc /
/// Linux /proc). BOTH installed comm names are probed — the installer links
/// the binary as `grok` AND `agent` — plus the from-source artifact name.
#[cfg(unix)]
fn leader_socket_owner(sock: &Path) -> Option<i32> {
    // Kernel-reported fd paths come back canonicalized (the /tmp →
    // /private/tmp class) — compare against the canonical socket path.
    let sock = sock.canonicalize().ok()?;
    for comm in ["grok", "agent", "xai-grok-pager"] {
        for pid in crate::source::fd_probe::pids_by_name(comm)? {
            if crate::source::fd_probe::open_vnode_paths(pid).contains(&sock) {
                return Some(pid);
            }
        }
    }
    None
}

/// The pure join half (unit-testable with a tempdir + injected `now`): walk
/// `sessions/<enc-cwd>/<session-id>/updates.jsonl` and bind every id whose
/// transcript was appended within the freshness window to `leader_pid` —
/// without displacing an existing (registry/client) binding.
#[cfg(unix)]
fn augment_with_fresh_sessions(
    mut snap: ProbeSnapshot,
    sessions_root: &Path,
    leader_pid: i32,
    now: std::time::SystemTime,
) -> ProbeSnapshot {
    let Ok(groups) = std::fs::read_dir(sessions_root) else {
        return snap;
    };
    for group in groups.flatten() {
        let Ok(sessions) = std::fs::read_dir(group.path()) else {
            continue;
        };
        for session in sessions.flatten() {
            let updates = session.path().join("updates.jsonl");
            let fresh = std::fs::metadata(&updates)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|mtime| now.duration_since(mtime).ok())
                .is_some_and(|age| age.as_secs() <= LEADER_SESSION_FRESH_SECS);
            if fresh {
                let id = super::grok_id_from_path(&updates);
                snap.pid_of.entry(id).or_insert(leader_pid);
            }
        }
    }
    snap
}

/// The pure join half of the probe (unit-testable with injected liveness
/// fns): parse the registry array, keep entries whose pid is alive AND — when
/// BOTH sides are available — whose `opened_at` matches the kernel-reported
/// process start within [`cc_probe::PID_START_TOLERANCE_SECS`] (the #220
/// pid-recycle identity check; either side missing → pid-alive-only, the
/// check is additive). Returns `None` only when the DOCUMENT doesn't parse as
/// an array of entries (format drift); junk VALUES inside an entry (pid <= 0)
/// skip that entry silently, mirroring the CC registry's value-vs-shape
/// distinction.
#[cfg(unix)]
fn grok_ids_from_registry(
    bytes: &[u8],
    alive: fn(i32) -> bool,
    start_time: impl Fn(i32) -> Option<u64>,
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
        if e.pid <= 0 || e.session_id.is_empty() || !alive(e.pid) {
            continue;
        }
        if let (Some(claimed_secs), Some(actual_secs)) = (
            e.opened_at.as_deref().and_then(rfc3339_to_epoch_secs),
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

/// Minimal RFC3339 → epoch seconds for the registry's `opened_at` (chrono
/// `DateTime<Utc>` serialization: `YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)`).
/// Core deliberately carries no date dependency for one field read on one
/// probe path; the identity check is additive, so `None` on anything
/// unexpected simply degrades that entry to pid-alive-only.
#[cfg(unix)]
fn rfc3339_to_epoch_secs(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' && b[10] != b't' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    // Trailing zone: skip an optional fraction, then Z or ±HH:MM.
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    let offset_secs: i64 = match b.get(i) {
        Some(b'Z') | Some(b'z') if i + 1 == b.len() => 0,
        Some(sign @ (b'+' | b'-')) if i + 6 == b.len() && b.get(i + 3) == Some(&b':') => {
            let oh = num(i + 1..i + 3)?;
            let om = num(i + 4..i + 6)?;
            let mag = oh * 3600 + om * 60;
            if *sign == b'+' {
                mag
            } else {
                -mag
            }
        }
        _ => return None,
    };
    // Howard Hinnant's days-from-civil (the standard branchless algorithm).
    let (y_adj, era_m) = if mo <= 2 {
        (y - 1, mo + 9)
    } else {
        (y, mo - 3)
    };
    let era = y_adj.div_euclid(400);
    let yoe = y_adj - era * 400;
    let doy = (153 * era_m + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec - offset_secs;
    u64::try_from(secs).ok()
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
        .with_id_deriver(grok_id_from_path)
        .with_path_filter(is_updates_jsonl)
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
    fn path_filter_admits_only_updates_jsonl() {
        let dir = Path::new("/h/.grok/sessions/%2Fr/0197-sess");
        assert!(is_updates_jsonl(&dir.join("updates.jsonl")));
        for sibling in [
            "chat_history.jsonl",
            "rewind_points.jsonl",
            "feedback.jsonl",
            "btw_history.jsonl",
            "prompt_history.jsonl",
        ] {
            assert!(
                !is_updates_jsonl(&dir.join(sibling)),
                "{sibling} must be filtered (rewrite-on-resume / not a session stream)"
            );
        }
    }

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

        #[test]
        fn live_entries_bind_ids_to_pids() {
            let snap = grok_ids_from_registry(REG.as_bytes(), alive_all, |_| None).unwrap();
            assert_eq!(snap.pid_of.get("0197-a"), Some(&100));
            assert_eq!(snap.pid_of.get("0197-b"), Some(&200));
        }

        #[test]
        fn dead_pids_and_junk_values_are_skipped_not_failures() {
            let snap = grok_ids_from_registry(REG.as_bytes(), alive_none, |_| None).unwrap();
            assert!(snap.pid_of.is_empty(), "dead pids yield a healthy empty");
            let junk = r#"[{"session_id":"","pid":100,"cwd":"/r","opened_at":"x"},
                           {"session_id":"s","pid":0,"cwd":"/r","opened_at":"x"}]"#;
            let snap = grok_ids_from_registry(junk.as_bytes(), alive_all, |_| None).unwrap();
            assert!(snap.pid_of.is_empty(), "junk VALUES skip entries silently");
        }

        #[test]
        fn unparseable_document_is_format_drift_none() {
            assert!(grok_ids_from_registry(b"not json", alive_all, |_| None).is_none());
            assert!(grok_ids_from_registry(br#"{"an":"object"}"#, alive_all, |_| None).is_none());
        }

        #[test]
        fn recycled_pid_is_rejected_when_both_sides_agree_it_is() {
            // opened_at 12:00:05Z = epoch 1784203205. A process started LATER
            // than the claim + tolerance ⇒ the original process died and the
            // pid was recycled — the entry must be skipped.
            let opened = rfc3339_to_epoch_secs("2026-07-16T12:00:05Z").unwrap();
            let tolerance = crate::source::cc_probe::PID_START_TOLERANCE_SECS;
            let recycled_start = opened + tolerance + 1;
            let snap = grok_ids_from_registry(REG.as_bytes(), alive_all, |_| Some(recycled_start));
            assert_eq!(snap.unwrap().pid_of.get("0197-a"), None);
            // At exactly claim + tolerance the entry SURVIVES (boundary
            // derived from the shared const, both sides pinned).
            let boundary_start = opened + tolerance;
            let snap = grok_ids_from_registry(REG.as_bytes(), alive_all, |_| Some(boundary_start));
            assert_eq!(snap.unwrap().pid_of.get("0197-a"), Some(&100));
            // A process started BEFORE the claim is the NORMAL welcome-screen
            // lag (session opened after process start) — never rejected.
            let snap = grok_ids_from_registry(REG.as_bytes(), alive_all, |_| Some(opened - 3600));
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
                let snap = grok_ids_from_registry(reg.as_bytes(), alive_all, |_| None).unwrap();
                assert_eq!(snap.pid_of.get("s"), Some(&200));
            }
        }

        #[test]
        fn leader_vouch_binds_fresh_sessions_without_displacing_client_pids() {
            use std::time::{Duration, SystemTime};
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().join("sessions");
            let mk = |group: &str, sid: &str| {
                let d = root.join(group).join(sid);
                std::fs::create_dir_all(&d).unwrap();
                std::fs::write(d.join("updates.jsonl"), "{}\n").unwrap();
            };
            mk("%2Frepo", "fresh-a");
            mk("%2Frepo", "fresh-b");
            let now = SystemTime::now();

            // A registry (client) binding survives; fresh ids gain the leader pid.
            let mut seeded = ProbeSnapshot::default();
            seeded.pid_of.insert("fresh-a".into(), 111);
            let snap = augment_with_fresh_sessions(seeded, &root, 999, now);
            assert_eq!(
                snap.pid_of.get("fresh-a"),
                Some(&111),
                "client-pid binding must win over the leader's"
            );
            assert_eq!(snap.pid_of.get("fresh-b"), Some(&999));

            // BOTH sides of the freshness boundary, offsets derived from the
            // const: at exactly the window the session is still vouched; one
            // second past it is not.
            let at_edge = now + Duration::from_secs(LEADER_SESSION_FRESH_SECS);
            let past_edge = now + Duration::from_secs(LEADER_SESSION_FRESH_SECS + 1);
            let snap = augment_with_fresh_sessions(ProbeSnapshot::default(), &root, 7, at_edge);
            assert_eq!(snap.pid_of.get("fresh-b"), Some(&7), "edge-inclusive");
            let snap = augment_with_fresh_sessions(ProbeSnapshot::default(), &root, 7, past_edge);
            assert_eq!(snap.pid_of.get("fresh-b"), None, "stale past the window");

            // A missing sessions root is a quiet no-op (additive-only arm).
            let snap = augment_with_fresh_sessions(
                ProbeSnapshot::default(),
                &tmp.path().join("nope"),
                7,
                now,
            );
            assert!(snap.pid_of.is_empty());
        }

        #[test]
        fn absent_leader_socket_is_a_no_op_augment() {
            // No leader.sock → the snapshot passes through untouched (the
            // non-leader common case pays one exists() only).
            let tmp = tempfile::tempdir().unwrap();
            let mut seeded = ProbeSnapshot::default();
            seeded.pid_of.insert("s".into(), 42);
            let snap = augment_with_leader_vouch(seeded, tmp.path());
            assert_eq!(snap.pid_of.get("s"), Some(&42));
            assert_eq!(snap.pid_of.len(), 1);
        }

        #[test]
        fn rfc3339_parses_chrono_utc_shapes_and_rejects_garbage() {
            // 2026-07-16T12:00:05Z — cross-checked epoch.
            assert_eq!(
                rfc3339_to_epoch_secs("2026-07-16T12:00:05Z"),
                Some(1_784_203_205)
            );
            // Fractional seconds + explicit offset forms (chrono's default
            // to_rfc3339 uses `+00:00`).
            assert_eq!(
                rfc3339_to_epoch_secs("2026-07-16T12:00:05.123456+00:00"),
                Some(1_784_203_205)
            );
            // A NON-UTC offset shifts the epoch.
            assert_eq!(
                rfc3339_to_epoch_secs("2026-07-16T14:00:05+02:00"),
                Some(1_784_203_205)
            );
            // Epoch anchor sanity.
            assert_eq!(rfc3339_to_epoch_secs("1970-01-01T00:00:00Z"), Some(0));
            for bad in ["", "2026-07-16", "not a date", "2026-13-01T00:00:00Z"] {
                assert_eq!(rfc3339_to_epoch_secs(bad), None, "{bad:?}");
            }
        }
    }
}
