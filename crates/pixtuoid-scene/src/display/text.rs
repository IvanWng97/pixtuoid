//! Text as the display list lays it out: a grapheme cluster takes the cells
//! ratatui's buffer writes for it, each `ADVANCE` art pixels wide on a line
//! `LINE_H` tall. At the pack's 4x art a cell is one logical column, the
//! classic badge's terminal column. The rasterizer's font draws into these
//! cells.

use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::{AgentId, AgentSlot};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::display::pen::ArtPx;
use crate::layout::Point;
use crate::theme::Theme;

/// A hand-drawn glyph's width in art pixels.
pub(crate) const GLYPH_W: u16 = 3;
/// A cell's width in art pixels: one glyph and its gap.
pub(crate) const ADVANCE: u16 = GLYPH_W + 1;
/// The rows above the capitals, which only accents and CJK ink.
pub(crate) const ACCENT_ROWS: u16 = 2;
/// A capital's height in art pixels.
pub(crate) const CAP_H: u16 = 5;
/// A line's height in art pixels: the accent rows, the capitals, and a row for
/// descenders, Fusion Pixel 8px's own box. Pinned to the fallback font's
/// header.
pub(crate) const LINE_H: u16 = ACCENT_ROWS + CAP_H + 1;

/// `text`'s width in art pixels, from its first ink column to its last.
pub(crate) fn width(text: &str) -> ArtPx {
    ArtPx(advance(text).0.saturating_sub(1))
}

/// Art pixels from a run's left edge to where a run `n` cells on starts.
pub(crate) fn columns(n: u16) -> ArtPx {
    ArtPx(n.saturating_mul(ADVANCE))
}

/// The cells `cluster` takes on a terminal: ratatui-core 0.1.2 writes none
/// for one holding a control (`Buffer::set_stringn`) and else its
/// `unicode-width` plus one per halfwidth (semi-)voiced sound mark
/// (`CellWidth for str`, `buffer/cell_width.rs:42-43`).
fn cluster_cells(cluster: &str) -> u16 {
    if cluster.contains(char::is_control) {
        return 0;
    }
    let marks = cluster
        .chars()
        .filter(|c| HALFWIDTH_SOUND_MARKS.contains(c))
        .count();
    u16::try_from(cluster.width() + marks).unwrap_or(u16::MAX)
}

/// U+FF9E and U+FF9F, which `unicode-width` gives no width but a terminal
/// draws in a cell of their own (ratatui-core's `count_halfwidth_sound_marks`).
const HALFWIDTH_SOUND_MARKS: [char; 2] = ['\u{ff9e}', '\u{ff9f}'];

/// Each grapheme cluster of `text` that takes a cell, and the cells it takes.
pub(crate) fn clusters(text: &str) -> impl Iterator<Item = (&str, u16)> {
    text.graphemes(true)
        .map(|cluster| (cluster, cluster_cells(cluster)))
        .filter(|&(_, n)| n > 0)
}

/// The cells `text` takes, the one width every run of it is laid out by.
pub fn cells(text: &str) -> u16 {
    clusters(text).fold(0, |sum, (_, n)| sum.saturating_add(n))
}

/// The longest start of `text` that fits `budget` cells, its clusters whole.
pub(crate) fn take(text: &str, budget: u16) -> &str {
    let mut used = 0u16;
    let end = text
        .grapheme_indices(true)
        .find(|&(_, cluster)| {
            used = used.saturating_add(cluster_cells(cluster));
            used > budget
        })
        .map_or(text.len(), |(i, _)| i);
    &text[..end]
}

/// [`columns`] past all of `text`: where the run after it starts.
pub(crate) fn advance(text: &str) -> ArtPx {
    columns(cells(text))
}

/// Logical rows between a badge's anchor and the line it sits on.
pub const LABEL_GAP: u16 = 2;

/// Logical rows a chitchat bubble's anchor rides over its speaker's badge's: a
/// cell row clear of the badge.
const BUBBLE_LIFT: u16 = 2 * crate::layout::CELL_ROWS;

/// One line of text a frame shows: its spans laid end to end, on `plate` if it
/// has one. Its inks are resolved, so a painter reads no theme for it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextRun {
    /// The logical point it is placed by, as `align` says.
    pub at: Point,
    /// Where `at` lies on its line.
    pub align: Align,
    /// Its spans, in reading order.
    pub spans: Vec<TextSpan>,
    /// The fill behind it, if any.
    pub plate: Option<Rgb>,
    /// What it labels.
    pub role: TextRole,
}

/// A stretch of a [`TextRun`] in one ink.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextSpan {
    /// What it reads.
    pub text: String,
    /// Its colour.
    pub ink: Rgb,
}

/// Where a [`TextRun`]'s `at` lies on its line, a cell row tall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Align {
    /// Centred over it, [`LABEL_GAP`] rows above: a badge over a head.
    Over,
    /// At its top-left.
    Left,
    /// At its top-right.
    Right,
    /// At its top-centre.
    Centre,
}

/// What a [`TextRun`] labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextRole {
    /// An agent's name badge.
    Badge(AgentId),
    /// The wall board's brand.
    Brand,
    /// The wall board's star: a link to the repo.
    Star,
    /// A line of the wall board's mood or context.
    Board,
    /// The floor indicator over the elevator.
    Indicator,
    /// An agent's chitchat bubble.
    Bubble(AgentId),
}

impl TextRun {
    /// `agent`'s badge over `anchor` ([`badge_anchor`](crate::sim::anchors::badge_anchor)):
    /// its marker in the source's hue, then its name in its tone, on the
    /// badge plate.
    pub(crate) fn badge(
        anchor: Point,
        agent: &AgentSlot,
        namesakes: &crate::overlay::Namesakes<'_>,
        theme: &Theme,
    ) -> Self {
        let text = namesakes.text(agent);
        let ink = crate::overlay::badge_ink(&text, crate::overlay::tone_of(agent), theme);
        Self {
            at: anchor,
            align: Align::Over,
            spans: vec![
                TextSpan {
                    text: crate::overlay::BADGE_MARKER.to_string(),
                    ink: ink.marker,
                },
                TextSpan {
                    text,
                    ink: ink.name,
                },
            ],
            plate: Some(crate::overlay::badge_plate(theme)),
            role: TextRole::Badge(agent.agent_id),
        }
    }

    /// The floor indicator naming floor `floor` (one-based) over the elevator
    /// at `door`: centred on the door, in the cell over it, on the badge plate.
    pub(crate) fn indicator(door: Point, floor: usize, theme: &Theme) -> Self {
        Self {
            at: Point {
                x: door.x + crate::layout::ELEVATOR_W / 2,
                y: crate::layout::floor_indicator_rows(door.y).start,
            },
            align: Align::Centre,
            spans: vec![TextSpan {
                text: crate::layout::floor_indicator_text(floor),
                ink: theme.ui.neon_brand,
            }],
            plate: Some(crate::overlay::badge_plate(theme)),
            role: TextRole::Indicator,
        }
    }

    /// `bubble` over its speaker's badge among `badges`, in the tooltip's
    /// ink on its plate; `None` when its speaker wears no badge.
    pub(crate) fn bubble(
        bubble: &crate::chitchat::ChitchatBubble,
        badges: &[TextRun],
        theme: &Theme,
    ) -> Option<Self> {
        let badge = badges
            .iter()
            .find(|run| run.role == TextRole::Badge(bubble.speaker))?;
        Some(Self {
            at: Point {
                x: badge.at.x,
                y: badge.at.y.saturating_sub(BUBBLE_LIFT),
            },
            align: Align::Over,
            spans: vec![TextSpan {
                text: bubble.text.into(),
                ink: theme.ui.tooltip_text,
            }],
            plate: Some(theme.ui.tooltip_bg),
            role: TextRole::Bubble(bubble.speaker),
        })
    }

    /// Its spans' text, end to end.
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// The star's hit area: the logical cells its line covers on the
    /// terminal's cell grid, one per cell of its text, where the classic
    /// writes it.
    pub fn hit_box(&self) -> crate::layout::Bounds {
        self.place(cells(&self.text()))
    }

    /// The logical cells a line `w` cells wide takes where its align puts it:
    /// the one placement the hit test and every terminal painter read. It
    /// fills the cell row its anchor's row lies in, [`LABEL_GAP`] rows up for
    /// [`Align::Over`].
    pub fn place(&self, w: u16) -> crate::layout::Bounds {
        let h = crate::layout::CELL_ROWS;
        let x = match self.align {
            Align::Left => self.at.x,
            Align::Right => self.at.x.saturating_sub(w),
            Align::Over | Align::Centre => self.at.x.saturating_sub(w / 2),
        };
        let row = match self.align {
            Align::Over => self.at.y.saturating_sub(LABEL_GAP),
            Align::Left | Align::Right | Align::Centre => self.at.y,
        };
        crate::layout::Bounds {
            x,
            y: row / h * h,
            width: w,
            height: h,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A budget never splits a grapheme cluster: a ZWJ sequence is kept whole
    /// or dropped whole.
    #[test]
    fn a_take_keeps_clusters_whole() {
        let coder = "\u{1f469}\u{200d}\u{1f4bb}";
        let text = format!("a{coder}b");
        assert_eq!(take(&text, 3), format!("a{coder}"));
        assert_eq!(take(&text, 2), "a");
        assert_eq!(take(&text, u16::MAX), text);
    }

    #[test]
    fn a_run_is_its_advances_less_the_trailing_gap() {
        assert_eq!(width(""), ArtPx(0));
        assert_eq!(width("cc\u{b7}a"), ArtPx(4 * ADVANCE - 1));
    }
}
