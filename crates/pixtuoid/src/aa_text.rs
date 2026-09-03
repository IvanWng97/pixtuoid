//! Shared anti-aliased text rasterizer (Monaspace Neon) for the binary's pixel
//! surfaces — the floating window's name badges + wall board, and the examples'
//! cell text, `--proof` panel and cutaway.
//!
//! Kept BINARY-side on purpose: `pixtuoid-scene` also compiles to wasm for the
//! web hero, so it stays font-dep-free — no font parser, no embedded font, no
//! wasm bundle bloat.
//!
//! ONE face by DESIGN. Monaspace Neon natively covers the office's FULL symbol
//! vocabulary (`★ ◐ ⬢ ▮ ▯ ↳ ◷ ▤`), which JetBrains Mono does not — not even the
//! Nerd Font patch, whose glyphs live entirely in the Private Use Area. A new
//! render glyph MUST be Monaspace-covered, never a second face; the gate is
//! `office_symbol_vocabulary_is_fully_covered`.
//!
//! The committed stills are pinned to [`ab_glyph_rasterizer`]'s AA curve, so
//! only the PARSER moved to [`skrifa`] (`ab_glyph`'s pulled the unmaintained
//! ttf-parser, RUSTSEC-2026-0192, #440). A whole-stack move (`swash`, or
//! `skrifa` + `zeno`) would shift every edge and reshoot every still.
//!
//! Surface-agnostic: [`draw_text_at`] hands each lit pixel's coverage to a
//! `put(x, y, coverage)` closure, so every caller applies its own pixel-format
//! blend — all through [`blend_channel`], the one blend curve.

use std::sync::LazyLock;

use ab_glyph_rasterizer::{point, Point, Rasterizer};
use skrifa::charmap::Charmap;
use skrifa::instance::{LocationRef, Size};
use skrifa::metrics::GlyphMetrics;
use skrifa::outline::{DrawSettings, OutlineGlyphCollection, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

/// SemiBold is the weight picked by eye for these small-size pixel surfaces.
/// License text in `fonts/OFL-Monaspace.txt`.
const FONT_BYTES: &[u8] = include_bytes!("../fonts/MonaspaceNeon-SemiBold.otf");

/// The face's derived lookup structures, built once: [`MetadataProvider::charmap`]
/// re-runs cmap subtable SELECTION on every call, so one per character would cost
/// a table scan. Held unscaled — [`px_per_unit`] scales at the call site.
struct Face {
    charmap: Charmap<'static>,
    outlines: OutlineGlyphCollection<'static>,
    metrics: GlyphMetrics<'static>,
    /// Vertical metrics in font units: ascent, descent (negative), line gap.
    v_metrics: (f32, f32, f32),
}

static FACE: LazyLock<Face> = LazyLock::new(|| {
    // Every field borrows FONT_BYTES, not `font`, so the FontRef is local.
    let font = FontRef::new(FONT_BYTES).expect("bundled Monaspace Neon OTF must parse");
    let m = font.metrics(Size::unscaled(), LocationRef::default());
    Face {
        charmap: font.charmap(),
        outlines: font.outline_glyphs(),
        metrics: font.glyph_metrics(Size::unscaled(), LocationRef::default()),
        v_metrics: (m.ascent, m.descent, m.leading),
    }
});

/// Pixels per font unit at `px`, where `px` spans ascent−descent, NOT the em
/// square — the two differ for this face. Every call site's size constant was
/// picked against that convention, so it is converted here rather than re-tuned.
fn px_per_unit(px: f32) -> f32 {
    let (ascent, descent, _) = FACE.v_metrics;
    px / (ascent - descent)
}

/// Linear per-channel coverage blend of `fg` over `bg` — THE one blend curve
/// every AA-text surface composites with, so a future curve change (e.g.
/// gamma-correct blending) lands once. `cov` is clamped here so callers don't
/// each re-clamp.
pub fn blend_channel(bg: u8, fg: u8, cov: f32) -> u8 {
    let a = cov.clamp(0.0, 1.0);
    (bg as f32 + (fg as f32 - bg as f32) * a).round() as u8
}

/// The face's glyph for `ch`, falling back to `.notdef` on a cmap miss — so an
/// uncovered char still advances by the tofu box rather than collapsing the run.
fn glyph_id(ch: char) -> GlyphId {
    FACE.charmap.map(ch).unwrap_or(GlyphId::NOTDEF)
}

/// Whether the face covers `ch` with a real glyph (not `.notdef`). Callers with a
/// non-text fallback gate on this so an uncovered symbol renders as the fallback,
/// never tofu.
pub fn has_glyph(ch: char) -> bool {
    glyph_id(ch) != GlyphId::NOTDEF
}

/// Summing real advances, rather than `chars × one advance`, stays correct even
/// for a future proportional face.
pub fn text_width(s: &str, px: f32) -> i32 {
    let k = px_per_unit(px);
    s.chars()
        .map(|c| FACE.metrics.advance_width(glyph_id(c)).unwrap_or(0.0) * k)
        .sum::<f32>()
        .round() as i32
}

pub fn line_height(px: f32) -> i32 {
    let (ascent, descent, leading) = FACE.v_metrics;
    let k = px_per_unit(px);
    // `descent` is negative below the baseline, so subtracting it adds depth.
    ((ascent - descent + leading) * k).round() as i32
}

/// One outline segment in FONT UNITS, y-UP from the baseline.
enum Seg {
    Line(Point, Point),
    Quad(Point, Point, Point),
    Cubic(Point, Point, Point, Point),
}

/// Collects a glyph's outline and the control-point bounds of what it emitted.
///
/// Bounds and geometry come from ONE draw pass because [`Rasterizer`] is sized
/// at construction; skrifa's `ControlBoundsPen` would draw the glyph twice.
/// Control points can only widen the box, and a wider box is inert: a sample's
/// absolute pixel is `origin + rasterizer coord`, so both shift by one integer.
#[derive(Default)]
struct OutlineCollector {
    segs: Vec<Seg>,
    /// `None` until the first point — a glyph like the space has no outline.
    bounds: Option<(f32, f32, f32, f32)>,
    start: Point,
    last: Point,
}

impl OutlineCollector {
    fn see(&mut self, p: Point) {
        self.bounds = Some(match self.bounds {
            None => (p.x, p.y, p.x, p.y),
            Some((x0, y0, x1, y1)) => (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y)),
        });
    }
}

impl OutlinePen for OutlineCollector {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = point(x, y);
        self.see(p);
        self.start = p;
        self.last = p;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = point(x, y);
        self.see(p);
        self.segs.push(Seg::Line(self.last, p));
        self.last = p;
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let (c, p) = (point(cx, cy), point(x, y));
        self.see(c);
        self.see(p);
        self.segs.push(Seg::Quad(self.last, c, p));
        self.last = p;
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (c0, c1, p) = (point(cx0, cy0), point(cx1, cy1), point(x, y));
        self.see(c0);
        self.see(c1);
        self.see(p);
        self.segs.push(Seg::Cubic(self.last, c0, c1, p));
        self.last = p;
    }

    fn close(&mut self) {
        // The rasterizer accumulates winding along explicit edges, so the
        // contour's closing edge has to be one — skrifa leaves it implicit.
        if self.last != self.start {
            self.segs.push(Seg::Line(self.last, self.start));
        }
        self.last = self.start;
    }
}

/// Rasterize `s` in the AA face at pixel size `px`, top-left at `(x, top_y)`,
/// calling `put(px_x, px_y, coverage)` for every lit pixel (`coverage` ∈ `[0,1]`
/// is the AA grayscale strength). Returns the total advance width, so a caller
/// placing a second run needn't recompute it via [`text_width`].
pub fn draw_text_at(
    s: &str,
    x: i32,
    top_y: i32,
    px: f32,
    mut put: impl FnMut(i32, i32, f32),
) -> i32 {
    let k = px_per_unit(px);
    let (ascent, _, _) = FACE.v_metrics;
    let baseline_y = top_y as f32 + ascent * k;
    let mut cursor_x = x as f32;
    for ch in s.chars() {
        let gid = glyph_id(ch);
        if let Some(glyph) = FACE.outlines.get(gid) {
            let mut collector = OutlineCollector::default();
            // A malformed charstring yields no outline; the run must still
            // advance, so a draw error is skipped rather than propagated.
            if glyph
                .draw(
                    DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
                    &mut collector,
                )
                .is_ok()
            {
                draw_outline(&collector, k, cursor_x, baseline_y, &mut put);
            }
        }
        cursor_x += FACE.metrics.advance_width(gid).unwrap_or(0.0) * k;
    }
    (cursor_x - x as f32).round() as i32
}

/// Scale `collector`'s font-unit outline by `k`, flip it onto the device grid at
/// `(pen_x, baseline_y)`, and hand every covered pixel to `put` in absolute
/// coordinates.
fn draw_outline(
    collector: &OutlineCollector,
    k: f32,
    pen_x: f32,
    baseline_y: f32,
    put: &mut impl FnMut(i32, i32, f32),
) {
    let Some((min_x, min_y, max_x, max_y)) = collector.bounds else {
        return;
    };
    // Round the SCALED offset against the pen's fraction and re-add its integer
    // part, never `pen + offset` — that sum loses the offset's low bits once the
    // pen is large, moving a whole device pixel (`ab_glyph`'s `px_bounds` does
    // the same, for the same reason).
    let (x_trunc, x_fract) = (pen_x.trunc(), pen_x.fract());
    let (y_trunc, y_fract) = (baseline_y.trunc(), baseline_y.fract());
    // y flips here: the outline's max_y is the glyph's TOP, the smaller device row.
    let ox = (min_x * k + x_fract).floor() + x_trunc;
    let oy = (-(max_y * k) + y_fract).floor() + y_trunc;
    let w = ((max_x * k + x_fract).ceil() + x_trunc - ox) as usize;
    let h = ((-(min_y * k) + y_fract).ceil() + y_trunc - oy) as usize;
    if w == 0 || h == 0 {
        return;
    }
    let (off_x, off_y) = (pen_x - ox, baseline_y - oy);
    let dev = |p: &Point| point(p.x * k + off_x, off_y - p.y * k);
    let mut r = Rasterizer::new(w, h);
    for seg in &collector.segs {
        match seg {
            Seg::Line(p0, p1) => r.draw_line(dev(p0), dev(p1)),
            Seg::Quad(p0, p1, p2) => r.draw_quad(dev(p0), dev(p1), dev(p2)),
            Seg::Cubic(p0, p1, p2, p3) => r.draw_cubic(dev(p0), dev(p1), dev(p2), dev(p3)),
        }
    }
    let (ox, oy) = (ox as i32, oy as i32);
    r.for_each_pixel_2d(|gx, gy, coverage| put(ox + gx as i32, oy + gy as i32, coverage));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_parses_and_metrics_are_positive() {
        assert!(text_width("M", 16.0) > 0, "a glyph has positive advance");
        assert!(line_height(16.0) > 0, "positive line height");
    }

    #[test]
    fn width_grows_with_length_and_size() {
        // Exact N× proportionality is NOT asserted: text_width rounds the summed
        // f32 advance ONCE, so round(4·adv) ≠ 4·round(adv) in general.
        let one = text_width("M", 16.0);
        assert!(one > 0);
        assert!(text_width("MM", 16.0) > one);
        assert!(text_width("MMMM", 16.0) > text_width("MM", 16.0));
        assert!(text_width("M", 32.0) > one, "larger px advances wider");
        // ±1px is the rounding slack above, not a tolerance on the face's metrics.
        assert!((text_width("MMMM", 16.0) - one * 4).abs() <= 1);
    }

    #[test]
    fn draw_emits_partial_coverage_pixels_the_bitmap_font_cannot() {
        let mut lit = 0usize;
        let mut partial = 0usize;
        let advance = draw_text_at("a", 0, 0, 24.0, |_x, _y, cov| {
            assert!((0.0..=1.0).contains(&cov), "coverage in [0,1]: {cov}");
            lit += 1;
            if cov > 0.02 && cov < 0.98 {
                partial += 1;
            }
        });
        assert!(lit > 0, "the glyph lit some pixels");
        assert!(
            partial > 0,
            "AA glyph has anti-aliased (partial-coverage) edges"
        );
        assert!(advance > 0, "returns the advance width");
    }

    #[test]
    fn an_open_contour_is_closed_before_rasterizing() {
        // `B` has an open contour; `a` and `M` above do not, and widening this
        // to the vocabulary would false-alarm on `◷`'s overlapping ones.
        let mut max_cov = 0.0f32;
        draw_text_at("B", 0, 0, 24.0, |_x, _y, cov| max_cov = max_cov.max(cov));
        assert!(max_cov <= 1.0, "open contour leaked winding: {max_cov}");
    }

    #[test]
    fn blend_channel_endpoints_midpoint_and_clamp() {
        assert_eq!(blend_channel(0, 200, 0.0), 0);
        assert_eq!(blend_channel(0, 200, 1.0), 200);
        assert_eq!(blend_channel(0, 200, 0.5), 100);
        assert_eq!(blend_channel(0, 200, 2.0), 200, "over-coverage clamps");
        assert_eq!(blend_channel(0, 200, -1.0), 0, "negative clamps");
    }

    #[test]
    fn office_symbol_vocabulary_is_fully_covered() {
        // HAND-MAINTAINED allowlist — no machine-readable source of the render
        // vocabulary exists to derive from, so emitting a new non-ASCII glyph
        // ANYWHERE in the TUI means adding it here.
        for ch in [
            '●', '○', '◐', '◌', '▲', '▼', '▸', '▾', '★', '⬢', '▮', '▯', '↳', '◷', '▤', '↑', '↓',
            '·', '×', '⚠', '…', '⋮', '─', '│', '█', '▓', '▒', '░', '▀', '✓', '└', '├', 'Σ', '♩',
        ] {
            assert!(has_glyph(ch), "Monaspace Neon does not cover {ch:?}");
        }
    }
}
