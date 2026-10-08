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

fn inputs<'a>(scene: &'a SceneState, pack: &'a OfficeArt, now: SystemTime) -> RenderInputs<'a> {
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
    let pack = Arc::new(crate::pack::test_office());
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
    let pack = Arc::new(crate::pack::test_office());
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

    let mut raster = Raster::new(Arc::new(crate::pack::test_office()));
    // Prime the cache, so the assertion below distinguishes a real reset from a
    // no-op on an already-empty cache.
    raster.classic().caches.sprites.get_or_make(
        FrameKey {
            agent_id: AgentId::from_parts("test", "agent"),
            anim_name: pixtuoid_core::sprite::format::Piece::Seated,
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
    let pack = Arc::new(crate::pack::test_office());
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
        !floor
            .raster
            .classic_drawn()
            .expect("shown")
            .signs
            .is_empty(),
        "the office is drawn"
    );
    let refused = RenderInputs {
        size: Size { w: 1, h: 1 },
        ..inputs(&scene, &pack, t0)
    };
    assert!(render(&mut floor, office.stores(), Look::Classic, refused).is_none());
    let drawn = floor.raster.classic_drawn().expect("shown");
    assert!(
        drawn.badges.is_empty() && drawn.bubbles.is_empty() && drawn.signs.is_empty(),
        "no badge, bubble or sign"
    );
}

/// The cutaway's recoloured figures leave with their agents: a long run that
/// sees sessions come and go holds none of the gone ones' art.
#[test]
fn the_cutaways_figures_leave_with_their_agents() {
    let pack = Arc::new(crate::pack::test_office());
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let scene = office(t0);
    let (mut floor, mut office) = (PerFloor::new(Arc::clone(&pack)), PerOffice::new());
    let cutaway = Look::Cutaway {
        scale: RenderScale::new(4).expect("nonzero"),
    };
    render(
        &mut floor,
        office.stores(),
        cutaway,
        inputs(&scene, &pack, t0),
    )
    .expect("lays out");
    assert!(
        office.raster.cutaway.figures_len() > 0,
        "premise: the frame drew its agents"
    );
    office.evict_missing(&SceneState::new([8; pixtuoid_core::state::MAX_FLOORS]));
    assert_eq!(office.raster.cutaway.figures_len(), 0);
}

/// A classic frame hands its badges, its bubbles and its signs over apart,
/// and a refused frame leaves them empty.
#[test]
fn a_classic_frame_hands_over_its_badges_and_signs_apart() {
    use crate::display::TextRole;
    let pack = Arc::new(crate::pack::test_office());
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
    let drawn = floor.raster.classic_drawn().expect("shown");
    assert!(!drawn.badges.is_empty(), "the office badges its agents");
    let signs: Vec<TextRole> = drawn.signs.iter().map(|r| r.role).collect();
    assert!(
        signs.contains(&TextRole::Star) && signs.contains(&TextRole::Indicator),
        "{signs:?}"
    );
    assert!(
        !signs.iter().any(|r| matches!(r, TextRole::Badge(_))),
        "{signs:?}"
    );
    let refused = RenderInputs {
        size: Size { w: 1, h: 1 },
        ..inputs(&scene, &pack, t0)
    };
    assert!(render(&mut floor, office.stores(), Look::Classic, refused).is_none());
    let drawn = floor.raster.classic_drawn().expect("shown");
    assert!(drawn.badges.is_empty() && drawn.bubbles.is_empty() && drawn.signs.is_empty());
}

/// The sim and the raster draw one pack: a frame stepped with another is
/// refused rather than drawn with art the sim never placed.
#[test]
fn a_frame_stepped_with_another_pack_is_refused() {
    let pack = Arc::new(crate::pack::test_office());
    let other = crate::pack::test_office();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let scene = office(t0);
    let (mut floor, mut office) = (PerFloor::new(Arc::clone(&pack)), PerOffice::new());
    let stepped = |pack| inputs(&scene, pack, t0);
    assert!(render(&mut floor, office.stores(), Look::Classic, stepped(&other)).is_none());
    assert!(render(&mut floor, office.stores(), Look::Classic, stepped(&pack)).is_some());
}

/// A frame of another pack or theme empties the office's caches first, and
/// one of the same keeps them.
#[test]
fn a_new_pack_or_theme_empties_the_office_caches() {
    use crate::outside::{OutsideCache, Wall};
    let normal = &crate::theme::NORMAL;
    let cyberpunk = crate::theme::theme_by_name("cyberpunk").expect("a registry theme");
    let one = Arc::new(crate::pack::test_office());
    // the curtain wall drops out of the near plane
    let other = Arc::new(
        OfficeArt::parse(crate::pack::test_pack_declaring(
            "planes = [\"mid\", \"near\"]",
            "planes = [\"mid\"]",
        ))
        .expect("the test pack parses"),
    );
    let now = crate::localclock::at_hour(12);
    let views = |outside: &mut OutsideCache, pack: &OfficeArt, theme| {
        let moment = crate::atmosphere::Moment::resolve(
            crate::sky::Sky::at_with(now, crate::sky::Weather::Overcast),
            theme,
            0.0,
            crate::anim::Motion::Full.timing(now),
        );
        let w = crate::layout::WINDOW_W * 3;
        outside.views(
            &moment,
            pack,
            theme,
            Wall {
                size: (w, 32),
                bays: crate::layout::window_slots(w).collect(),
            },
            pixtuoid_core::sprite::format::Density::ONE,
            crate::glass_weather::GlassWeather::of(&moment),
        )[0]
        .1
        .clone()
    };
    let mut raster = OfficeRaster::default();
    raster.serve(&one, normal);
    let first = views(&mut raster.outside, &one, normal);
    raster.serve(&one, normal);
    let kept = views(&mut raster.outside, &one, normal);
    assert!(
        Arc::ptr_eq(&first, &kept),
        "one pack in one theme lost its views"
    );
    raster.serve(&one, cyberpunk);
    let themed = views(&mut raster.outside, &one, cyberpunk);
    assert!(!Arc::ptr_eq(&kept, &themed), "a new theme kept the views");
    raster.serve(&other, cyberpunk);
    let packed = views(&mut raster.outside, &other, cyberpunk);
    assert!(!Arc::ptr_eq(&themed, &packed), "a new pack kept the views");
}
