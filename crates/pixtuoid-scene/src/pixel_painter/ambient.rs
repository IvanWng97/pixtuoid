//! Ambient pass — non-character, non-furniture effects painted between
//! the background and the y-sorted drawables: dust motes in window spill,
//! ceiling halos above active monitors.

use pixtuoid_core::sprite::RgbBuffer;

use crate::atmosphere::Moment;
use crate::layout::Layout;
use crate::lighting::{Emitter, EmitterKind};
use crate::motes::{DustMote, dust_mote_positions, window_spill_columns};
use crate::pixel_painter::PaintCtx;
use crate::pixel_painter::background::paint_light;
use crate::pixel_painter::palette::blend_pixel;
use crate::theme::Theme;

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
    layout: &Layout,
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
            let strength = alpha * crate::motes::MOTE_PEAK * visibility;
            blend_pixel(buf, x, y, warm, strength);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sky::{Sky, Weather};
    use pixtuoid_core::sprite::Rgb;

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
    fn ceiling_halo_near_edge_does_not_panic() {
        let mut buf = RgbBuffer::filled(6, 4, Rgb { r: 0, g: 0, b: 0 });
        let theme = &crate::theme::CYBERPUNK; // Dark theme so halos paint.
        let halos = one_halo(5, 0);
        paint_ceiling_halos(&mut buf, theme, &halos);
    }

    #[test]
    fn dust_motes_clamp_to_a_tiny_buffer() {
        let theme = &crate::theme::NORMAL;
        let layout = crate::layout::Layout::compute(192, 80, Some(4)).expect("layout fits");
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
