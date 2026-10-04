use std::path::Path;
use std::time::{Duration, SystemTime};

use pixtuoid_core::state::{ActivityState, GlobalDeskIndex};
use pixtuoid_core::{AgentId, AgentSlot, SceneState};

use super::*;
use crate::floor::{FloorMeta, FrameInputs, PetInputs, render_floor};

const SIZE: Size = Size { w: 192, h: 160 };

fn theme() -> &'static Theme {
    crate::theme::theme_by_name("normal").expect("normal theme")
}

/// Agents entering (the door's walk) and long idle (wander trips, coffee among them).
fn office(t0: SystemTime) -> SceneState {
    let mut scene = SceneState::uniform(8);
    for i in 0..6 {
        let id = AgentId::from_transcript_path(&format!("/look/{i}.jsonl"));
        let started = if i < 2 {
            t0
        } else {
            t0 - Duration::from_secs(600)
        };
        scene.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: std::sync::Arc::from("claude-code"),
                session_id: std::sync::Arc::from(format!("s{i}").as_str()),
                cwd: std::sync::Arc::from(Path::new("/repo")),
                label: format!("a{i}").into(),
                state: ActivityState::Idle,
                state_started_at: started,
                created_at: started,
                last_event_at: started,
                exiting_at: None,
                pending_idle_at: None,
                desk_index: GlobalDeskIndex(i),
                floor_idx: 0,
                tool_call_count: 0,
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
    scene
}

fn inputs<'a>(scene: &'a SceneState, pack: &'a Pack, now: SystemTime) -> RenderInputs<'a> {
    RenderInputs {
        world: FloorInputs {
            scene,
            pack,
            now,
            floor: FloorMeta::ground(),
            pets: PetInputs::default(),
        },
        theme: theme(),
        size: SIZE,
        place: Place::default(),
        debug_walkable: false,
    }
}

/// The entry draws the classic exactly as the old seam (`render_floor`) did,
/// and leaves the office and floor stores as it left them — the coffee every
/// carrier stamps and the door's clamp included — frame after frame.
#[test]
fn the_classic_entry_is_the_old_seam_frame_for_frame() {
    let pack = Arc::new(crate::pack::test_default_pack());
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let scene = office(t0);
    let (mut old_floor, mut old_buf, mut old_office) = (
        FloorCtx::new(),
        RgbBuffer::filled(0, 0, theme().surface.bg_fallback),
        PerOffice::new(),
    );
    let (mut floor, mut raster, mut new_office) = (
        FloorCtx::new(),
        Raster::new(Arc::clone(&pack)),
        PerOffice::new(),
    );
    let mut coffee_seen = false;
    let mut door_seen = false;
    for step in 0..1_200u64 {
        let now = t0 + Duration::from_millis(step * 500);
        let old = render_floor(
            &mut old_floor,
            &mut old_buf,
            &mut old_office.coffee,
            &mut old_office.chitchat,
            FrameInputs {
                world: inputs(&scene, &pack, now).world,
                theme: theme(),
                size: SIZE,
                debug_walkable: false,
            },
        )
        .expect("lays out");
        let new = render(
            &mut floor,
            &mut raster,
            &mut new_office,
            Look::Classic,
            inputs(&scene, &pack, now),
        )
        .expect("lays out");
        assert!(
            new.pixels.as_slice() == old_buf.as_slice(),
            "step {step}: the frames differ"
        );
        assert_eq!(
            new.occupied_waypoints, old.occupied_waypoints,
            "step {step}"
        );
        assert_eq!(
            new_office.coffee.map(),
            old_office.coffee.map(),
            "step {step}"
        );
        assert_eq!(
            floor.door_anim_max_ms, old_floor.door_anim_max_ms,
            "step {step}"
        );
        coffee_seen |= !new_office.coffee.map().is_empty();
        door_seen |= floor.door_anim_max_ms > 0;
        if coffee_seen && door_seen {
            break;
        }
    }
    assert!(
        door_seen,
        "no frame walked the door, so its clamp went untested"
    );
    assert!(
        coffee_seen,
        "no carrier fetched coffee, so its stamp went untested"
    );
}

/// A floor that switches looks repaints the whole frame on each switch and keeps
/// each look's raster, so switching back rebuilds nothing.
#[test]
fn a_floor_switching_looks_repaints_whole_and_keeps_each_raster() {
    let pack = Arc::new(crate::pack::test_default_pack());
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let scene = office(t0);
    let (mut floor, mut raster, mut office) = (
        FloorCtx::new(),
        Raster::new(Arc::clone(&pack)),
        PerOffice::new(),
    );
    let scale = RenderScale::new(2).expect("nonzero");
    let cutaway = Look::Cutaway { scale };
    let mut frame = |raster: &mut Raster, look, ms: u64| {
        let r = render(
            &mut floor,
            raster,
            &mut office,
            look,
            inputs(&scene, &pack, t0 + Duration::from_millis(ms)),
        )
        .expect("lays out");
        (r.dirty, (r.pixels.width(), r.pixels.height()))
    };
    let classic_size = (SIZE.w, SIZE.h);
    let cutaway_size = (scale.to_buffer(SIZE.w), scale.to_buffer(SIZE.h));
    assert_eq!(
        frame(&mut raster, Look::Classic, 0),
        (Dirty::All, classic_size)
    );
    assert_eq!(frame(&mut raster, cutaway, 1), (Dirty::All, cutaway_size));
    assert_ne!(
        frame(&mut raster, cutaway, 1).0,
        Dirty::All,
        "an unchanged frame"
    );
    assert_eq!(
        frame(&mut raster, Look::Classic, 2),
        (Dirty::All, classic_size)
    );
    let canvas = raster.cutaway.as_ref().map(std::ptr::from_ref);
    assert_eq!(frame(&mut raster, cutaway, 3), (Dirty::All, cutaway_size));
    assert_eq!(
        raster.cutaway.as_ref().map(std::ptr::from_ref),
        canvas,
        "switching back rebuilt the cutaway's canvas"
    );
}

#[test]
fn reset_sprite_cache_clears_cached_sprites() {
    use crate::frame_cache::FrameKey;
    use pixtuoid_core::sprite::Frame;

    let mut raster = Raster::new(Arc::new(crate::pack::test_default_pack()));
    // Prime the cache, so the assertion below distinguishes a real reset from a
    // no-op on an already-empty cache.
    raster.classic().caches.sprites.get_or_make(
        FrameKey {
            agent_id: AgentId::from_parts("test", "agent"),
            anim_name: "idle",
            frame_idx: 0,
            flip_x: false,
            glow_tint: None,
            burn: crate::burn::BurnTier::Normal,
            density: pixtuoid_core::sprite::format::Density::ONE,
        },
        Frame::default,
    );
    assert_eq!(
        raster.classic().caches.sprites.len(),
        1,
        "priming populates it"
    );
    raster.reset_sprite_cache();
    assert_eq!(raster.classic().caches.sprites.len(), 0, "reset clears it");
}
