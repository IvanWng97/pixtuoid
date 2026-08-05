//! Terminal-graphics capability, and the render scale it implies.
//!
//! The cutaway profile needs real pixels — a half-block cell is one buffer
//! pixel, which is the whole reason the classic office is drawn at the density
//! it is. Kitty/iTerm2/SIXEL give a terminal actual pixels, so this module
//! decides whether we HAVE them and, if so, how many a logical office unit is
//! worth.
//!
//! Split the way [`crate::term`] is: the policy is pure and unit-tested, the
//! one IO call (asking the terminal) is a thin wrapper that the tests never
//! reach. A terminal query cannot run under `cargo test` — output is captured,
//! so there is no tty to answer — and a detection module whose decisions are
//! only exercised through that query is a module with no tests at all.

use pixtuoid_scene::render_scale::RenderScale;

/// A terminal cell's size in real pixels, as the terminal reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CellSize {
    /// Cell width in pixels.
    pub w: u16,
    /// Cell height in pixels.
    pub h: u16,
}

/// What the user asked for on the command line.
///
/// The one `pub` item here, re-exported from the crate root: it is a field of
/// the `pub` [`crate::cli::Cmd`] and `main.rs` is a separate crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum GraphicsMode {
    /// Use terminal graphics when the terminal supports them.
    #[default]
    Auto,
    /// Never use terminal graphics, however capable the terminal is.
    Off,
}

/// Why a run is painting classic — the honest answer to "why is it not the
/// pretty one?", which a user is entitled to and `doctor` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClassicReason {
    /// `--graphics off`.
    Disabled,
    /// The terminal was never asked — output is not a terminal, so the query
    /// would have gone into a pipe and nothing could answer it.
    ///
    /// SEPARATE from [`Self::NoProtocol`] because collapsing them makes the
    /// report assert something it never established: it reads as a verdict on
    /// the terminal when the real cause is the pipe.
    NotQueried,
    /// The terminal answered, and it has no graphics protocol.
    NoProtocol,
    /// The terminal has a protocol but reports a cell too small to subdivide.
    ///
    /// Real case, not defensive: a terminal that answers the protocol query but
    /// not the pixel-size one reports a zero or 1-px cell, and one pixel per
    /// logical unit IS the classic density — there is nothing to gain.
    CellTooSmall(CellSize),
}

/// The resolved decision: what to paint, and — when it went the boring way —
/// why.
///
/// One type rather than a profile beside an `Option<reason>`, because "which
/// profile" and "why not the pretty one" are one answer: a cutaway plan has
/// nothing to explain and a classic plan always does. As two fields the pair
/// was constructible in both contradictory shapes, and the diagnostic carried
/// an "unknown" arm for a state no producer ever emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Plan {
    /// The orthographic cutaway, drawn at `scale` real pixels per logical unit
    /// and handed to the terminal as an image.
    Cutaway {
        /// Real pixels per logical office unit.
        scale: RenderScale,
    },
    /// The half-block office. One buffer pixel per cell.
    Classic {
        /// Why this run is not painting the cutaway.
        reason: ClassicReason,
    },
}

/// What the terminal said when asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Detected {
    /// True when the terminal speaks a graphics protocol we can drive.
    pub has_protocol: bool,
    /// The cell size it reports.
    pub cell: CellSize,
}

/// The largest scale the CELL alone allows, before the pack gets a say.
///
/// Classic paints ONE buffer pixel per half-block, so a logical unit is one
/// cell wide and half a cell tall. Drawing the SAME logical office into real
/// pixels makes a logical unit `cell.w` px wide and `cell.h / 2` px tall —
/// equal exactly when the cell is the ~1:2 the half-block technique already
/// assumes (the terminal-cell-aspect sharp edge). `RenderScale` is isotropic,
/// so the SMALLER of the two wins: a non-1:2 cell letterboxes the office
/// rather than stretching it, and stretched pixel art is the one outcome worth
/// ruling out by construction.
///
/// [`render_scale_for_cell`] is what callers want — this is half the answer.
fn raw_scale_for_cell(cell: CellSize) -> u16 {
    cell.w.min(cell.h / 2)
}

/// Real pixels per logical office unit, for this terminal and this pack.
///
/// Two facts meet here and neither belongs to the other: the CELL is the
/// terminal's, `max_density` is the PACK's. The pack half is
/// [`RenderScale::fit`] in the engine, so the window and canvas painters get
/// the same rule without re-deriving it — this function is only the terminal's
/// contribution to it.
pub(crate) fn render_scale_for_cell(cell: CellSize, max_density: u16) -> Option<RenderScale> {
    RenderScale::fit(raw_scale_for_cell(cell), max_density)
}

/// Decide what to paint. Pure — [`detect`] supplies the argument.
///
/// `detected` is `None` when the query could not run at all (not a tty, or the
/// terminal never answered) — the same profile as "no protocol" but a different
/// FACT, hence [`ClassicReason::NotQueried`] rather than one merged arm.
pub(crate) fn resolve(mode: GraphicsMode, detected: Option<Detected>, max_density: u16) -> Plan {
    if mode == GraphicsMode::Off {
        return Plan::Classic {
            reason: ClassicReason::Disabled,
        };
    }
    let Some(d) = detected else {
        return Plan::Classic {
            reason: ClassicReason::NotQueried,
        };
    };
    if !d.has_protocol {
        return Plan::Classic {
            reason: ClassicReason::NoProtocol,
        };
    }
    match render_scale_for_cell(d.cell, max_density) {
        // Scale 1 IS the classic density: an encode per frame that draws the
        // identical picture.
        Some(scale) if scale.get() > 1 => Plan::Cutaway { scale },
        _ => Plan::Classic {
            reason: ClassicReason::CellTooSmall(d.cell),
        },
    }
}

impl ClassicReason {
    /// One line for `doctor` / the boot log, explaining the fallback.
    pub(crate) fn describe(self) -> String {
        match self {
            Self::Disabled => "disabled by --graphics off".to_string(),
            Self::NotQueried => "output is not a terminal, so nothing could answer the capability \
                 query — run without a pipe to see what this terminal supports"
                .to_string(),
            Self::NoProtocol => {
                "terminal reports no graphics protocol (kitty/iterm2/sixel)".to_string()
            }
            Self::CellTooSmall(c) => {
                format!(
                    "terminal reports a {}x{} cell — too small to subdivide",
                    c.w, c.h
                )
            }
        }
    }
}

/// The `graphics:` line for `doctor` — the profile this terminal is CAPABLE of,
/// and why it falls back when it is not.
///
/// Capability, not a prediction: `run` paints classic unconditionally today, so
/// a row phrased as "what a run would paint" promised a cutaway the binary
/// never delivers. This row says what the profile WILL pick up once it is wired
/// to a painter.
///
/// Pure, so the wording is unit-tested; `doctor` supplies the probe result the
/// same way it does for the truecolor row beside it.
pub(crate) fn graphics_diagnostic_row(
    mode: GraphicsMode,
    detected: Option<Detected>,
    max_density: u16,
) -> String {
    match resolve(mode, detected, max_density) {
        Plan::Cutaway { scale } => {
            let cell = detected.map_or_else(
                || String::from("unknown cell"),
                |d| format!("{}x{} cell", d.cell.w, d.cell.h),
            );
            format!(
                "graphics: terminal graphics available ({cell}) — the cutaway profile \
                 would render at {}x (not yet wired to `run`)",
                scale.get()
            )
        }
        Plan::Classic { reason } => {
            format!("graphics: classic half-blocks — {}", reason.describe())
        }
    }
}

/// Ask the terminal what it can do.
///
/// The IO half, and the one part of this module tests never reach:
/// `Picker::from_query_stdio` writes escape sequences to the real terminal and
/// reads the replies, which needs a tty. Under `cargo test` stdout is captured,
/// so it would query nothing and answer nothing useful.
///
/// A failed query is `None`, not an error: every caller's fallback is the
/// classic profile, which is also what a terminal without graphics gets, and a
/// visualiser that refuses to start because it could not ask a question would
/// be worse than one that draws the plain office.
///
/// **Costs up to 2s on a terminal that never answers** — 20x this repo's own
/// 100ms [`crate::term::TRUECOLOR_PROBE_TIMEOUT`]. That is upstream's DEFAULT,
/// not a fixed cost: `STDIN_READ_TIMEOUT_MILLIS` is only what
/// `QueryStdioOptions::default()` puts in its `timeout` field, and
/// `Picker::from_query_stdio_with_options` takes any `Duration`. `doctor` keeps
/// the generous default because it is a diagnostic the user waits on
/// deliberately, and a slow terminal answering late is the answer it wants; a
/// future `run` wiring should pass [`crate::term::TRUECOLOR_PROBE_TIMEOUT`]
/// instead of designing the query off the boot path.
#[cfg(feature = "graphics")]
pub(crate) fn detect() -> Option<Detected> {
    use ratatui_image::picker::{Picker, ProtocolType};

    let picker = Picker::from_query_stdio().ok()?;
    let font = picker.font_size();
    let (w, h) = (font.width, font.height);
    Some(Detected {
        // `Halfblocks` is ratatui-image's OWN "no protocol here" fallback, so
        // driving it would hand our half-block office to a second one.
        has_protocol: picker.protocol_type() != ProtocolType::Halfblocks,
        cell: CellSize { w, h },
    })
}

/// Built without the `graphics` feature: there is no query to run.
///
/// `None` is the SAME answer a non-tty gets, and `ClassicReason::NotQueried`
/// already words it as "nothing could answer", so the report stays honest
/// without a fourth reason for "this build cannot ask".
#[cfg(not(feature = "graphics"))]
pub(crate) fn detect() -> Option<Detected> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL_8X16: CellSize = CellSize { w: 8, h: 16 };

    fn capable(cell: CellSize) -> Option<Detected> {
        Some(Detected {
            has_protocol: true,
            cell,
        })
    }

    /// The ~1:2 cell the whole half-block technique assumes: 8 wide, 16
    /// tall, so a logical unit is 8px either way and the office keeps its
    /// proportions exactly.
    #[test]
    fn a_standard_cell_yields_its_width_as_the_scale() {
        assert_eq!(
            render_scale_for_cell(CELL_8X16, 1).map(|s| s.get()),
            Some(8)
        );
        assert_eq!(
            render_scale_for_cell(CellSize { w: 10, h: 20 }, 1).map(|s| s.get()),
            Some(10)
        );
    }

    #[test]
    fn a_non_standard_cell_letterboxes_rather_than_stretching() {
        // Taller than 1:2 — the width is the binding constraint.
        assert_eq!(
            render_scale_for_cell(CellSize { w: 8, h: 24 }, 1).map(|s| s.get()),
            Some(8)
        );
        // WIDER than 1:2 — height binds. Picking width overflows the rows given,
        // which the image widget silently refuses to draw (spike-measured).
        assert_eq!(
            render_scale_for_cell(CellSize { w: 12, h: 16 }, 1).map(|s| s.get()),
            Some(8)
        );
    }

    /// A terminal that answers the protocol query but not the pixel-size
    /// one reports these. `RenderScale` cannot be zero, so the Option is
    /// the honest return rather than a clamp to 1.
    #[test]
    fn a_degenerate_cell_has_no_scale_at_all() {
        assert_eq!(render_scale_for_cell(CellSize { w: 0, h: 0 }, 1), None);
        assert_eq!(render_scale_for_cell(CellSize { w: 8, h: 1 }, 1), None);
        assert_eq!(render_scale_for_cell(CellSize { w: 0, h: 16 }, 1), None);
    }

    #[test]
    fn off_beats_a_capable_terminal() {
        // The flag is the user's, not a hint — a capable terminal must not
        // override it.
        assert_eq!(
            resolve(GraphicsMode::Off, capable(CELL_8X16), 1),
            Plan::Classic {
                reason: ClassicReason::Disabled
            }
        );
    }

    #[test]
    fn auto_takes_the_cutaway_on_a_capable_terminal() {
        let plan = resolve(GraphicsMode::Auto, capable(CELL_8X16), 1);
        let Plan::Cutaway { scale } = plan else {
            panic!("expected the cutaway, got {plan:?}");
        };
        assert_eq!(scale.get(), 8);
    }

    /// The fallback is the common path — most terminals have no protocol —
    /// so it must never be silent. A user asking "why is it not the pretty
    /// one" gets an answer in all four shapes.
    #[test]
    fn every_way_of_lacking_graphics_falls_back_with_a_reason() {
        let cases = [
            (None, ClassicReason::NotQueried),
            (
                Some(Detected {
                    has_protocol: false,
                    cell: CELL_8X16,
                }),
                ClassicReason::NoProtocol,
            ),
            (
                capable(CellSize { w: 0, h: 0 }),
                ClassicReason::CellTooSmall(CellSize { w: 0, h: 0 }),
            ),
            (
                capable(CellSize { w: 1, h: 2 }),
                ClassicReason::CellTooSmall(CellSize { w: 1, h: 2 }),
            ),
        ];
        for (detected, want) in cases {
            assert_eq!(
                resolve(GraphicsMode::Auto, detected, 1),
                Plan::Classic { reason: want },
                "for {detected:?}"
            );
            assert!(!want.describe().is_empty());
        }
    }

    #[test]
    fn the_doctor_row_names_the_profile_and_never_leaves_a_fallback_unexplained() {
        let row = graphics_diagnostic_row(GraphicsMode::Auto, capable(CELL_8X16), 1);
        assert!(row.contains("8x16 cell"), "{row}");
        assert!(row.contains("would render at 8x"), "{row}");
        // The row reports a CAPABILITY. Until the profile reaches a painter it
        // must not read as a prediction about `run`, which paints classic
        // whatever this says.
        assert!(row.contains("not yet wired to `run`"), "{row}");

        // The fallback is the COMMON path, so every classic row must carry its
        // reason — a bare "classic" reads as a verdict on the office.
        for (mode, detected) in [
            (GraphicsMode::Off, capable(CELL_8X16)),
            (GraphicsMode::Auto, None),
            (GraphicsMode::Auto, capable(CellSize { w: 0, h: 0 })),
        ] {
            let row = graphics_diagnostic_row(mode, detected, 1);
            assert!(row.starts_with("graphics: classic half-blocks — "), "{row}");
            assert!(
                row.len() > "graphics: classic half-blocks — ".len(),
                "the reason must not be empty: {row}"
            );
        }
    }

    /// The terminal half of the scale: a real Retina Ghostty reports a 17x41
    /// cell, and 17 is what the CELL alone allows. What the pack then does with
    /// it is `RenderScale::fit`'s test, not this one.
    #[test]
    fn the_cell_alone_gives_the_raw_scale() {
        assert_eq!(raw_scale_for_cell(CellSize { w: 17, h: 41 }), 17);
        // And the two halves compose: 4x art cannot land on a prime scale.
        assert_eq!(
            render_scale_for_cell(CellSize { w: 17, h: 41 }, 4).map(|s| s.get()),
            Some(16)
        );
    }

    /// Exactly 1 real pixel per logical unit IS the classic density, so an
    /// image encode every frame would cost the encode and draw the identical
    /// picture. 2 is the first scale that buys anything, and the boundary is
    /// pinned from BOTH sides so a future `>=` typo cannot slip through.
    #[test]
    fn the_cutoff_is_where_the_image_path_starts_buying_something() {
        assert_eq!(
            resolve(GraphicsMode::Auto, capable(CellSize { w: 1, h: 2 }), 1),
            Plan::Classic {
                reason: ClassicReason::CellTooSmall(CellSize { w: 1, h: 2 })
            },
            "1px per unit buys nothing"
        );
        assert_eq!(
            resolve(GraphicsMode::Auto, capable(CellSize { w: 2, h: 4 }), 1),
            Plan::Cutaway {
                scale: RenderScale::new(2).expect("nonzero"),
            },
            "2px per unit is the first density worth an encode"
        );
    }
}
