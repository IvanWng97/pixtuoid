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
    assert_eq!(row.display_name, "Codex", "name resolves from the install target");
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
fn toggle_flips_only_the_selected_row_and_feeds_decisions() {
    let mut ui = WelcomeUi::from_detected(&["codex", "claude-code"]);
    ui.move_down(); // select claude-code
    ui.toggle_selected(); // uncheck it
    let decisions: std::collections::HashMap<_, _> = ui.decisions().into_iter().collect();
    assert_eq!(decisions["codex"], true, "untouched row stays checked");
    assert_eq!(decisions["claude-code"], false, "selected row toggled off");
}
