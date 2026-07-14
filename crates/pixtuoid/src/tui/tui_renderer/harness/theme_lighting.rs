use super::*;

// ===================================================================
// Theme / palette
// ===================================================================

#[test]
fn theme_switch_recolors_floor() {
    let scene = scene_with(vec![idle("/t/0.jsonl", 0, t0())], 16);
    let mut r = build(100, 40, vec![]);
    let now = t0();
    r.render(&scene, &pack(), now).unwrap();
    let before = r.buf().clone();
    r.set_theme(dark_theme());
    r.render(&scene, &pack(), now).unwrap();
    let d = region_diff(&before, r.buf(), 0, 0, before.width(), before.height());
    assert!(
        d > 5_000,
        "switching to a different theme must recolor the floor (diff={d})"
    );
}

// The `ptr::eq` guard in `set_theme`: calling it with the SAME &'static theme
// the renderer already holds must NOT flush the frame cache, so the next frame
// is byte-identical. This is the false branch twin of `theme_switch_recolors_floor`
// (the true branch); together they pin both arms of the guard — a mutant that
// always-flushes fails here, one that never-flushes fails there.
#[test]
fn set_theme_with_same_theme_is_a_noop() {
    let scene = scene_with(vec![idle("/t/same.jsonl", 0, t0())], 16);
    let mut r = build(100, 40, vec![]); // built with normal_theme()
    let now = t0();
    r.render(&scene, &pack(), now).unwrap();
    let before = r.buf().clone();
    // The SAME &'static pointer the renderer holds ⇒ ptr::eq is true ⇒ skip.
    r.set_theme(normal_theme());
    r.render(&scene, &pack(), now).unwrap();
    let d = region_diff(&before, r.buf(), 0, 0, before.width(), before.height());
    assert_eq!(
        d, 0,
        "re-setting the identical theme must not recolor (cache not flushed), diff={d}"
    );
}

// ===================================================================
// Lighting
// ===================================================================

// NOTE: the *visible* empty-floor darkening is gated on `look.darkness`
// (time-of-day via `chrono::Local`), so it only manifests at night and is
// timezone-dependent — not robustly assertable through render headlessly.
// The fade math itself is covered by the `LightingState` unit tests in
// floor.rs. Here we only guard the time-independent invariant: an OCCUPIED
// floor must not fade.
#[test]
fn occupied_floor_stays_lit() {
    // A present agent keeps the floor lit (no fade).
    let scene = scene_with(vec![active("/lit/0.jsonl", 0, "Edit x", t0())], 16);
    let mut r = build(100, 40, vec![]);
    let mut now = t0();
    r.render(&scene, &pack(), now).unwrap();
    now += Duration::from_millis(2000);
    r.render(&scene, &pack(), now).unwrap();
    let early = avg_lum(r.buf(), 0, 0, r.buf().width(), r.buf().height());
    for _ in 0..700 {
        now += Duration::from_millis(33);
        r.render(&scene, &pack(), now).unwrap();
    }
    let late = avg_lum(r.buf(), 0, 0, r.buf().width(), r.buf().height());
    assert!(
        late > early * 0.9,
        "occupied floor must stay lit (early={early:.1}, late={late:.1})"
    );
}

// ===================================================================
// Theme picker + version-popup PAINT (renderer.rs / theme_picker.rs + version_popup.rs branches)
// ===================================================================

#[test]
fn theme_picker_renders_theme_names() {
    let scene = scene_with(vec![idle("/tp/0.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.set_theme_picker(Some(0));
    r.render(&scene, &pack(), t0()).unwrap();
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
    // Baseline (no popup).
    r.render(&scene, &pack(), t0()).unwrap();
    let baseline = r.buf().clone();
    // Open popup; render past the 200ms entrance so it's at full scale.
    r.set_version_popup(true, t0());
    let t1 = t0() + Duration::from_millis(250);
    r.render(&scene, &pack(), t1).unwrap();
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
