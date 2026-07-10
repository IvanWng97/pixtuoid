use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::state::{ActivityState, DaemonState, ToolKind, MAX_FLOORS};
use pixtuoid_core::SceneState;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::{
    centered_in, display_width, state_count, to_color, StateCounts, StateKind, PANEL_PAD_X,
    PANEL_PAD_Y,
};
use crate::tui::renderer::clip_widget_rect;

/// The pre-computed office/floor tallies the footer renders — assembled once per
/// frame at each of the three `paint_footer` sites (C3). `counts` is the CURRENT
/// (projected) floor's per-state breakdown (the rungs); `per_floor` + `gateway`
/// are office-wide (the cross-floor `▲F{n}` cue + the `⬢gw` chip), always present
/// so they render on a single-floor office too (C1).
pub(crate) struct FooterStats<'a> {
    pub counts: StateCounts,
    pub per_floor: &'a [StateCounts; MAX_FLOORS],
    pub gateway: Option<DaemonState>,
}

/// The two colors that characterize a theme in the picker swatch: its
/// accent (`neon_brand`) and its dominant office surface (`carpet_base`).
fn theme_swatch(t: &pixtuoid_scene::theme::Theme) -> (Color, Color) {
    (to_color(t.ui.neon_brand), to_color(t.surface.carpet_base))
}

pub(crate) fn paint_theme_picker(
    f: &mut ratatui::Frame<'_>,
    selected: usize,
    bounds: Rect,
    theme: &pixtuoid_scene::theme::Theme,
) {
    use pixtuoid_scene::theme;
    use ratatui::style::Modifier;
    use ratatui::text::{Line, Span as TSpan};

    // `centered_in` clamps to bounds.width: `borderless_panel`'s `Clear` (unlike
    // Block/Paragraph) does not intersect with the buffer area, so an over-wide
    // `area` panics on narrow terminals. The floor-transition paint path has no
    // layout gate, so this is reachable at widths the normal path rejects.
    // Borderless: 1 title row + the theme rows (no top/bottom border).
    let area = centered_in(
        bounds,
        28 + 2 * PANEL_PAD_X,
        theme::ALL_THEMES.len() as u16 + 1 + 2 * PANEL_PAD_Y,
    );
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
    let inner = super::borderless_panel(
        f,
        area,
        Some("Theme [\u{2191}\u{2193}/jk] Enter/Esc"),
        theme,
    );
    f.render_widget(Paragraph::new(items), inner);
}

/// One-line footer warning for dead sources (#157); `None` while healthy.
/// Deliberately terse — it shares the footer row — with the full error in the
/// log file (written by default since #157's logging fix; a failed log-file
/// install is announced on pre-altscreen stderr). `pub`: the snapshot
/// example's --source-warning reuses this exact formatter so screenshots
/// can't drift from production wording.
pub fn source_warning_message(
    deaths: &[pixtuoid_core::source::manager::SourceDeath],
) -> Option<String> {
    match deaths {
        [] => None,
        [d] => Some(format!(
            "{} source died — its agents are frozen; restart pixtuoid (see log)",
            d.source
        )),
        many => Some(format!(
            "{} sources died — restart pixtuoid (see log)",
            many.len()
        )),
    }
}

pub(crate) fn paint_footer(
    f: &mut ratatui::Frame<'_>,
    scene: &SceneState,
    stats: &FooterStats<'_>,
    full_rect: Rect,
    theme: &pixtuoid_scene::theme::Theme,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
    source_warning: Option<&str>,
) {
    use ratatui::text::Line;
    let spans = build_status_spans(
        scene,
        stats,
        full_rect.width,
        floor_info,
        theme,
        source_warning,
    );
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

/// Per-segment color role for the footer. The tier-selection logic emits a list
/// of `(text, role)` pieces once; the plain-string and colored-span renderers
/// both consume that list, so their text is always byte-identical and only the
/// color differs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SegRole {
    /// Labels, separators, counts, padding, quit hint — muted.
    Neutral,
    /// An activity-state rung — delegates its hue to the shared `StateKind`
    /// vocabulary (so footer/board/tooltip state colours can't drift).
    State(StateKind),
    /// A tool tally segment — hue from the TYPED `ToolKind` (C7), the same
    /// monitor-glow colour the sprite shows, NEVER a re-parse of the name.
    Tool(ToolKind),
    /// The gateway `⬢gw` chip — hue by daemon liveness.
    Gateway(DaemonState),
    /// Source-death warning (#157) — reuses the Waiting attention color
    /// rather than adding a theme key (the nearest themed "needs your eyes").
    Warning,
}

impl SegRole {
    fn color(self, theme: &pixtuoid_scene::theme::Theme) -> Color {
        match self {
            SegRole::Neutral => to_color(theme.ui.label_idle),
            SegRole::State(kind) => kind.color(theme),
            SegRole::Tool(kind) => to_color(pixtuoid_scene::pixel_painter::tool_glow_for_kind(
                kind,
                &theme.tool_glow,
            )),
            SegRole::Gateway(state) => match state {
                DaemonState::Idle => to_color(theme.ui.label_idle),
                DaemonState::Busy => to_color(theme.ui.label_active),
                DaemonState::Degraded | DaemonState::Down => to_color(theme.ui.label_waiting),
            },
            SegRole::Warning => to_color(theme.ui.label_waiting),
        }
    }
}

/// Build the footer as an ordered list of `(text, role)` segments, picking the
/// widest tier (full / medium / minimal) that fits inside `term_width` alongside
/// the fixed-right quit suffix. Single source of truth for both the plain-string
/// oracle (`build_status_summary`) and the colored footer (`build_status_spans`).
///
/// Tier breakdown (state rungs carry glyph+count+letter — see `StateKind`):
///   * **full** — `n/total`, a rung per non-zero state incl. a first-class
///     Exiting rung, the aggregate tool tally, and the gateway chip, e.g.
///     `13/20 · ●3 A · ◐2 W · ○7 I · ◌1 x · Edit×2 · ⬢gw ok`.
///   * **medium** — compact rungs (no separators/exiting/tools/chip), e.g.
///     `13/20 · ●3A ◐2W ○7I`.
///   * **minimal** — the waiting alarm LEADS (survives to the narrowest tier)
///     then the count, e.g. `▲2 · 13/20`.
///   * **fallback** — only the quit hint.
fn status_segments(
    scene: &SceneState,
    stats: &FooterStats<'_>,
    term_width: u16,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
    source_warning: Option<&str>,
) -> Vec<(String, SegRole)> {
    let counts = stats.counts;
    // A dead source outranks the stats (#157): the counts below silently go
    // stale once a transport is gone, so the warning IS the status until
    // restart. It survives every width — truncated to fit rather than tiered
    // away — but the waiting ALARM (`▲N need you`) rides along, the one
    // must-not-miss datum even in a partially-frozen office (design DEATH tier).
    if let Some(warn) = source_warning {
        let w = term_width as usize;
        let quit = " [q]uit ";
        let avail = w.saturating_sub(quit.len());
        let alarm = if counts.waiting > 0 {
            format!(" · \u{25b2}{} need you", counts.waiting)
        } else {
            String::new()
        };
        // The `⚠ {warn}` body truncates to fit, but the `▲N need you` alarm is
        // PINNED — the one must-not-miss datum rides through to the narrowest
        // width (design DEATH tier). Only when the fixed chrome + alarm ALONE
        // overflow does the alarm itself get cut. (The old code appended the
        // alarm to `text` before a blanket truncate, so at narrow widths it was
        // the TAIL cut first — the exact datum the comment promised to keep.)
        let prefix = " \u{26a0} ";
        let suffix = " ";
        let full = format!("{prefix}{warn}{alarm}{suffix}");
        let text = if display_width(&full) <= avail {
            full
        } else {
            let chrome = display_width(prefix) + display_width(suffix) + display_width(&alarm);
            let body_budget = avail.saturating_sub(chrome);
            if body_budget >= 1 {
                let mut body: String = warn.chars().take(body_budget.saturating_sub(1)).collect();
                body.push('\u{2026}');
                format!("{prefix}{body}{alarm}{suffix}")
            } else {
                // Too narrow even for the pinned alarm — truncate the whole line.
                let mut t: String = full.chars().take(avail.saturating_sub(1)).collect();
                t.push('\u{2026}');
                t
            }
        };
        let pad = w.saturating_sub(display_width(&text) + quit.len());
        let mut out = vec![(text, SegRole::Warning)];
        if pad > 0 {
            out.push((" ".repeat(pad), SegRole::Neutral));
        }
        out.push((quit.to_string(), SegRole::Neutral));
        return out;
    }

    // `n/total` — the floor's own agent count over the office total (the office
    // total is the sum of the per-floor tallies, == FloorInfo.total_agents).
    // Single-floor offices show just the floor count (no slash).
    let count_str = match floor_info {
        Some(fi) => format!("{}/{}", counts.total, fi.total_agents),
        None => format!("{}", counts.total),
    };

    // The aggregate tool tally: group Active slots by their raw display token
    // (kept verbatim) but carry the TYPED ToolKind for the hue (C7) — a Task
    // slot displays "Delegating" yet tints via `kind = Task`, never the name.
    let mut tool_counts: HashMap<&str, (ToolKind, usize)> = HashMap::new();
    for slot in scene.agents.values() {
        if let ActivityState::Active { detail, kind, .. } = &slot.state {
            if let Some(token) = detail
                .as_deref()
                .and_then(|d| d.split(|c: char| !c.is_alphanumeric()).next())
                .filter(|t| !t.is_empty())
            {
                tool_counts.entry(token).or_insert((*kind, 0)).1 += 1;
            }
        }
    }
    let mut tools: Vec<(&str, ToolKind, usize)> = tool_counts
        .iter()
        .map(|(name, (kind, count))| (*name, *kind, *count))
        .collect();
    tools.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(b.0)));
    tools.truncate(4);

    // Floor breadcrumb + the cross-floor `▲F{n}` cue: any OTHER floor holding a
    // waiting agent (the one you'd want to switch to). Rides the right-flushed
    // quit suffix so it's present at every tier that keeps the suffix.
    let cross_floor = floor_info.and_then(|fi| {
        let cur = fi.current.saturating_sub(1);
        (0..MAX_FLOORS)
            .find(|&fl| fl != cur && stats.per_floor[fl].waiting > 0)
            .map(|fl| fl + 1)
    });
    let floor_suffix = match floor_info {
        Some(fi) => {
            let cross = match cross_floor {
                Some(n) => format!(" \u{25b2}F{n}"),
                None => String::new(),
            };
            format!(
                " F{}/{}{cross} [\u{2191}\u{2193}]",
                fi.current, fi.total_floors
            )
        }
        None => String::new(),
    };
    let quit = format!("{floor_suffix} [?]help [p]ause [t]heme [q]uit ");

    // --- tier builders --------------------------------------------------------
    // An empty office reads as a bare count on every tier (the board owns the
    // friendly "— office empty —").
    if counts.total == 0 {
        return finish_tier(
            vec![(format!(" {count_str} "), SegRole::Neutral)],
            &quit,
            term_width,
        );
    }

    let seg_full = {
        let mut segs = vec![(format!(" {count_str}"), SegRole::Neutral)];
        for kind in StateKind::ALL {
            let c = state_count(counts, kind);
            if c == 0 {
                continue;
            }
            segs.push((" · ".to_string(), SegRole::Neutral));
            segs.push((
                format!("{}{} {}", kind.glyph(), c, kind.letter()),
                SegRole::State(kind),
            ));
        }
        if !tools.is_empty() {
            segs.push((" · ".to_string(), SegRole::Neutral));
            for (i, (name, kind, count)) in tools.iter().enumerate() {
                if i > 0 {
                    segs.push((" ".to_string(), SegRole::Neutral));
                }
                segs.push((format!("{name}\u{d7}{count}"), SegRole::Tool(*kind)));
            }
        }
        if let Some(g) = stats.gateway {
            segs.push((" · ".to_string(), SegRole::Neutral));
            segs.push((
                format!(
                    "{}gw {}",
                    pixtuoid_scene::board::GATEWAY_GLYPH,
                    pixtuoid_scene::board::gateway_label(g)
                ),
                SegRole::Gateway(g),
            ));
        }
        segs.push((" ".to_string(), SegRole::Neutral));
        segs
    };

    // Medium: compact rungs for the three resident states (exiting/tools/chip
    // drop out for width); space-separated `{glyph}{count}{letter}`.
    let seg_medium = {
        let mut rungs: Vec<(String, SegRole)> = Vec::new();
        for kind in [StateKind::Active, StateKind::Waiting, StateKind::Idle] {
            let c = state_count(counts, kind);
            if c == 0 {
                continue;
            }
            if !rungs.is_empty() {
                rungs.push((" ".to_string(), SegRole::Neutral));
            }
            rungs.push((
                format!("{}{}{}", kind.glyph(), c, kind.letter()),
                SegRole::State(kind),
            ));
        }
        let mut segs = vec![(format!(" {count_str} \u{b7} "), SegRole::Neutral)];
        segs.extend(rungs);
        segs.push((" ".to_string(), SegRole::Neutral));
        segs
    };

    // Minimal: the waiting alarm LEADS (the last stat to survive), then count.
    let seg_min = if counts.waiting > 0 {
        vec![
            (
                format!(" \u{25b2}{}", counts.waiting),
                SegRole::State(StateKind::Waiting),
            ),
            (format!(" \u{b7} {count_str} "), SegRole::Neutral),
        ]
    } else {
        vec![(format!(" {count_str} "), SegRole::Neutral)]
    };

    for tier in [seg_full, seg_medium, seg_min] {
        let stats_len: usize = tier.iter().map(|(s, _)| display_width(s)).sum();
        if stats_len + display_width(&quit) <= term_width as usize {
            return finish_tier(tier, &quit, term_width);
        }
    }
    vec![(quit, SegRole::Neutral)]
}

/// Right-flush a chosen stats tier: pad the gap between it and the fixed quit
/// suffix so `[q]uit` sits at the exact edge (display-column measured).
fn finish_tier(
    mut tier: Vec<(String, SegRole)>,
    quit: &str,
    term_width: u16,
) -> Vec<(String, SegRole)> {
    let stats_len: usize = tier.iter().map(|(s, _)| display_width(s)).sum();
    let pad = (term_width as usize).saturating_sub(stats_len + display_width(quit));
    if pad > 0 {
        tier.push((" ".repeat(pad), SegRole::Neutral));
    }
    tier.push((quit.to_string(), SegRole::Neutral));
    tier
}

/// Plain-string footer — renders `status_segments` to text. Test-only: it
/// is the text-contract oracle (insta snapshots + direct substring asserts)
/// that locks the exact footer wording, byte-identical to the colored
/// `build_status_spans` content. Production paints via `build_status_spans`.
#[cfg(test)]
pub(crate) fn build_status_summary(
    scene: &SceneState,
    stats: &FooterStats<'_>,
    term_width: u16,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
    source_warning: Option<&str>,
) -> String {
    status_segments(scene, stats, term_width, floor_info, source_warning)
        .into_iter()
        .map(|(s, _)| s)
        .collect()
}

/// Colored footer — same segments as `build_status_summary`, each tinted by
/// its role so state / tool / gateway pieces scan by hue.
pub(crate) fn build_status_spans<'a>(
    scene: &SceneState,
    stats: &FooterStats<'_>,
    term_width: u16,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
    theme: &pixtuoid_scene::theme::Theme,
    source_warning: Option<&str>,
) -> Vec<Span<'a>> {
    status_segments(scene, stats, term_width, floor_info, source_warning)
        .into_iter()
        .map(|(s, role)| Span::styled(s, Style::default().fg(role.color(theme))))
        .collect()
}

/// The wall board's text width + cell-origin pin to the painted neon panel's dark
/// INTERIOR (spine 2), so the lit sign's letters can never overrun the glowing
/// frame — the `PANTRY_COFFEE_COLS` anti-drift precedent. `NEON_PANEL_INNER_W` =
/// the outer panel minus its `NEON_PANEL_BORDER` on each side (laying text to the
/// full outer `NEON_PANEL_W` overran the frame — the board-overflow bug). Only the
/// horizontal derives; the 3-row height + the `+1` cell ROW stay literal (the
/// half-block 2:1 vertical is a different coordinate system — C2).
pub(super) const BOARD_W: u16 = pixtuoid_scene::pixel_painter::NEON_PANEL_INNER_W;

/// The board text's top-left terminal cell = the neon panel's dark interior origin
/// (`NEON_PANEL_INNER_X` px, 1:1 with cells; the `+1` row is the half-block 2:1
/// vertical, kept literal — C2). BOTH `paint_wall_display` and `star_hit_rect`
/// read THIS one helper, so the painted text and the click target share an origin.
fn board_cell_origin(scene_rect: Rect) -> (u16, u16) {
    (
        scene_rect.x + pixtuoid_scene::pixel_painter::NEON_PANEL_INNER_X,
        scene_rect.y + 1,
    )
}

/// Map a backend-agnostic `BoardTone` to this theme's ratatui color. Mirrors the
/// footer's role→color map so the board's tones (brand/star/state/dim/gateway)
/// resolve to the SAME hues the footer's `SegRole` uses.
fn board_tone_color(
    tone: pixtuoid_scene::board::BoardTone,
    theme: &pixtuoid_scene::theme::Theme,
) -> Color {
    // The tone→role map is the ONE authority in `scene::board`; this painter only
    // converts the resolved `Rgb` to ratatui `Color`.
    to_color(pixtuoid_scene::board::tone_rgb(tone, theme))
}

/// The in-scene neon wall board — the office's "lit sign": brand + ★ CTA (L1), the
/// mood pulse echoing the shared counts (L2), and the office context row (L3:
/// uptime + floor + gateway chip). It owns nothing critical exclusively (it may
/// clip off-screen); the must-not-miss signals live in the footer. `counts` is the
/// SAME `scene_stats` the footer reads (spine 1); `floor_info`/`gateway` are the
/// always-present office-wide `DrawCtx` fields (C1). The scrolling ticker is gone.
#[allow(clippy::too_many_arguments)] // a painter's distinct inputs (like paint_footer)
pub(crate) fn paint_wall_display(
    f: &mut ratatui::Frame<'_>,
    scene: &SceneState,
    scene_rect: Rect,
    now: SystemTime,
    counts: StateCounts,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
    gateway: Option<DaemonState>,
    theme: &pixtuoid_scene::theme::Theme,
) {
    use ratatui::style::Modifier;
    use ratatui::text::Line;

    let (cell_x, cell_y) = board_cell_origin(scene_rect);

    // The board's TEXT is the backend-agnostic `pixtuoid_scene::board` model (so
    // floating/wasm build the SAME content); this painter only maps each tone to a
    // ratatui color + owns the cell-space L1 right-flush.
    let oldest = scene
        .agents
        .values()
        .filter_map(|a| now.duration_since(a.created_at).ok())
        .max()
        .unwrap_or_default();
    let model = pixtuoid_scene::board::build_board(
        counts,
        oldest.as_secs(),
        floor_info.map(|fi| (fi.current, fi.total_floors)),
        gateway,
    );

    // L1 — brand + ★ Star CTA, the star right-flushed to the panel edge so its
    // left edge lands at `cell_x + BOARD_W - star_w` — the SAME position
    // `star_hit_rect` derives the click target from. The `.max(1)` floor keeps a
    // ≥1-col gap; the assert is STRICT (`<`) so the NATURAL gap is already ≥1,
    // making `.max(1)` a no-op and paint == hit-rect. At the exact-fit boundary
    // (`brand+star == BOARD_W`) `.max(1)` would shove the star one col past the
    // hit-rect (and clip it), so `<` forbids that boundary rather than `<=`.
    let star_w = display_width(&model.star.text);
    let gap = (BOARD_W as usize)
        .saturating_sub(display_width(&model.brand.text) + star_w)
        .max(1);
    debug_assert!(
        display_width(&model.brand.text) + star_w < BOARD_W as usize,
        "brand+star must STRICTLY fit the panel (natural gap ≥1) for the right-flush = star_hit_rect pairing"
    );
    let top_line = Line::from(vec![
        Span::styled(
            model.brand.text.clone(),
            Style::default()
                .fg(board_tone_color(model.brand.tone, theme))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".repeat(gap)),
        Span::styled(
            model.star.text.clone(),
            Style::default()
                .fg(board_tone_color(model.star.tone, theme))
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    // L2 — the mood pulse (shared counts, ▲ beacon leads); L3 — office context
    // (uptime · floor · gateway chip). Both are just tone-mapped model segments.
    let styled = |segs: &[pixtuoid_scene::board::BoardSegment]| -> Vec<Span<'static>> {
        segs.iter()
            .map(|s| {
                Span::styled(
                    s.text.clone(),
                    Style::default().fg(board_tone_color(s.tone, theme)),
                )
            })
            .collect()
    };
    let mood_line = Line::from(styled(&model.mood));
    let ctx_line = Line::from(styled(&model.context));

    if let Some(r) = clip_widget_rect(
        Rect {
            x: cell_x,
            y: cell_y,
            width: BOARD_W,
            height: 3,
        },
        scene_rect,
    ) {
        f.render_widget(Paragraph::new(vec![top_line, mood_line, ctx_line]), r);
    }
}

/// The project repository — opened when the board's ★ Star CTA is clicked.
/// `pub` (not `pub(crate)`): the BIN crate's crash reporter derives its
/// issue-report URL from this same authority — crash.rs is a `main.rs` module,
/// a separate crate the lib's `pub(crate)` can't reach (and the pixtuoid lib
/// target is not a semver surface, so the widening is free).
pub const REPO_URL: &str = "https://github.com/IvanWng97/pixtuoid";
/// URL shown on the version popup's "More details" line and opened on click:
/// `REPO_URL` + `/releases`. Kept a full literal (const &str can't `concat!`);
/// the two are pinned together by `version_popup_url_is_repo_releases`.
pub(crate) const VERSION_POPUP_URL: &str = "https://github.com/IvanWng97/pixtuoid/releases";

/// The precise screen rect of the board's `★ Star` CTA span, clipped to the
/// scene (`None` when it clips away on a very narrow terminal). Derived from the
/// SAME board geometry the L1 painter uses — `cell_x = scene.x + 2`, `cell_y =
/// scene.y + 1`, and the right-flush to `BOARD_W` — so the click target can't
/// drift from the painted star (the phantom-launch class the version-popup
/// url-rect also guards). Replaces the loose `hit_test_branding` (cols `1..31`),
/// which fired anywhere on the top-left row (C9).
pub(crate) fn star_hit_rect(scene_rect: Rect) -> Option<Rect> {
    let (cell_x, cell_y) = board_cell_origin(scene_rect);
    let star_w = display_width(pixtuoid_scene::board::BOARD_STAR) as u16;
    let star_x = cell_x + BOARD_W.saturating_sub(star_w);
    clip_widget_rect(
        Rect {
            x: star_x,
            y: cell_y,
            width: star_w,
            height: 1,
        },
        scene_rect,
    )
}
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
    // Borderless: no side-border columns, but the shared `borderless_panel`
    // insets content by PANEL_PAD_* — so the envelope must include 2× pad on each
    // axis. Content rows = title + blank + notes + blank + url + 1 slack.
    let needed_w = URL_PREFIX.len() as u16 + VERSION_POPUP_URL.len() as u16 + 2 + 2 * PANEL_PAD_X;
    let w_full = needed_w.min(bounds.width);
    let h_full = (notes_len as u16 + 5 + 2 * PANEL_PAD_Y).min(bounds.height);
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

pub(crate) fn paint_version_popup(
    f: &mut ratatui::Frame<'_>,
    version: &str,
    notes: &[&str],
    bounds: Rect,
    theme: &pixtuoid_scene::theme::Theme,
    scale: f32,
) {
    use ratatui::style::Modifier;
    use ratatui::text::{Line, Span as TSpan};

    let scale = scale.clamp(0.0, 1.0);
    if scale <= 0.01 {
        return; // fully dismissed, skip render
    }
    let area = version_popup_envelope(bounds, notes.len(), scale);

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

    let title = format!("What's new in v{version} \u{2014} Enter to close");
    let inner = super::borderless_panel(f, area, Some(&title), theme);
    f.render_widget(Paragraph::new(items), inner);
}

/// Computes the screen rect of the clickable URL inside the version popup.
/// Returns None if the popup would be too small to render. Mirrors the
/// geometry inside `paint_version_popup` (kept in sync by sharing the same
/// width calculation).
pub(crate) fn version_popup_url_rect(notes_len: usize, bounds: Rect, scale: f32) -> Option<Rect> {
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
    // URL line layout inside the borderless, PANEL_PAD_*-padded popup:
    //   y = popup_y + PAD_Y + 1 (title) + 1 (blank) + notes_len + 1 (blank)
    //   x = popup_x + PAD_X + URL_PREFIX.len()
    // The title row replaces the old top border; the pad shifts both offsets.
    let url_y = popup_y + PANEL_PAD_Y + notes_len as u16 + 3;
    let url_x = popup_x + PANEL_PAD_X + URL_PREFIX.len() as u16;

    // Clip against the popup's PADDED content area: when the painter clipped the
    // envelope (narrow / short terminal), the URL rect must shrink too — otherwise
    // clicks past the visible popup register as URL clicks.
    let inner_right = popup_x + w - PANEL_PAD_X; // content right edge (exclusive)
    let inner_bottom = popup_y + h - PANEL_PAD_Y; // content bottom edge (exclusive)
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

pub(crate) fn paint_elevator_indicator(
    f: &mut ratatui::Frame<'_>,
    door: pixtuoid_scene::layout::Point,
    current_floor: usize,
    scene_rect: Rect,
    theme: &pixtuoid_scene::theme::Theme,
) {
    use ratatui::style::Modifier;
    use ratatui::text::Line;

    let label = format!(" \u{25b2} F{current_floor} \u{25bc} ");
    // Measure in display COLUMNS, not bytes: the ▲/▼ arrows are 3-byte
    // single-column glyphs, so byte length over-counts by 4 — shifting the
    // label off the door's center and over-widening the clip rect. Matches
    // the footer's chars().count() convention.
    let label_w = label.chars().count() as u16;
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

    fn full_bounds(w: u16, h: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    #[test]
    fn version_popup_url_is_repo_releases() {
        // The two URL consts can't `concat!` (const &str), so pin them here — the
        // version popup opens the repo's releases, the ★ CTA opens the repo root.
        assert_eq!(VERSION_POPUP_URL, format!("{REPO_URL}/releases"));
    }

    // Read the `width` cells starting at `(x, y)` back as a string — the rendered
    // text of one board row.
    fn row_text(buf: &ratatui::buffer::Buffer, x: u16, y: u16, width: u16) -> String {
        (0..width).map(|dx| buf[(x + dx, y)].symbol()).collect()
    }

    #[test]
    fn wall_board_renders_the_three_model_lines_over_the_panel() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        // The mood reads `counts` (passed separately, like production's scene_stats);
        // uptime reads the scene, empty here → "<1m". A gateway + no floor exercises
        // the L3 chip and the single-floor (no breadcrumb) context.
        let counts = StateCounts {
            active: 2,
            waiting: 1,
            idle: 1,
            exiting: 0,
            total: 4,
        };
        let scene = SceneState::uniform(16);
        let scene_rect = full_bounds(120, 44);
        let mut term = Terminal::new(TestBackend::new(120, 44)).unwrap();
        term.draw(|f| {
            paint_wall_display(
                f,
                &scene,
                scene_rect,
                SystemTime::UNIX_EPOCH,
                counts,
                None,
                Some(DaemonState::Idle),
                &pixtuoid_scene::theme::NORMAL,
            );
        })
        .unwrap();
        let buf = term.backend().buffer();
        let (cx, cy) = board_cell_origin(scene_rect);
        let l1 = row_text(buf, cx, cy, BOARD_W);
        let l2 = row_text(buf, cx, cy + 1, BOARD_W);
        let l3 = row_text(buf, cx, cy + 2, BOARD_W);
        // L1: brand left, ★ Star right-flushed.
        assert!(l1.starts_with("pixtuoid v"), "brand leads L1: {l1:?}");
        assert!(
            l1.trim_end().ends_with("\u{2605} Star"),
            "star right-flushed: {l1:?}"
        );
        // L2: the mood pulse, beacon leading.
        assert!(
            l2.contains("\u{25b2}1 wait")
                && l2.contains("\u{25cf}2 work")
                && l2.contains("\u{25cb}1 idle"),
            "mood pulse: {l2:?}"
        );
        // L3: uptime + the ⬢gw chip (no floor breadcrumb on a single floor).
        assert!(l3.contains("\u{2191}<1m"), "uptime: {l3:?}");
        assert!(l3.contains("\u{2b22}gw ok"), "gateway chip: {l3:?}");
        assert!(
            !l3.contains('F'),
            "no floor breadcrumb when floor_info is None: {l3:?}"
        );
    }

    #[test]
    fn star_hit_rect_fits_and_truncates() {
        let star_w = display_width(pixtuoid_scene::board::BOARD_STAR) as u16; // "★ Star" == 6 cols
                                                                              // cell_x = the panel INTERIOR origin; the star right-flushes to the
                                                                              // interior's right edge, which must land INSIDE the outer frame.
        let inner_x = pixtuoid_scene::pixel_painter::NEON_PANEL_INNER_X;
        let star_x = inner_x + BOARD_W - star_w;
        let wide = star_hit_rect(full_bounds(120, 44)).expect("star fits");
        assert_eq!(
            (wide.x, wide.y, wide.width, wide.height),
            (star_x, 1, star_w, 1)
        );
        assert!(wide.x + wide.width <= 120, "clipped within the scene");
        // The star's right edge sits at or before the panel's inner-right edge, so
        // it never spills onto/past the glowing frame (the overflow bug).
        assert!(
            wide.x + wide.width <= inner_x + BOARD_W,
            "star must land inside the panel interior"
        );
        // A cramped scene truncates the span to its visible columns.
        let narrow = star_hit_rect(full_bounds(star_x + 2, 44)).expect("partial star");
        assert_eq!(narrow.width, 2, "clipped to the 2 visible cols");
        // Too narrow to show any of the star ⇒ no click target (no phantom launch).
        assert!(star_hit_rect(full_bounds(star_x, 44)).is_none());
    }

    #[test]
    fn url_rect_fits_inside_normal_popup() {
        let rect = version_popup_url_rect(4, full_bounds(200, 60), 1.0).expect("should fit");
        assert_eq!(rect.width, VERSION_POPUP_URL.len() as u16);
        assert_eq!(rect.height, 1);
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
            paint_theme_picker(
                f,
                0,
                Rect::new(0, 0, 24, 30),
                &pixtuoid_scene::theme::NORMAL,
            );
        })
        .unwrap();
        // Reaching here without a panic is the assertion.
    }

    #[test]
    fn theme_swatch_distinguishes_themes() {
        use pixtuoid_scene::theme;
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
            let needed_w =
                URL_PREFIX.len() as u16 + VERSION_POPUP_URL.len() as u16 + 2 + 2 * PANEL_PAD_X;
            let w = needed_w.min(bounds.width);
            let popup_x = bounds.width.saturating_sub(w) / 2;
            let popup_right = popup_x + w; // borderless: exclusive panel edge
            assert!(
                rect.x + rect.width <= popup_right,
                "url rect cols {}..{} extend past popup right edge {}",
                rect.x,
                rect.x + rect.width,
                popup_right
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
        // ≥ 0.7 gate; high enough that the padded URL row still clears the
        // scaled height (the extra PANEL_PAD_Y needs a bit more vertical room).
        let scale = 0.9;
        let needed_w =
            URL_PREFIX.len() as u16 + VERSION_POPUP_URL.len() as u16 + 2 + 2 * PANEL_PAD_X;
        let w_full = needed_w.min(bounds.width);
        let w_scaled = ((w_full as f32 * scale).round() as u16).max(2);
        let expected_popup_x = bounds.width.saturating_sub(w_scaled) / 2;
        // Borderless + padded: url_x = popup_x + PAD_X + prefix.
        let expected_url_x = expected_popup_x + PANEL_PAD_X + URL_PREFIX.len() as u16;
        let rect = version_popup_url_rect(4, bounds, scale)
            .expect("url rect should exist at scale=0.9 with notes_len=4");
        assert_eq!(
            rect.x, expected_url_x,
            "url click rect x={} must match painter's scaled-centering popup_x+pad+prefix={}",
            rect.x, expected_url_x
        );
    }

    // Regression for the off-screen URL row bug: on a too-short terminal,
    // the painter clips the popup envelope vertically, and the URL row used
    // to land on or below the clipped bottom border (where ratatui never
    // paints it). The rect must return None instead.
    #[test]
    fn url_rect_returns_none_when_url_row_falls_outside_clipped_popup() {
        // notes_len=4 → needed h=11 (borderless + 2·PAD_Y). With bounds.height=9
        // the popup clips to h=9; the padded URL row (PAD_Y + notes_len + 3 = 8)
        // lands at the exclusive content bottom (h − PAD_Y = 8) → None.
        let rect = version_popup_url_rect(4, full_bounds(200, 9), 1.0);
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
                &pixtuoid_scene::theme::NORMAL,
                0.0, // fully dismissed
            );
        })
        .unwrap();
        // Nothing painted ⇒ every cell is still the default blank space.
        let buf = term.backend().buffer();
        let any_glyph = buf.content().iter().any(|c| !c.symbol().trim().is_empty());
        assert!(!any_glyph, "dismissed popup must paint nothing");
    }

    // Regression: the elevator indicator measured its label by BYTE length.
    // " ▲ F1 ▼ " is 8 display columns but 12 bytes (the arrows are 3-byte
    // single-column glyphs), so the centering anchor `door.x + 8 - w/2`
    // landed 2 cells left of the door's center. Same byte-vs-column class
    // already fixed in the footer / tooltips (PR #210); this site was missed.
    #[test]
    fn elevator_indicator_centers_by_display_columns_not_bytes() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let theme = &pixtuoid_scene::theme::NORMAL;
        let door = pixtuoid_scene::layout::Point { x: 20, y: 10 };
        let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
        term.draw(|f| {
            paint_elevator_indicator(f, door, 1, Rect::new(0, 0, 80, 30), theme);
        })
        .unwrap();
        let buf = term.backend().buffer();
        let row = (door.y / 2 - 1) as usize; // indicator paints one cell above the door
        let bg = to_color(theme.ui.tooltip_bg);
        let cols: Vec<u16> = (0..80u16)
            .filter(|&x| buf.content()[row * 80 + x as usize].style().bg == Some(bg))
            .collect();
        assert_eq!(
            cols.len(),
            " \u{25b2} F1 \u{25bc} ".chars().count(),
            "label must paint exactly its display-column width"
        );
        assert_eq!(
            cols.first(),
            Some(&(door.x + 8 - cols.len() as u16 / 2)),
            "label must center on the 16-px door (door.x + 8)"
        );
    }

    // status_segments' tool-token guard: a detail whose first split token is
    // empty (leading non-alphanumeric) must be SKIPPED, not counted as a tool.
    #[test]
    fn status_segments_skips_empty_leading_token() {
        use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex};
        use std::path::PathBuf;
        use std::sync::Arc;
        let slot = AgentSlot {
            agent_id: AgentId::from_transcript_path("/p/lead.jsonl"),
            source: Arc::from("cc"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: "lead".into(),
            // Leading '/' ⇒ first token after split-on-non-alphanumeric is "".
            state: ActivityState::Active {
                tool_use_id: Some(Arc::from("t")),
                detail: Some(Arc::from("/usr/bin/thing")),
                kind: pixtuoid_core::state::ToolKind::Other,
            },
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
        };
        let mut scene = SceneState::uniform(16);
        scene.agents.insert(slot.agent_id, slot);
        // No '×' tool breakdown token survives — the empty leading token was
        // skipped, so the active agent contributes no tool count.
        let pf = crate::tui::widgets::per_floor_counts(&scene);
        let stats = FooterStats {
            counts: crate::tui::widgets::scene_stats(&scene),
            per_floor: &pf,
            gateway: None,
        };
        let line = build_status_summary(&scene, &stats, 200, None, None);
        assert!(
            !line.contains('\u{00d7}'),
            "empty leading token must not produce a tool count: {line}"
        );
        assert!(
            line.contains("\u{25cf}1 A"),
            "active rung still shows: {line}"
        );
    }

    // Footer tier-selection + padding must measure DISPLAY COLUMNS, not bytes:
    // the full tier carries single-column multi-byte glyphs (·, ×), so a byte
    // measure over-counts width and short-pads the row below the terminal width.
    #[test]
    fn status_segments_pads_to_full_column_width_with_multibyte_glyphs() {
        use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex};
        use std::path::PathBuf;
        use std::sync::Arc;
        let slot = AgentSlot {
            agent_id: AgentId::from_transcript_path("/p/mb.jsonl"),
            source: Arc::from("cc"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: "mb".into(),
            // A real tool token ⇒ the tail carries `· Bash×1` (·/× are 2-byte,
            // 1-column glyphs).
            state: ActivityState::Active {
                tool_use_id: Some(Arc::from("t")),
                detail: Some(Arc::from("Bash ls")),
                kind: pixtuoid_core::state::ToolKind::Bash,
            },
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
        };
        let mut scene = SceneState::uniform(16);
        scene.agents.insert(slot.agent_id, slot);
        let width: u16 = 200; // wide enough that the full tier is selected either way
        let pf = crate::tui::widgets::per_floor_counts(&scene);
        let stats = FooterStats {
            counts: crate::tui::widgets::scene_stats(&scene),
            per_floor: &pf,
            gateway: None,
        };
        let segs = status_segments(&scene, &stats, width, None, None);
        let cols: usize = segs.iter().map(|(s, _)| display_width(s)).sum();
        assert_eq!(
            cols, width as usize,
            "footer must fill the full width in display columns: {segs:?}"
        );
        assert!(
            segs.iter().any(|(s, _)| s.contains('\u{00d7}')),
            "full tier (with the tool breakdown) expected at width 200: {segs:?}"
        );
    }

    // DEATH tier: a source-death warning replaces the stats, but the `▲N need you`
    // alarm is the one must-not-miss datum — it must survive to a width so narrow
    // the warning body itself is truncated. (Regression: the alarm used to be
    // appended to the text BEFORE a blanket truncate, so it was the tail cut first.)
    #[test]
    fn death_tier_pins_the_waiting_alarm_through_the_narrowest_width() {
        use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex};
        use std::path::PathBuf;
        use std::sync::Arc;
        let slot = AgentSlot {
            agent_id: AgentId::from_transcript_path("/p/wait.jsonl"),
            source: Arc::from("cc"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: "w".into(),
            state: ActivityState::Waiting {
                reason: Arc::from("permission"),
            },
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
        };
        let mut scene = SceneState::uniform(16);
        scene.agents.insert(slot.agent_id, slot);
        let pf = crate::tui::widgets::per_floor_counts(&scene);
        let stats = FooterStats {
            counts: crate::tui::widgets::scene_stats(&scene),
            per_floor: &pf,
            gateway: None,
        };
        // A warning long enough that the body must truncate at this width.
        let warn = "transport pixtuoid-hook died: connection refused after 3 retries";
        let line = build_status_summary(&scene, &stats, 40, None, Some(warn));
        assert!(
            line.contains("\u{25b2}1 need you"),
            "the ▲N alarm must survive even when the warning body is truncated: {line}"
        );
        assert!(
            line.contains('\u{2026}'),
            "the warning body itself IS truncated at this width (proving the alarm was pinned, not merely fit): {line}"
        );
    }
}
