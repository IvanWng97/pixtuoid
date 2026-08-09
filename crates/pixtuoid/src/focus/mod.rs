//! Focus-jump: click a sprite → the terminal app hosting that agent comes to the
//! foreground. `resolve_pid` (slot cache → per-source probe) → `ancestor_walk`
//! (pid → the first *focusable* ancestor, i.e. the terminal GUI app) → per-OS
//! `activate`. App-level only by design — no tab/pane precision.
//! Spec: docs/superpowers/specs/2026-07-10-focus-jump-design.md.
//!
//! ONE failure rule (user-directed, no fallbacks): any miss — no pid, walk
//! reaches pid 1, remote agent, activation denied, unsupported compositor — is a
//! SILENT no-op with a `tracing::debug!` breadcrumb.
//!
//! KNOWN common miss (#538): an agent inside a terminal MULTIPLEXER. The
//! multiplexer SERVER is daemonized and owns no GUI surface, so the walk
//! dead-ends at pid 1.
//!
//! Lives in the BINARY (invariant #1: core/scene stay window-free); the walker
//! is PURE over an injected [`ProcessTable`], the per-OS glue thin and
//! codecov-ignored.

use std::path::Path;

use pixtuoid_core::AgentSlot;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::{HYPRLAND_ENV, SWAY_ENV};
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// The process-tree view `ancestor_walk` needs — injected so the walk is a pure
/// function. Deliberately BUNDLES the kernel reads (`ppid`/`start_time`) with
/// the window-ownership probe (`focusable`): one trait = one mock across 3 OSes.
/// Split it only when tab-precision lands and forces a terminal classifier — not
/// before (YAGNI).
pub(crate) trait ProcessTable {
    /// Parent pid, `None` when the process is gone / unreadable.
    fn ppid(&self, pid: i32) -> Option<i32>;
    /// Whether this pid owns a focusable surface (a regular GUI app on macOS,
    /// a top-level window on Windows/X11).
    fn focusable(&self, pid: i32) -> bool;
    /// Kernel start marker for `pid` — the identity half of the recycle guard.
    /// The default rides the SAME core read the hook stamp used, so equality
    /// means "same incarnation"; mock tables override it.
    fn start_time(&self, pid: i32) -> Option<u64> {
        pixtuoid_core::source::pid_start_marker(pid)
    }
}

/// Walk `ppid` upward from `start` until the first focusable ancestor.
/// Stops at pid ≤ 1 (launchd/init — headless/SSH chains end here) and on a
/// cycle (a corrupt/racing table must terminate, never loop).
pub(crate) fn ancestor_walk(table: &impl ProcessTable, start: i32) -> Option<i32> {
    let mut seen = std::collections::HashSet::new();
    let mut pid = start;
    while pid > 1 && seen.insert(pid) {
        if table.focusable(pid) {
            return Some(pid);
        }
        pid = table.ppid(pid)?;
    }
    None
}

/// The per-source pid lookup roots click-time resolution needs; `None` disables
/// that family.
pub(crate) struct FocusPaths<'a> {
    /// CC's projects root (`~/.claude/projects`).
    pub cc_projects_root: Option<&'a Path>,
    /// Codex's sessions root (rollout tree) for the fd probe.
    pub codex_sessions_root: Option<&'a Path>,
    /// grok's home root (`active_sessions.json`'s parent). No CLI override exists
    /// — `focus_slot` resolves it from `grok_home()`; the field exists so tests
    /// inject it like the other two.
    pub grok_root: Option<&'a Path>,
}

/// Resolve the agent's OS pid. Precedence: the slot's cached pid (hook-family
/// sources) → the transcript-family point queries (CC registry / Codex fd probe)
/// → None. Two click-time recycle guards on the cached path: an EXITING slot
/// refuses outright (its process is going or gone), and a cached start marker
/// must match the kernel's CURRENT marker for that pid — a mismatch means the
/// pid was recycled by an unrelated process after an abrupt death, and a missing
/// current read means the process is gone (on unix — a Windows marker outlives
/// the process while any handle to it is open). A cache stamped WITHOUT a
/// marker skips the identity check entirely: rare now that every supported OS
/// can read one, but it means an unreadable marker yields an UNGUARDED pid.
pub(crate) fn resolve_pid(
    slot: &AgentSlot,
    paths: &FocusPaths<'_>,
    table: &impl ProcessTable,
) -> Option<i32> {
    if slot.exiting_at.is_some() {
        tracing::debug!(agent = %slot.label, "focus: refused — agent is exiting");
        return None;
    }
    if let Some(cached) = slot.pid {
        if let Some(stamped) = cached.started {
            if table.start_time(cached.pid) != Some(stamped) {
                tracing::debug!(
                    agent = %slot.label,
                    pid = cached.pid,
                    "focus: refused — cached pid gone or recycled (start marker mismatch)"
                );
                return None;
            }
        }
        return Some(cached.pid);
    }
    // The probe FNS stay here in the binary: the registry const table compiles to
    // wasm and cannot hold a native-only fn pointer. The name match uses the
    // registry consts, not literals — a source rename must not silently drop an
    // arm to `_ => None`.
    use pixtuoid_core::source::registry::FocusChannel;
    let channel = pixtuoid_core::source::registry::descriptor_for(&slot.source)
        .map_or(FocusChannel::Unsupported, |d| d.focus_channel());
    if channel != FocusChannel::TranscriptProbe {
        return None;
    }
    match slot.source.as_ref() {
        s if s == pixtuoid_core::source::claude_code::SOURCE_NAME => paths
            .cc_projects_root
            .and_then(|d| pixtuoid_core::source::cc_pid_for_session(d, &slot.session_id)),
        s if s == pixtuoid_core::source::codex::SOURCE_NAME => paths
            .codex_sessions_root
            .and_then(|d| pixtuoid_core::source::codex_pid_for_session(d, &slot.session_id)),
        s if s == pixtuoid_core::source::grok::SOURCE_NAME => paths
            .grok_root
            .and_then(|d| pixtuoid_core::source::grok_pid_for_session(d, &slot.session_id)),
        _ => None,
    }
}

/// The painter-agnostic focus dispatch: ANY painter's trigger resolves + walks +
/// activates through this ONE entry with the real OS table. `roots` = (CC
/// projects root, Codex sessions root).
pub(crate) fn focus_slot(
    slot: &AgentSlot,
    roots: &(Option<std::path::PathBuf>, Option<std::path::PathBuf>),
) {
    let grok_home = pixtuoid_core::source::grok::grok_home();
    let paths = FocusPaths {
        cc_projects_root: roots.0.as_deref(),
        codex_sessions_root: roots.1.as_deref(),
        grok_root: Some(&grok_home),
    };
    focus_agent(slot, &paths, &OsProcessTable, activate_os);
}

/// The ONE caller of the per-OS glue. `activate` is injected so dispatch tests
/// never touch the OS; production passes [`activate_os`].
pub(crate) fn focus_agent(
    slot: &AgentSlot,
    paths: &FocusPaths<'_>,
    table: &impl ProcessTable,
    activate: impl FnOnce(i32) -> bool,
) {
    let Some(pid) = resolve_pid(slot, paths, table) else {
        tracing::debug!(agent = %slot.label, "focus: no pid resolved");
        return;
    };
    let Some(app_pid) = ancestor_walk(table, pid) else {
        tracing::debug!(agent = %slot.label, pid, "focus: no focusable ancestor");
        return;
    };
    if !activate(app_pid) {
        tracing::debug!(agent = %slot.label, app_pid, "focus: activation declined");
    }
}

#[cfg(target_os = "linux")]
pub(crate) use linux::{activate_os, OsProcessTable};
#[cfg(target_os = "macos")]
pub(crate) use macos::{activate_os, OsProcessTable};
#[cfg(windows)]
pub(crate) use windows::{activate_os, OsProcessTable};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct MockTable {
        parents: HashMap<i32, i32>,
        focusable: Vec<i32>,
        /// pid → current kernel start marker; empty = every pid reads `None`.
        started: HashMap<i32, u64>,
    }
    impl ProcessTable for MockTable {
        fn ppid(&self, pid: i32) -> Option<i32> {
            self.parents.get(&pid).copied()
        }
        fn focusable(&self, pid: i32) -> bool {
            self.focusable.contains(&pid)
        }
        fn start_time(&self, pid: i32) -> Option<u64> {
            self.started.get(&pid).copied()
        }
    }

    #[test]
    fn walk_finds_the_first_focusable_ancestor() {
        // cc(300) → zsh(200) → iTerm2(100, GUI) → launchd(1)
        let t = MockTable {
            parents: HashMap::from([(300, 200), (200, 100), (100, 1)]),
            focusable: vec![100],
            started: HashMap::new(),
        };
        assert_eq!(ancestor_walk(&t, 300), Some(100));
    }

    #[test]
    fn walk_stops_at_pid_1_without_a_hit() {
        // ssh chain: cc(300) → sshd(200) → launchd(1), nothing focusable.
        let t = MockTable {
            parents: HashMap::from([(300, 200), (200, 1)]),
            focusable: vec![],
            started: HashMap::new(),
        };
        assert_eq!(ancestor_walk(&t, 300), None);
    }

    #[test]
    fn walk_terminates_on_a_cycle() {
        // Corrupt/racing table: 300 → 200 → 300 → …
        let t = MockTable {
            parents: HashMap::from([(300, 200), (200, 300)]),
            focusable: vec![],
            started: HashMap::new(),
        };
        assert_eq!(ancestor_walk(&t, 300), None);
        let t2 = MockTable {
            parents: HashMap::from([(300, 300)]),
            focusable: vec![],
            started: HashMap::new(),
        };
        assert_eq!(ancestor_walk(&t2, 300), None);
    }

    #[test]
    fn walk_of_a_dead_pid_is_a_silent_miss() {
        let t = MockTable {
            parents: HashMap::new(),
            focusable: vec![],
            started: HashMap::new(),
        };
        assert_eq!(ancestor_walk(&t, 4242), None);
    }

    #[test]
    fn walk_start_itself_can_be_the_focusable_app() {
        let t = MockTable {
            parents: HashMap::new(),
            focusable: vec![300],
            started: HashMap::new(),
        };
        assert_eq!(ancestor_walk(&t, 300), Some(300));
    }

    fn pid_id(pid: i32, started: Option<u64>) -> pixtuoid_core::source::PidIdentity {
        pixtuoid_core::source::PidIdentity::new(pid, started)
    }

    fn slot(
        source: &str,
        session_id: &str,
        pid: Option<pixtuoid_core::source::PidIdentity>,
    ) -> AgentSlot {
        use pixtuoid_core::state::ActivityState;
        use std::sync::Arc;
        use std::time::SystemTime;
        let now = SystemTime::UNIX_EPOCH;
        AgentSlot {
            agent_id: pixtuoid_core::AgentId::from_parts(source, session_id),
            source: Arc::from(source),
            session_id: Arc::from(session_id),
            cwd: Arc::from(std::path::PathBuf::from("/w").as_path()),
            label: "x".into(),
            state: ActivityState::Idle,
            state_started_at: now,
            last_event_at: now,
            created_at: now,
            exiting_at: None,
            pending_idle_at: None,
            desk_index: pixtuoid_core::GlobalDeskIndex(0),
            floor_idx: 0,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
            pid,
            model: None,
            effort: None,
            tokens_used: 0,
            last_usage: None,
        }
    }

    const NO_PATHS: FocusPaths<'static> = FocusPaths {
        cc_projects_root: None,
        codex_sessions_root: None,
        grok_root: None,
    };

    fn empty_table() -> MockTable {
        MockTable {
            parents: HashMap::new(),
            focusable: vec![],
            started: HashMap::new(),
        }
    }

    #[test]
    fn resolve_pid_prefers_the_slot_cache() {
        // Markerless cache (no marker was readable): the identity check is skipped.
        let s = slot("opencode", "ses_a", Some(pid_id(4242, None)));
        assert_eq!(resolve_pid(&s, &NO_PATHS, &empty_table()), Some(4242));
    }

    #[test]
    fn resolve_pid_verifies_the_start_marker_when_stamped() {
        let s = slot("opencode", "ses_a", Some(pid_id(4242, Some(1_000))));
        let mut t = empty_table();
        t.started.insert(4242, 1_000);
        assert_eq!(resolve_pid(&s, &NO_PATHS, &t), Some(4242));
    }

    #[test]
    fn resolve_pid_refuses_a_recycled_or_dead_pid() {
        let s = slot("opencode", "ses_a", Some(pid_id(4242, Some(1_000))));
        let mut t = empty_table();
        t.started.insert(4242, 2_000);
        assert_eq!(resolve_pid(&s, &NO_PATHS, &t), None, "recycled → refuse");
        assert_eq!(
            resolve_pid(&s, &NO_PATHS, &empty_table()),
            None,
            "dead → refuse"
        );
    }

    #[test]
    fn resolve_pid_refuses_an_exiting_slot() {
        let mut s = slot("opencode", "ses_a", Some(pid_id(4242, None)));
        s.exiting_at = Some(std::time::SystemTime::UNIX_EPOCH);
        assert_eq!(resolve_pid(&s, &NO_PATHS, &empty_table()), None);
    }

    #[test]
    fn resolve_pid_misses_when_no_channel_exists() {
        let s = slot("cursor", "ses_b", None);
        assert_eq!(resolve_pid(&s, &NO_PATHS, &empty_table()), None);
        let r = slot("some-remote", "ses_c", None);
        assert_eq!(resolve_pid(&r, &NO_PATHS, &empty_table()), None);
        let a = slot("antigravity", "ses_d", None);
        assert_eq!(resolve_pid(&a, &NO_PATHS, &empty_table()), None);
    }

    #[test]
    fn transcript_probe_sources_all_have_a_resolve_arm() {
        // Lockstep pin: marking a new source `TranscriptProbe` in the registry
        // REQUIRES wiring a probe arm in `resolve_pid` — extend BOTH, then add
        // its name here.
        use pixtuoid_core::source::registry::FocusChannel;
        let wired = [
            pixtuoid_core::source::claude_code::SOURCE_NAME,
            pixtuoid_core::source::codex::SOURCE_NAME,
            pixtuoid_core::source::grok::SOURCE_NAME,
        ];
        for src in pixtuoid_core::source::registry::registered_source_names() {
            let Some(d) = pixtuoid_core::source::registry::descriptor_for(src) else {
                continue;
            };
            if d.focus_channel() == FocusChannel::TranscriptProbe {
                assert!(
                    wired.contains(&src),
                    "{src} is TranscriptProbe but resolve_pid has no probe arm for it"
                );
            }
        }
    }

    #[test]
    fn focus_agent_activates_the_walked_ancestor_and_only_then() {
        let t = MockTable {
            parents: HashMap::from([(4242, 100)]),
            focusable: vec![100],
            started: HashMap::new(),
        };
        let mut activated = None;
        focus_agent(
            &slot("opencode", "s", Some(pid_id(4242, None))),
            &NO_PATHS,
            &t,
            |p| {
                activated = Some(p);
                true
            },
        );
        assert_eq!(activated, Some(100), "the terminal app pid is activated");

        let mut called = false;
        focus_agent(&slot("cursor", "s", None), &NO_PATHS, &t, |_| {
            called = true;
            true
        });
        assert!(!called, "silent no-op without a pid");
    }

    #[test]
    fn focus_agent_no_focusable_ancestor_never_activates() {
        let t = MockTable {
            parents: HashMap::new(),
            focusable: vec![],
            started: HashMap::new(),
        };
        let mut activated: Option<i32> = None;
        focus_agent(
            &slot("opencode", "s", Some(pid_id(4242, None))),
            &NO_PATHS,
            &t,
            |p| {
                activated = Some(p);
                true
            },
        );
        assert_eq!(activated, None, "no focusable ancestor ⇒ nothing activated");
    }
}

/// Live dogfood (manual, `--ignored`): walks THIS test process's own ancestor
/// chain with the real OS table and activates the terminal app it finds. On
/// Windows it is the only reachable exercise of `activate_os`, but it reaches
/// the AttachThreadInput retry ONLY if another window holds focus when it
/// fires — run from the hosting terminal, the first unattached attempt already
/// wins. Alt-tab away after launching it.
#[cfg(all(test, any(target_os = "macos", windows)))]
mod live_dogfood {
    use super::*;

    #[test]
    #[ignore = "live: activates the real terminal hosting this test run"]
    fn walk_own_chain_and_activate_the_terminal() {
        let me = std::process::id() as i32;
        let app = ancestor_walk(&OsProcessTable, me)
            .expect("a GUI ancestor (terminal app) above this test process");
        assert!(
            activate_os(app),
            "the OS accepted the activation for pid {app}"
        );
    }
}
