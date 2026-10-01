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

use pixtuoid_core::sprite::format::Density;
use pixtuoid_scene::layout::Size;
use pixtuoid_scene::render_scale::RenderScale;
use ratatui::layout::Size as TermSize;

#[cfg(feature = "graphics")]
pub(crate) mod kitty;
#[cfg(feature = "graphics")]
mod probe;
#[cfg(feature = "graphics")]
pub(crate) mod tiles;

#[cfg(feature = "graphics")]
pub(crate) use probe::probe;
#[cfg(feature = "graphics")]
use tiles::TileShape;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    /// The cells one re-sent piece of the image covers.
    #[cfg(feature = "graphics")]
    pub(crate) fn tile(self) -> TileShape {
        match self {
            Self::Kitty => TileShape { cols: 4, rows: 2 },
            Self::Sixel | Self::Iterm2 => TileShape { cols: 8, rows: 4 },
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

/// What the user asked for: `--graphics`, else the `graphics` config key.
///
/// The one `pub` item here, re-exported from the crate root: it is a field of
/// the `pub` [`crate::cli::Cmd`] and `main.rs` is a separate crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum GraphicsMode {
    /// Use terminal graphics when the terminal supports them.
    Auto,
    /// Never use terminal graphics, however capable the terminal is.
    #[default]
    Off,
    /// kitty's graphics protocol, whatever the terminal answers.
    Kitty,
    /// SIXEL, whatever the terminal answers.
    Sixel,
    /// iTerm2's inline images, whatever the terminal answers.
    Iterm2,
}

impl GraphicsMode {
    /// The protocol the user named, which replaces the terminal's answer.
    fn forced(self) -> Option<ImageProtocol> {
        match self {
            Self::Auto | Self::Off => None,
            Self::Kitty => Some(ImageProtocol::Kitty),
            Self::Sixel => Some(ImageProtocol::Sixel),
            Self::Iterm2 => Some(ImageProtocol::Iterm2),
        }
    }
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
    /// pack's art lands on: a cell whose natural scale lies further than
    /// [`RenderScale::fit`]'s bound from every multiple of the pack's densest
    /// art, or a 1-px cell, where one pixel per logical unit IS the classic
    /// density.
    CellTooSmall {
        /// The cell the terminal reported.
        cell: CellSize,
        /// The pack's [`max_density_variant`](pixtuoid_core::sprite::format::Pack::max_density_variant).
        max_density: Density,
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
    /// The orthographic cutaway, handed to the terminal as an image.
    Cutaway {
        /// Its geometry on this terminal.
        fit: Fit,
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
    /// What the terminal's reply and the environment established — the
    /// environment alone where the reply never completed but the environment
    /// names a protocol, as upstream falls back (see [`probe()`]).
    #[cfg_attr(
        all(not(feature = "graphics"), not(test)),
        expect(dead_code, reason = "only the graphics probe returns it")
    )]
    Answered(Detected),
    /// The terminal was never asked: the caller said not to, or no controlling
    /// terminal took the query.
    NotQueried,
    /// The terminal was asked, its reply never completed, and the environment
    /// names no protocol to fall back on. Only the Unix probe can tell (see
    /// [`probe()`]).
    #[cfg_attr(
        all(not(all(feature = "graphics", unix)), not(test)),
        expect(dead_code, reason = "only the Unix graphics probe returns it")
    )]
    NoAnswer,
    /// Inside tmux with `allow-passthrough` off, so nothing was asked.
    #[cfg_attr(
        all(not(all(feature = "graphics", unix)), not(test)),
        expect(dead_code, reason = "only the Unix graphics probe returns it")
    )]
    TmuxPassthroughOff,
    /// This build cannot ask: it has no `graphics` feature, or, for `run`, no
    /// cancellable probe (see [`run_probe`]).
    #[cfg_attr(
        all(feature = "graphics", unix, not(test)),
        expect(
            dead_code,
            reason = "only the graphics-less or non-Unix probe returns it"
        )
    )]
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

/// The cutaway's geometry on one terminal: the office renders at the pack's
/// densest art, and the image is that render upscaled a whole number of times —
/// pixel-identical to a render at [`Fit::scale`] (pinned in `pixtuoid-scene` by
/// `the_cutaway_paints_whole_art_pixels`).
///
/// The densest art, not whichever density the cell lands nearest: the one
/// render draws every piece at one density, and a scale only a coarser density
/// divides would draw the densest art from coarser stand-ins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fit {
    scale: RenderScale,
    density: Density,
    render: RenderScale,
    logical: Size,
}

impl Fit {
    /// The terminal's half of [`RenderScale::fit`]: `cell`'s natural scale,
    /// fitted to `max_density` (the pack's
    /// [`max_density_variant`](pixtuoid_core::sprite::format::Pack::max_density_variant)),
    /// over an image `area` cells big. `None` when no multiple of it lies within
    /// the fit's bound.
    pub(crate) fn new(cell: CellSize, area: TermSize, max_density: Density) -> Option<Self> {
        let fit = Self {
            scale: RenderScale::fit(raw_scale_for_cell(cell), max_density)?,
            density: max_density,
            render: RenderScale::new(max_density.get())?,
            logical: Size { w: 0, h: 0 },
        };
        Some(fit.over(cell, area))
    }

    /// This fit over an image `area` cells big: the scale stays, and the office
    /// takes the area's shape.
    pub(crate) fn over(self, cell: CellSize, area: TermSize) -> Self {
        // The image anchors on cells, so its pixels are the cells', never a
        // window size that counts the terminal's padding; past what a buffer
        // can address, the office stops growing.
        let px = |cells: u16, cell_px: u16| {
            u16::try_from(u32::from(cells) * u32::from(cell_px)).unwrap_or(u16::MAX)
        };
        Self {
            logical: Size {
                w: self.scale.logical(px(area.width, cell.w)),
                h: self.scale.logical(px(area.height, cell.h)),
            },
            ..self
        }
    }

    /// Real pixels per logical office unit.
    pub(crate) fn scale(self) -> RenderScale {
        self.scale
    }

    /// The density the office renders at before the upscale.
    pub(crate) fn density(self) -> Density {
        self.density
    }

    /// [`Fit::density`] as the scale the office renders at.
    #[cfg(feature = "graphics")]
    pub(crate) fn render_scale(self) -> RenderScale {
        self.render
    }

    /// The whole factor the density render is upscaled by.
    pub(crate) fn upscale(self) -> u16 {
        self.scale.get() / self.density.get()
    }

    /// The office's extent in logical units: as many as the area's pixels hold
    /// on each axis, so the office takes the terminal's shape — no letterbox.
    pub(crate) fn logical(self) -> Size {
        self.logical
    }
}

/// Decide what to paint. Pure — [`probe()`] supplies the probe, `max_density`
/// is the pack's [`max_density_variant`](pixtuoid_core::sprite::format::Pack::max_density_variant),
/// and `area` is the image's extent in cells.
pub(crate) fn resolve(
    mode: GraphicsMode,
    probe: Probe,
    max_density: Density,
    area: TermSize,
) -> Plan {
    let classic = |reason| Plan::Classic { reason };
    if mode == GraphicsMode::Off {
        return classic(ClassicReason::Disabled);
    }
    let d = match probe {
        Probe::Answered(d) => d,
        Probe::NotQueried => return classic(ClassicReason::NotQueried),
        Probe::NoAnswer => return classic(ClassicReason::NoAnswer),
        Probe::TmuxPassthroughOff => return classic(ClassicReason::TmuxPassthroughOff),
        Probe::Unsupported => return classic(ClassicReason::Unsupported),
    };
    let Some(protocol) = mode.forced().or(d.protocol) else {
        return classic(ClassicReason::NoProtocol);
    };
    let Some(cell) = d.cell else {
        return classic(ClassicReason::NoCellSize);
    };
    // The cell before tmux: a user told to switch to kitty should not then
    // find the cell was too small all along.
    let fit = match Fit::new(cell, area, max_density) {
        // Scale 1 IS the classic density: an encode per frame that draws the
        // identical picture.
        Some(fit) if fit.scale().get() > 1 => fit,
        _ => return classic(ClassicReason::CellTooSmall { cell, max_density }),
    };
    if d.tmux && protocol != ImageProtocol::Kitty {
        return classic(ClassicReason::TmuxNeedsKitty(protocol));
    }
    Plan::Cutaway {
        fit,
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
            Self::CellTooSmall { cell, max_density }
                if max_density > Density::ONE && raw_scale_for_cell(cell) > 1 =>
            {
                format!(
                    "terminal reports a {}x{} cell — too small for the pack's {max_density}x art",
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

/// The plan for the terminal this process runs in: the one call `run` and
/// `doctor` both make, so `doctor` prints the plan `run` carries. `ask` is the
/// caller's probe; the area and the size read are shared.
pub(crate) fn plan_this_terminal(
    mode: GraphicsMode,
    max_density: Density,
    ask: impl FnOnce() -> Probe,
) -> Plan {
    detect(mode, max_density, terminal_cells(), ask)
}

/// The plan for a terminal `term` cells big. `ask` is the terminal query,
/// called only when `mode` can use its answer, so `--graphics off` never
/// touches the terminal.
fn detect(
    mode: GraphicsMode,
    max_density: Density,
    term: TermSize,
    ask: impl FnOnce() -> Probe,
) -> Plan {
    let probe = if mode == GraphicsMode::Off {
        Probe::NotQueried
    } else {
        ask()
    };
    resolve(mode, probe, max_density, image_area(term))
}

/// The cells the image covers: the scene, never the footer, since text and
/// image never share a cell.
fn image_area(term: TermSize) -> TermSize {
    let scene =
        crate::tui::renderer::scene_rect(ratatui::layout::Rect::new(0, 0, term.width, term.height));
    TermSize {
        width: scene.width,
        height: scene.height,
    }
}

/// The terminal's size in cells; empty when there is none to measure, where the
/// graphics probe is not asked either.
fn terminal_cells() -> TermSize {
    crossterm::terminal::size()
        .map(|(width, height)| TermSize { width, height })
        .unwrap_or_default()
}

/// `run`'s probe: [`probe()`], the cancellable one, on Unix.
#[cfg(all(feature = "graphics", unix))]
pub(crate) fn run_probe() -> Probe {
    probe(true)
}

/// Without the `graphics` feature there is no probe, and off Unix the only one
/// is upstream's, whose reader outlives its timeout and would steal the TUI's
/// keys (see [`probe()`]), so `run` does not ask.
#[cfg(not(all(feature = "graphics", unix)))]
pub(crate) fn run_probe() -> Probe {
    Probe::Unsupported
}

impl Plan {
    /// The plan as `doctor`'s `graphics:` line: the profile this terminal is
    /// CAPABLE of — "would render", since only `run --graphics` paints it —
    /// and why it falls back when it is not.
    pub(crate) fn diagnostic_row(self) -> String {
        match self {
            Plan::Cutaway {
                fit,
                protocol,
                cell,
                ..
            } => format!(
                "graphics: {} ({}x{} cell) — the cutaway profile would render at {}x \
             ({}x art upscaled {}x), a {}x{} office",
                protocol.name(),
                cell.w,
                cell.h,
                fit.scale().get(),
                fit.density(),
                fit.upscale(),
                fit.logical().w,
                fit.logical().h,
            ),
            Plan::Classic { reason } => {
                format!("graphics: classic half-blocks — {}", reason.describe())
            }
        }
    }
}

/// What the terminal unwind writes before it leaves the alt screen: our
/// images' delete, once any reached the terminal.
pub(crate) fn unwind_prelude() -> Vec<u8> {
    #[cfg(feature = "graphics")]
    return kitty::unwind();
    #[cfg(not(feature = "graphics"))]
    Vec::new()
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
    const BASE_ONLY: Density = Density::ONE;
    /// The bundled pack's densest art (`bundled_is_the_embedded_packs_max_density`).
    const BUNDLED: Density = Density::new(4).expect("nonzero");
    const AREA: TermSize = TermSize {
        width: 120,
        height: 40,
    };

    #[test]
    fn bundled_is_the_embedded_packs_max_density() {
        let pack =
            pixtuoid_scene::embedded_pack::load_bundled_pack().expect("the embedded pack loads");
        assert_eq!(pack.max_density_variant(), BUNDLED);
    }

    fn answered(protocol: Option<ImageProtocol>, cell: CellSize, tmux: bool) -> Probe {
        Probe::Answered(Detected {
            protocol,
            cell: Some(cell),
            tmux,
        })
    }

    fn capable(cell: CellSize) -> Probe {
        answered(Some(ImageProtocol::Kitty), cell, false)
    }

    fn d(n: u16) -> Density {
        Density::new(n).expect("nonzero")
    }

    fn cell(w: u16, h: u16) -> CellSize {
        CellSize { w, h }
    }

    fn scale(cell: CellSize, max_density: Density) -> Option<u16> {
        Fit::new(cell, AREA, max_density).map(|f| f.scale().get())
    }

    fn plan(probe: Probe, max_density: Density) -> Plan {
        resolve(GraphicsMode::Auto, probe, max_density, AREA)
    }

    fn row(mode: GraphicsMode, probe: Probe, max_density: Density) -> String {
        detect(mode, max_density, AREA, || probe).diagnostic_row()
    }

    fn too_small(cell: CellSize, max_density: Density) -> Plan {
        Plan::Classic {
            reason: ClassicReason::CellTooSmall { cell, max_density },
        }
    }

    /// The ~1:2 cell the whole half-block technique assumes: 8 wide, 16
    /// tall, so a logical unit is 8px either way and the office keeps its
    /// proportions exactly.
    #[test]
    fn a_standard_cell_yields_its_width_as_the_scale() {
        assert_eq!(scale(CELL_8X16, BASE_ONLY), Some(8));
        assert_eq!(scale(cell(10, 20), BASE_ONLY), Some(10));
    }

    #[test]
    fn a_non_standard_cell_never_stretches_a_logical_unit() {
        // Taller than 1:2 — the width is the binding constraint.
        assert_eq!(scale(cell(8, 24), BASE_ONLY), Some(8));
        // WIDER than 1:2 — height binds.
        assert_eq!(scale(cell(12, 16), BASE_ONLY), Some(8));
    }

    /// `RenderScale` cannot be zero, so the Option is the honest return rather
    /// than a clamp to 1.
    #[test]
    fn a_degenerate_cell_has_no_scale_at_all() {
        assert_eq!(scale(cell(0, 0), BASE_ONLY), None);
        assert_eq!(scale(cell(8, 1), BASE_ONLY), None);
        assert_eq!(scale(cell(0, 16), BASE_ONLY), None);
    }

    /// A cell whose natural scale IS a multiple of the densest art renders at
    /// it, upscaling the density render by exactly that multiple.
    #[test]
    fn a_cell_on_an_exact_multiple_renders_at_it() {
        let d = BUNDLED.get();
        for k in 1..=8 {
            let fit = Fit::new(cell(d * k, 2 * d * k), AREA, BUNDLED).expect("lands");
            assert_eq!(fit.scale().get(), d * k, "k={k}");
            assert_eq!(fit.density(), BUNDLED, "k={k}");
            assert_eq!(fit.upscale(), k, "k={k}");
        }
    }

    /// Only multiples of the densest art are candidates: a cell a coarser
    /// density would land on is still too small for the pack.
    #[test]
    fn only_multiples_of_the_densest_art_are_candidates() {
        assert_eq!(scale(CELL_8X16, d(16)), None, "8 is no multiple of 16");
        assert_eq!(scale(cell(16, 32), d(16)), Some(16));
        assert_eq!(scale(cell(20, 40), d(16)), Some(16));
        assert_eq!(scale(cell(17, 41), BUNDLED), Some(16));
        assert_eq!(
            scale(cell(6, 12), BUNDLED),
            Some(8),
            "6 lands nearer 8 by ratio"
        );
    }

    /// Below the densest art its own value is the one candidate, taken while the
    /// natural scale lies within √2 of it. The pair either side of the bound,
    /// derived from each density: no integer pair sits ON √2, which is
    /// irrational, so these two are the boundary.
    #[test]
    fn the_densest_art_is_reached_within_root_two_and_not_past_it() {
        for max in [BUNDLED, d(8), d(16), d(64)] {
            let dd = u32::from(max.get());
            let first = (1..=max.get())
                .find(|&n| 2 * u32::from(n) * u32::from(n) >= dd * dd)
                .expect("the density itself is within the bound");
            assert_ne!(2 * u32::from(first).pow(2), dd * dd, "√2 is never hit");
            let inside = cell(first, 2 * first);
            let outside = cell(first - 1, 2 * (first - 1));
            assert_eq!(scale(inside, max), Some(max.get()), "{max}x at {first}");
            assert_eq!(scale(outside, max), None, "{max}x at {}", first - 1);
            assert!(matches!(plan(capable(inside), max), Plan::Cutaway { .. }));
            assert_eq!(plan(capable(outside), max), too_small(outside, max));
        }
    }

    /// The office is sized from the area's pixels on each axis: as many
    /// logical units as fit, never a fixed aspect framed by bars.
    #[test]
    fn the_office_fills_the_terminal_without_a_letterbox() {
        let c = cell(17, 41);
        for (cols, rows) in [(120, 40), (80, 24), (240, 30), (40, 60)] {
            let fit = Fit::new(
                c,
                TermSize {
                    width: cols,
                    height: rows,
                },
                BUNDLED,
            )
            .expect("lands");
            let s = u32::from(fit.scale().get());
            for (logical, px) in [
                (fit.logical().w, u32::from(cols) * u32::from(c.w)),
                (fit.logical().h, u32::from(rows) * u32::from(c.h)),
            ] {
                let covered = u32::from(logical) * s;
                assert!(covered <= px && px < covered + s, "{cols}x{rows}: {fit:?}");
            }
        }
    }

    /// Past what a buffer can address the office stops growing rather than
    /// wrapping to a sliver.
    #[test]
    fn a_terminal_wider_than_a_buffer_gets_the_widest_office_a_buffer_holds() {
        let huge = TermSize {
            width: u16::MAX,
            height: 1,
        };
        let fit = Fit::new(cell(17, 41), huge, BUNDLED).expect("lands");
        assert_eq!(fit.logical().w, fit.scale().logical(u16::MAX));
    }

    #[test]
    fn off_beats_a_capable_terminal() {
        // The flag is the user's, not a hint — a capable terminal must not
        // override it.
        assert_eq!(
            resolve(GraphicsMode::Off, capable(CELL_8X16), BUNDLED, AREA),
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
            let got = plan(answered(Some(protocol), CELL_8X16, false), BUNDLED);
            let Plan::Cutaway {
                fit,
                protocol: p,
                cell,
                tmux,
            } = got
            else {
                panic!("{protocol:?}: {got:?}");
            };
            assert_eq!((p, cell, tmux), (protocol, CELL_8X16, false));
            assert_eq!((fit.scale().get(), fit.upscale()), (8, 2));
        }
    }

    /// Inside tmux, kitty alone survives — and carries the tmux fact on to
    /// the encoder; the rest fall back, naming the protocol that was refused.
    #[test]
    fn inside_tmux_only_kitty_takes_the_cutaway() {
        let plan = |p| plan(answered(Some(p), CELL_8X16, true), BUNDLED);
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

    /// A named protocol replaces the terminal's answer, whatever it was; every
    /// other fact — the cell, tmux, a terminal that never answered — still
    /// decides.
    #[test]
    fn a_named_protocol_overrides_the_answer_and_nothing_else() {
        for (mode, want) in [
            (GraphicsMode::Kitty, ImageProtocol::Kitty),
            (GraphicsMode::Sixel, ImageProtocol::Sixel),
            (GraphicsMode::Iterm2, ImageProtocol::Iterm2),
        ] {
            for answered_with in [None, Some(ImageProtocol::Kitty)] {
                let got = resolve(
                    mode,
                    answered(answered_with, CELL_8X16, false),
                    BUNDLED,
                    AREA,
                );
                assert!(
                    matches!(got, Plan::Cutaway { protocol, .. } if protocol == want),
                    "{mode:?} over {answered_with:?}: {got:?}"
                );
            }
            let tiny = cell(2, 4);
            assert_eq!(
                resolve(mode, capable(tiny), BUNDLED, AREA),
                too_small(tiny, BUNDLED)
            );
            assert_eq!(
                resolve(mode, Probe::NoAnswer, BUNDLED, AREA),
                Plan::Classic {
                    reason: ClassicReason::NoAnswer
                }
            );
        }
        assert_eq!(
            resolve(
                GraphicsMode::Sixel,
                answered(Some(ImageProtocol::Kitty), CELL_8X16, true),
                BUNDLED,
                AREA
            ),
            Plan::Classic {
                reason: ClassicReason::TmuxNeedsKitty(ImageProtocol::Sixel)
            }
        );
    }

    /// Off never reaches the terminal: the probe is the seam that would, and it
    /// panics if called.
    #[test]
    fn off_plans_without_asking_the_terminal() {
        assert_eq!(
            detect(GraphicsMode::Off, BUNDLED, AREA, || panic!(
                "Off must not query the terminal"
            )),
            Plan::Classic {
                reason: ClassicReason::Disabled
            }
        );
    }

    /// Every other mode asks exactly once and plans from the answer.
    #[test]
    fn every_other_mode_asks_once_and_plans_from_the_answer() {
        for mode in [
            GraphicsMode::Auto,
            GraphicsMode::Kitty,
            GraphicsMode::Sixel,
            GraphicsMode::Iterm2,
        ] {
            let mut asked = 0;
            let got = detect(mode, BUNDLED, AREA, || {
                asked += 1;
                capable(CELL_8X16)
            });
            assert_eq!(asked, 1, "{mode:?}");
            assert_eq!(
                got,
                resolve(mode, capable(CELL_8X16), BUNDLED, image_area(AREA))
            );
        }
    }

    /// The image covers the scene and never the footer: text and image never
    /// share a cell.
    #[test]
    fn the_image_area_leaves_the_footer_to_text() {
        let area = image_area(AREA);
        assert_eq!(area.width, AREA.width);
        assert_eq!(
            area.height,
            crate::tui::renderer::scene_rect(ratatui::layout::Rect::new(
                0,
                0,
                AREA.width,
                AREA.height
            ))
            .height
        );
        assert!(area.height < AREA.height);
    }

    /// A tmux user on SIXEL with a cell too small for the art hears about the
    /// cell, not about a switch to kitty that would not help.
    #[test]
    fn a_cell_too_small_is_reported_before_tmux() {
        let tiny = cell(2, 4);
        assert_eq!(
            plan(answered(Some(ImageProtocol::Sixel), tiny, true), BUNDLED),
            too_small(tiny, BUNDLED)
        );
    }

    /// The fallback is the common path — most terminals have no protocol —
    /// so it must never be silent. A user asking "why is it not the pretty
    /// one" gets an answer in every shape.
    #[test]
    fn every_way_of_lacking_graphics_falls_back_with_a_reason() {
        let small = |cell, max_density| ClassicReason::CellTooSmall { cell, max_density };
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
                Probe::Answered(Detected {
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
            (capable(cell(0, 0)), BASE_ONLY, small(cell(0, 0), BASE_ONLY)),
            (capable(cell(1, 2)), BASE_ONLY, small(cell(1, 2), BASE_ONLY)),
            (capable(cell(2, 4)), BUNDLED, small(cell(2, 4), BUNDLED)),
        ];
        for (probe, max_density, want) in cases {
            assert_eq!(
                plan(probe, max_density),
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
        let reason =
            |cell, max_density| ClassicReason::CellTooSmall { cell, max_density }.describe();
        let small_font = reason(cell(2, 4), BUNDLED);
        assert!(small_font.contains("the pack's 4x art"), "{small_font}");
        let no_pixels = reason(cell(1, 2), BUNDLED);
        assert!(no_pixels.ends_with("too small to subdivide"), "{no_pixels}");
    }

    #[test]
    fn the_doctor_row_names_the_protocol_and_the_fit() {
        let row = row(
            GraphicsMode::Auto,
            answered(Some(ImageProtocol::Sixel), cell(17, 41), false),
            BUNDLED,
        );
        assert_eq!(
            row,
            "graphics: sixel (17x41 cell) — the cutaway profile would render at 16x \
             (4x art upscaled 4x), a 127x99 office"
        );
    }

    /// A reason keeps its variant only by printing its own row, a remedy the
    /// user can act on: each is pinned whole, reached through [`resolve`], and
    /// no two may match. The exhaustive `match` fails to compile on a new reason
    /// until it gets a row here.
    #[test]
    fn every_classic_reason_prints_its_own_row() {
        let no_protocol = if cfg!(unix) {
            "terminal reports no graphics protocol (kitty/iterm2/sixel)"
        } else {
            "terminal reports no graphics protocol (kitty/iterm2/sixel), or did not answer \
             the capability query"
        };
        let cases = [
            (
                GraphicsMode::Off,
                capable(CELL_8X16),
                BUNDLED,
                "disabled by --graphics off",
            ),
            (
                GraphicsMode::Auto,
                Probe::Unsupported,
                BUNDLED,
                "this build has no terminal-graphics support",
            ),
            (
                GraphicsMode::Auto,
                Probe::NotQueried,
                BUNDLED,
                "the terminal was not asked (stdout is not a terminal, there is no controlling \
                 terminal, or $TERM is dumb) — run in an interactive terminal to see what it \
                 supports",
            ),
            (
                GraphicsMode::Auto,
                Probe::NoAnswer,
                BUNDLED,
                "the terminal did not answer the capability query",
            ),
            (
                GraphicsMode::Auto,
                answered(None, CELL_8X16, false),
                BUNDLED,
                no_protocol,
            ),
            (
                GraphicsMode::Auto,
                Probe::TmuxPassthroughOff,
                BUNDLED,
                "inside tmux with allow-passthrough off — `set -g allow-passthrough on` lets \
                 kitty graphics through",
            ),
            (
                GraphicsMode::Auto,
                Probe::Answered(Detected {
                    protocol: Some(ImageProtocol::Kitty),
                    cell: None,
                    tmux: false,
                }),
                BUNDLED,
                "terminal reports no cell size in pixels",
            ),
            (
                GraphicsMode::Auto,
                answered(Some(ImageProtocol::Iterm2), CELL_8X16, true),
                BUNDLED,
                "inside tmux only kitty graphics survive a pane switch here, and this terminal \
                 speaks iterm2",
            ),
            (
                GraphicsMode::Auto,
                capable(cell(2, 4)),
                BUNDLED,
                "terminal reports a 2x4 cell — too small for the pack's 4x art",
            ),
            (
                GraphicsMode::Auto,
                capable(cell(1, 2)),
                BUNDLED,
                "terminal reports a 1x2 cell — too small to subdivide",
            ),
        ];
        let n = cases.len();
        let (mut seen, mut rows) = (
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
        );
        for (mode, probe, max_density, want) in cases {
            let Plan::Classic { reason } = resolve(mode, probe, max_density, AREA) else {
                panic!("{probe:?} must fall back");
            };
            seen.insert(match reason {
                ClassicReason::Disabled => 0,
                ClassicReason::Unsupported => 1,
                ClassicReason::NotQueried => 2,
                ClassicReason::NoAnswer => 3,
                ClassicReason::NoProtocol => 4,
                ClassicReason::TmuxPassthroughOff => 5,
                ClassicReason::NoCellSize => 6,
                ClassicReason::TmuxNeedsKitty(_) => 7,
                ClassicReason::CellTooSmall { .. } => 8,
            });
            let row = row(mode, probe, max_density);
            assert_eq!(row, format!("graphics: classic half-blocks — {want}"));
            rows.insert(row);
        }
        assert_eq!(seen.len(), 9, "every reason has a pinned row");
        assert_eq!(rows.len(), n, "no two reasons print the same row");
    }

    /// A real Retina Ghostty reports a 17x41 cell: 17 is what the cell alone
    /// allows, and the pack's 4x art moves it to the nearest multiple.
    #[test]
    fn the_cell_gives_the_natural_scale_and_the_pack_fits_it() {
        assert_eq!(raw_scale_for_cell(cell(17, 41)), 17);
        assert_eq!(scale(cell(17, 41), BUNDLED), Some(16));
    }

    /// 2 is the first scale that buys anything over the classic density, and
    /// the boundary is pinned from BOTH sides so a future `>=` typo cannot
    /// slip through.
    #[test]
    fn the_cutoff_is_where_the_image_path_starts_buying_something() {
        assert_eq!(
            plan(capable(cell(1, 2)), BASE_ONLY),
            too_small(cell(1, 2), BASE_ONLY),
            "1px per unit buys nothing"
        );
        assert!(
            matches!(
                plan(capable(cell(2, 4)), BASE_ONLY),
                Plan::Cutaway { fit, .. } if fit.scale().get() == 2
            ),
            "2px per unit is the first density worth an encode"
        );
    }
}
