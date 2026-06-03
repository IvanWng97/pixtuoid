use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use pixtuoid_core::state::MAX_FLOORS;

use anyhow::Result;
use pixtuoid_core::source::antigravity::AntigravitySource;
use pixtuoid_core::source::claude_code::ClaudeCodeSource;
use pixtuoid_core::source::codex::CodexSource;
use pixtuoid_core::source::manager::SourceManager;
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentEvent, Reducer, SceneState, TaggedReceiver, Transport};
use tokio::sync::{mpsc, watch};

/// The reducer publishes a fresh `Arc<SceneState>` on every mutation through
/// this watch channel. Consumers (renderer, headless summary loop) hold a
/// `Receiver`, call `borrow()` for an O(1) pointer read, and never block
/// the writer. Replaces the old `Arc<RwLock<SceneState>>` so:
///   - cloning is a pointer copy (Arc::clone), not a heap allocation per
///     field (thanks to interned `Arc<str>` strings in `AgentSlot`)
///   - the renderer never holds a lock that could block the reducer
///   - swapping in a v2 daemon means publishing the same Arc over a socket
pub type SceneRx = watch::Receiver<Arc<SceneState>>;

/// Fallback desk capacity when the terminal cannot be queried (e.g.
/// headless mode). The real capacity is computed from terminal size in
/// `compute_boot_capacity` before the first TUI frame.
const FALLBACK_DESKS: usize = 16;

#[allow(clippy::too_many_arguments)]
pub fn run(
    socket: Option<PathBuf>,
    projects_root: Option<PathBuf>,
    codex_sessions_root: Option<PathBuf>,
    pack_dir: Option<PathBuf>,
    desk_cap: Option<usize>,
    headless: bool,
    theme_name: String,
    config_path: PathBuf,
    pets: Vec<crate::tui::pet::Pet>,
) -> Result<()> {
    let theme = crate::tui::theme::theme_by_name(&theme_name).ok_or_else(|| {
        let valid: Vec<&str> = crate::tui::theme::ALL_THEMES
            .iter()
            .map(|t| t.name)
            .collect();
        anyhow::anyhow!("unknown theme: {theme_name}. Valid: {}", valid.join(", "))
    })?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        run_async(
            socket,
            projects_root,
            codex_sessions_root,
            pack_dir,
            desk_cap,
            headless,
            theme,
            config_path,
            pets,
        )
        .await
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_async(
    socket: Option<PathBuf>,
    projects_root: Option<PathBuf>,
    codex_sessions_root: Option<PathBuf>,
    pack_dir: Option<PathBuf>,
    desk_cap: Option<usize>,
    headless: bool,
    theme: &'static crate::tui::theme::Theme,
    config_path: PathBuf,
    pets: Vec<crate::tui::pet::Pet>,
) -> Result<()> {
    let mut cc_src = ClaudeCodeSource::default_paths();
    if let Some(s) = socket {
        cc_src.socket_path = s;
    }
    if let Some(p) = projects_root {
        cc_src.projects_root = p;
    }

    let ag_src = AntigravitySource::default_paths();

    let mut codex_src = CodexSource::default_paths();
    if let Some(p) = codex_sessions_root {
        codex_src.sessions_root = p;
    }

    let (tx, rx) = mpsc::channel::<(Transport, AgentEvent)>(256);
    let boot_caps: [usize; MAX_FLOORS] = match (desk_cap, headless) {
        // Headless: no terminal to measure. Honor the cap as-is, else the fallback.
        (Some(cap), true) => [cap; MAX_FLOORS],
        (None, true) => [FALLBACK_DESKS; MAX_FLOORS],
        // Interactive: measure the real per-floor layout capacity FIRST, then clamp
        // to the optional cap. Clamping (not `[cap; _]`) keeps the boot atomics from
        // being seeded above the layout's real capacity — `fetch_max` only grows, so
        // an over-seed strands agents on non-existent desks until the terminal grows.
        (cap, false) => cap_boot_capacities(compute_boot_capacities(), cap),
    };
    let (scene_tx, scene_rx) = watch::channel(Arc::new(SceneState::new(boot_caps)));

    let floor_caps: Arc<[AtomicUsize; MAX_FLOORS]> =
        Arc::new(std::array::from_fn(|i| AtomicUsize::new(boot_caps[i])));

    tokio::spawn(reducer_task(rx, scene_tx, Arc::clone(&floor_caps)));

    let _source_handles = SourceManager::new()
        .with_source(Box::new(cc_src))
        .with_source(Box::new(ag_src))
        .with_source(Box::new(codex_src))
        .spawn(tx);

    if headless {
        headless_loop(scene_rx).await
    } else {
        crate::tui::run_tui(
            scene_rx,
            pack_dir,
            floor_caps,
            theme,
            config_path,
            desk_cap,
            pets,
        )
        .await
    }
}

async fn reducer_task(
    mut rx: TaggedReceiver,
    scene_tx: watch::Sender<Arc<SceneState>>,
    floor_caps: Arc<[AtomicUsize; MAX_FLOORS]>,
) {
    let mut reducer = Reducer::new();
    let initial_caps: [usize; MAX_FLOORS] =
        std::array::from_fn(|i| floor_caps[i].load(Ordering::Relaxed));
    let mut scene = SceneState::new(initial_caps);
    // 1-Hz tick so exit-grace sweeps run even when no new events arrive.
    let mut sweep_interval = tokio::time::interval(Duration::from_secs(1));
    sweep_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        // Sync per-floor capacities from the shared atomics so the
        // auto-computed layout capacity propagates to next_free_desk().
        for (i, a) in floor_caps.iter().enumerate() {
            scene.floor_capacities[i] = a.load(Ordering::Relaxed);
        }
        tokio::select! {
            event = rx.recv() => {
                let Some((transport, ev)) = event else { break };
                let now = SystemTime::now();
                tracing::debug!(?transport, ?ev, "event");
                reducer.apply(&mut scene, ev, now, transport);
                if scene_tx.send(Arc::new(scene.clone())).is_err() {
                    tracing::warn!("scene channel closed — renderer dropped");
                    break;
                }
            }
            _ = sweep_interval.tick() => {
                reducer.tick(&mut scene, SystemTime::now());
                if scene_tx.send(Arc::new(scene.clone())).is_err() {
                    tracing::warn!("scene channel closed — renderer dropped");
                    break;
                }
            }
        }
    }
}

async fn headless_loop(mut scene_rx: SceneRx) -> Result<()> {
    tracing::info!("pixtuoid headless mode — Ctrl-C to quit");
    let mut prev_summary = String::new();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                let snapshot = scene_rx.borrow_and_update().clone();
                let summary = summarize(&snapshot);
                if summary != prev_summary {
                    println!("{summary}");
                    prev_summary = summary;
                }
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutting down");
                return Ok(());
            }
        }
    }
}

fn compute_boot_capacities() -> [usize; MAX_FLOORS] {
    match crossterm::terminal::size().ok() {
        Some((cols, rows)) => boot_capacities_for(cols, rows),
        None => [FALLBACK_DESKS; MAX_FLOORS],
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
        let seed = (i as u64).wrapping_mul(crate::tui::floor::FLOOR_SEED_MULTIPLIER);
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
    let buf_h = rows.saturating_sub(1) * 2;
    pixtuoid_core::layout::SceneLayout::compute_with_seed(
        cols,
        buf_h,
        pixtuoid_core::layout::MAX_VISIBLE_DESKS,
        floor_seed,
    )
    .map(|l| l.home_desks.len())
    .unwrap_or(0)
}

fn summarize(scene: &SceneState) -> String {
    let agents: Vec<String> = scene
        .agents
        .values()
        .map(|a| {
            let state = match &a.state {
                ActivityState::Idle => "idle".to_string(),
                ActivityState::Active { detail, .. } => {
                    format!("active({})", detail.as_deref().unwrap_or("?"))
                }
                ActivityState::Waiting { reason } => format!("waiting({reason})"),
            };
            format!("{}@{}:{}", a.label, a.desk_index, state)
        })
        .collect();
    format!("agents=[{}]", agents.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor_seed(i: u64) -> u64 {
        i.wrapping_mul(crate::tui::floor::FLOOR_SEED_MULTIPLIER)
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
}
