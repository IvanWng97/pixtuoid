use super::*;

// Hand-built SkyState/Atmo values, not real clock times: a real moon's low
// altitude/luminance could never produce these, so a maximally warm/lit MOON
// proves the gate is absolute rather than merely well-behaved in practice.
#[test]
fn golden_hour_blaze_is_sun_only() {
    let full_atmo = sky::Atmo {
        direct: 1.0,
        diffuse: 1.0,
        disc: 1.0,
    };
    let moon = sky::SkyState {
        body: sky::Body::Moon,
        altitude: 1.0,
        azimuth: 0.5,
        warmth: 1.0,
        emitter_lum: 1.0,
    };
    assert_eq!(
        golden_hour_blaze(&moon, &full_atmo),
        0.0,
        "a moon must never blaze, even at maximal warmth/luminance"
    );
    let sun = sky::SkyState {
        body: sky::Body::Sun,
        ..moon
    };
    assert!(
        golden_hour_blaze(&sun, &full_atmo) > 0.9,
        "a maximal sun should blaze near-full"
    );
}

#[test]
fn weather_floor_tint_differs_by_variant() {
    let clear = weather_floor_tint(Weather::Clear);
    let rain = weather_floor_tint(Weather::Rain);
    let fog = weather_floor_tint(Weather::Fog);
    assert_ne!(clear, rain, "rain biases floor cooler");
    assert_ne!(clear, fog, "fog desaturates");
    assert!(
        rain.b >= rain.r,
        "rain tint should be cool (blue >= red), got {:?}",
        rain
    );
}

#[test]
fn weather_floor_tint_clear_is_near_neutral() {
    let clear = weather_floor_tint(Weather::Clear);
    assert!(
        clear.r > 200 && clear.g > 200 && clear.b > 200,
        "clear should be a near-white slight-warm tint, got {:?}",
        clear
    );
}

#[test]
fn fog_floor_tint_is_brighter_than_overcast() {
    let fog = weather_floor_tint(Weather::Fog);
    let oc = weather_floor_tint(Weather::Overcast);
    let lum = |c: Rgb| c.r as u16 + c.g as u16 + c.b as u16;
    assert!(
        lum(fog) > lum(oc),
        "fog {fog:?} should outshine overcast {oc:?}"
    );
}

#[test]
fn skyline_haze_obscures_fog_and_storm_only_when_expected() {
    let fog = skyline_haze(Weather::Fog).expect("fog hazes").1;
    let storm = skyline_haze(Weather::Storm).expect("storm hazes").1;
    assert!(fog > storm, "fog should obscure more than storm");
    assert!(
        skyline_haze(Weather::Clear).is_none(),
        "clear skyline is crisp"
    );
    assert!(
        skyline_haze(Weather::Snow).is_none(),
        "snow skyline is crisp"
    );
}

#[test]
fn lightning_envelope_is_a_two_pulse_then_dark() {
    assert_eq!(lightning_envelope(0), 1.0, "primary strike");
    assert!(
        lightning_envelope(30) < lightning_envelope(0),
        "dim between flickers"
    );
    assert!(
        lightning_envelope(50) > lightning_envelope(30),
        "after-flash rebrightens"
    );
    assert_eq!(lightning_envelope(LIGHTNING_FLASH_MS), 0.0, "flash is over");
    assert_eq!(lightning_envelope(5000), 0.0, "dark between strikes");
}

#[test]
fn lightning_flash_storm_only_and_mid_strike_only() {
    use std::time::{Duration, UNIX_EPOCH};
    // Strikes are jittered per bucket, so the flash is at `strike_offset(bucket)`
    // into the bucket, not phase 0. Pick a low-offset bucket so off+1000 (the
    // quiet probe) stays inside the same bucket.
    let bucket = (0u64..)
        .find(|&b| strike_offset(b) < 500)
        .expect("a low-offset bucket exists");
    let off = strike_offset(bucket);
    let at = |ms: u64| UNIX_EPOCH + Duration::from_millis(bucket * LIGHTNING_PERIOD_MS + ms);
    let mk = || {
        RgbBuffer::filled(
            8,
            4,
            Rgb {
                r: 10,
                g: 10,
                b: 12,
            },
        )
    };

    let mut b = mk();
    paint_lightning_flash(&mut b, at(off), Weather::Storm);
    assert!(b.get(0, 0).r > 10, "storm strike should brighten the room");

    let mut b = mk();
    paint_lightning_flash(&mut b, at(off + 1000), Weather::Storm);
    assert_eq!(
        b.get(0, 0),
        Rgb {
            r: 10,
            g: 10,
            b: 12
        },
        "no flash between strikes"
    );

    let mut b = mk();
    paint_lightning_flash(&mut b, at(off), Weather::Clear);
    assert_eq!(
        b.get(0, 0),
        Rgb {
            r: 10,
            g: 10,
            b: 12
        },
        "flash is storm-only"
    );
}

#[test]
fn lightning_strikes_are_jittered_not_metronomic() {
    let offsets: Vec<u64> = (0..24u64).map(strike_offset).collect();
    let distinct = offsets
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len();
    assert!(
        distinct > 12,
        "strike offsets should vary across buckets, got {offsets:?}"
    );
    assert!(offsets
        .iter()
        .all(|&o| o < LIGHTNING_PERIOD_MS - LIGHTNING_FLASH_MS));
}

#[test]
fn storm_window_bolt_brightens_glass_during_the_flash() {
    use std::time::{Duration, UNIX_EPOCH};
    // Low-offset bucket for the same reason as `lightning_flash_storm_only`.
    let bucket = (0u64..)
        .find(|&b| strike_offset(b) < 500)
        .expect("a low-offset bucket exists");
    let off = strike_offset(bucket);
    let at = |ms: u64| UNIX_EPOCH + Duration::from_millis(bucket * LIGHTNING_PERIOD_MS + ms);
    assert!(
        lightning_flash_level(at(off)) > 0.0,
        "flash at strike offset"
    );
    assert_eq!(
        lightning_flash_level(at(off + 1000)),
        0.0,
        "quiet 1 s later"
    );

    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let render_lum = |now: SystemTime| -> u64 {
        let look = time_of_day_look(now, theme);
        let (lit_colors, building, sky_row) = window_glass_invariants(30, &look, theme);
        let mut buf = RgbBuffer::filled(40, 40, Rgb { r: 8, g: 8, b: 10 });
        paint_floor_to_ceiling_window(
            &mut buf,
            0,
            0,
            WINDOW_W,
            30,
            theme.surface.window_frame,
            0,
            now,
            Weather::Storm,
            0.0,
            &lit_colors,
            building,
            &sky_row,
            None,
            0.0,
        );
        let mut sum = 0u64;
        for y in 1..29u16 {
            for x in 1..(WINDOW_W - 1) {
                let p = buf.get(x, y);
                sum += p.r as u64 + p.g as u64 + p.b as u64;
            }
        }
        sum
    };
    let flashing = render_lum(at(off));
    let quiet = render_lum(at(off + 1000));
    assert!(
        flashing > quiet,
        "the on-glass bolt must brighten the storm glass during the flash \
         (flash={flashing}, quiet={quiet})"
    );
}

#[test]
fn short_buffer_clamps_spill_and_window_without_panic() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let top_wall_h = 18u16;
    // buf_h sits just above top_wall_h so the spill (SPILL_DEPTH rows below
    // the wall band) and the window glass both straddle the bottom edge.
    let buf_h = top_wall_h + 2;
    let buf_w = 60u16;
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(12 * 3600);
    // A hand-built look with positive spill, so the spill path runs regardless
    // of the local clock.
    let look = TimeOfDayLook {
        glass_a: theme.office.building_light,
        glass_b: theme.office.building_dark,
        spill_strength: 0.8,
        spill_slant: 0.0,
        darkness: 0.2,
        // Strength 0 — this fixture exercises the spill path, not the wash.
        object_wash: [(theme.lighting.night_tint, 0.0); 2],
    };
    let mut buf = RgbBuffer::filled(buf_w, buf_h, Rgb { r: 5, g: 5, b: 5 });
    paint_floor_and_walls(
        &mut BaseFillCache::new(),
        &mut buf,
        buf_w,
        buf_h,
        now,
        &look,
        top_wall_h,
        None,
        theme,
        0.0,
    );
    // Reaching here without a panic IS the primary assertion — `RgbBuffer::put`
    // has no bounds guard.
    assert_ne!(
        buf.get(0, 0),
        Rgb { r: 5, g: 5, b: 5 },
        "the wall band should still paint in the in-bounds rows"
    );
}

/// Render a full office wall through the real `paint_floor_and_walls` path at a
/// forced January `day` + local `hour` + weather.
fn render_office_on(
    day: u32,
    hour: u32,
    weather: Weather,
    buf_w: u16,
    top_wall_h: u16,
) -> RgbBuffer {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    render_office_themed(day, hour, weather, theme, buf_w, top_wall_h)
}

/// [`render_office_on`] with the theme as a parameter — the weather/light
/// invariants hold per THEME (each ships its own night-sky + glass colours), so
/// their pins sweep `ALL_THEMES` rather than trusting `normal` to be worst-case.
/// The `Reset` guard clears the weather override even on a mid-test panic, which
/// would otherwise leak into a sibling test's thread.
fn render_office_themed(
    day: u32,
    hour: u32,
    weather: Weather,
    theme: &'static crate::theme::Theme,
    buf_w: u16,
    top_wall_h: u16,
) -> RgbBuffer {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            set_weather_override(None);
        }
    }
    let _reset = Reset;
    set_weather_override(Some(weather));
    let now = crate::localclock::on_day(day, hour);
    let look = time_of_day_look(now, theme);
    let buf_h = top_wall_h + 4;
    let mut buf = RgbBuffer::filled(buf_w, buf_h, Rgb { r: 4, g: 4, b: 6 });
    paint_floor_and_walls(
        &mut BaseFillCache::new(),
        &mut buf,
        buf_w,
        buf_h,
        now,
        &look,
        top_wall_h,
        None,
        theme,
        0.0,
    );
    buf
}

/// `render_office_on` pinned to January 1st, for the hour/weather-only tests.
fn render_office_at(hour: u32, weather: Weather, buf_w: u16, top_wall_h: u16) -> RgbBuffer {
    render_office_on(1, hour, weather, buf_w, top_wall_h)
}

/// Count "warm bright" pixels (the sun disc's signature) in the sky-only top
/// third of the window band. Restricted to the top third so it can never pick up
/// the SKYLINE's own lit city-window dots, which live in the glass's bottom half
/// regardless of time of day and would false-positive as a "disc".
fn count_warm_bright(buf: &RgbBuffer, top_wall_h: u16) -> usize {
    (1..(top_wall_h / 3).max(2))
        .flat_map(|y| (0..buf.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let p = buf.get(x, y);
            p.r > 200 && p.r > p.b.saturating_add(40)
        })
        .count()
}

/// Count "cool bright" pixels (the moon disc's signature) in the same sky-only
/// region. `moon_core` sits closer to neutral white than the warm `sun_core`, so
/// the blue-over-red margin is smaller than `count_warm_bright`'s — still well
/// clear of the base night-sky gradient, whose blue never approaches 200.
fn count_cool_bright(buf: &RgbBuffer, top_wall_h: u16) -> usize {
    (1..(top_wall_h / 3).max(2))
        .flat_map(|y| (0..buf.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let p = buf.get(x, y);
            p.b > 200 && p.b > p.r.saturating_add(10)
        })
        .count()
}

/// Count faint-white STAR pixels in the same sky-only top-third band. The base
/// night sky never gets close to this threshold on its own — only a `STAR_COLOR`
/// blend lifts a pixel this bright.
fn count_faint_white(buf: &RgbBuffer, top_wall_h: u16) -> usize {
    (1..(top_wall_h / 3).max(2))
        .flat_map(|y| (0..buf.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let p = buf.get(x, y);
            p.r > 90 && p.g > 90 && p.b > 90
        })
        .count()
}

#[test]
fn disc_appears_low_in_the_sky_at_a_low_sun_hour() {
    // 07:00: the sun sits low, under the HORIZON_FRAC/ARC_RISE_FRAC clip
    // threshold, so its disc lands inside the glass rather than off the top.
    let buf_w = 96u16;
    let top_wall_h = 40u16;
    let clear = render_office_at(7, Weather::Clear, buf_w, top_wall_h);
    let overcast = render_office_at(7, Weather::Overcast, buf_w, top_wall_h);
    let clear_n = count_warm_bright(&clear, top_wall_h);
    let overcast_n = count_warm_bright(&overcast, top_wall_h);
    assert!(
        clear_n >= 3,
        "a warm disc should show at a low clear sun hour, got {clear_n} bright px"
    );
    assert!(
        clear_n > overcast_n,
        "overcast (atmo disc visibility below MIN_DISC_VIS) should hide the \
         disc clear shows: clear={clear_n} overcast={overcast_n}"
    );
}

#[test]
fn rain_hides_the_disc_like_overcast() {
    let buf_w = 96u16;
    let top_wall_h = 40u16;
    let clear = render_office_at(7, Weather::Clear, buf_w, top_wall_h);
    let rain = render_office_at(7, Weather::Rain, buf_w, top_wall_h);
    let overcast = render_office_at(7, Weather::Overcast, buf_w, top_wall_h);
    let clear_n = count_warm_bright(&clear, top_wall_h);
    let rain_n = count_warm_bright(&rain, top_wall_h);
    let overcast_n = count_warm_bright(&overcast, top_wall_h);
    assert!(
        clear_n >= 3,
        "clear should show a disc at a low sun hour, got {clear_n}"
    );
    assert_eq!(
        rain_n, 0,
        "rain should hide the disc entirely, like overcast, got {rain_n}"
    );
    assert_eq!(
        overcast_n, 0,
        "overcast should hide the disc entirely, got {overcast_n}"
    );
}

#[test]
fn disc_clips_above_the_glass_at_the_arc_apex() {
    // `top_wall_h` is CONSTANT across both renders so the only difference is the
    // sun's altitude: at the apex `compute_disc`'s `cy` bracket goes negative
    // whatever the wall height, so the apex ALWAYS clips by construction.
    let buf_w = 96u16;
    let top_wall_h = 40u16;
    let low = render_office_at(7, Weather::Clear, buf_w, top_wall_h);
    let apex = render_office_at(12, Weather::Clear, buf_w, top_wall_h);
    let low_n = count_warm_bright(&low, top_wall_h);
    let apex_n = count_warm_bright(&apex, top_wall_h);
    assert!(low_n >= 3, "low sun should show a disc: {low_n}");
    assert_eq!(
        apex_n, 0,
        "the apex disc must clip entirely above the glass: {apex_n}"
    );
}

#[test]
fn short_window_apex_does_not_panic() {
    // top_wall_h=10 shrinks `window_h`/`glass_h` to their floor while the apex
    // disc's `cy` is solidly negative.
    let _ = render_office_at(12, Weather::Clear, 96, 10);
}

#[test]
fn disc_lands_in_a_window_never_on_the_wall_margin() {
    // The disc can legitimately hide behind an inter-window pillar at some hours,
    // so it is NOT visible at every hour — hence the two-part sweep: it must
    // appear inside a real window at least once, and NEVER paint past the last
    // painted window (the wall margin, which is the bug this guards).
    let top_wall_h = 40u16;
    let stride = (WINDOW_W + WINDOW_GAP) as f32;
    for buf_w in [76u16, 96, 120, 150, 192, 220, 300] {
        // Last painted window's right edge (mirrors compute_disc's tiling).
        let k_max = (((buf_w as f32) - WINDOW_W as f32 - 5.0) / stride).floor();
        let last_right = (3.0 + k_max.max(0.0) * stride + WINDOW_W as f32) as u16;
        let mut seen_in_a_window = false;
        for h in [5u32, 6, 7, 17, 18, 19] {
            let buf = render_office_at(h, Weather::Clear, buf_w, top_wall_h);
            for y in 1..(top_wall_h / 3).max(2) {
                for x in 0..buf.width() {
                    let p = buf.get(x, y);
                    if p.r > 240 && p.r as i16 - p.b as i16 > 40 {
                        assert!(
                            x < last_right,
                            "buf_w={buf_w} h={h}: disc pixel at x={x} is past the \
                             last window (wall margin; last right edge {last_right})"
                        );
                        seen_in_a_window = true;
                    }
                }
            }
        }
        assert!(
            seen_in_a_window,
            "buf_w={buf_w}: the disc never appeared in a window across the low-sun sweep"
        );
    }
}

#[test]
fn disc_sweeps_across_a_single_window_buffer() {
    // buf_w=40 paints EXACTLY one window (too narrow for a second pane) — the
    // degenerate case where a center-to-center azimuth mapping has zero span and
    // freezes `cx` on the mullion.
    let buf_w = 40u16;
    let top_wall_h = 40u16;
    let morning = render_office_at(7, Weather::Clear, buf_w, top_wall_h);
    let evening = render_office_at(18, Weather::Clear, buf_w, top_wall_h);
    let warm_center_x = |buf: &RgbBuffer| -> f32 {
        let mut sum = 0u32;
        let mut count = 0u32;
        for y in 1..(top_wall_h / 3).max(2) {
            for x in 0..buf.width() {
                let p = buf.get(x, y);
                if p.r > 200 && p.r > p.b.saturating_add(40) {
                    sum += x as u32;
                    count += 1;
                }
            }
        }
        assert!(count > 0, "expected a warm disc to render in this buffer");
        sum as f32 / count as f32
    };
    let morning_x = warm_center_x(&morning);
    let evening_x = warm_center_x(&evening);
    assert!(
        (morning_x - evening_x).abs() > 1.0,
        "the disc must sweep across a single-window buffer, not freeze on \
         the mullion: morning_x={morning_x} evening_x={evening_x}"
    );
}

#[test]
fn moon_disc_shows_at_night() {
    // 21:00, not the small hours: those sit near the night arc's OWN apex and
    // clip above the glass exactly like a midday sun.
    let buf_w = 96u16;
    let top_wall_h = 40u16;
    let clear = render_office_at(21, Weather::Clear, buf_w, top_wall_h);
    let overcast = render_office_at(21, Weather::Overcast, buf_w, top_wall_h);
    let clear_n = count_cool_bright(&clear, top_wall_h);
    let overcast_n = count_cool_bright(&overcast, top_wall_h);
    assert!(
        clear_n >= 3,
        "a cool moon disc should show at a clear night hour, got {clear_n} bright px"
    );
    assert!(
        clear_n > overcast_n,
        "overcast should hide the moon disc clear shows: \
         clear={clear_n} overcast={overcast_n}"
    );
}

#[test]
fn stars_appear_on_a_clear_night_and_vanish_under_overcast() {
    // 02:00 sits near the moon's night-arc apex, so its disc clips above the
    // glass — the only bright thing left in the upper sky band is a star.
    let buf_w = 96u16;
    let top_wall_h = 40u16;
    let clear = render_office_at(2, Weather::Clear, buf_w, top_wall_h);
    let overcast = render_office_at(2, Weather::Overcast, buf_w, top_wall_h);
    let clear_n = count_faint_white(&clear, top_wall_h);
    let overcast_n = count_faint_white(&overcast, top_wall_h);
    assert!(
        clear_n >= 3,
        "a clear night should show some stars in the upper sky, got {clear_n}"
    );
    assert!(
        clear_n > overcast_n,
        "overcast (atmo.disc below STAR_MIN once multiplied by darkness) \
         should hide the stars a clear sky shows: clear={clear_n} overcast={overcast_n}"
    );
}

#[test]
fn stars_gate_on_night_not_darkness_alone() {
    // Counting rendered pixels can't test this — the pale dawn sky is itself
    // "faint-white" — so assert the pure gate directly, with a HIGH darkness
    // passed at an hour when the sun is up.
    let at = crate::localclock::at_hour;
    assert_eq!(
        night_star_strength(at(7), 0.6, Weather::Clear),
        0.0,
        "no stars at 7am while the sun is up"
    );
    assert!(
        night_star_strength(at(2), 0.9, Weather::Clear) > STAR_MIN,
        "a clear night should light the stars"
    );
    assert!(
        night_star_strength(at(2), 0.9, Weather::Overcast) < STAR_MIN,
        "overcast should hide the stars even at night"
    );
}

#[test]
fn disc_never_bleeds_across_a_window_pillar() {
    // A disc whose `cx` lands near an inter-window gap is wide enough (radius +
    // glow) to reach the glass on BOTH sides of the solid wall pillar — the
    // sun/moon showing THROUGH a wall. A wide buffer has many internal gaps;
    // sweeping the low-sun hours makes `cx` pass over one.
    let buf_w = 280u16;
    let top_wall_h = 40u16;
    let stride = (WINDOW_W + WINDOW_GAP) as i32;
    for h in [5u32, 6, 7, 17, 18, 19] {
        let buf = render_office_at(h, Weather::Clear, buf_w, top_wall_h);
        let mut wins = std::collections::HashSet::new();
        // Top third only, so the skyline's lit city dots can't pose as disc pixels.
        for y in 1..(top_wall_h / 3).max(2) {
            for x in 0..buf.width() {
                let p = buf.get(x, y);
                if !(p.r > 240 && p.r as i16 - p.b as i16 > 40) {
                    continue;
                }
                let rel = x as i32 - 3;
                if rel < 0 {
                    continue;
                }
                if rel % stride < WINDOW_W as i32 {
                    wins.insert(rel / stride);
                }
            }
        }
        assert!(
            wins.len() <= 1,
            "at {h}:00 the disc lit {} windows {:?} — it bled across a wall pillar",
            wins.len(),
            wins
        );
    }
}

#[test]
fn crescent_moon_leaves_the_dark_limb_unlit() {
    // 21:00 Clear puts the disc in-glass at FULL atmo visibility, so every
    // disc-interior pixel is EXACTLY `moon_core` or EXACTLY `MOON_SHADOW` — no
    // partial blend to muddy the count. (cx, cy, r) depend only on the hour, not
    // the date, so one `compute_disc` call gives the bounding box for every day.
    let buf_w = 96u16;
    let top_wall_h = 40u16;
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let geom = compute_disc(
        crate::localclock::at_hour(21),
        Weather::Clear,
        buf_w,
        top_wall_h,
        theme,
    )
    .expect("moon disc visible at 21:00 under Clear");

    let crescent_day = (1..=31u32)
        .find(|&d| sky::moon_phase(crate::localclock::on_day(d, 21)) < 0.35)
        .expect("a crescent night exists in January 2026");
    let full_day = (1..=31u32)
        .find(|&d| sky::moon_phase(crate::localclock::on_day(d, 21)) > 0.9)
        .expect("a near-full night exists in January 2026");

    let count_dark_and_bright = |day: u32| -> (usize, usize) {
        let buf = render_office_on(day, 21, Weather::Clear, buf_w, top_wall_h);
        let r = geom.r.ceil() as i32;
        let (cx, cy) = (geom.cx.round() as i32, geom.cy.round() as i32);
        let mut dark = 0usize;
        let mut bright = 0usize;
        for py in (cy - r)..=(cy + r) {
            for px in (cx - r)..=(cx + r) {
                if px < 0 || py < 0 || px as u16 >= buf.width() || py as u16 >= buf.height() {
                    continue;
                }
                let dx = px as f32 - geom.cx;
                let dy = py as f32 - geom.cy;
                if dx * dx + dy * dy > geom.r * geom.r {
                    continue; // outside the disc proper
                }
                let p = buf.get(px as u16, py as u16);
                if p == MOON_SHADOW {
                    dark += 1;
                } else if p.b > 200 && p.b > p.r.saturating_add(10) {
                    bright += 1;
                }
            }
        }
        (dark, bright)
    };

    let (crescent_dark, crescent_bright) = count_dark_and_bright(crescent_day);
    let (full_dark, full_bright) = count_dark_and_bright(full_day);

    assert!(
        crescent_bright >= 2,
        "the crescent should still show a lit sliver, got {crescent_bright}"
    );
    assert!(
        crescent_dark >= 2,
        "the crescent should leave a dark limb unlit, got {crescent_dark}"
    );
    assert!(
        full_bright >= 2,
        "a near-full moon should be lit, got {full_bright}"
    );
    assert!(
        crescent_dark > full_dark,
        "a crescent should have strictly MORE dark-within-disc pixels than \
         a near-full moon: crescent={crescent_dark} full={full_dark}"
    );
    assert!(
        crescent_dark >= full_dark + 10,
        "assert a real margin, not a hair's-breadth win: \
         crescent={crescent_dark} full={full_dark}"
    );
}

#[test]
fn moon_glow_dims_at_new_moon() {
    let buf_w = 96u16;
    let top_wall_h = 40u16;
    let (mut new_moon_day, mut new_moon_frac) = (1u32, f32::MAX);
    let (mut full_moon_day, mut full_moon_frac) = (1u32, f32::MIN);
    for day in 1..=31u32 {
        let frac = sky::moon_phase(crate::localclock::on_day(day, 21));
        if frac < new_moon_frac {
            new_moon_frac = frac;
            new_moon_day = day;
        }
        if frac > full_moon_frac {
            full_moon_frac = frac;
            full_moon_day = day;
        }
    }

    // A softer bar than `count_cool_bright`'s core threshold, so it catches the
    // halo blend rather than requiring a fully-opaque core hit.
    let count_glow_ring = |buf: &RgbBuffer| -> usize {
        (1..(top_wall_h / 3).max(2))
            .flat_map(|y| (0..buf.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let p = buf.get(x, y);
                p.b > 90 && p.b > p.r.saturating_add(5)
            })
            .count()
    };

    let new_moon_buf = render_office_on(new_moon_day, 21, Weather::Clear, buf_w, top_wall_h);
    let full_moon_buf = render_office_on(full_moon_day, 21, Weather::Clear, buf_w, top_wall_h);
    let new_moon_glow = count_glow_ring(&new_moon_buf);
    let full_moon_glow = count_glow_ring(&full_moon_buf);
    assert!(
        new_moon_glow < full_moon_glow,
        "a new moon's glow ring (phase={new_moon_frac}) should show fewer/dimmer \
         cool pixels than a full moon's (phase={full_moon_frac}): \
         new={new_moon_glow} full={full_moon_glow}"
    );
}

#[test]
fn window_columns_tiles_from_the_start_and_keeps_absolute_idx_across_a_skip() {
    let buf_w = FIRST_WINDOW_X + 4 * (WINDOW_W + WINDOW_GAP) + WINDOW_W + WINDOW_EDGE_MARGIN;
    let all: Vec<_> = window_columns(buf_w, None).collect();
    assert!(all.len() >= 3, "expected several panes, got {}", all.len());
    for (k, w) in all.iter().enumerate() {
        assert_eq!(w.idx as usize, k, "idx is the 0-based absolute position");
        assert_eq!(
            w.x_left,
            FIRST_WINDOW_X + k as u16 * (WINDOW_W + WINDOW_GAP)
        );
        assert_eq!(w.center_x, w.x_left + WINDOW_W / 2);
        assert!(w.x_left + WINDOW_W + WINDOW_EDGE_MARGIN <= buf_w);
    }

    // Skip the SECOND pane's x-range — what the elevator door does to the wall.
    let doomed = all[1];
    let skip = Some((doomed.x_left, doomed.x_left + WINDOW_W));
    let kept: Vec<_> = window_columns(buf_w, skip).collect();
    assert_eq!(
        kept.len(),
        all.len() - 1,
        "exactly the overlapping pane is skipped"
    );
    assert!(
        kept.iter().all(|w| w.idx != doomed.idx),
        "the skipped pane's idx never appears"
    );
    assert!(
        kept.iter().any(|w| w.idx == 2),
        "the pane after the door keeps idx 2"
    );
}

/// Mean channel value over every PAINTED window pane's glass interior. The
/// day-over-night invariant is asserted on THIS, not on
/// `time_of_day_look().darkness`: the weather veils are painted onto the glass
/// AFTER the light model produced `sky_row`, so a `darkness`-only assertion is
/// structurally blind to them.
fn glass_mean_luminance(buf: &RgbBuffer, top_wall_h: u16) -> f32 {
    let window_y: u16 = 1;
    let window_h: u16 = top_wall_h.saturating_sub(2).max(8);
    let mut sum = 0.0f64;
    let mut n = 0u32;
    for w in window_columns(buf.width(), None) {
        for y in (window_y + 1)..(window_y + window_h).saturating_sub(1) {
            for x in (w.x_left + 1)..(w.x_left + WINDOW_W).saturating_sub(1) {
                if x < buf.width() && y < buf.height() {
                    let p = buf.get(x, y);
                    sum += f64::from(p.r) + f64::from(p.g) + f64::from(p.b);
                    n += 3;
                }
            }
        }
    }
    assert!(n > 0, "the rig must sample real glass");
    (sum / f64::from(n)) as f32
}

/// The brightest night January 2026 can offer.
fn fullest_moon_day() -> u32 {
    (1..=31u32)
        .max_by(|&a, &b| {
            let phase = |d: u32| sky::moon_phase(crate::localclock::on_day(d, 0));
            phase(a)
                .partial_cmp(&phase(b))
                .expect("moon_phase is never NaN")
        })
        .expect("January has days")
}

/// The pane-luminance rig every weather/time invariant below shares: the glass
/// mean for one theme × hour × weather, at the year's brightest night.
fn pane(theme: &'static crate::theme::Theme, hour: u32, w: Weather) -> f32 {
    const BUF_W: u16 = 120;
    const TOP_WALL_H: u16 = 26;
    glass_mean_luminance(
        &render_office_themed(fullest_moon_day(), hour, w, theme, BUF_W, TOP_WALL_H),
        TOP_WALL_H,
    )
}

/// Local midnight — the moon arc's apex hour. It is NOT the brightest RENDERED
/// night pane (the pre-dawn twilight tint reads brighter on every theme), which
/// is why the two ordering pins below sweep [`night_hours`] instead of sampling
/// this one.
const NIGHT_HOUR: u32 = 0;
/// Solar noon — the daytime reference the pane orderings measure against.
const NOON_HOUR: u32 = 12;

/// Every whole hour at which the sky shows the MOON, straight off
/// [`sky::hour_is_day`] — the ONE day/night boundary, so this sweep can't drift
/// from a second hand-written hour list.
fn night_hours() -> impl Iterator<Item = u32> {
    (0..24u32).filter(|h| !sky::hour_is_day(*h as f32))
}

/// The most of its OWN solar-noon brightness a pane may still show at any night
/// hour. A bare `night < noon` has NO teeth against the veil defect: an
/// absolute-grey veil leaves each weather's night pane just barely under its own
/// noon pane, so the ordering holds while the rendered day/night cycle has
/// collapsed to a few percent. This floor sits in the gap between the veiled and
/// unveiled weather populations.
const MAX_NIGHT_PANE_FRACTION: f32 = 0.75;

// The day/night CONTRAST the light model produces has to survive the weather
// VEIL the painter lays over the glass afterwards — the veils were absolute
// daylight-grey constants with no time input, so a night-lit room sat behind
// daylight-white windows.
#[test]
fn no_weather_flattens_the_glass_day_night_contrast() {
    for theme in crate::theme::ALL_THEMES {
        for w in Weather::ALL {
            let noon = pane(theme, NOON_HOUR, w);
            for hour in night_hours() {
                let night = pane(theme, hour, w);
                assert!(
                    night <= noon * MAX_NIGHT_PANE_FRACTION,
                    "{}/{:?}: the {hour:02}:00 pane must stay under {MAX_NIGHT_PANE_FRACTION} \
                     of its own solar-noon pane (night={night:.1} noon={noon:.1} ratio={:.3})",
                    theme.name,
                    w,
                    night / noon
                );
            }
        }
    }
}

// The cross-weather half of the same defect. The reference is CLEAR noon, not
// the dimmest noon: how bright a snowy/stormy noon pane renders is a
// theme-palette choice, so "the dimmest noon of any weather" is not a property
// of the light model and is deliberately not asserted.
#[test]
fn no_night_pane_outshines_the_clear_solar_noon_pane() {
    for theme in crate::theme::ALL_THEMES {
        let clear_noon = pane(theme, NOON_HOUR, Weather::Clear);
        for w in Weather::ALL {
            for hour in night_hours() {
                let night = pane(theme, hour, w);
                assert!(
                    night < clear_noon,
                    "{}/{:?}: the {hour:02}:00 pane ({night:.1}) must stay below the \
                     clear solar-noon pane ({clear_noon:.1})",
                    theme.name,
                    w
                );
            }
        }
    }
}

// The counter-pin: night-adapting the veil must not ERASE it. A veil scaled to
// zero at night, or deleted outright, passes the two tests above and fails this.
#[test]
fn fog_still_glows_over_the_midnight_sky() {
    const FOG_NIGHT_GLOW_MIN: f32 = 1.25;
    for theme in crate::theme::ALL_THEMES {
        let clear = pane(theme, NIGHT_HOUR, Weather::Clear);
        let fog = pane(theme, NIGHT_HOUR, Weather::Fog);
        assert!(
            fog > clear * FOG_NIGHT_GLOW_MIN,
            "{}: fog must still read as a lit murk at midnight (fog={fog:.1} \
             vs clear={clear:.1})",
            theme.name
        );
        let smog = pane(theme, NIGHT_HOUR, Weather::Smog);
        assert!(
            smog > clear,
            "{}: smog must still veil the midnight sky (smog={smog:.1} vs clear={clear:.1})",
            theme.name
        );
    }
}

#[test]
fn base_fill_cache_hit_is_byte_identical_and_a_key_change_repaints() {
    let normal = crate::theme::theme_by_name("normal").expect("normal theme");
    let other = crate::theme::ALL_THEMES
        .iter()
        .find(|t| {
            t.surface.carpet_base != normal.surface.carpet_base
                || t.surface.wall != normal.surface.wall
        })
        .copied()
        .expect("a theme with a different carpet/wall exists");
    let now = crate::localclock::on_day(1, 12);
    let (buf_w, buf_h, top_wall_h) = (96u16, 64u16, 14u16);
    let paint = |base_fill: &mut BaseFillCache, theme: &'static crate::theme::Theme| {
        let look = time_of_day_look(now, theme);
        let mut buf = RgbBuffer::filled(buf_w, buf_h, Rgb { r: 9, g: 9, b: 9 });
        paint_floor_and_walls(
            base_fill, &mut buf, buf_w, buf_h, now, &look, top_wall_h, None, theme, 0.0,
        );
        buf
    };
    let mut shared = BaseFillCache::new();
    let first = paint(&mut shared, normal);
    let hit = paint(&mut shared, normal);
    assert_eq!(
        first.as_slice(),
        hit.as_slice(),
        "a cache HIT must be byte-identical to the fill it memoized"
    );
    let switched = paint(&mut shared, other);
    let fresh = paint(&mut BaseFillCache::new(), other);
    assert_eq!(
        switched.as_slice(),
        fresh.as_slice(),
        "a theme swap on a warm cache must repaint, not serve the stale fill"
    );
    let back = paint(&mut shared, normal);
    assert_eq!(
        first.as_slice(),
        back.as_slice(),
        "swapping back must re-derive the original fill"
    );

    // Weather leg: the tint changes the CARPET colours while the wall stays
    // put — the one key component nothing else covers.
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            set_weather_override(None);
        }
    }
    let _reset = Reset;
    set_weather_override(Some(Weather::Clear));
    let clear = paint(&mut shared, normal);
    set_weather_override(Some(Weather::Rain));
    let rain_shared = paint(&mut shared, normal);
    let rain_fresh = paint(&mut BaseFillCache::new(), normal);
    assert_eq!(
        rain_shared.as_slice(),
        rain_fresh.as_slice(),
        "a weather-tint change on a warm cache must repaint the carpet, not serve the stale fill"
    );
    assert_ne!(
        clear.as_slice(),
        rain_shared.as_slice(),
        "clear vs rain must differ somewhere in the carpet (else this leg pins nothing)"
    );
}
