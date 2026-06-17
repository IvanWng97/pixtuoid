//! Runtime wiring: `RunConfig` (the startup inputs), the boot-capacity math,
//! and the headless summary formatter — everything here is exercised by unit
//! tests. The untestable async glue (tokio runtime, reducer task, source
//! spawn, Ctrl-C loop) lives in `driver.rs`, which is excluded from coverage
//! (issue #103).

pub(crate) mod driver;

pub use driver::run;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pixtuoid_core::source::manager::SourceDeath;
use pixtuoid_core::state::{ActivityState, DaemonState, MAX_FLOORS};
use pixtuoid_core::SceneState;
use tokio::sync::watch;

/// The reducer publishes a fresh `Arc<SceneState>` on every mutation through
/// this watch channel. Consumers (renderer, headless summary loop) hold a
/// `Receiver`, call `borrow()` for an O(1) pointer read, and never block
/// the writer. Replaces the old `Arc<RwLock<SceneState>>` so:
///   - cloning is a pointer copy (Arc::clone), not a heap allocation per
///     field (thanks to interned `Arc<str>` strings in `AgentSlot`)
///   - the renderer never holds a lock that could block the reducer
///   - the Arc is serializable, so an out-of-process viewer could read it
///     over a socket (no such consumer today)
pub type SceneRx = watch::Receiver<Arc<SceneState>>;

/// Fallback desk capacity when the terminal cannot be queried (e.g.
/// headless mode). The real capacity is computed from terminal size in
/// `compute_boot_capacities` before the first TUI frame.
const FALLBACK_DESKS: usize = 16;

/// The startup inputs shared by `run` + `run_async`. Bundled so a new boot
/// flag is one struct field, not a fourth copy of the arg list to thread
/// through both signatures + the main.rs call. The `theme` is already resolved
/// (`config::resolve_theme` validates CLI + config in one place), so an
/// unknown theme can't reach the runtime by construction.
pub struct RunConfig {
    pub socket: Option<PathBuf>,
    pub projects_root: Option<PathBuf>,
    pub codex_sessions_root: Option<PathBuf>,
    pub pack_dir: Option<PathBuf>,
    pub desk_cap: Option<usize>,
    pub headless: bool,
    pub config_path: PathBuf,
    pub theme: &'static pixtuoid_scene::theme::Theme,
    pub pets: Vec<pixtuoid_scene::pet::Pet>,
    /// The resolved set of CONNECTED source ids (registry names). Seeded at boot
    /// from `config::resolve_connected`; the runtime wraps it in a shared
    /// [`ConnectedSources`] the reducer gate reads and the Sources panel
    /// mutates. A disconnected source's events are dropped + its sprites evicted.
    pub connected: HashSet<String>,
    /// The warn-floor log path (`main` owns the resolution). `run_tui`
    /// throttle-scans it for decode-drift breadcrumbs → the footer nudge. `None`
    /// in headless / when no log file (then no footer drift surfacing).
    pub log_path: Option<PathBuf>,
}

/// A live, shared set of connected source ids — the runtime mirror of the
/// persisted `[sources]` flags. One writer (the Sources panel toggle), many
/// readers (the reducer-task event gate + its per-tick reconcile sweep). On lock
/// poison it recovers the set via `into_inner` (insert/remove/contains never
/// panic, so the data is always valid — losing it would mass-evict the office).
#[derive(Clone, Default)]
pub struct ConnectedSources(Arc<Mutex<HashSet<String>>>);

impl ConnectedSources {
    pub fn new(initial: HashSet<String>) -> Self {
        Self(Arc::new(Mutex::new(initial)))
    }
    fn guard(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn is_connected(&self, source_id: &str) -> bool {
        self.guard().contains(source_id)
    }
    pub fn snapshot(&self) -> HashSet<String> {
        self.guard().clone()
    }
    pub fn set(&self, source_id: &str, connected: bool) {
        let mut g = self.guard();
        if connected {
            g.insert(source_id.to_string());
        } else {
            g.remove(source_id);
        }
    }
}

/// Per-floor boot capacities derived from the real terminal size. Each floor
/// uses its own seed, so different layout variants can yield different desk
/// counts. When a floor's layout rejects the terminal (e.g. too small), fall
/// back to `FALLBACK_DESKS` for that floor so the reducer can still seat
/// agents — they may render off-grid on the tiny terminal, but won't be
/// silently dropped during the boot race before the first TUI frame.
pub(crate) fn boot_capacities_for(cols: u16, rows: u16) -> [usize; MAX_FLOORS] {
    std::array::from_fn(|i| {
        let seed = (i as u64).wrapping_mul(pixtuoid_scene::floor::FLOOR_SEED_MULTIPLIER);
        let cap = capacity_for_terminal(cols, rows, seed);
        if cap == 0 {
            FALLBACK_DESKS
        } else {
            cap
        }
    })
}

/// Clamp each per-floor boot capacity to an optional `--max-desks` cap. Returns
/// `min(layout_capacity, cap)` per floor so the boot atomics are never seeded
/// above the real layout capacity (`fetch_max` only grows; an over-seed strands
/// agents on non-existent desks until the terminal grows). `None` is a no-op.
fn cap_boot_capacities(base: [usize; MAX_FLOORS], cap: Option<usize>) -> [usize; MAX_FLOORS] {
    match cap {
        Some(c) => base.map(|x| x.min(c)),
        None => base,
    }
}

pub(crate) fn capacity_for_terminal(cols: u16, rows: u16, floor_seed: u64) -> usize {
    // The footer eats one terminal row, and a half-block ▀ cell is 2 pixels
    // tall — so the pixel-buffer height is (rows-1)*2.
    let buf_h = rows.saturating_sub(1) * 2;
    pixtuoid_scene::floor::floor_capacity(cols, buf_h, floor_seed)
}

// The headless `println!` summary derives labels / tool detail / Notification
// reason from untrusted transcript+hook input, so a crafted ANSI/OSC escape
// would otherwise reach the user's terminal verbatim (the TUI is immune —
// ratatui neutralizes escapes in its cell buffer). One chokepoint: the
// canonical `crate::strip_control_chars`.
use crate::strip_control_chars as sanitize_line;

fn summarize(scene: &SceneState) -> String {
    let agents: Vec<String> = scene
        .agents
        .values()
        .map(|a| {
            let state = match &a.state {
                ActivityState::Idle => "idle".to_string(),
                ActivityState::Active { detail, .. } => {
                    format!(
                        "active({})",
                        sanitize_line(detail.as_deref().unwrap_or("?"))
                    )
                }
                ActivityState::Waiting { reason } => {
                    format!("waiting({})", sanitize_line(reason))
                }
            };
            format!("{}@{}:{}", sanitize_line(&a.label), a.desk_index.0, state)
        })
        .collect();
    // Daemon-style sources (the OpenClaw gateway lobster) render as wandering
    // mascots, not desk agents — surface them here too so headless is a complete
    // window onto the scene (and the live-e2e harness can assert the lobster's state).
    // Source name is a registry id (controlled), but sanitize for defense like
    // every other field on this println path (R0609-02).
    let daemons: Vec<String> = scene
        .daemons()
        .iter()
        .map(|(source, p)| {
            let state = match p.state {
                DaemonState::Idle => "idle",
                DaemonState::Busy => "busy",
                DaemonState::Degraded => "degraded",
                DaemonState::Down => "down",
            };
            format!("{}:{}", sanitize_line(source), state)
        })
        .collect();
    format!(
        "agents=[{}] daemons=[{}]",
        agents.join(", "),
        daemons.join(", ")
    )
}

/// Format a `SourceDeath` for the headless stdout health line. Both fields are
/// `sanitize_line`d before printing: `error` is `format!("{e:#}")` of an
/// `anyhow` chain that can embed external strings (a malformed transcript path,
/// a parse error quoting file content) carrying terminal escapes, and `source`
/// is sanitized too for defense-in-depth — the same escape-injection class the
/// `summarize` path already guards (R0609-02).
fn format_source_death(d: &SourceDeath) -> String {
    format!(
        "warning: source '{}' died: {}",
        sanitize_line(&d.source),
        sanitize_line(&d.error)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::{Reducer, Transport};
    use std::time::SystemTime;

    fn floor_seed(i: u64) -> u64 {
        i.wrapping_mul(pixtuoid_scene::floor::FLOOR_SEED_MULTIPLIER)
    }

    #[test]
    fn format_source_death_strips_terminal_escapes_from_both_fields() {
        // `error` is the untrusted vector (anyhow `{e:#}` can quote a malformed
        // transcript path / file content); `source` is sanitized too for
        // defense-in-depth. Headless prints this straight to a terminal.
        let d = SourceDeath::new(
            "codex\u{1b}]0;pwned\u{7}",
            "open /tmp/a\u{1b}[2Jb.jsonl: \u{1b}[31mboom\u{7}",
        );
        let out = format_source_death(&d);
        assert!(
            !out.chars().any(|c| c.is_control()),
            "no control chars may survive into the headless terminal line: {out:?}"
        );
        // The human-readable text is preserved (only control chars stripped).
        assert!(out.contains("source 'codex]0;pwned' died"), "got {out:?}");
        assert!(
            out.contains("open /tmp/a[2Jb.jsonl: [31mboom"),
            "got {out:?}"
        );
    }

    #[test]
    fn capacity_for_normal_terminal() {
        let cap = capacity_for_terminal(192, 48, 0);
        assert!(cap > 0 && cap <= pixtuoid_core::layout::MAX_VISIBLE_DESKS);
    }

    #[test]
    fn capacity_for_small_terminal() {
        let cap = capacity_for_terminal(80, 35, 0);
        assert!(cap > 0, "80x35 should fit at least one desk");
    }

    #[test]
    fn capacity_for_tiny_terminal_returns_zero() {
        assert_eq!(capacity_for_terminal(10, 10, 0), 0);
    }

    #[test]
    fn capacity_for_zero_rows_returns_zero() {
        assert_eq!(capacity_for_terminal(192, 0, 0), 0);
    }

    #[test]
    fn capacity_matches_renderer_formula() {
        let cols: u16 = 160;
        let rows: u16 = 50;
        let buf_h = rows.saturating_sub(1) * 2;
        let expected = pixtuoid_core::layout::SceneLayout::compute_with_seed(
            cols,
            buf_h,
            pixtuoid_core::layout::MAX_VISIBLE_DESKS,
            0,
        )
        .map(|l| l.home_desks.len())
        .unwrap_or(0);
        assert_eq!(capacity_for_terminal(cols, rows, 0), expected);
    }

    // Regression for the pre-0.4.1 bug where boot capacity used floor-0's seed
    // for all floors. Different seeds select different layout variants (mid_x
    // splits {28%, 18%, 22%, 35%, 22%}) which can yield different desk counts
    // at the same terminal size; capacity_for_terminal must respect the seed.
    #[test]
    fn seed_can_produce_distinct_capacities() {
        let mut found = false;
        'outer: for cols in [120u16, 140, 160, 180, 200, 220, 240] {
            for rows in [30u16, 36, 40, 48, 56, 64] {
                let mut unique = std::collections::HashSet::new();
                for i in 0..MAX_FLOORS as u64 {
                    unique.insert(capacity_for_terminal(cols, rows, floor_seed(i)));
                }
                if unique.len() > 1 {
                    found = true;
                    break 'outer;
                }
            }
        }
        assert!(
            found,
            "expected at least one terminal size in the swept range where \
             per-floor seeds produce distinct capacities"
        );
    }

    #[test]
    fn boot_capacities_uses_each_floor_seed() {
        let caps = boot_capacities_for(192, 48);
        let expected: [usize; MAX_FLOORS] = std::array::from_fn(|i| {
            let c = capacity_for_terminal(192, 48, floor_seed(i as u64));
            if c == 0 {
                FALLBACK_DESKS
            } else {
                c
            }
        });
        assert_eq!(caps, expected);
    }

    // Regression for the boot-race window where SessionStart events fired
    // between SourceManager spawn and the first TUI frame's fetch_max were
    // silently dropped because boot=0 left every floor capacity at zero.
    #[test]
    fn boot_capacities_falls_back_to_default_on_tiny_terminal() {
        let caps = boot_capacities_for(10, 10);
        assert_eq!(caps, [FALLBACK_DESKS; MAX_FLOORS]);
    }

    // Regression: an explicit --max-desks must CLAMP each floor to the real
    // layout capacity, never seed a floor ABOVE it. The boot atomics grow via
    // fetch_max only, so an over-seed (the old `[cap; MAX_FLOORS]` path) strands
    // agents on non-existent desks on small terminals until the terminal grows.
    #[test]
    fn summarize_reports_each_activity_state() {
        use pixtuoid_core::source::AgentEvent;
        use pixtuoid_core::AgentId;

        let mut scene = SceneState::new([8; MAX_FLOORS]);
        let mut reducer = Reducer::new();
        let now = SystemTime::now();

        let seat = |reducer: &mut Reducer, scene: &mut SceneState, id: AgentId| {
            reducer.apply(
                scene,
                AgentEvent::SessionStart {
                    agent_id: id,
                    source: "claude-code".into(),
                    session_id: "s".into(),
                    cwd: std::path::PathBuf::from("/repo"),
                    parent_id: None,
                },
                now,
                Transport::Hook,
            );
        };

        // Agent A: active with a detail.
        let a = AgentId::from_transcript_path("/p/a.jsonl");
        seat(&mut reducer, &mut scene, a);
        reducer.apply(
            &mut scene,
            AgentEvent::ActivityStart {
                agent_id: a,
                tool_use_id: Some("t1".into()),
                detail: Some("Edit: foo.rs".into()),
            },
            now,
            Transport::Hook,
        );

        // Agent B: waiting on a permission prompt.
        let b = AgentId::from_transcript_path("/p/b.jsonl");
        seat(&mut reducer, &mut scene, b);
        reducer.apply(
            &mut scene,
            AgentEvent::Waiting {
                agent_id: b,
                reason: "permission".into(),
            },
            now,
            Transport::Hook,
        );

        // Agent C: bare SessionStart → idle.
        let c = AgentId::from_transcript_path("/p/c.jsonl");
        seat(&mut reducer, &mut scene, c);

        let summary = summarize(&scene);
        assert!(summary.starts_with("agents=["), "got: {summary}");
        assert!(summary.contains("active(Edit: foo.rs)"), "got: {summary}");
        assert!(summary.contains("waiting(permission)"), "got: {summary}");
        assert!(summary.contains(":idle"), "got: {summary}");
        // The "@desk_index" format is present for each agent.
        assert!(summary.contains('@'), "got: {summary}");
    }

    // Headless must surface the DAEMON layer too (the OpenClaw gateway lobster),
    // not just agents — in headless it is the ONLY programmatic window onto a
    // daemon's presence. Format is `<source>:<idle|busy|down>`, source-keyed so N
    // daemons each get an entry. (This is also what the live-e2e harness asserts.)
    #[test]
    fn summarize_reports_daemon_presence() {
        use pixtuoid_core::source::daemon::{apply_presence, DaemonPresenceUpdate};

        let mut scene = SceneState::new([8; MAX_FLOORS]);
        let now = SystemTime::now();

        // No daemon configured → an empty (but present) daemons section.
        assert!(
            summarize(&scene).contains("daemons=[]"),
            "got: {}",
            summarize(&scene)
        );

        // gateway_start → idle.
        apply_presence(
            &mut scene,
            "openclaw",
            DaemonPresenceUpdate::GatewayUp { pid: Some(4242) },
            now,
        );
        assert!(
            summarize(&scene).contains("daemons=[openclaw:idle]"),
            "got: {}",
            summarize(&scene)
        );

        // a run in flight → busy.
        apply_presence(
            &mut scene,
            "openclaw",
            DaemonPresenceUpdate::RunStarted {
                run_key: "r".into(),
            },
            now,
        );
        assert!(
            summarize(&scene).contains("daemons=[openclaw:busy]"),
            "got: {}",
            summarize(&scene)
        );

        // a FAILED run (agent_end.success:false) → degraded (#317). Drains the
        // in-flight run, so the subsequent GatewayDown still reads as down.
        apply_presence(
            &mut scene,
            "openclaw",
            DaemonPresenceUpdate::RunFailed {
                run_key: "r".into(),
            },
            now,
        );
        assert!(
            summarize(&scene).contains("daemons=[openclaw:degraded]"),
            "got: {}",
            summarize(&scene)
        );

        // gateway_stop → down.
        apply_presence(
            &mut scene,
            "openclaw",
            DaemonPresenceUpdate::GatewayDown,
            now,
        );
        assert!(
            summarize(&scene).contains("daemons=[openclaw:down]"),
            "got: {}",
            summarize(&scene)
        );
    }

    // Headless `summarize` feeds `println!` directly. The label (cwd basename),
    // tool detail, and Notification reason are all untrusted, so a crafted
    // ANSI/OSC escape must be stripped before it reaches the user's terminal.
    #[test]
    fn summarize_strips_terminal_escapes_from_untrusted_fields() {
        use pixtuoid_core::source::AgentEvent;
        use pixtuoid_core::AgentId;

        let mut scene = SceneState::new([8; MAX_FLOORS]);
        let mut reducer = Reducer::new();
        let now = SystemTime::now();
        let id = AgentId::from_parts("cc", "esc");
        // The label derives from the cwd basename — an attacker-controlled path
        // can smuggle an OSC set-title + BEL sequence.
        reducer.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: id,
                source: "claude-code".into(),
                session_id: "s".into(),
                cwd: std::path::PathBuf::from("/repo\u{1b}]0;pwned\u{7}"),
                parent_id: None,
            },
            now,
            Transport::Hook,
        );
        // The Notification reason is wholly untrusted and length-uncapped.
        reducer.apply(
            &mut scene,
            AgentEvent::Waiting {
                agent_id: id,
                reason: "needs \u{1b}[2J approval".to_string(),
            },
            now,
            Transport::Hook,
        );

        let out = summarize(&scene);
        assert!(
            !out.chars().any(|c| c.is_control()),
            "summary must carry no control chars (terminal-escape injection): {out:?}"
        );
        // The benign text survives the scrub.
        assert!(out.contains("repo]0;pwned"), "got: {out}");
        assert!(out.contains("needs [2J approval"), "got: {out}");
    }

    #[test]
    fn explicit_cap_clamps_to_layout_capacity_not_above() {
        let base = boot_capacities_for(192, 48);
        let layout_max = *base.iter().max().unwrap();
        // A cap far above the layout must NOT inflate any floor.
        assert_eq!(
            cap_boot_capacities(base, Some(layout_max + 100)),
            base,
            "cap above layout capacity must clamp down to the layout, not inflate"
        );
        // A cap of 1 clamps every floor to at most 1.
        assert!(cap_boot_capacities(base, Some(1)).iter().all(|&c| c <= 1));
        // No cap leaves the base untouched.
        assert_eq!(cap_boot_capacities(base, None), base);
    }

    #[test]
    fn connected_sources_set_get_snapshot() {
        let cs = ConnectedSources::new(HashSet::from(["claude-code".to_string()]));
        assert!(cs.is_connected("claude-code"));
        assert!(!cs.is_connected("codex"));
        cs.set("codex", true);
        assert!(cs.is_connected("codex"));
        cs.set("claude-code", false);
        assert!(!cs.is_connected("claude-code"));
        assert_eq!(cs.snapshot(), HashSet::from(["codex".to_string()]));
    }
}
