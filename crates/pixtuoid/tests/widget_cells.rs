//! Widget cell assertion tests: inspect the ratatui buffer after `draw_scene`.

mod common;

use std::time::{Duration, SystemTime};

use common::fixture_scene;
use pixtuoid::tui::renderer::draw_scene;
use pixtuoid_scene::pack::load_bundled_pack;
use pixtuoid_scene::theme;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

const NOW_SECS: u64 = 1_716_286_800;

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(NOW_SECS)
}

fn render_and_get_buffer(
    now: SystemTime,
    floor_info: Option<pixtuoid_scene::footer::FooterFloor>,
) -> (Buffer, u16, u16) {
    let w = 96u16;
    let h = 48u16;
    let scene = fixture_scene(now);
    let backend = TestBackend::new(w, h);
    let mut term = Terminal::new(backend).unwrap();
    let pack = load_bundled_pack().unwrap();
    make_draw_ctx!(draw_ctx, &scene, &pack, now);
    draw_ctx.footer.floor = floor_info;
    draw_scene(&mut term, &mut draw_ctx).unwrap();
    let buffer = term.backend().buffer().clone();
    (buffer, w, h)
}

fn row_text(buf: &Buffer, y: u16, w: u16) -> String {
    (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect()
}

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

#[test]
fn elevator_indicator_visible() {
    let (buf, w, h) = render_and_get_buffer(
        now(),
        Some(pixtuoid_scene::footer::FooterFloor {
            current: 2,
            total_floors: 2,
            total_agents: 0,
        }),
    );
    let mut found = false;
    for y in 0..h {
        let row = row_text(&buf, y, w);
        if row.contains("F2") {
            found = true;
            break;
        }
    }
    assert!(found, "elevator indicator with 'F2' not found in any row");
}

#[test]
fn branding_visible_in_wall_display() {
    let (buf, w, h) = render_and_get_buffer(now(), None);
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

#[test]
fn chitchat_bubble_text_appears_in_buffer() {
    use pixtuoid_scene::chitchat::ChitchatBubble;
    use pixtuoid_scene::layout::Point;
    use pixtuoid_scene::pixel_painter::AgentFrame;

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
    let speaker = AgentFrame {
        agent_id: pixtuoid_core::AgentId::from_transcript_path("/chat/0.jsonl"),
        anchor: Point { x: 26, y: 40 },
        w: 8,
        h: 12,
        label_anchor: Point { x: 30, y: 40 },
    };
    let bubbles = vec![ChitchatBubble {
        text: bubble_text,
        speaker: speaker.agent_id,
    }];

    term.draw(|f| {
        pixtuoid::tui::widgets::paint_chitchat_bubbles(
            f,
            &bubbles,
            &[speaker],
            scene_rect,
            &theme::NORMAL,
        );
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
