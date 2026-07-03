//! Unit tests for `FrameCache`. The cache lives in front of `recolor_frame`,
//! which runs once per agent per frame (~30fps × N agents). A broken cache
//! either (a) misses on every call (latent perf regression, no visual bug)
//! or (b) hits when it shouldn't (stale recolored frames, wrong colors).
//! Both are hard to spot in a running TUI — covered here instead.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use pixtuoid_core::sprite::{Frame, Rgb};
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex, SceneState};
use pixtuoid_scene::frame_cache::{FrameCache, FrameKey};

/// Build a glow-less frame key (no test here varies glow_tint).
fn key(id: AgentId, anim_name: &'static str, frame_idx: usize, flip_x: bool) -> FrameKey {
    FrameKey {
        agent_id: id,
        anim_name,
        frame_idx,
        flip_x,
        glow_tint: None,
    }
}

fn dummy_frame(seed: u8) -> Frame {
    Frame::from_pixels(
        1,
        1,
        vec![Some(Rgb {
            r: seed,
            g: seed,
            b: seed,
        })],
    )
}

fn make_slot(id: AgentId) -> AgentSlot {
    let now = SystemTime::UNIX_EPOCH;
    AgentSlot {
        agent_id: id,
        source: Arc::from("claude-code"),
        session_id: Arc::from("s"),
        cwd: Arc::from(PathBuf::from("/x").as_path()),
        label: "x".into(),
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
    }
}

#[test]
fn get_or_make_caches_by_full_key() {
    use std::cell::Cell;

    let mut cache = FrameCache::new();
    let id = AgentId::from_transcript_path("/a.jsonl");
    let compute_calls = Cell::new(0u32);

    let f1 = cache
        .get_or_make(key(id, "walking", 0, false), || {
            compute_calls.set(compute_calls.get() + 1);
            dummy_frame(1)
        })
        .clone();
    assert_eq!(compute_calls.get(), 1);
    assert_eq!(f1.as_slice()[0], Some(Rgb { r: 1, g: 1, b: 1 }));

    // Same key — must hit.
    let f2 = cache
        .get_or_make(key(id, "walking", 0, false), || {
            compute_calls.set(compute_calls.get() + 1);
            dummy_frame(99)
        })
        .clone();
    assert_eq!(
        compute_calls.get(),
        1,
        "second lookup with same key must not recompute"
    );
    assert_eq!(f2.as_slice()[0], Some(Rgb { r: 1, g: 1, b: 1 }));

    // Different frame_idx — distinct entry.
    cache.get_or_make(key(id, "walking", 1, false), || {
        compute_calls.set(compute_calls.get() + 1);
        dummy_frame(2)
    });
    assert_eq!(compute_calls.get(), 2);

    // Different flip_x — distinct entry (mirrored walker caches separately).
    cache.get_or_make(key(id, "walking", 0, true), || {
        compute_calls.set(compute_calls.get() + 1);
        dummy_frame(3)
    });
    assert_eq!(compute_calls.get(), 3);

    // Different anim_name — distinct entry.
    cache.get_or_make(key(id, "seated", 0, false), || {
        compute_calls.set(compute_calls.get() + 1);
        dummy_frame(4)
    });
    assert_eq!(compute_calls.get(), 4);

    assert_eq!(cache.len(), 4);
}

#[test]
fn evict_missing_drops_entries_for_absent_agents() {
    let mut cache = FrameCache::new();
    let kept = AgentId::from_transcript_path("/kept.jsonl");
    let gone = AgentId::from_transcript_path("/gone.jsonl");

    cache.get_or_make(key(kept, "walking", 0, false), || dummy_frame(1));
    cache.get_or_make(key(gone, "walking", 0, false), || dummy_frame(2));
    cache.get_or_make(key(gone, "seated", 0, false), || dummy_frame(3));
    assert_eq!(cache.len(), 3);

    // Scene now contains only `kept`.
    let mut scene = SceneState::uniform(4);
    scene.agents.insert(kept, make_slot(kept));

    cache.evict_missing(&scene);

    assert_eq!(
        cache.len(),
        1,
        "two entries for the absent agent should be dropped"
    );
    // Surviving entry must be the kept one — exercise it.
    let _ = cache.get_or_make(key(kept, "walking", 0, false), || {
        panic!("evict must not have dropped the kept agent's entry")
    });
}
