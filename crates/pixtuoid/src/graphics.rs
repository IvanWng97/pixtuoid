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
pub struct CellSize {
    /// Cell width in pixels.
    pub w: u16,
    /// Cell height in pixels.
    pub h: u16,
}

/// What the user asked for on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GraphicsMode {
    /// Use terminal graphics when the terminal supports them.
    #[default]
    Auto,
    /// Never use terminal graphics, however capable the terminal is.
    Off,
}

/// Which profile a run will actually paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// The half-block office. One buffer pixel per cell.
    Classic,
    /// The orthographic cutaway, drawn at `scale` real pixels per logical unit
    /// and handed to the terminal as an image.
    Cutaway {
        /// Real pixels per logical office unit.
        scale: RenderScale,
    },
}

/// Why a run is painting `Classic` — the honest answer to "why is it not the
/// pretty one?", which a user is entitled to and `doctor` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassicReason {
    /// `--graphics off`.
    Disabled,
    /// The terminal was never asked — output is not a terminal, so the query
    /// would have gone into a pipe and nothing could answer it.
    ///
    /// SEPARATE from [`Self::NoProtocol`] because collapsing them makes the
    /// report assert something it never established. It reads as a verdict on
    /// the terminal when the real cause is the pipe, and the first person it
    /// misled was its author, running `doctor | grep graphics:` and believing
    /// the answer.
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

/// The resolved decision plus, when it went the boring way, the reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// What to paint.
    pub profile: Profile,
    /// Set exactly when `profile` is [`Profile::Classic`].
    pub reason: Option<ClassicReason>,
}

/// What the terminal said when asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detected {
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
/// Rounded DOWN to a multiple of `max_density`, because a density variant is
/// only usable at a scale its density divides. Found on a real Retina Ghostty:
/// a 17px cell makes 17 the natural scale, 17 is PRIME, and every variant in
/// the pack sits unused while the base art block-scales 17x. Giving up at most
/// `max_density - 1` px of office (17 -> 16, ~6%) to make the richer art
/// reachable is not a close call — not being upscaled is the whole point of
/// the variants.
///
/// A pack with no variants reports 1, so this is the identity there and the
/// classic-density path is untouched.
pub fn render_scale_for_cell(cell: CellSize, max_density: u16) -> Option<RenderScale> {
    let raw = raw_scale_for_cell(cell);
    let usable = if max_density >= 2 && raw >= max_density {
        (raw / max_density) * max_density
    } else {
        raw
    };
    RenderScale::new(usable)
}

/// Decide what to paint. Pure — [`detect`] supplies the argument.
///
/// `detected` is `None` when the query could not run at all (not a tty, or the
/// terminal never answered). Same PROFILE as "no protocol", different FACT —
/// hence [`ClassicReason::NotQueried`] rather than one merged arm. The profile
/// is all the renderer needs, but the reason is what the user reads, and a
/// report that says the terminal answered when it was never asked sends them
/// to fix the wrong thing.
pub fn resolve(mode: GraphicsMode, detected: Option<Detected>, max_density: u16) -> Plan {
    let classic = |reason| Plan {
        profile: Profile::Classic,
        reason: Some(reason),
    };
    if mode == GraphicsMode::Off {
        return classic(ClassicReason::Disabled);
    }
    let Some(d) = detected else {
        return classic(ClassicReason::NotQueried);
    };
    if !d.has_protocol {
        return classic(ClassicReason::NoProtocol);
    }
    match render_scale_for_cell(d.cell, max_density) {
        // Scale 1 IS the classic density: an encode per frame that draws the
        // identical picture.
        Some(scale) if scale.get() > 1 => Plan {
            profile: Profile::Cutaway { scale },
            reason: None,
        },
        _ => classic(ClassicReason::CellTooSmall(d.cell)),
    }
}

impl ClassicReason {
    /// One line for `doctor` / the boot log, explaining the fallback.
    pub fn describe(self) -> String {
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

/// The `graphics:` line for `doctor` — what a `run` in THIS terminal would
/// paint, and why.
///
/// Pure, so the wording is unit-tested; `doctor` supplies the probe result the
/// same way it does for the truecolor row beside it.
pub fn graphics_diagnostic_row(
    mode: GraphicsMode,
    detected: Option<Detected>,
    max_density: u16,
) -> String {
    let plan = resolve(mode, detected, max_density);
    match plan.profile {
        Profile::Cutaway { scale } => {
            let cell = detected.map_or_else(
                || String::from("unknown cell"),
                |d| format!("{}x{} cell", d.cell.w, d.cell.h),
            );
            format!(
                "graphics: cutaway at {}x ({cell}) — terminal graphics available",
                scale.get()
            )
        }
        Profile::Classic => format!(
            "graphics: classic half-blocks — {}",
            plan.reason
                .map_or_else(|| "unknown".to_string(), ClassicReason::describe)
        ),
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
/// **Costs up to 2s on a terminal that never answers** (upstream's
/// `STDIN_READ_TIMEOUT_MILLIS`, not configurable through `from_query_stdio`) —
/// 20x this repo's own 100ms [`crate::term::TRUECOLOR_PROBE_TIMEOUT`]. Fine for
/// `doctor`, which is a diagnostic the user waits on deliberately. NOT fine on
/// the `run` boot path, where it would be a two-second stall before the office
/// appears on exactly the terminals that get the plain office anyway — so
/// wiring `run` needs the query off the critical path (a first-frame-classic
/// then upgrade, or a cached answer), not a call in the same place.
pub fn detect() -> Option<Detected> {
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
        let plan = resolve(GraphicsMode::Off, capable(CELL_8X16), 1);
        assert_eq!(plan.profile, Profile::Classic);
        assert_eq!(plan.reason, Some(ClassicReason::Disabled));
    }

    #[test]
    fn auto_takes_the_cutaway_on_a_capable_terminal() {
        let plan = resolve(GraphicsMode::Auto, capable(CELL_8X16), 1);
        let Profile::Cutaway { scale } = plan.profile else {
            panic!("expected the cutaway, got {:?}", plan.profile);
        };
        assert_eq!(scale.get(), 8);
        assert_eq!(plan.reason, None, "a cutaway run has nothing to explain");
    }

    /// The fallback is the common path — most terminals have no protocol —
    /// so it must never be silent. A user asking "why is it not the pretty
    /// one" gets an answer in all four shapes.
    #[test]
    fn every_way_of_lacking_graphics_falls_back_with_a_reason() {
        let cases = [
            // Never-asked vs asked-and-told-no are different facts — collapsing
            // them made a piped `doctor | grep` read as a verdict on the terminal.
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
            let plan = resolve(GraphicsMode::Auto, detected, 1);
            assert_eq!(plan.profile, Profile::Classic, "for {detected:?}");
            assert_eq!(plan.reason, Some(want), "for {detected:?}");
            assert!(!plan.reason.expect("set").describe().is_empty());
        }
    }

    #[test]
    fn the_doctor_row_names_the_profile_and_never_leaves_a_fallback_unexplained() {
        let row = graphics_diagnostic_row(GraphicsMode::Auto, capable(CELL_8X16), 1);
        assert!(row.starts_with("graphics: cutaway at 8x"), "{row}");
        assert!(row.contains("8x16 cell"), "{row}");

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

    /// The real case, from a Retina Ghostty: a 17x41 cell. 17 is PRIME, so
    /// with the natural scale every 4x variant in the pack is unusable and
    /// the base art block-scales 17x — the mixed-density work would be inert
    /// on this machine while reporting success.
    #[test]
    fn the_scale_rounds_down_so_the_packs_densest_art_can_actually_land() {
        let retina = CellSize { w: 17, h: 41 };
        assert_eq!(raw_scale_for_cell(retina), 17, "the cell alone says 17");
        assert_eq!(
            render_scale_for_cell(retina, 4).map(|s| s.get()),
            Some(16),
            "4x art must divide the scale, so 17 rounds to 16"
        );

        // A pack with no variants must be untouched — this rule may only ever
        // COST office area when there is richer art to spend it on.
        assert_eq!(render_scale_for_cell(retina, 1).map(|s| s.get()), Some(17));

        // Already a multiple: nothing to give up.
        assert_eq!(
            render_scale_for_cell(CELL_8X16, 4).map(|s| s.get()),
            Some(8)
        );

        // Art DENSER than the whole scale can never land however we round, so
        // rounding to zero (no office at all) must not be the answer.
        assert_eq!(
            render_scale_for_cell(CellSize { w: 3, h: 8 }, 4).map(|s| s.get()),
            Some(3)
        );
    }

    /// Exactly 1 real pixel per logical unit IS the classic density, so an
    /// image encode every frame would cost the encode and draw the identical
    /// picture. 2 is the first scale that buys anything, and the boundary is
    /// pinned from BOTH sides so a future `>=` typo cannot slip through.
    #[test]
    fn the_cutoff_is_where_the_image_path_starts_buying_something() {
        let plan = resolve(GraphicsMode::Auto, capable(CellSize { w: 1, h: 2 }), 1);
        assert_eq!(plan.profile, Profile::Classic, "1px per unit buys nothing");

        let plan = resolve(GraphicsMode::Auto, capable(CellSize { w: 2, h: 4 }), 1);
        assert_eq!(
            plan.profile,
            Profile::Cutaway {
                scale: RenderScale::new(2).expect("nonzero"),
            },
            "2px per unit is the first density worth an encode"
        );
    }
}
