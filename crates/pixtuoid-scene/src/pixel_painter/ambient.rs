//! Ambient pass — non-character, non-furniture effects painted between
//! the background and the y-sorted drawables: dust motes in window spill,
//! ceiling halos above active monitors.

use std::time::SystemTime;

use pixtuoid_core::sprite::RgbBuffer;

use crate::atmosphere::Moment;
use crate::layout::SceneLayout;
use crate::lighting::{Emitter, EmitterKind};
use crate::pixel_painter::PaintCtx;
use crate::pixel_painter::background::{paint_light, window_spill_columns};
use crate::pixel_painter::palette::blend_pixel;
use crate::theme::Theme;

pub(super) struct SunbeamColumn {
    pub x: u16,
    pub top_y: u16,
    pub depth: u16,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct DustMote {
    pub x: u16,
    pub y: u16,
    pub alpha: f32,
}

const MOTES_PER_COLUMN: usize = 3;

/// Deterministic per `(floor_seed, particle_id, now)`: sine drift in x, slow
/// fall in y, alpha fading in the top/bottom 15% bands so motes don't pop
/// on/off at the spill boundary.
pub(super) fn dust_mote_positions(
    floor_seed: u64,
    now: SystemTime,
    col: &SunbeamColumn,
) -> Vec<DustMote> {
    let t_ms = super::epoch_ms(now);
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

pub(super) fn paint_ambient(ctx: &mut PaintCtx<'_>, moment: &Moment, monitor_halos: &[Emitter]) {
    paint_dust_motes(ctx.buf, ctx.theme, ctx.layout, ctx.floor.floor_seed, moment);
    paint_ceiling_halos(ctx.buf, ctx.theme, monitor_halos);
}

/// Each halo over a lit monitor, tinted by its tool. Dark themes only — on a
/// light theme the warm tint reads as grime.
pub(super) fn paint_ceiling_halos(buf: &mut RgbBuffer, theme: &Theme, halos: &[Emitter]) {
    use crate::theme::ThemeKind;
    if theme.kind != ThemeKind::Dark {
        return;
    }
    for halo in halos {
        if let EmitterKind::MonitorHalo(tool) = halo.kind {
            let color = theme.tool_glow.for_kind(tool);
            paint_light(buf, halo, color);
        }
    }
}

/// Drift 1-pixel warm specks through each window's sunbeam spill column.
pub(super) fn paint_dust_motes(
    buf: &mut RgbBuffer,
    theme: &Theme,
    layout: &SceneLayout,
    floor_seed: u64,
    moment: &Moment,
) {
    let look = &moment.look;
    // Motes scatter the DIRECT beam, so density rides [`SkyTones::beam`](crate::atmosphere::SkyTones::beam) (full
    // under clear sky, faint through haze/snow-glare, zero under thick
    // overcast/rain); `look.sunlight` adds the daylight ramp.
    if look.beam <= 0.0 {
        return;
    }
    let visibility = look.sunlight * look.beam;
    if visibility <= 0.0 {
        return;
    }
    let warm = theme.lighting.sun_spill;
    for col in window_spill_columns(layout) {
        for DustMote { x, y, alpha } in dust_mote_positions(floor_seed, moment.now, &col) {
            let strength = alpha * 0.7 * visibility;
            blend_pixel(buf, x, y, warm, strength);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sky::{Sky, Weather};
    use pixtuoid_core::sprite::Rgb;
    use std::time::Duration;

    #[test]
    fn dust_mote_positions_deterministic_per_seed() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(12 * 3600 + 5);
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
        let now1 = SystemTime::UNIX_EPOCH + Duration::from_secs(12 * 3600);
        let now2 = now1 + Duration::from_millis(500);
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
        let now1 = SystemTime::UNIX_EPOCH + Duration::from_millis(1_752_000_000_000);
        let now2 = now1 + Duration::from_millis(500);
        let col = SunbeamColumn {
            x: 100,
            top_y: 12,
            depth: 12,
        };
        let a = dust_mote_positions(7, now1, &col);
        let b = dust_mote_positions(7, now2, &col);
        assert_ne!(a, b, "positions should advance over time at wall-clock ms");
    }

    /// One monitor halo centred at `(x, y)`, over an edit.
    fn one_halo(x: u16, y: u16) -> Vec<Emitter> {
        vec![Emitter {
            kind: EmitterKind::MonitorHalo(pixtuoid_core::state::ToolKind::Edit),
            light: crate::lighting::Light::Patch {
                centre: crate::layout::Point { x, y },
            },
            strength: 0.8,
        }]
    }

    #[test]
    fn ceiling_halo_painted_on_dark_theme() {
        let mut buf = RgbBuffer::filled(160, 90, Rgb { r: 0, g: 0, b: 0 });
        let theme = &crate::theme::CYBERPUNK;
        let halos = one_halo(50, 10);
        let baseline = buf.get(50, 10);
        paint_ceiling_halos(&mut buf, theme, &halos);
        assert_ne!(baseline, buf.get(50, 10), "halo should brighten the pixel");
    }

    #[test]
    fn ceiling_halo_skipped_on_light_theme() {
        let mut buf = RgbBuffer::filled(160, 90, Rgb { r: 0, g: 0, b: 0 });
        let theme = &crate::theme::NORMAL;
        let halos = one_halo(50, 10);
        let baseline = buf.get(50, 10);
        paint_ceiling_halos(&mut buf, theme, &halos);
        assert_eq!(baseline, buf.get(50, 10), "no halo on light themes");
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
            let now = SystemTime::UNIX_EPOCH + Duration::from_millis(ms * 50);
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

    #[test]
    fn ceiling_halo_near_edge_does_not_panic() {
        let mut buf = RgbBuffer::filled(6, 4, Rgb { r: 0, g: 0, b: 0 });
        let theme = &crate::theme::CYBERPUNK; // Dark theme so halos paint.
        let halos = one_halo(5, 0);
        paint_ceiling_halos(&mut buf, theme, &halos);
    }

    #[test]
    fn dust_motes_clamp_to_a_tiny_buffer() {
        let theme = &crate::theme::NORMAL;
        let layout = crate::layout::SceneLayout::compute(192, 80, Some(4)).expect("layout fits");
        // 07:00 Clear morning → sun up + full beam.
        let now = (1..=60u32)
            .map(|day| crate::localclock::on_day(day, 7))
            .find(|t| Sky::clock(*t).weather() == Weather::Clear)
            .expect("a clear morning");
        // No assertion: the test is that the clamped, out-of-bounds puts on a
        // buffer far smaller than the layout's spill columns don't panic.
        let fill = Rgb { r: 0, g: 0, b: 0 };
        let mut buf = RgbBuffer::filled(1, 1, fill);
        paint_dust_motes(
            &mut buf,
            theme,
            &layout,
            7,
            &Moment::resolve(Sky::clock(now), theme, 0.0, now),
        );
    }
}
