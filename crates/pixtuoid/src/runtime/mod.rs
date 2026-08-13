//! Runtime wiring: `RunConfig` (the startup inputs), the boot-capacity math, and the
//! headless summary formatter. The untestable async glue (tokio runtime, reducer task,
//! source spawn, Ctrl-C loop) lives in `driver.rs`, which is excluded from coverage.

pub(crate) mod driver;
pub(crate) mod gate;
pub(crate) mod pipeline;

pub use driver::run;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pixtuoid_core::source::manager::SourceDeath;
use pixtuoid_core::state::{ActivityState, DaemonState, MAX_FLOORS};
use pixtuoid_core::SceneState;
use tokio::sync::watch;

/// The reducer publishes a fresh `Arc<SceneState>` on every mutation through this watch
/// channel. Consumers (renderer, headless summary loop) hold a `Receiver`, call
/// `borrow()` for an O(1) pointer read, and never block the writer.
pub type SceneRx = watch::Receiver<Arc<SceneState>>;

/// Fallback desk capacity when the terminal cannot be queried (e.g. headless mode).
pub(crate) const FALLBACK_DESKS: usize = 16;

/// The startup inputs shared by `run` + `run_async`. The `theme` is already resolved
/// (`config::resolve_theme` validates CLI + config in one place), so an unknown theme
/// can't reach the runtime by construction.
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
    /// The resolved set of CONNECTED source ids (registry names). A disconnected
    /// source's events are dropped + its sprites evicted.
    pub connected: HashSet<String>,
    /// The warn-floor log path. `run_tui` throttle-scans it for decode-drift
    /// breadcrumbs → the footer nudge. `None` in headless / when no log file.
    pub log_path: Option<PathBuf>,
    /// First launch ever (no `[sources]` flags persisted yet) — the TUI plays the
    /// one-time onboarding "move-in" overlay. Ignored by headless + `floating`.
    pub first_run: bool,
    /// Resolved `[audio]` settings — muted defaults TRUE (the lazy spawn waits for the
    /// first `m`), volume pre-clamped by `config::resolve_audio`. Headless ignores it.
    pub audio: crate::config::AudioConfig,
}

/// A live, shared set of connected source ids — the runtime mirror of the persisted
/// `[sources]` flags. On lock poison it recovers the set via `into_inner`: the data is
/// always valid (insert/remove/contains never panic), and losing it would mass-evict
/// the office.
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

/// Per-floor boot capacities derived from the real terminal size, each floor with its
/// own seed. When a floor's layout rejects the terminal (e.g. too small), fall back to
/// `FALLBACK_DESKS` for that floor so the reducer can still seat agents — they may
/// render off-grid on the tiny terminal, but won't be silently dropped during the boot
/// race before the first TUI frame.
pub(crate) fn boot_capacities_for(cols: u16, rows: u16) -> [usize; MAX_FLOORS] {
    std::array::from_fn(|i| {
        // The ONE seed derivation every call site shares — an inline copy of the
        // formula would silently drift the boot capacities from the rendered layout
        // (over-seeded atomics strand agents on unrendered desks).
        let seed = pixtuoid_scene::floor::floor_seed(i);
        let cap = capacity_for_terminal(cols, rows, seed);
        if cap == 0 {
            FALLBACK_DESKS
        } else {
            cap
        }
    })
}

/// Clamp each per-floor boot capacity to an optional `--max-desks` cap, so the boot
/// atomics are never seeded above the real layout capacity (`fetch_max` only grows; an
/// over-seed strands agents on non-existent desks until the terminal grows).
fn cap_boot_capacities(base: [usize; MAX_FLOORS], cap: Option<usize>) -> [usize; MAX_FLOORS] {
    match cap {
        Some(c) => base.map(|x| x.min(c)),
        None => base,
    }
}

/// The headless-vs-interactive boot capacity POLICY. Headless honors `--max-desks`
/// UNCLAMPED (there is no terminal layout to measure) and never calls `measure`;
/// interactive measures the real per-floor layout (injected so the terminal query stays
/// in the shell) then clamps to the cap — clamping, not `[cap; N]`, keeps the
/// `fetch_max` boot atomics from over-seeding agents onto non-existent desks.
pub(crate) fn resolve_boot_caps(
    desk_cap: Option<usize>,
    headless: bool,
    measure: impl FnOnce() -> [usize; MAX_FLOORS],
) -> [usize; MAX_FLOORS] {
    match (desk_cap, headless) {
        (Some(cap), true) => [cap; MAX_FLOORS],
        (None, true) => [FALLBACK_DESKS; MAX_FLOORS],
        (cap, false) => cap_boot_capacities(measure(), cap),
    }
}

pub(crate) fn capacity_for_terminal(cols: u16, rows: u16, floor_seed: u64) -> usize {
    // The INVERSE of `renderer::min_terminal_size`, so it reads the same footer
    // reserve: a half-block ▀ cell is 2 pixels tall.
    let buf_h = rows.saturating_sub(crate::tui::renderer::FOOTER_ROWS) * 2;
    pixtuoid_scene::floor::floor_capacity(cols, buf_h, floor_seed)
}

// The headless `println!` summary derives labels / tool detail / Notification reason
// from untrusted transcript+hook input, so a crafted ANSI/OSC escape would otherwise
// reach the user's terminal verbatim (the TUI is immune — ratatui neutralizes escapes
// in its cell buffer).
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
    // Daemon-style sources (the OpenClaw gateway lobster) render as wandering mascots,
    // not desk agents — surface them here too so headless is a complete window onto the
    // scene. The source name is a registry id (controlled), but sanitize it like every
    // other field on this println path.
    let daemons: Vec<String> = scene
        .daemons()
        .map(|(source, instance, p)| {
            let state = match p.display_state() {
                DaemonState::Idle => "idle",
                DaemonState::Busy => "busy",
                DaemonState::Degraded => "degraded",
                DaemonState::Down => "down",
            };
            // The instance (an OpenClaw gateway port) is load-bearing, not cosmetic:
            // two gateways are two rows, so the live-e2e can assert one going down
            // leaves the other alone.
            format!(
                "{}@{}:{}",
                sanitize_line(source),
                sanitize_line(instance.as_str()),
                state
            )
        })
        .collect();
    format!(
        "agents=[{}] daemons=[{}]",
        agents.join(", "),
        daemons.join(", ")
    )
}

/// Format a `SourceDeath` for the headless stdout health line. Both fields are
/// `sanitize_line`d before printing: `error` is `format!("{e:#}")` of an `anyhow` chain
/// that can embed external strings carrying terminal escapes, and `source` is sanitized
/// too for defense-in-depth.
fn format_source_death(d: &SourceDeath) -> String {
    format!(
        "warning: source '{}' died: {}",
        sanitize_line(&d.source),
        sanitize_line(&d.error)
    )
}

/// The not-yet-surfaced tail of the grow-only `SourceDeath` health watch, advancing
/// `seen` past it. Each consumer surfaces a death exactly once by tracking a running
/// count — logging the whole borrow on every change would re-warn all prior deaths,
/// reading as repeated crashes in forensics.
pub(crate) fn unseen_deaths<'a>(deaths: &'a [SourceDeath], seen: &mut usize) -> &'a [SourceDeath] {
    let start = (*seen).min(deaths.len());
    *seen = deaths.len();
    &deaths[start..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::{Reducer, Transport};
    use std::time::SystemTime;

    // The shared derivation, NOT a copy: a test-local restatement structurally
    // couldn't catch the impl diverging from `floor_seed`.
    fn floor_seed(i: usize) -> u64 {
        pixtuoid_scene::floor::floor_seed(i)
    }

    #[test]
    fn format_source_death_strips_terminal_escapes_from_both_fields() {
        let d = SourceDeath::new(
            "codex\u{1b}]0;pwned\u{7}",
            "open /tmp/a\u{1b}[2Jb.jsonl: \u{1b}[31mboom\u{7}",
        );
        let out = format_source_death(&d);
        assert!(
            !out.chars().any(|c| c.is_control()),
            "no control chars may survive into the headless terminal line: {out:?}"
        );
        assert!(out.contains("source 'codex]0;pwned' died"), "got {out:?}");
        assert!(
            out.contains("open /tmp/a[2Jb.jsonl: [31mboom"),
            "got {out:?}"
        );
    }

    #[test]
    fn unseen_deaths_yields_each_death_exactly_once() {
        let mut seen = 0usize;
        let one = vec![SourceDeath::new("codex", "boom")];
        assert_eq!(unseen_deaths(&one, &mut seen).len(), 1);
        assert_eq!(seen, 1);

        let two = vec![
            SourceDeath::new("codex", "boom"),
            SourceDeath::new("claude-code", "bind"),
        ];
        let fresh = unseen_deaths(&two, &mut seen);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].source, "claude-code");

        assert!(unseen_deaths(&two, &mut seen).is_empty());
    }

    #[test]
    fn capacity_for_normal_terminal() {
        let cap = capacity_for_terminal(192, 48, 0);
        assert!(cap > 0);
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

    /// The notice advertises `min_terminal_size()`; boot capacity computes the buffer
    /// back from rows. If the two ever disagree on the footer reserve, the advertised
    /// size seeds zero desks — the #803 silent over/under-seed class, other direction.
    #[test]
    fn the_advertised_minimum_seats_someone_through_the_boot_capacity_path() {
        let (cols, rows) = crate::tui::renderer::min_terminal_size();
        assert!(
            capacity_for_terminal(cols, rows, pixtuoid_scene::floor::floor_seed(0)) > 0,
            "the advertised minimum {cols}x{rows} seeds no desks through capacity_for_terminal"
        );
    }

    #[test]
    fn capacity_matches_renderer_formula() {
        let cols: u16 = 160;
        let rows: u16 = 50;
        let buf_h = rows.saturating_sub(crate::tui::renderer::FOOTER_ROWS) * 2;
        let expected = pixtuoid_scene::layout::SceneLayout::compute_with_seed(cols, buf_h, None, 0)
            .map(|l| l.home_desks.len())
            .unwrap_or(0);
        assert_eq!(capacity_for_terminal(cols, rows, 0), expected);
    }

    #[test]
    fn seed_can_produce_distinct_capacities() {
        let mut found = false;
        'outer: for cols in [120u16, 140, 160, 180, 200, 220, 240] {
            for rows in [30u16, 36, 40, 48, 56, 64] {
                let mut unique = std::collections::HashSet::new();
                for i in 0..MAX_FLOORS {
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
    fn resolve_boot_caps_headless_honors_cap_unclamped_interactive_clamps() {
        assert_eq!(
            resolve_boot_caps(Some(99), true, || panic!("headless must not measure")),
            [99; MAX_FLOORS]
        );
        assert_eq!(
            resolve_boot_caps(None, true, || panic!("headless must not measure")),
            [FALLBACK_DESKS; MAX_FLOORS]
        );
        assert_eq!(
            resolve_boot_caps(Some(4), false, || [10; MAX_FLOORS]),
            [4; MAX_FLOORS]
        );
        assert_eq!(
            resolve_boot_caps(None, false, || [10; MAX_FLOORS]),
            [10; MAX_FLOORS]
        );
    }

    #[test]
    fn boot_capacities_uses_each_floor_seed() {
        let caps = boot_capacities_for(192, 48);
        let expected: [usize; MAX_FLOORS] = std::array::from_fn(|i| {
            let c = capacity_for_terminal(192, 48, floor_seed(i));
            if c == 0 {
                FALLBACK_DESKS
            } else {
                c
            }
        });
        assert_eq!(caps, expected);
    }

    #[test]
    fn boot_capacities_falls_back_to_default_on_tiny_terminal() {
        let caps = boot_capacities_for(10, 10);
        assert_eq!(caps, [FALLBACK_DESKS; MAX_FLOORS]);
    }

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

        let c = AgentId::from_transcript_path("/p/c.jsonl");
        seat(&mut reducer, &mut scene, c);

        let summary = summarize(&scene);
        assert!(summary.starts_with("agents=["), "got: {summary}");
        assert!(summary.contains("active(Edit: foo.rs)"), "got: {summary}");
        assert!(summary.contains("waiting(permission)"), "got: {summary}");
        assert!(summary.contains(":idle"), "got: {summary}");
        assert!(summary.contains('@'), "got: {summary}");
    }

    #[test]
    fn summarize_reports_daemon_presence() {
        use pixtuoid_core::source::daemon::{
            apply_presence, DaemonInstanceKey, DaemonPresenceUpdate,
        };
        use pixtuoid_core::state::DaemonInstanceId;

        let mut scene = SceneState::new([8; MAX_FLOORS]);
        let now = SystemTime::now();
        let gw = DaemonInstanceKey::new(
            "openclaw",
            DaemonInstanceId::new("18789").expect("non-empty"),
        );

        assert!(
            summarize(&scene).contains("daemons=[]"),
            "got: {}",
            summarize(&scene)
        );

        apply_presence(
            &mut scene,
            &gw,
            DaemonPresenceUpdate::GatewayUp { pid: Some(4242) },
            now,
        );
        assert!(
            summarize(&scene).contains("daemons=[openclaw@18789:idle]"),
            "got: {}",
            summarize(&scene)
        );

        apply_presence(
            &mut scene,
            &gw,
            DaemonPresenceUpdate::RunStarted {
                run_key: "r".into(),
            },
            now,
        );
        assert!(
            summarize(&scene).contains("daemons=[openclaw@18789:busy]"),
            "got: {}",
            summarize(&scene)
        );

        // RunFailed drains the in-flight run, so the later GatewayDown still reads down.
        apply_presence(
            &mut scene,
            &gw,
            DaemonPresenceUpdate::RunFailed {
                run_key: "r".into(),
            },
            now,
        );
        assert!(
            summarize(&scene).contains("daemons=[openclaw@18789:degraded]"),
            "got: {}",
            summarize(&scene)
        );

        apply_presence(&mut scene, &gw, DaemonPresenceUpdate::GatewayDown, now);
        assert!(
            summarize(&scene).contains("daemons=[openclaw@18789:down]"),
            "got: {}",
            summarize(&scene)
        );
    }

    #[test]
    fn summarize_strips_terminal_escapes_from_untrusted_fields() {
        use pixtuoid_core::source::AgentEvent;
        use pixtuoid_core::AgentId;

        let mut scene = SceneState::new([8; MAX_FLOORS]);
        let mut reducer = Reducer::new();
        let now = SystemTime::now();
        let id = AgentId::from_parts("cc", "esc");
        // The label derives from the cwd basename — an attacker-controlled path can
        // smuggle an OSC set-title + BEL sequence.
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
        assert!(out.contains("repo]0;pwned"), "got: {out}");
        assert!(out.contains("needs [2J approval"), "got: {out}");
    }

    #[test]
    fn explicit_cap_clamps_to_layout_capacity_not_above() {
        let base = boot_capacities_for(192, 48);
        let layout_max = *base.iter().max().unwrap();
        assert_eq!(
            cap_boot_capacities(base, Some(layout_max + 100)),
            base,
            "cap above layout capacity must clamp down to the layout, not inflate"
        );
        assert!(cap_boot_capacities(base, Some(1)).iter().all(|&c| c <= 1));
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
