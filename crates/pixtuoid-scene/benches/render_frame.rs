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
//! window is observed up front, so each iteration is paint only — the part the
//! cutaway adds on top of the shared sim — at the scale and extent it ships at,
//! once at noon and once at night (the night room lights its lamps).
//! Distinct instrument:
//! `crates/pixtuoid/examples/render_bench.rs` measures buffer-size SCALING
//! through the floating offscreen renderer for the 2.5D design gate.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use criterion::{criterion_group, criterion_main, Criterion};
use pixtuoid_core::id::AgentId;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::state::{ActivityState, GlobalDeskIndex, ToolKind};
use pixtuoid_core::{AgentSlot, SceneState};
use pixtuoid_scene::cutaway::paint::{render_cutaway, Office};
use pixtuoid_scene::floor::{
    render_floor, CoffeeState, FloorCtx, FloorMeta, FloorSession, FrameInputs, ObservedFloor,
};
use pixtuoid_scene::layout::Size;
use pixtuoid_scene::render_scale::RenderScale;

// Inside a weather slot (`sky::WEATHER_CYCLE_SECS`, crate-private) with room to
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
/// The cutaway's logical office and the density it paints it at.
const CUTAWAY_LOGICAL: Size = Size { w: 240, h: 144 };
const CUTAWAY_SCALE: u16 = 4;
/// Observed sim frames each cutaway case cycles through — enough that walks
/// and bubbles move under the painter, few enough to hold in memory at once.
const CUTAWAY_FRAMES: u32 = 60;
/// Local wall-clock hours: the sky and the room's lighting decode `now`
/// through `chrono::Local`, so a fixed epoch would be a different hour in
/// every `$TZ`.
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
    let pack = pixtuoid_scene::embedded_pack::load_sprite_pack(
        pixtuoid_scene::embedded_pack::PackSource::Bundled,
    )
    .expect("embedded pack");
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
        group.bench_function(name, |b| {
            let mut fctx = FloorCtx::new();
            let mut buf = RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 });
            let mut coffee = CoffeeState::new();
            let mut chitchat = HashMap::new();
            let mut i = 0u32;
            b.iter(|| {
                i = (i + 1) % SIM_WINDOW_FRAMES;
                render_floor(
                    &mut fctx,
                    &mut buf,
                    &mut coffee,
                    &mut chitchat,
                    FrameInputs {
                        scene,
                        pack: &pack,
                        theme,
                        now: base + Duration::from_millis(u64::from(i) * FRAME_STEP_MS),
                        size,
                        floor_meta: FloorMeta::ground(),
                        active_pet: None,
                        floor_pet: None,
                        debug_walkable: false,
                    },
                )
                .expect("layout")
            });
        });
    }
    group.finish();
}

/// Local `hour:00` on a fixed January day.
fn local_hour(hour: u32) -> SystemTime {
    use chrono::TimeZone;
    chrono::Local
        .with_ymd_and_hms(2026, 1, 1, hour, 0, 0)
        .single()
        .expect("a fixed January local time is unambiguous")
        .into()
}

fn render_cutaway_frame(c: &mut Criterion) {
    let pack = pixtuoid_scene::embedded_pack::load_sprite_pack(
        pixtuoid_scene::embedded_pack::PackSource::Bundled,
    )
    .expect("embedded pack");
    let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme");
    let scale = RenderScale::new(CUTAWAY_SCALE).expect("nonzero scale");
    // The sky otherwise picks its weather from the clock, so noon and night
    // would each also be a different weather.
    pixtuoid_scene::pixel_painter::force_weather(Some("clear")).expect("clear is a weather");
    let meta = FloorMeta::ground();

    let mut group = c.benchmark_group("render_cutaway");
    for (label, hour) in [("noon", NOON), ("night", NIGHT)] {
        let base = local_hour(hour);
        let scene = office_scene(12, 16, base, true);
        let mut session = FloorSession::new();
        let observed: Vec<ObservedFloor> = (0..CUTAWAY_FRAMES)
            .map(|i| {
                let now = base + Duration::from_millis(u64::from(i) * FRAME_STEP_MS);
                session
                    .observe(&scene, &pack, CUTAWAY_LOGICAL, meta, now)
                    .expect("the cutaway extent lays out")
            })
            .collect();
        let Size { w, h } = CUTAWAY_LOGICAL;
        group.bench_function(format!("busy12_{w}x{h}_x{CUTAWAY_SCALE}_{label}"), |b| {
            let mut buf = RgbBuffer::filled(
                scale.to_buffer(w),
                scale.to_buffer(h),
                theme.surface.bg_fallback,
            );
            let mut cache = pixtuoid_scene::frame_cache::FrameCache::new();
            let mut i = 0u32;
            b.iter(|| {
                let ObservedFloor { layout, frame } = &observed[i as usize];
                let now = base + Duration::from_millis(u64::from(i) * FRAME_STEP_MS);
                i = (i + 1) % CUTAWAY_FRAMES;
                render_cutaway(
                    frame,
                    Office {
                        layout,
                        pack: &pack,
                        theme,
                        scale,
                    },
                    meta,
                    now,
                    &mut cache,
                    &mut buf,
                )
            });
        });
    }
    group.finish();
    pixtuoid_scene::pixel_painter::force_weather(None).expect("None always resets");
}

criterion_group!(benches, render_frame, render_cutaway_frame);
criterion_main!(benches);
