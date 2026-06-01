//! Ambient pass — non-character, non-furniture effects painted between
//! the background and the y-sorted drawables: sun spot on wall, dust
//! motes in window spill, ceiling halos above active monitors.
//!
//! Each subroutine is independently togglable and self-contained. New
//! ambient effects go here, not in `background/` or `drawable.rs`.

use std::time::{Duration, SystemTime};

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::tui::layout::Layout;
use crate::tui::pixel_painter::background::{
    sun_on_wall, time_of_day_look, weather_light, weather_state, window_spill_columns, WallSide,
};
use crate::tui::pixel_painter::palette::blend;
use crate::tui::pixel_painter::PixelCtx;
use crate::tui::theme::Theme;

pub(super) struct SunbeamColumn {
    pub x: u16,
    pub top_y: u16,
    pub depth: u16,
}

const MOTES_PER_COLUMN: usize = 3;

/// Deterministic per `(floor_seed, particle_id, now)`. Returns up to
/// `MOTES_PER_COLUMN` positions inside the column: sine drift in x,
/// slow fall in y, alpha fades in/out at the top/bottom 15% bands so
/// motes don't pop on/off at the spill boundary.
pub(super) fn dust_mote_positions(
    floor_seed: u64,
    now: SystemTime,
    col: &SunbeamColumn,
) -> Vec<(u16, u16, f32)> {
    let t_ms = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;
    let mut out = Vec::with_capacity(MOTES_PER_COLUMN);
    for i in 0..MOTES_PER_COLUMN {
        // Mix floor_seed, column x, and particle id through splitmix64 so
        // every (column, mote) pair gets an independent 64-bit seed. The
        // prior approach `floor_seed * K + i` only varied the lowest few
        // bits, leaving the >> 4/12/14 shifts identical across all three
        // motes — they collapsed to a single drifting pixel per column.
        let mut s = floor_seed
            .wrapping_add((col.x as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9))
            .wrapping_add((i as u64).wrapping_mul(0x94d0_49bb_1331_11eb));
        s = (s ^ (s >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        s = (s ^ (s >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        s ^= s >> 31;
        let phase = (s % 6283) as f32 / 1000.0;
        let speed_y = 0.6 + ((s >> 12) & 0x3) as f32 * 0.2;
        let speed_x = 0.4 + ((s >> 14) & 0x3) as f32 * 0.15;
        let cycle = col.depth as f32;
        let y_offset = ((t_ms as f32 / 1000.0) * speed_y + ((s >> 4) & 0xFF) as f32) % cycle;
        let y = col.top_y + y_offset as u16;
        let sx = (phase + (t_ms as f32 / 1000.0) * speed_x).sin();
        // Clamp x to [0, u16::MAX] before casting — negative f32 silently
        // wraps to 0 via `as u16`, dragging motes to the left buffer edge
        // on narrow terminals where col.x is small.
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
        out.push((x, y, alpha));
    }
    out
}

pub(super) fn paint_ambient(
    ctx: &mut PixelCtx<'_>,
    seated_agents: &std::collections::HashMap<usize, bool>,
) {
    paint_sun_spot(ctx.buf, ctx.theme, ctx.layout, ctx.now);
    paint_dust_motes(
        ctx.buf,
        ctx.theme,
        ctx.layout,
        ctx.floor.floor_seed,
        ctx.now,
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

/// Soft 5×2 tinted halo above each lit monitor — tied to the active
/// tool's glow color so the ceiling reads "this desk is doing edits"
/// at a glance. Painted only on dark themes; on light themes the warm
/// tint reads as grime, not glow, so we short-circuit.
pub(super) fn paint_ceiling_halos(buf: &mut RgbBuffer, theme: &Theme, halos: &[CeilingHalo]) {
    use crate::tui::theme::ThemeKind;
    if theme.kind != ThemeKind::Dark {
        return;
    }
    for halo in halos {
        for dy in 0..2u16 {
            for dx in 0..5u16 {
                let x = halo.x.saturating_sub(2).saturating_add(dx);
                let y = halo.y.saturating_sub(dy);
                if x >= buf.width || y >= buf.height {
                    continue;
                }
                let dist = ((dx as i32 - 2).abs() as f32 + dy as f32) / 3.0;
                let strength = (halo.intensity * (1.0 - dist).max(0.0) * 0.4).clamp(0.0, 1.0);
                let cur = buf.get(x, y);
                buf.put(
                    x,
                    y,
                    Rgb(
                        blend(cur.0, halo.color.0, strength),
                        blend(cur.1, halo.color.1, strength),
                        blend(cur.2, halo.color.2, strength),
                    ),
                );
            }
        }
    }
}

/// Gather one halo per agent currently mid-tool-call. Monitor x is the
/// centre of the screen sprite that `paint_screen_glow` lights up
/// (desk.x + 6, matching the 4..=9 lit column band). Ceiling y is one
/// row above the desk's top edge so the halo sits in the wall band
/// rather than on the monitor frame itself.
fn collect_ceiling_halos(
    ctx: &PixelCtx<'_>,
    seated_agents: &std::collections::HashMap<usize, bool>,
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
        // mid-walk (entry / snap-back) during the Active grace window. Mirrors
        // the desk-cubicle screen-glow gate so the two never disagree.
        if !seated_agents
            .get(&agent.desk_index)
            .copied()
            .unwrap_or(false)
        {
            continue;
        }
        let Some(desk) = ctx.layout.home_desks.get(agent.desk_index) else {
            continue;
        };
        let Some(color) =
            crate::tui::pixel_painter::palette::tool_glow_tint(agent, &ctx.theme.tool_glow)
        else {
            continue;
        };
        halos.push(CeilingHalo {
            x: desk.x + 6,
            y: desk.y.saturating_sub(1),
            color,
            intensity: 0.8,
        });
    }
    halos
}

/// Drift 1-pixel warm specks through each window's sunbeam spill column.
/// Only paints when `sun_on_wall(now)` reports the sun is visible —
/// otherwise there's no sunbeam for motes to ride. Cheap: 3 motes per
/// column × ~6-8 columns × 1 px each.
pub(super) fn paint_dust_motes(
    buf: &mut RgbBuffer,
    theme: &Theme,
    layout: &Layout,
    floor_seed: u64,
    now: SystemTime,
) {
    if sun_on_wall(now).is_none() {
        return;
    }
    // Dust motes scatter the direct beam; their density rides `beam_strength`
    // (full under clear sky, faint through thin cloud/haze/snow-glare, zero
    // under thick overcast/rain/storm). `look.spill_strength` adds the daylight
    // ramp so they also fade in/out with the hour.
    let beam = weather_light(weather_state(now)).beam_strength;
    if beam <= 0.0 {
        return;
    }
    let look = time_of_day_look(now, theme);
    let visibility = look.spill_strength * beam;
    if visibility <= 0.0 {
        return;
    }
    let warm = theme.lighting.sun_spill;
    for col in window_spill_columns(layout) {
        for (x, y, alpha) in dust_mote_positions(floor_seed, now, &col) {
            if x >= buf.width || y >= buf.height {
                continue;
            }
            let cur = buf.get(x, y);
            let strength = alpha * 0.7 * visibility;
            buf.put(
                x,
                y,
                Rgb(
                    blend(cur.0, warm.0, strength),
                    blend(cur.1, warm.1, strength),
                    blend(cur.2, warm.2, strength),
                ),
            );
        }
    }
}

pub(super) fn paint_sun_spot(buf: &mut RgbBuffer, theme: &Theme, layout: &Layout, now: SystemTime) {
    let Some(spot) = sun_on_wall(now) else {
        return;
    };
    // South wall is the window wall — paint_window_light_spill already
    // conveys midday sun via warm spill on the floor under the glass.
    // Painting on the glass itself would ghost-glow over the skyline.
    if matches!(spot.wall, WallSide::South) {
        return;
    }
    // The wall sun-spot is the projected direct beam. Its strength rides
    // `beam_strength`: a sharp rectangle under clear sky, a faint smudge through
    // haze/thin-cloud/snow-glare, gone entirely under thick overcast/rain/storm
    // (diffuse light reaches the wall but never as a defined rectangle).
    // `look.spill_strength` adds the daylight ramp so it fades in/out smoothly.
    let beam = weather_light(weather_state(now)).beam_strength;
    if beam <= 0.0 {
        return;
    }
    let look = time_of_day_look(now, theme);
    let effective_intensity = spot.intensity * look.spill_strength * beam;
    if effective_intensity <= 0.0 {
        return;
    }
    let warm = theme.lighting.sun_spill;
    // Blend warm toward white as the sun climbs (warmth → 0 at noon).
    let cool = 1.0 - spot.warmth;
    let color = Rgb(
        blend(warm.0, 255, cool * 0.6),
        blend(warm.1, 255, cool * 0.6),
        blend(warm.2, 255, cool * 0.6),
    );

    // A visible sun rectangle, not a 4px speck: keep a generous floor size so
    // the radial falloff doesn't collapse the spot to nothing on the dark wall.
    let base_w = 10u16;
    let base_h = 4u16;
    let w = (((base_w as f32) * effective_intensity).round() as u16).max(7);
    let h = (((base_h as f32) * effective_intensity).round() as u16).max(3);

    // The top wall band is the visible window wall; East/West sun spots
    // project onto the outer 1-px column at the left/right edge of that band.
    let wall_band_h = layout
        .top_margin
        .saturating_sub(pixtuoid_core::layout::WALL_BAND_TO_TOP_MARGIN);
    if wall_band_h == 0 {
        return;
    }

    // Slide range keeps the spot WITHIN the wall band: along_px ∈ [0, band-h].
    // (The old `.max(band-1)` let ry reach band-1, so the spot bled `h` px below
    // the wall band onto the room floor.) On a band shorter than the spot this
    // is 0 → the spot pins to the band top, correct (it fills the band).
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

    // Visible warm lift on the dark wall: a strong base so the (small, radially
    // falling-off) spot actually reads, gently scaled by how direct the light is.
    // The old `0.35 * effective_intensity` (~0.10) blended sub-1-RGB → invisible.
    let tint_strength = (0.45 + 0.35 * effective_intensity).min(0.7);
    let max_x = (rx + w).min(buf.width);
    let max_y = (ry + h).min(buf.height);
    // Centre at (rx + (w-1)/2, ry + (h-1)/2) so the ellipse spans the
    // loop's full inclusive index range symmetrically — pre-fix used
    // `rx + w/2` which biased the centre half a cell off-grid, making
    // the falloff sample only the top-left quadrant at small sizes.
    let cx = rx as f32 + (w.saturating_sub(1)) as f32 * 0.5;
    let cy = ry as f32 + (h.saturating_sub(1)) as f32 * 0.5;
    let rx_norm = ((w.saturating_sub(1)) as f32 * 0.5).max(1.0);
    let ry_norm = ((h.saturating_sub(1)) as f32 * 0.5).max(1.0);
    for y in ry..max_y {
        for x in rx..max_x {
            // Quadratic radial falloff so the spot reads round, not boxy.
            let nx = (x as f32 - cx) / rx_norm;
            let ny = (y as f32 - cy) / ry_norm;
            let r2 = nx * nx + ny * ny;
            if r2 > 1.0 {
                continue;
            }
            let t = (1.0 - r2) * tint_strength;
            let cur = buf.get(x, y);
            buf.put(
                x,
                y,
                Rgb(
                    blend(cur.0, color.0, t),
                    blend(cur.1, color.1, t),
                    blend(cur.2, color.2, t),
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn ceiling_halo_painted_on_dark_theme() {
        let mut buf = RgbBuffer::filled(160, 90, Rgb(0, 0, 0));
        let theme = &crate::tui::theme::CYBERPUNK;
        let halos = vec![CeilingHalo {
            x: 50,
            y: 10,
            color: Rgb(0, 200, 255),
            intensity: 0.8,
        }];
        let baseline = buf.get(50, 10);
        paint_ceiling_halos(&mut buf, theme, &halos);
        assert_ne!(baseline, buf.get(50, 10), "halo should brighten the pixel");
    }

    #[test]
    fn ceiling_halo_skipped_on_light_theme() {
        let mut buf = RgbBuffer::filled(160, 90, Rgb(0, 0, 0));
        let theme = &crate::tui::theme::NORMAL;
        let halos = vec![CeilingHalo {
            x: 50,
            y: 10,
            color: Rgb(0, 200, 255),
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
            for (_, _, alpha) in dust_mote_positions(123, now, &col) {
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

    // The F4 change: the wall sun-spot now SCALES by `beam_strength` instead of
    // gating on a bool. A clear morning beams hard (brightest spot), a snowy
    // morning throws a faint-but-real spot (beam 0.25), and rain has no beam at
    // all (no spot). Verifies the multiply, the faint-beam path, and the early-out.
    #[test]
    fn sun_spot_scales_with_beam_strength() {
        use crate::tui::pixel_painter::background::Weather;
        use chrono::TimeZone;
        let theme = &crate::tui::theme::NORMAL;
        let layout = crate::tui::layout::Layout::compute(192, 80, 4).expect("layout fits");
        // 07:00 → East-wall spot. Weather varies by day at a fixed hour, so
        // search days for each weather (TZ-independent).
        let morning = |day: u32| -> SystemTime {
            chrono::Local
                .with_ymd_and_hms(2026, 1, day, 7, 0, 0)
                .single()
                .unwrap()
                .into()
        };
        let find = |want: Weather| (1..=60u32).map(morning).find(|t| weather_state(*t) == want);
        let clear_t = find(Weather::Clear).expect("a clear morning");
        let snow_t = find(Weather::Snow).expect("a snow morning");
        let rain_t = find(Weather::Rain).expect("a rain morning");

        let brightness = |now: SystemTime| -> u64 {
            let mut buf = RgbBuffer::filled(192, 80, Rgb(20, 20, 24));
            paint_sun_spot(&mut buf, theme, &layout, now);
            let mut sum = 0u64;
            for y in 0..buf.height {
                for x in 0..buf.width {
                    let p = buf.get(x, y);
                    sum += p.0 as u64 + p.1 as u64 + p.2 as u64;
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
}
