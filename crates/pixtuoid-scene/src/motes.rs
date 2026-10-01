//! The dust motes drifting through each window's sunbeam, pixel-free: where
//! each one is and how much of it shows. A painter draws each its own way.

use crate::layout::Layout;

/// One window's sunbeam, the column its motes drift down.
pub(crate) struct SunbeamColumn {
    pub(crate) x: u16,
    pub(crate) top_y: u16,
    pub(crate) depth: u16,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DustMote {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) alpha: f32,
}

const MOTES_PER_COLUMN: usize = 3;

/// How much of a mote shows in a full beam, at the height of its fall.
pub(crate) const MOTE_PEAK: f32 = 0.7;

/// How much of the motes `look`'s sky lets show: they scatter the direct beam
/// ([`SkyTones::beam`](crate::atmosphere::SkyTones::beam)), and the daylight
/// ramp brings them up.
pub(crate) fn visibility(look: &crate::atmosphere::SkyTones) -> f32 {
    (look.sunlight * look.beam).max(0.0)
}

/// Deterministic per `(floor_seed, particle_id, beat)`: sine drift in x, slow
/// fall in y, alpha fading in the top/bottom 15% bands so motes don't pop
/// on/off at the spill boundary.
pub(crate) fn dust_mote_positions(
    floor_seed: u64,
    beat: crate::anim::Beat,
    col: &SunbeamColumn,
) -> Vec<DustMote> {
    let t_ms = beat.ms();
    let mut out = Vec::with_capacity(MOTES_PER_COLUMN);
    for i in 0..MOTES_PER_COLUMN {
        // Mix floor_seed, column x, and particle id so every (column, mote) pair
        // gets an independent 64-bit seed: a plain `floor_seed * K + i` varies
        // only the lowest bits, collapsing a column's `MOTES_PER_COLUMN` motes into
        // one pixel.
        let s = pixtuoid_core::id::splitmix64(
            floor_seed
                .wrapping_add((col.x as u64).wrapping_mul(pixtuoid_core::id::SPLITMIX64_M1))
                .wrapping_add((i as u64).wrapping_mul(pixtuoid_core::id::SPLITMIX64_M2)),
        );
        let phase = (s % 6283) as f32 / 1000.0;
        let speed_y = 0.6 + ((s >> 12) & 0x3) as f32 * 0.2;
        let speed_x = 0.4 + ((s >> 14) & 0x3) as f32 * 0.15;
        let cycle = col.depth as f32;
        // Keep the time term in f64: `t_ms as f32` would round to the nearest
        // f32 (ULP ~131 s at epoch magnitude) and freeze the drift for ~2 min.
        let ts = t_ms as f64 / 1000.0;
        let y_offset = ((ts * speed_y as f64 + ((s >> 4) & 0xFF) as f64) % cycle as f64) as f32;
        let y = col.top_y + y_offset as u16;
        let sx = (phase as f64 + ts * speed_x as f64).sin() as f32;
        // Clamp before casting — a negative f32 silently wraps to 0 via
        // `as u16`, dragging motes to the left buffer edge when col.x is small.
        let raw_x = (col.x as f32 + sx * 2.5).round();
        let x = raw_x.max(0.0).min(u16::MAX as f32) as u16;
        let norm = y_offset / cycle.max(1.0);
        let alpha = if norm < 0.15 {
            norm / 0.15
        } else if norm > 0.85 {
            (1.0 - norm) / 0.15
        } else {
            1.0
        };
        out.push(DustMote { x, y, alpha });
    }
    out
}

/// Returns one `SunbeamColumn` per painted window, centred on the pane and
/// starting at the ground's first row, so the motes drift through the window's
/// [`Light::Spill`](crate::lighting::Light::Spill).
pub(crate) fn window_spill_columns(layout: &Layout) -> Vec<SunbeamColumn> {
    let top_wall_h = layout.wall_band_h();
    layout
        .window_bays()
        .map(|w| SunbeamColumn {
            x: w.center_x(),
            top_y: top_wall_h,
            depth: crate::lighting::SPILL_DEPTH,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::Beat;

    #[test]
    fn dust_mote_positions_deterministic_per_seed() {
        let now = Beat::at_ms((12 * 3600 + 5) * 1000);
        let col = SunbeamColumn {
            x: 100,
            top_y: 12,
            depth: 12,
        };
        let a = dust_mote_positions(42, now, &col);
        let b = dust_mote_positions(42, now, &col);
        assert_eq!(a, b, "same seed + time → same positions");
        assert_eq!(a.len(), MOTES_PER_COLUMN);
    }

    #[test]
    fn dust_motes_drift_over_time() {
        let now1 = Beat::at_ms(12 * 3600 * 1000);
        let now2 = Beat::at_ms(12 * 3600 * 1000 + 500);
        let col = SunbeamColumn {
            x: 100,
            top_y: 12,
            depth: 12,
        };
        let a = dust_mote_positions(7, now1, &col);
        let b = dust_mote_positions(7, now2, &col);
        assert_ne!(a, b, "positions should advance over time");
    }

    #[test]
    fn dust_motes_drift_at_wall_clock_scale() {
        let now1 = Beat::at_ms(1_752_000_000_000);
        let now2 = Beat::at_ms(1_752_000_000_500);
        let col = SunbeamColumn {
            x: 100,
            top_y: 12,
            depth: 12,
        };
        let a = dust_mote_positions(7, now1, &col);
        let b = dust_mote_positions(7, now2, &col);
        assert_ne!(a, b, "positions should advance over time at wall-clock ms");
    }

    #[test]
    fn dust_motes_alpha_fades_at_edges() {
        let col = SunbeamColumn {
            x: 100,
            top_y: 12,
            depth: 20,
        };
        let mut saw_partial = false;
        'outer: for ms in 0..5000u64 {
            let now = Beat::at_ms(ms * 50);
            for DustMote { alpha, .. } in dust_mote_positions(123, now, &col) {
                if alpha < 0.5 {
                    saw_partial = true;
                    break 'outer;
                }
            }
        }
        assert!(
            saw_partial,
            "expected at least one frame where a mote is in its fade band"
        );
    }
}
