//! Image regression tests for the pixel painter.
//!
//! These tests render deterministic scenes through `draw_scene` and compare
//! pixel-buffer hashes. They complement `snapshot_regression.rs` (which
//! already covers determinism and time-of-day sensitivity) by exercising
//! floor variants, weather cycles, and theme switching.

mod common;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use pixtuoid::tui::renderer::draw_scene;
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex, SceneState};
use pixtuoid_scene::embedded_pack::load_sprite_pack;
use pixtuoid_scene::floor::FloorMeta;
use pixtuoid_scene::theme::{self, Theme};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn fixture_scene(now: SystemTime) -> SceneState {
    let mut s = SceneState::uniform(12);
    let age_offset = Duration::from_secs(60);
    let cases: &[(&str, ActivityState)] = &[
        (
            "agent-a",
            ActivityState::Active {
                tool_use_id: Some("tu_a".into()),
                detail: Some("Write".into()),
            },
        ),
        ("agent-b", ActivityState::Idle),
        (
            "agent-c",
            ActivityState::Waiting {
                reason: "perm?".into(),
            },
        ),
        ("agent-d", ActivityState::Idle),
    ];
    for (i, (key, state)) in cases.iter().enumerate() {
        let id = AgentId::from_transcript_path(&format!("/demo/{key}.jsonl"));
        let created_at = now - age_offset;
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: Arc::from("claude-code"),
                session_id: Arc::from(format!("session-{i}").as_str()),
                cwd: Arc::from(PathBuf::from("/demo").as_path()),
                label: Arc::from(*key),
                state: state.clone(),
                state_started_at: now,
                last_event_at: now,
                created_at,
                exiting_at: None,
                pending_idle_at: None,

                desk_index: GlobalDeskIndex(i),
                floor_idx: 0,
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
            },
        );
    }
    s
}

/// Render a scene and return a hash of the pixel buffer. Parameterised over
/// theme and floor metadata so tests can compare across configurations.
fn render_hash(scene: &SceneState, now: SystemTime, theme: &Theme, floor: FloorMeta) -> u64 {
    let backend = TestBackend::new(96, 36);
    let mut term = Terminal::new(backend).unwrap();
    let pack = load_sprite_pack(None).unwrap();
    make_draw_ctx!(draw_ctx, theme: theme);
    draw_ctx.floor = floor;
    draw_scene(&mut term, scene, &pack, now, &mut draw_ctx).unwrap();

    let mut hasher = DefaultHasher::new();
    for px in draw_ctx.buf.as_slice() {
        px.r.hash(&mut hasher);
        px.g.hash(&mut hasher);
        px.b.hash(&mut hasher);
    }
    hasher.finish()
}

// --- Floor variant visual difference -----------------------------------------

#[test]
fn floor_seed_affects_render() {
    // Different floor seeds produce different room layouts / decoration
    // rotations. Seed 0 (ground) vs seed from floor_idx=2 should differ.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_716_286_800);
    let scene = fixture_scene(now);

    let ground = FloorMeta::ground();
    let upper = FloorMeta::for_floor(2, 4);

    let hash_ground = render_hash(&scene, now, &theme::NORMAL, ground);
    let hash_upper = render_hash(&scene, now, &theme::NORMAL, upper);

    assert_ne!(
        hash_ground, hash_upper,
        "ground floor and floor 2 produced identical pixels -- floor seed has no effect"
    );
}

// --- Weather affects render --------------------------------------------------

#[test]
fn weather_cycle_affects_render() {
    // Weather state is derived from wallclock / 600 (10 min cycles). Two
    // timestamps 20 minutes apart should hit different weather variants
    // (assuming splitmix64 doesn't collide on adjacent inputs).
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_716_286_800);
    let later = base + Duration::from_secs(20 * 60);
    let scene = fixture_scene(base);

    let hash_a = render_hash(&scene, base, &theme::NORMAL, FloorMeta::ground());
    // Re-create scene with `later` so `created_at` stays before `now`.
    let scene_later = fixture_scene(later);
    let hash_b = render_hash(&scene_later, later, &theme::NORMAL, FloorMeta::ground());

    assert_ne!(
        hash_a, hash_b,
        "render is identical 20 minutes apart -- weather cycle appears bypassed"
    );
}

// --- Theme affects render ----------------------------------------------------

#[test]
fn theme_affects_render() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_716_286_800);
    let scene = fixture_scene(now);
    let floor = FloorMeta::ground();

    let hash_normal = render_hash(&scene, now, &theme::NORMAL, floor);
    let hash_cyberpunk = render_hash(&scene, now, &theme::CYBERPUNK, floor);

    assert_ne!(
        hash_normal, hash_cyberpunk,
        "NORMAL and CYBERPUNK themes produced identical pixels"
    );
}

#[test]
fn all_themes_render_distinctly() {
    // Verify every built-in theme produces a unique pixel hash. Guards
    // against a copy-paste theme that is visually identical to another.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_716_286_800);
    let scene = fixture_scene(now);
    let floor = FloorMeta::ground();

    let hashes: Vec<(&str, u64)> = theme::ALL_THEMES
        .iter()
        .map(|t| (t.name, render_hash(&scene, now, t, floor)))
        .collect();

    for i in 0..hashes.len() {
        for j in (i + 1)..hashes.len() {
            assert_ne!(
                hashes[i].1, hashes[j].1,
                "themes '{}' and '{}' produced identical pixels",
                hashes[i].0, hashes[j].0
            );
        }
    }
}
