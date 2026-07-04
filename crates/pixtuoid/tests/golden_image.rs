//! Golden image regression tests.
//!
//! Render deterministic scenes and snapshot the pixel hash. Any visual
//! regression (missing sprite, wrong color, broken layout) changes the
//! hash. Tests only compare same-machine renders against each other
//! (eq/ne assertions), so timezone-dependent code paths like the sky
//! emitter's golden-hour blaze (`golden_hour_blaze`) don't cause
//! cross-platform failures.

mod common;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, SystemTime};

use pixtuoid::tui::renderer::draw_scene;
use pixtuoid_core::state::{ActivityState, ToolKind};
use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex, SceneState};
use pixtuoid_scene::embedded_pack::load_sprite_pack;
use pixtuoid_scene::theme;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_716_292_800)
}

fn empty_scene() -> SceneState {
    SceneState::uniform(12)
}

fn populated_scene(now: SystemTime) -> SceneState {
    let mut s = SceneState::uniform(12);
    let age_offset = Duration::from_secs(60);
    let labels = ["alice", "bob", "carol", "dave"];
    let states = [
        ActivityState::Active {
            tool_use_id: None,
            detail: Some("Edit src/main.rs".into()),
            kind: ToolKind::Edit,
        },
        ActivityState::Idle,
        ActivityState::Waiting {
            reason: "user input".into(),
        },
        ActivityState::Active {
            tool_use_id: None,
            detail: Some("Bash ls".into()),
            kind: ToolKind::Bash,
        },
    ];
    for (i, (label, state)) in labels.iter().zip(states).enumerate() {
        let id = AgentId::from_parts("cc", &format!("/tmp/test/{label}"));
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: "cc".into(),
                session_id: format!("sess-{i}").into(),
                cwd: std::path::PathBuf::from(format!("/tmp/test/{label}")).into(),
                label: (*label).into(),
                desk_index: GlobalDeskIndex(i),
                floor_idx: 0,
                state,
                created_at: now - age_offset,
                state_started_at: now,
                last_event_at: now,
                exiting_at: None,
                pending_idle_at: None,

                tool_call_count: i as u32,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
            },
        );
    }
    s
}

fn render_hash(scene: &SceneState, now: SystemTime, t: &theme::Theme, floor_seed: u64) -> u64 {
    let backend = TestBackend::new(96, 36);
    let mut term = Terminal::new(backend).unwrap();
    let pack = load_sprite_pack(None).unwrap();
    make_draw_ctx!(draw_ctx, theme: t, floor_seed: floor_seed);
    draw_scene(&mut term, scene, &pack, now, &mut draw_ctx).unwrap();

    let mut hasher = DefaultHasher::new();
    for px in draw_ctx.buf.as_slice() {
        px.r.hash(&mut hasher);
        px.g.hash(&mut hasher);
        px.b.hash(&mut hasher);
    }
    hasher.finish()
}

#[test]
fn golden_empty_office_is_deterministic() {
    let scene = empty_scene();
    let h1 = render_hash(&scene, now(), &theme::NORMAL, 0);
    let h2 = render_hash(&scene, now(), &theme::NORMAL, 0);
    assert_eq!(h1, h2, "empty office render is non-deterministic");
}

#[test]
fn golden_populated_vs_empty_differ() {
    let n = now();
    let h_empty = render_hash(&empty_scene(), n, &theme::NORMAL, 0);
    let h_pop = render_hash(&populated_scene(n), n, &theme::NORMAL, 0);
    assert_ne!(h_empty, h_pop, "populated and empty scenes look identical");
}

#[test]
fn golden_cyberpunk_vs_normal_differ() {
    let n = now();
    let scene = populated_scene(n);
    let h_normal = render_hash(&scene, n, &theme::NORMAL, 0);
    let h_cyber = render_hash(&scene, n, &theme::CYBERPUNK, 0);
    assert_ne!(
        h_normal, h_cyber,
        "normal and cyberpunk themes produce identical pixels"
    );
}
