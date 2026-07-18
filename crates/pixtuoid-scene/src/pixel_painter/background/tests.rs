use super::*;

// Task 4's headline invariant for the golden-hour blaze: it must be
// SUN-only. Uses hand-built SkyState/Atmo values (not real clock times) so
// even a maximally warm/lit MOON — which a real moon's low altitude/luminance
// could never actually produce — still proves the gate is absolute.
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
    // Regression for the "fog read as dark mist" bug — fog must be the
    // brighter (luminous white-out) of the two.
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
    // Fog is the heaviest veil; clear/windy/snow leave the skyline crisp.
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
    // Every offset keeps the whole flash inside its own bucket.
    assert!(offsets
        .iter()
        .all(|&o| o < LIGHTNING_PERIOD_MS - LIGHTNING_FLASH_MS));
}

// The Storm window arm paints rain streaks plus a bright on-glass bolt that
// fires only inside the ~90 ms lightning flash. Drive the painter directly
// with Weather::Storm at a `now` inside a low-offset strike window (same
// technique as lightning_flash_storm_only) and assert the glass interior is
// markedly brighter than the same window painted one second later (no flash).
#[test]
fn storm_window_bolt_brightens_glass_during_the_flash() {
    use std::time::{Duration, UNIX_EPOCH};
    let bucket = (0u64..)
        .find(|&b| strike_offset(b) < 500)
        .expect("a low-offset bucket exists");
    let off = strike_offset(bucket);
    let at = |ms: u64| UNIX_EPOCH + Duration::from_millis(bucket * LIGHTNING_PERIOD_MS + ms);
    // Sanity: the chosen instant has a positive flash level, the next-second
    // probe does not — so the only difference between the two renders is the
    // bolt block.
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
        // Sum luminance over the glass interior (inside the 1px frame).
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

// The spill/window bounds clamps: a buffer barely taller than the wall band
// forces the window-light spill trapezoid AND the floor-to-ceiling window to
// run off the bottom edge, exercising the `break` / `continue` guards. The
// render must not panic and the in-bounds rows must still paint.
#[test]
fn short_buffer_clamps_spill_and_window_without_panic() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let top_wall_h = 18u16;
    // buf_h sits just above top_wall_h so the spill (SPILL_DEPTH rows below
    // the wall band) and the window glass both straddle the bottom edge.
    let buf_h = top_wall_h + 2;
    let buf_w = 60u16;
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(12 * 3600);
    // Construct the look directly with a positive spill so the spill path
    // runs regardless of the local clock.
    let look = TimeOfDayLook {
        glass_a: theme.office.building_light,
        glass_b: theme.office.building_dark,
        spill_strength: 0.8,
        spill_slant: 0.0,
        darkness: 0.2,
    };
    let mut buf = RgbBuffer::filled(buf_w, buf_h, Rgb { r: 5, g: 5, b: 5 });
    paint_floor_and_walls(
        &mut buf, buf_w, buf_h, now, &look, top_wall_h, None, theme, 0.0,
    );
    // No panic reaching here is the primary assertion (RgbBuffer::put has no
    // bounds guard). The wall band's in-bounds rows must still be painted.
    assert_ne!(
        buf.get(0, 0),
        Rgb { r: 5, g: 5, b: 5 },
        "the wall band should still paint in the in-bounds rows"
    );
}

/// Build a `SystemTime` for local `h:mi` on a fixed date — mirrors
/// `sky.rs`'s private `at_hour`, TZ-independent since every derivation
/// (`sky::emitter`/`weather_state`) decodes back into `chrono::Local`.
fn at_local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> SystemTime {
    use chrono::TimeZone;
    chrono::Local
        .with_ymd_and_hms(y, mo, d, h, mi, 0)
        .single()
        .expect("local time should be unambiguous")
        .into()
}

/// Render a full office wall (via the real `paint_floor_and_walls` path —
/// exercises `compute_disc` + the sky-branch blend exactly as production
/// does) at a forced January `day` + local `hour` + weather. Resets the
/// weather override on drop so a mid-test panic can't leak into a
/// sibling test's thread.
fn render_office_on(
    day: u32,
    hour: u32,
    weather: Weather,
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
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let now = at_local(2026, 1, day, hour, 0);
    let look = time_of_day_look(now, theme);
    let buf_h = top_wall_h + 4;
    let mut buf = RgbBuffer::filled(buf_w, buf_h, Rgb { r: 4, g: 4, b: 6 });
    paint_floor_and_walls(
        &mut buf, buf_w, buf_h, now, &look, top_wall_h, None, theme, 0.0,
    );
    buf
}

/// `render_office_on` pinned to January 1st — the fixed date every
/// existing hour/weather-only test uses (the moon-phase tests below are
/// the ones that vary the day).
fn render_office_at(hour: u32, weather: Weather, buf_w: u16, top_wall_h: u16) -> RgbBuffer {
    render_office_on(1, hour, weather, buf_w, top_wall_h)
}

/// Count "warm bright" pixels (the sun disc's signature — its core color
/// fully replaces the sky pixel at full atmo visibility) in the sky-only
/// top third of the window band. Restricted to the top third (not the
/// full `1..top_wall_h`) so it can never pick up the SKYLINE's own lit
/// city-window dots (`theme.office.city_lit_windows`), which live in the
/// glass's bottom half regardless of time of day and would otherwise
/// false-positive as a "disc".
fn count_warm_bright(buf: &RgbBuffer, top_wall_h: u16) -> usize {
    (1..(top_wall_h / 3).max(2))
        .flat_map(|y| (0..buf.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let p = buf.get(x, y);
            p.r > 200 && p.r > p.b.saturating_add(40)
        })
        .count()
}

/// Count "cool bright" pixels (the moon disc's signature) in the same
/// sky-only region. Per-theme `moon_core` values sit closer to neutral
/// white than each theme's warm `sun_core`, so the blue-over-red margin
/// is smaller than `count_warm_bright`'s (10 vs 40) — still well clear of
/// the base night-sky gradient (`theme.lighting.night_sky_a/b`), whose
/// blue channel never approaches 200.
fn count_cool_bright(buf: &RgbBuffer, top_wall_h: u16) -> usize {
    (1..(top_wall_h / 3).max(2))
        .flat_map(|y| (0..buf.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let p = buf.get(x, y);
            p.b > 200 && p.b > p.r.saturating_add(10)
        })
        .count()
}

/// Count faint-white STAR pixels in the same sky-only top-third band as
/// `count_warm_bright`/`count_cool_bright` (so it can't pick up the
/// skyline's lit city-window dots). The base night sky (`night_sky_a/b`,
/// (18,26,52)/(28,36,70)) never gets close to this threshold on its own —
/// only a `STAR_COLOR` blend lifts a pixel this bright.
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
    // 07:00: the sun sits low (altitude ≈0.41, well under the
    // HORIZON_FRAC/ARC_RISE_FRAC ≈0.69 clip threshold), so its disc
    // lands inside the glass rather than climbing off the top.
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
    // Thick cloud hides the disc UNIFORMLY: Rain's disc channel (0.05,
    // same as Overcast/Storm) must hide it entirely too, not just dim it
    // — regression guard for the old 0.20 value that let Rain out-show
    // Overcast.
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
    // Hold `top_wall_h` CONSTANT and vary only the hour, so the only
    // difference between the two renders is the sun's altitude:
    // `compute_disc`'s `cy` is `top_wall_h * (HORIZON_FRAC -
    // altitude*ARC_RISE_FRAC)`, and at the apex (12:00, altitude ≈0.99)
    // the bracket is solidly negative — the disc has climbed entirely
    // above the glass, regardless of `top_wall_h`'s size ("real low
    // window": the apex ALWAYS clips, by construction).
    let buf_w = 96u16;
    let top_wall_h = 40u16; // same height for both renders — only the HOUR varies
    let low = render_office_at(7, Weather::Clear, buf_w, top_wall_h); // altitude ~0.41, in-glass
    let apex = render_office_at(12, Weather::Clear, buf_w, top_wall_h); // altitude ~0.99, clipped
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
    // A SHORT window at the apex shrinks `window_h`/`glass_h` to their
    // floor while the disc's `cy` is solidly negative — must not panic.
    let _ = render_office_at(12, Weather::Clear, 96, 10);
}

#[test]
fn disc_lands_in_a_window_never_on_the_wall_margin() {
    // Regression guard for the original wall-margin-vanish bug (the OLD
    // linear `cx` overshot the last painted pane onto blank wall). Now that
    // `compute_disc` maps azimuth across the real tiled span AND the disc is
    // gated to the window its centre is over, the disc can legitimately hide
    // behind an inter-window pillar at some hours — so it is NOT visible at
    // every hour. Across a sweep of low-sun hours it must, for every buffer
    // width: (a) appear inside a real window at least once (not perpetually
    // lost), and (b) NEVER paint a pixel past the last painted window (the
    // wall margin — the bug this guards).
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
    // Regression guard for the old center-to-center mapping: with only
    // ONE window painted, `first_center == last_center` (both the same
    // window's centre), so `cx` was CONSTANT regardless of azimuth — the
    // disc froze on the shared mullion column instead of sweeping. The
    // new inset-span mapping must still sweep even on a single-window
    // buffer. buf_w=40 paints exactly one window (WINDOW_W=22 + a margin
    // too narrow for a second 22+3px pane).
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
    // 21:00: one hour past dusk, the moon's night-arc altitude is still
    // low (≈0.59, under the clip threshold) — unlike the small hours
    // (00:00-02:00), which sit near the night arc's OWN apex (the
    // dusk-to-dawn span's midpoint) and clip exactly like a midday sun.
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
    // 02:00: deep night, near the moon's own night-arc apex, so its disc
    // clips (near-)entirely above the glass (see `moon_disc_shows_at_night`'s
    // doc comment on why THAT test uses 21:00 instead) — the only bright
    // thing left to find in the upper sky band is a star.
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
    // The star gate must key on the sun being BELOW the horizon (night), not
    // on `darkness`. At 07:00 the sun is up but low, so a HIGH darkness (0.6)
    // is passed — yet stars must be OFF (else a full field wrongly shows at
    // dawn). Counting rendered pixels can't test this (the pale dawn sky is
    // itself "faint-white"), so assert the pure gate directly.
    let at = |h: u32| at_local(2026, 1, 1, h, 0);
    // Dawn: sun up (emitter is the Sun) → no stars regardless of darkness.
    assert_eq!(
        night_star_strength(at(7), 0.6, Weather::Clear),
        0.0,
        "no stars at 7am while the sun is up"
    );
    // Deep night, clear: sun down (emitter is the Moon) → stars visible.
    assert!(
        night_star_strength(at(2), 0.9, Weather::Clear) > STAR_MIN,
        "a clear night should light the stars"
    );
    // Night but overcast: the clear-sky factor (atmo.disc≈0.05) drops it
    // below STAR_MIN → the thick cloud hides the stars.
    assert!(
        night_star_strength(at(2), 0.9, Weather::Overcast) < STAR_MIN,
        "overcast should hide the stars even at night"
    );
}

#[test]
fn disc_never_bleeds_across_a_window_pillar() {
    // Physics-audit repro: a disc whose `cx` lands near an inter-window gap
    // is wide enough (radius + glow) to reach the glass on BOTH sides of the
    // solid wall pillar (frame + WINDOW_GAP + frame). Before the per-window
    // gate it painted in both panes at once — the sun/moon showing THROUGH a
    // wall. The disc must light at most ONE window at any instant. A wide
    // buffer has many internal gaps; sweep the low-sun hours so `cx` passes
    // over one.
    let buf_w = 280u16;
    let top_wall_h = 40u16;
    let stride = (WINDOW_W + WINDOW_GAP) as i32;
    for h in [5u32, 6, 7, 17, 18, 19] {
        let buf = render_office_at(h, Weather::Clear, buf_w, top_wall_h);
        let mut wins = std::collections::HashSet::new();
        // Upper sky band only (top third) so the skyline's own lit city dots
        // can't masquerade as disc-core pixels.
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
    // At 21:00 the moon disc sits low & in-glass at FULL atmo visibility
    // under Clear (`vis == atmo(Clear).disc == 1.0`), so every
    // disc-interior pixel becomes EXACTLY `theme.lighting.moon_core` (lit)
    // or EXACTLY `MOON_SHADOW` (the dark limb) — no partial blend to
    // muddy the count. The disc's (cx, cy, r) depend only on the hour
    // (not the date), so one `compute_disc` call gives the shared
    // bounding box for every day.
    let buf_w = 96u16;
    let top_wall_h = 40u16;
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let geom = compute_disc(
        at_local(2026, 1, 1, 21, 0),
        Weather::Clear,
        buf_w,
        top_wall_h,
        theme,
    )
    .expect("moon disc visible at 21:00 under Clear");

    let crescent_day = (1..=31u32)
        .find(|&d| sky::moon_phase(at_local(2026, 1, d, 21, 0)) < 0.35)
        .expect("a crescent night exists in January 2026");
    let full_day = (1..=31u32)
        .find(|&d| sky::moon_phase(at_local(2026, 1, d, 21, 0)) > 0.9)
        .expect("a near-full night exists in January 2026");

    // (dark-limb count, lit-bright count) within the disc proper.
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
    // The glow halo must track the phase: a new moon's near-dark core
    // should cast (almost) no ring, unlike a full moon's bright one.
    // Search January 2026 at 21:00 for the min/max illuminated fraction
    // (mirrors `moon_luminance_tracks_phase` in sky.rs).
    let buf_w = 96u16;
    let top_wall_h = 40u16;
    let (mut new_moon_day, mut new_moon_frac) = (1u32, f32::MAX);
    let (mut full_moon_day, mut full_moon_frac) = (1u32, f32::MIN);
    for day in 1..=31u32 {
        let frac = sky::moon_phase(at_local(2026, 1, day, 21, 0));
        if frac < new_moon_frac {
            new_moon_frac = frac;
            new_moon_day = day;
        }
        if frac > full_moon_frac {
            full_moon_frac = frac;
            full_moon_day = day;
        }
    }

    // Count faint cool "glow ring" pixels — a softer bar than
    // `count_cool_bright`'s core threshold, catching the halo blend
    // around the disc rather than requiring a fully-opaque core hit.
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
