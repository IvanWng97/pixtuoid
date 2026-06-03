use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::SceneState;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::{centered_in, compact_hms, to_color, TickerQueue};
use crate::tui::renderer::clip_widget_rect;

/// The two colors that characterize a theme in the picker swatch: its
/// accent (`neon_brand`) and its dominant office surface (`carpet_base`).
fn theme_swatch(t: &crate::tui::theme::Theme) -> (Color, Color) {
    (to_color(t.ui.neon_brand), to_color(t.surface.carpet_base))
}

/// Border glow color for the version popup: a ~3s sine pulse that lerps
/// from 60% to 100% of `brand` toward `bg`, so the frame breathes without
/// ever dropping so dim it reads as "off". Deterministic in `now`.
fn pulse_border_color(bg: Rgb, brand: Rgb, now: SystemTime) -> Color {
    let ms = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let phase = (ms % 3000) as f32 / 3000.0 * std::f32::consts::TAU;
    let t = (phase.sin() * 0.5 + 0.5) * 0.4 + 0.6;
    let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color::Rgb(
        lerp(bg.r, brand.r),
        lerp(bg.g, brand.g),
        lerp(bg.b, brand.b),
    )
}

pub(in crate::tui) fn paint_theme_picker(
    f: &mut ratatui::Frame<'_>,
    selected: usize,
    bounds: Rect,
    theme: &crate::tui::theme::Theme,
) {
    use crate::tui::theme;
    use ratatui::style::Modifier;
    use ratatui::text::{Line, Span as TSpan};
    use ratatui::widgets::{Block, Borders, Clear};

    // `centered_in` clamps to bounds.width: `Clear::render` (unlike
    // Block/Paragraph) does not intersect with the buffer area, so an
    // over-wide `area` panics on narrow terminals. The floor-transition paint
    // path has no layout gate, so this is reachable at widths the normal path
    // rejects.
    let area = centered_in(bounds, 28, theme::ALL_THEMES.len() as u16 + 2);
    f.render_widget(Clear, area);
    let items: Vec<Line> = theme::ALL_THEMES
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let prefix = if i == selected { "\u{25b8} " } else { "  " };
            let name_style = if i == selected {
                Style::default()
                    .fg(to_color(theme.ui.neon_brand))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(to_color(theme.ui.label_idle))
            };
            // Each row previews the theme it would switch to via a 2-cell
            // swatch (accent + office floor), so the picker reads visually
            // rather than by name alone.
            let (brand, surface) = theme_swatch(t);
            Line::from(vec![
                TSpan::styled(format!("{prefix}{:<12}", t.name), name_style),
                TSpan::raw(" "),
                TSpan::styled("\u{2588}", Style::default().fg(brand)),
                TSpan::styled("\u{2588}", Style::default().fg(surface)),
            ])
        })
        .collect();
    let block = Block::default()
        .title(" Theme [\u{2191}\u{2193}/jk] Enter/Esc ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(to_color(theme.ui.neon_brand)))
        .style(Style::default().bg(to_color(theme.ui.tooltip_bg)));
    f.render_widget(Paragraph::new(items).block(block), area);
}

pub(in crate::tui) fn paint_footer(
    f: &mut ratatui::Frame<'_>,
    scene: &SceneState,
    full_rect: Rect,
    theme: &crate::tui::theme::Theme,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
) {
    use ratatui::text::Line;
    let spans = build_status_spans(scene, full_rect.width, floor_info, theme);
    // Base style on the whole row (label_idle) for parity with the old
    // single-Span footer: cells past the rendered spans (quit-only tier on a
    // wide-ish terminal) keep the muted footer tone rather than default.
    let footer =
        Paragraph::new(Line::from(spans)).style(Style::default().fg(to_color(theme.ui.label_idle)));
    f.render_widget(
        footer,
        Rect {
            x: full_rect.x,
            y: full_rect.y + full_rect.height.saturating_sub(1),
            width: full_rect.width,
            height: 1,
        },
    );
}

/// Per-segment color role for the footer. The counting / tier-selection
/// logic emits a list of `(text, role)` pieces once; the plain-string and
/// colored-span renderers both consume that list, so their text is always
/// byte-identical and only the color differs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SegRole {
    /// Labels, separators, counts, tools, padding, quit hint — muted.
    Neutral,
    Active,
    Waiting,
    Idle,
}

impl SegRole {
    fn color(self, theme: &crate::tui::theme::Theme) -> Color {
        match self {
            SegRole::Neutral | SegRole::Idle => to_color(theme.ui.label_idle),
            SegRole::Active => to_color(theme.ui.label_active),
            SegRole::Waiting => to_color(theme.ui.label_waiting),
        }
    }
}

/// Build the footer as an ordered list of `(text, role)` segments, picking
/// the widest tier (full / medium / minimal) that fits inside `term_width`
/// alongside the fixed-right quit suffix. Single source of truth for both
/// the plain-string footer (`build_status_summary`) and the colored footer
/// (`build_status_spans`).
///
/// Tier breakdown:
///   * **full** (~50+ cells) — total count, per-state counts, top tool
///     names with usage tallies, e.g. `12 agents · 3 active · 2 waiting
///     · 7 idle · Edit×2 Bash×1`.
///   * **medium** (~30+ cells) — compact letters, e.g. `12a · 3A · 2W · 7I`.
///   * **minimal** — just the total, e.g. `12a`.
///   * **fallback** — only the quit hint (any narrower terminal will
///     truncate this naturally).
fn status_segments(
    scene: &SceneState,
    term_width: u16,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
) -> Vec<(String, SegRole)> {
    let n = scene.agents.len();
    // Multi-floor view always shows `n/total` so the total stays visible
    // even when an agent migrates and per-floor matches total transiently.
    let count_str = match floor_info {
        Some(fi) => format!("{n}/{}", fi.total_agents),
        None => format!("{n}"),
    };
    let mut active = 0usize;
    let mut waiting = 0usize;
    let mut idle = 0usize;
    let mut tool_counts: HashMap<&str, usize> = HashMap::new();
    for slot in scene.agents.values() {
        match &slot.state {
            ActivityState::Idle => idle += 1,
            ActivityState::Waiting { .. } => waiting += 1,
            ActivityState::Active { detail, .. } => {
                active += 1;
                if let Some(d) = detail.as_deref() {
                    let token = d.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("");
                    if !token.is_empty() {
                        *tool_counts.entry(token).or_insert(0) += 1;
                    }
                }
            }
        }
    }

    let floor_suffix = match floor_info {
        Some(fi) => format!(" F{}/{} [\u{2191}\u{2193}]", fi.current, fi.total_floors),
        None => String::new(),
    };
    let quit_base = " [?]help [p]ause [t]heme [q]uit ";
    let quit = format!("{floor_suffix}{quit_base}");
    let tools_str = {
        let mut tools: Vec<(&&str, &usize)> = tool_counts.iter().collect();
        tools.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        tools
            .iter()
            .take(4)
            .map(|(name, count)| format!("{name}×{count}"))
            .collect::<Vec<_>>()
            .join(" ")
    };

    // Each tier is a list of (text, role) segments whose concatenation is
    // exactly the old plain-string output for that tier.
    let seg_full: Vec<(String, SegRole)> = if n == 0 {
        vec![(format!(" {count_str} agents "), SegRole::Neutral)]
    } else {
        let tail = if tools_str.is_empty() {
            " ".to_string()
        } else {
            format!(" · {tools_str} ")
        };
        vec![
            (format!(" {count_str} agents · "), SegRole::Neutral),
            (format!("{active} active"), SegRole::Active),
            (" · ".to_string(), SegRole::Neutral),
            (format!("{waiting} waiting"), SegRole::Waiting),
            (" · ".to_string(), SegRole::Neutral),
            (format!("{idle} idle"), SegRole::Idle),
            (tail, SegRole::Neutral),
        ]
    };
    // Narrow tiers use bare `n` — "5/12a" parses as "5 slash 12a" at a glance.
    let seg_medium: Vec<(String, SegRole)> = vec![
        (format!(" {n}a · "), SegRole::Neutral),
        (format!("{active}A"), SegRole::Active),
        (" · ".to_string(), SegRole::Neutral),
        (format!("{waiting}W"), SegRole::Waiting),
        (" · ".to_string(), SegRole::Neutral),
        (format!("{idle}I"), SegRole::Idle),
        (" ".to_string(), SegRole::Neutral),
    ];
    let seg_min: Vec<(String, SegRole)> = vec![(format!(" {n}a "), SegRole::Neutral)];

    let w = term_width as usize;
    let q = quit.len();
    for tier in [seg_full, seg_medium, seg_min] {
        let stats_len: usize = tier.iter().map(|(s, _)| s.len()).sum();
        if stats_len + q <= w {
            let pad = w.saturating_sub(stats_len + q);
            let mut out = tier;
            if pad > 0 {
                out.push((" ".repeat(pad), SegRole::Neutral));
            }
            out.push((quit, SegRole::Neutral));
            return out;
        }
    }
    vec![(quit, SegRole::Neutral)]
}

/// Plain-string footer — renders `status_segments` to text. Test-only: it
/// is the text-contract oracle (insta snapshots + direct substring asserts)
/// that locks the exact footer wording, byte-identical to the colored
/// `build_status_spans` content. Production paints via `build_status_spans`.
#[cfg(test)]
pub(in crate::tui) fn build_status_summary(
    scene: &SceneState,
    term_width: u16,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
) -> String {
    status_segments(scene, term_width, floor_info)
        .into_iter()
        .map(|(s, _)| s)
        .collect()
}

/// Colored footer — same segments as `build_status_summary`, each tinted by
/// its state role so active/waiting/idle counts scan by hue.
pub(in crate::tui) fn build_status_spans<'a>(
    scene: &SceneState,
    term_width: u16,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
    theme: &crate::tui::theme::Theme,
) -> Vec<Span<'a>> {
    status_segments(scene, term_width, floor_info)
        .into_iter()
        .map(|(s, role)| Span::styled(s, Style::default().fg(role.color(theme))))
        .collect()
}

pub(in crate::tui) fn paint_wall_display(
    f: &mut ratatui::Frame<'_>,
    scene: &SceneState,
    scene_rect: Rect,
    now: SystemTime,
    ticker: &TickerQueue,
    theme: &crate::tui::theme::Theme,
) {
    use ratatui::style::Modifier;
    use ratatui::text::Line;

    let cell_x = scene_rect.x + 2;
    let cell_y = scene_rect.y + 1;

    let live: Vec<&pixtuoid_core::AgentSlot> = scene
        .agents
        .values()
        .filter(|a| a.exiting_at.is_none())
        .collect();
    let active = live
        .iter()
        .filter(|a| matches!(a.state, ActivityState::Active { .. }))
        .count();
    let waiting = live
        .iter()
        .filter(|a| matches!(a.state, ActivityState::Waiting { .. }))
        .count();
    let idle = live.len() - active - waiting;

    let version = env!("CARGO_PKG_VERSION");
    let top_spans = vec![
        Span::styled(
            format!("pixtuoid v{version}"),
            Style::default()
                .fg(to_color(theme.ui.neon_brand))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            "\u{2605} Star",
            Style::default()
                .fg(to_color(theme.ui.neon_star))
                .add_modifier(Modifier::BOLD),
        ),
    ];
    let top_line = Line::from(top_spans);

    let oldest = live
        .iter()
        .filter_map(|a| now.duration_since(a.created_at).ok())
        .max()
        .unwrap_or_default();
    let uptime_secs = oldest.as_secs();
    let uptime_str = format!("\u{2191}{}", compact_hms(uptime_secs));

    let bot_line = Line::from(vec![
        Span::styled(
            "\u{25cf}".repeat(active),
            Style::default().fg(to_color(theme.ui.label_active)),
        ),
        Span::styled(
            "\u{25cf}".repeat(waiting),
            Style::default().fg(to_color(theme.ui.label_waiting)),
        ),
        Span::styled(
            "\u{25cf}".repeat(idle),
            Style::default().fg(to_color(theme.ui.label_idle)),
        ),
        Span::raw("  "),
        Span::styled(uptime_str, Style::default().fg(Color::DarkGray)),
    ]);

    let ticker_width = 28usize;
    let visible = ticker.visible(ticker_width, now);
    let ticker_line = Line::from(Span::styled(
        visible,
        Style::default().fg(to_color(theme.ui.neon_ticker)),
    ));

    let w = 30u16;
    if let Some(r) = clip_widget_rect(
        Rect {
            x: cell_x,
            y: cell_y,
            width: w,
            height: 3,
        },
        scene_rect,
    ) {
        f.render_widget(Paragraph::new(vec![top_line, bot_line, ticker_line]), r);
    }
}

/// URL shown on the "More details" line and opened on click.
pub(in crate::tui) const VERSION_POPUP_URL: &str = "https://github.com/IvanWng97/pixtuoid/releases";
/// Prefix rendered before the URL. Its byte-length determines the URL's
/// click-rect x-offset; keep `paint_version_popup` and
/// `version_popup_url_rect` consistent by using this constant.
const URL_PREFIX: &str = "  More details: ";

/// The scaled, bounds-clamped, centered envelope Rect of the version popup.
/// Single source of truth for `paint_version_popup` (which paints into it) and
/// `version_popup_url_rect` (which derives the URL click-rect off it): clamp
/// w_full/h_full to `bounds` BEFORE scaling, then floor the scaled dims at 2.
/// `scale` must already be clamped to `0.0..=1.0` by the caller.
fn version_popup_envelope(bounds: Rect, notes_len: usize, scale: f32) -> Rect {
    let needed_w = 2 + URL_PREFIX.len() as u16 + VERSION_POPUP_URL.len() as u16 + 2;
    let w_full = needed_w.min(bounds.width);
    let h_full = (notes_len as u16 + 6).min(bounds.height);
    let w = ((w_full as f32 * scale).round() as u16).max(2);
    let h = ((h_full as f32 * scale).round() as u16).max(2);
    let x = bounds.x + bounds.width.saturating_sub(w) / 2;
    let y = bounds.y + bounds.height.saturating_sub(h) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

pub(in crate::tui) fn paint_version_popup(
    f: &mut ratatui::Frame<'_>,
    version: &str,
    notes: &[&str],
    bounds: Rect,
    theme: &crate::tui::theme::Theme,
    scale: f32,
    now: SystemTime,
) {
    use ratatui::style::Modifier;
    use ratatui::text::{Line, Span as TSpan};
    use ratatui::widgets::{Block, Borders, Clear};

    let scale = scale.clamp(0.0, 1.0);
    if scale <= 0.01 {
        return; // fully dismissed, skip render
    }
    let area = version_popup_envelope(bounds, notes.len(), scale);
    f.render_widget(Clear, area);

    let mut items: Vec<Line> = Vec::with_capacity(notes.len() + 3);
    items.push(Line::from(""));
    for note in notes {
        items.push(Line::from(TSpan::styled(
            format!("  \u{00b7} {note}"),
            Style::default().fg(to_color(theme.ui.label_idle)),
        )));
    }
    items.push(Line::from(""));
    items.push(Line::from(vec![
        TSpan::styled(
            URL_PREFIX,
            Style::default().fg(to_color(theme.ui.label_idle)),
        ),
        TSpan::styled(
            VERSION_POPUP_URL,
            Style::default()
                .fg(to_color(theme.ui.neon_brand))
                .add_modifier(Modifier::UNDERLINED),
        ),
    ]));

    let title = format!(" What's new in v{version} \u{2014} Enter to close ");
    // Gentle ~3s glow pulse on the border: lerp between 60% and 100% of the
    // neon_brand toward the popup background, so the frame breathes like a
    // marketing-shot neon sign without distracting from the notes.
    let border = pulse_border_color(theme.ui.tooltip_bg, theme.ui.neon_brand, now);
    let block = Block::default()
        .title(TSpan::styled(
            title,
            Style::default()
                .fg(to_color(theme.ui.neon_brand))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(to_color(theme.ui.tooltip_bg)));

    f.render_widget(Paragraph::new(items).block(block), area);
}

/// Computes the screen rect of the clickable URL inside the version popup.
/// Returns None if the popup would be too small to render. Mirrors the
/// geometry inside `paint_version_popup` (kept in sync by sharing the same
/// width calculation).
pub(in crate::tui) fn version_popup_url_rect(
    notes_len: usize,
    bounds: Rect,
    scale: f32,
) -> Option<Rect> {
    let scale = scale.clamp(0.0, 1.0);
    if scale < 0.7 {
        return None; // URL not clickable until popup reaches 70% scale
    }
    // Mirror paint_version_popup's geometry exactly by deriving from the same
    // shared envelope (clamp-to-bounds-then-scale, centered off the SCALED
    // w/h). Centering off the unscaled w/h leaves the click rect offset from
    // the painted popup at any scale < 1.0.
    let Rect {
        x: popup_x,
        y: popup_y,
        width: w,
        height: h,
    } = version_popup_envelope(bounds, notes_len, scale);
    if w < 4 || h < 3 {
        return None;
    }
    // URL line layout inside popup (Block with Borders::ALL has 1-cell border):
    //   y = popup_y + 1 (border) + 1 (blank) + notes_len (notes) + 1 (blank)
    //   x = popup_x + 1 (border) + URL_PREFIX.len()
    let url_y = popup_y + notes_len as u16 + 3;
    let url_x = popup_x + 1 + URL_PREFIX.len() as u16;

    // Clip against the popup's inner content area: when the painter clipped
    // the envelope (narrow / short terminal), the URL rect must shrink too —
    // otherwise clicks past the visible popup register as URL clicks.
    let inner_right = popup_x + w - 1; // bottom-right border column (exclusive)
    let inner_bottom = popup_y + h - 1; // bottom border row (exclusive)
    if url_x >= inner_right || url_y >= inner_bottom {
        return None;
    }
    let width = (VERSION_POPUP_URL.len() as u16).min(inner_right - url_x);
    if width == 0 {
        return None;
    }
    Some(Rect {
        x: url_x,
        y: url_y,
        width,
        height: 1,
    })
}

pub(in crate::tui) fn paint_elevator_indicator(
    f: &mut ratatui::Frame<'_>,
    door: crate::tui::layout::Point,
    current_floor: usize,
    scene_rect: Rect,
    theme: &crate::tui::theme::Theme,
) {
    use ratatui::style::Modifier;
    use ratatui::text::Line;

    let label = format!(" \u{25b2} F{current_floor} \u{25bc} ");
    let label_w = label.len() as u16;
    let door_cell_x = door.x + 8u16.saturating_sub(label_w / 2);
    let door_cell_y = door.y / 2;
    let indicator_y = door_cell_y.saturating_sub(1);

    if let Some(r) = crate::tui::renderer::clip_widget_rect(
        Rect {
            x: scene_rect.x + door_cell_x,
            y: scene_rect.y + indicator_y,
            width: label_w,
            height: 1,
        },
        scene_rect,
    ) {
        let style = Style::default()
            .fg(to_color(theme.ui.neon_brand))
            .bg(to_color(theme.ui.tooltip_bg))
            .add_modifier(Modifier::BOLD);
        f.render_widget(Paragraph::new(Line::from(Span::styled(label, style))), r);
    }
}

#[cfg(test)]
mod hud_tests {
    use super::*;
    use std::time::Duration;

    fn full_bounds(w: u16, h: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    #[test]
    fn url_rect_fits_inside_normal_popup() {
        let rect = version_popup_url_rect(4, full_bounds(200, 60), 1.0).expect("should fit");
        assert_eq!(rect.width, VERSION_POPUP_URL.len() as u16);
        assert_eq!(rect.height, 1);
    }

    #[test]
    fn pulse_border_color_breathes_within_bounds() {
        use crate::tui::theme;
        let bg = theme::NORMAL.ui.tooltip_bg;
        let brand = theme::NORMAL.ui.neon_brand;
        let at = |ms: u64| {
            pulse_border_color(bg, brand, std::time::UNIX_EPOCH + Duration::from_millis(ms))
        };
        // Peak at 750ms (phase = π/2) → full brand.
        assert_eq!(at(750), Color::Rgb(brand.r, brand.g, brand.b));
        // Deterministic + 3s-periodic.
        assert_eq!(at(1234), at(1234 + 3000));
        // Trough at 2250ms (phase = 3π/2) → dimmer than peak but never fully
        // dropped to the background.
        let trough = at(2250);
        assert_ne!(trough, at(750), "trough should be dimmer than peak");
        assert_ne!(
            trough,
            Color::Rgb(bg.r, bg.g, bg.b),
            "border never drops fully to background"
        );
    }

    // Regression: paint_theme_picker rendered Clear onto an unclamped
    // 28-wide area; on a narrower buffer (reachable via the gate-less
    // floor-transition paint path) Clear panics indexing past the buffer.
    #[test]
    fn theme_picker_narrow_terminal_does_not_panic() {
        use ratatui::backend::TestBackend;
        use ratatui::layout::Rect;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(24, 30)).unwrap();
        term.draw(|f| {
            paint_theme_picker(f, 0, Rect::new(0, 0, 24, 30), &crate::tui::theme::NORMAL);
        })
        .unwrap();
        // Reaching here without a panic is the assertion.
    }

    #[test]
    fn theme_swatch_distinguishes_themes() {
        use crate::tui::theme;
        // Each theme's (accent, surface) pair should reflect that theme's
        // own palette, not the currently-active one — so the picker rows
        // preview distinct colors.
        let cyber = theme_swatch(&theme::CYBERPUNK);
        let normal = theme_swatch(&theme::NORMAL);
        assert_ne!(
            cyber, normal,
            "distinct themes must yield distinct swatches"
        );
        assert_eq!(cyber.0, to_color(theme::CYBERPUNK.ui.neon_brand));
        assert_eq!(cyber.1, to_color(theme::CYBERPUNK.surface.carpet_base));
    }

    // Regression for the phantom-browser-launch bug: on a narrow terminal
    // the painter clips the popup envelope, but the URL click rect used to
    // extend past the visible popup's right edge, registering clicks on the
    // scene behind as URL clicks. The rect must stay inside the envelope.
    #[test]
    fn url_rect_does_not_extend_past_clipped_popup_right_edge() {
        let bounds = full_bounds(50, 30);
        if let Some(rect) = version_popup_url_rect(4, bounds, 1.0) {
            let needed_w = 2 + URL_PREFIX.len() as u16 + VERSION_POPUP_URL.len() as u16 + 2;
            let w = needed_w.min(bounds.width);
            let popup_x = bounds.width.saturating_sub(w) / 2;
            let popup_inner_right = popup_x + w - 1;
            assert!(
                rect.x + rect.width <= popup_inner_right,
                "url rect cols {}..{} extend past popup inner-right {}",
                rect.x,
                rect.x + rect.width,
                popup_inner_right
            );
        }
    }

    // Regression: at scale < 1.0 the URL click rect must center off the
    // SCALED width, mirroring paint_version_popup. Centering off unscaled
    // w shifts the click area ~((1-scale)*needed_w)/2 columns left of the
    // painted URL.
    #[test]
    fn url_rect_centering_matches_painter_at_partial_scale() {
        let bounds = full_bounds(200, 60);
        let scale = 0.85; // ≥ 0.7 gate and ≥ 0.8 vertical threshold for notes_len=4
        let needed_w = 2 + URL_PREFIX.len() as u16 + VERSION_POPUP_URL.len() as u16 + 2;
        let w_full = needed_w.min(bounds.width);
        let w_scaled = ((w_full as f32 * scale).round() as u16).max(2);
        let expected_popup_x = bounds.width.saturating_sub(w_scaled) / 2;
        let expected_url_x = expected_popup_x + 1 + URL_PREFIX.len() as u16;
        let rect = version_popup_url_rect(4, bounds, scale)
            .expect("url rect should exist at scale=0.85 with notes_len=4");
        assert_eq!(
            rect.x, expected_url_x,
            "url click rect x={} must match painter's scaled-centering popup_x+1+prefix={}",
            rect.x, expected_url_x
        );
    }

    // Regression for the off-screen URL row bug: on a too-short terminal,
    // the painter clips the popup envelope vertically, and the URL row used
    // to land on or below the clipped bottom border (where ratatui never
    // paints it). The rect must return None instead.
    #[test]
    fn url_rect_returns_none_when_url_row_falls_outside_clipped_popup() {
        // notes_len=4 → needed h=10. With bounds.height=8 the popup clips
        // to h=8, leaving room for at most ~3 notes — the URL row at offset
        // (notes_len + 3) = 7 lands on the bottom border.
        let rect = version_popup_url_rect(4, full_bounds(200, 8), 1.0);
        assert!(
            rect.is_none(),
            "expected None when URL row falls on the clipped popup's bottom border: got {rect:?}"
        );
    }

    // The URL is not clickable until the popup reaches 70% entrance scale.
    #[test]
    fn url_rect_none_below_seventy_percent_scale() {
        assert!(version_popup_url_rect(4, full_bounds(200, 60), 0.5).is_none());
        assert!(version_popup_url_rect(4, full_bounds(200, 60), 0.0).is_none());
    }

    // A popup envelope clamped to a tiny bounds (w<4 || h<3) yields no rect.
    #[test]
    fn url_rect_none_when_envelope_too_small() {
        // 3-col bounds → envelope width clamps to 3 (<4) → None.
        assert!(version_popup_url_rect(4, full_bounds(3, 60), 1.0).is_none());
    }

    // paint_version_popup's fully-dismissed early return (scale ≤ 0.01): a
    // near-zero scale paints nothing, so the buffer stays blank.
    #[test]
    fn version_popup_skips_render_when_fully_dismissed() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
        term.draw(|f| {
            paint_version_popup(
                f,
                "1.2.3",
                &["note a", "note b"],
                Rect::new(0, 0, 80, 30),
                &crate::tui::theme::NORMAL,
                0.0, // fully dismissed
                std::time::UNIX_EPOCH,
            );
        })
        .unwrap();
        // Nothing painted ⇒ every cell is still the default blank space.
        let buf = term.backend().buffer();
        let any_glyph = buf.content().iter().any(|c| !c.symbol().trim().is_empty());
        assert!(!any_glyph, "dismissed popup must paint nothing");
    }

    // status_segments' tool-token guard: a detail whose first split token is
    // empty (leading non-alphanumeric) must be SKIPPED, not counted as a tool.
    #[test]
    fn status_segments_skips_empty_leading_token() {
        use pixtuoid_core::source::Activity;
        use pixtuoid_core::{AgentId, AgentSlot};
        use std::path::PathBuf;
        use std::sync::Arc;
        let slot = AgentSlot {
            agent_id: AgentId::from_transcript_path("/p/lead.jsonl"),
            source: Arc::from("cc"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: Arc::from("lead"),
            // Leading '/' ⇒ first token after split-on-non-alphanumeric is "".
            state: ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: Some(Arc::from("t")),
                detail: Some(Arc::from("/usr/bin/thing")),
            },
            state_started_at: SystemTime::UNIX_EPOCH,
            created_at: SystemTime::UNIX_EPOCH,
            last_event_at: SystemTime::UNIX_EPOCH,
            exiting_at: None,
            pending_idle_at: None,
            desk_index: 0,
            floor_idx: 0,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
        };
        let mut scene = SceneState::uniform(16);
        scene.agents.insert(slot.agent_id, slot);
        // No '×' tool breakdown token survives — the empty leading token was
        // skipped, so the active agent contributes no tool count.
        let line = build_status_summary(&scene, 200, None);
        assert!(
            !line.contains('\u{00d7}'),
            "empty leading token must not produce a tool count: {line}"
        );
        assert!(
            line.contains("1 active"),
            "active count still shows: {line}"
        );
    }
}
