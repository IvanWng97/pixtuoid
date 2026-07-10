//! Backend-agnostic neon wall-board model.
//!
//! The SINGLE source of truth for the office's "lit sign" — brand + ★ CTA (L1),
//! the plain-English mood pulse echoing the shared `StateCounts` (L2), and the
//! office-context row (L3: uptime · floor · gateway chip). Three painters render
//! it: the TUI (ratatui `Paragraph`), the floating window (AA JetBrains Mono
//! blitted into its surface), and the wasm hero (exported to a DOM overlay).
//! `scene` has no terminal/window deps (invariant #1), so the model carries a
//! backend-agnostic `BoardTone`; `tone_rgb` (here) is the ONE tone→theme-role map
//! all three painters share — each only converts the resolved `Rgb` to its own
//! surface color type (ratatui `Color` / packed XRGB / `#hex`), so the hues can't
//! drift across surfaces.
//!
//! This module ALSO owns the shared per-scene activity tally (`StateCounts` +
//! `scene_stats`/`per_floor_counts`/`gateway_rollup`) — pure, backend-free, and
//! now reachable by every painter crate (the binary's footer AND `pixtuoid-web`),
//! which is why it moved here out of the binary's tui widgets.

use std::collections::BTreeMap;

use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::state::{ActivityState, DaemonPresence, DaemonState, MAX_FLOORS};
use pixtuoid_core::{AgentSlot, SceneState};

use crate::theme::Theme;

// --- Shared scene stats (footer + board agree) -------------------------------
// ONE per-scene activity tally with ONE exiting-first bucketing policy, computed
// once per frame and handed to BOTH the footer (authoritative integers) and the
// wall board (plain-English echo) so the two surfaces can never disagree — the
// historical footer(counts-all)-vs-board(counts-live) walkout drift.

/// Per-scene tally of agent activity states. `total == active + waiting + idle +
/// exiting` (debug-asserted). `exiting` is a first-class bucket, not folded into
/// idle, so the footer can render an authoritative `n/total` incl. walkouts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StateCounts {
    pub active: usize,
    pub waiting: usize,
    pub idle: usize,
    pub exiting: usize,
    pub total: usize,
}

/// Add one slot to `c` under the ONE exiting-first bucketing policy: an
/// **exiting** agent (walking out) counts as `exiting` regardless of its last
/// activity state. Shared by [`scene_stats`] and [`per_floor_counts`] so the
/// policy can't drift between the office-wide and per-floor tallies.
fn bucket_slot(c: &mut StateCounts, slot: &AgentSlot) {
    c.total += 1;
    if slot.exiting_at.is_some() {
        c.exiting += 1;
        return;
    }
    match slot.state {
        ActivityState::Active { .. } => c.active += 1,
        ActivityState::Waiting { .. } => c.waiting += 1,
        ActivityState::Idle => c.idle += 1,
    }
}

/// Bucket every agent in `scene` — the office-wide (or current-projected-floor)
/// tally the footer and board both read.
pub fn scene_stats(scene: &SceneState) -> StateCounts {
    let mut c = StateCounts::default();
    for slot in scene.agents.values() {
        bucket_slot(&mut c, slot);
    }
    debug_assert_eq!(c.active + c.waiting + c.idle + c.exiting, c.total);
    c
}

/// Per-floor [`StateCounts`], bucketed by `AgentSlot.floor_idx` (clamped to the
/// last floor). The office-wide breakdown feeding the footer's cross-floor
/// `▲F{n}` cue — computed from the FULL scene, deliberately distinct from the
/// footer's per-state integers (`scene_stats` on the projected floor); don't
/// derive one from the other.
pub fn per_floor_counts(scene: &SceneState) -> [StateCounts; MAX_FLOORS] {
    let mut floors = [StateCounts::default(); MAX_FLOORS];
    for slot in scene.agents.values() {
        bucket_slot(&mut floors[slot.floor_idx.min(MAX_FLOORS - 1)], slot);
    }
    floors
}

/// The worst-of daemon-liveness rollup for the gateway chip. `None` = no daemon
/// configured (chip suppressed), distinct from `Some(DaemonState::Down)` (a
/// daemon was seen, then died). `DaemonState` has no `Ord`, so severity is
/// ranked explicitly: Idle < Busy < Degraded < Down.
pub fn gateway_rollup(daemons: &BTreeMap<String, DaemonPresence>) -> Option<DaemonState> {
    fn severity(s: DaemonState) -> u8 {
        match s {
            DaemonState::Idle => 0,
            DaemonState::Busy => 1,
            DaemonState::Degraded => 2,
            DaemonState::Down => 3,
        }
    }
    daemons
        .values()
        .map(|p| p.display_state())
        .max_by_key(|s| severity(*s))
}

/// Format a duration in seconds as a compact `"{h}h{m}m"` / `"{m}m"` / `"<1m"`
/// string (no prefix). The board's uptime badge prepends "↑". Bucket thresholds:
/// ≥1h shows hours+minutes, ≥1m shows minutes.
pub fn compact_hms(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        "<1m".to_string()
    }
}

// --- The board model ---------------------------------------------------------

/// The board text's tone — backend-agnostic. Each painter maps it to its own
/// color (ratatui `Color` in tui, `Rgb`/hex elsewhere). Deliberately NOT
/// `overlay::LabelTone`: the variant sets are disjoint (labels never show
/// Brand/Star/Dim; the board never shows a per-agent Exiting).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardTone {
    /// L1 brand — `neon_brand`.
    Brand,
    /// L1 ★ Star CTA — `neon_star`.
    Star,
    /// A working/active count — `label_active`.
    Active,
    /// A waiting "needs-you" count — `label_waiting`.
    Waiting,
    /// An idle count — `label_idle`.
    Idle,
    /// Muted context/separator text — `tooltip_dim`.
    Dim,
}

/// Resolve a `BoardTone` to its theme color role — the SINGLE authority the three
/// board painters share (tui `board_tone_color`→`to_color`, floating→`pack_xrgb`,
/// wasm `board_hex`→`#hex`), so a `theme.ui` role change lands in ONE place and the
/// surfaces can't drift. The model carries the tone; only the output color TYPE
/// differs per surface.
pub fn tone_rgb(tone: BoardTone, theme: &Theme) -> Rgb {
    match tone {
        BoardTone::Brand => theme.ui.neon_brand,
        BoardTone::Star => theme.ui.neon_star,
        BoardTone::Active => theme.ui.label_active,
        BoardTone::Waiting => theme.ui.label_waiting,
        BoardTone::Idle => theme.ui.label_idle,
        BoardTone::Dim => theme.ui.tooltip_dim,
    }
}

/// One tone-tagged text run of the board. Painters concatenate/position these;
/// the model bakes in the inter-segment separators so no painter re-derives them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardSegment {
    pub text: String,
    pub tone: BoardTone,
}

impl BoardSegment {
    fn new(text: impl Into<String>, tone: BoardTone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }
}

/// The whole board, as tone-tagged segments — L1 `brand` + `star`, L2 `mood`,
/// L3 `context`. No baked padding between brand/star (that's each painter's own
/// right-flush in its coordinate space); the mood + context separators ARE baked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardModel {
    pub brand: BoardSegment,
    pub star: BoardSegment,
    pub mood: Vec<BoardSegment>,
    pub context: Vec<BoardSegment>,
}

/// The board's brand line — `pixtuoid v{version}`. Every workspace crate shares
/// one `version.workspace` value, so this is identical no matter which crate's
/// `env!` evaluates it (the binary's footer, floating, and wasm all agree).
pub fn board_brand() -> String {
    format!("pixtuoid v{}", env!("CARGO_PKG_VERSION"))
}

/// The board's L1 ★ CTA text — the ONE definition every painter renders AND the
/// TUI's `star_hit_rect` measures, so the clickable target can't drift.
pub const BOARD_STAR: &str = "\u{2605} Star";

/// The gateway chip's GLYPH — one definition for the footer chip AND the board
/// context row (`⬢gw ok`), so the two surfaces can't drift.
pub const GATEWAY_GLYPH: char = '\u{2b22}';

/// The `⬢gw` chip's terse liveness word.
pub fn gateway_label(state: DaemonState) -> &'static str {
    match state {
        DaemonState::Idle => "ok",
        DaemonState::Busy => "busy",
        DaemonState::Degraded => "err",
        DaemonState::Down => "down",
    }
}

/// The gateway chip's tone — mirrors the footer's `SegRole::Gateway` severity
/// map (Idle→Idle, Busy→Active, Degraded/Down→Waiting), but returns a plain
/// `BoardTone` so `DaemonState` is only ever an INPUT to the model, never leaks
/// a color out of `scene`.
pub fn gateway_tone(state: DaemonState) -> BoardTone {
    match state {
        DaemonState::Idle => BoardTone::Idle,
        DaemonState::Busy => BoardTone::Active,
        DaemonState::Degraded | DaemonState::Down => BoardTone::Waiting,
    }
}

/// The board's plain-English "mood pulse" — one tone-tagged segment per non-zero
/// present state, echoing the SHARED `StateCounts` the footer reads.
///
/// The ▲ "needs-you" beacon LEADS (waiting first) — on the board, waiting is the
/// amber attention flag. Counts are NUMERIC, never one-dot-per-agent; the words
/// abbreviate (`wt`/`wk`/`id`) when the full form would overrun the fixed panel
/// interior (`NEON_PANEL_INNER_W`). Exiting agents are absent by design — a
/// walkout isn't the office mood. Empty office → the neutral `— office empty —`.
///
/// The mood vocabulary is all single-column (the geometric glyphs `▲●○` are
/// East-Asian *ambiguous* = 1 col in a non-CJK terminal, the rest ASCII), so a
/// `chars().count()` width equals the terminal display width here — no
/// `unicode-width` dep is pulled into `scene`.
pub fn board_mood_segments(counts: StateCounts) -> Vec<BoardSegment> {
    if counts.active + counts.waiting + counts.idle == 0 {
        return vec![BoardSegment::new(
            "\u{2014} office empty \u{2014}",
            BoardTone::Dim,
        )];
    }
    // ▲ leads (waiting beacon), then ● work, ○ idle. Waiting borrows the alarm
    // triangle, not the detail surfaces' ◐ — the board is the lit "needs-you" sign.
    let build = |words: [&str; 3]| -> Vec<BoardSegment> {
        let rows = [
            (counts.waiting, '\u{25b2}', words[0], BoardTone::Waiting),
            (counts.active, '\u{25cf}', words[1], BoardTone::Active),
            (counts.idle, '\u{25cb}', words[2], BoardTone::Idle),
        ];
        let mut segs: Vec<BoardSegment> = Vec::new();
        for (n, glyph, word, tone) in rows {
            if n == 0 {
                continue;
            }
            if !segs.is_empty() {
                segs.push(BoardSegment::new("  ", BoardTone::Dim));
            }
            segs.push(BoardSegment::new(format!("{glyph}{n} {word}"), tone));
        }
        segs
    };
    let full = build(["wait", "work", "idle"]);
    let width: usize = full.iter().map(|s| s.text.chars().count()).sum();
    if width <= crate::pixel_painter::NEON_PANEL_INNER_W as usize {
        full
    } else {
        build(["wt", "wk", "id"])
    }
}

/// Assemble the whole board model. `counts` is the SAME `scene_stats` the footer
/// reads; `uptime_secs` is the oldest live-or-exiting agent's age; `floor` is
/// `(current, total_floors)` (a single-floor office passes `None`); `gateway` is
/// the `gateway_rollup` (`None` suppresses the chip). The context separators
/// (`"  "`) are baked into each following segment so painters just concatenate.
pub fn build_board(
    counts: StateCounts,
    uptime_secs: u64,
    floor: Option<(usize, usize)>,
    gateway: Option<DaemonState>,
) -> BoardModel {
    let mut context = vec![BoardSegment::new(
        format!("\u{2191}{}", compact_hms(uptime_secs)),
        BoardTone::Dim,
    )];
    if let Some((current, total)) = floor {
        context.push(BoardSegment::new(
            format!("  F{current}/{total}"),
            BoardTone::Dim,
        ));
    }
    if let Some(state) = gateway {
        context.push(BoardSegment::new(
            format!("  {GATEWAY_GLYPH}gw {}", gateway_label(state)),
            gateway_tone(state),
        ));
    }
    BoardModel {
        brand: BoardSegment::new(board_brand(), BoardTone::Brand),
        star: BoardSegment::new(BOARD_STAR, BoardTone::Star),
        mood: board_mood_segments(counts),
        context,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mood_text(counts: StateCounts) -> String {
        board_mood_segments(counts)
            .into_iter()
            .map(|s| s.text)
            .collect()
    }

    #[test]
    fn board_brand_carries_the_workspace_version() {
        // The workspace pins one version; the board reads scene's env!, which is
        // that same value the binary/wasm painters would compute.
        assert_eq!(
            board_brand(),
            format!("pixtuoid v{}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn mood_echoes_counts_with_waiting_beacon_leading() {
        let c = StateCounts {
            active: 3,
            waiting: 2,
            idle: 5,
            exiting: 1,
            total: 11,
        };
        let segs = board_mood_segments(c);
        let text = mood_text(c);
        assert!(text.contains("\u{25b2}2 wait"), "waiting beacon: {text}");
        assert!(text.contains("\u{25cf}3 work"), "active: {text}");
        assert!(text.contains("\u{25cb}5 idle"), "idle: {text}");
        assert!(!text.contains("exit"), "no exiting on the mood: {text}");
        // Beacon (waiting) leads active.
        let w = text.find('\u{25b2}').unwrap();
        let a = text.find('\u{25cf}').unwrap();
        assert!(w < a, "waiting leads active: {text}");
        // Tones are tagged per state — the whole point of the model.
        let tone_of = |glyph: char| {
            segs.iter()
                .find(|s| s.text.starts_with(glyph))
                .map(|s| s.tone)
        };
        assert_eq!(tone_of('\u{25b2}'), Some(BoardTone::Waiting));
        assert_eq!(tone_of('\u{25cf}'), Some(BoardTone::Active));
        assert_eq!(tone_of('\u{25cb}'), Some(BoardTone::Idle));
    }

    #[test]
    fn mood_calm_office_drops_the_beacon() {
        let c = StateCounts {
            active: 1,
            waiting: 0,
            idle: 2,
            exiting: 0,
            total: 3,
        };
        let text = mood_text(c);
        assert!(
            !text.contains('\u{25b2}'),
            "no beacon when nobody waits: {text}"
        );
        assert!(text.contains("\u{25cf}1 work") && text.contains("\u{25cb}2 idle"));
    }

    #[test]
    fn mood_empty_office_reads_plainly_and_is_dim() {
        let segs = board_mood_segments(StateCounts::default());
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "\u{2014} office empty \u{2014}");
        assert_eq!(segs[0].tone, BoardTone::Dim);
    }

    #[test]
    fn mood_big_office_abbreviates_to_fit_the_panel_interior() {
        // An extreme office must fit the fixed panel interior; the words shorten
        // (wait/work/idle → wt/wk/id) rather than overflow.
        let c = StateCounts {
            active: 150,
            waiting: 150,
            idle: 150,
            exiting: 0,
            total: 450,
        };
        let text = mood_text(c);
        let width = text.chars().count();
        assert!(
            width <= crate::pixel_painter::NEON_PANEL_INNER_W as usize,
            "fits the panel interior: {text} = {width}"
        );
        assert!(text.contains("\u{25b2}150 wt"), "abbreviated big-N: {text}");
    }

    #[test]
    fn gateway_tone_mirrors_the_footer_severity_map() {
        assert_eq!(gateway_tone(DaemonState::Idle), BoardTone::Idle);
        assert_eq!(gateway_tone(DaemonState::Busy), BoardTone::Active);
        assert_eq!(gateway_tone(DaemonState::Degraded), BoardTone::Waiting);
        assert_eq!(gateway_tone(DaemonState::Down), BoardTone::Waiting);
        assert_eq!(gateway_label(DaemonState::Idle), "ok");
        assert_eq!(gateway_label(DaemonState::Down), "down");
    }

    #[test]
    fn build_board_assembles_all_three_rows() {
        let c = StateCounts {
            active: 2,
            waiting: 0,
            idle: 1,
            exiting: 0,
            total: 3,
        };
        // No floor, no gateway → context is just uptime.
        let b = build_board(c, 3661, None, None);
        assert_eq!(b.brand.tone, BoardTone::Brand);
        assert!(b.brand.text.starts_with("pixtuoid v"));
        assert_eq!(b.star.text, BOARD_STAR);
        assert_eq!(b.star.tone, BoardTone::Star);
        assert_eq!(b.context.len(), 1);
        assert_eq!(b.context[0].text, "\u{2191}1h1m");
        assert_eq!(b.context[0].tone, BoardTone::Dim);

        // With a floor + a busy gateway, the separators are baked in and the chip
        // carries the gateway tone.
        let b2 = build_board(c, 30, Some((2, 3)), Some(DaemonState::Busy));
        let ctx: String = b2.context.iter().map(|s| s.text.clone()).collect();
        assert_eq!(ctx, "\u{2191}<1m  F2/3  \u{2b22}gw busy");
        let chip = b2.context.last().unwrap();
        assert_eq!(chip.tone, BoardTone::Active, "busy gateway chip tone");
    }
}
