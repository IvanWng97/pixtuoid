//! Whole-frame render benchmark: `render_floor` on a 12-agent office at the
//! two sizes issue #900 measured, in a busy (mixed-activity) and an idle
//! configuration. Run via `just bench`; profile via
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
    for i in 0..n {
        let id = AgentId::from_transcript_path(&format!("/p/{i}.jsonl"));
        let state = match i {
            0..=5 if busy => ActivityState::Active {
                tool_use_id: Some(Arc::from(format!("t{i}").as_str())),
                detail: Some(Arc::from("cargo test")),
                kind: kinds[i % kinds.len()],
            },
            6 | 7 if busy => ActivityState::Waiting {
                reason: Arc::from("permission"),
            },
            _ => ActivityState::Idle,
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

    let mut group = c.benchmark_group("render_floor");
    for (label, scene) in [("busy", &busy), ("idle", &idle)] {
        for (w, h) in [(192u16, 160u16), (360, 240)] {
            group.bench_function(format!("{label}12_{w}x{h}"), |b| {
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
    }
    group.finish();
}

criterion_group!(benches, render_frame);
criterion_main!(benches);
