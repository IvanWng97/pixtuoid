//! Ordered dither: how a gradient is drawn on a pixel grid, the one threshold
//! matrix both painters step by. A falloff is a few flat tones with a dithered
//! seam between each pair, never a soft blend.

/// The 4x4 ordered (Bayer) threshold matrix.
///
/// Its evenly-spread levels are why a dither reads as a smooth ramp rather than
/// as noise or as banding: the classic pixel-art answer.
pub(crate) const BAYER_4X4: [[u8; 4]; 4] =
    [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
/// How many threshold levels [`BAYER_4X4`] spreads.
pub(crate) const BAYER_LEVELS: u32 = 16;

/// Whether the pixel at `(x, y)` takes the next tone of an ordered dither
/// covering `coverage` of its area.
pub(crate) fn takes_next(x: u16, y: u16, coverage: f32) -> bool {
    let level = (coverage.clamp(0.0, 1.0) * BAYER_LEVELS as f32) as u8;
    BAYER_4X4[usize::from(y % 4)][usize::from(x % 4)] < level
}

/// The share of a step, centred on the midpoint between two tones, that
/// checkers the two: a level anywhere else paints its nearer tone flat, so a
/// falloff reads as flat rings with a dithered seam, not as speckle.
const SEAM: f32 = 0.3;
/// Within this of a whole tone, a level IS that tone: f32 residue from the
/// ramp's division must not drop one cell in a flat ring a tone lower.
const TONE_SNAP: f32 = 1e-4;

/// `level` drawn at `(x, y)` as one of `tones` flat steps of `peak`: its
/// nearer step, or — within [`SEAM`] of the midpoint — a checker of the two.
pub(crate) fn stepped(level: f32, peak: f32, tones: u8, x: u16, y: u16) -> f32 {
    if peak <= 0.0 || level <= 0.0 {
        return 0.0;
    }
    let step = peak / f32::from(tones);
    let at = (level / step).min(f32::from(tones));
    let at = if (at - at.round()).abs() < TONE_SNAP {
        at.round()
    } else {
        at
    };
    let below = at.floor();
    let frac = at - below;
    let upper = if (frac - 0.5).abs() < SEAM / 2.0 {
        takes_next(x, y, 0.5)
    } else {
        frac > 0.5
    };
    (below + f32::from(u8::from(upper))).min(f32::from(tones)) * step
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ramp_steps_through_its_tones_and_nothing_between() {
        let peak = 0.6;
        for y in 0..8 {
            for x in 0..8 {
                for i in 0..=20 {
                    let level = peak * i as f32 / 20.0;
                    let got = stepped(level, peak, 3, x, y);
                    let tone = got / (peak / 3.0);
                    assert!(
                        (tone - tone.round()).abs() < 1e-4,
                        "{level} at ({x},{y}) gave {got}, between tones"
                    );
                    assert!((0.0..=peak + 1e-6).contains(&got));
                }
            }
        }
    }

    /// Flat rings: a level near a tone paints it across the whole tile; only
    /// the seam midway checkers the two.
    #[test]
    fn a_level_paints_its_nearer_tone_flat_and_checkers_only_the_seam() {
        let (peak, tones) = (0.9, 3);
        let step = peak / f32::from(tones);
        let tile = |level: f32| -> Vec<f32> {
            (0..4u16)
                .flat_map(|y| (0..4u16).map(move |x| stepped(level, peak, tones, x, y)))
                .collect()
        };
        assert!(
            tile(step * 1.2).iter().all(|&t| t == step),
            "near the first tone"
        );
        assert!(
            tile(step * 1.8).iter().all(|&t| t == step * 2.0),
            "near the second"
        );
        let seam = tile(step * 1.5);
        let upper = seam.iter().filter(|&&t| t == step * 2.0).count();
        assert_eq!(upper, 8, "midway checkers the two tones half and half");
        assert!(seam.iter().all(|&t| t == step || t == step * 2.0));
    }

    /// A level on a tone, however the ramp's division rounds it, paints that
    /// tone at every phase of the matrix.
    #[test]
    fn an_exact_tone_paints_flat_at_every_phase() {
        for (level, peak) in [(0.8 * 0.4 * (1.0 - 1.0 / 3.0), 0.8 * 0.4), (0.3, 0.9)] {
            let tones = 3;
            let want = stepped(level, peak, tones, 0, 0);
            for y in 0..4 {
                for x in 0..4 {
                    assert_eq!(
                        stepped(level, peak, tones, x, y),
                        want,
                        "{level} at ({x},{y})"
                    );
                }
            }
        }
    }
}
