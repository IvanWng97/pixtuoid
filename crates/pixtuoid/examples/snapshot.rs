//! Renders the TUI off-screen via ratatui's TestBackend, then converts every
//! cell into an 8x16-px tile in a PNG so we can verify the visual output
//! without needing a real terminal. Used to validate the TUI after code-review
//! fixes — see `cargo run --example snapshot --release`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result};
use clap::Parser;
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame as GifFrame, Rgb as ImgRgb, RgbImage, Rgba, RgbaImage};
use pixtuoid::tui::embedded_pack::load_sprite_pack;
use pixtuoid::tui::frame_cache::FrameCache;
use pixtuoid::tui::renderer::{draw_scene, DrawCtx, TickerQueue};
use pixtuoid_core::source::jsonl::JsonlWatcher;
use pixtuoid_core::source::AgentEvent;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex, Reducer, SceneState, Transport};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;
use tokio::sync::{mpsc, RwLock};

const COLS: u16 = 192;
const ROWS: u16 = 80;
const CELL_W: u32 = 8;
const CELL_H: u32 = 16;

#[derive(Debug, Parser)]
#[command(about = "Render the TUI off-screen to a PNG for verification")]
struct SnapshotArgs {
    /// Output PNG path.
    #[arg(default_value = "snapshot.png")]
    out: PathBuf,

    /// Capture real CC events by watching --projects-root for --listen-secs.
    #[arg(long)]
    live: bool,

    /// CC project root to watch (only with --live).
    #[arg(long, default_value_t = default_projects_root())]
    projects_root: String,

    /// How many seconds to listen for events (only with --live).
    #[arg(long, default_value_t = 5)]
    listen_secs: u64,

    /// After rendering the scene normally, overlay every non-walkable
    /// pixel in semi-transparent red so the restricted zones are visible.
    /// Use this to verify that all open areas are connected (no isolated
    /// pockets that would cause an A* fallback / character teleport).
    #[arg(long)]
    debug_walkable: bool,

    /// Custom sprite pack directory.
    #[arg(long)]
    pack_dir: Option<std::path::PathBuf>,

    /// Override the snapshot terminal width (cells). Default 192.
    #[arg(long)]
    cols: Option<u16>,

    /// Override the snapshot terminal height (cells). Default 64.
    #[arg(long)]
    rows: Option<u16>,

    /// Cap on home desks per floor for the sample scene. Agents past
    /// this count overflow to additional floors (up to MAX_FLOORS=10).
    /// Pair with `--agents` >16 and `--max-desks 16` to capture a
    /// full floor-1 + populated floor-2 multi-floor gif.
    #[arg(long, default_value_t = 12)]
    max_desks: usize,

    /// Number of agents in the sample scene (default 12). With more agents
    /// than --max-desks, the extras overflow to additional floors — pair
    /// more than 16 agents with --max-desks 16 for an honest full-floor +
    /// floor-2 multi-floor capture.
    #[arg(long, default_value_t = 12)]
    agents: usize,

    /// Output an animated GIF instead of a static PNG. Renders
    /// `--gif-duration` seconds at `--gif-fps` frames per second,
    /// advancing the clock each frame so animations (typing bob,
    /// walking, wander cycles) play out.
    #[arg(long)]
    gif: bool,

    /// GIF duration in seconds (only with --gif).
    #[arg(long, default_value_t = 5)]
    gif_duration: u64,

    /// GIF frame rate (only with --gif). 10 fps is a good balance of
    /// smoothness vs file size (~2-5 MB for a 5s clip).
    #[arg(long, default_value_t = 10)]
    gif_fps: u64,

    /// Color theme name.
    #[arg(long, default_value = "normal")]
    theme: String,

    /// Floor seed — selects floor layout variant (0–4).
    #[arg(long, default_value_t = 0)]
    floor_seed: u64,

    /// Schedule floor navigations inside a --gif capture: repeatable
    /// `--navigate-at <sec>:<floor>` (0-based floor). Renders through the
    /// real TuiRenderer — slide transition, footer floor chip and all —
    /// instead of the single-floor draw_scene path. Pair with --max-desks
    /// so overflow agents populate the extra floors. Navigations less than
    /// ~1s apart are dropped (a slide in flight ignores navigate_floor).
    #[arg(
        long = "navigate-at",
        value_name = "SEC:FLOOR",
        requires = "gif",
        conflicts_with = "anim"
    )]
    navigate_at: Vec<String>,

    /// Render an empty office (no agents) — useful for capturing the
    /// dimmed empty-floor look.
    #[arg(long)]
    empty: bool,

    /// Override local hour-of-day (0–23) used by time-of-day effects
    /// (sun spot, dust motes, lighting). Useful for capturing screenshots
    /// of daylight effects from a machine running at night.
    #[arg(long)]
    now_hour: Option<u32>,

    /// Override local day-of-January-2026 used by time-of-day. Combined
    /// with --now-hour, lets us walk through enough 10-minute weather
    /// slots to hit rare variants.
    #[arg(long, default_value_t = 1)]
    now_day: u32,

    /// Force a specific weather, bypassing the clock-based 10-minute cycle.
    /// One of: clear | rain | storm | snow | fog | overcast | windy | smog.
    /// Drives the weather gallery (`just gen-media`); pair with --now-hour
    /// to pick a flattering time of day per weather.
    #[arg(long)]
    weather: Option<String>,

    /// Force the `?` keyboard help overlay open (for screenshots).
    #[arg(long)]
    help_open: bool,

    /// Force the source-death footer warning (#157) with the given source
    /// name (for screenshots), e.g. --source-warning claude-code.
    #[arg(long)]
    source_warning: Option<String>,

    /// Force the theme picker open at the given row index (for screenshots).
    #[arg(long)]
    theme_picker: Option<usize>,

    /// Force the version popup fully visible (for screenshots).
    #[arg(long)]
    popup: bool,

    /// Add a wandering office pet to a renderer-driven --gif capture
    /// (cat | dog). Routes the capture through the real TuiRenderer,
    /// which owns pet motion -- the pet roams desks/pantry/sofas and
    /// naps near idle agents.
    #[arg(long, value_name = "KIND", requires = "gif", conflicts_with = "anim")]
    pets: Option<String>,

    /// Render with the agent-dashboard popup open over a representative mixed
    /// cc/cx/rx scene (a cc parent with 2 subagents + a cx root + an rx root,
    /// varied activity states). Drives the dashboard demo image + visual checks.
    #[arg(long, conflicts_with_all = ["anim", "gif", "live", "empty", "pets"])]
    dashboard: bool,

    /// Render with the Connection panel open over a representative mixed fixture
    /// (per-CLI hook state + live connection). Drives the connection demo image +
    /// borderless visual checks.
    #[arg(long, conflicts_with_all = ["anim", "gif", "live", "empty", "pets", "dashboard"])]
    connection: bool,

    /// Animation-verification mode: render ONE agent walking to + settling at a
    /// chosen furniture, so the approach→settle reads correctly (no pop, no
    /// teleport) BEFORE human verify. One of: couch | sofa | stand | pantry |
    /// desk. Forces `--gif`; the agent is back-dated so its walk-out starts at
    /// frame 0. Pair with `--gif-duration`/`--gif-fps`. The target furniture's
    /// buffer position is printed so you can crop to it.
    #[arg(long)]
    anim: Option<String>,

    /// Override the `--anim` pre-roll skip (ms). The default skips to the
    /// walk-out (settle/sit follow); set a LARGER value to start the capture at
    /// a later phase — e.g. desk_dwell + walk + sit_dwell to capture the LEAVE
    /// (walk-back). Lets the harness verify the full walk→settle→sit→leave cycle
    /// in short clips instead of one huge GIF.
    #[arg(long)]
    anim_skip_ms: Option<u64>,

    /// Stage N agents (2–3) converging on ONE meeting room in a `--gif`
    /// capture: each agent's wander state is back-dated onto a cycle whose
    /// deterministic destination is a distinct meeting-room slot, with their
    /// desk dwells aligned (≤3s spread) so they rise near-together, walk over,
    /// and chitchat fires within seconds of the second arrival. Composes with
    /// `--agents`: the staged agents take desk indices 0..N and the normal
    /// sample archetypes fill desks N..--agents so the floor stays alive.
    /// Auto-computes a pre-roll (min staged desk-dwell − 1.5s) so the encoded
    /// clip starts just before the first agent rises; `--warmup-secs`
    /// overrides it. Drives the MEETINGS clip in scripts/media.json.
    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(u8).range(2..=3),
        requires = "gif",
        conflicts_with_all = ["anim", "dashboard", "empty", "live", "pets", "navigate_at"]
    )]
    meeting: Option<u8>,

    /// Pre-roll a `--gif` capture: advance the simulated clock through the
    /// real per-frame render (motion state advances) WITHOUT encoding frames
    /// for the first N seconds, so the clip starts mid-action. Overrides the
    /// `--meeting` auto-computed warmup. (`--anim` has its own pre-roll knob,
    /// `--anim-skip-ms`; `--pets`/`--navigate-at` render through
    /// save_renderer_gif, which has no skip seam — conflict rather than
    /// silently no-op.)
    #[arg(
        long,
        value_name = "SECS",
        requires = "gif",
        conflicts_with_all = ["anim", "pets", "navigate_at"]
    )]
    warmup_secs: Option<f64>,

    /// Restrict `--anim sofa`/`couch`/`stand` to a seat with a given SEATED
    /// facing: `north` (back-view, `back_couch` sprite — sofa occludes the lower
    /// body) or `south` (front-view, `seated` sprite). Lets a single meeting room
    /// be captured from BOTH its sofas (north-of-table faces south, south-of-table
    /// faces north). Ignored for non-seat targets.
    #[arg(long)]
    anim_facing: Option<String>,

    /// Crop the generated PNG (and text preview) to a 40x24-cell window
    /// centered on the agent with this label — e.g. for sprite-iteration
    /// close-ups without quadrant guessing. Static-PNG path only: the
    /// --gif/--anim paths return before the crop is computed, so clap
    /// rejects the combination instead of silently ignoring the flag.
    #[arg(long, conflicts_with_all = ["crop_furniture", "gif", "anim"])]
    crop_agent: Option<String>,

    /// Crop the generated PNG (and text preview) to a 40x24-cell window
    /// centered on a furniture piece. Static-PNG path only (see --crop-agent).
    /// One of: pantry | couch | vending | printer | meeting | sofa | desk.
    #[arg(long, conflicts_with_all = ["gif", "anim"])]
    crop_furniture: Option<String>,
}

fn default_projects_root() -> String {
    // Honor CLAUDE_CONFIG_DIR (#168) via the same resolver the runtime uses,
    // rather than re-hardcoding the ~/.claude shape (a third drift site).
    pixtuoid_core::source::claude_code::ClaudeCodeSource::default_paths()
        .projects_root
        .to_string_lossy()
        .into_owned()
}

fn parse_navigations(specs: &[String]) -> Result<Vec<(u64, usize)>> {
    specs
        .iter()
        .map(|s| {
            let (sec, floor) = s
                .split_once(':')
                .with_context(|| format!("--navigate-at '{s}': expected SEC:FLOOR"))?;
            let ms = (sec
                .parse::<f64>()
                .with_context(|| format!("--navigate-at '{s}': bad SEC"))?
                * 1000.0) as u64;
            let floor = floor
                .parse::<usize>()
                .with_context(|| format!("--navigate-at '{s}': bad FLOOR"))?;
            Ok((ms, floor))
        })
        .collect()
}

fn main() -> Result<()> {
    let args = SnapshotArgs::parse();

    // Force-weather override (screenshot/gallery only) — set once; the
    // thread-local it sets is honored by every weather derivation on this
    // thread, including each frame of the GIF path.
    if let Err(valid) = pixtuoid::tui::pixel_painter::force_weather(args.weather.as_deref()) {
        anyhow::bail!(
            "unknown --weather {:?}; valid: {}",
            args.weather.unwrap_or_default(),
            valid.join(" | ")
        );
    }

    let now = match args.now_hour {
        Some(h) => {
            use chrono::TimeZone;
            chrono::Local
                .with_ymd_and_hms(2026, 1, args.now_day, h, 0, 0)
                .single()
                .ok_or_else(|| {
                    anyhow::anyhow!("invalid --now-day/--now-hour {}:{}", args.now_day, h)
                })?
                .into()
        }
        None => SystemTime::now(),
    };
    let cols = args.cols.unwrap_or(COLS);
    let rows = args.rows.unwrap_or(ROWS);
    let mut skip_ms = 0u64;
    let scene = if let Some(target) = args.anim.as_deref() {
        let (s, skip) = anim_scene(
            now,
            target,
            cols,
            rows,
            args.floor_seed,
            args.anim_facing.as_deref(),
        );
        skip_ms = args.anim_skip_ms.unwrap_or(skip);
        eprintln!("ANIM pre-roll skip = {skip_ms}ms (default {skip}ms)");
        s
    } else if let Some(n) = args.meeting {
        let (s, warmup) = meeting_scene(
            now,
            n as usize,
            cols,
            rows,
            args.floor_seed,
            args.max_desks,
            args.agents,
        )?;
        skip_ms = warmup;
        eprintln!("MEETING auto warmup = {warmup}ms");
        s
    } else if args.empty {
        SceneState::uniform(args.max_desks)
    } else if args.live {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        rt.block_on(capture_live_scene(&args.projects_root, args.listen_secs))?
    } else if args.dashboard {
        dashboard_scene(now)
    } else {
        sample_scene(now, args.max_desks, args.agents)
    };
    if let Some(secs) = args.warmup_secs {
        skip_ms = (secs * 1000.0) as u64;
        eprintln!("WARMUP pre-roll = {skip_ms}ms (explicit --warmup-secs)");
    }
    let backend = TestBackend::new(cols, rows);
    let mut term = Terminal::new(backend)?;
    let mut buf = RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 });
    let pack = load_sprite_pack(args.pack_dir.clone())?;
    let mut cache = FrameCache::new();
    let mut router = pixtuoid::tui::pathfind::AStarRouter::new();
    let mut overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut history = pixtuoid::tui::pose::PoseHistory::new();
    // Fail loudly like --weather above — a typo'd theme silently rendering
    // NORMAL would put wrong-palette art into the docs/site screenshot pipelines.
    let theme = pixtuoid::tui::theme::theme_by_name(&args.theme).ok_or_else(|| {
        let valid: Vec<&str> = pixtuoid::tui::theme::ALL_THEMES
            .iter()
            .map(|t| t.name)
            .collect();
        anyhow::anyhow!(
            "unknown --theme {:?}; valid: {}",
            args.theme,
            valid.join(" | ")
        )
    })?;
    let ticker = TickerQueue::new();

    let navigations = parse_navigations(&args.navigate_at)?;
    let pet_vec: Vec<pixtuoid::tui::pet::Pet> = match args.pets.as_deref() {
        None => vec![],
        Some(kind_str) => {
            use pixtuoid::tui::pet::{Pet, PetKind};
            let kind = match kind_str {
                "cat" => PetKind::Cat,
                "dog" => PetKind::Dog,
                other => anyhow::bail!("unknown --pets {:?}; valid: cat | dog", other),
            };
            vec![Pet {
                kind,
                name: "Pixel".into(),
            }]
        }
    };

    if args.floor_seed != 0 && (!navigations.is_empty() || !pet_vec.is_empty()) {
        eprintln!(
            "--floor-seed is ignored on the renderer path (--navigate-at / --pets): \
             TuiRenderer derives per-floor seeds internally"
        );
    }
    if !navigations.is_empty() || !pet_vec.is_empty() {
        save_renderer_gif(
            term,
            &scene,
            &pack,
            now,
            &args.out,
            cols,
            rows,
            args.gif_fps,
            args.gif_duration,
            theme,
            &navigations,
            pet_vec,
        )?;
        println!("wrote {}", args.out.display());
        return Ok(());
    }

    if args.gif || args.anim.is_some() {
        save_as_gif(
            &mut term,
            &scene,
            &pack,
            now,
            &args.out,
            cols,
            rows,
            &mut buf,
            &mut cache,
            &mut router,
            &mut overlay,
            &mut history,
            args.gif_fps,
            args.gif_duration,
            theme,
            args.floor_seed,
            skip_ms,
            args.debug_walkable,
        )?;
        println!("wrote {}", args.out.display());
        return Ok(());
    }

    // Reuse the REAL formatter so the screenshot wording can't drift from
    // production.
    let warning_text = args.source_warning.as_deref().and_then(|src| {
        pixtuoid::tui::widgets::source_warning_message(&[
            pixtuoid_core::source::manager::SourceDeath::new(src, "forced for screenshot"),
        ])
    });
    let mut chitchat_state = std::collections::HashMap::new();
    let mut light = pixtuoid::tui::floor::LightingState::new();
    let mut motion: std::collections::HashMap<
        pixtuoid_core::AgentId,
        pixtuoid::tui::motion::MotionState,
    > = std::collections::HashMap::new();
    // Static snapshots have no time to animate the fade — snap straight
    // to the steady-state level for the chosen scene.
    if args.empty {
        light.snap_to_empty();
    }
    let (dash_rows, dash_selected) = if args.dashboard {
        let folds = pixtuoid::tui::dashboard::DashboardFolds::default();
        let rows = pixtuoid::tui::dashboard::build_dashboard_rows(&scene, &folds);
        let sel = rows.first().map(|r| r.agent_id);
        (rows, sel)
    } else {
        (Vec::new(), None)
    };

    // Representative Connection-panel fixture (deterministic — no FS probes), so the
    // demo image is reproducible across machines.
    let (connection_rows, connection_live, connection_socket_line) = if args.connection {
        use pixtuoid::tui::connection::{ConnState, ConnectionRow, LiveInfo};
        use std::path::PathBuf;
        use std::time::Duration;
        let mk = |source_id, label_prefix, display_name, state, cfg: Option<&str>| ConnectionRow {
            source_id,
            label_prefix,
            display_name,
            state,
            config_path: cfg.map(PathBuf::from),
            target: None,
        };
        let rows = vec![
            mk(
                "claude-code",
                "cc",
                "Claude Code",
                ConnState::Connected,
                Some("~/.claude/settings.json"),
            ),
            mk(
                "codex",
                "cx",
                "Codex",
                ConnState::Disconnected,
                Some("~/.codex/config.toml"),
            ),
            mk("reasonix", "rx", "Reasonix", ConnState::NoCli, None),
            mk(
                "codewhale",
                "cw",
                "CodeWhale",
                ConnState::Connected,
                Some("~/.codewhale/config.toml"),
            ),
            mk(
                "opencode",
                "oc",
                "opencode",
                ConnState::Disconnected,
                Some("~/.config/opencode/plugins/pixtuoid.ts"),
            ),
            mk(
                "antigravity",
                "ag",
                "Antigravity",
                ConnState::Connected,
                None,
            ),
        ];
        let live = vec![
            LiveInfo {
                agents: 2,
                last_event_age: Some(Duration::from_secs(3)),
                dead: false,
            },
            LiveInfo::default(),
            LiveInfo::default(),
            LiveInfo {
                agents: 0,
                last_event_age: None,
                dead: true,
            },
            LiveInfo::default(),
            LiveInfo {
                agents: 1,
                last_event_age: Some(Duration::from_secs(12)),
                dead: false,
            },
        ];
        (
            rows,
            live,
            "socket  /run/user/501/pixtuoid.sock  (listening)".to_string(),
        )
    } else {
        (Vec::new(), Vec::new(), String::new())
    };
    let mut draw_ctx = DrawCtx {
        buf: &mut buf,
        cache: &mut cache,
        router: &mut router,
        overlay: &mut overlay,
        history: &mut history,
        motion: &mut motion,
        door_anim_max_ms: 0,
        light: &mut light,
        mouse_pos: None,
        pinned_agent: None,
        // `--debug-walkable` drives BOTH the live `w` pixel overlay (mask +
        // approach-point/seat markers + A* routes, painted into the RgbBuffer
        // here) AND the cell-level red wash + BFS connectivity report below.
        debug_walkable: args.debug_walkable,
        ticker: &ticker,
        theme,
        theme_picker: args.theme_picker,
        floor_info: None,
        floor: {
            let mut m = pixtuoid::tui::floor::FloorMeta::ground();
            m.floor_seed = args.floor_seed;
            m
        },
        active_pet: None,
        last_pet_pos: None,
        floor_pet: None,
        chitchat_state: &mut chitchat_state,
        chitchat_bubbles: Vec::new(),
        coffee_holders: &std::collections::HashSet::new(),
        coffee_fetched_at: &std::collections::HashMap::new(),
        new_coffee_carriers: Vec::new(),
        popup_scale: if args.popup { 1.0 } else { 0.0 },
        help_open: args.help_open,
        source_warning: warning_text.as_deref(),
        dashboard_open: args.dashboard,
        dashboard_rows: &dash_rows,
        dashboard_selected: dash_selected,
        dashboard_scroll: 0,
        connection_open: args.connection,
        connection_rows: &connection_rows,
        connection_live: &connection_live,
        connection_selected: 0,
        connection_confirm: None,
        connection_result: None,
        connection_socket_line: &connection_socket_line,
    };
    draw_scene(&mut term, &scene, &pack, now, &mut draw_ctx)?;

    if args.debug_walkable {
        debug_paint_walkable_overlay(&mut term, &scene)?;
    }

    let crop_rect = compute_crop_rect(&args, &scene, &history, cols, rows, now)?;

    save_backend_as_png(&term, &args.out, cols, rows, crop_rect)?;
    println!("wrote {}", args.out.display());

    println!("\n--- text preview (symbols only) ---");
    let buf = term.backend().buffer();
    let (start_x, start_y, render_w, render_h) = match crop_rect {
        Some(r) => (r.x, r.y, r.width, r.height),
        None => (0, 0, cols, rows),
    };
    for y in 0..render_h {
        for x in 0..render_w {
            print!("{}", buf[(start_x + x, start_y + y)].symbol());
        }
        println!();
    }
    Ok(())
}

/// Tint every non-walkable terminal cell red and print a connectedness
/// report. A non-walkable cell = either of its two half-block pixels is
/// blocked in the mask. Bright red FG = top pixel blocked; bright red BG
/// = bottom pixel blocked.
///
/// Also runs a BFS from the door threshold and prints how many walkable
/// pixels are reachable vs total — if the two numbers differ, the mask
/// has an isolated region and A* will fall back to a straight line when
/// crossing into it. That's the root cause of any remaining "闪现"
/// (character teleport) the user sees.
fn debug_paint_walkable_overlay(
    term: &mut Terminal<TestBackend>,
    scene: &SceneState,
) -> Result<()> {
    use pixtuoid::tui::layout::SceneLayout;

    let size = term.size()?;
    let scene_w = size.width;
    let scene_h = size.height.saturating_sub(1);
    let buf_w = scene_w;
    let buf_h = scene_h * 2;
    let Some(layout) = SceneLayout::compute(buf_w, buf_h, scene.floor_capacities[0]) else {
        println!("(debug_walkable) layout too small to compute");
        return Ok(());
    };

    // BFS reachability from door_threshold (always inside the corridor,
    // always walkable by construction).
    let reach_mask = compute_reachable(&layout);
    let w = layout.buf_w as usize;
    let h = layout.buf_h as usize;
    let mut reachable = 0usize;
    let mut walkable_total = 0usize;
    let mut sample_disconnects: Vec<(u16, u16)> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if layout.is_walkable(x as u16, y as u16) {
                walkable_total += 1;
                if reach_mask[y * w + x] {
                    reachable += 1;
                } else if sample_disconnects.len() < 10 {
                    sample_disconnects.push((x as u16, y as u16));
                }
            }
        }
    }
    let disconnected = walkable_total.saturating_sub(reachable);
    println!(
        "--- walkability report ---\n\
        total walkable pixels   : {walkable_total}\n\
        reachable from threshold: {reachable}\n\
        disconnected pixels     : {disconnected}{}",
        if disconnected == 0 {
            "  ✓ all open areas connected"
        } else {
            "  ⚠ disconnected components present"
        }
    );
    if !sample_disconnects.is_empty() {
        print!("sample disconnected   : ");
        for (i, (x, y)) in sample_disconnects.iter().enumerate() {
            if i > 0 {
                print!(", ");
            }
            print!("({x},{y})");
        }
        println!();
        // Probe the door-threshold neighborhood + the suspected bridge
        // pixel so we can spot which step of the chain is actually blocked.
        let probe = |x: u16, y: u16, name: &str| {
            let wk = layout.is_walkable(x, y);
            let r = is_reachable(&reach_mask, &layout, x, y);
            println!("  probe {name} ({x},{y}): walkable={wk} reachable={r}");
        };
        if let Some(t) = layout.door_threshold {
            probe(t.x, t.y, "threshold");
        }
        probe(0, layout.top_margin, "MR top-left");
        // Probe the row y=66 (pantry's last row above baseboard).
        println!("row y=66 walkability:");
        for x in 0..30u16 {
            let w = layout.is_walkable(x, 66);
            let r = is_reachable(&reach_mask, &layout, x, 66);
            println!("  x={x}: walk={w} reach={r}");
        }
    }

    // No cell-level redraw: the live `w` pixel overlay (painted into the
    // RgbBuffer in draw_scene) already visualizes the mask + approach/seat
    // markers + routes at pixel resolution. A crude full-cell wash here would
    // just overwrite it. The text report above is the unique value this pass
    // adds (the BFS isolated-region "闪现" detector), so keep that and stop.
    Ok(())
}

fn compute_reachable(layout: &pixtuoid::tui::layout::SceneLayout) -> Vec<bool> {
    use std::collections::VecDeque;
    let w = layout.buf_w as usize;
    let h = layout.buf_h as usize;
    let mut visited = vec![false; w * h];
    let Some(start) = layout.door_threshold else {
        return visited;
    };
    if !layout.is_walkable(start.x, start.y) {
        return visited;
    }
    let (sx, sy) = (start.x as usize, start.y as usize);
    visited[sy * w + sx] = true;
    let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
    queue.push_back((sx, sy));
    while let Some((x, y)) = queue.pop_front() {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            if nx >= w || ny >= h || visited[ny * w + nx] {
                continue;
            }
            if !layout.is_walkable(nx as u16, ny as u16) {
                continue;
            }
            visited[ny * w + nx] = true;
            queue.push_back((nx, ny));
        }
    }
    visited
}

fn is_reachable(
    mask: &[bool],
    layout: &pixtuoid::tui::layout::SceneLayout,
    x: u16,
    y: u16,
) -> bool {
    let w = layout.buf_w as usize;
    let h = layout.buf_h as usize;
    let (xi, yi) = (x as usize, y as usize);
    if xi >= w || yi >= h {
        return false;
    }
    mask[yi * w + xi]
}

/// BFS from `layout.door_threshold` and count visited vs total walkable
/// pixels. If the two differ, the mask has multiple connected components
/// — that's the structural cause of A*'s "no path found" fallback, which
async fn capture_live_scene(projects_root: &str, listen_secs: u64) -> Result<SceneState> {
    println!(
        "listening for real CC events under {} for {}s...",
        projects_root, listen_secs
    );
    let scene: Arc<RwLock<SceneState>> = Arc::new(RwLock::new(SceneState::uniform(12)));
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(1024);
    let root = PathBuf::from(projects_root);
    let watcher = JsonlWatcher::new(
        root,
        pixtuoid_core::source::claude_code::SOURCE_NAME.to_string(),
        pixtuoid_core::source::claude_code::decode_cc_line,
        pixtuoid_core::source::claude_code::cc_derive_label,
        pixtuoid_core::source::claude_code::cc_session_ended,
    );
    let watcher_handle = tokio::spawn(async move { watcher.run(tx).await });

    let mut reducer = Reducer::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(listen_secs);
    let mut event_count: u64 = 0;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some((transport, ev))) => {
                let now = SystemTime::now();
                let mut s = scene.write().await;
                reducer.apply(&mut s, ev, now, transport);
                event_count += 1;
            }
            _ => break,
        }
    }
    let snapshot = scene.read().await.clone();
    println!(
        "captured {} events; final scene has {} agents",
        event_count,
        snapshot.agents.len()
    );
    for (id, slot) in &snapshot.agents {
        println!(
            "  {} ({}) at desk {}: {:?}",
            slot.label, id, slot.desk_index.0, slot.state
        );
    }
    watcher_handle.abort();
    Ok(snapshot)
}

fn sample_scene(now: SystemTime, max_desks: usize, n_agents: usize) -> SceneState {
    let mut s = SceneState::uniform(max_desks);
    fill_sample_agents(&mut s, now, 0..n_agents);
    s
}

/// Insert the standard sample-scene archetypes at desk indices `desks`
/// (`i % 12` cycles the archetype list). Split out of `sample_scene` so
/// `meeting_scene` can fill desks N.. around its staged agents.
fn fill_sample_agents(s: &mut SceneState, now: SystemTime, desks: std::ops::Range<usize>) {
    use std::time::Duration as D;
    let agents: [(&str, ActivityState, D); 12] = [
        (
            "working",
            ActivityState::Active {
                tool_use_id: Some("tu_a".into()),
                detail: Some("Write: src/foo.rs".into()),
            },
            D::from_millis(0),
        ),
        (
            "waiting",
            ActivityState::Waiting {
                reason: "permission?".into(),
            },
            D::from_secs(10),
        ),
        ("thinking", ActivityState::Idle, D::from_secs(5)), // 5s ago — within thinking window
        ("idle-a", ActivityState::Idle, D::from_secs(300)), // 5 min — wander/sleep cycle
        ("idle-b", ActivityState::Idle, D::from_secs(301)),
        ("idle-c", ActivityState::Idle, D::from_secs(303)),
        (
            "couch-act",
            ActivityState::Active {
                tool_use_id: Some("tu_c".into()),
                detail: Some("Read: README.md".into()),
            },
            D::from_millis(140),
        ),
        (
            "couch-bk",
            ActivityState::Waiting {
                reason: "review".into(),
            },
            D::from_millis(0),
        ),
        (
            "floor-act",
            ActivityState::Active {
                tool_use_id: Some("tu_d".into()),
                detail: Some("Bash: cargo test".into()),
            },
            D::from_millis(140),
        ),
        ("floor-idle", ActivityState::Idle, D::from_millis(2_000)),
        (
            "floor-act2",
            ActivityState::Active {
                tool_use_id: Some("tu_e".into()),
                detail: Some("Grep: TODO".into()),
            },
            D::from_millis(280),
        ),
        ("floor-idle2", ActivityState::Idle, D::from_millis(3_000)),
    ];
    for i in desks {
        let (key, state, age) = &agents[i % agents.len()];
        // Keys must be unique across the full desk range: bare key for the first
        // pass over the archetypes, suffixed once they cycle so each desk slot gets
        // its own AgentId and BTreeMap entry.
        let unique_key = if i < agents.len() {
            key.to_string()
        } else {
            format!("{key}-{i}")
        };
        let id = AgentId::from_transcript_path(&format!("/demo/{unique_key}.jsonl"));
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: std::sync::Arc::from("claude-code"),
                session_id: std::sync::Arc::from(format!("demo-{unique_key}").as_str()),
                cwd: std::sync::Arc::from(PathBuf::from("/demo").as_path()),
                label: std::sync::Arc::from(unique_key.as_str()),
                state: state.clone(),
                state_started_at: now - *age,
                created_at: now - *age,
                last_event_at: now - *age,
                exiting_at: None,
                pending_idle_at: None,

                desk_index: GlobalDeskIndex(i),
                // floor_of maps the global desk_index to the correct floor based on
                // per-floor capacities; hardcoding 0 would leave overflow agents invisible.
                floor_idx: s.floor_of(GlobalDeskIndex(i)),
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
            },
        );
    }
}

/// One (agent, cycle) whose deterministic wander destination is a meeting-room
/// slot — a candidate for `--meeting` staging.
#[derive(Debug, Clone)]
struct MeetingCandidate {
    path: String,
    id: AgentId,
    cycle_n: u64,
    wp_idx: usize,
    room_id: usize,
    is_sofa: bool,
    /// The room's bottom-most meeting seat renders the sitter in BACK view,
    /// mostly occluded behind the table — a staged group should avoid it so
    /// all sprites read on camera (review finding; the seat itself is fine
    /// for organic wander).
    is_south_seat: bool,
    dwell_ms: u64,
}

/// Max per-group spread of the staged agents' desk dwells (ms) — they should
/// rise from their desks near-together so arrivals overlap well inside the
/// 20–40s meeting dwell.
const MEETING_DWELL_SPREAD_MS: u64 = 3_000;

/// Encoded clip starts this long before the earliest staged rise.
const MEETING_WARMUP_LEAD_MS: u64 = 1_500;

/// Build a scene where `n` agents (desks 0..n) are staged to converge on ONE
/// meeting room: each agent's `state_started_at` is back-dated by
/// `cycle_n * est_wander_cycle_ms + ε` so motion's bootstrap fast-forward
/// selects a cycle whose deterministic `waypoint_index_for_cycle` lands on a
/// DISTINCT slot of the same room (so they don't fight for one seat). Motion
/// deliberately restarts the phase clock on first observation (anti-teleport),
/// so every staged agent still sits out its full `seated_dwell_ms` before
/// rising — the returned warmup (min dwell − 1.5s) pre-rolls the capture to
/// just before the first rise. Desks n..n_agents get the sample archetypes.
fn meeting_scene(
    now: SystemTime,
    n: usize,
    cols: u16,
    rows: u16,
    floor_seed: u64,
    max_desks: usize,
    n_agents: usize,
) -> Result<(SceneState, u64)> {
    use pixtuoid_core::layout::{SceneLayout, WaypointKind};
    use pixtuoid_core::pose::{
        est_wander_cycle_ms, is_aimless_cycle, seated_dwell_ms, takes_trip,
        waypoint_index_for_cycle,
    };

    // Match the renderer's layout EXACTLY (terminal minus 1-row footer,
    // half-block doubling) — same convention as anim_scene. Capacity must be
    // the scene's floor-0 capacity (max_desks), which is what draw_scene
    // passes, or the waypoint indices shift and the staging silently misses.
    let (buf_w, buf_h) = (cols, rows.saturating_sub(1).saturating_mul(2));
    let l = SceneLayout::compute_with_seed(buf_w, buf_h, max_desks, floor_seed)
        .ok_or_else(|| anyhow::anyhow!("--meeting: scene too small to compute a layout"))?;
    let nw = l.waypoints.len();

    // Candidate sweep: deterministic synthetic ids; for each, the LOWEST cycle
    // (≥1 — cycle 0's 400ms back-date sits inside ENTRY_ANIMATION_MS, so the
    // door entry-walk override would hijack the staging; the thinking window
    // can never fire here since last_event_at == created_at) whose trip
    // deterministically lands on a meeting slot. The sweep trusts motion's
    // approach_point fallback for reachability: a boxed-in seat degrades to an
    // aimless amble, caught by the visual check at `just gen` time, not here.
    // Per room, the bottom-most (max-y) meeting seat seats its sitter in BACK
    // view behind the table — flag it so the picker can avoid staging there.
    let south_y_of_room = |room: usize| -> Option<u16> {
        l.waypoints
            .iter()
            .filter(|w| {
                w.room_id == Some(room)
                    && matches!(
                        w.kind,
                        WaypointKind::MeetingSofa | WaypointKind::MeetingStand
                    )
            })
            .map(|w| w.pos.y)
            .max()
    };

    let mut cands: Vec<MeetingCandidate> = Vec::new();
    for i in 0..20_000u64 {
        let path = format!("/meeting/agent_{i}.jsonl");
        let id = AgentId::from_transcript_path(&path);
        for cycle_n in 1..=5u64 {
            if !takes_trip(id, cycle_n) || is_aimless_cycle(id, cycle_n) {
                continue;
            }
            let wp_idx = waypoint_index_for_cycle(id, cycle_n, nw);
            let wp = l.waypoints[wp_idx];
            let is_sofa = match wp.kind {
                WaypointKind::MeetingSofa => true,
                WaypointKind::MeetingStand => false,
                _ => continue,
            };
            let room_id = wp.room_id.unwrap_or(0);
            cands.push(MeetingCandidate {
                path,
                id,
                cycle_n,
                wp_idx,
                room_id,
                is_sofa,
                is_south_seat: south_y_of_room(room_id) == Some(wp.pos.y),
                dwell_ms: seated_dwell_ms(id),
            });
            break;
        }
    }
    cands.sort_by_key(|c| (c.dwell_ms, c.id.raw()));

    // Lowest-dwell window of n candidates with: same room, distinct slots,
    // dwell spread ≤ 3s. Prefer windows seating ≥2 on sofas (reads "meeting"
    // far better than a stand-up cluster); fall back without that bias.
    let pick = |need_sofas: usize, avoid_south: bool| -> Option<Vec<MeetingCandidate>> {
        for (i, base) in cands.iter().enumerate() {
            if avoid_south && base.is_south_seat {
                continue;
            }
            let mut sel = vec![base.clone()];
            for c in cands[i + 1..].iter() {
                if c.dwell_ms - base.dwell_ms > MEETING_DWELL_SPREAD_MS {
                    break;
                }
                if (avoid_south && c.is_south_seat)
                    || c.room_id != base.room_id
                    || sel.iter().any(|s| s.wp_idx == c.wp_idx)
                {
                    continue;
                }
                sel.push(c.clone());
                if sel.len() == n {
                    break;
                }
            }
            if sel.len() == n && sel.iter().filter(|c| c.is_sofa).count() >= need_sofas {
                return Some(sel);
            }
        }
        None
    };
    let staged = pick(2.min(n), true)
        .or_else(|| pick(2.min(n), false))
        .or_else(|| pick(0, false))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "--meeting {n}: no candidate group found ({} meeting-bound candidates at \
             {buf_w}x{buf_h} seed {floor_seed})",
                cands.len()
            )
        })?;

    let min_dwell = staged.iter().map(|c| c.dwell_ms).min().unwrap_or(0);
    let warmup_ms = min_dwell.saturating_sub(MEETING_WARMUP_LEAD_MS);

    let mut s = SceneState::uniform(max_desks);
    for (i, c) in staged.iter().enumerate() {
        let wp = l.waypoints[c.wp_idx];
        eprintln!(
            "MEETING agent {} desk {} id={} cycle={} → waypoint[{}] {:?} room {} at ({}, {}) \
             desk_dwell={}ms rise@{}ms",
            (b'a' + i as u8) as char,
            i,
            c.path,
            c.cycle_n,
            c.wp_idx,
            wp.kind,
            c.room_id,
            wp.pos.x,
            wp.pos.y,
            c.dwell_ms,
            c.dwell_ms.saturating_sub(warmup_ms),
        );
        // Back-date past `cycle_n` whole estimated cycles (+ a hair so the
        // integer division can't land on the boundary). created_at matches so
        // the entry-walk override can't fire.
        let back_date = Duration::from_millis(c.cycle_n * est_wander_cycle_ms(c.id) + 400);
        let label = format!("meet-{}", (b'a' + i as u8) as char);
        s.agents.insert(
            c.id,
            AgentSlot {
                agent_id: c.id,
                source: std::sync::Arc::from("claude-code"),
                session_id: std::sync::Arc::from(format!("meeting-{i}").as_str()),
                cwd: std::sync::Arc::from(PathBuf::from("/meeting").as_path()),
                label: std::sync::Arc::from(label.as_str()),
                state: ActivityState::Idle,
                state_started_at: now - back_date,
                created_at: now - back_date,
                last_event_at: now - back_date,
                exiting_at: None,
                pending_idle_at: None,
                desk_index: GlobalDeskIndex(i),
                floor_idx: s.floor_of(GlobalDeskIndex(i)),
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
            },
        );
    }
    // Fill the rest of the office with the normal archetypes so the floor
    // doesn't look dead around the meeting.
    fill_sample_agents(&mut s, now, n..n_agents);
    eprintln!("MEETING staged {n} agents, warmup={warmup_ms}ms (min desk_dwell {min_dwell}ms − {MEETING_WARMUP_LEAD_MS}ms)");
    Ok((s, warmup_ms))
}

/// Build a representative 5-agent scene for the `--dashboard` demo: a CC parent
/// with 2 subagents, a Codex root, and a Reasonix root — distinct badges and
/// varied activity states.
fn dashboard_scene(now: SystemTime) -> SceneState {
    use pixtuoid_core::source::{claude_code, codex, reasonix};
    use pixtuoid_core::state::ActivityState;
    use std::time::Duration as D;

    // Named to satisfy clippy::type_complexity (a 6-field tuple trips the lint):
    // (label, transcript path, state, parent_id, desk_index, source SOURCE_NAME).
    type DashAgentSpec = (
        &'static str,
        &'static str,
        ActivityState,
        Option<AgentId>,
        usize,
        &'static str,
    );

    let mut s = SceneState::uniform(12);

    let cc_root_id = AgentId::from_transcript_path("/demo/dash_cc_root.jsonl");

    let agents: &[DashAgentSpec] = &[
        (
            "cc·pixtuoid",
            "/demo/dash_cc_root.jsonl",
            ActivityState::Active {
                tool_use_id: Some("tu0".into()),
                detail: Some("Edit: reducer.rs".into()),
            },
            None,
            0,
            claude_code::SOURCE_NAME,
        ),
        (
            "code-explorer",
            "/demo/dash_cc_sub1.jsonl",
            ActivityState::Active {
                tool_use_id: Some("tu1".into()),
                detail: Some("Grep: TODO".into()),
            },
            Some(cc_root_id),
            1,
            claude_code::SOURCE_NAME,
        ),
        (
            "code-reviewer",
            "/demo/dash_cc_sub2.jsonl",
            ActivityState::Idle,
            Some(cc_root_id),
            2,
            claude_code::SOURCE_NAME,
        ),
        (
            "cx·sidecar",
            "/demo/dash_cx_root.jsonl",
            ActivityState::Idle,
            None,
            3,
            codex::SOURCE_NAME,
        ),
        (
            "rx·helper",
            "/demo/dash_rx_root.jsonl",
            ActivityState::Waiting {
                reason: "permission?".into(),
            },
            None,
            4,
            reasonix::SOURCE_NAME,
        ),
    ];

    for (label, path, state, parent_id, desk_index, source) in agents {
        let id = AgentId::from_transcript_path(path);
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: std::sync::Arc::from(*source),
                session_id: std::sync::Arc::from(
                    format!("demo-dash-{}", label.replace('·', "-")).as_str(),
                ),
                cwd: std::sync::Arc::from(PathBuf::from("/demo").as_path()),
                label: std::sync::Arc::from(*label),
                state: state.clone(),
                state_started_at: now,
                created_at: now - D::from_secs(*desk_index as u64),
                last_event_at: now,
                exiting_at: None,
                pending_idle_at: None,
                desk_index: GlobalDeskIndex(*desk_index),
                floor_idx: s.floor_of(GlobalDeskIndex(*desk_index)),
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: *parent_id,
            },
        );
    }
    s
}

/// Build a ONE-agent scene whose wander targets `target` furniture, back-dated so
/// the walk-OUT starts at frame 0 — for `--anim` visual verification of the
/// approach→settle (no pop, no teleport). Prints the furniture's buffer position
/// so the caller can crop the GIF to it. `target` ∈ {couch, sofa, stand, pantry,
/// desk}; "desk" captures the always-present return-to-desk leg.
fn anim_scene(
    now: SystemTime,
    target: &str,
    cols: u16,
    rows: u16,
    floor_seed: u64,
    facing: Option<&str>,
) -> (SceneState, u64) {
    use pixtuoid_core::layout::{Facing, SceneLayout, WaypointKind, MAX_VISIBLE_DESKS};
    use pixtuoid_core::pose::{
        is_aimless_cycle, seated_dwell_ms, takes_trip, waypoint_index_for_cycle,
    };

    // Match the renderer EXACTLY: it draws into scene_rect = terminal minus the
    // 1-row footer, then buf_h = scene_rect.height*2 (half-block). A 2px mismatch
    // shifts the waypoint set and the agent targets the wrong furniture.
    let (buf_w, buf_h) = (cols, rows.saturating_sub(1).saturating_mul(2));
    let l = SceneLayout::compute_with_seed(buf_w, buf_h, MAX_VISIBLE_DESKS, floor_seed)
        .expect("anim layout computes");
    let n = l.waypoints.len();

    let target_kind = match target {
        "couch" => Some(WaypointKind::Couch),
        "sofa" => Some(WaypointKind::MeetingSofa),
        "stand" => Some(WaypointKind::MeetingStand),
        "pantry" => Some(WaypointKind::Pantry),
        _ => None, // "desk": always visited (return-to-desk), not a waypoint
    };
    let want_facing = match facing {
        Some("north") => Some(Facing::North),
        Some("south") => Some(Facing::South),
        Some("east") => Some(Facing::East),
        Some("west") => Some(Facing::West),
        _ => None,
    };
    let target_idxs: Vec<usize> = l
        .waypoints
        .iter()
        .enumerate()
        .filter(|(_, w)| Some(w.kind) == target_kind)
        .filter(|(_, w)| want_facing.is_none_or(|f| w.facing == f))
        .map(|(i, _)| i)
        .collect();

    if target == "desk" {
        if let Some(d) = l.home_desks.first() {
            eprintln!("ANIM target=desk buf_pos≈({}, {}) [home desk 0]", d.x, d.y);
        }
    } else if let Some(&i) = target_idxs.first() {
        let p = l.waypoints[i].pos;
        eprintln!(
            "ANIM target={target} buf_pos=({}, {}) [{} matching waypoints, {n} total]",
            p.x,
            p.y,
            target_idxs.len()
        );
    } else {
        eprintln!(
            "ANIM target={target}: no matching waypoint at {buf_w}x{buf_h} seed {floor_seed}"
        );
    }

    // Brute-force an agent whose cycle-0 trip lands on the target (any tripping,
    // non-aimless agent for "desk").
    let path = (0u64..40_000)
        .map(|i| format!("/anim/{target}_{i}.jsonl"))
        .find(|p| {
            let id = AgentId::from_transcript_path(p);
            takes_trip(id, 0)
                && !is_aimless_cycle(id, 0)
                && (target == "desk"
                    || (n > 0 && target_idxs.contains(&waypoint_index_for_cycle(id, 0, n))))
        })
        .unwrap_or_else(|| format!("/anim/{target}_fallback.jsonl"));

    let id = AgentId::from_transcript_path(&path);
    // Print the agent's ACTUAL cycle-0 target — NOT the first matching waypoint
    // above (which is misleading when several seats match: the agent may sit on
    // a different one, so cropping to the printed pos shows an empty seat). This
    // is the buffer position to crop to for verification.
    if target != "desk" && n > 0 {
        let wi = waypoint_index_for_cycle(id, 0, n);
        let wp = l.waypoints[wi];
        eprintln!(
            "ANIM agent ACTUAL target = waypoint[{wi}] {:?} facing {:?} at buf_pos=({}, {})",
            wp.kind, wp.facing, wp.pos.x, wp.pos.y
        );
    }
    // Fresh agent at `now` (clean Seated start — the TUI re-anchors fresh agents
    // there regardless of created_at). The GIF PRE-ROLLS `skip_ms` past the
    // seated dwell so capture begins right as it walks out (see save_as_gif).
    let skip_ms = seated_dwell_ms(id).saturating_sub(1_000);
    eprintln!(
        "ANIM agent seated_dwell={}ms → pre-roll skip={skip_ms}ms",
        seated_dwell_ms(id)
    );

    let mut s = SceneState::uniform(MAX_VISIBLE_DESKS);
    s.agents.insert(
        id,
        AgentSlot {
            agent_id: id,
            source: std::sync::Arc::from("claude-code"),
            session_id: std::sync::Arc::from("anim"),
            cwd: std::sync::Arc::from(PathBuf::from("/anim").as_path()),
            label: std::sync::Arc::from(target),
            state: ActivityState::Idle,
            state_started_at: now,
            created_at: now,
            last_event_at: now,
            exiting_at: None,
            pending_idle_at: None,
            desk_index: GlobalDeskIndex(0),
            floor_idx: 0,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
        },
    );
    (s, skip_ms)
}

fn compute_crop_rect(
    args: &SnapshotArgs,
    scene: &SceneState,
    history: &pixtuoid::tui::pose::PoseHistory,
    cols: u16,
    rows: u16,
    now: SystemTime,
) -> Result<Option<ratatui::layout::Rect>> {
    use pixtuoid_core::layout::WaypointKind;

    // Fail loudly like --theme/--weather above — a typo'd crop target silently
    // writing the full uncropped PNG defeats the point of the flag.
    let target_pixel: pixtuoid::tui::layout::Point = if let Some(ref agent_label) = args.crop_agent
    {
        let slot = scene
            .agents
            .values()
            .find(|s| s.label.as_ref() == agent_label)
            .ok_or_else(|| {
                let labels: Vec<&str> = scene.agents.values().map(|s| s.label.as_ref()).collect();
                anyhow::anyhow!(
                    "--crop-agent {agent_label:?} not found in scene; labels: {}",
                    labels.join(", ")
                )
            })?;
        history
            .recent(slot.agent_id, u64::MAX, now)
            .ok_or_else(|| anyhow::anyhow!("agent {agent_label:?} has no visual position"))?
    } else if let Some(ref furniture_str) = args.crop_furniture {
        let buf_w = cols;
        let buf_h = rows.saturating_sub(1).saturating_mul(2);
        let layout = pixtuoid_core::layout::SceneLayout::compute_with_seed(
            buf_w,
            buf_h,
            scene.floor_capacities[0],
            args.floor_seed,
        )
        .ok_or_else(|| anyhow::anyhow!("scene too small to compute a layout"))?;
        let found = match furniture_str.to_lowercase().as_str() {
            "desk" => layout.home_desks.first().copied(),
            name => {
                let kind = match name {
                    "pantry" => WaypointKind::Pantry,
                    "couch" => WaypointKind::Couch,
                    "vending" => WaypointKind::VendingMachine,
                    "printer" => WaypointKind::Printer,
                    "meeting" | "sofa" => WaypointKind::MeetingSofa,
                    other => anyhow::bail!(
                        "unknown --crop-furniture {other:?}; valid: pantry | couch | vending | printer | meeting | sofa | desk"
                    ),
                };
                layout
                    .waypoints
                    .iter()
                    .find(|w| w.kind == kind)
                    .map(|w| w.pos)
            }
        };
        found.ok_or_else(|| {
            anyhow::anyhow!("no {furniture_str:?} waypoint in this layout (terminal too small?)")
        })?
    } else {
        return Ok(None);
    };

    // Positions are in the LOGICAL half-block buffer (1 px per cell across,
    // 2 px per cell down — the same buf_w/buf_h fed to compute_with_seed
    // above), NOT in PNG pixels: the 8x16 px-per-cell scaling happens later
    // in save_backend_as_png.
    Ok(Some(centered_crop(
        target_pixel.x,
        target_pixel.y / 2,
        cols,
        rows,
    )))
}

/// 40x24-cell window centered on (cell_x, cell_y), clamped to stay inside the
/// cols x rows buffer (shrinks only when the terminal itself is smaller).
fn centered_crop(cell_x: u16, cell_y: u16, cols: u16, rows: u16) -> ratatui::layout::Rect {
    let crop_w = 40u16.min(cols);
    let crop_h = 24u16.min(rows);

    let crop_x = cell_x
        .saturating_sub(crop_w / 2)
        .min(cols.saturating_sub(crop_w));
    let crop_y = cell_y
        .saturating_sub(crop_h / 2)
        .min(rows.saturating_sub(crop_h));

    ratatui::layout::Rect {
        x: crop_x,
        y: crop_y,
        width: crop_w,
        height: crop_h,
    }
}

fn save_backend_as_png(
    term: &Terminal<TestBackend>,
    path: &PathBuf,
    cols: u16,
    rows: u16,
    crop: Option<ratatui::layout::Rect>,
) -> Result<()> {
    let buf = term.backend().buffer();
    let (start_x, start_y, render_w, render_h) = match crop {
        Some(r) => (r.x, r.y, r.width, r.height),
        None => (0, 0, cols, rows),
    };
    let img_w = render_w as u32 * CELL_W;
    let img_h = render_h as u32 * CELL_H;
    let mut img = RgbImage::new(img_w, img_h);

    for y in 0..render_h {
        for x in 0..render_w {
            let cell = &buf[(start_x + x, start_y + y)];
            let symbol = cell.symbol();
            let fg = color_to_rgb(cell.fg, ImgRgb([220, 220, 220]));
            let bg = color_to_rgb(cell.bg, ImgRgb([20, 22, 28]));

            // For the half-block character "▀", the cell is split: top half = fg, bottom half = bg.
            // Other characters are rasterized as real text via the 8x8 bitmap font (glyph8x8);
            // a glyph no font set covers falls back to a centered fg block.
            let x0 = x as u32 * CELL_W;
            let y0 = y as u32 * CELL_H;

            if symbol == "▀" {
                fill_rect(&mut img, x0, y0, CELL_W, CELL_H / 2, fg);
                fill_rect(&mut img, x0, y0 + CELL_H / 2, CELL_W, CELL_H / 2, bg);
            } else if symbol.trim().is_empty() {
                fill_rect(&mut img, x0, y0, CELL_W, CELL_H, bg);
            } else if let Some(rows) = glyph8x8(symbol) {
                fill_rect(&mut img, x0, y0, CELL_W, CELL_H, bg);
                blit_glyph_cell(rows, x0, y0, |px, py| {
                    if px < img_w && py < img_h {
                        img.put_pixel(px, py, fg);
                    }
                });
            } else {
                // No glyph in any font set (a decorative symbol): keep the old
                // centered block so the cell still reads in its fg color.
                fill_rect(&mut img, x0, y0, CELL_W, CELL_H, bg);
                let pad_x = 1;
                let pad_y = 3;
                fill_rect(
                    &mut img,
                    x0 + pad_x,
                    y0 + pad_y,
                    CELL_W - pad_x * 2,
                    CELL_H - pad_y * 2,
                    fg,
                );
            }
        }
    }

    img.save(path)?;
    Ok(())
}

/// Rasterize a post-draw ratatui cell buffer to RGBA: half-block cells become
/// two stacked pixels (fg = top, bg = bottom); text cells are drawn as real
/// glyphs via the 8x8 bitmap font (glyph8x8) — same path as the PNG rasterizer.
fn cells_to_rgba(
    term_buf: &ratatui::buffer::Buffer,
    cols: u16,
    rows: u16,
    img_w: u32,
    img_h: u32,
) -> RgbaImage {
    let mut rgba = RgbaImage::new(img_w, img_h);
    for y in 0..rows {
        for x in 0..cols {
            let cell = &term_buf[(x, y)];
            let symbol = cell.symbol();
            let fg = color_to_rgb(cell.fg, ImgRgb([220, 220, 220]));
            let bg = color_to_rgb(cell.bg, ImgRgb([20, 22, 28]));
            let x0 = x as u32 * CELL_W;
            let y0 = y as u32 * CELL_H;
            if symbol == "▀" {
                fill_rgba_rect(&mut rgba, x0, y0, CELL_W, CELL_H / 2, fg);
                fill_rgba_rect(&mut rgba, x0, y0 + CELL_H / 2, CELL_W, CELL_H / 2, bg);
            } else if symbol.trim().is_empty() {
                fill_rgba_rect(&mut rgba, x0, y0, CELL_W, CELL_H, bg);
            } else if let Some(rows) = glyph8x8(symbol) {
                fill_rgba_rect(&mut rgba, x0, y0, CELL_W, CELL_H, bg);
                let fg_rgba = Rgba([fg[0], fg[1], fg[2], 255]);
                blit_glyph_cell(rows, x0, y0, |px, py| {
                    if px < img_w && py < img_h {
                        rgba.put_pixel(px, py, fg_rgba);
                    }
                });
            } else {
                fill_rgba_rect(&mut rgba, x0, y0, CELL_W, CELL_H, bg);
                let pad_x = 1;
                let pad_y = 3;
                fill_rgba_rect(
                    &mut rgba,
                    x0 + pad_x,
                    y0 + pad_y,
                    CELL_W - pad_x * 2,
                    CELL_H - pad_y * 2,
                    fg,
                );
            }
        }
    }
    rgba
}

/// Floors whose scheduled navigation comes due at `elapsed_ms`, firing each
/// schedule entry exactly once (marks `fired`). Pure so the timing contract
/// is unit-testable — an off-by-one here silently shifts a slide out of the
/// capture window.
fn due_navigations(
    navigations: &[(u64, usize)],
    fired: &mut [bool],
    elapsed_ms: u64,
) -> Vec<usize> {
    let mut due = Vec::new();
    for (n, &(at_ms, floor)) in navigations.iter().enumerate() {
        if !fired[n] && elapsed_ms >= at_ms {
            fired[n] = true;
            due.push(floor);
        }
    }
    due
}

/// Drive the real TuiRenderer (slide transition, footer floor chip, pet motion)
/// frame by frame and encode its TestBackend cell buffer. Covers multi-floor
/// captures (via `navigations`) and pet clips (via `pets`).
#[allow(clippy::too_many_arguments)]
fn save_renderer_gif(
    term: Terminal<TestBackend>,
    scene: &SceneState,
    pack: &pixtuoid_core::sprite::format::Pack,
    start_now: SystemTime,
    path: &PathBuf,
    cols: u16,
    rows: u16,
    fps: u64,
    duration_secs: u64,
    theme: &'static pixtuoid::tui::theme::Theme,
    navigations: &[(u64, usize)],
    pets: Vec<pixtuoid::tui::pet::Pet>,
) -> Result<()> {
    use pixtuoid_core::render::Renderer as _;
    let frame_count = (duration_secs * fps) as usize;
    let frame_ms = 1000 / fps.max(1);
    let img_w = cols as u32 * CELL_W;
    let img_h = rows as u32 * CELL_H;

    let file = std::fs::File::create(path)?;
    let mut encoder = GifEncoder::new(file);
    encoder.set_repeat(Repeat::Infinite)?;

    let mut r = pixtuoid::tui::tui_renderer::TuiRenderer::new(term, theme, pets);
    let mut fired = vec![false; navigations.len()];
    for i in 0..frame_count {
        // Exact, not i * frame_ms: the truncated frame_ms accumulates (15fps → a
        // "10s" gif spans only 9834ms, so a late --navigate-at would never fire).
        let elapsed_ms = i as u64 * 1000 / fps.max(1);
        let now = start_now + Duration::from_millis(elapsed_ms);
        for floor in due_navigations(navigations, &mut fired, elapsed_ms) {
            r.navigate_floor(floor, now);
        }
        r.render(scene, pack, now)?;
        let rgba = cells_to_rgba(r.terminal.backend().buffer(), cols, rows, img_w, img_h);
        let delay = Delay::from_numer_denom_ms(frame_ms as u32, 1);
        encoder.encode_frame(GifFrame::from_parts(rgba, 0, 0, delay))?;
        let cap = i + 1;
        if cap.is_multiple_of(fps as usize) {
            eprint!("\r  encoding: {}/{}s", cap / fps as usize, duration_secs);
        }
    }
    eprintln!("\r  encoded {frame_count} frames @ {fps}fps");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn save_as_gif(
    term: &mut Terminal<TestBackend>,
    scene: &SceneState,
    pack: &pixtuoid_core::sprite::format::Pack,
    start_now: SystemTime,
    path: &PathBuf,
    cols: u16,
    rows: u16,
    buf: &mut RgbBuffer,
    cache: &mut FrameCache,
    router: &mut pixtuoid::tui::pathfind::AStarRouter,
    overlay: &mut pixtuoid_core::walkable::OccupancyOverlay,
    history: &mut pixtuoid::tui::pose::PoseHistory,
    fps: u64,
    duration_secs: u64,
    theme: &pixtuoid::tui::theme::Theme,
    floor_seed: u64,
    skip_ms: u64,
    debug_walkable: bool,
) -> Result<()> {
    let frame_count = (duration_secs * fps) as usize;
    let frame_ms = 1000 / fps.max(1);
    // Pre-roll: render (advancing the persistent motion state) WITHOUT encoding
    // for `skip_ms`, so an `--anim` capture starts at the agent's walk-out
    // instead of its long seated dwell. 0 for normal GIFs.
    let skip_frames = (skip_ms / frame_ms.max(1)) as usize;
    let img_w = cols as u32 * CELL_W;
    let img_h = rows as u32 * CELL_H;
    let ticker = TickerQueue::new();

    let file = std::fs::File::create(path)?;
    let mut encoder = GifEncoder::new(file);
    encoder.set_repeat(Repeat::Infinite)?;

    let mut chitchat_state = std::collections::HashMap::new();
    let mut light = pixtuoid::tui::floor::LightingState::new();
    let mut motion: std::collections::HashMap<
        pixtuoid_core::AgentId,
        pixtuoid::tui::motion::MotionState,
    > = std::collections::HashMap::new();
    for i in 0..(skip_frames + frame_count) {
        let now = start_now + Duration::from_millis(i as u64 * frame_ms);
        let mut draw_ctx = DrawCtx {
            buf,
            cache,
            router,
            overlay,
            history,
            motion: &mut motion,
            door_anim_max_ms: 0,
            light: &mut light,
            mouse_pos: None,
            pinned_agent: None,
            debug_walkable,
            ticker: &ticker,
            theme,
            theme_picker: None,
            floor_info: None,
            floor: {
                let mut m = pixtuoid::tui::floor::FloorMeta::ground();
                m.floor_seed = floor_seed;
                m
            },
            active_pet: None,
            last_pet_pos: None,
            floor_pet: None,
            chitchat_state: &mut chitchat_state,
            chitchat_bubbles: Vec::new(),
            coffee_holders: &std::collections::HashSet::new(),
            coffee_fetched_at: &std::collections::HashMap::new(),
            new_coffee_carriers: Vec::new(),
            popup_scale: 0.0,
            help_open: false,
            source_warning: None,
            dashboard_open: false,
            dashboard_rows: &[],
            dashboard_selected: None,
            dashboard_scroll: 0,
            connection_open: false,
            connection_rows: &[],
            connection_live: &[],
            connection_selected: 0,
            connection_confirm: None,
            connection_result: None,
            connection_socket_line: "",
        };
        draw_scene(term, scene, pack, now, &mut draw_ctx)?;
        if i < skip_frames {
            continue; // pre-roll: advance the motion state, don't encode
        }

        let rgba = cells_to_rgba(term.backend().buffer(), cols, rows, img_w, img_h);
        let delay = Delay::from_numer_denom_ms(frame_ms as u32, 1);
        let frame = GifFrame::from_parts(rgba, 0, 0, delay);
        encoder.encode_frame(frame)?;
        let cap = i + 1 - skip_frames;
        if cap.is_multiple_of(fps as usize) {
            eprint!("\r  encoding: {}/{}s", cap / fps as usize, duration_secs);
        }
    }
    eprintln!("\r  encoded {frame_count} frames @ {fps}fps");
    Ok(())
}

fn fill_rgba_rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, color: ImgRgb<u8>) {
    let (img_w, img_h) = (img.width(), img.height());
    let rgba = Rgba([color[0], color[1], color[2], 255]);
    for j in 0..h {
        for i in 0..w {
            let px_x = x + i;
            let px_y = y + j;
            if px_x < img_w && px_y < img_h {
                img.put_pixel(px_x, px_y, rgba);
            }
        }
    }
}

fn fill_rect(img: &mut RgbImage, x: u32, y: u32, w: u32, h: u32, color: ImgRgb<u8>) {
    let (img_w, img_h) = (img.width(), img.height());
    for j in 0..h {
        for i in 0..w {
            let px_x = x + i;
            let px_y = y + j;
            if px_x < img_w && px_y < img_h {
                img.put_pixel(px_x, px_y, color);
            }
        }
    }
}

/// 8x8 bitmaps for the few status glyphs font8x8 lacks but the popups lean on
/// for at-a-glance state — the dashboard/connection dots `●` `○` `◐`. Without
/// these all three collapse to the same fallback block, distinct only by color.
/// Rows top→bottom, bit 0 (LSB) = leftmost pixel — same convention as font8x8,
/// so `blit_glyph_cell` renders them unchanged.
fn custom_glyph(ch: char) -> Option<[u8; 8]> {
    match ch {
        '\u{25CF}' => Some([0x3C, 0x7E, 0xFF, 0xFF, 0xFF, 0xFF, 0x7E, 0x3C]), // ● filled disc
        '\u{25CB}' => Some([0x3C, 0x42, 0x81, 0x81, 0x81, 0x81, 0x42, 0x3C]), // ○ ring
        '\u{25D0}' => Some([0x3C, 0x4E, 0x8F, 0x8F, 0x8F, 0x8F, 0x4E, 0x3C]), // ◐ left-half disc
        _ => None,
    }
}

/// The demo rasterizer's "font": an 8x8 bitmap per char — our `custom_glyph`
/// status dots first, then font8x8's sets (ASCII, then Latin/box/block/misc/
/// greek). Returns rows top→bottom; within a row bit 0 (LSB) is the leftmost
/// pixel. `None` for a glyph nothing covers — the caller draws a block.
fn glyph8x8(symbol: &str) -> Option<[u8; 8]> {
    use font8x8::{
        UnicodeFonts, BASIC_FONTS, BLOCK_FONTS, BOX_FONTS, GREEK_FONTS, LATIN_FONTS, MISC_FONTS,
    };
    let ch = symbol.chars().next()?;
    custom_glyph(ch)
        .or_else(|| BASIC_FONTS.get(ch))
        .or_else(|| LATIN_FONTS.get(ch))
        .or_else(|| BOX_FONTS.get(ch))
        .or_else(|| BLOCK_FONTS.get(ch))
        .or_else(|| MISC_FONTS.get(ch))
        .or_else(|| GREEK_FONTS.get(ch))
}

/// Blit an 8x8 glyph into one 8x16 cell, doubled vertically (1px → 2px tall) so
/// it fills the cell. `put` paints one foreground pixel (bg is pre-filled).
fn blit_glyph_cell(rows: [u8; 8], x0: u32, y0: u32, mut put: impl FnMut(u32, u32)) {
    for (fr, &bits) in rows.iter().enumerate() {
        for col in 0..CELL_W {
            if bits & (1u8 << col) != 0 {
                let px = x0 + col;
                let py = y0 + fr as u32 * 2;
                put(px, py);
                put(px, py + 1);
            }
        }
    }
}

fn color_to_rgb(c: Color, default: ImgRgb<u8>) -> ImgRgb<u8> {
    match c {
        Color::Rgb(r, g, b) => ImgRgb([r, g, b]),
        Color::Black => ImgRgb([0, 0, 0]),
        Color::Red => ImgRgb([180, 50, 50]),
        Color::Green => ImgRgb([60, 180, 60]),
        Color::Yellow => ImgRgb([220, 200, 50]),
        Color::Blue => ImgRgb([60, 120, 220]),
        Color::Magenta => ImgRgb([200, 60, 200]),
        Color::Cyan => ImgRgb([50, 200, 220]),
        Color::Gray => ImgRgb([160, 160, 160]),
        Color::DarkGray => ImgRgb([80, 80, 80]),
        Color::White => ImgRgb([240, 240, 240]),
        Color::LightRed => ImgRgb([230, 100, 100]),
        Color::LightGreen => ImgRgb([100, 230, 100]),
        Color::LightYellow => ImgRgb([240, 230, 100]),
        Color::LightBlue => ImgRgb([130, 180, 250]),
        Color::LightMagenta => ImgRgb([240, 130, 240]),
        Color::LightCyan => ImgRgb([130, 240, 240]),
        Color::Indexed(_) | Color::Reset => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph8x8_covers_ascii_and_custom_dots_but_not_arbitrary_symbols() {
        // ASCII resolves via font8x8's BASIC set; · (U+00B7) via LATIN — both
        // are the legibility win, so pin them.
        assert!(glyph8x8("A").is_some());
        assert!(glyph8x8("z").is_some());
        assert!(
            glyph8x8("\u{00b7}").is_some(),
            "middle-dot separator must be legible"
        );
        // The status dots resolve via our custom_glyph table (added so they're
        // shape-distinct, not merely color-distinct).
        assert!(glyph8x8("\u{25cf}").is_some(), "\u{25cf} filled disc");
        assert!(glyph8x8("\u{25cb}").is_some(), "\u{25cb} ring");
        assert!(glyph8x8("\u{25d0}").is_some(), "\u{25d0} left-half disc");
        // ...and the three are DISTINCT bitmaps — the whole point of custom_glyph.
        let (filled, ring, half) = (
            glyph8x8("\u{25cf}").unwrap(),
            glyph8x8("\u{25cb}").unwrap(),
            glyph8x8("\u{25d0}").unwrap(),
        );
        assert_ne!(filled, ring);
        assert_ne!(filled, half);
        assert_ne!(ring, half);
        // A glyph nothing covers falls through to None → the caller's block fallback.
        assert!(
            glyph8x8("\u{2713}").is_none(),
            "\u{2713} check has no glyph"
        );
        assert!(glyph8x8("").is_none(), "empty symbol → None");
    }

    #[test]
    fn blit_glyph_cell_lsb_is_leftmost_and_doubles_vertically() {
        // Row 0 = 0x01: only bit 0 (LSB) set → only col 0 fires, doubled to 2px.
        // A font8x8 bit-order change would silently MIRROR all demo text — this
        // is the guard against that (the HIGH review finding on PR #288).
        let mut hits: Vec<(u32, u32)> = Vec::new();
        let mut rows = [0u8; 8];
        rows[0] = 0x01;
        blit_glyph_cell(rows, 0, 0, |px, py| hits.push((px, py)));
        assert_eq!(
            hits,
            vec![(0, 0), (0, 1)],
            "LSB must map to the LEFTMOST column"
        );

        // Bit 7 (MSB) → rightmost col (7); font row 1 → image rows 2,3.
        let mut hits2: Vec<(u32, u32)> = Vec::new();
        let mut rows2 = [0u8; 8];
        rows2[1] = 0x80;
        blit_glyph_cell(rows2, 0, 0, |px, py| hits2.push((px, py)));
        assert_eq!(
            hits2,
            vec![(7, 2), (7, 3)],
            "MSB → rightmost col; row 1 → img rows 2,3"
        );

        // The x0/y0 origin offset is honored.
        let mut hits3: Vec<(u32, u32)> = Vec::new();
        let mut rows3 = [0u8; 8];
        rows3[0] = 0x01;
        blit_glyph_cell(rows3, 8, 16, |px, py| hits3.push((px, py)));
        assert_eq!(hits3, vec![(8, 16), (8, 17)]);
    }

    #[test]
    fn parse_navigations_happy_and_fractional() {
        assert_eq!(
            parse_navigations(&["3:1".to_string(), "2.5:0".to_string()]).unwrap(),
            vec![(3000, 1), (2500, 0)]
        );
    }

    #[test]
    fn parse_navigations_truncates_fractional_ms() {
        // (0.9999 * 1000.0) as u64 == 999 — pin the truncation so it's explicit
        assert_eq!(
            parse_navigations(&["0.9999:0".to_string()]).unwrap(),
            vec![(999, 0)]
        );
    }

    #[test]
    fn parse_navigations_rejects_bad_input() {
        for bad in ["5-1", "5:x", "x:1", "", ":", "5:"] {
            assert!(
                parse_navigations(&[bad.to_string()]).is_err(),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn due_navigations_fires_each_exactly_once_in_schedule_order() {
        // unordered schedule; frame clock at 15fps exact math: i * 1000 / 15
        let navs = vec![(7000u64, 0usize), (3000, 1)];
        let mut fired = vec![false; navs.len()];
        let mut hits: Vec<(u64, usize)> = Vec::new();
        for i in 0..150u64 {
            let elapsed_ms = i * 1000 / 15;
            for floor in due_navigations(&navs, &mut fired, elapsed_ms) {
                hits.push((i, floor));
            }
        }
        // 3000ms: first frame with i*1000/15 >= 3000 is i=45 (exactly 3000)
        // 7000ms: first frame with i*1000/15 >= 7000 is i=105 (exactly 7000)
        assert_eq!(hits, vec![(45, 1), (105, 0)]);
    }

    #[test]
    fn due_navigations_late_schedule_still_fires_within_capture() {
        // regression pin for the exact elapsed math: with truncating per-frame
        // accumulation (i * 66ms) a 9.9s navigation never fired in a 10s/15fps
        // capture; exact math reaches 9933ms at i=149.
        let navs = vec![(9900u64, 1usize)];
        let mut fired = vec![false; 1];
        let mut hit = None;
        for i in 0..150u64 {
            let elapsed_ms = i * 1000 / 15;
            if !due_navigations(&navs, &mut fired, elapsed_ms).is_empty() {
                hit = Some(i);
            }
        }
        assert_eq!(hit, Some(149));
    }

    fn crop_args(extra: &[&str]) -> SnapshotArgs {
        SnapshotArgs::try_parse_from([&["snapshot"], extra].concat()).unwrap()
    }

    #[test]
    fn centered_crop_centers_in_the_open() {
        let r = centered_crop(96, 32, 192, 64);
        assert_eq!((r.x, r.y, r.width, r.height), (76, 20, 40, 24));
    }

    #[test]
    fn centered_crop_clamps_at_origin_and_far_edge() {
        let near_origin = centered_crop(2, 1, 192, 64);
        assert_eq!((near_origin.x, near_origin.y), (0, 0));
        let near_edge = centered_crop(191, 63, 192, 64);
        assert_eq!((near_edge.x, near_edge.y), (152, 40));
    }

    #[test]
    fn centered_crop_shrinks_to_a_small_terminal() {
        let r = centered_crop(10, 5, 30, 20);
        assert_eq!((r.x, r.y, r.width, r.height), (0, 0, 30, 20));
    }

    #[test]
    fn crop_rect_centers_on_the_pantry_waypoint() {
        let now = SystemTime::now();
        let scene = sample_scene(now, 12, 12);
        let history = pixtuoid::tui::pose::PoseHistory::new();
        let args = crop_args(&["--crop-furniture", "pantry"]);
        let rect = compute_crop_rect(&args, &scene, &history, 192, 64, now)
            .unwrap()
            .expect("pantry crop");
        let layout =
            pixtuoid_core::layout::SceneLayout::compute_with_seed(192, 126, 12, 0).unwrap();
        let pantry = layout
            .waypoints
            .iter()
            .find(|w| w.kind == pixtuoid_core::layout::WaypointKind::Pantry)
            .unwrap();
        let (cx, cy) = (pantry.pos.x, pantry.pos.y / 2);
        assert!(rect.x <= cx && cx < rect.x + rect.width, "x not in crop");
        assert!(rect.y <= cy && cy < rect.y + rect.height, "y not in crop");
    }

    #[test]
    fn crop_rect_without_flags_is_none() {
        let now = SystemTime::now();
        let scene = sample_scene(now, 12, 12);
        let history = pixtuoid::tui::pose::PoseHistory::new();
        let args = crop_args(&[]);
        assert!(compute_crop_rect(&args, &scene, &history, 192, 64, now)
            .unwrap()
            .is_none());
    }

    #[test]
    fn crop_rect_fails_loudly_on_typos_and_unknown_agents() {
        let now = SystemTime::now();
        let scene = sample_scene(now, 12, 12);
        let history = pixtuoid::tui::pose::PoseHistory::new();

        let typo = crop_args(&["--crop-furniture", "fridge"]);
        let err = compute_crop_rect(&typo, &scene, &history, 192, 64, now).unwrap_err();
        assert!(err.to_string().contains("valid: pantry"), "{err}");

        let ghost = crop_args(&["--crop-agent", "ghost"]);
        let err = compute_crop_rect(&ghost, &scene, &history, 192, 64, now).unwrap_err();
        assert!(err.to_string().contains("labels:"), "{err}");
    }

    #[test]
    fn crop_flags_conflict_with_gif_and_anim() {
        assert!(SnapshotArgs::try_parse_from(["snapshot", "--gif", "--crop-agent", "x"]).is_err());
        assert!(SnapshotArgs::try_parse_from([
            "snapshot",
            "--anim",
            "couch",
            "--crop-furniture",
            "pantry"
        ])
        .is_err());
    }

    #[test]
    fn meeting_flag_parses_caps_and_conflicts() {
        let ok = ["snapshot", "out.gif", "--gif", "--meeting", "3"];
        assert!(SnapshotArgs::try_parse_from(ok).is_ok());
        // 2..=3 cap; requires --gif; conflicts with the other scene modes.
        for bad in [
            vec!["snapshot", "--gif", "--meeting", "1"],
            vec!["snapshot", "--gif", "--meeting", "4"],
            vec!["snapshot", "--meeting", "3"],
            vec!["snapshot", "--gif", "--meeting", "3", "--anim", "sofa"],
            vec!["snapshot", "--gif", "--meeting", "3", "--pets", "cat"],
            vec![
                "snapshot",
                "--gif",
                "--meeting",
                "3",
                "--navigate-at",
                "3:1",
            ],
            vec!["snapshot", "--gif", "--meeting", "3", "--dashboard"],
            vec!["snapshot", "--gif", "--meeting", "3", "--empty"],
        ] {
            assert!(
                SnapshotArgs::try_parse_from(bad.clone()).is_err(),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn warmup_flag_requires_gif_and_conflicts_with_anim() {
        assert!(
            SnapshotArgs::try_parse_from(["snapshot", "--gif", "--warmup-secs", "13.5"]).is_ok()
        );
        assert!(SnapshotArgs::try_parse_from(["snapshot", "--warmup-secs", "5"]).is_err());
        assert!(SnapshotArgs::try_parse_from([
            "snapshot",
            "--gif",
            "--warmup-secs",
            "5",
            "--anim",
            "sofa"
        ])
        .is_err());
    }

    /// Re-derive the motion bootstrap from the staged slots and pin every
    /// invariant the convergence depends on: motion's first-frame fast-forward
    /// (`elapsed_idle / est_wander_cycle_ms`) must select a trip cycle whose
    /// deterministic destination is a meeting slot, all staged agents in ONE
    /// room on DISTINCT slots, desk dwells within the spread, and the warmup
    /// pre-roll just under the earliest rise.
    #[test]
    fn meeting_scene_stages_a_convergent_group() {
        use pixtuoid_core::layout::{SceneLayout, WaypointKind};
        use pixtuoid_core::pose::{
            est_wander_cycle_ms, is_aimless_cycle, seated_dwell_ms, takes_trip,
            waypoint_index_for_cycle,
        };

        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let (cols, rows, max_desks) = (208u16, 88u16, 12);
        let (scene, warmup_ms) = meeting_scene(now, 3, cols, rows, 0, max_desks, 12).unwrap();
        assert_eq!(scene.agents.len(), 12, "staged 3 + 9 archetype fillers");

        let layout = SceneLayout::compute_with_seed(cols, (rows - 1) * 2, max_desks, 0).unwrap();
        let staged: Vec<_> = scene
            .agents
            .values()
            .filter(|s| s.label.starts_with("meet-"))
            .collect();
        assert_eq!(staged.len(), 3);

        let mut wp_idxs = Vec::new();
        let mut rooms = Vec::new();
        let mut dwells = Vec::new();
        for slot in &staged {
            assert!(slot.desk_index.0 < 3, "staged agents take desks 0..n");
            let id = slot.agent_id;
            let elapsed_ms = now
                .duration_since(slot.state_started_at)
                .unwrap()
                .as_millis() as u64;
            // Mirror motion's bootstrap fast-forward exactly.
            let cycle_n = elapsed_ms / est_wander_cycle_ms(id);
            assert!(cycle_n >= 1, "cycle 0 back-dates under the thinking window");
            assert!(takes_trip(id, cycle_n), "staged cycle must be a trip");
            assert!(!is_aimless_cycle(id, cycle_n), "trip must be directed");
            let wp_idx = waypoint_index_for_cycle(id, cycle_n, layout.waypoints.len());
            let wp = layout.waypoints[wp_idx];
            assert!(
                matches!(
                    wp.kind,
                    WaypointKind::MeetingSofa | WaypointKind::MeetingStand
                ),
                "destination must be a meeting slot, got {:?}",
                wp.kind
            );
            wp_idxs.push(wp_idx);
            rooms.push(wp.room_id.expect("meeting slots carry a room_id"));
            dwells.push(seated_dwell_ms(id));
        }
        wp_idxs.sort_unstable();
        wp_idxs.dedup();
        assert_eq!(wp_idxs.len(), 3, "slots must be distinct (no seat fights)");
        assert!(
            rooms.iter().all(|r| *r == rooms[0]),
            "one room → one chitchat venue"
        );
        let (min_d, max_d) = (*dwells.iter().min().unwrap(), *dwells.iter().max().unwrap());
        assert!(
            max_d - min_d <= MEETING_DWELL_SPREAD_MS,
            "dwell spread {}ms exceeds {}ms",
            max_d - min_d,
            MEETING_DWELL_SPREAD_MS
        );
        assert_eq!(warmup_ms, min_d.saturating_sub(MEETING_WARMUP_LEAD_MS));
    }

    #[test]
    fn dashboard_flag_parses_and_conflicts_with_anim() {
        assert!(SnapshotArgs::try_parse_from(["snapshot", "out.png", "--dashboard"]).is_ok());
        assert!(SnapshotArgs::try_parse_from([
            "snapshot",
            "out.png",
            "--dashboard",
            "--anim",
            "desk"
        ])
        .is_err());
        assert!(
            SnapshotArgs::try_parse_from(["snapshot", "out.png", "--dashboard", "--gif"]).is_err()
        );
    }
}
