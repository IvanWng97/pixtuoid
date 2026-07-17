//! The playback seam. `AudioSink` is the ONE device boundary: tests (and
//! CI runners with no sound card) use [`NullSink`]; production uses the
//! rodio-backed sink behind the `audio` feature. The LISTEN gate's wav
//! renderer implements the same trait — everything above this line is
//! device-free.

use std::sync::Arc;

use super::mixer::LoopStem;

pub(crate) trait AudioSink: Send {
    /// Start `stem` looping `samples` (mono f32 @ 44_100) at gain 0.
    fn start_loop(&mut self, stem: LoopStem, samples: Arc<Vec<f32>>);
    /// Replace a looping stem's buffer (the #644 mood-track switch) — the
    /// caller guarantees the stem is at gain 0, so the cut is inaudible.
    fn swap_loop(&mut self, stem: LoopStem, samples: Arc<Vec<f32>>);
    /// Set a looping stem's gain (0..=1, already master-scaled).
    fn set_loop_gain(&mut self, stem: LoopStem, gain: f32);
    /// Fire-and-forget one-shot at `gain`.
    fn play_once(&mut self, samples: Arc<Vec<f32>>, gain: f32);
}

/// Records calls instead of making sound — the CI/test double. Keeps the
/// registered BUFFERS (not just the stem tags) so tests can pin that each
/// stem got the RIGHT bed — a `bed()` arm swap must not pass (review
/// finding: tag-only recording was blind to it).
#[cfg(test)]
#[derive(Default)]
pub(crate) struct NullSink {
    pub(crate) loops_started: Vec<LoopStem>,
    pub(crate) loop_samples: std::collections::HashMap<LoopStem, Arc<Vec<f32>>>,
    /// (stem, new buffer length) per swap — the #644 switch-machine pin.
    pub(crate) swaps: Vec<(LoopStem, usize)>,
    pub(crate) last_gain: std::collections::HashMap<LoopStem, f32>,
    pub(crate) one_shots: usize,
}

#[cfg(test)]
impl AudioSink for NullSink {
    fn start_loop(&mut self, stem: LoopStem, samples: Arc<Vec<f32>>) {
        self.loops_started.push(stem);
        self.loop_samples.insert(stem, samples);
    }
    fn swap_loop(&mut self, stem: LoopStem, samples: Arc<Vec<f32>>) {
        self.swaps.push((stem, samples.len()));
        self.loop_samples.insert(stem, samples);
    }
    fn set_loop_gain(&mut self, stem: LoopStem, gain: f32) {
        self.last_gain.insert(stem, gain);
    }
    fn play_once(&mut self, _samples: Arc<Vec<f32>>, gain: f32) {
        if gain > 0.0 {
            self.one_shots += 1;
        }
    }
}

/// The real device sink — rodio/cpal construction glue (winit-class:
/// needs real audio hardware, codecov-excluded, no unit tests; the LISTEN
/// gate + dogfood are its verification).
#[cfg(feature = "audio")]
pub(crate) mod rodio_sink {
    use super::*;
    use std::collections::HashMap;

    pub(crate) struct RodioSink {
        // field order = drop order: players release before the device sink
        loops: HashMap<LoopStem, rodio::Player>,
        stream: rodio::MixerDeviceSink,
    }

    impl RodioSink {
        /// `None` when no output device is available (headless boxes) —
        /// callers degrade to silence, never error the office.
        pub(crate) fn open() -> Option<Self> {
            match with_stderr_silenced(rodio::DeviceSinkBuilder::open_default_sink) {
                Ok(mut stream) => {
                    stream.log_on_drop(false);
                    Some(Self {
                        loops: HashMap::new(),
                        stream,
                    })
                }
                Err(e) => {
                    tracing::warn!("audio: no output device, running silent: {e}");
                    None
                }
            }
        }

        fn source_of(samples: &Arc<Vec<f32>>) -> rodio::buffer::SamplesBuffer {
            let mono = std::num::NonZero::new(1u16).expect("1 != 0");
            let rate = std::num::NonZero::new(super::super::dsp::SAMPLE_RATE).expect("44100 != 0");
            rodio::buffer::SamplesBuffer::new(mono, rate, samples.as_slice())
        }
    }

    /// Run `f` with fd 2 pointed at /dev/null (Unix): ALSA and friends
    /// print raw diagnostics to stderr during device open, and with the
    /// lazy spawn that happens MID-ALTSCREEN — one stray line corrupts the
    /// TUI (lowfi's first-ever issue was exactly this). rodio's own logs
    /// are already off via `log_on_drop(false)`.
    #[cfg(unix)]
    fn with_stderr_silenced<T>(f: impl FnOnce() -> T) -> T {
        // SAFETY: plain dup/dup2 fd shuffling; restored before returning.
        // A panic inside `f` would leak the redirect — acceptable for a
        // device-open that must not unwind (rodio returns Result).
        unsafe {
            let saved = libc::dup(2);
            if saved >= 0 {
                let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
                if devnull >= 0 {
                    libc::dup2(devnull, 2);
                    libc::close(devnull);
                }
            }
            let out = f();
            if saved >= 0 {
                libc::dup2(saved, 2);
                libc::close(saved);
            }
            out
        }
    }

    #[cfg(not(unix))]
    fn with_stderr_silenced<T>(f: impl FnOnce() -> T) -> T {
        f()
    }

    impl AudioSink for RodioSink {
        fn start_loop(&mut self, stem: LoopStem, samples: Arc<Vec<f32>>) {
            use rodio::Source;
            let player = rodio::Player::connect_new(self.stream.mixer());
            player.set_volume(0.0);
            player.append(Self::source_of(&samples).repeat_infinite());
            self.loops.insert(stem, player);
        }

        fn swap_loop(&mut self, stem: LoopStem, samples: Arc<Vec<f32>>) {
            // dropping the old Player stops it (the caller holds the stem
            // at gain 0 across the swap, so nothing audible is cut)
            if let Some(old) = self.loops.remove(&stem) {
                old.stop();
            }
            self.start_loop(stem, samples);
        }

        fn set_loop_gain(&mut self, stem: LoopStem, gain: f32) {
            if let Some(player) = self.loops.get(&stem) {
                player.set_volume(gain);
            }
        }

        fn play_once(&mut self, samples: Arc<Vec<f32>>, gain: f32) {
            if gain <= 0.0 {
                return;
            }
            let player = rodio::Player::connect_new(self.stream.mixer());
            player.set_volume(gain);
            player.append(Self::source_of(&samples));
            player.detach();
        }
    }
}
