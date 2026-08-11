//! Ambient pass — non-character, non-furniture effects painted between
//! the background and the y-sorted drawables: sun spot on wall, dust
//! motes in window spill, ceiling halos above active monitors.

use std::time::SystemTime;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::state::FloorLocalDeskIndex;

use crate::layout::Layout;
use crate::pixel_painter::background::{
    beam_strength, paint_radial_falloff, sun_on_wall, window_spill_columns, RadialFalloff,
    TimeOfDayLook, WallSide,
};
use crate::pixel_painter::palette::{blend_pixel, blend_rgb};
use crate::pixel_painter::PaintCtx;
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
        // only the lowest bits, collapsing all three motes into one pixel.
        let mut s = floor_seed
            .wrapping_add((col.x as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9))
            .wrapping_add((i as u64).wrapping_mul(0x94d0_49bb_1331_11eb));
        // splitmix64 finalizer, open-coded by deliberate choice (see
        // `strike_offset` in background/mod.rs for the rationale).
        s = (s ^ (s >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        s = (s ^ (s >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        s ^= s >> 31;
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

pub(super) fn paint_ambient(
    ctx: &mut PaintCtx<'_>,
    look: &TimeOfDayLook,
    seated_agents: &std::collections::HashMap<FloorLocalDeskIndex, bool>,
) {
    paint_sun_spot(ctx.buf, ctx.theme, ctx.layout, ctx.now, look);
    paint_dust_motes(
        ctx.buf,
        ctx.theme,
        ctx.layout,
        ctx.floor.floor_seed,
        ctx.now,
        look,
    );
    let halos = collect_ceiling_halos(ctx, seated_agents);
    paint_ceiling_halos(ctx.buf, ctx.theme, &halos);
}

#[derive(Debug, Clone, Copy)]
pub(super) struct CeilingHalo {
    pub x: u16,
    pub y: u16,
    pub color: Rgb,
    pub intensity: f32,
}

/// Base brightness of a ceiling halo before distance falloff.
const CEILING_HALO_INTENSITY: f32 = 0.8;
/// Peak additive glow at a halo's center (caps the composited strength).
const CEILING_HALO_MAX_STRENGTH: f32 = 0.4;

/// Soft 5×2 halo above each lit monitor, tinted by the active tool's glow
/// color. Dark themes only — on a light theme the warm tint reads as grime.
pub(super) fn paint_ceiling_halos(buf: &mut RgbBuffer, theme: &Theme, halos: &[CeilingHalo]) {
    use crate::theme::ThemeKind;
    if theme.kind != ThemeKind::Dark {
        return;
    }
    for halo in halos {
        for dy in 0..2u16 {
            for dx in 0..5u16 {
                let x = halo.x.saturating_sub(2).saturating_add(dx);
                let y = halo.y.saturating_sub(dy);
                if x >= buf.width() || y >= buf.height() {
                    continue;
                }
                let dist = ((dx as i32 - 2).abs() as f32 + dy as f32) / 3.0;
                let strength = (halo.intensity * (1.0 - dist).max(0.0) * CEILING_HALO_MAX_STRENGTH)
                    .clamp(0.0, 1.0);
                let cur = buf.get(x, y);
                buf.put(x, y, blend_rgb(cur, halo.color, strength));
            }
        }
    }
}

/// Gather one halo per agent currently mid-tool-call. `desk.x + 6` is the
/// centre of the lit screen column band `paint_screen_glow` uses; y sits one
/// row above the desk so the halo lands in the wall band, not on the monitor.
fn collect_ceiling_halos(
    ctx: &PaintCtx<'_>,
    seated_agents: &std::collections::HashMap<FloorLocalDeskIndex, bool>,
) -> Vec<CeilingHalo> {
    use pixtuoid_core::state::ActivityState;
    let mut halos = Vec::new();
    for agent in ctx.scene.agents.values() {
        if !matches!(
            agent.state,
            ActivityState::Active {
                detail: Some(_),
                ..
            }
        ) {
            continue;
        }
        if agent.exiting_at.is_some() {
            continue;
        }
        if agent.floor_idx != ctx.floor.floor_idx {
            continue;
        }
        // Only halo a desk whose occupant is actually SEATED right now — not
        // mid-walk (entry / snap-back) during the Active grace window.
        if !seated_agents
            .get(&agent.desk_index.single_floor_local())
            .copied()
            .unwrap_or(false)
        {
            continue;
        }
        let Some(desk) = ctx.layout.home_desk(agent.desk_index.single_floor_local()) else {
            continue;
        };
        // Unreachable given the `Active { detail: Some(_) }` guard above — a
        // total binding, not a missing-coverage target.
        let Some(color) =
            crate::pixel_painter::palette::tool_glow_tint(agent, &ctx.theme.tool_glow)
        else {
            continue;
        };
        halos.push(CeilingHalo {
            x: desk.x + 6,
            y: desk.y.saturating_sub(1),
            color,
            intensity: CEILING_HALO_INTENSITY,
        });
    }
    halos
}

/// Drift 1-pixel warm specks through each window's sunbeam spill column.
pub(super) fn paint_dust_motes(
    buf: &mut RgbBuffer,
    theme: &Theme,
    layout: &Layout,
    floor_seed: u64,
    now: SystemTime,
    look: &TimeOfDayLook,
) {
    if sun_on_wall(now).is_none() {
        return;
    }
    // Motes scatter the DIRECT beam, so density rides `beam_strength` (full
    // under clear sky, faint through haze/snow-glare, zero under thick
    // overcast/rain); `look.spill_strength` adds the daylight ramp.
    let beam = beam_strength(now);
    if beam <= 0.0 {
        return;
    }
    let visibility = look.spill_strength * beam;
    if visibility <= 0.0 {
        return;
    }
    let warm = theme.lighting.sun_spill;
    for col in window_spill_columns(layout) {
        for DustMote { x, y, alpha } in dust_mote_positions(floor_seed, now, &col) {
            let strength = alpha * 0.7 * visibility;
            blend_pixel(buf, x, y, warm, strength);
        }
    }
}

pub(super) fn paint_sun_spot(
    buf: &mut RgbBuffer,
    theme: &Theme,
    layout: &Layout,
    now: SystemTime,
    look: &TimeOfDayLook,
) {
    let Some(spot) = sun_on_wall(now) else {
        return;
    };
    // South wall is the glass: a spot painted on it would ghost-glow over the
    // skyline, and the floor spill already conveys midday sun.
    if matches!(spot.wall, WallSide::South) {
        return;
    }
    // The spot is the projected DIRECT beam, so diffuse light under thick
    // overcast/rain reaches the wall but never as a defined rectangle.
    let beam = beam_strength(now);
    if beam <= 0.0 {
        return;
    }
    let effective_intensity = spot.intensity * look.spill_strength * beam;
    if effective_intensity <= 0.0 {
        return;
    }
    let warm = theme.lighting.sun_spill;
    // Blend warm toward white as the sun climbs (warmth → 0 at noon).
    let cool = 1.0 - spot.warmth;
    let white = Rgb {
        r: 255,
        g: 255,
        b: 255,
    };
    let color = blend_rgb(warm, white, cool * 0.6);

    // A visible sun rectangle, not a 4px speck: keep a generous floor size so
    // the radial falloff doesn't collapse the spot to nothing on the dark wall.
    let base_w = 10u16;
    let base_h = 4u16;
    let w = (((base_w as f32) * effective_intensity).round() as u16).max(7);
    let h = (((base_h as f32) * effective_intensity).round() as u16).max(3);

    let wall_band_h = layout.wall_band_h();
    if wall_band_h == 0 {
        return;
    }

    // Slide range keeps the spot WITHIN the wall band: along_px ∈ [0, band−h].
    // A band shorter than the spot gives 0, pinning it to the band top.
    let along_range = wall_band_h.saturating_sub(h) as f32;
    let (rx, ry) = match spot.wall {
        WallSide::East => {
            let along_px = along_range * spot.along.min(1.0);
            let cx = layout.buf_w.saturating_sub(w);
            (cx, along_px as u16)
        }
        WallSide::West => {
            let along_px = along_range * spot.along.min(1.0);
            (0u16, along_px as u16)
        }
        WallSide::South => unreachable!("guarded above"),
    };

    // Visible warm lift on the dark wall: a strong base so the small, radially
    // falling-off spot actually reads, gently scaled by how direct the light is.
    let tint_strength = (0.45 + 0.35 * effective_intensity).min(0.7);
    let max_x = (rx + w).min(buf.width());
    let max_y = (ry + h).min(buf.height());
    // Centre on (w−1)/2 so the ellipse spans the loop's full inclusive index
    // range symmetrically; `w/2` biases it half a cell off-grid, sampling only
    // the top-left quadrant at small sizes.
    let cx = rx as f32 + (w.saturating_sub(1)) as f32 * 0.5;
    let cy = ry as f32 + (h.saturating_sub(1)) as f32 * 0.5;
    let rx_norm = ((w.saturating_sub(1)) as f32 * 0.5).max(1.0);
    let ry_norm = ((h.saturating_sub(1)) as f32 * 0.5).max(1.0);
    paint_radial_falloff(
        buf,
        RadialFalloff {
            min_x: rx,
            max_x,
            min_y: ry,
            max_y,
            cx,
            cy,
            rx_norm,
            ry_norm,
        },
        tint_strength,
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel_painter::background::{time_of_day_look, weather_state};
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

    #[test]
    fn ceiling_halo_painted_on_dark_theme() {
        let mut buf = RgbBuffer::filled(160, 90, Rgb { r: 0, g: 0, b: 0 });
        let theme = &crate::theme::CYBERPUNK;
        let halos = vec![CeilingHalo {
            x: 50,
            y: 10,
            color: Rgb {
                r: 0,
                g: 200,
                b: 255,
            },
            intensity: 0.8,
        }];
        let baseline = buf.get(50, 10);
        paint_ceiling_halos(&mut buf, theme, &halos);
        assert_ne!(baseline, buf.get(50, 10), "halo should brighten the pixel");
    }

    #[test]
    fn ceiling_halo_skipped_on_light_theme() {
        let mut buf = RgbBuffer::filled(160, 90, Rgb { r: 0, g: 0, b: 0 });
        let theme = &crate::theme::NORMAL;
        let halos = vec![CeilingHalo {
            x: 50,
            y: 10,
            color: Rgb {
                r: 0,
                g: 200,
                b: 255,
            },
            intensity: 0.8,
        }];
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
    fn sun_spot_scales_with_beam_strength() {
        use crate::pixel_painter::background::Weather;
        let theme = &crate::theme::NORMAL;
        let layout = crate::layout::Layout::compute(192, 80, Some(4)).expect("layout fits");
        // 07:00 → East-wall spot; weather varies by day, so search days for each.
        let morning = |day: u32| crate::localclock::on_day(day, 7);
        let find = |want: Weather| (0..60u32).map(morning).find(|t| weather_state(*t) == want);
        let clear_t = find(Weather::Clear).expect("a clear morning");
        let snow_t = find(Weather::Snow).expect("a snow morning");
        let rain_t = find(Weather::Rain).expect("a rain morning");

        let brightness = |now: SystemTime| -> u64 {
            let mut buf = RgbBuffer::filled(
                192,
                80,
                Rgb {
                    r: 20,
                    g: 20,
                    b: 24,
                },
            );
            paint_sun_spot(&mut buf, theme, &layout, now, &time_of_day_look(now, theme));
            let mut sum = 0u64;
            for y in 0..buf.height() {
                for x in 0..buf.width() {
                    let p = buf.get(x, y);
                    sum += p.r as u64 + p.g as u64 + p.b as u64;
                }
            }
            sum
        };
        let base = 192u64 * 80 * (20 + 20 + 24);
        let clear = brightness(clear_t);
        let snow = brightness(snow_t);
        let rain = brightness(rain_t);

        assert!(
            clear > snow,
            "clear beam brighter than snow ({clear} vs {snow})"
        );
        assert!(
            snow > base,
            "snow still throws a faint spot ({snow} vs {base})"
        );
        assert_eq!(rain, base, "rain has no direct beam → no sun spot");
    }

    // At the exact sunrise instant BOTH `spot.intensity` and `beam_strength`
    // are exactly zero (`sin(pi * 0.0) == 0.0`, no precision fuzz), so the
    // no-op must hold whichever early-return catches it.
    #[test]
    fn sun_spot_paints_nothing_at_the_exact_sunrise_instant() {
        let theme = &crate::theme::NORMAL;
        let layout = crate::layout::Layout::compute(192, 80, Some(4)).expect("layout fits");
        let sunrise = crate::localclock::at_hour(5);
        let spot = sun_on_wall(sunrise).expect("sun is up (just risen) at 05:00");
        assert_eq!(spot.intensity, 0.0, "altitude is exactly zero at sunrise");

        let fill = Rgb {
            r: 20,
            g: 20,
            b: 24,
        };
        let mut buf = RgbBuffer::filled(192, 80, fill);
        paint_sun_spot(
            &mut buf,
            theme,
            &layout,
            sunrise,
            &time_of_day_look(sunrise, theme),
        );
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                assert_eq!(
                    buf.get(x, y),
                    fill,
                    "zero-intensity sun spot must paint nothing"
                );
            }
        }
    }

    #[test]
    fn ceiling_halo_near_edge_does_not_panic() {
        let mut buf = RgbBuffer::filled(6, 4, Rgb { r: 0, g: 0, b: 0 });
        let theme = &crate::theme::CYBERPUNK; // Dark theme so halos paint.
        let halos = vec![CeilingHalo {
            x: 5,
            y: 0,
            color: Rgb {
                r: 0,
                g: 200,
                b: 255,
            },
            intensity: 0.8,
        }];
        paint_ceiling_halos(&mut buf, theme, &halos);
    }

    #[test]
    fn dust_motes_clamp_to_a_tiny_buffer() {
        let theme = &crate::theme::NORMAL;
        let layout = crate::layout::Layout::compute(192, 80, Some(4)).expect("layout fits");
        // 07:00 Clear morning → sun up + full beam.
        let now = (1..=60u32)
            .map(|day| crate::localclock::on_day(day, 7))
            .find(|t| weather_state(*t) == crate::pixel_painter::background::Weather::Clear)
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
            now,
            &time_of_day_look(now, theme),
        );
    }

    #[test]
    fn sun_spot_zero_wall_band_returns_early() {
        let theme = &crate::theme::NORMAL;
        // top_margin == WALL_BAND_TO_TOP_MARGIN → wall_band_h saturating_sub to 0.
        let mut layout = crate::layout::Layout::compute(192, 80, Some(4)).expect("layout fits");
        layout.top_margin = crate::layout::WALL_BAND_TO_TOP_MARGIN;
        // A real beam under Clear, so execution reaches the wall_band_h == 0
        // guard rather than an earlier return.
        let clear_morning = (1..=60u32)
            .map(|day| crate::localclock::on_day(day, 7))
            .find(|t| weather_state(*t) == crate::pixel_painter::background::Weather::Clear)
            .expect("a clear morning");
        let fill = Rgb {
            r: 20,
            g: 20,
            b: 24,
        };
        let mut buf = RgbBuffer::filled(layout.buf_w, layout.buf_h, fill);
        paint_sun_spot(
            &mut buf,
            theme,
            &layout,
            clear_morning,
            &time_of_day_look(clear_morning, theme),
        );
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                assert_eq!(buf.get(x, y), fill, "zero wall band → no sun spot");
            }
        }
    }
}
