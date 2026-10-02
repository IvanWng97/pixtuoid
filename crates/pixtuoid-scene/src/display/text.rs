//! The words a frame carries beside its pixels, for the painter to set.

use pixtuoid_core::sprite::Rgb;

use crate::layout::Point;

pub(crate) use crate::overlay::LabelTone as BadgeTone;

/// One run of text the painter sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextRun {
    /// Baseline-left, in logical pixels.
    pub(crate) at: Point,
    pub(crate) text: String,
    pub(crate) role: TextRole,
}

/// What a [`TextRun`] is, which decides how it is set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TextRole {
    Badge { tone: BadgeTone, hue: Rgb },
    Board,
    Bubble,
    Sign,
}
