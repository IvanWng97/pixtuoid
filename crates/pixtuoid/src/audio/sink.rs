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
    /// Set a looping stem's gain (0..=1, already master-scaled).
    fn set_loop_gain(&mut self, stem: LoopStem, gain: f32);
    /// Fire-and-forget one-shot at `gain`.
    fn play_once(&mut self, samples: Arc<Vec<f32>>, gain: f32);
}

/// Records calls instead of making sound — the CI/test double.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct NullSink {
    pub(crate) loops_started: Vec<LoopStem>,
    pub(crate) last_gain: std::collections::HashMap<LoopStem, f32>,
    pub(crate) one_shots: usize,
}

#[cfg(test)]
impl AudioSink for NullSink {
    fn start_loop(&mut self, stem: LoopStem, _samples: Arc<Vec<f32>>) {
        self.loops_started.push(stem);
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
            match rodio::DeviceSinkBuilder::open_default_sink() {
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

    impl AudioSink for RodioSink {
        fn start_loop(&mut self, stem: LoopStem, samples: Arc<Vec<f32>>) {
            use rodio::Source;
            let player = rodio::Player::connect_new(self.stream.mixer());
            player.set_volume(0.0);
            player.append(Self::source_of(&samples).repeat_infinite());
            self.loops.insert(stem, player);
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
