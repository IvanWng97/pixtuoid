//! Shared anti-aliased text rasterizer (JetBrains Mono) for the binary's pixel
//! surfaces — the floating window's name badges + wall board (`floating/`) and
//! the snapshot example's cell text + `--proof` panel.
//!
//! Kept BINARY-side on purpose: `pixtuoid-scene` (which also compiles to wasm for
//! the web hero) stays font-dep-free — no `ab_glyph`, no embedded TTF, no wasm
//! bundle bloat (the web hero renders text as a crisp DOM overlay instead of
//! baking it). `ab_glyph` + the vendored `fonts/JetBrainsMono-Regular.ttf`
//! already lived here for `--proof`; this promotes them to a real module both
//! `floating/` and the example share.
//!
//! TWO faces, one seam: JetBrains Mono is the face, but it has NO glyph for part
//! of the office's symbol vocabulary (`★ ◐ ⬢ ▮ ▯ ⏱ ↳` — the board's star CTA,
//! the waiting dot, the gateway chip, the tooltip meter/stopwatch…), so every
//! per-character operation resolves the char to a face first: JetBrains Mono if
//! it covers it, else the bundled `fonts/PixtuoidSymbols.ttf` (a renamed
//! symbol-block subset of JuliaMono — see `fonts/README.md`). Without the
//! fallback those characters rasterized as `.notdef` tofu boxes.
//!
//! Surface-agnostic: [`draw_text_at`] hands each lit pixel's coverage to a
//! `put(x, y, coverage)` closure, so every caller applies its own pixel-format
//! blend (`RgbImage`/`RgbaImage` in the snapshot example, `u32` XRGB in the
//! floating window).

use std::sync::LazyLock;

use ab_glyph::{point, Font, FontRef, GlyphId, PxScale, ScaleFont};

/// The bundled JetBrains Mono Regular (OFL 1.1) — the primary AA face all the
/// binary's pixel surfaces share. License text in `fonts/OFL.txt`.
const FONT_BYTES: &[u8] = include_bytes!("../fonts/JetBrainsMono-Regular.ttf");
/// The symbol fallback face (a renamed JuliaMono subset, OFL 1.1 — see
/// `fonts/README.md` + `fonts/OFL-JuliaMono.txt`).
const SYMBOLS_BYTES: &[u8] = include_bytes!("../fonts/PixtuoidSymbols.ttf");

static FONT: LazyLock<FontRef<'static>> = LazyLock::new(|| {
    FontRef::try_from_slice(FONT_BYTES).expect("bundled JetBrains Mono TTF must parse")
});
static SYMBOLS: LazyLock<FontRef<'static>> = LazyLock::new(|| {
    FontRef::try_from_slice(SYMBOLS_BYTES).expect("bundled Pixtuoid Symbols TTF must parse")
});

/// The face that renders `ch`: JetBrains Mono when it covers it, else the
/// symbol fallback. Falls back to JetBrains Mono's `.notdef` only when NEITHER
/// face covers the char (callers with a better fallback gate on [`has_glyph`]).
fn face_for(ch: char) -> &'static FontRef<'static> {
    if FONT.glyph_id(ch) != GlyphId(0) {
        &FONT
    } else if SYMBOLS.glyph_id(ch) != GlyphId(0) {
        &SYMBOLS
    } else {
        &FONT
    }
}

/// Linear per-channel coverage blend of `fg` over `bg` — THE one blend curve
/// every AA-text surface composites with (snapshot PNG `mix_rgb`, proof-panel
/// `blend_px`, floating `blend_xrgb` all wrap this per their pixel type), so a
/// future curve change (e.g. gamma-correct blending) lands once, never drifts.
/// `cov` is clamped here so callers don't each re-clamp.
pub fn blend_channel(bg: u8, fg: u8, cov: f32) -> u8 {
    let a = cov.clamp(0.0, 1.0);
    (bg as f32 + (fg as f32 - bg as f32) * a).round() as u8
}

/// Whether EITHER bundled face covers `ch` with a real glyph (not `.notdef`).
/// Callers with a non-text fallback (the snapshot cell rasterizer's centered
/// block) gate on this so an uncovered decorative symbol renders as the
/// fallback, never tofu.
pub fn has_glyph(ch: char) -> bool {
    FONT.glyph_id(ch) != GlyphId(0) || SYMBOLS.glyph_id(ch) != GlyphId(0)
}

/// Sum of the per-glyph pixel-scaled advances at size `px` (each char measured
/// in the face that renders it) — the width function for wrapping / right-flush.
pub fn text_width(s: &str, px: f32) -> i32 {
    s.chars()
        .map(|c| {
            let sf = face_for(c).as_scaled(PxScale::from(px));
            sf.h_advance(sf.glyph_id(c))
        })
        .sum::<f32>()
        .round() as i32
}

/// The primary face's line height (ascent − descent + line gap) at size `px` —
/// the row advance for stacking multiple text lines (the wall board's 3 rows).
pub fn line_height(px: f32) -> i32 {
    let sf = FONT.as_scaled(PxScale::from(px));
    (sf.ascent() - sf.descent() + sf.line_gap()).round() as i32
}

/// Rasterize `s` at pixel size `px`, top-left at `(x, top_y)`, calling
/// `put(px_x, px_y, coverage)` for every lit pixel (`coverage` ∈ [0,1] is the AA
/// grayscale strength). Each char renders in the face that covers it (fallback
/// chars advance by THEIR face's advance). The baseline is the PRIMARY face's
/// ascent, so mixed-face runs share one baseline. Backend-agnostic — the caller
/// composites into its own surface. Returns the total advance width (so a caller
/// placing a cursor / second run doesn't recompute via [`text_width`]).
pub fn draw_text_at(
    s: &str,
    x: i32,
    top_y: i32,
    px: f32,
    mut put: impl FnMut(i32, i32, f32),
) -> i32 {
    let scale = PxScale::from(px);
    let baseline_y = top_y as f32 + FONT.as_scaled(scale).ascent();
    let mut cursor_x = x as f32;
    for ch in s.chars() {
        let face = face_for(ch);
        let sf = face.as_scaled(scale);
        let gid = sf.glyph_id(ch);
        let glyph = gid.with_scale_and_position(scale, point(cursor_x, baseline_y));
        if let Some(outlined) = face.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            let (ox, oy) = (bounds.min.x.round() as i32, bounds.min.y.round() as i32);
            outlined.draw(|gx, gy, coverage| {
                put(ox + gx as i32, oy + gy as i32, coverage);
            });
        }
        cursor_x += sf.h_advance(gid);
    }
    (cursor_x - x as f32).round() as i32
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
        // More glyphs → wider; bigger size → wider. (Exact N× proportionality
        // isn't asserted — text_width rounds the summed f32 advance ONCE, so
        // round(4·adv) ≠ 4·round(adv) in general.)
        let one = text_width("M", 16.0);
        assert!(one > 0);
        assert!(text_width("MM", 16.0) > one);
        assert!(text_width("MMMM", 16.0) > text_width("MM", 16.0));
        assert!(text_width("M", 32.0) > one, "larger px advances wider");
        // Monospace sanity: 4 M's land within ±1px of 4× one (pure rounding slack).
        assert!((text_width("MMMM", 16.0) - one * 4).abs() <= 1);
    }

    #[test]
    fn draw_emits_partial_coverage_pixels_the_bitmap_font_cannot() {
        // The whole point of the AA path: glyph edges emit intermediate coverage,
        // not the all-or-nothing pixels an 8×8 bitmap font produces.
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
    fn office_symbol_vocabulary_is_fully_covered() {
        // Every non-ASCII glyph the TUI's text surfaces emit (labels, footer,
        // board, tooltip, dashboard) must resolve to a REAL glyph in one of the
        // two faces — a new vocabulary glyph that's covered by neither would
        // rasterize as the snapshot fallback block (and as tofu in floating).
        // HAND-MAINTAINED allowlist (no single machine-readable source of the
        // render vocabulary exists to derive from): adding a glyph to hud.rs /
        // board.rs / overlay.rs / tooltip.rs / dashboard.rs means ADDING IT
        // HERE — the test can only guard chars it knows about.
        for ch in [
            '●', '○', '◐', '◌', '▲', '▸', '▾', '★', '⬢', '▮', '▯', '⏱', '↳', '↑', '↓', '·', '×',
            '⚠', '…', '⋮', '─', '│', '█', '▀',
        ] {
            assert!(has_glyph(ch), "no bundled face covers {ch:?}");
        }
    }

    #[test]
    fn fallback_symbols_render_ink_not_tofu() {
        // ★ has no JetBrains Mono glyph — pre-fallback it drew the .notdef box.
        // Now it must render real ink via the symbols face.
        let mut lit = 0usize;
        draw_text_at("★", 0, 0, 16.0, |_x, _y, cov| {
            if cov > 0.5 {
                lit += 1;
            }
        });
        assert!(lit > 10, "the star renders substantial ink: {lit} px");
    }
}
