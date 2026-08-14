//! Whole-frame render benchmark over two axes: the two SIZES issue #900
//! measured (12 agents, busy and idle), and OCCUPANCY at the larger of them.
//! Size is the saturated axis — #900 put frame cost at 2.814x for 2.812x the
//! pixels — and 360x240 is where it stops: `office_scale` pins the floating
//! window's buffer near 180px tall, so a bigger display renders a SMALLER
//! office. Occupancy is what still varies in real use, and the per-agent work
//! (z-sort, routing, chitchat) is the part that isn't linear in pixels.
//! Run via `just bench`; profile via
//! `cargo bench -p pixtuoid-scene --bench render_frame -- --profile-time 10`
//! under `samply record`. Numbers are LOCAL statistical evidence (criterion's
//! own FAQ warns shared-CI wall-clock is noise) — CI runs this advisory-only.
//! The pack is the compiled-in default, never the operator's XDG pack, so two
//! machines measure the same drawable set. Distinct instrument:
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
use pixtuoid_scene::floor::{render_floor, CoffeeState, FloorCtx, FloorMeta, FrameInputs};
use pixtuoid_scene::layout::Size;

// 400 s into `weather_state`'s 600 s bucket, so the 60 s simulated window
// below never crosses a weather change.
const BASE_EPOCH_SECS: u64 = 1_700_000_200;
const SIM_WINDOW_FRAMES: u32 = 600;
const FRAME_STEP_MS: u64 = 100;
/// The largest office buffer any painter produces (`FLOATING_DEFAULT_W/H`, and
/// the scale-1 case of `office_scale`).
const CEILING: (u16, u16) = (360, 240);
/// Crowds above the 12 the size axis fixes — a busy pod-farm and a full floor.
const OCCUPANCY: [usize; 2] = [32, 64];

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
    // Half the office mid-tool, a sixth parked on a prompt — at n=12 this is
    // exactly the 6/2/4 split the size cases have measured since #900.
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
    let pack = pixtuoid_scene::embedded_pack::embedded_default_pack().expect("embedded pack");
    let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme");
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(BASE_EPOCH_SECS);
    let busy = office_scene(12, 16, base, true);
    let idle = office_scene(12, 16, base, false);
    // A 360x240 floor lays out 80 desks, so both crowds seat fully.
    let crowds: Vec<(usize, SceneState)> = OCCUPANCY
        .iter()
        .map(|&n| (n, office_scene(n, n, base, true)))
        .collect();

    let mut cases: Vec<(String, &SceneState, u16, u16)> = Vec::new();
    for (label, scene) in [("busy", &busy), ("idle", &idle)] {
        for (w, h) in [(192u16, 160u16), CEILING] {
            cases.push((format!("{label}12_{w}x{h}"), scene, w, h));
        }
    }
    for (n, scene) in &crowds {
        let (w, h) = CEILING;
        cases.push((format!("busy{n}_{w}x{h}"), scene, w, h));
    }

    let mut group = c.benchmark_group("render_floor");
    for (name, scene, w, h) in cases {
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
                        size: Size { w, h },
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

criterion_group!(benches, render_frame);
criterion_main!(benches);
