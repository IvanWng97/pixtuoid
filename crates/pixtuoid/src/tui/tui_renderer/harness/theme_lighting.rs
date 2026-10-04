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
        let (mut r, screen) = half_blocks_on_screen(cols, rows);
        r.set_weather(strike.weather);
        r.set_motion(pixtuoid_scene::anim::Motion::Full);
        let scene = scene_with(vec![idle("/s/0.jsonl", 0, t0())], 16);
        let mut now = strike.start - lead(frame) + offset;
        screen.at(now);
        r.render(&scene, pack(), now).expect("render");
        let cells = crate::tui::renderer::scene_rect(flushed(&r).area);
        let mut shown = flushed(&r).clone();
        let mut changed = Vec::new();
        while now < strike.end + lead(frame) {
            now += frame;
            screen.at(now);
            r.render(&scene, pack(), now).expect("render");
            let differ = cells
                .positions()
                .filter(|&p| flushed(&r)[p] != shown[p])
                .count();
            if 2 * differ > cells.area() as usize {
                changed.push(now);
            }
            shown = flushed(&r).clone();
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
        let (mut r, screen) = half_blocks_on_screen(cols, rows);
        r.set_weather(strike.weather);
        r.set_motion(pixtuoid_scene::anim::Motion::Full);
        let scene = two_floor_scene();
        if sliding {
            r.navigate_floor(1, dark);
        }
        for at in [dark, late] {
            screen.at(at);
            r.render(&scene, pack(), at).expect("render");
        }
        let before = flushed(&r).clone();
        let layout = r.cached_layout().map(|l| l as *const SceneLayout);
        screen.at(held);
        r.render(&scene, pack(), held).expect("render");
        assert_eq!(*flushed(&r), before, "sliding {sliding}: held");
        assert_eq!(r.cached_layout().map(|l| l as *const SceneLayout), layout);
        screen.at(shown);
        r.render(&scene, pack(), shown).expect("render");
        assert_ne!(*flushed(&r), before, "sliding {sliding}: shown");
        assert_eq!(r.transition().is_some(), sliding);
    }
}

/// A phase holds the floor from when its flush lands, not from when its frame
/// began: after a slow flush, the next phase waits for the floor to pass on
/// the screen clock, though its frame's own clock says it has.
#[test]
fn a_slow_flushs_phase_holds_the_floor_from_when_it_lands() {
    use crate::test_flash::storm_strike;
    const SLOW: Duration = Duration::from_millis(60);
    let floor = Duration::from_millis(pixtuoid_scene::anim::PHOTOSENSITIVE_PHASE_MIN_MS);
    let strike = storm_strike();
    let [first, second] = [strike.changes[0], strike.changes[1]];
    let (cols, rows) = crate::tui::renderer::min_terminal_size();
    let (mut r, screen) = half_blocks_on_screen(cols, rows);
    r.set_weather(strike.weather);
    r.set_motion(pixtuoid_scene::anim::Motion::Full);
    let scene = scene_with(vec![idle("/s/0.jsonl", 0, t0())], 16);
    screen.at(first - 2 * floor);
    r.render(&scene, pack(), first - 2 * floor).expect("render");
    r.terminal.backend_mut().slow = Some((screen.clone(), SLOW));
    screen.at(first);
    r.render(&scene, pack(), first).expect("render");
    let landed = screen.now();
    r.terminal.backend_mut().slow = None;
    let before = flushed(&r).clone();
    assert!(
        second.duration_since(first).expect("in order") >= floor,
        "the frame clock says the floor has passed"
    );
    screen.at(second);
    r.render(&scene, pack(), second).expect("render");
    assert_eq!(
        *flushed(&r),
        before,
        "held until the slow flush's phase shows the floor"
    );
    let shown = std::time::UNIX_EPOCH + landed + floor;
    screen.at(shown);
    r.render(&scene, pack(), shown).expect("render");
    assert_ne!(*flushed(&r), before, "shown once it has");
}

/// A pause freezes the frame clock inside a hold; the screen clock runs on,
/// so the hold runs out and the paused frame, with a theme change, is flushed.
#[test]
fn a_pause_inside_a_hold_never_wedges_the_half_blocks() {
    use crate::test_flash::{held_frames, storm_strike};
    let floor = Duration::from_millis(pixtuoid_scene::anim::PHOTOSENSITIVE_PHASE_MIN_MS);
    let strike = storm_strike();
    let [dark, late, paused, _] = held_frames(&strike);
    let (cols, rows) = crate::tui::renderer::min_terminal_size();
    let (mut r, screen) = half_blocks_on_screen(cols, rows);
    r.set_weather(strike.weather);
    r.set_motion(pixtuoid_scene::anim::Motion::Full);
    let scene = scene_with(vec![idle("/s/0.jsonl", 0, t0())], 16);
    for at in [dark, late] {
        screen.at(at);
        r.render(&scene, pack(), at).expect("render");
    }
    let before = flushed(&r).clone();
    screen.at(paused);
    r.render(&scene, pack(), paused).expect("render");
    assert_eq!(*flushed(&r), before, "held");
    r.set_theme(dark_theme());
    screen.at(late);
    screen.advance(floor);
    r.render(&scene, pack(), paused).expect("render");
    assert_ne!(
        *flushed(&r),
        before,
        "the paused frame was flushed once the hold ran out"
    );
}

/// A starved neon's every catch, and every dark between, stays on screen at
/// least the photosensitive floor in the half-blocks, at each frame grid. The
/// cells by the tube's west side change only with it.
#[test]
fn each_stutter_phase_holds_the_floor_on_screen_in_half_blocks() {
    use crate::test_flash::{
        assert_each_phase_holds_the_floor, frame_grid, lead, neon_tube, starved_stutter,
    };
    let stutter = starved_stutter();
    let tick = Duration::from_millis(crate::tui::FRAME_TICK_MS);
    let scene = scene_with(vec![], 16);
    for (frame, offset) in frame_grid(tick) {
        let (cols, rows) = crate::tui::renderer::min_terminal_size();
        let (mut r, screen) = half_blocks_on_screen(cols, rows);
        r.set_weather(stutter.weather);
        r.set_motion(pixtuoid_scene::anim::Motion::Full);
        for at in stutter.setup {
            screen.at(at);
            r.render(&scene, pack(), at).expect("render");
        }
        let cells = crate::tui::renderer::scene_rect(flushed(&r).area);
        let tube: Vec<ratatui::layout::Position> = cells
            .positions()
            .filter(|p| neon_tube(p.x - cells.x, 2 * (p.y - cells.y)))
            .collect();
        let snapshot = |r: &TuiRenderer<Slow>| -> Vec<ratatui::buffer::Cell> {
            tube.iter().map(|&p| flushed(r)[p].clone()).collect()
        };
        let mut now = stutter.start - lead(frame) + offset;
        screen.at(now);
        r.render(&scene, pack(), now).expect("render");
        let mut shown = snapshot(&r);
        let mut changed = Vec::new();
        while now < stutter.end + lead(frame) {
            now += frame;
            screen.at(now);
            r.render(&scene, pack(), now).expect("render");
            let shot = snapshot(&r);
            if shot != shown {
                changed.push(now);
            }
            shown = shot;
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
