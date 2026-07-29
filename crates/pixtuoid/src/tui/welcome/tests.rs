use super::*;

#[test]
fn from_detected_pre_checks_every_row_and_resolves_badges() {
    // `codex` is a real registered, target-bearing source → known badge (`cx`)
    // and display name from its install target.
    let ui = WelcomeUi::from_detected(&["codex"]);
    assert_eq!(ui.rows.len(), 1);
    let row = &ui.rows[0];
    assert_eq!(row.source_id, "codex");
    assert_eq!(row.label_prefix, "cx", "badge resolves from the registry");
    assert_eq!(
        row.display_name, "Codex",
        "name resolves from the install target"
    );
    assert!(row.checked, "all rows pre-checked on first run");
    assert_eq!(ui.selected, 0);
}

#[test]
fn empty_detected_is_empty() {
    let ui = WelcomeUi::from_detected(&[]);
    assert!(ui.is_empty());
    // Navigation/toggle on an empty roster never panics.
    let mut ui = ui;
    ui.move_down();
    ui.move_up();
    ui.toggle_selected();
    assert!(ui.decisions().is_empty());
}

#[test]
fn navigation_clamps_at_both_ends() {
    let mut ui = WelcomeUi::from_detected(&["codex", "claude-code", "cursor"]);
    assert_eq!(ui.selected, 0);
    ui.move_up(); // already at top
    assert_eq!(ui.selected, 0);
    ui.move_down();
    ui.move_down();
    assert_eq!(ui.selected, 2);
    ui.move_down(); // already at bottom
    assert_eq!(ui.selected, 2, "clamps at the last row");
    ui.move_up();
    assert_eq!(ui.selected, 1);
}

#[test]
fn dim_ramps_in_on_open_and_out_on_close() {
    // Opening: starts at full brightness, ends at the floor, monotone down.
    assert!((dim_opening(0) - 1.0).abs() < 1e-6, "no dim at t=0");
    assert!(
        (dim_opening(DIM_RAMP_MS) - DIM_FLOOR).abs() < 1e-6,
        "floor at full ramp"
    );
    assert!(
        (dim_opening(DIM_RAMP_MS * 10) - DIM_FLOOR).abs() < 1e-6,
        "clamps past the ramp"
    );
    assert!(dim_opening(DIM_RAMP_MS / 2) > DIM_FLOOR && dim_opening(DIM_RAMP_MS / 2) < 1.0);

    // Closing: starts at whatever the open ramp reached, climbs back toward
    // full, None once restored.
    assert!(
        (dim_closing(DIM_FLOOR, 0).unwrap() - DIM_FLOOR).abs() < 1e-6,
        "floor at fade start"
    );
    let mid = dim_closing(DIM_FLOOR, DIM_FADE_OUT_MS / 2).unwrap();
    assert!(mid > DIM_FLOOR && mid < 1.0, "climbing back: {mid}");
    assert_eq!(
        dim_closing(DIM_FLOOR, DIM_FADE_OUT_MS),
        None,
        "fully restored ⇒ drop the state"
    );
    assert_eq!(dim_closing(DIM_FLOOR, DIM_FADE_OUT_MS + 100), None);

    // The default frame never dims.
    assert!((OnboardingFrame::default().dim - 1.0).abs() < 1e-6);
}

/// An overlay skipped MID-RAMP must fade out from where it actually was — a
/// close ramp hardcoded to `DIM_FLOOR` darkened the whole office by up to
/// `1.0 - DIM_FLOOR` for a frame before climbing back.
#[test]
fn the_close_fade_continues_from_the_dim_it_was_interrupted_at() {
    let interrupted = dim_opening(DIM_RAMP_MS / 3);
    assert!(
        interrupted > DIM_FLOOR,
        "the probe must be mid-ramp: {interrupted}"
    );
    let first_close_frame = dim_closing(interrupted, 0).expect("the fade is armed");
    assert!(
        (first_close_frame - interrupted).abs() < 1e-6,
        "the fade must resume at {interrupted}, not snap to {first_close_frame}"
    );
    // …and still climb monotonically to full over the same window.
    let mid = dim_closing(interrupted, DIM_FADE_OUT_MS / 2).expect("still fading");
    assert!(mid > interrupted && mid < 1.0, "climbing back: {mid}");
    assert_eq!(dim_closing(interrupted, DIM_FADE_OUT_MS), None);
}

#[test]
fn toggle_flips_only_the_selected_row_and_feeds_decisions() {
    let mut ui = WelcomeUi::from_detected(&["codex", "claude-code"]);
    ui.move_down(); // select claude-code
    ui.toggle_selected(); // uncheck it
    let decisions: std::collections::HashMap<_, _> = ui.decisions().into_iter().collect();
    assert!(decisions["codex"], "untouched row stays checked");
    assert!(!decisions["claude-code"], "selected row toggled off");
}
