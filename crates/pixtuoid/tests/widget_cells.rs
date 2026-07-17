//! Widget cell assertion tests.
//!
//! After `draw_scene` renders into a `TestBackend`, inspect the ratatui buffer
//! cells to verify that footer, elevator indicator, and wall-display branding
//! widgets wrote the expected text at the expected positions.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use pixtuoid::tui::renderer::draw_scene;
use pixtuoid_core::state::{ActivityState, ToolKind};
use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex, SceneState};
use pixtuoid_scene::embedded_pack::load_sprite_pack;
use pixtuoid_scene::theme;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

/// Deterministic timestamp shared by all tests.
const NOW_SECS: u64 = 1_716_286_800;

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(NOW_SECS)
}

fn fixture_scene(now: SystemTime) -> SceneState {
    let mut s = SceneState::uniform(12);
    let age_offset = Duration::from_secs(60);
    let cases: &[(&str, ActivityState)] = &[
        (
            "agent-a",
            ActivityState::Active {
                tool_use_id: Some("tu_a".into()),
                detail: Some("Write".into()),
                kind: ToolKind::Edit,
            },
        ),
        ("agent-b", ActivityState::Idle),
        (
            "agent-c",
            ActivityState::Waiting {
                reason: "perm?".into(),
            },
        ),
        ("agent-d", ActivityState::Idle),
    ];
    for (i, (key, state)) in cases.iter().enumerate() {
        let id = AgentId::from_transcript_path(&format!("/demo/{key}.jsonl"));
        let created_at = now - age_offset;
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: Arc::from("claude-code"),
                session_id: Arc::from(format!("session-{i}").as_str()),
                cwd: Arc::from(PathBuf::from("/demo").as_path()),
                label: (*key).into(),
                state: state.clone(),
                state_started_at: now,
                last_event_at: now,
                created_at,
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
    s
}

/// Render a scene and return the `TestBackend` buffer plus dimensions.
fn render_and_get_buffer(
    now: SystemTime,
    floor_info: Option<pixtuoid::tui::renderer::FloorInfo>,
) -> (Buffer, u16, u16) {
    let w = 96u16;
    let h = 48u16;
    let scene = fixture_scene(now);
    let backend = TestBackend::new(w, h);
    let mut term = Terminal::new(backend).unwrap();
    let pack = load_sprite_pack(None).unwrap();
    make_draw_ctx!(draw_ctx, floor_info: floor_info);
    draw_scene(&mut term, &scene, &pack, now, &mut draw_ctx).unwrap();
    let buffer = term.backend().buffer().clone();
    (buffer, w, h)
}

/// Extract a single row from the buffer as a String.
fn row_text(buf: &Buffer, y: u16, w: u16) -> String {
    (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect()
}

// ---------------------------------------------------------------------------
// Footer tests
// ---------------------------------------------------------------------------

#[test]
fn footer_contains_quit_hint() {
    let (buf, w, h) = render_and_get_buffer(now(), None);
    let bottom = row_text(&buf, h - 1, w);
    assert!(
        bottom.contains("q"),
        "footer should contain quit hint, got: {bottom:?}"
    );
}

#[test]
fn footer_shows_agent_count() {
    let (buf, w, h) = render_and_get_buffer(now(), None);
    let bottom = row_text(&buf, h - 1, w);
    // fixture_scene creates 4 agents
    assert!(
        bottom.contains('4'),
        "footer should contain agent count '4', got: {bottom:?}"
    );
}

// ---------------------------------------------------------------------------
// Elevator indicator test
// ---------------------------------------------------------------------------

#[test]
fn elevator_indicator_visible() {
    // Pass floor_info so the elevator door is placed and the indicator paints.
    let (buf, w, h) = render_and_get_buffer(
        now(),
        Some(pixtuoid::tui::renderer::FloorInfo {
            current: 1,
            total_floors: 2,
            total_agents: 0,
        }),
    );
    // Scan all rows for "F1" -- the elevator indicator renders " ▲ F1 ▼ ".
    let mut found = false;
    for y in 0..h {
        let row = row_text(&buf, y, w);
        if row.contains("F1") {
            found = true;
            break;
        }
    }
    assert!(found, "elevator indicator with 'F1' not found in any row");
}

// ---------------------------------------------------------------------------
// Wall display branding test
// ---------------------------------------------------------------------------

#[test]
fn branding_visible_in_wall_display() {
    let (buf, w, h) = render_and_get_buffer(now(), None);
    // The wall display branding "pixtuoid" is painted in the top rows
    // by paint_wall_display. Scan the upper quarter for the text.
    let upper_quarter = h / 4;
    let mut found = false;
    for y in 0..upper_quarter {
        let row = row_text(&buf, y, w);
        if row.contains("pixtuoid") {
            found = true;
            break;
        }
    }
    assert!(
        found,
        "branding 'pixtuoid' not found in the upper quarter of the display"
    );
}

// ---------------------------------------------------------------------------
// Chitchat bubble test
// ---------------------------------------------------------------------------

#[test]
fn chitchat_bubble_text_appears_in_buffer() {
    use pixtuoid_scene::chitchat::ChitchatBubble;
    use pixtuoid_scene::layout::Point;

    let w = 60u16;
    let h = 30u16;
    let backend = TestBackend::new(w, h);
    let mut term = Terminal::new(backend).unwrap();
    let scene_rect = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: w,
        height: h,
    };
    let bubble_text = "LGTM!";
    let bubbles = vec![ChitchatBubble {
        text: bubble_text,
        anchor: Point { x: 30, y: 40 },
    }];

    term.draw(|f| {
        pixtuoid::tui::widgets::paint_chitchat_bubbles(f, &bubbles, scene_rect, &theme::NORMAL);
    })
    .unwrap();

    let buf = term.backend().buffer();
    let mut found = false;
    for y in 0..h {
        let row = row_text(buf, y, w);
        if row.contains(bubble_text) {
            found = true;
            break;
        }
    }
    assert!(
        found,
        "chitchat bubble text '{}' not found in any row",
        bubble_text
    );
}
