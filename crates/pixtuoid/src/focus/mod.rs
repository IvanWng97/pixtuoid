//! Focus-jump: click a sprite → the terminal app hosting that agent comes to
//! the foreground. Spec: docs/superpowers/specs/2026-07-10-focus-jump-design.md.
//!
//! Pipeline: `resolve_pid` (slot cache → per-source probe) → `ancestor_walk`
//! (pid → the first *focusable* ancestor, i.e. the terminal GUI app) →
//! per-OS `activate`. App-level only, by design (v1) — no tab/pane precision.
//!
//! ONE failure rule (user-directed, no fallbacks): any miss — no pid, walk
//! reaches pid 1, remote agent, activation denied, unsupported compositor —
//! is a SILENT no-op with a `tracing::debug!` breadcrumb. Success = the
//! window comes forward; failure = nothing happens.
//!
//! KNOWN common miss (#538): an agent inside a terminal MULTIPLEXER
//! (tmux/screen/zellij). The multiplexer SERVER is daemonized (parent =
//! launchd/init) and owns no GUI surface, so the walk dead-ends at pid 1 —
//! live-verified. The fix (walk the CLIENT's chain via e.g.
//! `tmux display -p '#{client_pid}'`) is backlog #538.
//!
//! Lives in the BINARY (invariant #1: core/scene stay window-free). The
//! walker is PURE over an injected [`ProcessTable`], so the logic is unit
//! tested on mock tables; the per-OS glue (`macos`/`windows`/`linux`) is
//! thin and codecov-ignored (needs a real session/display — the
//! `floating/window.rs` class).

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

/// The process-tree view `ancestor_walk` needs — injected so the walk is a
/// pure function (mock tables in tests; the real per-OS impls query the
/// kernel). Deliberately BUNDLES kernel reads (`ppid`/`start_time`) with the
/// window-ownership probe (`focusable`): one trait = one mock across 3 OSes.
/// Split it (ProcessTree vs WindowBackend, with `activate` moving onto the
/// window half) only when tab-precision lands and forces a terminal
/// classifier — not before (YAGNI).
pub(crate) trait ProcessTable {
    /// Parent pid, `None` when the process is gone / unreadable.
    fn ppid(&self, pid: i32) -> Option<i32>;
    /// Whether this pid owns a focusable surface (a regular GUI app on macOS,
    /// a top-level window on Windows/X11).
    fn focusable(&self, pid: i32) -> bool;
    /// Kernel start marker for `pid` — the identity half of the recycle guard
    /// (#527). The default rides the shared core read (the SAME source the
    /// hook stamp used, so equality means "same incarnation"); mock tables
    /// override it.
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

/// The per-source pid lookup roots the click-time resolution needs — built by
/// the trigger site from its existing config (None disables that family).
pub(crate) struct FocusPaths<'a> {
    /// CC's projects root (`~/.claude/projects`); the sibling `sessions` pid
    /// registry is derived inside the core seam (standard-layout-gated).
    pub cc_projects_root: Option<&'a Path>,
    /// Codex's sessions root (rollout tree) for the fd probe.
    pub codex_sessions_root: Option<&'a Path>,
    /// grok's home root (`active_sessions.json`'s parent). Unlike the CC/Codex
    /// roots there is NO CLI override to thread from the driver — `focus_slot`
    /// resolves it from the one `grok_home()` authority at click time; the
    /// field exists so `resolve_pid` tests inject it like the other two.
    pub grok_root: Option<&'a Path>,
}

/// Resolve the agent's OS pid. Precedence: the slot's cached pid (hook-family
/// sources — filled from the shim/plugin `_pid` riding each Identity) → the
/// transcript-family point queries (CC registry / Codex fd probe, both
/// recycle-guarded) → None. Two click-time recycle guards on the cached path:
/// an EXITING slot refuses outright (its process is going or gone), and a
/// cached start marker must match the kernel's CURRENT marker for that pid
/// (#527) — a mismatch means the pid was recycled by an unrelated process
/// after an abrupt death, and a missing current read means the process is
/// gone. A cache stamped WITHOUT a marker (non-unix daemon) skips the
/// identity check — additive, the #220 posture.
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
    // The registry's FocusChannel capability decides WHETHER a probe applies;
    // the probe FNS themselves stay here in the binary (deliberate: the
    // registry const table compiles to wasm, a native-only fn pointer can't
    // live in it). The name match uses the registry consts, not literals — a
    // source rename must not silently drop an arm to `_ => None` — and the
    // `transcript_probe_sources_all_have_a_resolve_arm` lockstep test pins
    // that every `TranscriptProbe` row has an arm below.
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

/// The painter-agnostic focus dispatch — ANY painter's trigger (the TUI's
/// sprite click and dashboard `f` today; the floating window's future
/// trigger) resolves + walks + activates through this ONE entry with the
/// real OS table. `roots` = (CC projects root, Codex sessions root), the
/// clone `runtime/driver.rs` takes BEFORE `build_source_set` consumes the
/// originals.
pub(crate) fn focus_slot(
    slot: &AgentSlot,
    roots: &(Option<std::path::PathBuf>, Option<std::path::PathBuf>),
) {
    // grok's root has no CLI override (the tuple exists for --projects-root /
    // --codex-sessions-root), so it resolves here from the one `grok_home()`
    // authority instead of threading a third always-Some element through.
    let grok_home = pixtuoid_core::source::grok::grok_home();
    let paths = FocusPaths {
        cc_projects_root: roots.0.as_deref(),
        codex_sessions_root: roots.1.as_deref(),
        grok_root: Some(&grok_home),
    };
    focus_agent(slot, &paths, &OsProcessTable, activate_os);
}

/// The orchestration entry — the ONE caller of the per-OS glue. `activate`
/// is injected (the `headless_loop` ctrl_c seam precedent) so dispatch tests
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
/// The real per-OS process table.
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
        /// pid → current kernel start marker; empty = "every pid reads None"
        /// (gone), which the default-marker tests below never hit because a
        /// markerless cache skips the check.
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
        // Corrupt/racing table: 300 → 200 → 300 → … must return None, not loop.
        let t = MockTable {
            parents: HashMap::from([(300, 200), (200, 300)]),
            focusable: vec![],
            started: HashMap::new(),
        };
        assert_eq!(ancestor_walk(&t, 300), None);
        // And the degenerate self-parent.
        let t2 = MockTable {
            parents: HashMap::from([(300, 300)]),
            focusable: vec![],
            started: HashMap::new(),
        };
        assert_eq!(ancestor_walk(&t2, 300), None);
    }

    #[test]
    fn walk_of_a_dead_pid_is_a_silent_miss() {
        // A dead/recycled-away pid: the table knows nothing about it (the real
        // per-OS reads fail → None/false), so the walk no-ops without any
        // extra liveness check — the documented dead-pid posture.
        let t = MockTable {
            parents: HashMap::new(),
            focusable: vec![],
            started: HashMap::new(),
        };
        assert_eq!(ancestor_walk(&t, 4242), None);
    }

    #[test]
    fn walk_start_itself_can_be_the_focusable_app() {
        // Alacritty-style: one window per process — the agent's own ancestor
        // chain starts at a focusable pid immediately.
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
        // Markerless cache (stamped where no marker was readable): the
        // identity check is skipped — additive, the #220 posture.
        let s = slot("opencode", "ses_a", Some(pid_id(4242, None)));
        assert_eq!(resolve_pid(&s, &NO_PATHS, &empty_table()), Some(4242));
    }

    #[test]
    fn resolve_pid_verifies_the_start_marker_when_stamped() {
        let s = slot("opencode", "ses_a", Some(pid_id(4242, Some(1_000))));
        // Same incarnation: current marker matches the stamp.
        let mut t = empty_table();
        t.started.insert(4242, 1_000);
        assert_eq!(resolve_pid(&s, &NO_PATHS, &t), Some(4242));
    }

    #[test]
    fn resolve_pid_refuses_a_recycled_or_dead_pid() {
        // #527: an abruptly-dead agent's pid got recycled — the kernel's
        // CURRENT start marker differs from the stamped one → refuse.
        let s = slot("opencode", "ses_a", Some(pid_id(4242, Some(1_000))));
        let mut t = empty_table();
        t.started.insert(4242, 2_000);
        assert_eq!(resolve_pid(&s, &NO_PATHS, &t), None, "recycled → refuse");
        // Process gone entirely (marker unreadable) → refuse too.
        assert_eq!(
            resolve_pid(&s, &NO_PATHS, &empty_table()),
            None,
            "dead → refuse"
        );
    }

    #[test]
    fn resolve_pid_refuses_an_exiting_slot() {
        // The first click-time guard: an exiting agent's process is going or
        // gone — a recycled pid would focus a random app.
        let mut s = slot("opencode", "ses_a", Some(pid_id(4242, None)));
        s.exiting_at = Some(std::time::SystemTime::UNIX_EPOCH);
        assert_eq!(resolve_pid(&s, &NO_PATHS, &empty_table()), None);
    }

    #[test]
    fn resolve_pid_misses_when_no_channel_exists() {
        // Hook-family slot without a cached pid and no probe roots → None
        // (the ONE failure rule: silent).
        let s = slot("cursor", "ses_b", None);
        assert_eq!(resolve_pid(&s, &NO_PATHS, &empty_table()), None);
        // Unknown/remote source likewise.
        let r = slot("some-remote", "ses_c", None);
        assert_eq!(resolve_pid(&r, &NO_PATHS, &empty_table()), None);
        // FocusChannel::Unsupported (transcript-only, no probe) likewise.
        let a = slot("antigravity", "ses_d", None);
        assert_eq!(resolve_pid(&a, &NO_PATHS, &empty_table()), None);
    }

    #[test]
    fn transcript_probe_sources_all_have_a_resolve_arm() {
        // The registry's FocusChannel is DATA; the probe fns live here in the
        // binary. This is the lockstep pin: marking a new source
        // `TranscriptProbe` in the registry REQUIRES wiring a probe arm in
        // `resolve_pid` — extend BOTH, then add its name here.
        use pixtuoid_core::source::registry::FocusChannel;
        let wired = [
            pixtuoid_core::source::claude_code::SOURCE_NAME,
            pixtuoid_core::source::codex::SOURCE_NAME,
            pixtuoid_core::source::grok::SOURCE_NAME,
        ];
        for &src in pixtuoid_core::source::REGISTERED_SOURCES {
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

        // No pid → the activate seam is never reached.
        let mut called = false;
        focus_agent(&slot("cursor", "s", None), &NO_PATHS, &t, |_| {
            called = true;
            true
        });
        assert!(!called, "silent no-op without a pid");
    }

    #[test]
    fn focus_agent_no_focusable_ancestor_never_activates() {
        // resolve yields a pid but the walk finds NO focusable ancestor → the fn
        // must return without activating. Teeth: a regression that activates the
        // raw resolved pid (the agent's own process) instead of a walked terminal-
        // app ancestor would call `activate` here.
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

/// Live dogfood (manual, `cargo test -p pixtuoid --lib focus -- --ignored`):
/// walks THIS test process's own ancestor chain with the real OS table and
/// activates the found terminal app — the exact path a sprite click runs.
/// Ignored in CI: needs a real GUI session, and it yanks a window forward.
#[cfg(all(test, target_os = "macos"))]
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
            "macOS accepted the activation for pid {app}"
        );
    }
}
