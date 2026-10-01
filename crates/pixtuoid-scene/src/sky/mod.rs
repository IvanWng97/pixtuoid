//! The sky as a MODEL: the weather, the sun and the moon, lightning timing, and
//! the light they let into the office.
//!
//! Painters read it through one [`Sky`] sampled per frame, so no two of them can
//! disagree about the hour, the weather or whether lightning is striking.
//! Nothing here knows a theme or a pixel.

use std::cell::Cell;
use std::time::SystemTime;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Weather {
    Clear,
    Rain,
    Storm,
    Snow,
    Fog,
    Overcast,
    Windy,
    Smog,
}

impl Weather {
    /// All variants, in canonical order. The site's gallery manifest
    /// (site/src/weather.json) mirrors it; `weather_gallery_manifest_matches_the_weather_enum`
    /// fails on any add/rename here until the manifest (+ gen-media art) follows.
    pub(crate) const ALL: [Weather; 8] = [
        Weather::Clear,
        Weather::Rain,
        Weather::Storm,
        Weather::Snow,
        Weather::Fog,
        Weather::Overcast,
        Weather::Windy,
        Weather::Smog,
    ];

    /// Lowercase CLI name (`Weather::Rain` → `"rain"`).
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Weather::Clear => "clear",
            Weather::Rain => "rain",
            Weather::Storm => "storm",
            Weather::Snow => "snow",
            Weather::Fog => "fog",
            Weather::Overcast => "overcast",
            Weather::Windy => "windy",
            Weather::Smog => "smog",
        }
    }

    /// Parse a CLI name (case-insensitive) back to a variant.
    pub(crate) fn from_name(s: &str) -> Option<Weather> {
        let s = s.trim().to_ascii_lowercase();
        Weather::ALL.into_iter().find(|w| w.name() == s)
    }
}

thread_local! {
    /// [`force_weather`](crate::pixel_painter::force_weather)'s override: when
    /// `Some`, every [`Sky`] sampled on this thread shows it instead of the
    /// clock's pick.
    static WEATHER_OVERRIDE: Cell<Option<Weather>> = const { Cell::new(None) };
}

pub(crate) fn set_weather_override(w: Option<Weather>) {
    WEATHER_OVERRIDE.with(|c| c.set(w));
}

/// A forced weather that clears itself when dropped, so a test cannot leak it
/// into a sibling sharing its thread (plain `cargo test` reuses threads), even
/// when an assert panics first.
#[cfg(test)]
pub(crate) struct ForcedWeather;

#[cfg(test)]
impl ForcedWeather {
    pub(crate) fn new(w: Weather) -> Self {
        set_weather_override(Some(w));
        Self
    }
}

#[cfg(test)]
impl Drop for ForcedWeather {
    fn drop(&mut self) {
        set_weather_override(None);
    }
}

/// How long one weather holds before the next slot picks again.
const WEATHER_CYCLE_SECS: u64 = 600;

/// The weather at `now`: one hashed pick per [`WEATHER_CYCLE_SECS`] slot.
fn weather_at(now: SystemTime) -> Weather {
    if let Some(forced) = WEATHER_OVERRIDE.with(Cell::get) {
        return forced;
    }
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cycle = secs / WEATHER_CYCLE_SECS;
    match crate::splitmix_draw(cycle, 1) % 15 {
        0..=5 => Weather::Clear,
        6..=7 => Weather::Rain,
        8 => Weather::Storm,
        9 => Weather::Snow,
        10 => Weather::Fog,
        11..=12 => Weather::Overcast,
        13 => Weather::Windy,
        _ => Weather::Smog,
    }
}

// Weights folding the two transmission channels into one interior illuminance,
// calibrated so a CLEAR noon lands at full brightness
// (`K_BEAM + transmission(Clear).diffuse · K_FILL ≈ 1`).
const K_BEAM: f32 = 0.70;
const K_FILL: f32 = 0.55;

/// City-light bounce reaching the interior at night — a small, weather-keyed
/// FLOOR so the room is never pitch black even at a new moon. Snow albedo bounces
/// the most; a storm swallows it. Independent of the moon's (date-varying) phase,
/// so the night weather-ordering is phase-stable.
fn city_bounce(w: Weather) -> f32 {
    // Magnitudes stay low enough that a moonlit night's floor can never
    // out-light a stormy solar noon (`solar_noon_outshines_the_brightest_night`).
    let v = match w {
        Weather::Snow => 0.08,
        Weather::Clear => 0.055,
        Weather::Windy => 0.05,
        Weather::Fog => 0.045,
        Weather::Smog => 0.045,
        Weather::Overcast => 0.035,
        Weather::Rain => 0.03,
        Weather::Storm => 0.015,
    };
    debug_assert!(
        (0.0..=1.0).contains(&v),
        "city_bounce out of range: {w:?} -> {v}"
    );
    v
}

/// How much of the emitter's light the weather lets through to the interior:
/// a hard directional beam, a flat diffuse fill, and the disc's own visibility.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Transmission {
    pub(crate) direct: f32,
    pub(crate) diffuse: f32,
    pub(crate) disc: f32,
}

pub(crate) fn transmission(w: Weather) -> Transmission {
    // Storm < Rain in BOTH transmission channels (denser cloud), and
    // Overcast/Rain/Storm share one near-zero disc (below `MIN_DISC_VIS`) so a
    // thicker cloud never shows MORE of the disc than a thinner one.
    let (direct, diffuse, disc) = match w {
        Weather::Clear => (1.00, 0.55, 1.00),
        Weather::Windy => (0.90, 0.55, 0.95),
        Weather::Snow => (0.25, 0.70, 0.30),
        Weather::Smog => (0.30, 0.45, 0.45),
        Weather::Fog => (0.05, 0.75, 0.10),
        Weather::Overcast => (0.00, 0.50, 0.05),
        Weather::Rain => (0.00, 0.40, 0.05),
        Weather::Storm => (0.00, 0.28, 0.05),
    };
    debug_assert!(
        [direct, diffuse, disc]
            .iter()
            .all(|c| (0.0..=1.0).contains(c)),
        "Transmission channels must be 0..=1: {w:?} -> ({direct}, {diffuse}, {disc})"
    );
    Transmission {
        direct,
        diffuse,
        disc,
    }
}

/// Which body the sky shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Body {
    Sun,
    Moon,
}

/// The physical sky emitter — sun by day, moon by night. Luminance + warmth
/// follow altitude (low body = longer air path = dimmer + warmer). The ONE
/// source the interior light, the disc and the spill derive from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Emitter {
    pub(crate) body: Body,
    /// 0 horizon .. 1 apex.
    pub(crate) altitude: f32,
    /// Progress along this body's arc: 0 as it rises .. 1 as it sets.
    pub(crate) azimuth: f32,
    /// 0 neutral (apex) .. 1 warm/red (horizon).
    pub(crate) warmth: f32,
    /// 0..1 luminance reaching the atmosphere.
    pub(crate) emitter_lum: f32,
}

// Sun rides the arc over its up-span; the moon owns the complementary night span.
const SUN_RISE_H: f32 = 5.0;
const SUN_SET_H: f32 = 20.0;
/// How long a moon stays up: the night's length, so a full moon rises at dusk
/// and sets at dawn.
const MOON_UP_H: f32 = SUN_RISE_H + 24.0 - SUN_SET_H;
/// How long, after dusk and before dawn, the night takes to come in and to go:
/// the moon's light, the city's glow and the stars ride [`nightfall`] up and
/// down across it rather than switching with the body.
const NIGHTFALL_H: f32 = 0.75;
/// Moon luminance at a full phase — low enough that a full-moon midnight (plus
/// the `city_bounce` floor) still stays dimmer than a stormy solar noon; see
/// `solar_noon_outshines_the_brightest_night`.
const MOON_PEAK_LUM: f32 = 0.12;
/// The mean synodic month: NASA GSFC, "Phases of the Moon 2001 to 2100"
/// (<https://eclipse.gsfc.nasa.gov/phase/phases2001.html>).
const SYNODIC_DAYS: f32 = 29.530_588;
/// The MEAN new moon of 2019-11-27 02:56 TT, in unix days (Meeus, *Astronomical
/// Algorithms*, eq. 49.1, k = 246): a true new moon would carry its own
/// lunation's offset from the mean into every phase.
const NEW_MOON_EPOCH_UNIX_DAYS: f32 = 18_227.123;

fn arc_progress(h: f32, rise: f32, set: f32) -> f32 {
    ((h - rise) / (set - rise)).clamp(0.0, 1.0)
}

/// Whether the sky shows the SUN (not the moon) at hour-of-day `h` (0..24) — the
/// ONE definition of the day/night boundary, so the emitter and any external
/// consumer can't drift from a second hardcoded copy.
pub(crate) fn hour_is_day(h: f32) -> bool {
    (SUN_RISE_H..SUN_SET_H).contains(&h)
}

/// Fractional local hour (`hour + minute/60`, in `0.0..24.0`) for `now` — the
/// sky's clock decode.
pub(crate) fn local_hour_frac(now: SystemTime) -> f32 {
    use chrono::Timelike;
    let unix_now = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let local = chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + unix_now);
    local.hour() as f32 + local.minute() as f32 / 60.0
}

/// How far into the night hour `h` is, 0..=1: zero by day, rising over
/// [`NIGHTFALL_H`] after dusk and falling over it before dawn.
fn nightfall(h: f32) -> f32 {
    if hour_is_day(h) {
        return 0.0;
    }
    let since_dusk = (h - SUN_SET_H).rem_euclid(24.0);
    let until_dawn = (SUN_RISE_H - h).rem_euclid(24.0);
    (since_dusk.min(until_dawn) / NIGHTFALL_H).min(1.0)
}

/// The moon's progress along its arc at hour `h`, 0 as it rises .. 1 as it
/// sets, or `None` while it is down. A full moon rises at dusk; each day of
/// its `age` puts the rise a synodic share of the day later.
fn moon_arc(h: f32, age: f32) -> Option<f32> {
    let rise = SUN_SET_H + 24.0 * (age / SYNODIC_DAYS - 0.5);
    let t = (h - rise).rem_euclid(24.0) / MOON_UP_H;
    (t <= 1.0).then_some(t)
}

fn emitter_at(now: SystemTime, moon_phase: f32, moon_age: f32) -> Emitter {
    let h = local_hour_frac(now);
    if hour_is_day(h) {
        let t = arc_progress(h, SUN_RISE_H, SUN_SET_H);
        let altitude = (std::f32::consts::PI * t).sin();
        return Emitter {
            body: Body::Sun,
            altitude,
            azimuth: t,
            warmth: (1.0 - altitude).clamp(0.0, 1.0),
            emitter_lum: altitude,
        };
    }
    // A moon below the horizon stands at its rim, lighting nothing.
    let t = moon_arc(h, moon_age).unwrap_or(0.0);
    let altitude = (std::f32::consts::PI * t).sin();
    Emitter {
        body: Body::Moon,
        altitude,
        azimuth: t,
        warmth: (1.0 - altitude).clamp(0.0, 1.0),
        emitter_lum: MOON_PEAK_LUM * altitude * moon_phase * nightfall(h),
    }
}

/// Days into the mean lunation at `now`: 0 at new moon, half a synodic month at
/// full.
fn moon_age_at(now: SystemTime) -> f32 {
    let unix_days = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f32() / 86_400.0)
        .unwrap_or(0.0);
    (unix_days - NEW_MOON_EPOCH_UNIX_DAYS).rem_euclid(SYNODIC_DAYS)
}

/// Illuminated fraction of the moon (0 new .. 1 full), from the synodic month.
fn moon_phase_at(now: SystemTime) -> f32 {
    (1.0 - (std::f32::consts::TAU * moon_age_at(now) / SYNODIC_DAYS).cos()) / 2.0
}

/// Lightning cadence: one strike per bucket this long, at a hashed offset
/// ([`strike_offset`]) — a much faster cadence reads as a hyperactive storm.
const LIGHTNING_PERIOD_MS: u64 = 15000;
/// How long one strike's [`lightning_envelope`] window lasts.
const LIGHTNING_FLASH_MS: u64 = 90;

/// Intensity envelope (0..1) of a lightning flash given ms since the strike
/// began: primary strike → brief dim → after-flash, so it reads as a real
/// flicker rather than a single on/off blink. Returns 0 outside the flash.
fn lightning_envelope(since_strike_ms: u64) -> f32 {
    match since_strike_ms {
        0..=24 => 1.0,   // primary strike
        25..=39 => 0.15, // dim between flickers
        40..=69 => 0.55, // after-flash
        _ => 0.0,
    }
}

/// Per-bucket strike offset (ms into the bucket) so strikes don't fire on a
/// fixed metronome. Each `LIGHTNING_PERIOD_MS`-long bucket hashes to its own
/// offset in `[0, PERIOD - FLASH)`, keeping the whole flash inside the bucket.
fn strike_offset(bucket: u64) -> u64 {
    crate::splitmix_draw(bucket, 1) % (LIGHTNING_PERIOD_MS - LIGHTNING_FLASH_MS)
}

/// [`lightning_envelope`] for the clock at `now`, or 0 when not mid-strike —
/// whatever the weather; a painter only shows it under a storm.
fn flash_level_at(now: SystemTime) -> f32 {
    let elapsed_ms = crate::anim::epoch_ms(now);
    let bucket = elapsed_ms / LIGHTNING_PERIOD_MS;
    let phase = elapsed_ms % LIGHTNING_PERIOD_MS;
    match phase.checked_sub(strike_offset(bucket)) {
        Some(since) if since < LIGHTNING_FLASH_MS => lightning_envelope(since),
        _ => 0.0,
    }
}

/// The light the sky lets into the office: the interior illuminance and what
/// the window glass shows outside.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InteriorLight {
    /// Emitter light reaching the interior through the atmosphere, 0..=1.
    pub(crate) interior: f32,
    /// The glass's daylight: the interior plus the night's city-light floor, 0..=1.
    pub(crate) exterior: f32,
}

/// The sky at one instant, sampled once per frame.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Sky {
    weather: Weather,
    emitter: Emitter,
    moon_phase: f32,
    moon_waxing: bool,
    nightfall: f32,
    flash: f32,
}

impl Sky {
    pub(crate) fn at(now: SystemTime) -> Self {
        let (moon_phase, moon_age) = (moon_phase_at(now), moon_age_at(now));
        Self {
            weather: weather_at(now),
            emitter: emitter_at(now, moon_phase, moon_age),
            moon_phase,
            moon_waxing: moon_age < SYNODIC_DAYS / 2.0,
            nightfall: nightfall(local_hour_frac(now)),
            flash: flash_level_at(now),
        }
    }

    /// The sky at `now` under `weather`, whatever the clock picks — a test's
    /// fixed weather without the thread-local override.
    #[cfg(test)]
    pub(crate) fn at_with(now: SystemTime, weather: Weather) -> Self {
        Self {
            weather,
            ..Self::at(now)
        }
    }

    /// This sky with the lightning envelope at `flash` — a painter test's
    /// strike without the clock arithmetic that places one.
    #[cfg(test)]
    pub(crate) fn with_flash(self, flash: f32) -> Self {
        Self { flash, ..self }
    }

    pub(crate) fn weather(&self) -> Weather {
        self.weather
    }

    pub(crate) fn emitter(&self) -> &Emitter {
        &self.emitter
    }

    pub(crate) fn transmission(&self) -> Transmission {
        transmission(self.weather)
    }

    /// [`moon_phase_at`] at this instant.
    pub(crate) fn moon_phase(&self) -> f32 {
        self.moon_phase
    }

    /// Whether the moon is waxing (new to full) rather than waning at this
    /// instant: the side its lit limb faces.
    pub(crate) fn moon_waxing(&self) -> bool {
        self.moon_waxing
    }

    /// [`nightfall`] at this instant.
    pub(crate) fn nightfall(&self) -> f32 {
        self.nightfall
    }

    /// [`flash_level_at`] at this instant.
    pub(crate) fn flash(&self) -> f32 {
        self.flash
    }

    /// Direct-beam strength reaching the interior = emitter luminance carried by
    /// the weather's DIRECT transmission. Zero at night (the moon casts no usable
    /// beam) and under thick cloud.
    pub(crate) fn beam(&self) -> f32 {
        match self.emitter.body {
            Body::Sun => self.emitter.emitter_lum * self.transmission().direct,
            Body::Moon => 0.0,
        }
    }

    /// How hard it is raining, as a scalar (0.0 dry … 1.0 storm) — precipitation
    /// you can HEAR, so snow and fog are 0.0.
    pub(crate) fn precipitation(&self) -> f32 {
        // The gap to Storm is an audible "getting heavier", not a new mix profile.
        const RAIN_LEVEL: f32 = 0.6;
        match self.weather {
            Weather::Storm => 1.0,
            Weather::Rain => RAIN_LEVEL,
            _ => 0.0,
        }
    }

    pub(crate) fn light(&self) -> InteriorLight {
        let e = &self.emitter;
        let a = self.transmission();
        // The moon casts no USABLE direct beam (mirrors `beam`'s gate) — a
        // moonlit night must never out-light a cloudy solar noon, so the moon's
        // illuminance is diffuse-fill only.
        let direct_eff = match e.body {
            Body::Sun => a.direct,
            Body::Moon => 0.0,
        };
        let interior = (e.emitter_lum * (direct_eff * K_BEAM + a.diffuse * K_FILL)).clamp(0.0, 1.0);
        let night_floor = city_bounce(self.weather) * self.nightfall;
        InteriorLight {
            interior,
            exterior: (interior + night_floor).min(1.0),
        }
    }
}
