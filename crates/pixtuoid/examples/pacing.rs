//! Frame pacing: drives the real TUI painter (`TuiRenderer` over ratatui's
//! crossterm encoder, or the cutaway's tile sink per image protocol) through a
//! fixed scenario and reports what a viewer feels: how long each frame takes,
//! split into compose / rasterize / encode / write, how many overrun the paint
//! interval, how even the interval between frames is, and how many bytes each
//! frame writes. Advisory, local: `just bench-pacing`.
//!
//! The scene advances a VIRTUAL clock one paint interval per frame, so every
//! case paints the same frames; the interval columns are therefore computed
//! from the measured render times under each loop model, not observed. The
//! one `real-clock` case runs the production loop's shape on the wall clock
//! to check that model once.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use pixtuoid::pacing::{Protocol, renderer};
use pixtuoid_core::state::{ActivityState, SceneState, ToolKind};
use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex};
use pixtuoid_scene::anim::{Motion, PAINT_FPS};
use pixtuoid_scene::look::spans;
use pixtuoid_scene::pet::{Pet, PetKind};
use pixtuoid_scene::sky::{Weather, WeatherPolicy, first_strike_after};
use tracing_subscriber::layer::{Context as LayerContext, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// The scenario's length.
const SCENARIO: Duration = Duration::from_secs(10);
/// Agents in the office: more than one floor seats, so a slide has a floor.
const AGENTS: usize = 12;
/// Desks per floor.
const FLOOR_DESKS: usize = 8;
/// When the strike lands, from the scenario's start.
const STRIKE_AT: Duration = Duration::from_secs(2);
/// When the view slides to the second floor.
const SLIDE_AT: Duration = Duration::from_secs(4);
/// When the terminal shrinks, and by how many cells each way.
const RESIZE_AT: Duration = Duration::from_secs(7);
const RESIZE_BY: (u16, u16) = (12, 4);
/// The terminal every case starts in.
const TERMINAL: (u16, u16) = (160, 50);
/// The half-block's cell, and the cutaway's: the cutaway always renders at the
/// pack's densest art, so the cell picks its upscale (none, then 2x).
const HALF_BLOCK_CELL: (u16, u16) = (8, 16);
const CUTAWAY_CELLS: [(u16, u16); 2] = [(4, 8), (8, 16)];

/// Span time per name, summed since the last take.
#[derive(Clone, Default)]
struct SpanClock(Arc<Mutex<HashMap<&'static str, Duration>>>);

impl SpanClock {
    fn take(&self) -> HashMap<&'static str, Duration> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// When the span was entered, kept in its extensions.
struct Entered(Instant);

impl<S> tracing_subscriber::Layer<S> for SpanClock
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_enter(&self, id: &tracing::span::Id, ctx: LayerContext<'_, S>) {
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().replace(Entered(Instant::now()));
        }
    }

    fn on_exit(&self, id: &tracing::span::Id, ctx: LayerContext<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let Some(Entered(at)) = span.extensions_mut().remove::<Entered>() else {
            return;
        };
        *self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(span.name())
            .or_default() += at.elapsed();
    }
}

/// One frame's measurements.
#[derive(Clone, Copy, Default)]
struct Frame {
    total: Duration,
    compose: Duration,
    rasterize: Duration,
    encode: Duration,
    write: Duration,
    bytes: u64,
}

struct Case {
    name: String,
    protocol: Protocol,
    cell: (u16, u16),
    motion: Motion,
}

fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    for motion in [Motion::Full, Motion::Calm] {
        out.push(Case {
            name: format!("half-block classic {motion:?}"),
            protocol: Protocol::HalfBlock,
            cell: HALF_BLOCK_CELL,
            motion,
        });
        for (protocol, label) in [
            (Protocol::Kitty, "kitty"),
            (Protocol::Sixel, "sixel"),
            (Protocol::Iterm2, "iterm2"),
        ] {
            for cell in CUTAWAY_CELLS {
                out.push(Case {
                    name: format!("{label} cutaway cell {}x{} {motion:?}", cell.0, cell.1),
                    protocol,
                    cell,
                    motion,
                });
            }
        }
    }
    out
}

/// The office at `start`: agents walking in through the scenario, some long
/// at their desks (and so wandering), the rest working.
fn office(start: SystemTime) -> SceneState {
    let mut scene = SceneState::uniform(FLOOR_DESKS);
    let long_ago = start - Duration::from_secs(600);
    for i in 0..AGENTS {
        let id = AgentId::from_transcript_path(&format!("/pacing/a{i}.jsonl"));
        let (state, started) = match i % 3 {
            0 => (
                ActivityState::Active {
                    tool_use_id: Some(format!("tu_{i}").into()),
                    detail: Some("Edit: src/lib.rs".into()),
                    kind: ToolKind::Edit,
                },
                long_ago,
            ),
            1 => (ActivityState::Idle, long_ago),
            // Arriving one by one across the scenario: entry walks.
            _ => (
                ActivityState::Idle,
                start + SCENARIO.mul_f64(i as f64 / AGENTS as f64),
            ),
        };
        scene.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: Arc::from("claude-code"),
                session_id: Arc::from(format!("pacing-{i:04x}").as_str()),
                cwd: Arc::from(Path::new("/pacing")),
                label: format!("a{i}").into(),
                state,
                state_started_at: started,
                created_at: started,
                last_event_at: started,
                exiting_at: None,
                pending_idle_at: None,
                desk_index: GlobalDeskIndex(i),
                floor_idx: scene.floor_of(GlobalDeskIndex(i)),
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
                model: None,
                pid: None,
                effort: None,
                tokens_used: 0,
                last_usage: None,
            },
        );
    }
    scene
}

/// The scenario's start for `motion`: a strike lands [`STRIKE_AT`] in.
fn start_for(motion: Motion) -> Result<SystemTime> {
    let base = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let strike = first_strike_after(base + STRIKE_AT, motion).context("a moving tier strikes")?;
    Ok(strike - STRIKE_AT)
}

/// Run one case on the virtual clock: every frame's measurements.
fn run(
    case: &Case,
    clock: &SpanClock,
    pack: &Arc<pixtuoid_core::sprite::format::Pack>,
) -> Result<Vec<Frame>> {
    let tick = Duration::from_secs(1) / PAINT_FPS;
    let pets = vec![Pet::defaulted(PetKind::Cat), Pet::defaulted(PetKind::Dog)];
    let (mut r, wire) = renderer(
        case.protocol,
        TERMINAL.0,
        TERMINAL.1,
        case.cell,
        pets,
        Arc::clone(pack),
    )?;
    r.set_weather(WeatherPolicy::Forced(Weather::Storm));
    r.set_motion(case.motion);
    let start = start_for(case.motion)?;
    let scene = office(start);
    let frames = SCENARIO.as_nanos() / tick.as_nanos();
    let mut out = Vec::new();
    for n in 0..frames {
        let at = tick * u32::try_from(n)?;
        let now = start + at;
        if at == SLIDE_AT {
            r.navigate_floor(1, now);
        }
        if at == RESIZE_AT {
            r.terminal
                .backend_mut()
                .resize(TERMINAL.0 - RESIZE_BY.0, TERMINAL.1 - RESIZE_BY.1);
        }
        wire.take();
        clock.take();
        let begun = Instant::now();
        r.render(&scene, pack, now)?;
        let total = begun.elapsed();
        let (bytes, write_ns) = wire.take();
        let spans = clock.take();
        let compose = spans.get(spans::COMPOSE).copied().unwrap_or_default();
        let rasterize = spans.get(spans::RASTERIZE).copied().unwrap_or_default();
        let write = Duration::from_nanos(write_ns);
        out.push(Frame {
            total,
            compose,
            rasterize,
            encode: total.saturating_sub(compose + rasterize + write),
            write,
            bytes,
        });
    }
    Ok(out)
}

/// The production loop's shape on the wall clock: render, then wait a paint
/// interval for input that never comes. Each frame's interval, observed.
fn real_clock(
    pack: &Arc<pixtuoid_core::sprite::format::Pack>,
) -> Result<(Vec<Duration>, Vec<Duration>)> {
    let tick = Duration::from_secs(1) / PAINT_FPS;
    let (mut r, _wire) = renderer(
        Protocol::HalfBlock,
        TERMINAL.0,
        TERMINAL.1,
        HALF_BLOCK_CELL,
        vec![Pet::defaulted(PetKind::Cat)],
        Arc::clone(pack),
    )?;
    r.set_weather(WeatherPolicy::Forced(Weather::Storm));
    r.set_motion(Motion::Full);
    let start = start_for(Motion::Full)?;
    let scene = office(start);
    let clock = Instant::now();
    let (mut renders, mut intervals) = (Vec::new(), Vec::new());
    let mut last = None;
    while clock.elapsed() < SCENARIO {
        let begun = Instant::now();
        if let Some(prev) = last.replace(begun) {
            intervals.push(begun - prev);
        }
        r.render(&scene, pack, start + clock.elapsed())?;
        renders.push(begun.elapsed());
        std::thread::sleep(tick);
    }
    Ok((renders, intervals))
}

/// The `p`th percentile of `xs`, nearest rank.
fn pct(xs: &[Duration], p: f64) -> Duration {
    let mut v = xs.to_vec();
    v.sort();
    let i = ((p / 100.0) * v.len() as f64).ceil() as usize;
    v.get(i.saturating_sub(1)).copied().unwrap_or_default()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// Each frame's interval under the production loop today: render, then a full
/// paint interval of polling.
fn sleep_loop(renders: &[Duration], tick: Duration) -> Vec<Duration> {
    renders.iter().map(|&r| r + tick).collect()
}

/// Each frame's interval under a deadline-paced loop: the next frame starts at
/// the previous deadline plus a paint interval, or as soon as the render ends
/// when it overran, re-anchoring there rather than bursting to catch up.
fn deadline_loop(renders: &[Duration], tick: Duration) -> Vec<Duration> {
    renders.iter().map(|&r| r.max(tick)).collect()
}

fn stats(xs: &[Duration]) -> serde_json::Value {
    serde_json::json!({
        "p50_ms": ms(pct(xs, 50.0)),
        "p95_ms": ms(pct(xs, 95.0)),
        "p99_ms": ms(pct(xs, 99.0)),
        "max_ms": ms(xs.iter().copied().max().unwrap_or_default()),
    })
}

fn main() -> Result<()> {
    let clock = SpanClock::default();
    let subscriber = tracing_subscriber::registry().with(clock.clone()).with(
        tracing_subscriber::filter::Targets::new()
            .with_target("pixtuoid_scene::look", tracing::Level::TRACE),
    );
    tracing::subscriber::set_global_default(subscriber)?;
    let pack = Arc::new(pixtuoid_scene::pack::load_bundled_pack()?);
    let tick = Duration::from_secs(1) / PAINT_FPS;
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(
        stdout,
        "paint interval {:.1} ms (1 s / PAINT_FPS); intervals and jitter are COMPUTED from render times\n",
        ms(tick)
    );
    let _ = writeln!(
        stdout,
        "{:<38} {:>7} {:>7} {:>7} {:>7} | {:>6} {:>6} {:>6} {:>6} | {:>5} | {:>13} {:>13} | {:>9} {:>9}",
        "case",
        "p50",
        "p95",
        "p99",
        "max",
        "comp",
        "rast",
        "enc",
        "write",
        "over%",
        "now p95/jit",
        "deadl p95/jit",
        "B p50",
        "B p95"
    );
    let mut report = Vec::new();
    for case in cases() {
        let frames = run(&case, &clock, &pack)?;
        let col = |f: fn(&Frame) -> Duration| frames.iter().map(f).collect::<Vec<_>>();
        let total = col(|f| f.total);
        let over = total.iter().filter(|&&t| t > tick).count() as f64 * 100.0 / total.len() as f64;
        let now_loop = sleep_loop(&total, tick);
        let deadline = deadline_loop(&total, tick);
        let jitter = |xs: &[Duration]| pct(xs, 95.0).saturating_sub(pct(xs, 50.0));
        let mut bytes: Vec<u64> = frames.iter().map(|f| f.bytes).collect();
        bytes.sort_unstable();
        let byte_pct = |p: f64| {
            let i = ((p / 100.0) * bytes.len() as f64).ceil() as usize;
            bytes.get(i.saturating_sub(1)).copied().unwrap_or_default()
        };
        let _ = writeln!(
            stdout,
            "{:<38} {:>7.2} {:>7.2} {:>7.2} {:>7.2} | {:>6.2} {:>6.2} {:>6.2} {:>6.2} | {:>5.1} | {:>6.1}/{:>6.2} {:>6.1}/{:>6.2} | {:>9} {:>9}",
            case.name,
            ms(pct(&total, 50.0)),
            ms(pct(&total, 95.0)),
            ms(pct(&total, 99.0)),
            ms(total.iter().copied().max().unwrap_or_default()),
            ms(pct(&col(|f| f.compose), 50.0)),
            ms(pct(&col(|f| f.rasterize), 50.0)),
            ms(pct(&col(|f| f.encode), 50.0)),
            ms(pct(&col(|f| f.write), 50.0)),
            over,
            ms(pct(&now_loop, 95.0)),
            ms(jitter(&now_loop)),
            ms(pct(&deadline, 95.0)),
            ms(jitter(&deadline)),
            byte_pct(50.0),
            byte_pct(95.0),
        );
        report.push(serde_json::json!({
            "case": case.name,
            "frames": frames.len(),
            "frame": stats(&total),
            "compose": stats(&col(|f| f.compose)),
            "rasterize": stats(&col(|f| f.rasterize)),
            "encode": stats(&col(|f| f.encode)),
            "write": stats(&col(|f| f.write)),
            "over_budget_pct": over,
            "interval_render_then_poll": stats(&now_loop),
            "jitter_render_then_poll_ms": ms(jitter(&now_loop)),
            "interval_deadline": stats(&deadline),
            "jitter_deadline_ms": ms(jitter(&deadline)),
            "bytes_p50": byte_pct(50.0),
            "bytes_p95": byte_pct(95.0),
        }));
    }
    let (renders, observed) = real_clock(&pack)?;
    let modeled = sleep_loop(&renders, tick);
    let _ = writeln!(
        stdout,
        "\nreal-clock half-block Full: interval OBSERVED p50/p95/p99 {:.2}/{:.2}/{:.2} ms vs COMPUTED {:.2}/{:.2}/{:.2} ms",
        ms(pct(&observed, 50.0)),
        ms(pct(&observed, 95.0)),
        ms(pct(&observed, 99.0)),
        ms(pct(&modeled, 50.0)),
        ms(pct(&modeled, 95.0)),
        ms(pct(&modeled, 99.0)),
    );
    report.push(serde_json::json!({
        "case": "real-clock half-block Full",
        "interval_observed": stats(&observed),
        "interval_computed": stats(&modeled),
    }));
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/pacing");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("report.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&serde_json::json!({
            "paint_interval_ms": ms(tick),
            "scenario_s": SCENARIO.as_secs_f64(),
            "intervals_are": "computed from render times, but for the real-clock case",
            "cases": report,
        }))?,
    )?;
    let _ = writeln!(stdout, "\nreport: {}", path.display());
    Ok(())
}
