use super::*;
use footer::{build_status_spans, build_status_summary, FooterStats};
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex, SceneState};
use std::path::PathBuf;
use std::sync::Arc;
use wall_board::BOARD_W;

fn stat_slot(path: &str, state: ActivityState, exiting: bool) -> AgentSlot {
    let now = SystemTime::UNIX_EPOCH;
    AgentSlot {
        agent_id: AgentId::from_transcript_path(path),
        source: Arc::from("claude-code"),
        session_id: Arc::from("s"),
        cwd: Arc::from(PathBuf::from("/p").as_path()),
        label: "x".into(),
        state,
        state_started_at: now,
        created_at: now,
        last_event_at: now,
        exiting_at: exiting.then_some(now),
        pending_idle_at: None,
        desk_index: GlobalDeskIndex(0),
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
    }
}

#[test]
fn scene_stats_buckets_exiting_first_and_totals() {
    use pixtuoid_core::state::ToolKind;
    let active = || ActivityState::Active {
        tool_use_id: None,
        detail: None,
        kind: ToolKind::Other,
    };
    let mut scene = SceneState::uniform(16);
    for s in [
        stat_slot("/exiting-active.jsonl", active(), true),
        stat_slot("/live-active.jsonl", active(), false),
        stat_slot(
            "/waiting.jsonl",
            ActivityState::Waiting {
                reason: Arc::from("perm"),
            },
            false,
        ),
        stat_slot("/idle.jsonl", ActivityState::Idle, false),
    ] {
        scene.agents.insert(s.agent_id, s);
    }
    let c = scene_stats(&scene);
    assert_eq!(
        c.exiting, 1,
        "an exiting agent buckets as exiting even mid-Active"
    );
    assert_eq!(c.active, 1, "only the LIVE Active counts as active");
    assert_eq!(c.waiting, 1);
    assert_eq!(c.idle, 1);
    assert_eq!(c.total, 4);
    assert_eq!(
        c.active + c.waiting + c.idle + c.exiting,
        c.total,
        "the four buckets must partition the total"
    );
}

#[test]
fn scene_stats_empty_scene_is_all_zero() {
    let c = scene_stats(&SceneState::uniform(16));
    assert_eq!(c, StateCounts::default());
    assert_eq!(c.total, 0);
}

#[test]
fn state_vocab_is_total_and_distinct() {
    use std::collections::HashSet;
    let kinds = StateKind::ALL;
    assert_eq!(kinds.len(), 4, "the vocab covers exactly the four buckets");
    let glyphs: HashSet<char> = kinds.iter().map(|k| k.glyph()).collect();
    let letters: HashSet<char> = kinds.iter().map(|k| k.letter()).collect();
    let words: HashSet<&str> = kinds.iter().map(|k| k.word()).collect();
    assert_eq!(glyphs.len(), 4, "each state has a distinct glyph");
    assert_eq!(letters.len(), 4, "each state has a distinct letter");
    assert_eq!(words.len(), 4, "each state has a distinct word");
    let t = &pixtuoid_scene::theme::NORMAL;
    assert_eq!(
        state_color(StateKind::Waiting, t),
        to_color(t.ui.label_waiting)
    );
    assert_eq!(
        state_color(StateKind::Exiting, t),
        to_color(t.ui.label_exiting)
    );
}

#[test]
fn display_width_counts_terminal_columns_not_chars() {
    // The state/HUD glyphs are all East-Asian *ambiguous* = 1 column under the
    // non-CJK `.width()`, so this measure == chars().count() for them.
    assert_eq!(display_width("\u{b7}\u{d7}\u{2191}\u{2193}"), 4); // · × ↑ ↓
    assert_eq!(
        display_width("\u{25cf}\u{25d0}\u{25cb}\u{25cc}"),
        4,
        "● ◐ ○ ◌ are one column each"
    );
    assert_eq!(display_width("[q]uit"), 6);
    assert_eq!(display_width("\u{1f99e}"), 2); // 🦞
    assert_eq!(display_width("a\u{0301}"), 1);
}

// `pixtuoid_scene::footer::build_footer` measures column width via
// `chars().count()` (no `unicode-width` dep — the `board` discipline keeps `scene`
// window/terminal-free), which is byte-identical to `display_width` ONLY while
// every footer glyph is single-column.
#[test]
fn footer_vocabulary_is_single_column_so_scene_chars_count_matches_display_width() {
    let vocab = "\u{b7}\u{d7}\u{2191}\u{2193}\u{25cf}\u{25d0}\u{25cb}\u{25cc}\u{2b22}\u{25b2}\u{2669}\u{26a0}\u{2026}";
    for c in vocab.chars() {
        let s = c.to_string();
        assert_eq!(
            display_width(&s),
            s.chars().count(),
            "footer glyph U+{:04X} {c:?} must be single-column, else scene's chars().count() drifts from display_width",
            c as u32,
        );
    }
}

#[test]
fn state_count_maps_each_kind() {
    let c = StateCounts {
        active: 3,
        waiting: 2,
        idle: 7,
        exiting: 1,
        total: 13,
    };
    assert_eq!(StateKind::Active.count(c), 3);
    assert_eq!(StateKind::Waiting.count(c), 2);
    assert_eq!(StateKind::Idle.count(c), 7);
    assert_eq!(StateKind::Exiting.count(c), 1);
}

// Busy needs a placeholder in-flight run key because Busy is DERIVED from the run
// set, never stored — without one the fixture is silently Idle.
fn daemon(state: pixtuoid_core::state::DaemonState) -> pixtuoid_core::state::DaemonPresence {
    use pixtuoid_core::state::{DaemonLiveness, DaemonState};
    let (liveness, in_flight_runs) = match state {
        DaemonState::Idle => (DaemonLiveness::UP, Default::default()),
        DaemonState::Busy => (
            DaemonLiveness::UP,
            [("fixture-run".to_string(), SystemTime::UNIX_EPOCH)]
                .into_iter()
                .collect::<std::collections::BTreeMap<String, SystemTime>>(),
        ),
        DaemonState::Degraded => (DaemonLiveness::Up { degraded: true }, Default::default()),
        DaemonState::Down => (DaemonLiveness::Down, Default::default()),
    };
    pixtuoid_core::state::DaemonPresence {
        liveness,
        active_sessions: 0,
        last_seen: SystemTime::UNIX_EPOCH,
        entered_at: SystemTime::UNIX_EPOCH,
        in_flight_runs,
        current_pid: None,
    }
}

#[test]
fn gateway_rollup_is_worst_of() {
    use pixtuoid_core::state::DaemonState;
    assert_eq!(gateway_rollup(std::iter::empty()), None);
    let busy = daemon(DaemonState::Busy);
    assert_eq!(
        gateway_rollup(std::iter::once(&busy)),
        Some(DaemonState::Busy)
    );
    let (idle, degraded, down) = (
        daemon(DaemonState::Idle),
        daemon(DaemonState::Degraded),
        daemon(DaemonState::Down),
    );
    assert_eq!(
        gateway_rollup([&busy, &idle, &degraded, &down].into_iter()),
        Some(DaemonState::Down)
    );
    assert_eq!(
        gateway_rollup([&idle, &degraded].into_iter()),
        Some(DaemonState::Degraded)
    );
}

#[test]
fn per_floor_buckets_by_floor_idx() {
    use pixtuoid_core::state::ToolKind;
    let active = || ActivityState::Active {
        tool_use_id: None,
        detail: None,
        kind: ToolKind::Other,
    };
    let mut scene = SceneState::uniform(16);
    let add = |scene: &mut SceneState, path, state, exiting, floor: usize| {
        let mut s = stat_slot(path, state, exiting);
        s.floor_idx = floor;
        scene.agents.insert(s.agent_id, s);
    };
    add(&mut scene, "/f0a.jsonl", active(), false, 0);
    add(&mut scene, "/f0b.jsonl", active(), false, 0);
    add(
        &mut scene,
        "/f1w.jsonl",
        ActivityState::Waiting {
            reason: Arc::from("p"),
        },
        false,
        1,
    );
    add(&mut scene, "/f1x.jsonl", active(), true, 1); // exiting on floor 1
    add(&mut scene, "/f2i.jsonl", ActivityState::Idle, false, 2);

    let pf = per_floor_counts(&scene);
    assert_eq!((pf[0].active, pf[0].total), (2, 2));
    assert_eq!((pf[1].waiting, pf[1].exiting, pf[1].total), (1, 1, 2));
    assert_eq!((pf[2].idle, pf[2].total), (1, 1));
    assert_eq!(pf[3], StateCounts::default(), "an untouched floor is zero");
}

// A 10-char string scrolled in a 5-col window: max_off=5, scroll_ms=750,
// pause=1200, cycle = 2*1200 + 2*750 = 3900. Phases (ms):
//   [0,1200)        hold head  -> "ABCDE"
//   [1200,1950)     scroll out -> off=(p-1200)/150
//   [1950,3150)     hold tail  -> "FGHIJ"
//   [3150,3900)     scroll back-> off=5-((p-3150)/150)
const M: &str = "ABCDEFGHIJ";
fn at(ms: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(ms)
}

#[test]
fn marquee_fits_returns_unchanged_no_ellipsis() {
    assert_eq!(marquee_window("short", 10, at(99_999)), "short");
    assert_eq!(marquee_window("EXACTLYTEN", 10, at(99_999)), "EXACTLYTEN");
}

#[test]
fn marquee_zero_width_is_empty() {
    assert_eq!(marquee_window(M, 0, at(0)), "");
    assert_eq!(marquee_window("", 0, at(0)), "");
}

#[test]
fn marquee_holds_head_then_tail() {
    assert_eq!(marquee_window(M, 5, at(0)), "ABCDE");
    assert_eq!(marquee_window(M, 5, at(2000)), "FGHIJ");
}

#[test]
fn marquee_scrolls_out_and_back() {
    assert_eq!(marquee_window(M, 5, at(1500)), "CDEFG");
    assert_eq!(marquee_window(M, 5, at(3450)), "DEFGH");
}

#[test]
fn marquee_is_deterministic_and_cycles() {
    assert_eq!(
        marquee_window(M, 5, at(1500)),
        marquee_window(M, 5, at(1500))
    );
    assert_eq!(
        marquee_window(M, 5, at(1500)),
        marquee_window(M, 5, at(1500 + 3900))
    );
}

#[test]
fn marquee_min_overflow_reaches_both_ends() {
    // len == width + 1 (max_off=1), so scroll_ms=150 and the cycle is 2700.
    let s = "ABCDEF"; // len 6, width 5
    assert_eq!(marquee_window(s, 5, at(0)), "ABCDE"); // head
    assert_eq!(marquee_window(s, 5, at(1500)), "BCDEF"); // tail-hold [1350,2550)
}

#[test]
fn marquee_never_panics_on_multibyte() {
    let s = "café·ünïcödé·scroll·test";
    for ms in [0u64, 500, 1500, 2500, 5000, 9999] {
        let out = marquee_window(s, 8, at(ms));
        assert_eq!(out.chars().count(), 8, "ms={ms}: {out:?}");
    }
}

#[test]
fn marquee_or_truncate_selected_scrolls_unselected_ellipsizes() {
    assert_eq!(marquee_or_truncate(M, 5, true, at(0)), "ABCDE");
    assert_eq!(marquee_or_truncate(M, 5, false, at(0)), "ABCD\u{2026}");
}

fn slot_with(state: ActivityState, label: &str) -> AgentSlot {
    AgentSlot {
        agent_id: AgentId::from_transcript_path(&format!("/p/{label}.jsonl")),
        source: Arc::from("claude-code"),
        session_id: Arc::from("s"),
        cwd: Arc::from(PathBuf::from("/p").as_path()),
        label: label.into(),
        state,
        state_started_at: SystemTime::UNIX_EPOCH,
        created_at: SystemTime::UNIX_EPOCH,
        last_event_at: SystemTime::UNIX_EPOCH,
        exiting_at: None,
        pending_idle_at: None,

        desk_index: GlobalDeskIndex(0),
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
    }
}
fn active_with(detail: &str, label: &str) -> AgentSlot {
    slot_with(
        ActivityState::Active {
            tool_use_id: Some(Arc::from("t")),
            detail: Some(Arc::from(detail)),
            kind: pixtuoid_core::state::ToolKind::from_display(detail),
        },
        label,
    )
}
fn waiting(label: &str) -> AgentSlot {
    slot_with(
        ActivityState::Waiting {
            reason: Arc::from("perm"),
        },
        label,
    )
}
fn idle(label: &str) -> AgentSlot {
    slot_with(ActivityState::Idle, label)
}
fn active_kind(detail: &str, kind: pixtuoid_core::state::ToolKind, label: &str) -> AgentSlot {
    slot_with(
        ActivityState::Active {
            tool_use_id: Some(Arc::from("t")),
            detail: Some(Arc::from(detail)),
            kind,
        },
        label,
    )
}
fn scene_of(slots: Vec<AgentSlot>) -> SceneState {
    let mut s = SceneState::uniform(16);
    for slot in slots {
        s.agents.insert(slot.agent_id, slot);
    }
    s
}

/// Assemble a `FooterStats` the way `draw_scene` does, then render the
/// plain-string footer oracle.
fn footer_line(
    scene: &SceneState,
    width: u16,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
    warning: Option<&str>,
) -> String {
    let pf = per_floor_counts(scene);
    let stats = FooterStats {
        counts: scene_stats(scene),
        per_floor: &pf,
        gateway: None,
        audio_audible: false,
        volume_flash: None,
    };
    build_status_summary(scene, &stats, width, floor_info, warning)
}

const QUIT_SUFFIX: &str = " [?]help [p]ause [t]heme [q]uit ";

#[test]
fn source_warning_message_formats_by_death_count() {
    use pixtuoid_core::source::manager::SourceDeath;
    let d = |s: &str| SourceDeath::new(s, "boom");
    assert_eq!(super::source_warning_message(&[]), None);
    assert_eq!(
        super::source_warning_message(&[d("claude-code")]).unwrap(),
        "claude-code source died — its agents are frozen; restart pixtuoid (see log)"
    );
    assert_eq!(
        super::source_warning_message(&[d("claude-code"), d("codex")]).unwrap(),
        "2 sources died — restart pixtuoid (see log)"
    );
}

#[test]
fn footer_source_warning_replaces_stats_and_keeps_quit() {
    let s = scene_of(vec![idle("myproject")]);
    let line = footer_line(
        &s,
        100,
        None,
        Some("claude-code source died — its agents are frozen; restart pixtuoid (see log)"),
    );
    assert!(line.contains('⚠'), "warning marker present: {line}");
    assert!(line.contains("claude-code source died"), "got: {line}");
    assert!(line.ends_with(" [q]uit "), "quit hint survives: {line}");
    assert!(
        !line.contains('\u{25cb}') && !line.contains('\u{25cf}'),
        "stale state rungs are replaced by the warning: {line}"
    );
    insta::assert_snapshot!(line);
}

#[test]
fn footer_source_warning_survives_every_width() {
    let s = scene_of(vec![idle("myproject")]);
    for w in [20u16, 30, 40, 60, 80] {
        let line = footer_line(
            &s,
            w,
            None,
            Some("claude-code source died — its agents are frozen; restart pixtuoid (see log)"),
        );
        assert!(
            line.contains('⚠') || line.contains('…'),
            "warning must never be tiered away (w={w}): {line}"
        );
        assert!(
            line.chars().count() <= w as usize,
            "must fit the row (w={w}): {line:?}"
        );
    }
}

#[test]
fn footer_zero_agents() {
    let s = scene_of(vec![]);
    let line = footer_line(&s, 80, None, None);
    assert_eq!(line.len(), 80, "should pad to full width");
    insta::assert_snapshot!(line);
}

#[test]
fn footer_single_idle_agent() {
    let s = scene_of(vec![idle("myproject")]);
    let line = footer_line(&s, 80, None, None);
    assert!(line.contains(" 1 \u{b7} \u{25cb}1 I"), "got: {line}");
    insta::assert_snapshot!(line);
}

#[test]
fn footer_full_width_mixed_states() {
    let s = scene_of(vec![
        active_with("Edit src/a.rs", "a"),
        active_with("Edit src/b.rs", "b"),
        active_with("Bash: ls", "c"),
        waiting("d"),
        waiting("e"),
        idle("f"),
        idle("g"),
        idle("h"),
    ]);
    let line = footer_line(&s, 120, None, None);
    for frag in [
        "\u{25cf}3 A",
        "\u{25d0}2 W",
        "\u{25cb}3 I",
        "Edit\u{d7}2",
        "Bash\u{d7}1",
    ] {
        assert!(line.contains(frag), "missing {frag:?} in: {line}");
    }
    insta::assert_snapshot!(line);
}

#[test]
fn footer_medium_width_compact() {
    let s = scene_of(vec![
        active_with("Edit src/a.rs", "a"),
        waiting("b"),
        idle("c"),
    ]);
    let line = footer_line(&s, 60, None, None);
    assert!(!line.contains("Edit"), "medium drops tools: {line}");
    assert!(line.contains("\u{25cf}1A"), "compact active rung: {line}");
    insta::assert_snapshot!(line);
}

#[test]
fn footer_minimal_width() {
    let s = scene_of(vec![idle("a"), idle("b")]);
    let w = QUIT_SUFFIX.len() + 6;
    let line = footer_line(&s, w as u16, None, None);
    assert_eq!(line.len(), w);
    insta::assert_snapshot!(line);
}

#[test]
fn footer_quit_only_below_threshold() {
    let s = scene_of(vec![idle("a")]);
    let w = QUIT_SUFFIX.len();
    let line = footer_line(&s, w as u16, None, None);
    insta::assert_snapshot!(line);
}

#[test]
fn footer_caps_tools_at_four() {
    let s = scene_of(vec![
        active_with("Edit x", "a"),
        active_with("Bash x", "b"),
        active_with("Read x", "c"),
        active_with("Write x", "d"),
        active_with("Grep x", "e"),
        active_with("Glob x", "f"),
    ]);
    let line = footer_line(&s, 200, None, None);
    let crosses = line.matches('\u{00d7}').count();
    assert_eq!(crosses, 4, "expected <=4 tools in breakdown");
    insta::assert_snapshot!(line);
}

#[test]
fn footer_minimal_leads_with_waiting_alarm() {
    let s = scene_of(vec![waiting("a"), waiting("b"), idle("c"), idle("d")]);
    let w = QUIT_SUFFIX.len() + 10;
    let line = footer_line(&s, w as u16, None, None);
    assert!(
        line.contains("\u{25b2}2 \u{b7} 4"),
        "▲2 · 4 (alarm leads): {line}"
    );
}

#[test]
fn footer_death_keeps_the_waiting_alarm() {
    let s = scene_of(vec![waiting("a"), waiting("b"), idle("c")]);
    let line = footer_line(&s, 120, None, Some("codex disconnected"));
    assert!(line.contains('\u{26a0}'), "warning present: {line}");
    assert!(
        line.contains("\u{25b2}2 need you"),
        "alarm survives death: {line}"
    );
}

fn fi(current: usize, total_floors: usize, total_agents: usize) -> crate::tui::renderer::FloorInfo {
    crate::tui::renderer::FloorInfo {
        current,
        total_floors,
        total_agents,
    }
}

#[test]
fn footer_with_floor_info() {
    let s = scene_of(vec![idle("a"), idle("b")]);
    let line = footer_line(&s, 120, Some(fi(2, 3, 5)), None);
    assert!(line.contains(" F2/3 "), "floor breadcrumb: {line}");
    insta::assert_snapshot!(line);
}

// Direct assertions for count_str: snapshots alone mask regressions, being easy to
// ratify away in `cargo insta review`.

#[test]
fn count_str_single_floor_shows_bare_n() {
    let s = scene_of(vec![idle("a"), idle("b")]);
    let line = footer_line(&s, 120, None, None);
    assert!(line.contains(" 2 \u{b7} \u{25cb}2 I"), "got: {line}");
    assert!(!line.contains("2/"), "no slash on a single floor: {line}");
}

#[test]
fn count_str_multi_floor_shows_n_slash_total() {
    let s = scene_of(vec![idle("a"), idle("b")]);
    let line = footer_line(&s, 120, Some(fi(2, 3, 5)), None);
    assert!(line.contains(" 2/5 \u{b7}"), "got: {line}");
}

#[test]
fn count_str_multi_floor_shows_slash_even_when_total_equals_n() {
    let s = scene_of(vec![idle("a"), idle("b")]);
    let line = footer_line(&s, 120, Some(fi(1, 3, 2)), None);
    assert!(line.contains(" 2/2 \u{b7}"), "got: {line}");
}

#[test]
fn count_str_empty_floor_still_shows_total() {
    let s = scene_of(vec![]);
    let line = footer_line(&s, 120, Some(fi(2, 3, 5)), None);
    assert!(line.contains(" 0/5 "), "got: {line}");
}

#[test]
fn count_str_multi_floor_keeps_slash_at_narrow_tier() {
    let s = scene_of(vec![idle("a"), idle("b"), idle("c")]);
    let line = footer_line(&s, 50, Some(fi(1, 3, 10)), None);
    assert!(
        line.contains("3/10"),
        "slash kept at the narrow tier: {line}"
    );
}

fn footer_spans_text(
    scene: &SceneState,
    width: u16,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
    theme: &pixtuoid_scene::theme::Theme,
) -> String {
    let pf = per_floor_counts(scene);
    let stats = FooterStats {
        counts: scene_stats(scene),
        per_floor: &pf,
        gateway: None,
        audio_audible: false,
        volume_flash: None,
    };
    build_status_spans(scene, &stats, width, floor_info, theme, None)
        .iter()
        .map(|sp| sp.content.as_ref().to_string())
        .collect()
}

#[test]
fn status_spans_text_matches_summary_across_tiers() {
    let theme = &pixtuoid_scene::theme::NORMAL;
    let s = scene_of(vec![
        active_with("Edit src/a.rs", "a"),
        waiting("b"),
        idle("c"),
        idle("d"),
    ]);
    for (w, fl) in [
        (120u16, None),
        (60, None),
        (28, None),
        (10, None),
        (120, Some(fi(2, 3, 9))),
    ] {
        let summary = footer_line(&s, w, fl, None);
        let spans_text = footer_spans_text(&s, w, fl, theme);
        assert_eq!(spans_text, summary, "tier width {w} drifted");
    }
}

#[test]
fn status_spans_color_code_state_segments() {
    let theme = &pixtuoid_scene::theme::NORMAL;
    let s = scene_of(vec![
        active_with("Edit src/a.rs", "a"),
        waiting("b"),
        idle("c"),
    ]);
    let pf = per_floor_counts(&s);
    let stats = FooterStats {
        counts: scene_stats(&s),
        per_floor: &pf,
        gateway: None,
        audio_audible: false,
        volume_flash: None,
    };
    let spans = build_status_spans(&s, &stats, 120, None, theme, None);
    let active = spans
        .iter()
        .find(|sp| sp.content.contains('\u{25cf}'))
        .unwrap();
    let waiting = spans
        .iter()
        .find(|sp| sp.content.contains('\u{25d0}'))
        .unwrap();
    assert_eq!(active.style.fg, Some(to_color(theme.ui.label_active)));
    assert_eq!(waiting.style.fg, Some(to_color(theme.ui.label_waiting)));
}

#[test]
fn footer_counts_agree_with_board_on_walkout() {
    let mut gone = active_with("Edit x", "gone");
    gone.exiting_at = Some(SystemTime::UNIX_EPOCH);
    let s = scene_of(vec![
        active_with("Edit a", "a"),
        active_with("Bash b", "b"),
        gone,
    ]);
    let c = scene_stats(&s);
    assert_eq!((c.active, c.exiting, c.total), (2, 1, 3), "shared spine");
    let line = footer_line(&s, 160, None, None);
    assert!(
        line.contains(" 3 \u{b7} \u{25cf}2 A"),
        "total incl. exiting: {line}"
    );
    assert!(
        line.contains("\u{25cc}1 x"),
        "first-class exiting rung: {line}"
    );
}

#[test]
fn footer_tool_hue_reads_kind_field() {
    // A Task dispatch DISPLAYS "Delegating" but its typed kind is Task, so the hue
    // must never come from a re-parse of the displayed string.
    let theme = &pixtuoid_scene::theme::NORMAL;
    let s = scene_of(vec![active_kind(
        "Delegating",
        pixtuoid_core::state::ToolKind::Task,
        "lead",
    )]);
    let pf = per_floor_counts(&s);
    let stats = FooterStats {
        counts: scene_stats(&s),
        per_floor: &pf,
        gateway: None,
        audio_audible: false,
        volume_flash: None,
    };
    let spans = build_status_spans(&s, &stats, 160, None, theme, None);
    let tool = spans
        .iter()
        .find(|sp| sp.content.contains("Delegating"))
        .expect("tool segment present");
    let expected = to_color(pixtuoid_scene::pixel_painter::tool_glow_for_kind(
        pixtuoid_core::state::ToolKind::Task,
        &theme.tool_glow,
    ));
    assert_eq!(tool.style.fg, Some(expected), "hue from the typed kind");
    assert_eq!(
        expected,
        to_color(theme.tool_glow.agent),
        "== the agent glow"
    );
}

#[test]
fn footer_gateway_chip_reflects_rollup_and_suppresses_when_absent() {
    use pixtuoid_core::state::DaemonState;
    let s = scene_of(vec![idle("a")]);
    let pf = per_floor_counts(&s);
    let with_gw = FooterStats {
        counts: scene_stats(&s),
        per_floor: &pf,
        gateway: Some(DaemonState::Degraded),
        audio_audible: false,
        volume_flash: None,
    };
    let line = build_status_summary(&s, &with_gw, 160, None, None);
    assert!(line.contains("\u{2b22}gw err"), "degraded chip: {line}");
    let no_gw = FooterStats {
        counts: scene_stats(&s),
        per_floor: &pf,
        gateway: None,
        audio_audible: false,
        volume_flash: None,
    };
    let line2 = build_status_summary(&s, &no_gw, 160, None, None);
    assert!(
        !line2.contains("gw"),
        "chip suppressed when no daemon: {line2}"
    );
}

#[test]
fn footer_cross_floor_alarm_points_at_waiting_floor() {
    // The waiting agent sits on floor 2 (index 1) while floor 1 is the one shown —
    // `per_floor` is office-wide, not the projected floor.
    let s = scene_of(vec![idle("a")]);
    let mut pf = per_floor_counts(&s);
    pf[1].waiting = 1;
    pf[1].total = 1;
    let stats = FooterStats {
        counts: scene_stats(&s),
        per_floor: &pf,
        gateway: None,
        audio_audible: false,
        volume_flash: None,
    };
    let line = build_status_summary(&s, &stats, 160, Some(fi(1, 3, 2)), None);
    assert!(
        line.contains("\u{25b2}F2"),
        "cross-floor waiting cue: {line}"
    );
}

#[test]
fn board_width_pins_to_neon_panel_interior() {
    assert_eq!(
        BOARD_W,
        pixtuoid_scene::pixel_painter::NEON_PANEL_INNER_W,
        "board width must equal the painted panel's dark interior width"
    );
}
