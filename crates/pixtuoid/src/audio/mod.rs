//! Ambient office audio — the ONE consumer of the scene's
//! `pixtuoid_scene::audio::AudioFrame` model and the only owner of any
//! audio-device dependency (#633; the plan's single-gateway rule). Pure
//! synthesis (`dsp`/`synth`) pre-renders every sample buffer at startup;
//! playback rides its own thread behind a bounded channel and never blocks
//! the render loop.

pub(crate) mod dsp;
pub(crate) mod synth;
