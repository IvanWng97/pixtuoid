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
//! reach. A terminal query cannot be a test — it writes escapes to whatever
//! terminal runs the suite and waits on a reply that may never come — and a
//! detection module whose decisions are only exercised through that query is a
//! module with no tests at all.

use pixtuoid_scene::render_scale::RenderScale;

#[cfg(feature = "graphics")]
mod probe;
#[cfg(feature = "graphics")]
pub(crate) use probe::probe;

/// How long each wait of the capability probe may take: on Unix, the query start
/// to finish, where [`probe()`] reads the reply itself, and, inside tmux, the
/// `allow-passthrough` read before it; on Windows, each read of the reply, since
/// the upstream probe restarts the clock on every one (ratatui-image 11.0.8
/// `picker.rs:615`).
///
/// The query ends with a device-status request (ratatui-image 11.0.8
/// `cap_parser.rs:132-134`), so a terminal that answers ends the wait the
/// moment its reply is complete; the budget is only spent where no reply comes
/// (a ConPTY that drops replies, a hidden tmux pane refusing passthrough), which
/// is why it can outlast [`crate::term::TRUECOLOR_PROBE_TIMEOUT`], whose query
/// has no such terminator.
#[cfg(feature = "graphics")]
pub(crate) const GRAPHICS_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// A graphics protocol the terminal speaks and the cutaway can be handed over.
///
/// Built only by the `graphics`-feature [`probe()`]; a build without it still
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
    pub(crate) w: u16,
    /// Cell height in pixels.
    pub(crate) h: u16,
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
/// One variant per fact the probe can establish: a merged reason makes the
/// report assert something it never established — a verdict on the terminal
/// when the real cause was a pipe, or on the pipe when this build cannot ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClassicReason {
    /// `--graphics off`.
    Disabled,
    /// This build has no `graphics` feature, so there is no query to run.
    Unsupported,
    /// The terminal was never asked.
    NotQueried,
    /// The terminal was asked and its reply never completed.
    NoAnswer,
    /// The terminal answered, and it has no graphics protocol — or, off Unix,
    /// never answered: upstream's probe there folds a silence into no protocol
    /// (see [`probe()`]).
    NoProtocol,
    /// Inside tmux with `allow-passthrough` off: no image, and no query for
    /// one, reaches the terminal (tmux(1), `allow-passthrough`).
    TmuxPassthroughOff,
    /// The terminal has a protocol but reports no cell size, so there is no
    /// scale to fit.
    NoCellSize,
    /// Inside tmux, with a protocol other than kitty.
    ///
    /// SIXEL and iTerm2, as ratatui-image sends them inside tmux, ride tmux's
    /// passthrough (tmux(1), `allow-passthrough`), which bypasses tmux: tmux
    /// never holds the image, so a pane or window switch redraws the pane
    /// without it. kitty's Unicode placeholders are ordinary cells tmux stores
    /// and redraws.
    TmuxNeedsKitty(ImageProtocol),
    /// The terminal has a protocol, but its cell is too small for any scale the
    /// pack's art lands on: a small font against dense art (a 5-px-wide cell
    /// cannot reach 8x art within [`RenderScale::fit`]'s bound), or a 1-px
    /// cell, where one pixel per logical unit IS the classic density.
    CellTooSmall {
        /// The cell the terminal reported.
        cell: CellSize,
        /// The least dense of the pack's density variants — the one the cell
        /// came nearest to landing — or 1 when it ships none.
        density: u16,
    },
}

/// The resolved decision: what to paint, and — when it went the boring way —
/// why.
///
/// One type rather than a profile beside an `Option<reason>`: a cutaway plan
/// has nothing to explain and a classic plan always does, and two fields could
/// hold both contradictory shapes.
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
        /// Inside tmux: the encoder wraps the image in passthrough.
        tmux: bool,
    },
    /// The half-block office: one buffer pixel per half-block.
    Classic {
        /// Why this run is not painting the cutaway.
        reason: ClassicReason,
    },
}

/// What the probe learned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Detected {
    /// The graphics protocol the terminal speaks, `None` when it has none we
    /// can drive.
    pub(crate) protocol: Option<ImageProtocol>,
    /// The cell size the terminal reports, `None` when it reports none.
    pub(crate) cell: Option<CellSize>,
    /// Whether this process runs inside tmux — from the environment, not the
    /// terminal's answer.
    pub(crate) tmux: bool,
}

/// The outcome of asking the terminal — [`resolve`]'s input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Probe {
    /// What the terminal's answer and the environment established — the
    /// environment alone where the terminal never answered but names a
    /// protocol, as upstream falls back (see [`probe()`]).
    #[cfg_attr(not(feature = "graphics"), allow(dead_code))]
    Detected(Detected),
    /// The terminal was never asked: the caller said not to, or no controlling
    /// terminal took the query.
    #[cfg_attr(not(feature = "graphics"), allow(dead_code))]
    NotQueried,
    /// The terminal was asked, its reply never completed, and the environment
    /// names no protocol to fall back on. Only the Unix probe can tell (see
    /// [`probe()`]).
    #[cfg_attr(not(all(feature = "graphics", unix)), allow(dead_code))]
    NoAnswer,
    /// Inside tmux with `allow-passthrough` off, so nothing was asked.
    #[cfg_attr(not(all(feature = "graphics", unix)), allow(dead_code))]
    TmuxPassthroughOff,
    /// This build cannot ask.
    #[cfg_attr(feature = "graphics", allow(dead_code))]
    Unsupported,
}

/// The natural scale: what the CELL alone makes a logical unit, before the
/// pack gets a say.
///
/// Classic paints ONE buffer pixel per half-block, so a logical unit is one
/// cell wide and half a cell tall. Drawing the SAME logical office into real
/// pixels makes a logical unit `cell.w` px wide and `cell.h / 2` px tall —
/// equal exactly when the cell is the ~1:2 the half-block technique already
/// assumes. `RenderScale` is isotropic, so one axis has to give; the SMALLER
/// wins so neither axis shows less office than classic does.
fn raw_scale_for_cell(cell: CellSize) -> u16 {
    cell.w.min(cell.h / 2)
}

/// The terminal's half of [`RenderScale::fit`]: the natural scale its cell
/// gives, fitted to the densest of `densities` (the pack's
/// [`Pack::density_variants`](pixtuoid_core::sprite::format::Pack::density_variants))
/// that lands.
///
/// The densest that lands, not the densest alone: one outlier `@16x` sprite
/// must not switch the cutaway off on a terminal its 8x art lands on. A pack
/// with no variants lands its base art at the natural scale.
pub(crate) fn render_scale_for_cell(cell: CellSize, densities: &[u16]) -> Option<RenderScale> {
    let natural = raw_scale_for_cell(cell);
    if densities.is_empty() {
        return RenderScale::fit(natural, 1);
    }
    densities
        .iter()
        .filter_map(|&d| Some((d, RenderScale::fit(natural, d)?)))
        .max_by_key(|&(d, _)| d)
        .map(|(_, scale)| scale)
}

/// Decide what to paint. Pure — [`probe()`] supplies the probe, and `densities`
/// are the pack's [`Pack::density_variants`](pixtuoid_core::sprite::format::Pack::density_variants).
pub(crate) fn resolve(mode: GraphicsMode, probe: Probe, densities: &[u16]) -> Plan {
    let classic = |reason| Plan::Classic { reason };
    if mode == GraphicsMode::Off {
        return classic(ClassicReason::Disabled);
    }
    let d = match probe {
        Probe::Detected(d) => d,
        Probe::NotQueried => return classic(ClassicReason::NotQueried),
        Probe::NoAnswer => return classic(ClassicReason::NoAnswer),
        Probe::TmuxPassthroughOff => return classic(ClassicReason::TmuxPassthroughOff),
        Probe::Unsupported => return classic(ClassicReason::Unsupported),
    };
    let Some(protocol) = d.protocol else {
        return classic(ClassicReason::NoProtocol);
    };
    let Some(cell) = d.cell else {
        return classic(ClassicReason::NoCellSize);
    };
    // The cell before tmux: a user told to switch to kitty should not then
    // find the cell was too small all along.
    let scale = match render_scale_for_cell(cell, densities) {
        // Scale 1 IS the classic density: an encode per frame that draws the
        // identical picture.
        Some(scale) if scale.get() > 1 => scale,
        _ => {
            return classic(ClassicReason::CellTooSmall {
                cell,
                density: densities.iter().copied().min().unwrap_or(1),
            })
        }
    };
    if d.tmux && protocol != ImageProtocol::Kitty {
        return classic(ClassicReason::TmuxNeedsKitty(protocol));
    }
    Plan::Cutaway {
        scale,
        protocol,
        cell,
        tmux: d.tmux,
    }
}

impl ClassicReason {
    /// One line for `doctor`, explaining the fallback.
    pub(crate) fn describe(self) -> String {
        match self {
            Self::Disabled => "disabled by --graphics off".to_string(),
            Self::Unsupported => "this build has no terminal-graphics support".to_string(),
            Self::NotQueried => "the terminal was not asked (stdout is not a terminal, there \
                 is no controlling terminal, or $TERM is dumb) — run in an interactive \
                 terminal to see what it supports"
                .to_string(),
            Self::NoAnswer => "the terminal did not answer the capability query".to_string(),
            Self::NoProtocol if cfg!(unix) => {
                "terminal reports no graphics protocol (kitty/iterm2/sixel)".to_string()
            }
            Self::NoProtocol => "terminal reports no graphics protocol (kitty/iterm2/sixel), \
                 or did not answer the capability query"
                .to_string(),
            Self::TmuxPassthroughOff => "inside tmux with allow-passthrough off — \
                 `set -g allow-passthrough on` lets kitty graphics through"
                .to_string(),
            Self::NoCellSize => "terminal reports no cell size in pixels".to_string(),
            Self::TmuxNeedsKitty(p) => format!(
                "inside tmux only kitty graphics survive a pane switch here, and this terminal \
                 speaks {}",
                p.name()
            ),
            Self::CellTooSmall { cell, density } if density > 1 && raw_scale_for_cell(cell) > 1 => {
                format!(
                    "terminal reports a {}x{} cell — too small for the pack's {density}x art",
                    cell.w, cell.h
                )
            }
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
    densities: &[u16],
) -> String {
    match resolve(mode, probe, densities) {
        Plan::Cutaway {
            scale,
            protocol,
            cell,
            ..
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

/// Built without the `graphics` feature: there is no query to run, whatever
/// the terminal.
#[cfg(not(feature = "graphics"))]
pub(crate) fn probe(_ask: bool) -> Probe {
    Probe::Unsupported
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL_8X16: CellSize = CellSize { w: 8, h: 16 };
    /// A pack with no density variants.
    const BASE_ONLY: &[u16] = &[];
    /// The bundled pack's densities.
    const BUNDLED: &[u16] = &[8];

    fn answered(protocol: Option<ImageProtocol>, cell: CellSize, tmux: bool) -> Probe {
        Probe::Detected(Detected {
            protocol,
            cell: Some(cell),
            tmux,
        })
    }

    fn capable(cell: CellSize) -> Probe {
        answered(Some(ImageProtocol::Kitty), cell, false)
    }

    fn scale(cell: CellSize, densities: &[u16]) -> Option<u16> {
        render_scale_for_cell(cell, densities).map(RenderScale::get)
    }

    /// The ~1:2 cell the whole half-block technique assumes: 8 wide, 16
    /// tall, so a logical unit is 8px either way and the office keeps its
    /// proportions exactly.
    #[test]
    fn a_standard_cell_yields_its_width_as_the_scale() {
        assert_eq!(scale(CELL_8X16, BASE_ONLY), Some(8));
        assert_eq!(scale(CellSize { w: 10, h: 20 }, BASE_ONLY), Some(10));
    }

    #[test]
    fn a_non_standard_cell_never_stretches_a_logical_unit() {
        // Taller than 1:2 — the width is the binding constraint.
        assert_eq!(scale(CellSize { w: 8, h: 24 }, BASE_ONLY), Some(8));
        // WIDER than 1:2 — height binds.
        assert_eq!(scale(CellSize { w: 12, h: 16 }, BASE_ONLY), Some(8));
    }

    /// `RenderScale` cannot be zero, so the Option is the honest return rather
    /// than a clamp to 1.
    #[test]
    fn a_degenerate_cell_has_no_scale_at_all() {
        assert_eq!(scale(CellSize { w: 0, h: 0 }, BASE_ONLY), None);
        assert_eq!(scale(CellSize { w: 8, h: 1 }, BASE_ONLY), None);
        assert_eq!(scale(CellSize { w: 0, h: 16 }, BASE_ONLY), None);
    }

    /// One outlier density must not switch the cutaway off where the pack's
    /// other art lands: the densest that lands wins, whatever order the
    /// densities come in.
    #[test]
    fn the_densest_density_that_lands_wins_not_the_densest_alone() {
        assert_eq!(scale(CELL_8X16, &[16, 8]), Some(8), "16x cannot land at 8");
        assert_eq!(scale(CELL_8X16, &[8, 16]), Some(8));
        assert_eq!(scale(CellSize { w: 16, h: 32 }, &[8, 16]), Some(16));
        assert_eq!(
            scale(CellSize { w: 20, h: 40 }, &[8, 16]),
            Some(16),
            "the densest wins"
        );
        assert_eq!(
            scale(CellSize { w: 5, h: 10 }, &[16, 8]),
            None,
            "nothing lands"
        );
    }

    #[test]
    fn off_beats_a_capable_terminal() {
        // The flag is the user's, not a hint — a capable terminal must not
        // override it.
        assert_eq!(
            resolve(GraphicsMode::Off, capable(CELL_8X16), BUNDLED),
            Plan::Classic {
                reason: ClassicReason::Disabled
            }
        );
    }

    /// Every protocol the probe can report reaches the plan as itself.
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
                    BUNDLED
                ),
                Plan::Cutaway {
                    scale: RenderScale::new(8).expect("nonzero"),
                    protocol,
                    cell: CELL_8X16,
                    tmux: false,
                }
            );
        }
    }

    /// Inside tmux, kitty alone survives — and carries the tmux fact on to
    /// the encoder; the rest fall back, naming the protocol that was refused.
    #[test]
    fn inside_tmux_only_kitty_takes_the_cutaway() {
        let plan = |p| {
            resolve(
                GraphicsMode::Auto,
                answered(Some(p), CELL_8X16, true),
                BUNDLED,
            )
        };
        assert!(matches!(
            plan(ImageProtocol::Kitty),
            Plan::Cutaway {
                protocol: ImageProtocol::Kitty,
                tmux: true,
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

    /// A tmux user on SIXEL with a cell too small for the art hears about the
    /// cell, not about a switch to kitty that would not help.
    #[test]
    fn a_cell_too_small_is_reported_before_tmux() {
        let tiny = CellSize { w: 5, h: 10 };
        assert_eq!(
            resolve(
                GraphicsMode::Auto,
                answered(Some(ImageProtocol::Sixel), tiny, true),
                BUNDLED
            ),
            Plan::Classic {
                reason: ClassicReason::CellTooSmall {
                    cell: tiny,
                    density: 8
                }
            }
        );
    }

    /// The fallback is the common path — most terminals have no protocol —
    /// so it must never be silent. A user asking "why is it not the pretty
    /// one" gets an answer in every shape.
    #[test]
    fn every_way_of_lacking_graphics_falls_back_with_a_reason() {
        let too_small = |cell, density| ClassicReason::CellTooSmall { cell, density };
        let cases = [
            (Probe::NotQueried, BASE_ONLY, ClassicReason::NotQueried),
            (Probe::NoAnswer, BASE_ONLY, ClassicReason::NoAnswer),
            (
                Probe::TmuxPassthroughOff,
                BASE_ONLY,
                ClassicReason::TmuxPassthroughOff,
            ),
            (Probe::Unsupported, BASE_ONLY, ClassicReason::Unsupported),
            (
                Probe::Detected(Detected {
                    protocol: Some(ImageProtocol::Kitty),
                    cell: None,
                    tmux: false,
                }),
                BASE_ONLY,
                ClassicReason::NoCellSize,
            ),
            (
                answered(None, CELL_8X16, false),
                BASE_ONLY,
                ClassicReason::NoProtocol,
            ),
            (
                capable(CellSize { w: 0, h: 0 }),
                BASE_ONLY,
                too_small(CellSize { w: 0, h: 0 }, 1),
            ),
            (
                capable(CellSize { w: 1, h: 2 }),
                BASE_ONLY,
                too_small(CellSize { w: 1, h: 2 }, 1),
            ),
            // 5 cannot reach 8x art within the fit's bound.
            (
                capable(CellSize { w: 5, h: 10 }),
                BUNDLED,
                too_small(CellSize { w: 5, h: 10 }, 8),
            ),
        ];
        for (probe, densities, want) in cases {
            assert_eq!(
                resolve(GraphicsMode::Auto, probe, densities),
                Plan::Classic { reason: want },
                "for {probe:?}"
            );
            assert!(!want.describe().is_empty());
        }
    }

    /// A cell the art cannot reach and a cell with no pixels to subdivide are
    /// different sentences: a bigger font fixes the first, the second needs a
    /// terminal that reports its pixels.
    #[test]
    fn a_cell_too_small_names_the_art_it_is_too_small_for() {
        let reason = |cell, density| ClassicReason::CellTooSmall { cell, density }.describe();
        let small_font = reason(CellSize { w: 5, h: 10 }, 8);
        assert!(small_font.contains("the pack's 8x art"), "{small_font}");
        let no_pixels = reason(CellSize { w: 1, h: 2 }, 8);
        assert!(no_pixels.ends_with("too small to subdivide"), "{no_pixels}");
    }

    #[test]
    fn the_doctor_row_names_the_protocol_and_never_leaves_a_fallback_unexplained() {
        let row = graphics_diagnostic_row(
            GraphicsMode::Auto,
            answered(Some(ImageProtocol::Sixel), CellSize { w: 17, h: 41 }, false),
            BUNDLED,
        );
        assert!(row.starts_with("graphics: sixel (17x41 cell)"), "{row}");
        assert!(row.contains("would render at 16x"), "{row}");
        // The row reports a CAPABILITY. Until the profile reaches a painter it
        // must not read as a prediction about `run`, which paints classic
        // whatever this says.
        assert!(row.contains("not yet wired to `run`"), "{row}");

        // Every classic row must carry its reason — a bare "classic" reads as
        // a verdict on the office.
        for (mode, probe) in [
            (GraphicsMode::Off, capable(CELL_8X16)),
            (GraphicsMode::Auto, Probe::NotQueried),
            (GraphicsMode::Auto, Probe::NoAnswer),
            (GraphicsMode::Auto, Probe::TmuxPassthroughOff),
            (GraphicsMode::Auto, Probe::Unsupported),
            (GraphicsMode::Auto, capable(CellSize { w: 0, h: 0 })),
        ] {
            let row = graphics_diagnostic_row(mode, probe, BASE_ONLY);
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
        assert_eq!(scale(CellSize { w: 17, h: 41 }, BUNDLED), Some(16));
    }

    /// 2 is the first scale that buys anything over the classic density, and
    /// the boundary is pinned from BOTH sides so a future `>=` typo cannot
    /// slip through.
    #[test]
    fn the_cutoff_is_where_the_image_path_starts_buying_something() {
        assert_eq!(
            resolve(
                GraphicsMode::Auto,
                capable(CellSize { w: 1, h: 2 }),
                BASE_ONLY
            ),
            Plan::Classic {
                reason: ClassicReason::CellTooSmall {
                    cell: CellSize { w: 1, h: 2 },
                    density: 1,
                }
            },
            "1px per unit buys nothing"
        );
        assert!(
            matches!(
                resolve(GraphicsMode::Auto, capable(CellSize { w: 2, h: 4 }), BASE_ONLY),
                Plan::Cutaway { scale, .. } if scale.get() == 2
            ),
            "2px per unit is the first density worth an encode"
        );
    }
}
