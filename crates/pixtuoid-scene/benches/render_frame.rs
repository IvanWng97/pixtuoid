//! Whole-frame render benchmark over two axes: the two SIZES issue #900
//! measured (12 agents, busy and idle), and OCCUPANCY at the larger of them.
//! Size is the near-linear axis — #900 measured frame cost tracking pixel
//! count — so occupancy is the one that still teaches something: the per-agent
//! work is the part that isn't linear in pixels. The largest crowd carries an
//! idle twin for the same reason the size axis does: without it the delta can't
//! be split into headcount-driven and activity-driven halves.
//! Run via `just bench`; profile via
//! `cargo bench -p pixtuoid-scene --bench render_frame -- --profile-time 10`
//! under `samply record`. Numbers are LOCAL statistical evidence: shared-CI
//! wall-clock is noise, so CI runs this advisory-only.
//! A second group, `render_cutaway`, costs the 2.5D painter alone: the sim
//! window is stepped up front, so each iteration is paint only — what the
//! cutaway adds on top of the shared sim — at the pack's densest art, once at
//! noon and once at night, when the dark room recolours every pixel; an idle
//! office both ways too, painted whole and through `CutawayCanvas`.
//! Distinct instrument:
//! `crates/pixtuoid/examples/render_bench.rs` measures buffer-size SCALING
//! through the floating offscreen renderer for the 2.5D design gate.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use criterion::{Criterion, criterion_main};
use pixtuoid_core::id::AgentId;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::state::{ActivityState, GlobalDeskIndex, ToolKind};
use pixtuoid_core::{AgentSlot, SceneState};
use pixtuoid_scene::board::BoardModel;
use pixtuoid_scene::cutaway::canvas::CutawayCanvas;
use pixtuoid_scene::cutaway::paint::render_cutaway;
use pixtuoid_scene::display::{Office, Showing};
use pixtuoid_scene::floor::{
    CoffeeState, FloorCtx, FloorInputs, FloorMeta, FloorSession, FrameInputs, PetInputs,
    SteppedFloor, render_floor,
};
use pixtuoid_scene::layout::Size;
use pixtuoid_scene::localclock;
use pixtuoid_scene::render_scale::RenderScale;
use pixtuoid_scene::sky::{Weather, WeatherPolicy, hour_is_day};

// Inside a weather slot (`sky::WEATHER_CYCLE_MS`, crate-private) with room to
// spare, so the `SIM_WINDOW_FRAMES` × `FRAME_STEP_MS` window below never
// crosses a weather change.
const BASE_EPOCH_SECS: u64 = 1_700_000_200;
const SIM_WINDOW_FRAMES: u32 = 600;
const FRAME_STEP_MS: u64 = 100;
/// The DEFAULT floating window's office buffer (`pixtuoid`'s
/// `FLOATING_DEFAULT_W/H`, which `office_scale` leaves at scale 1). NOT a
/// ceiling: the TUI buffer is the terminal's own size and `office_scale`
/// divides by HEIGHT only, so a wide window or wide terminal exceeds it.
const FLOATING_DEFAULT: Size = Size { w: 360, h: 240 };
/// Crowds above the 12 the size axis fixes — a busy pod-farm and a near-full floor.
const OCCUPANCY: [usize; 2] = [32, 64];
/// The extent the cutaway's own paint tests lay out. Nothing ships a cutaway
/// extent yet — a painter will size it from its terminal or window.
const CUTAWAY_LOGICAL: Size = Size { w: 240, h: 144 };
/// Observed sim frames each cutaway case cycles through, so walks and bubbles
/// move under the painter.
const CUTAWAY_FRAMES: usize = 60;
/// One hour on each side of the sky's day/night boundary; `render_cutaway_frame`
/// pins which side each is on.
const NOON: u32 = 12;
const NIGHT: u32 = 23;

fn office_scene(n: usize, max_desks: usize, base: SystemTime, busy: bool) -> SceneState {
    let mut s = SceneState::uniform(max_desks);
    let kinds = [
        ToolKind::Bash,
        ToolKind::Edit,
        ToolKind::Read,
        ToolKind::Search,
        ToolKind::Task,
        ToolKind::Other,
    ];
    // The idle office's clocks sit far in the past so agents settle into the
    // deep-idle poses a real overnight office shows.
    let idle_epoch = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    // At n=12 this is exactly the 6/2/4 split the size cases have measured since #900.
    let active = n / 2;
    let waiting = active + n / 6;
    for i in 0..n {
        let id = AgentId::from_transcript_path(&format!("/p/{i}.jsonl"));
        let state = match i {
            _ if !busy || i >= waiting => ActivityState::Idle,
            _ if i < active => ActivityState::Active {
                tool_use_id: Some(Arc::from(format!("t{i}").as_str())),
                detail: Some(Arc::from("cargo test")),
                kind: kinds[i % kinds.len()],
            },
            _ => ActivityState::Waiting {
                reason: Arc::from("permission"),
            },
        };
        let stamp = if busy {
            base + Duration::from_secs(i as u64)
        } else {
            idle_epoch
        };
        let floor_idx = s.floor_of(GlobalDeskIndex(i));
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: Arc::from("cc"),
                session_id: Arc::from(format!("s{i}").as_str()),
                cwd: Arc::from(Path::new("/repo")),
                label: format!("a{i}").into(),
                state,
                state_started_at: stamp,
                created_at: stamp,
                last_event_at: stamp,
                exiting_at: None,
                pending_idle_at: None,
                desk_index: GlobalDeskIndex(i),
                floor_idx,
                tool_call_count: if busy { i as u32 * 7 } else { 0 },
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
                pid: None,
                model: None,
                effort: None,
                tokens_used: 0,
                last_usage: None,
            },
        );
    }
    s
}

fn render_frame(c: &mut Criterion) {
    let pack = pixtuoid_scene::pack::load_bundled_pack().expect("bundled pack");
    let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme");
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(BASE_EPOCH_SECS);
    let busy = office_scene(12, 16, base, true);
    let idle = office_scene(12, 16, base, false);
    let crowds: Vec<(usize, SceneState)> = OCCUPANCY
        .iter()
        .map(|&n| (n, office_scene(n, n, base, true)))
        .collect();
    let biggest = OCCUPANCY[OCCUPANCY.len() - 1];
    let idle_crowd = office_scene(biggest, biggest, base, false);
    // Every crowd must seat, else the case measures a smaller office than its name claims.
    let seats = pixtuoid_scene::floor::floor_capacity(
        FLOATING_DEFAULT.w,
        FLOATING_DEFAULT.h,
        pixtuoid_scene::floor::floor_seed(0),
    );
    assert!(
        seats >= biggest,
        "{FLOATING_DEFAULT:?} seats {seats} < {biggest}"
    );

    let mut cases: Vec<(String, &SceneState, Size)> = Vec::new();
    for (label, scene) in [("busy", &busy), ("idle", &idle)] {
        for size in [Size { w: 192, h: 160 }, FLOATING_DEFAULT] {
            cases.push((format!("{label}12_{}x{}", size.w, size.h), scene, size));
        }
    }
    let Size { w, h } = FLOATING_DEFAULT;
    for (n, scene) in &crowds {
        cases.push((format!("busy{n}_{w}x{h}"), scene, FLOATING_DEFAULT));
    }
    cases.push((
        format!("idle{biggest}_{w}x{h}"),
        &idle_crowd,
        FLOATING_DEFAULT,
    ));

    let mut group = c.benchmark_group("render_floor");
    for (name, scene, size) in cases {
        // Hoisted past criterion's per-sample closure; rebuilt at each wrap, where `now` steps back.
        let mut fctx = FloorCtx::new();
        let mut buf = RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 });
        let mut coffee = CoffeeState::new();
        let mut chitchat = HashMap::new();
        let mut i = 0u32;
        group.bench_function(name, |b| {
            b.iter(|| {
                if i == 0 {
                    fctx = FloorCtx::new();
                    coffee = CoffeeState::new();
                    chitchat.clear();
                }
                let now = base + Duration::from_millis(u64::from(i) * FRAME_STEP_MS);
                i = (i + 1) % SIM_WINDOW_FRAMES;
                render_floor(
                    &mut fctx,
                    &mut buf,
                    &mut coffee,
                    &mut chitchat,
                    FrameInputs {
                        world: FloorInputs {
                            scene,
                            pack: &pack,
                            now,
                            floor: FloorMeta::ground(),
                            pets: PetInputs::default(),
                        },
                        theme,
                        size,
                        debug_walkable: false,
                    },
                )
                .expect("layout")
            });
        });
    }
    group.finish();
}

fn render_cutaway_frame(c: &mut Criterion) {
    let pack = Arc::new(pixtuoid_scene::pack::load_bundled_pack().expect("bundled pack"));
    let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme");
    let scale = RenderScale::new(pack.max_density_variant().get()).expect("a nonzero density");
    // The case names are claims about the sky model; hold it to them.
    assert!(
        hour_is_day(NOON as f32) && !hour_is_day(NIGHT as f32),
        "NOON must be day and NIGHT night"
    );
    // Every agent must seat, else the case measures a smaller office than its name claims.
    let seats = pixtuoid_scene::floor::floor_capacity(
        CUTAWAY_LOGICAL.w,
        CUTAWAY_LOGICAL.h,
        pixtuoid_scene::floor::floor_seed(0),
    );
    assert!(seats >= 12, "{CUTAWAY_LOGICAL:?} seats {seats} < 12");
    // The sky otherwise picks its weather from the clock, so noon and night
    // would each also be a different weather.
    let meta = FloorMeta::ground().with_weather(WeatherPolicy::Forced(Weather::Clear));

    let mut group = c.benchmark_group("render_cutaway");
    let Size { w, h } = CUTAWAY_LOGICAL;
    for (busy, when, hour) in [
        (true, "noon", NOON),
        (true, "night", NIGHT),
        (false, "noon", NOON),
        (false, "night", NIGHT),
    ] {
        let base = localclock::at_hour(hour);
        let scene = office_scene(12, 16, base, busy);
        let mut session = FloorSession::new();
        let stepped: Vec<(SystemTime, SteppedFloor, BoardModel)> = (0..CUTAWAY_FRAMES as u64)
            .map(|i| {
                let now = base + Duration::from_millis(i * FRAME_STEP_MS);
                let floor = session
                    .step(
                        pixtuoid_scene::floor::FloorInputs {
                            scene: &scene,
                            pack: &pack,
                            now,
                            floor: meta,
                            pets: pixtuoid_scene::floor::PetInputs::default(),
                        },
                        CUTAWAY_LOGICAL,
                    )
                    .expect("the cutaway extent lays out");
                (now, floor, session.board(&scene, meta.motion, now))
            })
            .collect();
        let office = |layout| Office {
            layout,
            pack: &pack,
            theme,
            scale,
        };
        let label = if busy { "busy12" } else { "idle12" };
        let name = format!("{label}_{w}x{h}_x{}_{when}", scale.get());
        // Outside the bench closure, which criterion calls afresh per sample: a
        // live painter's recolour cache stays warm and its frames keep advancing.
        let mut buf = RgbBuffer::filled(
            scale.to_buffer(w),
            scale.to_buffer(h),
            theme.surface.bg_fallback,
        );
        let mut cache = pixtuoid_scene::cutaway::paint::CutawayCache::default();
        let mut i = 0;
        group.bench_function(&name, |b| {
            b.iter(|| {
                let (now, SteppedFloor { layout, frame }, board) = &stepped[i];
                i = (i + 1) % CUTAWAY_FRAMES;
                let showing = Showing {
                    floor: meta,
                    now: *now,
                    board,
                };
                render_cutaway(frame, office(layout), showing, &mut cache, &mut buf)
            });
        });
        if busy {
            continue;
        }
        // The same frames through the canvas, which paints only those that
        // change what the office shows.
        let mut canvas = CutawayCanvas::new(Arc::clone(&pack));
        let mut cache = pixtuoid_scene::cutaway::paint::CutawayCache::default();
        let mut i = 0;
        group.bench_function(format!("{name}_canvas"), |b| {
            b.iter(|| {
                let (now, floor, board) = &stepped[i];
                i = (i + 1) % CUTAWAY_FRAMES;
                let showing = Showing {
                    floor: meta,
                    now: *now,
                    board,
                };
                canvas.frame(floor, theme, scale, showing, &mut cache).dirty
            });
        });
    }
    group.finish();
}

// A module, because rustc ignores a lint attribute on the macro call itself.
#[expect(
    clippy::disallowed_methods,
    reason = "codspeed's `criterion_group!` reads CODSPEED_ENV and CODSPEED_CARGO_WORKSPACE_ROOT with `env::var`"
)]
mod group {
    use super::{render_cutaway_frame, render_frame};
    criterion::criterion_group!(benches, render_frame, render_cutaway_frame);
}
criterion_main!(group::benches);
