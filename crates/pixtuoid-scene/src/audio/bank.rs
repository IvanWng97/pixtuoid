//! The pre-rendered sample banks — the ONE place the office's sounds are
//! synthesized into buffers, for the native rodio gateway and the wasm WebAudio
//! painter alike: the same synthesis, not the same bits, as [`f32::sin`]'s and
//! [`f32::exp`]'s precision varies by platform. Pure: no device deps.
//!
//! The `rng` DRAW ORDER is the sound — [`Synthesis`] draws the one-shot pools,
//! then rain, then each track's lanes from ONE stream in the ratified order, so
//! every buffer matches the LISTEN-ratified renders. Don't reorder its calls.

use std::sync::Arc;

use super::mixer::LoopStem;
use super::{OneShot, TrackId, dsp, synth};

/// Per-key / per-drop variant pool sizes: playback picks randomly so
/// typing/rain never sound repeated, while runtime stays synthesis-free.
pub const KEYSTROKE_POOL: usize = 16;
pub const DROP_POOL: usize = 12;

/// One-shot playback gains relative to master — loudness-matched unit levels.
pub(crate) const KEYSTROKE_GAIN: f32 = 0.35;
pub(crate) const ONE_SHOT_GAIN: f32 = 0.5;
/// Foreground raindrops sit well ABOVE the wash per the reference; the bed
/// peaks under 1.0, so drops ride at the rain level itself.
pub(crate) const DROP_GAIN: f32 = 0.9;

/// The six TRACK-OWNED loop stems, in lane render order. Rain is not here — it
/// is weather, shared by every mood track.
pub const TRACK_STEMS: [LoopStem; synth::Lane::ALL.len()] = {
    let mut out = [LoopStem::Pad; synth::Lane::ALL.len()];
    let mut i = 0;
    while i < out.len() {
        out[i] = stem_of(synth::Lane::ALL[i]);
        i += 1;
    }
    out
};

/// The stem each music lane plays on — exhaustive, so a new lane fails to
/// compile until it has one.
const fn stem_of(lane: synth::Lane) -> LoopStem {
    match lane {
        synth::Lane::Pad => LoopStem::Pad,
        synth::Lane::Sparkle => LoopStem::Sparkle,
        synth::Lane::Keys => LoopStem::Keys,
        synth::Lane::Drums => LoopStem::Drums,
        synth::Lane::Texture => LoopStem::Texture,
        synth::Lane::Bass => LoopStem::Bass,
    }
}

/// Which one-shot pool a play draws from — the ONE vocabulary both backends
/// share: the native gateway resolves it to a buffer via [`AssetBank::sample`],
/// the wasm painter sends `(wire, index)` to JS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OneShotPool {
    Keystroke,
    Drop,
    DoorChime,
    PrinterWhir,
    VendingDrop,
}

impl OneShotPool {
    /// Which appliance-cue pool a scene [`OneShot`] plays from.
    pub(crate) fn from_event(event: OneShot) -> Self {
        match event {
            OneShot::DoorChime => OneShotPool::DoorChime,
            OneShot::PrinterWhir => OneShotPool::PrinterWhir,
            OneShot::VendingDrop => OneShotPool::VendingDrop,
        }
    }

    /// Stable index for the wasm JSON wire (JS maps it back to its buffer bank).
    pub fn wire(self) -> u8 {
        match self {
            OneShotPool::Keystroke => 0,
            OneShotPool::Drop => 1,
            OneShotPool::DoorChime => 2,
            OneShotPool::PrinterWhir => 3,
            OneShotPool::VendingDrop => 4,
        }
    }

    /// Every pool in wire order — a new pool is declared in exactly one place.
    pub const ALL: [OneShotPool; 5] = [
        OneShotPool::Keystroke,
        OneShotPool::Drop,
        OneShotPool::DoorChime,
        OneShotPool::PrinterWhir,
        OneShotPool::VendingDrop,
    ];

    /// The inverse of [`OneShotPool::wire`] (`None` for an unknown wire).
    pub fn from_wire(wire: u8) -> Option<Self> {
        OneShotPool::ALL.into_iter().find(|p| p.wire() == wire)
    }
}

/// The ONE-SHOT pools a player keeps for its whole life. The loop beds live in
/// [`TrackBeds`] instead and are NOT retained — `RodioSink` copies each into its
/// own `SamplesBuffer`, so holding the Arcs would double the bed RAM.
#[derive(Debug)]
pub struct AssetBank {
    keystrokes: Vec<Arc<Vec<f32>>>,
    drops: Vec<Arc<Vec<f32>>>,
    door_chime: Arc<Vec<f32>>,
    printer_whir: Arc<Vec<f32>>,
    vending_drop: Arc<Vec<f32>>,
}

impl AssetBank {
    pub fn build(rng: &mut dsp::NoiseStream) -> Self {
        Self {
            keystrokes: (0..KEYSTROKE_POOL)
                .map(|_| Arc::new(synth::keystroke(rng)))
                .collect(),
            drops: (0..DROP_POOL)
                .map(|_| Arc::new(synth::rain_drop(rng)))
                .collect(),
            door_chime: Arc::new(synth::door_chime()),
            printer_whir: Arc::new(synth::printer_whir(rng)),
            vending_drop: Arc::new(synth::vending_drop(rng)),
        }
    }

    /// A bank of buffers built elsewhere (the wasm worker's handoff); `None`
    /// unless each pool holds its full size.
    pub fn adopt(
        keystrokes: Vec<Arc<Vec<f32>>>,
        drops: Vec<Arc<Vec<f32>>>,
        door_chime: Arc<Vec<f32>>,
        printer_whir: Arc<Vec<f32>>,
        vending_drop: Arc<Vec<f32>>,
    ) -> Option<Self> {
        (keystrokes.len() == KEYSTROKE_POOL && drops.len() == DROP_POOL).then_some(Self {
            keystrokes,
            drops,
            door_chime,
            printer_whir,
            vending_drop,
        })
    }

    /// The `index`th buffer of `pool`, `None` past its end; an appliance pool
    /// holds one.
    pub fn get(&self, pool: OneShotPool, index: usize) -> Option<&Arc<Vec<f32>>> {
        match pool {
            OneShotPool::Keystroke => self.keystrokes.get(index),
            OneShotPool::Drop => self.drops.get(index),
            OneShotPool::DoorChime => (index == 0).then_some(&self.door_chime),
            OneShotPool::PrinterWhir => (index == 0).then_some(&self.printer_whir),
            OneShotPool::VendingDrop => (index == 0).then_some(&self.vending_drop),
        }
    }

    /// Resolve an engine-emitted `(pool, index)` play to its buffer, `index`
    /// taken modulo the pool's size.
    pub fn sample(&self, pool: OneShotPool, index: usize) -> Arc<Vec<f32>> {
        let size = match pool {
            OneShotPool::Keystroke => self.keystrokes.len(),
            OneShotPool::Drop => self.drops.len(),
            OneShotPool::DoorChime | OneShotPool::PrinterWhir | OneShotPool::VendingDrop => 1,
        };
        // `build` and `adopt` fill every pool; an empty one would play silence.
        index
            .checked_rem(size)
            .and_then(|i| self.get(pool, i))
            .map_or_else(Default::default, Arc::clone)
    }
}

/// One mood track's loop beds — built by [`Synthesis`] per [`TrackId`], registered (or swapped
/// in) with the sink, then DROPPED. The five musical beds and the NIGHT texture
/// share ONE sample count (phase-locked); the DAY texture keeps its
/// free-running power-of-two length.
#[derive(Debug)]
pub struct TrackBeds {
    beds: [Arc<Vec<f32>>; TRACK_STEMS.len()],
}

impl TrackBeds {
    /// Compose the id's take (the seed is the track-epoch block) and render it
    /// through the SAME cores the owner-ratified takes were built on.
    pub fn build(rng: &mut dsp::NoiseStream, track: TrackId) -> Self {
        let score = super::compose_track(track);
        Self {
            beds: synth::Lane::ALL.map(|lane| Arc::new(synth::render_lane(&score, lane, rng))),
        }
    }

    /// Assemble from beds already built lane-by-lane (the wasm driver's chunked
    /// rebuild) — `TRACK_STEMS` order.
    pub fn from_arcs(beds: [Arc<Vec<f32>>; TRACK_STEMS.len()]) -> Self {
        Self { beds }
    }

    pub fn bed(&self, stem: LoopStem) -> Arc<Vec<f32>> {
        Arc::clone(&self.beds[self.index(stem)])
    }

    /// The bed samples as a borrow tied to `&self` — for a consumer that reads
    /// but does not retain the buffer, where an `Arc` clone's slice would dangle.
    pub fn bed_slice(&self, stem: LoopStem) -> &[f32] {
        &self.beds[self.index(stem)]
    }

    fn index(&self, stem: LoopStem) -> usize {
        TRACK_STEMS
            .iter()
            .position(|s| *s == stem)
            .expect("every track stem has a bed")
    }
}

/// The office's synthesis in its one ratified order: the one-shot pools, then
/// rain, then each track's lanes as tracks are needed — one rng stream, so the
/// order is the sound. Every painter builds through here.
#[derive(Debug)]
pub struct Synthesis {
    rng: dsp::NoiseStream,
}

impl Default for Synthesis {
    fn default() -> Self {
        Self::new()
    }
}

impl Synthesis {
    pub fn new() -> Self {
        Self {
            rng: dsp::NoiseStream::new(super::BUILD_SEED),
        }
    }

    pub fn assets(&mut self) -> AssetBank {
        AssetBank::build(&mut self.rng)
    }

    pub fn rain(&mut self) -> Arc<Vec<f32>> {
        Arc::new(synth::rain_bed(&mut self.rng))
    }

    /// Every lane of `track` at once — the native thread, which may block.
    pub fn track(&mut self, track: TrackId) -> TrackBeds {
        let mut build = TrackBuild::new(track);
        loop {
            if let Some(beds) = build.step(self) {
                return beds;
            }
        }
    }
}

/// A track built one lane per [`TrackBuild::step`] — the wasm driver spreads a
/// build across ticks, because every lane in one rAF tick hitches the page.
#[derive(Debug)]
pub struct TrackBuild {
    score: super::compose::Score,
    beds: Vec<Arc<Vec<f32>>>,
}

impl TrackBuild {
    pub fn new(track: TrackId) -> Self {
        Self {
            score: super::compose_track(track),
            beds: Vec::with_capacity(TRACK_STEMS.len()),
        }
    }

    /// Render the next lane; the finished beds once the last one is in.
    pub fn step(&mut self, synthesis: &mut Synthesis) -> Option<TrackBeds> {
        let lane = synth::Lane::ALL[self.beds.len()];
        self.beds.push(Arc::new(synth::render_lane(
            &self.score,
            lane,
            &mut synthesis.rng,
        )));
        if self.beds.len() < TRACK_STEMS.len() {
            return None;
        }
        <[Arc<Vec<f32>>; TRACK_STEMS.len()]>::try_from(std::mem::take(&mut self.beds))
            .ok()
            .map(|beds| TrackBeds { beds })
    }
}

/// Whether every TRACK-owned stem's live gain has reached exactly 0.0 — the
/// silence gate a player checks before swapping a mood track's beds. Rain/typing
/// gains are ignored (track-independent).
pub(crate) fn track_stems_silent(gains: &[(LoopStem, f32)]) -> bool {
    gains
        .iter()
        .filter(|(s, _)| TRACK_STEMS.contains(s))
        .all(|(_, g)| *g == 0.0)
}

#[cfg(test)]
mod tests {
    use super::super::compose::{Mood, compose};
    use super::*;

    /// Both sinks register track stems in this order and the web wire indexes
    /// `LoopStem::ALL` by it, so the derivation must not reorder them.
    #[test]
    fn track_stems_keep_their_registration_order() {
        assert_eq!(
            TRACK_STEMS,
            [
                LoopStem::Pad,
                LoopStem::Sparkle,
                LoopStem::Keys,
                LoopStem::Drums,
                LoopStem::Texture,
                LoopStem::Bass,
            ]
        );
        for (lane, stem) in synth::Lane::ALL.into_iter().zip(TRACK_STEMS) {
            assert_eq!(stem_of(lane), stem);
        }
    }

    #[test]
    fn rain_bed_is_one_power_of_two_loop() {
        assert_eq!(
            synth::rain_bed(&mut dsp::NoiseStream::new(1)).len(),
            1 << 19
        );
    }

    #[test]
    fn a_mood_swap_never_touches_the_rain_stem() {
        assert!(!TRACK_STEMS.contains(&LoopStem::Rain));
    }

    /// The one ratified order, rebuilt by hand from the same functions in this
    /// process: pools, then rain, then each track's lanes, one stream.
    #[test]
    fn synthesis_draws_in_the_ratified_order() {
        let mut s = Synthesis::new();
        let assets = s.assets();
        let rain = s.rain();
        let day = s.track(TrackId::GenDay(0));
        let night = s.track(TrackId::GenNight(0));

        let mut rng = dsp::NoiseStream::new(crate::audio::BUILD_SEED);
        let want_assets = AssetBank::build(&mut rng);
        let want_rain = synth::rain_bed(&mut rng);
        let mut lanes = |track| {
            let score = super::super::compose_track(track);
            synth::Lane::ALL.map(|lane| synth::render_lane(&score, lane, &mut rng))
        };
        let want_day = lanes(TrackId::GenDay(0));
        let want_night = lanes(TrackId::GenNight(0));

        for pool in OneShotPool::ALL {
            for i in 0..KEYSTROKE_POOL.max(DROP_POOL) {
                assert_eq!(
                    assets.get(pool, i),
                    want_assets.get(pool, i),
                    "{pool:?}[{i}]"
                );
            }
        }
        assert_eq!(*rain, want_rain);
        for (i, stem) in TRACK_STEMS.into_iter().enumerate() {
            assert_eq!(day.bed_slice(stem), want_day[i].as_slice(), "day {stem:?}");
            assert_eq!(
                night.bed_slice(stem),
                want_night[i].as_slice(),
                "night {stem:?}"
            );
        }
    }

    /// The web's chunked build: one lane per step, the same beds as a whole
    /// build, and a fresh build after it finishes rather than an index past the end.
    #[test]
    fn a_stepped_build_matches_a_whole_one_and_restarts_clean() {
        let whole = Synthesis::new().track(TrackId::GenNight(3));
        let mut s = Synthesis::new();
        let mut build = TrackBuild::new(TrackId::GenNight(3));
        let mut steps = 0;
        let stepped = loop {
            steps += 1;
            if let Some(beds) = build.step(&mut s) {
                break beds;
            }
        };
        assert_eq!(steps, TRACK_STEMS.len(), "one lane per step");
        for stem in TRACK_STEMS {
            assert_eq!(whole.bed_slice(stem), stepped.bed_slice(stem), "{stem:?}");
        }
        assert!(
            build.step(&mut s).is_none(),
            "a finished build starts over at lane 0"
        );
    }

    /// Each track id composes its own mood from its own seed — wiring, not content.
    #[test]
    fn a_track_id_composes_its_mood_and_seed() {
        for seed in [0, 7] {
            assert_eq!(
                super::super::compose_track(TrackId::GenDay(seed)),
                compose(Mood::Day, seed)
            );
            assert_eq!(
                super::super::compose_track(TrackId::GenNight(seed)),
                compose(Mood::Night, seed)
            );
        }
    }
}
