//! The symbols text shows as art rather than as a font's glyph. A pixel
//! painter draws an icon's art from the pack's `[icons]`; a terminal writes
//! its [`Icon::terminal`] glyph, in the cells that glyph takes.

/// What a terminal leads a badge with, in
/// [`BadgeInk::marker`](crate::badge::BadgeInk::marker); a pixel painter draws
/// the plate's [`strip`](super::TextRun::strip) in its place.
pub const BADGE_MARKER: char = '\u{25cf}';

/// A symbol in a line of text, named by what it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    /// Agents waiting on you: the board's amber lamp.
    Waiting,
    /// Agents at work: the board's green lamp.
    Active,
    /// Agents idle: the board's grey lamp.
    Idle,
    /// The board's star, a link to the repo.
    Star,
    /// A gateway daemon.
    Gateway,
    /// The floor indicator's arrow to a floor above.
    Up,
    /// Its arrow where no floor is above.
    NoUp,
    /// Its arrow to a floor below.
    Down,
    /// Its arrow where no floor is below.
    NoDown,
}

impl Icon {
    pub const ALL: [Self; 9] = [
        Self::Waiting,
        Self::Active,
        Self::Idle,
        Self::Star,
        Self::Gateway,
        Self::Up,
        Self::NoUp,
        Self::Down,
        Self::NoDown,
    ];

    /// Its `[icons]` name in the pack: the lamps are one lamp in their
    /// tones' inks, and an arrow with no floor its way is the lit arrow in a
    /// dim one.
    pub fn art(self) -> &'static str {
        match self {
            Self::Waiting | Self::Active | Self::Idle => "lamp",
            Self::Star => "star",
            Self::Gateway => "gateway",
            Self::Up | Self::NoUp => "floor_up",
            Self::Down | Self::NoDown => "floor_down",
        }
    }

    /// What a terminal writes for it: an arrow with no floor its way is
    /// hollow.
    pub fn terminal(self) -> &'static str {
        match self {
            Self::Waiting | Self::Up => "\u{25b2}",
            Self::Active => "\u{25cf}",
            Self::Idle => "\u{25cb}",
            Self::Star => "\u{2605}",
            Self::Gateway => "\u{2b22}",
            Self::NoUp => "\u{25b3}",
            Self::Down => "\u{25bc}",
            Self::NoDown => "\u{25bd}",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::text::{LINE_H, cells, columns};

    /// Every icon has world art in the bundled pack, one cell wide less the
    /// gap per cell its terminal glyph takes, a line tall; and the pack draws
    /// no icon nothing names.
    #[test]
    fn every_icon_has_its_art_and_every_art_an_icon() {
        let pack = crate::pack::test_default_pack();
        for icon in Icon::ALL {
            let art = pack
                .icon(icon.art())
                .and_then(|a| a.world())
                .and_then(|s| s.frames().first())
                .unwrap_or_else(|| panic!("{icon:?} has no world art"));
            let w = columns(cells(icon.terminal())).0 - 1;
            assert_eq!((art.width(), art.height()), (w, LINE_H), "{icon:?}");
        }
        let named: std::collections::BTreeSet<&str> = Icon::ALL.iter().map(|i| i.art()).collect();
        let drawn: std::collections::BTreeSet<&str> = pack.icon_names().collect();
        assert_eq!(drawn, named);
    }

    /// Each icon's art draws in its text's ink: it would keep the pack's own
    /// colour whatever its text's tone.
    #[test]
    fn every_icon_draws_in_its_texts_ink() {
        let pack = crate::pack::test_default_pack();
        for icon in Icon::ALL {
            let inked = pack
                .icon(icon.art())
                .and_then(|a| a.world())
                .and_then(|s| s.recolorable(0))
                .is_some_and(|f| f.drawn_in(&[crate::pack::ICON_INK_KEY]).contains(&true));
            assert!(inked, "{icon:?}");
        }
    }
}
