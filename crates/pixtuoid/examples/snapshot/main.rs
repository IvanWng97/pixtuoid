//! Renders the TUI off-screen via ratatui's TestBackend, then converts every
//! cell into an 8x16-px tile in a PNG so we can verify the visual output
//! without needing a real terminal. Used to validate the TUI after code-review
//! fixes — see `cargo run --example snapshot --release`.

mod encode;
mod proof;
mod scenes;

use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use clap::Parser;
use pixtuoid::tui::renderer::{draw_scene, DrawCtx};
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::SceneState;
use pixtuoid_scene::embedded_pack::load_sprite_pack;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::encode::{
    centered_crop, compute_crop_rect, debug_paint_walkable_overlay, save_as_gif,
    save_backend_as_png, save_renderer_gif,
};
use crate::scenes::{
    anim_scene, capture_live_scene, dashboard_scene, inject_openclaw_presence, meeting_scene,
    sample_scene,
};

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

    /// Inject an OpenClaw gateway presence (the wandering lobster mascot) in the
    /// given state (idle | busy | down) for the beautify visual loop. Off by
    /// default so the gen-media baselines are unaffected.
    #[arg(long)]
    openclaw: Option<String>,

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

    /// Force the decode-drift footer nudge (#308) for the given comma-separated
    /// source label-prefixes (for screenshots), e.g. --drift-warning cc,cx.
    /// Lower priority than --source-warning (source-death preempts it).
    #[arg(long)]
    drift_warning: Option<String>,

    /// Force the theme picker open at the given row index (for screenshots).
    #[arg(long)]
    theme_picker: Option<usize>,

    /// Force the version popup fully visible (for screenshots).
    #[arg(long)]
    popup: bool,

    /// Force a hover tooltip at terminal cell "x,y" (for screenshots / shadow
    /// visual checks), e.g. --hover 30,40.
    #[arg(long, value_name = "X,Y")]
    hover: Option<String>,

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

    /// Render with the Sources panel open over a representative mixed fixture
    /// (per-CLI hook state + live connection). Drives the connection demo image +
    /// borderless visual checks.
    #[arg(long, conflicts_with_all = ["anim", "gif", "live", "empty", "pets", "dashboard"])]
    connection: bool,

    /// Render with the first-run onboarding "move-in" overlay open (fully revealed)
    /// over a representative roster. Drives the onboarding demo image + the
    /// borderless/typewriter visual check.
    #[arg(long, conflicts_with_all = ["anim", "gif", "live", "empty", "pets", "dashboard", "connection"])]
    onboarding: bool,

    /// Animation-verification mode: render ONE agent walking to + settling at a
    /// chosen furniture, so the approach→settle reads correctly (no pop, no
    /// teleport) BEFORE human verify. One of: couch | sofa | chair | pantry |
    /// printer | vending | island | snackshelf | desk. Forces `--gif`; the
    /// agent is back-dated so its walk-out starts at frame 0. Pair with
    /// `--gif-duration`/`--gif-fps`. The target furniture's buffer position
    /// is printed so you can crop to it.
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

    /// Restrict `--anim sofa`/`couch`/`chair` to a seat with a given SEATED
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
    /// One of: pantry | couch | vending | printer | meeting | sofa | chair |
    /// island | snackshelf | desk.
    #[arg(long, conflicts_with_all = ["gif", "anim"])]
    crop_furniture: Option<String>,

    /// Give the agent with this label a Top burn tier (claude-fable-5 at
    /// fresh ultra effort → ember hair + flame crown) — the visual-iteration
    /// knob for the burn feature. NOT used by any gen-media job, so the
    /// committed baselines stay flame-free.
    #[arg(long)]
    flame: Option<String>,

    /// Crop the generated PNG to a window centered on the gateway lobster mascot
    /// — its position is time-derived, so this reads it back from the renderer
    /// AFTER the draw (unlike --crop-agent/--crop-furniture, which precompute).
    /// Needs a VISIBLE mascot: pass --openclaw <state> (and not `down` past the
    /// leave window, which renders none) — enforced at runtime, not by clap, since
    /// "visible" isn't expressible as a static flag dependency. Static-PNG path only.
    #[arg(long, conflicts_with_all = ["crop_agent", "crop_furniture", "gif", "anim"])]
    crop_mascot: bool,

    /// Render the §3 split-screen proof replay from a captured CC session
    /// fixture: left = typed transcript, right = the real reducer+renderer
    /// replaying the same decoded events; annotations burned in. Emits PNG
    /// frame sequences to --frames-dir/{wide,tall}/ (both compositions from
    /// ONE render pass); scripts/gen-media.py (kind:"proof") encodes them.
    #[arg(long, value_name = "FIXTURE", value_hint = clap::ValueHint::FilePath,
          conflicts_with_all = ["gif", "anim", "meeting", "pets", "navigate_at",
          "dashboard", "connection", "onboarding", "empty", "live",
          "crop_agent", "crop_furniture", "crop_mascot", "debug_walkable"])]
    proof: Option<std::path::PathBuf>,

    /// Output directory for --proof frame sequences (wide/ + tall/ created inside).
    #[arg(long, value_hint = clap::ValueHint::DirPath, requires = "proof")]
    frames_dir: Option<std::path::PathBuf>,

    /// --proof frame rate.
    #[arg(long, default_value_t = 12, requires = "proof")]
    proof_fps: u64,

    /// --proof clip length in seconds (the idle tail past the last event included).
    #[arg(long, default_value_t = 26, requires = "proof")]
    proof_secs: u64,
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
    if let Err(valid) = pixtuoid_scene::pixel_painter::force_weather(args.weather.as_deref()) {
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
    let mut scene = scene;
    // --flame applies to WHATEVER scene the mode above produced (sample, anim,
    // meeting, dashboard, live capture) — the burn-tier visual-iteration knob
    // shouldn't silently vanish in the pose-preview modes.
    if let Some(label) = &args.flame {
        let hit = scene
            .agents
            .values_mut()
            .find(|a| a.label.as_ref() == label.as_str());
        match hit {
            Some(a) => {
                a.model = Some("claude-fable-5".into());
                a.effort = Some(pixtuoid_core::state::EffortObservation::new(
                    "ultra".into(),
                    now,
                ));
            }
            None => {
                let labels: Vec<_> = scene.agents.values().map(|a| a.label.to_string()).collect();
                anyhow::bail!("--flame {label:?} not found in scene; labels: {labels:?}");
            }
        }
    }
    if let Some(state) = args.openclaw.as_deref() {
        inject_openclaw_presence(&mut scene, state, now)?;
    }
    let backend = TestBackend::new(cols, rows);
    let mut term = Terminal::new(backend)?;
    let mut buf = RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 });
    let pack = load_sprite_pack(args.pack_dir.clone())?;
    // The per-floor sim/paint stores, grouped (cache/router/overlay/history/
    // light/motion): DrawCtx + save_as_gif now take them as ONE FloorCtx.
    let mut store = pixtuoid_scene::floor::FloorCtx::new();
    // Fail loudly like --weather above — a typo'd theme silently rendering
    // NORMAL would put wrong-palette art into the docs/site screenshot pipelines.
    let theme = pixtuoid_scene::theme::theme_by_name(&args.theme).ok_or_else(|| {
        let valid: Vec<&str> = pixtuoid_scene::theme::ALL_THEMES
            .iter()
            .map(|t| t.name)
            .collect();
        anyhow::anyhow!(
            "unknown --theme {:?}; valid: {}",
            args.theme,
            valid.join(" | ")
        )
    })?;

    if let Some(fixture) = args.proof.as_deref() {
        let frames_dir = args
            .frames_dir
            .as_deref()
            .context("--proof requires --frames-dir")?;
        proof::render_proof(&proof::ProofJob {
            fixture,
            frames_dir,
            cols,
            rows,
            fps: args.proof_fps,
            secs: args.proof_secs,
            max_desks: args.max_desks,
            theme,
            pack: &pack,
            start: now,
        })?;
        println!("wrote proof frames → {}", frames_dir.display());
        return Ok(());
    }

    let navigations = parse_navigations(&args.navigate_at)?;
    let pet_vec: Vec<pixtuoid_scene::pet::Pet> = match args.pets.as_deref() {
        None => vec![],
        Some(kind_str) => {
            use pixtuoid_scene::pet::{Pet, PetKind};
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
            &mut store,
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

    // Reuse the REAL formatters so the screenshot wording can't drift from
    // production. The drift nudge and the source-death warning share the footer
    // channel; `footer_warning` merges them with death > drift priority — the
    // same merge `run_tui` performs live.
    let death_text = args.source_warning.as_deref().and_then(|src| {
        pixtuoid::tui::widgets::source_warning_message(&[
            pixtuoid_core::source::manager::SourceDeath::new(src, "forced for screenshot"),
        ])
    });
    let drifted: Vec<String> = args
        .drift_warning
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let warning_text = pixtuoid::doctor::footer_warning(death_text.as_deref(), &drifted);
    let mut chitchat_state = std::collections::HashMap::new();
    // Static snapshots have no time to animate the fade — snap straight
    // to the steady-state level for the chosen scene.
    if args.empty {
        store.light.snap_to_empty();
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
            health: None,
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
            mk(
                "reasonix",
                "rx",
                "Reasonix",
                ConnState::NoCli { connected: false },
                None,
            ),
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
    let onboarding_frame = if args.onboarding {
        use pixtuoid::tui::welcome::WelcomeRow;
        let mk = |source_id, label_prefix, display_name: &str, checked| WelcomeRow {
            source_id,
            label_prefix,
            display_name: display_name.to_string(),
            checked,
        };
        pixtuoid::tui::welcome::OnboardingFrame {
            open: true,
            rows: vec![
                mk("claude-code", "cc", "Claude Code", true),
                mk("codex", "cx", "Codex", true),
                mk("cursor", "cu", "Cursor CLI", false),
            ],
            selected: 0,
            elapsed_ms: 100_000,
            dim: pixtuoid::tui::welcome::dim_opening(100_000),
        }
    } else {
        pixtuoid::tui::welcome::OnboardingFrame::default()
    };
    let dashboard_frame = pixtuoid::tui::dashboard::DashboardFrame {
        open: args.dashboard,
        rows: dash_rows,
        selected: dash_selected,
        scroll: 0,
    };
    let connection_frame = pixtuoid::tui::connection::ConnectionFrame {
        open: args.connection,
        rows: connection_rows,
        live: connection_live,
        selected: 0,
        confirm: None,
        result: None,
        socket_line: connection_socket_line,
    };
    let mut draw_ctx = DrawCtx {
        buf: &mut buf,
        store: &mut store,
        mouse_pos: args.hover.as_deref().and_then(|s| {
            let (x, y) = s.split_once(',')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        }),
        // `--debug-walkable` drives BOTH the live `w` pixel overlay (mask +
        // approach-point/seat markers + A* routes, painted into the RgbBuffer
        // here) AND the cell-level red wash + BFS connectivity report below.
        debug_walkable: args.debug_walkable,
        theme,
        theme_picker: args.theme_picker,
        floor_info: None,
        // Single-floor still, no gateway: empty office-wide tallies (no cross-floor
        // cue, chip suppressed). The footer's counts come from `scene_stats(scene)`.
        per_floor: Default::default(),
        gateway: None,
        floor: {
            let mut m = pixtuoid_scene::floor::FloorMeta::ground();
            m.floor_seed = args.floor_seed;
            m
        },
        active_pet: None,
        last_pet_pos: None,
        last_mascot_pos: None,
        floor_pet: None,
        chitchat_state: &mut chitchat_state,
        chitchat_bubbles: Vec::new(),
        coffee: &std::collections::HashMap::new(),
        new_coffee_carriers: Vec::new(),
        popup_scale: if args.popup { 1.0 } else { 0.0 },
        help_open: args.help_open,
        source_warning: warning_text.as_deref(),
        dashboard: &dashboard_frame,
        connection: &connection_frame,
        onboarding: &onboarding_frame,
    };
    draw_scene(&mut term, &scene, &pack, now, &mut draw_ctx)?;

    if args.debug_walkable {
        debug_paint_walkable_overlay(&mut term)?;
    }

    let crop_rect = if args.crop_mascot {
        // The mascot wanders to a time-derived cell, so we crop on the position
        // the renderer actually resolved (written back to last_mascot_pos), not
        // a precomputed layout point. pos is the logical half-block buffer (1px
        // per cell across, 2px down — same convention as compute_crop_rect).
        let m = draw_ctx.last_mascot_pos.as_ref().ok_or_else(|| {
            anyhow::anyhow!("--crop-mascot needs a visible mascot; pass --openclaw <state>")
        })?;
        Some(centered_crop(m.pos.x, m.pos.y / 2, cols, rows))
    } else {
        compute_crop_rect(&args, &scene, &store.history, cols, rows, now)?
    };

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

    fn crop_args(extra: &[&str]) -> SnapshotArgs {
        SnapshotArgs::try_parse_from([&["snapshot"], extra].concat()).unwrap()
    }

    #[test]
    fn crop_rect_centers_on_the_pantry_waypoint() {
        let now = SystemTime::now();
        let scene = sample_scene(now, 12, 12);
        let history = pixtuoid_scene::pose::PoseHistory::new();
        let args = crop_args(&["--crop-furniture", "pantry"]);
        let rect = compute_crop_rect(&args, &scene, &history, 192, 64, now)
            .unwrap()
            .expect("pantry crop");
        let layout =
            pixtuoid_scene::layout::SceneLayout::compute_with_seed(192, 126, Some(12), 0).unwrap();
        let pantry = layout
            .waypoints
            .iter()
            .find(|w| w.kind == pixtuoid_scene::layout::WaypointKind::Pantry)
            .unwrap();
        let (cx, cy) = (pantry.pos.x, pantry.pos.y / 2);
        assert!(rect.x <= cx && cx < rect.x + rect.width, "x not in crop");
        assert!(rect.y <= cy && cy < rect.y + rect.height, "y not in crop");
    }

    #[test]
    fn crop_rect_without_flags_is_none() {
        let now = SystemTime::now();
        let scene = sample_scene(now, 12, 12);
        let history = pixtuoid_scene::pose::PoseHistory::new();
        let args = crop_args(&[]);
        assert!(compute_crop_rect(&args, &scene, &history, 192, 64, now)
            .unwrap()
            .is_none());
    }

    #[test]
    fn crop_rect_fails_loudly_on_typos_and_unknown_agents() {
        let now = SystemTime::now();
        let scene = sample_scene(now, 12, 12);
        let history = pixtuoid_scene::pose::PoseHistory::new();

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
