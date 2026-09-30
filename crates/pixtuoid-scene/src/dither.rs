//! Ordered dither: how a gradient is drawn on a pixel grid, the one threshold
//! matrix both painters step by, never a soft blend. A ramp of several tones is
//! flat bands with a dithered seam ([`step`]); a gradient between two is
//! dithered across its span ([`takes_next`]).

/// The 4x4 ordered (Bayer) threshold matrix.
///
/// Its evenly-spread levels are why a dither reads as a smooth ramp rather than
/// as noise or as banding: the classic pixel-art answer.
const BAYER_4X4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
/// How many threshold levels [`BAYER_4X4`] spreads.
const BAYER_LEVELS: u32 = 16;

/// Whether the pixel at `(x, y)` takes the next tone of an ordered dither
/// covering `coverage` of its area.
pub(crate) fn takes_next(x: u16, y: u16, coverage: f32) -> bool {
    let level = (coverage.clamp(0.0, 1.0) * BAYER_LEVELS as f32) as u8;
    BAYER_4X4[usize::from(y % 4)][usize::from(x % 4)] < level
}

/// The share of each band, at its top, that dithers into the next: a whole band
/// of dither reads as grain.
pub(crate) const SEAM: f32 = 0.3;

/// The whole tone a ramp `stops` along paints at `(x, y)`: solid through each
/// band, dithered into the next only across its top [`SEAM`].
pub(crate) fn step(stops: f32, x: u16, y: u16) -> u8 {
    let whole = stops.max(0.0).floor();
    let into_seam = (stops - whole - (1.0 - SEAM)) / SEAM;
    whole as u8 + u8::from(into_seam > 0.0 && takes_next(x, y, into_seam))
}

/// [`step`], rounding to the nearer tone: the [`SEAM`] sits just below the
/// midpoint between two, so a falloff keeps the reach its level gives it, where
/// flooring would shave its faint outer ring off.
pub(crate) fn nearest(stops: f32, x: u16, y: u16) -> u8 {
    step(stops + 0.5, x, y)
}

/// `level` drawn at `(x, y)` as one of `tones` flat steps of `peak`, by
/// [`nearest`].
pub(crate) fn stepped(level: f32, peak: f32, tones: u8, x: u16, y: u16) -> f32 {
    if peak <= 0.0 || level <= 0.0 {
        return 0.0;
    }
    let tone = peak / f32::from(tones);
    f32::from(nearest(level / tone, x, y).min(tones)) * tone
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
    /// the seam below the midpoint mixes in the next.
    #[test]
    fn a_level_paints_its_nearer_tone_flat_and_dithers_only_the_seam() {
        let (peak, tones) = (0.9, 3);
        let tone = peak / f32::from(tones);
        let tile = |level: f32| -> Vec<f32> {
            (0..4u16)
                .flat_map(|y| (0..4u16).map(move |x| stepped(level, peak, tones, x, y)))
                .collect()
        };
        let seam_start = 1.5 - SEAM;
        assert!(
            tile(tone * (seam_start - 0.05)).iter().all(|&t| t == tone),
            "below the seam"
        );
        assert!(
            tile(tone * 1.55).iter().all(|&t| t == tone * 2.0),
            "past the midpoint"
        );
        let seam = tile(tone * (seam_start + SEAM / 2.0));
        let upper = seam.iter().filter(|&&t| t == tone * 2.0).count();
        assert_eq!(upper, 8, "mid-seam dithers the two tones half and half");
        assert!(seam.iter().all(|&t| t == tone || t == tone * 2.0));
    }

    /// A band is one tone below its [`SEAM`], mixed with the next across it.
    #[test]
    fn a_band_is_solid_but_for_its_seam() {
        for band in 0..4u8 {
            for tenth in 0..10 {
                let stops = f32::from(band) + tenth as f32 / 10.0;
                let steps: std::collections::BTreeSet<u8> = (0..8)
                    .flat_map(|y| (0..8).map(move |x| step(stops, x, y)))
                    .collect();
                if (tenth as f32 / 10.0) < 1.0 - SEAM {
                    assert_eq!(steps, [band].into(), "{stops} is dithered off its seam");
                } else {
                    assert!(
                        steps.is_subset(&[band, band + 1].into()),
                        "{stops}: {steps:?}"
                    );
                }
            }
        }
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
