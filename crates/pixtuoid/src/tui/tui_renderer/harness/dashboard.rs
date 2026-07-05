use super::*;

// --- agent dashboard overlay -------------------------------------------------

use crate::tui::dashboard::{build_dashboard_rows, DashboardFolds};

#[test]
fn dashboard_popup_renders_labels_states_and_live_tool() {
    let mut r = build(120, 44, vec![]);
    let mut a = active("/h/alpha.jsonl", 0, "Edit reducer.rs", t0());
    a.label = "cc\u{b7}alpha".into();
    let mut b = idle("/h/beta.jsonl", 1, t0());
    b.label = "cc\u{b7}beta".into();
    let scene = scene_with(vec![a, b], 16);

    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    let first = rows[0].agent_id;
    r.set_dashboard_frame_parts(true, rows, Some(first), 0);
    r.render(&scene, &pack(), t0()).unwrap();

    let text = frame_text(r.frame_buffer());
    assert!(text.contains("Agents ("), "header missing:\n{text}");
    assert!(text.contains("cc\u{b7}alpha"), "alpha row missing:\n{text}");
    assert!(text.contains("cc\u{b7}beta"), "beta row missing:\n{text}");
    assert!(
        text.contains("Edit reducer.rs"),
        "live tool detail missing:\n{text}"
    );
    assert!(text.contains("idle"), "idle state missing:\n{text}");
}

#[test]
fn connection_panel_renders_both_facets_borderless() {
    use crate::tui::connection::{ConnState, ConnectionRow, LiveInfo};
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    let rows = vec![
        ConnectionRow {
            source_id: "claude",
            label_prefix: "cc",
            display_name: "Claude Code",
            state: ConnState::Connected,
            config_path: Some(std::path::PathBuf::from("~/.claude/settings.json")),
            target: None,
            health: None,
        },
        ConnectionRow {
            source_id: "antigravity",
            label_prefix: "ag",
            display_name: "Antigravity",
            state: ConnState::Disconnected,
            config_path: None,
            target: None,
            health: None,
        },
    ];
    let live = vec![
        LiveInfo {
            agents: 2,
            last_event_age: Some(std::time::Duration::from_secs(3)),
            dead: false,
        },
        LiveInfo {
            agents: 1,
            last_event_age: Some(std::time::Duration::from_secs(12)),
            dead: false,
        },
    ];
    r.set_connection_frame_parts(
        true,
        rows,
        live,
        0,
        None,
        None,
        "socket  /tmp/p.sock  (listening)".into(),
    );
    r.render(&scene, &pack(), t0()).unwrap();

    let text = frame_text(r.frame_buffer());
    assert!(text.contains("Sources"), "title missing:\n{text}");
    assert!(text.contains("[cc]"), "cc badge missing:\n{text}");
    assert!(text.contains("[ag]"), "ag badge missing:\n{text}");
    assert!(text.contains("2 agents"), "live count missing:\n{text}");
    assert!(text.contains("socket"), "socket line missing:\n{text}");
    // Selected row (cc) is Connected → the detail line shows where it's installed.
    assert!(
        text.contains("installed at"),
        "connected detail (install path) missing:\n{text}"
    );
    // Borderless: the popup (the tooltip_bg-filled region) carries no box glyphs.
    let popup = dash_popup(r.frame_buffer());
    for g in [
        '\u{256d}', '\u{256e}', '\u{2570}', '\u{256f}', '\u{2502}', '\u{2500}',
    ] {
        assert!(
            !popup.contains(g),
            "Sources panel must be borderless, found {g}:\n{popup}"
        );
    }
}

// #309 / health-consolidation: a connected row whose cached health summary is
// set (a) shows a per-row ⚠ FLAG (scannable in the list), and (b) shows the full
// reason in the detail line, PREEMPTING the benign "installed at" hint. The
// health string here is glyph-FREE so the only ⚠ in the buffer is the per-row
// flag the painter adds — proving the flag specifically.
#[test]
fn connection_panel_health_flag_and_detail_preempt_the_install_path() {
    use crate::tui::connection::{ConnState, ConnectionRow, LiveInfo};
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    let rows = vec![ConnectionRow {
        source_id: "reasonix",
        label_prefix: "rx",
        display_name: "Reasonix",
        state: ConnState::Connected,
        config_path: Some(std::path::PathBuf::from("~/.reasonix/settings.json")),
        target: None,
        health: Some("install broken: shim binary missing".into()), // NO ⚠ prefix
    }];
    r.set_connection_frame_parts(
        true,
        rows,
        vec![LiveInfo::default()],
        0,
        None,
        None,
        "socket  /tmp/p.sock  (listening)".into(),
    );
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains('\u{26a0}'),
        "the per-row health flag (⚠) must render (health string carries none):\n{text}"
    );
    assert!(
        text.contains("install broken"),
        "the full reason must show in the detail line:\n{text}"
    );
    assert!(
        !text.contains("installed at"),
        "the health verdict must PREEMPT the install-path hint:\n{text}"
    );
}

#[test]
fn connection_panel_armed_shows_confirm_prompt() {
    use crate::tui::connection::{ConnState, ConnectionRow, LiveInfo};
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    let rows = vec![ConnectionRow {
        source_id: "codex",
        label_prefix: "cx",
        display_name: "Codex",
        state: ConnState::Connected,
        config_path: Some(std::path::PathBuf::from("~/.codex/config.toml")),
        target: None,
        health: None,
    }];
    r.set_connection_frame_parts(
        true,
        rows,
        vec![LiveInfo::default()],
        0,
        Some(0),
        None,
        String::new(),
    );
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("(y/n)"),
        "armed confirm prompt missing:\n{text}"
    );
    assert!(text.contains("Codex"), "armed target name missing:\n{text}");
}

// Selected row is Disconnected (no health/confirm/result) → the detail line
// surfaces the connect ACTION, not a (meaningless-until-bound) install path.
#[test]
fn connection_panel_disconnected_selected_shows_connect_hint() {
    use crate::tui::connection::{ConnState, ConnectionRow, LiveInfo};
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    let rows = vec![ConnectionRow {
        source_id: "antigravity",
        label_prefix: "ag",
        display_name: "Antigravity",
        state: ConnState::Disconnected,
        config_path: None,
        target: None,
        health: None,
    }];
    r.set_connection_frame_parts(
        true,
        rows,
        vec![LiveInfo::default()],
        0,
        None,
        None,
        "socket  /tmp/p.sock  (listening)".into(),
    );
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("press t to connect"),
        "disconnected detail hint missing:\n{text}"
    );
    assert!(
        !text.contains("installed at"),
        "a disconnected row must NOT show an install path:\n{text}"
    );
}

// Selected row is NoCli → the detail line is `no_action_hint` = "<name> not
// detected on this machine" (explains why it can't be bound).
#[test]
fn connection_panel_no_cli_selected_shows_not_detected_hint() {
    use crate::tui::connection::{ConnState, ConnectionRow, LiveInfo};
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    let rows = vec![ConnectionRow {
        source_id: "codex",
        label_prefix: "cx",
        display_name: "Codex",
        state: ConnState::NoCli { connected: false },
        config_path: None,
        target: None,
        health: None,
    }];
    r.set_connection_frame_parts(
        true,
        rows,
        vec![LiveInfo::default()],
        0,
        None,
        None,
        "socket  /tmp/p.sock  (listening)".into(),
    );
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("not detected on this machine"),
        "no-CLI detail hint missing:\n{text}"
    );
    assert!(
        text.contains("Codex"),
        "no-CLI hint must name the CLI:\n{text}"
    );
}

// Selected Connected row WITHOUT a known config_path → detail is the bare
// "connected" fallback, NOT the "installed at: <path>" arm. Distinguishes the
// `None` config_path branch from the `Some(p)` branch (covered elsewhere).
#[test]
fn connection_panel_connected_without_config_path_shows_connected() {
    use crate::tui::connection::{ConnState, ConnectionRow, LiveInfo};
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    let rows = vec![ConnectionRow {
        source_id: "claude",
        label_prefix: "cc",
        display_name: "Claude Code",
        state: ConnState::Connected,
        config_path: None,
        target: None,
        health: None,
    }];
    r.set_connection_frame_parts(
        true,
        rows,
        vec![LiveInfo::default()],
        0,
        None,
        None,
        "socket  /tmp/p.sock  (listening)".into(),
    );
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("connected"),
        "connected fallback detail missing:\n{text}"
    );
    assert!(
        !text.contains("installed at"),
        "no config_path → must NOT show an install path:\n{text}"
    );
}

// `last_result` (a post-action result string) preempts the per-state install
// hint: even with a Connected row whose config_path WOULD render "installed
// at:", the result string wins (it's higher precedence than the per-state arm).
#[test]
fn connection_panel_last_result_overrides_per_state_detail() {
    use crate::tui::connection::{ConnState, ConnectionRow, LiveInfo};
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    let rows = vec![ConnectionRow {
        source_id: "claude",
        label_prefix: "cc",
        display_name: "Claude Code",
        state: ConnState::Connected,
        config_path: Some(std::path::PathBuf::from("~/.claude/settings.json")),
        target: None,
        health: None,
    }];
    r.set_connection_frame_parts(
        true,
        rows,
        vec![LiveInfo::default()],
        0,
        None,
        Some("X-RESULT-SENTINEL".to_string()),
        "socket  /tmp/p.sock  (listening)".into(),
    );
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("X-RESULT-SENTINEL"),
        "last_result string must render in the detail line:\n{text}"
    );
    assert!(
        !text.contains("installed at"),
        "last_result must PREEMPT the per-state install hint:\n{text}"
    );
}

// Empty rows (selected index out of range), no confirm/result → the detail
// `else` branch yields an empty string. The panel still paints its title /
// socket / footer; no stale per-state hint leaks in.
#[test]
fn connection_panel_empty_rows_renders_panel_with_blank_detail() {
    use crate::tui::connection::LiveInfo;
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    r.set_connection_frame_parts(
        true,
        vec![],
        Vec::<LiveInfo>::new(),
        0,
        None,
        None,
        "socket  /tmp/p.sock  (listening)".into(),
    );
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(text.contains("Sources"), "title missing:\n{text}");
    assert!(text.contains("j/k move"), "footer missing:\n{text}");
    assert!(
        !text.contains("installed at"),
        "empty rows → blank detail, no install hint:\n{text}"
    );
    assert!(
        !text.contains("press t to connect"),
        "empty rows → blank detail, no connect hint:\n{text}"
    );
    assert!(
        !text.contains("not detected"),
        "empty rows → blank detail, no no-CLI hint:\n{text}"
    );
}

#[test]
fn dashboard_collapsed_big_tree_shows_badge_and_hides_children() {
    let mut r = build(120, 44, vec![]);
    let root_id = AgentId::from_transcript_path("/h/root.jsonl");
    let mut root = slot(root_id, 0, 0, t0());
    root.label = "cc\u{b7}root".into();
    let mut agents = vec![root];
    // 6 > AUTO_COLLAPSE_THRESHOLD (5) → the root auto-collapses on open.
    for i in 0..6 {
        let cid = AgentId::from_transcript_path(&format!("/h/root/subagents/agent-{i}.jsonl"));
        let mut c = slot(cid, 0, 1 + i, t0());
        c.label = format!("explorer{i}").into();
        c.parent_id = Some(root_id);
        agents.push(c);
    }
    let scene = scene_with(agents, 16);

    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    r.set_dashboard_frame_parts(true, rows, Some(root_id), 0);
    r.render(&scene, &pack(), t0()).unwrap();

    let text = frame_text(r.frame_buffer());
    assert!(text.contains("cc\u{b7}root"), "root row missing:\n{text}");
    assert!(
        text.contains("(6)"),
        "collapsed hidden-count badge missing:\n{text}"
    );
    // The popup's own count proves only the root is listed (children hidden);
    // a global `!contains("explorer0")` would false-fail on the office sprite
    // label behind the popup. Child-hiding in the model is covered by
    // `dashboard::tests::root_over_threshold_auto_collapses_and_hides_its_subtree`.
    assert!(
        text.contains("Agents (1)"),
        "collapsed tree must list exactly one row:\n{text}"
    );
}

#[test]
fn dashboard_closed_paints_no_popup() {
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![idle("/h/a.jsonl", 0, t0())], 16);
    r.set_dashboard_frame_parts(false, Vec::new(), None, 0);
    r.render(&scene, &pack(), t0()).unwrap();

    let text = frame_text(r.frame_buffer());
    assert!(
        !text.contains("Agents ("),
        "no dashboard popup when closed:\n{text}"
    );
}

/// The popup's box-bordered content lines (those containing the vertical rule),
/// so substring assertions don't false-match the office sprite labels behind it.
fn dash_popup(buf: &ratatui::buffer::Buffer) -> String {
    // The popup is borderless (no `│` to key on) but is the only region painted
    // with the UI `tooltip_bg` fill, so isolate it by background color. (The
    // pixel office never produces this exact chrome RGB.)
    let tb = pixtuoid_scene::theme::NORMAL.ui.tooltip_bg;
    let bg = ratatui::style::Color::Rgb(tb.r, tb.g, tb.b);
    let area = buf.area;
    let mut out = String::new();
    for y in area.y..area.y + area.height {
        let mut row = String::new();
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell((x, y)) {
                if cell.bg == bg {
                    row.push_str(cell.symbol());
                }
            }
        }
        if !row.trim().is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&row);
        }
    }
    out
}

#[test]
fn dashboard_renders_waiting_reason_and_active_without_detail() {
    let mut r = build(120, 44, vec![]);
    let mut w = idle("/h/w.jsonl", 0, t0());
    w.label = "cc\u{b7}wait".into();
    w.state = ActivityState::Waiting {
        reason: Arc::from("permission"),
    };
    let mut a = idle("/h/a.jsonl", 1, t0());
    a.label = "cc\u{b7}act".into();
    a.state = ActivityState::Active {
        tool_use_id: Some(Arc::from("t")),
        detail: None,
        kind: ToolKind::Other,
    };
    let scene = scene_with(vec![w, a], 16);

    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    r.set_dashboard_frame_parts(true, rows, None, 0);
    r.render(&scene, &pack(), t0()).unwrap();

    let popup = dash_popup(r.frame_buffer());
    assert!(
        popup.contains("waiting: permission"),
        "waiting reason missing:\n{popup}"
    );
    assert!(
        popup.contains("\u{25cf} active"),
        "active-without-detail must render the bare state word:\n{popup}"
    );
}

#[test]
fn dashboard_scrolls_to_keep_a_deep_selection_visible() {
    // 20 roots (> DASHBOARD_VIEWPORT_ROWS = 16) spread across floors so the
    // office only labels a couple — the popup lists all 20. Selecting row 18
    // must scroll the window down (painter re-clamps via clamp_scroll), so the
    // popup shows row 18 and NOT row 00.
    let mut agents = Vec::new();
    for i in 0..20 {
        let mut s = idle(&format!("/h/r{i}.jsonl"), i, t0());
        s.label = format!("row{i:02}").into();
        s.floor_idx = i % 10;
        agents.push(s);
    }
    let scene = scene_with(agents, 16);
    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    let row18 = rows[18].agent_id;
    let mut r = build(120, 44, vec![]);
    r.set_dashboard_frame_parts(true, rows, Some(row18), 0);
    r.render(&scene, &pack(), t0()).unwrap();

    let buf = r.frame_buffer();
    let text = frame_text(buf);
    let popup = dash_popup(buf);
    // The title sits on the top border (┌…┐), not a │ row — assert it on the full frame.
    assert!(text.contains("Agents (20)"), "all 20 rows counted:\n{text}");
    assert!(
        popup.contains("row18"),
        "deep selection scrolled into view (painter re-clamps scroll to the real window):\n{popup}"
    );
    assert!(
        !popup.contains("row00"),
        "top row must scroll off when a deep row is selected:\n{popup}"
    );
}

#[test]
fn dashboard_empty_scene_shows_placeholder() {
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    r.set_dashboard_frame_parts(true, Vec::new(), None, 0);
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(
        frame_text(r.frame_buffer()).contains("No active agents"),
        "empty dashboard must show the placeholder"
    );
}

// NOTE: a dedicated short-terminal clamp test isn't possible via the harness —
// draw_scene footer-onlys (skips the popup) when the office layout can't fit
// (terminal shorter than ~the office min height), and a 16-row popup (max 18
// tall) always fits whenever the office DOES render. So the painter's
// real-window re-clamp (clamp_scroll on visible = popup-inner-height) can only
// fire with visible == DASHBOARD_VIEWPORT_ROWS in practice — exercised by
// dashboard_scrolls_to_keep_a_deep_selection_visible above (scroll 0 → 3). The
// visible < viewport arithmetic is covered directly by
// dashboard::tests::clamp_scroll_* with small viewports.

#[test]
fn dashboard_badge_text_present_for_cc_and_cx() {
    let mut r = build(120, 44, vec![]);
    let mut cc_slot = idle("/h/cc.jsonl", 0, t0());
    cc_slot.source = Arc::from("claude-code");
    cc_slot.label = "cc\u{b7}alpha".into();
    let mut cx_slot = idle("/h/cx.jsonl", 1, t0());
    cx_slot.source = Arc::from("codex");
    cx_slot.label = "cx\u{b7}beta".into();
    let scene = scene_with(vec![cc_slot, cx_slot], 16);

    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    r.set_dashboard_frame_parts(true, rows, None, 0);
    r.render(&scene, &pack(), t0()).unwrap();

    let popup = dash_popup(r.frame_buffer());
    assert!(popup.contains("[cc]"), "cc badge missing:\n{popup}");
    assert!(popup.contains("[cx]"), "cx badge missing:\n{popup}");
}

#[test]
fn dashboard_overflow_cue_appears_below_when_more_than_viewport() {
    // 20 root slots, scroll=0 → visible ≤ DASHBOARD_VIEWPORT_ROWS=16 → hidden_below > 0.
    let mut agents = Vec::new();
    for i in 0..20 {
        let mut s = idle(&format!("/h/r{i}.jsonl"), i % 16, t0());
        s.label = format!("overflow{i:02}").into();
        s.floor_idx = i % 10;
        agents.push(s);
    }
    let scene = scene_with(agents, 32);
    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    let mut r = build(120, 44, vec![]);
    r.set_dashboard_frame_parts(true, rows, None, 0);
    r.render(&scene, &pack(), t0()).unwrap();

    let popup = dash_popup(r.frame_buffer());
    assert!(
        popup.contains('\u{22ee}'),
        "overflow cue ⋮ must appear:\n{popup}"
    );
}

#[test]
fn dashboard_overflow_cue_absent_when_all_visible() {
    // 8 root slots ≤ DASHBOARD_VIEWPORT_ROWS=16 → no hidden rows → no cue.
    let mut agents = Vec::new();
    for i in 0..8 {
        let mut s = idle(&format!("/h/r{i}.jsonl"), i, t0());
        s.label = format!("fit{i:02}").into();
        agents.push(s);
    }
    let scene = scene_with(agents, 16);
    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    let mut r = build(120, 44, vec![]);
    r.set_dashboard_frame_parts(true, rows, None, 0);
    r.render(&scene, &pack(), t0()).unwrap();

    let popup = dash_popup(r.frame_buffer());
    assert!(
        !popup.contains('\u{22ee}'),
        "no overflow cue for 8 rows:\n{popup}"
    );
}

#[test]
fn dashboard_overflow_cue_keeps_a_bottom_navigated_selection_visible() {
    // 25 rows; select row20 — clamp_scroll parks it at the window's bottom with
    // several rows still below. The cue must NOT displace the selected row.
    let mut agents = Vec::new();
    for i in 0..25 {
        let mut s = idle(&format!("/h/r{i}.jsonl"), i, t0());
        s.label = format!("row{i:02}").into();
        s.floor_idx = i % 10;
        agents.push(s);
    }
    let scene = scene_with(agents, 16);
    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    let row20 = rows[20].agent_id;
    let mut r = build(120, 44, vec![]);
    r.set_dashboard_frame_parts(true, rows, Some(row20), 0);
    r.render(&scene, &pack(), t0()).unwrap();
    let popup = dash_popup(r.frame_buffer());
    assert!(
        popup.contains("row20"),
        "selected bottom row must stay visible when a cue shows:\n{popup}"
    );
    assert!(
        popup.contains('\u{22ee}'),
        "overflow cue present (rows below):\n{popup}"
    );
}

#[test]
fn dashboard_overflow_no_blank_line_when_selection_is_last_row() {
    // 17 rows (> DASHBOARD_VIEWPORT_ROWS=16) → overflow. Selecting the LAST row
    // scrolls to the very end where nothing is below: no cue is needed, so the
    // popup must fill all 16 visible lines (NOT reserve a now-empty cue line).
    let mut agents = Vec::new();
    for i in 0..17 {
        let mut s = idle(&format!("/h/r{i}.jsonl"), i, t0());
        s.label = format!("row{i:02}").into();
        s.floor_idx = i % 10;
        agents.push(s);
    }
    let scene = scene_with(agents, 16);
    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    let last = rows[16].agent_id;
    let mut r = build(120, 44, vec![]);
    r.set_dashboard_frame_parts(true, rows, Some(last), 0);
    r.render(&scene, &pack(), t0()).unwrap();
    let popup = dash_popup(r.frame_buffer());
    assert!(
        popup.contains("row16"),
        "selected last row visible:\n{popup}"
    );
    // The fix renders 16 rows (rows 1..16); the blank-line bug renders only 15
    // (rows 2..16, plus a blank reserved line), so `row01` present distinguishes.
    assert!(
        popup.contains("row01"),
        "all 16 visible lines must be filled — no blank reserved cue line:\n{popup}"
    );
    assert!(
        !popup.contains('\u{22ee}'),
        "no cue when scrolled to the end (nothing below):\n{popup}"
    );
}
