use std::path::Path;
use std::time::{Duration, SystemTime};

use pixtuoid_core::state::{ActivityState, GlobalDeskIndex};
use pixtuoid_core::{AgentId, AgentSlot, SceneState};

use super::*;
use crate::floor::{FloorMeta, PerOffice, PetInputs};

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

/// The entry runs the sim's epilogue every frame it steps: a carrier's cup is
/// stamped with the frame that saw it, and the door's clamp is the frame's own.
#[test]
fn the_entry_runs_the_epilogue_every_frame() {
    let pack = Arc::new(crate::pack::test_default_pack());
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let scene = office(t0);
    let (mut floor, mut office) = (PerFloor::new(Arc::clone(&pack)), PerOffice::new());
    let mut coffee_seen = false;
    let mut door_seen = false;
    for step in 0..1_200u64 {
        let now = t0 + Duration::from_millis(step * 500);
        let before = office.coffee.map().clone();
        render(
            &mut floor,
            office.stores(),
            Look::Classic,
            inputs(&scene, &pack, now),
        )
        .expect("lays out");
        for (id, at) in office.coffee.map() {
            if !before.contains_key(id) {
                assert_eq!(*at, now, "step {step}: a cup stamped off its frame");
                coffee_seen = true;
            }
        }
        let clamp = floor.ctx.door_anim_max_ms;
        floor.ctx.recompute_door_anim_max_ms(now);
        assert_eq!(
            clamp, floor.ctx.door_anim_max_ms,
            "step {step}: a stale door clamp"
        );
        door_seen |= clamp > 0;
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
    let (mut floor, mut office) = (PerFloor::new(Arc::clone(&pack)), PerOffice::new());
    let scale = RenderScale::new(2).expect("nonzero");
    let cutaway = Look::Cutaway { scale };
    let mut frame = |floor: &mut PerFloor, look, ms: u64| {
        let r = render(
            floor,
            office.stores(),
            look,
            inputs(&scene, &pack, t0 + Duration::from_millis(ms)),
        )
        .expect("lays out");
        (r.dirty, (r.pixels.width(), r.pixels.height()))
    };
    let classic_size = (SIZE.w, SIZE.h);
    let cutaway_size = (scale.to_buffer(SIZE.w), scale.to_buffer(SIZE.h));
    assert_eq!(
        frame(&mut floor, Look::Classic, 0),
        (Dirty::All, classic_size)
    );
    assert_eq!(frame(&mut floor, cutaway, 1), (Dirty::All, cutaway_size));
    assert_ne!(
        frame(&mut floor, cutaway, 1).0,
        Dirty::All,
        "an unchanged frame"
    );
    assert_eq!(
        frame(&mut floor, Look::Classic, 2),
        (Dirty::All, classic_size)
    );
    let canvas = floor.raster.cutaway.as_ref().map(std::ptr::from_ref);
    assert_eq!(frame(&mut floor, cutaway, 3), (Dirty::All, cutaway_size));
    assert_eq!(
        floor.raster.cutaway.as_ref().map(std::ptr::from_ref),
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

/// A refused frame shows the bare floor, so nothing hover named on the last
/// drawn one is still there.
#[test]
fn a_refused_classic_frame_names_no_one() {
    let pack = Arc::new(crate::pack::test_default_pack());
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let scene = office(t0);
    let (mut floor, mut office) = (PerFloor::new(Arc::clone(&pack)), PerOffice::new());
    render(
        &mut floor,
        office.stores(),
        Look::Classic,
        inputs(&scene, &pack, t0),
    )
    .expect("lays out");
    assert!(
        !floor.raster.classic_texts().is_empty(),
        "the office is drawn"
    );
    let refused = RenderInputs {
        size: Size { w: 1, h: 1 },
        ..inputs(&scene, &pack, t0)
    };
    assert!(render(&mut floor, office.stores(), Look::Classic, refused).is_none());
    assert!(floor.raster.classic_texts().is_empty());
    assert!(
        floor
            .raster
            .classic_drawn()
            .expect("shown")
            .bubbles
            .is_empty()
    );
}

/// The sim and the raster draw one pack: a frame stepped with another is
/// refused rather than drawn with art the sim never placed.
#[test]
fn a_frame_stepped_with_another_pack_is_refused() {
    let pack = Arc::new(crate::pack::test_default_pack());
    let other = crate::pack::test_default_pack();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let scene = office(t0);
    let (mut floor, mut office) = (PerFloor::new(Arc::clone(&pack)), PerOffice::new());
    let stepped = |pack| inputs(&scene, pack, t0);
    assert!(render(&mut floor, office.stores(), Look::Classic, stepped(&other)).is_none());
    assert!(render(&mut floor, office.stores(), Look::Classic, stepped(&pack)).is_some());
}
