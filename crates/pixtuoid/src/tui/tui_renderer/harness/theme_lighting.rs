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
    use pixtuoid_scene::floor::LightingState;
    let scene = scene_with(vec![active("/lit/0.jsonl", 0, "Edit x", t0())], 16);
    let mut r = build(100, 40, vec![]);
    for now in [
        t0(),
        t0() + Duration::from_millis(LightingState::EMPTY_DEBOUNCE_MS),
    ] {
        r.render(&scene, pack(), now).unwrap();
    }
    assert_eq!(r.floors[0].ctx.light.level(), 1.0);
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
