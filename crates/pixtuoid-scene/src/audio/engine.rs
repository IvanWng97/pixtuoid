//! The per-frame audio engine — the ONE place the office's sound is advanced
//! each tick, so the native rodio gateway (`pixtuoid`) and the wasm WebAudio
//! painter (`pixtuoid-web`) run byte-identical mixing/crossfade/scheduling
//! logic instead of two hand-kept-in-sync copies (#633 web-audio deepened the
//! shared surface from just [`TrackSwitch`] to the whole tick).
//!
//! It owns ONLY per-tick STATE (mixer, schedulers, the switch machine). The
//! BUILD — synthesizing a track's beds — stays caller-side, because that is the
//! one thing the two backends genuinely do differently (native blocks the audio
//! thread under the crossfade silence; web chunks it across `warmup_step` /
//! rebuilds in one tick under the hold). So [`AudioEngine::tick`] SIGNALS a
//! build via [`TickCommands::swap`]; the caller builds + registers the beds.
//!
//! Time is a PARAMETER: `tick` takes `dt` and never reads a clock. Each shell
//! clamps `dt` to [`MAX_DT_S`] before calling, so a native track-build stall or
//! a backgrounded wasm tab neither snaps the crossfade nor bursts the
//! schedulers — the gap-immunity is identical on both backends.

use super::bank::{
    track_stems_silent, OneShotPool, DROP_GAIN, DROP_POOL, KEYSTROKE_GAIN, KEYSTROKE_POOL,
    ONE_SHOT_GAIN,
};
use super::dsp::NoiseStream;
use super::mixer::{DropScheduler, LoopStem, Mixer, TypingScheduler};
use super::{
    AudioFrame, OneShot, StemLevels, TrackId, TrackSwitch, DROP_SEED, PICK_SEED, TYPING_SEED,
};

/// dt ceiling (s): a bigger inter-tick gap (a native track-build stall, a
/// backgrounded wasm tab, a GC pause) is clamped so one tick can neither snap
/// the crossfade nor burst-replay the schedulers. BOTH shells clamp to it, so
/// the gap-immunity is identical on native and web.
pub const MAX_DT_S: f32 = 0.10;

/// One one-shot to spawn this tick: a fresh source from `(pool, index)` at
/// `gain` (already master-scaled; the native sink drops `gain <= 0`, the wasm
/// glue sends it to JS verbatim).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayCmd {
    pub pool: OneShotPool,
    pub index: usize,
    pub gain: f32,
}

/// What one [`AudioEngine::tick`] produces — the backend-agnostic result each
/// shell flushes its own way (native → `device.*` calls, wasm → JSON for JS).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TickCommands {
    /// Target gain per LOOP stem, in `LoopStem::ALL` order (0=Pad … 5=Rain).
    pub gains: [f32; LoopStem::ALL.len()],
    /// One-shots to fire this tick (scene cues + typing/rain schedulers).
    pub plays: Vec<PlayCmd>,
    /// A mood-track change that just committed — the caller builds `swap`'s beds
    /// and swaps them into the sink. `None` on an ordinary tick.
    pub swap: Option<TrackId>,
}

/// The per-tick audio state both backends share. Constructed once; `init_track`
/// once the caller has built the first track's beds; then `tick` per frame.
pub struct AudioEngine {
    mixer: Mixer,
    typing: TypingScheduler,
    drops: DropScheduler,
    /// One-shot variant picker — its draw order is part of the ratified sound.
    pick: NoiseStream,
    switch: TrackSwitch,
    typing_level: f32,
    rain_level: f32,
    /// The last frame's target levels — held across a timeout tick (`None`).
    wanted: StemLevels,
    /// Monotonic scheduler clock (s), advanced by the passed (clamped) dt.
    sched_s: f64,
}

impl AudioEngine {
    /// A fresh engine at `master` volume (native reads the config volume; wasm
    /// passes the fixed 1.0 trimmed bus). Inert until [`AudioEngine::init_track`].
    pub fn new(master: f32) -> Self {
        Self {
            mixer: Mixer::new(master),
            typing: TypingScheduler::new(TYPING_SEED),
            drops: DropScheduler::new(DROP_SEED),
            pick: NoiseStream::new(PICK_SEED),
            switch: TrackSwitch::new(),
            typing_level: 0.0,
            rain_level: 0.0,
            wanted: StemLevels::default(),
            sched_s: 0.0,
        }
    }

    /// Adopt the initial mood track — call ONCE, after the caller has built and
    /// registered its beds. Before this, `tick` is inert (silence). Idempotent.
    pub fn init_track(&mut self, track: TrackId) {
        self.switch.init(track);
    }

    /// Live mute (native `m` key). No-op on wasm (mute = JS suspends the context).
    pub fn set_muted(&mut self, muted: bool) {
        self.mixer.set_muted(muted);
    }

    /// Live master volume (native `+/-`). No-op on wasm (fixed 1.0 bus).
    pub fn set_master(&mut self, master: f32) {
        self.mixer.set_master(master);
    }

    /// Advance one tick by `dt` seconds. `frame` is the scene's audio intent —
    /// `None` on a native `recv_timeout` (no new levels/events; the ramp and
    /// schedulers still advance from the held state). Returns the loop gains,
    /// the one-shots to fire, and any committed track swap (whose beds the
    /// caller then builds).
    pub fn tick(&mut self, dt: f32, frame: Option<AudioFrame>) -> TickCommands {
        self.sched_s += dt as f64;

        let events: Vec<OneShot> = if let Some(f) = frame {
            self.typing_level = f.stems.typing;
            self.rain_level = f.stems.rain;
            self.wanted = f.stems;
            self.switch.request(f.track); // no-op before init / on an unchanged track
            f.events
        } else {
            Vec::new()
        };

        // Inert until a caller has built the initial beds and called init_track
        // (matches the wasm painter's empty command set before warmup completes).
        if self.switch.current().is_none() {
            return TickCommands::default();
        }

        // Hold the five track stems silent while a switch settles; rain/typing
        // keep following the scene (weather + activity are track-independent).
        let mut target = self.wanted;
        if self.switch.is_holding() {
            target.silence_track_stems();
        }
        self.mixer.set_target(target);
        let gain_pairs = self.mixer.step(dt);

        // Once the held track stems reach silence, commit the pending swap — the
        // caller builds `to`'s beds and swaps them under the silence.
        let swap = self.switch.try_swap(track_stems_silent(&gain_pairs));

        let mut gains = [0.0f32; LoopStem::ALL.len()];
        for (i, (_, g)) in gain_pairs.iter().enumerate() {
            gains[i] = *g;
        }

        // One-shots: the scene's fired cues + the typing/rain schedulers. Muted
        // rides through `one_shot_gain` = 0 (plays still enqueue at gain 0, the
        // sink drops them) so the schedulers' clocks never desync on mute.
        let mut plays = Vec::new();
        let os_gain = self.mixer.one_shot_gain();
        for event in events {
            plays.push(PlayCmd {
                pool: OneShotPool::from_event(event),
                index: 0,
                gain: ONE_SHOT_GAIN * os_gain,
            });
        }
        for _ in 0..self.typing.tick(self.sched_s, self.typing_level) {
            let index = (self.pick.unit() * KEYSTROKE_POOL as f32) as usize % KEYSTROKE_POOL;
            plays.push(PlayCmd {
                pool: OneShotPool::Keystroke,
                index,
                gain: KEYSTROKE_GAIN * os_gain,
            });
        }
        for _ in 0..self.drops.tick(self.sched_s, self.rain_level) {
            let index = (self.pick.unit() * DROP_POOL as f32) as usize % DROP_POOL;
            plays.push(PlayCmd {
                pool: OneShotPool::Drop,
                index,
                gain: DROP_GAIN * self.rain_level * os_gain,
            });
        }

        TickCommands { gains, plays, swap }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn busy_frame(track: TrackId) -> AudioFrame {
        AudioFrame {
            stems: StemLevels {
                pad: 0.8,
                sparkle: 0.6,
                keys: 0.5,
                drums: 0.5,
                texture: 0.4,
                rain: 0.0,
                typing: 0.8,
            },
            events: Vec::new(),
            track,
        }
    }

    /// Run `ticks` 50ms frames of the given track to settle the mix.
    fn settle(engine: &mut AudioEngine, track: TrackId, ticks: usize) {
        for _ in 0..ticks {
            engine.tick(0.05, Some(busy_frame(track)));
        }
    }

    #[test]
    fn is_inert_until_init_track() {
        let mut e = AudioEngine::new(1.0);
        let cmd = e.tick(0.05, Some(busy_frame(TrackId::Day)));
        assert!(cmd.gains.iter().all(|g| *g == 0.0), "no gains before init");
        assert!(cmd.plays.is_empty(), "no plays before init");
        assert!(cmd.swap.is_none(), "no swap before init");
        // once inited, the same frame ramps
        e.init_track(TrackId::Day);
        let cmd = e.tick(0.05, Some(busy_frame(TrackId::Day)));
        assert!(cmd.gains[0] > 0.0, "pad ramps once inited");
    }

    #[test]
    fn ramps_loop_gains_and_fires_typing_for_a_busy_office() {
        let mut e = AudioEngine::new(1.0);
        e.init_track(TrackId::Day);
        let mut typed = 0;
        let mut last_pad = 0.0;
        for _ in 0..200 {
            let cmd = e.tick(0.05, Some(busy_frame(TrackId::Day)));
            last_pad = cmd.gains[0];
            typed += cmd
                .plays
                .iter()
                .filter(|p| p.pool == OneShotPool::Keystroke)
                .count();
        }
        assert!(last_pad > 0.0, "pad ramped up");
        assert!(typed > 0, "the typing scheduler fired keystrokes");
    }

    #[test]
    fn mute_zeroes_every_loop_gain_and_one_shot() {
        let mut e = AudioEngine::new(1.0);
        e.init_track(TrackId::Day);
        settle(&mut e, TrackId::Day, 200);
        e.set_muted(true);
        for _ in 0..200 {
            e.tick(0.05, Some(busy_frame(TrackId::Day)));
        }
        let cmd = e.tick(0.05, Some(busy_frame(TrackId::Day)));
        assert!(cmd.gains.iter().all(|g| *g == 0.0), "muted loops fall to 0");
        assert!(
            cmd.plays.iter().all(|p| p.gain == 0.0),
            "muted one-shots ride at gain 0 (sink drops them)"
        );
    }

    #[test]
    fn master_zero_silences_the_bus_and_raising_it_ramps_back() {
        let mut e = AudioEngine::new(0.0);
        e.init_track(TrackId::Day);
        for _ in 0..200 {
            e.tick(0.05, Some(busy_frame(TrackId::Day)));
        }
        let cmd = e.tick(0.05, Some(busy_frame(TrackId::Day)));
        assert!(cmd.gains.iter().all(|g| *g == 0.0), "master 0 = silent");
        e.set_master(1.0);
        let mut pad = 0.0;
        for _ in 0..200 {
            pad = e.tick(0.05, Some(busy_frame(TrackId::Day))).gains[0];
        }
        assert!(pad > 0.0, "master up ramps the mix back");
    }

    #[test]
    fn a_timeout_tick_advances_without_consuming_levels_or_events() {
        // native passes None on a recv_timeout: no new levels, no scene events,
        // but the ramp + schedulers still advance from the held state.
        let mut e = AudioEngine::new(1.0);
        e.init_track(TrackId::Day);
        settle(&mut e, TrackId::Day, 100);
        let cmd = e.tick(0.05, None);
        assert!(
            cmd.plays.iter().all(|p| !matches!(
                p.pool,
                OneShotPool::DoorChime | OneShotPool::PrinterWhir | OneShotPool::VendingDrop
            )),
            "a None tick fires no scene one-shots"
        );
        assert!(
            cmd.gains[0] > 0.0,
            "the held pad keeps ramping on a None tick"
        );
    }

    #[test]
    fn scene_events_become_one_shot_plays() {
        let mut e = AudioEngine::new(1.0);
        e.init_track(TrackId::Day);
        settle(&mut e, TrackId::Day, 20);
        let mut frame = busy_frame(TrackId::Day);
        frame.events = vec![OneShot::DoorChime, OneShot::VendingDrop];
        let cmd = e.tick(0.05, Some(frame));
        assert!(cmd.plays.iter().any(|p| p.pool == OneShotPool::DoorChime));
        assert!(cmd.plays.iter().any(|p| p.pool == OneShotPool::VendingDrop));
    }

    #[test]
    fn a_track_change_holds_silent_then_swaps_then_ramps_back_as_a_slew() {
        let mut e = AudioEngine::new(1.0);
        e.init_track(TrackId::Day);
        settle(&mut e, TrackId::Day, 80);
        let mut swap_track = None;
        for _ in 0..200 {
            let cmd = e.tick(0.05, Some(busy_frame(TrackId::Night)));
            if let Some(to) = cmd.swap {
                swap_track = Some(to);
                for g in &cmd.gains[0..5] {
                    assert!(*g <= 1e-6, "track stems held silent through the swap");
                }
                break;
            }
        }
        assert_eq!(swap_track, Some(TrackId::Night), "the night switch swapped");
        // the ramp back is a slew, never a snap (the bot HIGH: a stalled clock
        // once made dt cover the ~2s synth and snapped gains straight to target)
        let mut first_nonzero = 0.0;
        for _ in 0..200 {
            let g = e.tick(0.05, Some(busy_frame(TrackId::Night))).gains[0];
            if g > 0.0 {
                first_nonzero = g;
                break;
            }
        }
        assert!(
            first_nonzero > 0.0 && first_nonzero < 0.1,
            "post-swap gain fades in (first step {first_nonzero}), not a snap"
        );
    }

    #[test]
    fn the_dt_clamp_ceiling_stays_a_slew_while_an_unclamped_gap_would_snap() {
        // WHY both shells clamp dt to MAX_DT_S: one tick at the ceiling is a
        // SLEW, but an unclamped multi-second gap (a native track-build stall, a
        // backgrounded wasm tab) would snap the gains straight to target — the
        // "bot HIGH" pop. Pins the ceiling VALUE, not just the shells' clamp.
        let one_tick = |dt: f32| {
            let mut e = AudioEngine::new(1.0);
            e.init_track(TrackId::Day);
            e.tick(dt, Some(busy_frame(TrackId::Day))).gains[0]
        };
        let mut settled = AudioEngine::new(1.0);
        settled.init_track(TrackId::Day);
        let mut full = 0.0;
        for _ in 0..500 {
            full = settled.tick(0.05, Some(busy_frame(TrackId::Day))).gains[0];
        }
        assert!(
            one_tick(MAX_DT_S) < full * 0.5,
            "one clamped-ceiling tick is a slew, well under the settled target {full}"
        );
        assert!(
            (one_tick(2.0) - full).abs() < 1e-6,
            "an unclamped multi-second gap snaps straight to the target"
        );
    }
}
