use super::*;

#[test]
fn theme_switch_recolors_floor() {
    let scene = scene_with(vec![idle("/t/0.jsonl", 0, t0())], 16);
    let mut r = build(100, 40, vec![]);
    let now = t0();
    r.render(&scene, pack(), now).unwrap();
    let before = r.buf().clone();
    r.set_theme(dark_theme());
    r.render(&scene, pack(), now).unwrap();
    let d = region_diff(&before, r.buf(), 0, 0, before.width(), before.height());
    assert!(
        d > 5_000,
        "switching to a different theme must recolor the floor (diff={d})"
    );
}

#[test]
fn set_theme_with_same_theme_is_a_noop() {
    let scene = scene_with(vec![idle("/t/same.jsonl", 0, t0())], 16);
    let mut r = build(100, 40, vec![]); // built with normal_theme()
    let now = t0();
    r.render(&scene, pack(), now).unwrap();
    let before = r.buf().clone();
    r.set_theme(normal_theme());
    r.render(&scene, pack(), now).unwrap();
    let d = region_diff(&before, r.buf(), 0, 0, before.width(), before.height());
    assert_eq!(
        d, 0,
        "re-setting the identical theme must not recolor (cache not flushed), diff={d}"
    );
}

/// Asserts the lit state, not the frame: the level dims only the room's
/// artificial lights, under 1% of the frame mean.
#[test]
fn occupied_floor_stays_lit() {
    use pixtuoid_scene::floor::VacancyDim;
    let scene = scene_with(vec![active("/lit/0.jsonl", 0, "Edit x", t0())], 16);
    let mut r = build(100, 40, vec![]);
    for now in [
        t0(),
        t0() + Duration::from_millis(VacancyDim::EMPTY_DEBOUNCE_MS),
    ] {
        r.render(&scene, pack(), now).unwrap();
    }
    assert_eq!(r.floors[0].ctx.vacancy_dim.level(), 1.0);
}

/// Each phase of a strike stays on screen at least the photosensitive floor
/// in the half-blocks, at each frame grid: from the flush that first shows it
/// to the one that replaces it. A strike lifts the whole room, so a flush that
/// changes most of the scene's cells is a change of phase.
#[test]
fn each_strike_phase_holds_the_floor_on_screen_in_half_blocks() {
    use crate::test_flash::{assert_each_phase_holds_the_floor, frame_grid, lead, storm_strike};
    let strike = storm_strike();
    let tick = Duration::from_millis(crate::tui::FRAME_TICK_MS);
    for (frame, offset) in frame_grid(tick) {
        let (cols, rows) = crate::tui::renderer::min_terminal_size();
        let mut r = build(cols, rows, vec![]);
        r.set_weather(strike.weather);
        r.set_motion(pixtuoid_scene::anim::Motion::Full);
        let scene = scene_with(vec![idle("/s/0.jsonl", 0, t0())], 16);
        let mut now = strike.start - lead(frame) + offset;
        r.render(&scene, pack(), now).expect("render");
        let cells = crate::tui::renderer::scene_rect(r.frame_buffer().area);
        let mut shown = r.frame_buffer().clone();
        let mut changed = Vec::new();
        while now < strike.end + lead(frame) {
            now += frame;
            r.render(&scene, pack(), now).expect("render");
            let flushed = r.frame_buffer();
            let differ = cells
                .positions()
                .filter(|&p| flushed[p] != shown[p])
                .count();
            if 2 * differ > cells.area() as usize {
                changed.push(now);
            }
            shown = flushed.clone();
        }
        let at = format!("a frame each {frame:?} from +{offset:?}");
        assert_each_phase_holds_the_floor(&changed, strike.changes.len(), &at);
    }
}

/// A frame whose strike phase would replace one shown under the floor is not
/// flushed, so the terminal and its hit targets stay the last frame's, alone
/// or sliding; the frame the floor later is.
#[test]
fn a_held_frame_leaves_the_terminal_and_its_hit_targets_alone() {
    use crate::test_flash::{held_frames, storm_strike};
    let strike = storm_strike();
    let [dark, late, held, shown] = held_frames(&strike);
    for sliding in [false, true] {
        let (cols, rows) = crate::tui::renderer::min_terminal_size();
        let mut r = build(cols, rows, vec![]);
        r.set_weather(strike.weather);
        r.set_motion(pixtuoid_scene::anim::Motion::Full);
        let scene = two_floor_scene();
        if sliding {
            r.navigate_floor(1, dark);
        }
        for at in [dark, late] {
            r.render(&scene, pack(), at).expect("render");
        }
        let before = r.frame_buffer().clone();
        let layout = r.cached_layout().map(|l| l as *const SceneLayout);
        r.render(&scene, pack(), held).expect("render");
        assert_eq!(*r.frame_buffer(), before, "sliding {sliding}: held");
        assert_eq!(r.cached_layout().map(|l| l as *const SceneLayout), layout);
        r.render(&scene, pack(), shown).expect("render");
        assert_ne!(*r.frame_buffer(), before, "sliding {sliding}: shown");
        assert_eq!(r.transition().is_some(), sliding);
    }
}

/// A starved neon's every catch, and every dark between, stays on screen at
/// least the photosensitive floor in the half-blocks, at each frame grid. The
/// cells by the tube's west side change only with it.
#[test]
fn each_stutter_phase_holds_the_floor_on_screen_in_half_blocks() {
    use crate::test_flash::{
        assert_each_phase_holds_the_floor, frame_grid, lead, neon_tube, starved_stutter,
    };
    let stutter = starved_stutter(pack());
    let tick = Duration::from_millis(crate::tui::FRAME_TICK_MS);
    let scene = scene_with(vec![], 16);
    for (frame, offset) in frame_grid(tick) {
        let (cols, rows) = crate::tui::renderer::min_terminal_size();
        let mut r = build(cols, rows, vec![]);
        r.set_weather(stutter.weather);
        r.set_motion(pixtuoid_scene::anim::Motion::Full);
        for at in stutter.setup {
            r.render(&scene, pack(), at).expect("render");
        }
        let cells = crate::tui::renderer::scene_rect(r.frame_buffer().area);
        let tube: Vec<ratatui::layout::Position> = cells
            .positions()
            .filter(|p| neon_tube(p.x - cells.x, 2 * (p.y - cells.y)))
            .collect();
        let snapshot = |r: &TuiRenderer<TestBackend>| -> Vec<ratatui::buffer::Cell> {
            tube.iter().map(|&p| r.frame_buffer()[p].clone()).collect()
        };
        let mut now = stutter.start - lead(frame) + offset;
        r.render(&scene, pack(), now).expect("render");
        let mut shown = snapshot(&r);
        let mut changed = Vec::new();
        while now < stutter.end + lead(frame) {
            now += frame;
            r.render(&scene, pack(), now).expect("render");
            let flushed = snapshot(&r);
            if flushed != shown {
                changed.push(now);
            }
            shown = flushed;
        }
        let at = format!("a frame each {frame:?} from +{offset:?}");
        assert_each_phase_holds_the_floor(&changed, stutter.changes, &at);
    }
}

#[test]
fn theme_picker_renders_theme_names() {
    let scene = scene_with(vec![idle("/tp/0.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.set_theme_picker(Some(0));
    r.render(&scene, pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("cyberpunk") || text.contains("normal"),
        "the theme picker lists theme names"
    );
}

#[test]
fn version_popup_paints_when_open() {
    let scene = scene_with(vec![idle("/vp/0.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, pack(), t0()).unwrap();
    let baseline = r.buf().clone();
    // Render past the 200ms entrance animation so the popup is at full scale.
    r.set_version_popup(true, t0());
    let t1 = t0() + Duration::from_millis(250);
    r.render(&scene, pack(), t1).unwrap();
    assert!(
        r.last_popup_scale() > 0.9,
        "popup should be near full scale"
    );
    let d = region_diff(
        &baseline,
        r.buf(),
        0,
        0,
        baseline.width(),
        baseline.height(),
    );
    assert!(
        d > 1000,
        "an open version popup must paint over the scene (diff={d})"
    );
}
