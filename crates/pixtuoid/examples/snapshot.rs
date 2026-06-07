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
use pixtuoid_core::source::{Activity, AgentEvent};
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, AgentSlot, Reducer, SceneState, Transport};
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
    /// this count overflow to additional floors (up to MAX_FLOORS=5).
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
    /// Drives the weather gallery (scripts/gen-demos.sh); pair with --now-hour
    /// to pick a flattering time of day per weather.
    #[arg(long)]
    weather: Option<String>,

    /// Force the `?` keyboard help overlay open (for screenshots).
    #[arg(long)]
    help_open: bool,

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

    /// Restrict `--anim sofa`/`couch`/`stand` to a seat with a given SEATED
    /// facing: `north` (back-view, `back_couch` sprite — sofa occludes the lower
    /// body) or `south` (front-view, `seated` sprite). Lets a single meeting room
    /// be captured from BOTH its sofas (north-of-table faces south, south-of-table
    /// faces north). Ignored for non-seat targets.
    #[arg(long)]
    anim_facing: Option<String>,
}

fn default_projects_root() -> String {
    format!(
        "{}/.claude/projects",
        pixtuoid::install::io::user_home().unwrap_or_else(|| ".".into())
    )
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
    let mut anim_skip_ms = 0u64;
    let scene = if let Some(target) = args.anim.as_deref() {
        let (s, skip) = anim_scene(
            now,
            target,
            args.cols.unwrap_or(COLS),
            args.rows.unwrap_or(ROWS),
            args.floor_seed,
            args.anim_facing.as_deref(),
        );
        anim_skip_ms = args.anim_skip_ms.unwrap_or(skip);
        eprintln!("ANIM pre-roll skip = {anim_skip_ms}ms (default {skip}ms)");
        s
    } else if args.empty {
        SceneState::uniform(args.max_desks)
    } else if args.live {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        rt.block_on(capture_live_scene(&args.projects_root, args.listen_secs))?
    } else {
        sample_scene(now, args.max_desks, args.agents)
    };

    let cols = args.cols.unwrap_or(COLS);
    let rows = args.rows.unwrap_or(ROWS);
    let backend = TestBackend::new(cols, rows);
    let mut term = Terminal::new(backend)?;
    let mut buf = RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 });
    let pack = load_sprite_pack(args.pack_dir)?;
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
            anim_skip_ms,
            args.debug_walkable,
        )?;
        println!("wrote {}", args.out.display());
        return Ok(());
    }

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
    };
    draw_scene(&mut term, &scene, &pack, now, &mut draw_ctx)?;

    if args.debug_walkable {
        debug_paint_walkable_overlay(&mut term, &scene)?;
    }

    save_backend_as_png(&term, &args.out, cols, rows)?;
    println!("wrote {}", args.out.display());

    println!("\n--- text preview (symbols only) ---");
    let buf = term.backend().buffer();
    for y in 0..rows {
        for x in 0..cols {
            print!("{}", buf[(x, y)].symbol());
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
            slot.label, id, slot.desk_index, slot.state
        );
    }
    watcher_handle.abort();
    Ok(snapshot)
}

fn sample_scene(now: SystemTime, max_desks: usize, n_agents: usize) -> SceneState {
    use std::time::Duration as D;
    let mut s = SceneState::uniform(max_desks);
    let agents: [(&str, ActivityState, D); 12] = [
        (
            "working",
            ActivityState::Active {
                activity: Activity::Typing,
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
                activity: Activity::Typing,
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
                activity: Activity::Typing,
                tool_use_id: Some("tu_d".into()),
                detail: Some("Bash: cargo test".into()),
            },
            D::from_millis(140),
        ),
        ("floor-idle", ActivityState::Idle, D::from_millis(2_000)),
        (
            "floor-act2",
            ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: Some("tu_e".into()),
                detail: Some("Grep: TODO".into()),
            },
            D::from_millis(280),
        ),
        ("floor-idle2", ActivityState::Idle, D::from_millis(3_000)),
    ];
    for i in 0..n_agents {
        let (key, state, age) = &agents[i % agents.len()];
        // Keys must be unique across the full n_agents range: bare key for the first
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

                desk_index: i,
                // floor_of maps the global desk_index to the correct floor based on
                // per-floor capacities; hardcoding 0 would leave overflow agents invisible.
                floor_idx: s.floor_of(i),
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
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
        .filter(|(_, w)| want_facing.map_or(true, |f| w.facing == f))
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
            desk_index: 0,
            floor_idx: 0,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
        },
    );
    (s, skip_ms)
}

fn save_backend_as_png(
    term: &Terminal<TestBackend>,
    path: &PathBuf,
    cols: u16,
    rows: u16,
) -> Result<()> {
    let buf = term.backend().buffer();
    let img_w = cols as u32 * CELL_W;
    let img_h = rows as u32 * CELL_H;
    let mut img = RgbImage::new(img_w, img_h);

    for y in 0..rows {
        for x in 0..cols {
            let cell = &buf[(x, y)];
            let symbol = cell.symbol();
            let fg = color_to_rgb(cell.fg, ImgRgb([220, 220, 220]));
            let bg = color_to_rgb(cell.bg, ImgRgb([20, 22, 28]));

            // For the half-block character "▀", the cell is split: top half = fg, bottom half = bg.
            // For other characters, we approximate by drawing the cell as one bg-color tile and
            // overlaying a roughly-centered fg-color glyph rectangle.
            let x0 = x as u32 * CELL_W;
            let y0 = y as u32 * CELL_H;

            if symbol == "▀" {
                fill_rect(&mut img, x0, y0, CELL_W, CELL_H / 2, fg);
                fill_rect(&mut img, x0, y0 + CELL_H / 2, CELL_W, CELL_H / 2, bg);
            } else if symbol.trim().is_empty() {
                fill_rect(&mut img, x0, y0, CELL_W, CELL_H, bg);
            } else {
                // Background, then a small fg square in the middle to represent text.
                fill_rect(&mut img, x0, y0, CELL_W, CELL_H, bg);
                // Tiny glyph box — gives a visible indication of where text lives.
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
/// two stacked pixels (fg = top, bg = bottom); text cells a blocky glyph pad —
/// the same look the existing demo.gif path produces.
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
        if cap % (fps as usize) == 0 {
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
        if cap % (fps as usize) == 0 {
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
}
