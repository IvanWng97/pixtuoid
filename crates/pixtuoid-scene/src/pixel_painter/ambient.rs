//! Ambient pass — non-character, non-furniture effects painted between
//! the background and the y-sorted drawables: sun spot on wall, dust
//! motes in window spill, ceiling halos above active monitors.
//!
//! Each subroutine is independently togglable and self-contained. New
//! ambient effects go here, not in `background/` or `drawable.rs`.

use std::time::{Duration, SystemTime};

use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::state::FloorLocalDeskIndex;

use crate::layout::Layout;
use crate::pixel_painter::background::{
    sun_on_wall, weather_light, weather_state, window_spill_columns, TimeOfDayLook, WallSide,
};
use crate::pixel_painter::palette::blend_rgb;
use crate::pixel_painter::PixelCtx;
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

/// Deterministic per `(floor_seed, particle_id, now)`. Returns up to
/// `MOTES_PER_COLUMN` positions inside the column: sine drift in x,
/// slow fall in y, alpha fades in/out at the top/bottom 15% bands so
/// motes don't pop on/off at the spill boundary.
pub(super) fn dust_mote_positions(
    floor_seed: u64,
    now: SystemTime,
    col: &SunbeamColumn,
) -> Vec<DustMote> {
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
        // splitmix64 finalizer over the per-(column, mote) seed, open-coded by
        // deliberate choice (see `strike_offset` in background/mod.rs for the
        // cross-crate-copy rationale).
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
        out.push(DustMote { x, y, alpha });
    }
    out
}

pub(super) fn paint_ambient(
    ctx: &mut PixelCtx<'_>,
    look: &TimeOfDayLook,
    seated_agents: &std::collections::HashMap<FloorLocalDeskIndex, bool>,
) {
    // `look` is the per-frame `time_of_day_look(ctx.now, ctx.theme)` already
    // computed once in `render_to_rgb_buffer` — forwarded so the sub-passes
    // read its `spill_strength` instead of recomputing the identical value.
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

/// Soft 5×2 tinted halo above each lit monitor — tied to the active
/// tool's glow color so the ceiling reads "this desk is doing edits"
/// at a glance. Painted only on dark themes; on light themes the warm
/// tint reads as grime, not glow, so we short-circuit.
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

/// Gather one halo per agent currently mid-tool-call. Monitor x is the
/// centre of the screen sprite that `paint_screen_glow` lights up
/// (desk.x + 6, matching the 4..=9 lit column band). Ceiling y is one
/// row above the desk's top edge so the halo sits in the wall band
/// rather than on the monitor frame itself.
fn collect_ceiling_halos(
    ctx: &PixelCtx<'_>,
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
        // mid-walk (entry / snap-back) during the Active grace window. Mirrors
        // the desk-cubicle screen-glow gate so the two never disagree.
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
        // `tool_glow_tint` only returns None for a non-Active state, but the
        // `Active { detail: Some(_) }` guard above has already filtered to
        // Active-with-detail agents — so this else arm is unreachable here
        // (the `_ => glow.default` fallback always yields Some). Kept as a
        // total binding; not a missing-coverage target.
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
/// Only paints when `sun_on_wall(now)` reports the sun is visible —
/// otherwise there's no sunbeam for motes to ride. Cheap: 3 motes per
/// column × ~6-8 columns × 1 px each.
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
    // Dust motes scatter the direct beam; their density rides `beam_strength`
    // (full under clear sky, faint through thin cloud/haze/snow-glare, zero
    // under thick overcast/rain/storm). `look.spill_strength` adds the daylight
    // ramp so they also fade in/out with the hour. `look` is the per-frame
    // value forwarded from `render_to_rgb_buffer` (was recomputed here).
    let beam = weather_light(weather_state(now)).beam_strength;
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
            if x >= buf.width() || y >= buf.height() {
                continue;
            }
            let cur = buf.get(x, y);
            let strength = alpha * 0.7 * visibility;
            buf.put(x, y, blend_rgb(cur, warm, strength));
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
    // `look` is the per-frame `time_of_day_look(now, theme)` forwarded from
    // `render_to_rgb_buffer` (was recomputed here for the identical value).
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
    let max_x = (rx + w).min(buf.width());
    let max_y = (ry + h).min(buf.height());
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
            buf.put(x, y, blend_rgb(cur, color, t));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The paint passes now take the per-frame `look` by reference (computed once
    // in `render_to_rgb_buffer`); tests build it the same way the production
    // caller does, so the asserted output is unchanged.
    use crate::pixel_painter::background::time_of_day_look;

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

    // The F4 change: the wall sun-spot now SCALES by `beam_strength` instead of
    // gating on a bool. A clear morning beams hard (brightest spot), a snowy
    // morning throws a faint-but-real spot (beam 0.25), and rain has no beam at
    // all (no spot). Verifies the multiply, the faint-beam path, and the early-out.
    #[test]
    fn sun_spot_scales_with_beam_strength() {
        use crate::pixel_painter::background::Weather;
        use chrono::TimeZone;
        let theme = &crate::theme::NORMAL;
        let layout = crate::layout::Layout::compute(192, 80, Some(4)).expect("layout fits");
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

    // At the exact sun-up boundary (~05:30 / ~19:30 local) `sun_on_wall` returns
    // an East/West spot whose `boundary_fade` is 0, so `spot.intensity == 0` and
    // `effective_intensity` collapses to 0 even with a positive beam — the
    // `effective_intensity <= 0.0` early-return must fire (NOT the earlier beam
    // gate). Search days for a beam-bearing weather at that 10-min bucket, same
    // pattern as the beam-strength test above.
    #[test]
    fn sun_spot_zero_intensity_at_hour_edge_leaves_buffer_untouched() {
        use crate::pixel_painter::background::Weather;
        use chrono::TimeZone;
        let theme = &crate::theme::NORMAL;
        let layout = crate::layout::Layout::compute(192, 80, Some(4)).expect("layout fits");
        // 19:30 local → West-wall spot at the upper boundary (boundary_fade=0).
        let dusk_edge = |day: u32| -> SystemTime {
            chrono::Local
                .with_ymd_and_hms(2026, 1, day, 19, 30, 0)
                .single()
                .unwrap()
                .into()
        };
        // A beam-bearing weather (Clear/Windy/Snow/Smog/Fog) so the earlier
        // `beam <= 0.0` gate is NOT what returns — it must be the intensity gate.
        let beam_bearing = |w: Weather| {
            matches!(
                w,
                Weather::Clear | Weather::Windy | Weather::Snow | Weather::Smog | Weather::Fog
            )
        };
        let now = (1..=120u32)
            .map(dusk_edge)
            .find(|t| beam_bearing(weather_state(*t)))
            .expect("a beam-bearing dusk-edge bucket exists");
        // Precondition: the spot exists and its intensity is zero at this edge.
        let spot = sun_on_wall(now).expect("sun is up at the boundary");
        assert!(
            !matches!(spot.wall, WallSide::South),
            "19:30 must be a West-wall spot, not the South window"
        );
        assert_eq!(spot.intensity, 0.0, "boundary_fade=0 → zero intensity");

        let fill = Rgb {
            r: 20,
            g: 20,
            b: 24,
        };
        let mut buf = RgbBuffer::filled(192, 80, fill);
        paint_sun_spot(&mut buf, theme, &layout, now, &time_of_day_look(now, theme));
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

    // Dust motes and ceiling halos clamp per-pixel writes against the buffer
    // bounds. A mote/halo positioned at the far edge of a tight buffer must hit
    // the `>= buf.width || >= buf.height` continue without panicking (RgbBuffer
    // has no internal bounds guard).
    #[test]
    fn ceiling_halo_near_edge_does_not_panic() {
        let mut buf = RgbBuffer::filled(6, 4, Rgb { r: 0, g: 0, b: 0 });
        let theme = &crate::theme::CYBERPUNK; // Dark theme so halos paint.
                                              // Halo centred at the bottom-right corner: the 5×2 stamp runs off both
                                              // edges, exercising the clamp.
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

    // Dust motes anchored to the layout's spill columns can land outside a
    // buffer narrower/shorter than the layout, exercising the bounds `continue`.
    // Render into a 1×1 buffer with a daytime beam-bearing `now` so the mote
    // loop runs but every put is clamped.
    #[test]
    fn dust_motes_clamp_to_a_tiny_buffer() {
        use chrono::TimeZone;
        let theme = &crate::theme::NORMAL;
        let layout = crate::layout::Layout::compute(192, 80, Some(4)).expect("layout fits");
        // 07:00 Clear morning → sun up + full beam.
        let now = (1..=60u32)
            .map(|day| -> SystemTime {
                chrono::Local
                    .with_ymd_and_hms(2026, 1, day, 7, 0, 0)
                    .single()
                    .unwrap()
                    .into()
            })
            .find(|t| weather_state(*t) == crate::pixel_painter::background::Weather::Clear)
            .expect("a clear morning");
        // Buffer far smaller than the layout's spill columns → every mote put is
        // out of bounds and clamped. The assertion is simply "no panic".
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

    // wall_band_h == 0 (a degenerate tiny top margin) makes paint_sun_spot
    // early-return before any wall projection — must not panic / paint.
    #[test]
    fn sun_spot_zero_wall_band_returns_early() {
        use chrono::TimeZone;
        let theme = &crate::theme::NORMAL;
        // top_margin == WALL_BAND_TO_TOP_MARGIN → wall_band_h saturating_sub to 0.
        let mut layout = crate::layout::Layout::compute(192, 80, Some(4)).expect("layout fits");
        layout.top_margin = pixtuoid_core::layout::WALL_BAND_TO_TOP_MARGIN;
        // 07:00 → East wall spot with a real beam under Clear, so execution
        // reaches the wall_band_h==0 guard rather than an earlier return.
        let clear_morning = (1..=60u32)
            .map(|day| -> SystemTime {
                chrono::Local
                    .with_ymd_and_hms(2026, 1, day, 7, 0, 0)
                    .single()
                    .unwrap()
                    .into()
            })
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
        // wall_band_h == 0 → nothing painted.
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                assert_eq!(buf.get(x, y), fill, "zero wall band → no sun spot");
            }
        }
    }
}
