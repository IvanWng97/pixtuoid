use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use super::*;
use crate::sky::{Sky, Weather};

/// A desk away from every edge.
const DESK: Point = Point { x: 40, y: 30 };

/// A WALL-CLOCK-scale epoch — the magnitude [`neon_breath`]'s integer modulo
/// exists for.
const WALL_CLOCK_MS: u64 = 1_767_000_000_000;

fn neon_at(levels: NeonLevels, darkness: f32) -> f32 {
    neon_halo_strength(
        levels,
        SystemTime::UNIX_EPOCH + Duration::from_millis(WALL_CLOCK_MS),
        darkness,
    )
}

/// The lights of a `w`×`h` office with nobody in it, at `hour` under a clear sky.
fn lights_at(w: u16, h: u16, hour: u32) -> (Layout, Lights) {
    let layout = Layout::compute(w, h, Some(crate::layout::TEST_DEFAULT_DESKS)).expect("fits");
    let sky = Sky::at_with(crate::localclock::at_hour(hour), Weather::Clear);
    let look = Look::resolve(&sky, &crate::theme::NORMAL);
    let lights = Lights::of(
        &layout,
        &look,
        &LightInputs {
            agents: &[],
            seated: &HashMap::new(),
            floor_idx: 0,
            indoor_scale: 1.0,
            neon: NeonLevels::BUSY,
            now: SystemTime::UNIX_EPOCH,
        },
    );
    (layout, lights)
}

#[test]
fn neon_breath_advances_at_wall_clock_scale_and_stays_in_band() {
    let a = neon_breath(WALL_CLOCK_MS, NEON_BREATH_MS, NEON_BREATH_FLOOR);
    let b = neon_breath(WALL_CLOCK_MS + 33, NEON_BREATH_MS, NEON_BREATH_FLOOR);
    assert_ne!(a, b, "the breath must move across a 33ms frame");
    for ms in (0..NEON_BREATH_MS).step_by(97) {
        let v = neon_breath(WALL_CLOCK_MS + ms, NEON_BREATH_MS, NEON_BREATH_FLOOR);
        assert!((NEON_BREATH_FLOOR..=1.0).contains(&v), "{v} at +{ms}ms");
    }
}

#[test]
fn neon_halo_drops_to_its_daylight_floor_and_a_calm_sign_glows_less_than_a_busy_one() {
    let night = neon_at(NeonLevels::BUSY, 1.0);
    let day = neon_at(NeonLevels::BUSY, 0.0);
    assert!(
        (day - night * NEON_DAYLIGHT_FLOOR).abs() < 1e-6,
        "{day} vs {night}"
    );
    let calm = neon_at(NeonLevels::CALM, 1.0);
    assert!(calm > 0.0 && calm < night, "{calm} vs {night}");
}

#[test]
fn a_starved_tube_throws_no_halo_and_a_flash_does() {
    assert_eq!(neon_at(NeonLevels::EMPTY, 1.0), 0.0);
    assert!(neon_at(NeonLevels::FLASH, 1.0) > 0.0);
}

#[test]
fn the_neon_halo_throws_the_signs_own_levels() {
    let layout = Layout::compute(192, 80, Some(crate::layout::TEST_DEFAULT_DESKS)).expect("fits");
    let sky = Sky::at_with(crate::localclock::at_hour(0), Weather::Clear);
    let look = Look::resolve(&sky, &crate::theme::NORMAL);
    let now = SystemTime::UNIX_EPOCH + Duration::from_millis(WALL_CLOCK_MS);
    for levels in [NeonLevels::CALM, NeonLevels::ALERT, NeonLevels::EMPTY] {
        let lights = Lights::of(
            &layout,
            &look,
            &LightInputs {
                agents: &[],
                seated: &HashMap::new(),
                floor_idx: 0,
                indoor_scale: 1.0,
                neon: levels,
                now,
            },
        );
        assert_eq!(
            lights.neon.expect("a neon at this size").strength,
            neon_halo_strength(levels, now, look.darkness),
            "{levels:?}"
        );
    }
}

#[test]
fn the_sun_spills_through_every_window_by_day_and_none_by_night() {
    let (layout, noon) = lights_at(192, 80, 12);
    assert_eq!(noon.spills.len(), layout.window_bays().count());
    assert!(noon.spills.iter().all(|s| s.strength > 0.0));
    let (_, midnight) = lights_at(192, 80, 0);
    assert!(midnight.spills.is_empty());
}

#[test]
fn a_spills_bounds_hold_every_row_whichever_way_it_leans() {
    for slant in [-0.7_f32, 0.0, 0.7] {
        let spill = Emitter {
            kind: EmitterKind::WindowSpill,
            light: Light::Spill {
                x: 40,
                w: WINDOW_W,
                top: 10,
                slant,
            },
            strength: 1.0,
        };
        let ((x0, y0), (x1, y1)) = spill.bounds();
        assert_eq!((y0, y1), (10, 10 + SPILL_DEPTH));
        let rows: Vec<_> = spill_rows(40, WINDOW_W, slant).collect();
        assert!(
            rows.iter()
                .all(|r| r.start >= i32::from(x0) && r.end <= i32::from(x1))
        );
        assert!(
            rows.iter().any(|r| r.start == i32::from(x0)),
            "{slant}: tight on the west"
        );
        assert!(
            rows.iter().any(|r| r.end == i32::from(x1)),
            "{slant}: tight on the east"
        );
    }
}

#[test]
fn a_monitor_halo_hangs_over_each_lit_screen_only() {
    let layout = Layout::compute(192, 80, Some(crate::layout::TEST_DEFAULT_DESKS)).expect("fits");
    let facing = |i: usize| layout.desk_facing(FloorLocalDeskIndex(i));
    let north: Vec<usize> = (0..layout.home_desks.len())
        .filter(|&i| facing(i) == Facing::North)
        .collect();
    let south = (0..layout.home_desks.len())
        .find(|&i| facing(i) == Facing::South)
        .expect("this layout seats both ways");
    let [lit, walking, idle, ..] = north[..] else {
        panic!("this layout needs three screens facing us: {north:?}");
    };
    let id = |p: &str| pixtuoid_core::AgentId::from_transcript_path(p);
    // No tool detail: a screen lights for any tool call.
    let active = pixtuoid_core::state::ActivityState::Active {
        tool_use_id: None,
        detail: None,
        kind: pixtuoid_core::state::ToolKind::Edit,
    };
    let at_desk = |path: &str, desk: usize, state: pixtuoid_core::state::ActivityState| {
        let mut a = crate::pixel_painter::tests::make_slot(id(path), state);
        a.desk_index = pixtuoid_core::state::GlobalDeskIndex(desk);
        a
    };
    let agents = [
        at_desk("/lit.jsonl", lit, active.clone()),
        at_desk("/walking.jsonl", walking, active.clone()),
        at_desk(
            "/idle.jsonl",
            idle,
            pixtuoid_core::state::ActivityState::Idle,
        ),
        at_desk("/back.jsonl", south, active),
    ];
    let seated = HashMap::from([
        (FloorLocalDeskIndex(lit), true),
        (FloorLocalDeskIndex(walking), false),
        (FloorLocalDeskIndex(idle), true),
        (FloorLocalDeskIndex(south), true),
    ]);
    let sky = Sky::at_with(crate::localclock::at_hour(0), Weather::Clear);
    let lights = Lights::of(
        &layout,
        &Look::resolve(&sky, &crate::theme::NORMAL),
        &LightInputs {
            agents: &agents,
            seated: &seated,
            floor_idx: 0,
            indoor_scale: 1.0,
            neon: NeonLevels::BUSY,
            now: SystemTime::UNIX_EPOCH,
        },
    );
    let kinds: Vec<_> = lights.monitor_halos.iter().map(|h| h.kind).collect();
    assert_eq!(
        kinds,
        [EmitterKind::MonitorHalo(
            pixtuoid_core::state::ToolKind::Edit
        )],
        "only the seated, mid-call agent at a screen facing us"
    );
    let desk = layout.home_desks[lit];
    assert_eq!(
        lights.monitor_halos[0].light,
        Light::Patch {
            centre: Point {
                x: desk.x + MONITOR_HALO_DX,
                y: desk.y - 1,
            }
        }
    );
}

#[test]
fn a_desk_lamp_is_lit_whichever_way_the_desk_seats_its_occupant() {
    use crate::layout::Facing;
    // A lamp is a FIXTURE on the desk's west wing, visible from either side; the
    // standby SCREEN is the one that gates on facing.
    for darkness in [0.0_f32, 0.5, 1.0] {
        let north = desk_lights(DESK, Facing::North, darkness, 1.0);
        let south = desk_lights(DESK, Facing::South, darkness, 1.0);
        assert_eq!(
            north.lamp.strength, south.lamp.strength,
            "the lamp may not depend on facing (darkness {darkness})"
        );
        assert!(
            south.screen_idle == 0.0 && north.screen_idle >= south.screen_idle,
            "only a back-turned desk shows its screen (darkness {darkness})"
        );
    }
    assert!(
        desk_lights(DESK, Facing::South, 1.0, 1.0).lamp.strength > 0.0,
        "a viewer-facing desk must still light its lamp after dark"
    );
}

/// The two desk emitters (`lamp`, `screen_idle`); the floor lamp shares the
/// rule but not this pin. Dropping either factor makes that emitter's two
/// readings equal.
#[test]
fn an_emptied_floor_takes_both_desk_emitters_down_with_the_level() {
    use crate::layout::Facing;
    let min = crate::floor::LightingState::MIN_LEVEL;
    let lit = desk_lights(DESK, Facing::North, 1.0, 1.0);
    let empty = desk_lights(DESK, Facing::North, 1.0, min);
    for (what, lit, empty) in [
        ("lamp", lit.lamp.strength, empty.lamp.strength),
        ("screen_idle", lit.screen_idle, empty.screen_idle),
    ] {
        assert!(
            (empty - lit * min).abs() < f32::EPSILON,
            "an empty floor's {what} must scale with the level, got {empty} against {lit}"
        );
    }
}

/// One of every shape, lit.
fn every_shape() -> [Emitter; 4] {
    let lit = |kind, light| Emitter {
        kind,
        light,
        strength: 0.8,
    };
    [
        lit(
            EmitterKind::DeskLamp,
            Light::Halo {
                centre: Point { x: 30, y: 20 },
                radius: 5,
                share: 0.4,
            },
        ),
        lit(
            EmitterKind::NeonGlow,
            Light::Glow {
                at: Point { x: 25, y: 15 },
                w: 8,
                h: 4,
                reach: 6,
            },
        ),
        lit(
            EmitterKind::WindowSpill,
            Light::Spill {
                x: 26,
                w: WINDOW_W,
                top: 12,
                slant: 0.4,
            },
        ),
        lit(
            EmitterKind::MonitorHalo(ToolKind::Read),
            Light::Patch {
                centre: Point { x: 30, y: 20 },
            },
        ),
    ]
}

#[test]
fn a_light_sampled_between_cells_stays_inside_its_bounds() {
    for e in every_shape() {
        let ((x0, y0), (x1, y1)) = e.bounds();
        for y in 0..(48 * 4) {
            for x in 0..(64 * 4) {
                let (fx, fy) = ((x as f32 + 0.5) / 4.0, (y as f32 + 0.5) / 4.0);
                if e.level_at_f(fx, fy).is_some() {
                    assert!(
                        fx >= f32::from(x0)
                            && fx < f32::from(x1)
                            && fy >= f32::from(y0)
                            && fy < f32::from(y1),
                        "{:?} lights ({fx}, {fy}) outside {:?}",
                        e.kind,
                        e.bounds()
                    );
                }
            }
        }
    }
}

/// A painter steps a light's levels down from its peak, so the peak must be
/// the level its brightest cell actually gets — every shape.
#[test]
fn a_lights_peak_is_its_brightest_cell() {
    let at = |x, y| Point { x, y };
    let lights = [
        Light::Halo {
            centre: at(30, 20),
            radius: 11,
            share: 1.0,
        },
        Light::Halo {
            centre: at(30, 20),
            radius: 5,
            share: 0.42,
        },
        Light::Glow {
            at: at(20, 20),
            w: 30,
            h: 8,
            reach: NEON_HALO_RADIUS,
        },
        Light::Spill {
            x: 20,
            w: WINDOW_W,
            top: 10,
            slant: 0.3,
        },
        Light::Patch { centre: at(30, 20) },
    ];
    for light in lights {
        let e = Emitter {
            kind: EmitterKind::FloorLamp,
            light,
            strength: 0.6,
        };
        let ((x0, y0), (x1, y1)) = e.bounds();
        let brightest = (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .filter_map(|(x, y)| e.level_at(x, y))
            .fold(0.0_f32, f32::max);
        assert!(
            (e.peak() - brightest).abs() < 1e-6,
            "{light:?}: peak {} vs brightest {brightest}",
            e.peak()
        );
    }
}
