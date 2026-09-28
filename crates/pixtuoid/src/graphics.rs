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

/// How long the capability query waits for the terminal, for `doctor` and a
/// run alike: one budget, or `doctor` could report a profile a run then misses.
///
/// The query ends with a device-status request every terminal answers, so this
/// only has to outlast a round trip — a second clears any real link.
#[cfg(feature = "graphics")]
pub(crate) const GRAPHICS_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// A graphics protocol the terminal speaks and the cutaway can be handed over.
///
/// Built only by the `graphics`-feature [`detect`]; a build without it still
/// names every protocol, so the plan is one type in both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "graphics"), allow(dead_code))]
pub(crate) enum ImageProtocol {
    /// kitty's graphics protocol.
    Kitty,
    /// DEC SIXEL.
    Sixel,
    /// iTerm2's inline images.
    Iterm2,
}

impl ImageProtocol {
    /// The protocol's name as a user would search for it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Kitty => "kitty",
            Self::Sixel => "sixel",
            Self::Iterm2 => "iterm2",
        }
    }
}

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
///
/// One variant per FACT, never merged: a merged reason makes the report assert
/// something it never established — "no answer" read as a verdict on the
/// terminal when the real cause was a pipe, or on the pipe when this build
/// cannot ask at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClassicReason {
    /// `--graphics off`.
    Disabled,
    /// This build has no `graphics` feature, so there is no query to run.
    Unsupported,
    /// The terminal was never asked — output is not a terminal, so the query
    /// would have gone into a pipe and nothing could answer it.
    NotQueried,
    /// The terminal was asked and did not answer.
    NoAnswer,
    /// The terminal answered, and it has no graphics protocol.
    NoProtocol,
    /// Inside tmux, with a protocol other than kitty.
    ///
    /// Every other protocol reaches the terminal through tmux's passthrough,
    /// which bypasses tmux (tmux(1), `allow-passthrough`): tmux never holds the
    /// image, so a pane or window switch redraws the pane without it. kitty's
    /// Unicode placeholders are ordinary cells tmux stores and redraws.
    TmuxNeedsKitty(ImageProtocol),
    /// The terminal has a protocol but its cell is too small for any scale
    /// the pack's art can land on.
    ///
    /// Real case, not defensive: a terminal that answers the protocol query but
    /// not the pixel-size one reports a zero or 1-px cell, and one pixel per
    /// logical unit IS the classic density — there is nothing to gain.
    CellTooSmall {
        /// The cell the terminal reported.
        cell: CellSize,
        /// The pack's densest art, `Pack::max_density_variant`.
        max_density: u16,
    },
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
        /// How the image reaches the terminal.
        protocol: ImageProtocol,
        /// The cell the scale was fitted to.
        cell: CellSize,
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
    /// The graphics protocol it speaks, `None` when it has none we can drive.
    pub protocol: Option<ImageProtocol>,
    /// The cell size it reports.
    pub cell: CellSize,
    /// Whether this process runs inside tmux.
    pub tmux: bool,
}

/// The outcome of asking the terminal — [`resolve`]'s input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "a build with the `graphics` feature never makes `Unsupported`, one without \
              never makes `Answered` or `NoAnswer`; `resolve` handles all of them"
)]
pub(crate) enum Probe {
    /// The terminal answered.
    Answered(Detected),
    /// Nothing was asked: output is not a terminal.
    NotQueried,
    /// The query ran and got no answer.
    NoAnswer,
    /// This build cannot ask.
    Unsupported,
}

/// The natural scale: what the CELL alone makes a logical unit, before the
/// pack gets a say.
///
/// Classic paints ONE buffer pixel per half-block, so a logical unit is one
/// cell wide and half a cell tall. Drawing the SAME logical office into real
/// pixels makes a logical unit `cell.w` px wide and `cell.h / 2` px tall —
/// equal exactly when the cell is the ~1:2 the half-block technique already
/// assumes. `RenderScale` is isotropic, so the SMALLER of the two wins:
/// stretched pixel art is the one outcome worth ruling out by construction.
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

/// Decide what to paint. Pure — [`detect`] supplies the probe.
pub(crate) fn resolve(mode: GraphicsMode, probe: Probe, max_density: u16) -> Plan {
    let classic = |reason| Plan::Classic { reason };
    if mode == GraphicsMode::Off {
        return classic(ClassicReason::Disabled);
    }
    let d = match probe {
        Probe::Answered(d) => d,
        Probe::NotQueried => return classic(ClassicReason::NotQueried),
        Probe::NoAnswer => return classic(ClassicReason::NoAnswer),
        Probe::Unsupported => return classic(ClassicReason::Unsupported),
    };
    let Some(protocol) = d.protocol else {
        return classic(ClassicReason::NoProtocol);
    };
    if d.tmux && protocol != ImageProtocol::Kitty {
        return classic(ClassicReason::TmuxNeedsKitty(protocol));
    }
    match render_scale_for_cell(d.cell, max_density) {
        // Scale 1 IS the classic density: an encode per frame that draws the
        // identical picture.
        Some(scale) if scale.get() > 1 => Plan::Cutaway {
            scale,
            protocol,
            cell: d.cell,
        },
        _ => classic(ClassicReason::CellTooSmall {
            cell: d.cell,
            max_density,
        }),
    }
}

impl ClassicReason {
    /// One line for `doctor` / the boot log, explaining the fallback.
    pub(crate) fn describe(self) -> String {
        match self {
            Self::Disabled => "disabled by --graphics off".to_string(),
            Self::Unsupported => "this build has no terminal-graphics support".to_string(),
            Self::NotQueried => "output is not a terminal, so nothing could answer the capability \
                 query — run without a pipe to see what this terminal supports"
                .to_string(),
            Self::NoAnswer => "the terminal did not answer the capability query".to_string(),
            Self::NoProtocol => {
                "terminal reports no graphics protocol (kitty/iterm2/sixel)".to_string()
            }
            Self::TmuxNeedsKitty(p) => format!(
                "inside tmux only kitty graphics survive a pane switch, and this terminal \
                 speaks {}",
                p.name()
            ),
            Self::CellTooSmall { cell, max_density } if max_density > 1 => format!(
                "terminal reports a {}x{} cell — too small for the pack's {max_density}x art",
                cell.w, cell.h
            ),
            Self::CellTooSmall { cell, .. } => format!(
                "terminal reports a {}x{} cell — too small to subdivide",
                cell.w, cell.h
            ),
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
    probe: Probe,
    max_density: u16,
) -> String {
    match resolve(mode, probe, max_density) {
        Plan::Cutaway {
            scale,
            protocol,
            cell,
        } => format!(
            "graphics: {} ({}x{} cell) — the cutaway profile would render at {}x \
             (not yet wired to `run`)",
            protocol.name(),
            cell.w,
            cell.h,
            scale.get()
        ),
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
/// Not a run's probe: upstream answers on a thread it detaches, which after a
/// timeout keeps reading stdin and, once a late reply lands, restores the
/// terminal mode it saw when it started (ratatui-image 11.0.8
/// `picker.rs:584-622`, `372-391`) — harmless before `doctor` exits, a stolen
/// keystroke and a dropped raw mode under a running TUI.
#[cfg(feature = "graphics")]
pub(crate) fn detect() -> Probe {
    use ratatui_image::picker::cap_parser::QueryStdioOptions;
    use ratatui_image::picker::{Picker, ProtocolType};

    let options = QueryStdioOptions {
        timeout: GRAPHICS_PROBE_TIMEOUT,
        ..QueryStdioOptions::default()
    };
    let Ok(picker) = Picker::from_query_stdio_with_options(options) else {
        return Probe::NoAnswer;
    };
    let font = picker.font_size();
    Probe::Answered(Detected {
        protocol: match picker.protocol_type() {
            ProtocolType::Kitty => Some(ImageProtocol::Kitty),
            ProtocolType::Sixel => Some(ImageProtocol::Sixel),
            ProtocolType::Iterm2 => Some(ImageProtocol::Iterm2),
            // ratatui-image's OWN "no protocol here" fallback: driving it would
            // hand our half-block office to a second one.
            ProtocolType::Halfblocks => None,
        },
        cell: CellSize {
            w: font.width,
            h: font.height,
        },
        tmux: is_tmux_env(
            std::env::var("TERM").ok().as_deref(),
            std::env::var("TERM_PROGRAM").ok().as_deref(),
        ),
    })
}

/// Built without the `graphics` feature: there is no query to run.
#[cfg(not(feature = "graphics"))]
pub(crate) fn detect() -> Probe {
    Probe::Unsupported
}

/// Whether `TERM`/`TERM_PROGRAM` place this process inside tmux — the test
/// ratatui-image itself applies before wrapping every image in passthrough
/// (11.0.8 `picker.rs:320-326`), mirrored because its `is_tmux` is private:
/// the two answers must agree, or a plan would ask for kitty-in-tmux while the
/// encoder wrapped nothing.
#[cfg(any(feature = "graphics", test))]
fn is_tmux_env(term: Option<&str>, term_program: Option<&str>) -> bool {
    term.is_some_and(|t| t.starts_with("tmux")) || term_program == Some("tmux")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL_8X16: CellSize = CellSize { w: 8, h: 16 };

    fn answered(protocol: Option<ImageProtocol>, cell: CellSize, tmux: bool) -> Probe {
        Probe::Answered(Detected {
            protocol,
            cell,
            tmux,
        })
    }

    fn capable(cell: CellSize) -> Probe {
        answered(Some(ImageProtocol::Kitty), cell, false)
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
    fn a_non_standard_cell_never_stretches_a_logical_unit() {
        // Taller than 1:2 — the width is the binding constraint.
        assert_eq!(
            render_scale_for_cell(CellSize { w: 8, h: 24 }, 1).map(|s| s.get()),
            Some(8)
        );
        // WIDER than 1:2 — height binds.
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

    /// Every protocol the picker can report reaches the plan as itself.
    #[test]
    fn auto_takes_the_cutaway_through_the_protocol_the_terminal_speaks() {
        for protocol in [
            ImageProtocol::Kitty,
            ImageProtocol::Sixel,
            ImageProtocol::Iterm2,
        ] {
            assert_eq!(
                resolve(
                    GraphicsMode::Auto,
                    answered(Some(protocol), CELL_8X16, false),
                    1
                ),
                Plan::Cutaway {
                    scale: RenderScale::new(8).expect("nonzero"),
                    protocol,
                    cell: CELL_8X16,
                }
            );
        }
    }

    /// Inside tmux, kitty alone survives; the rest fall back, naming the
    /// protocol that was refused.
    #[test]
    fn inside_tmux_only_kitty_takes_the_cutaway() {
        let plan = |p| resolve(GraphicsMode::Auto, answered(Some(p), CELL_8X16, true), 1);
        assert!(matches!(
            plan(ImageProtocol::Kitty),
            Plan::Cutaway {
                protocol: ImageProtocol::Kitty,
                ..
            }
        ));
        for p in [ImageProtocol::Sixel, ImageProtocol::Iterm2] {
            assert_eq!(
                plan(p),
                Plan::Classic {
                    reason: ClassicReason::TmuxNeedsKitty(p)
                }
            );
        }
    }

    /// The same `TERM`/`TERM_PROGRAM` test ratatui-image applies.
    #[test]
    fn tmux_is_named_by_term_or_term_program() {
        assert!(is_tmux_env(Some("tmux-256color"), None));
        assert!(is_tmux_env(Some("xterm-ghostty"), Some("tmux")));
        assert!(!is_tmux_env(Some("screen-256color"), Some("ghostty")));
        assert!(!is_tmux_env(None, None));
    }

    /// The fallback is the common path — most terminals have no protocol —
    /// so it must never be silent. A user asking "why is it not the pretty
    /// one" gets an answer in every shape.
    #[test]
    fn every_way_of_lacking_graphics_falls_back_with_a_reason() {
        let too_small = |cell, max_density| ClassicReason::CellTooSmall { cell, max_density };
        let cases = [
            (Probe::NotQueried, 1, ClassicReason::NotQueried),
            (Probe::NoAnswer, 1, ClassicReason::NoAnswer),
            (Probe::Unsupported, 1, ClassicReason::Unsupported),
            (
                answered(None, CELL_8X16, false),
                1,
                ClassicReason::NoProtocol,
            ),
            (
                capable(CellSize { w: 0, h: 0 }),
                1,
                too_small(CellSize { w: 0, h: 0 }, 1),
            ),
            (
                capable(CellSize { w: 1, h: 2 }),
                1,
                too_small(CellSize { w: 1, h: 2 }, 1),
            ),
            // 5 cannot reach 8x art within the fit's bound.
            (
                capable(CellSize { w: 5, h: 10 }),
                8,
                too_small(CellSize { w: 5, h: 10 }, 8),
            ),
        ];
        for (probe, max_density, want) in cases {
            assert_eq!(
                resolve(GraphicsMode::Auto, probe, max_density),
                Plan::Classic { reason: want },
                "for {probe:?}"
            );
            assert!(!want.describe().is_empty());
        }
    }

    /// A cell too small for the pack's art is a different sentence from one
    /// too small for any art: the first is fixed by a bigger font, the second
    /// by a terminal that reports its pixels.
    #[test]
    fn a_cell_too_small_names_the_art_it_is_too_small_for() {
        let reason = |max_density| ClassicReason::CellTooSmall {
            cell: CellSize { w: 5, h: 10 },
            max_density,
        };
        assert!(
            reason(8).describe().contains("the pack's 8x art"),
            "{}",
            reason(8).describe()
        );
        assert!(reason(1).describe().ends_with("too small to subdivide"));
    }

    #[test]
    fn the_doctor_row_names_the_protocol_and_never_leaves_a_fallback_unexplained() {
        let row = graphics_diagnostic_row(
            GraphicsMode::Auto,
            answered(Some(ImageProtocol::Sixel), CellSize { w: 17, h: 41 }, false),
            8,
        );
        assert!(row.starts_with("graphics: sixel (17x41 cell)"), "{row}");
        assert!(row.contains("would render at 16x"), "{row}");
        // The row reports a CAPABILITY. Until the profile reaches a painter it
        // must not read as a prediction about `run`, which paints classic
        // whatever this says.
        assert!(row.contains("not yet wired to `run`"), "{row}");

        // The fallback is the COMMON path, so every classic row must carry its
        // reason — a bare "classic" reads as a verdict on the office.
        for (mode, probe) in [
            (GraphicsMode::Off, capable(CELL_8X16)),
            (GraphicsMode::Auto, Probe::NotQueried),
            (GraphicsMode::Auto, Probe::NoAnswer),
            (GraphicsMode::Auto, Probe::Unsupported),
            (GraphicsMode::Auto, capable(CellSize { w: 0, h: 0 })),
        ] {
            let row = graphics_diagnostic_row(mode, probe, 1);
            assert!(row.starts_with("graphics: classic half-blocks — "), "{row}");
            assert!(
                row.len() > "graphics: classic half-blocks — ".len(),
                "the reason must not be empty: {row}"
            );
        }
    }

    /// A real Retina Ghostty reports a 17x41 cell: 17 is what the cell alone
    /// allows, and the pack's 8x art moves it to the nearest multiple.
    #[test]
    fn the_cell_gives_the_natural_scale_and_the_pack_fits_it() {
        assert_eq!(raw_scale_for_cell(CellSize { w: 17, h: 41 }), 17);
        assert_eq!(
            render_scale_for_cell(CellSize { w: 17, h: 41 }, 8).map(|s| s.get()),
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
                reason: ClassicReason::CellTooSmall {
                    cell: CellSize { w: 1, h: 2 },
                    max_density: 1,
                }
            },
            "1px per unit buys nothing"
        );
        assert!(
            matches!(
                resolve(GraphicsMode::Auto, capable(CellSize { w: 2, h: 4 }), 1),
                Plan::Cutaway { scale, .. } if scale.get() == 2
            ),
            "2px per unit is the first density worth an encode"
        );
    }
}
