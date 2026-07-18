---
name: procedural-lofi
version: 1.0.0
description: "Generate a royalty-free lofi (or rain / typing / chime / any ambient) soundtrack ENTIRELY in code — no sampled audio ships. Fingerprint a beloved reference recording, shape synthesis to the measured spectral + temporal curve, freeze one human-blessed take into constant tables, and re-synthesize at launch (loop the bed, scatter the foreground → never repeats, ~0 KB, no licensing risk). Use when adding ambient/generative audio to an app, game, terminal UI, or site, or on 'add another lofi/rain/ambient sound'. Bundles the parameter tables (LOFI-BIBLE.md) + the numpy fingerprint/synth/freeze pipeline."
---

# Procedural Lofi — build a soundtrack in code, not in a sample pack

This skill is the end-to-end recipe for a lofi bed (and its sibling ambient sounds:
rain, keystrokes, door chimes, printer, water cooler…) that is **synthesized at
runtime from constants** — zero audio files, zero royalties, and no two minutes ever
sound the same. It was proven out on a real shipping product (an animated pixel-art
office that plays ambient sound scaled by how busy the on-screen agents are), then
distilled here.

Two documents ship alongside this one:

- **`reference/LOFI-BIBLE.md`** — the parameter tables. Harmony (chord grammar,
  voice-leading, register clamps), groove (swing %, drag ms, velocity curves), per-voice
  sound design (pad / bass / EP keys / sparkle / drums / texture / tape chain), mix
  targets (band shares, HPF strategy, loudness), and generative lessons from prior art.
  Every number is cited and, where it conflicts with a measurement, the measurement wins.
- **`scripts/`** — the Python (numpy) pipeline: `analyze_*.py` (fingerprint a reference),
  `synth_audition.py` (a runnable numpy synth that produces `.wav` auditions), and
  `export_score.py` (freeze a good take into a constants table for the port).

Read those two when you reach the step that needs them. This file is the map.

---

## The core idea (why code, not samples)

A sampled lofi loop is: (a) a licensing liability, (b) heavy to bundle, and (c) audibly
repetitive — the human brain catches a loop seam on the 3rd or 4th pass. Synthesizing it
solves all three:

- **Legal**: *acoustic parameters are not copyrightable.* You measure a reference to get
  target numbers (this band has 55% of the power, the centroid sits at 150 Hz, drops land
  13 dB above the bed). You never keep or ship the reference bytes. What ships is your own
  oscillator code hitting those numbers.
- **Size**: a few kilobytes of constant tables + a synth function vs. megabytes of PCM.
- **Never repeats (in practice)**, from three stacked tricks: (1) the frozen musical
  composition — pad, drums, *and* the melody (keys + sparkle) — loops in lockstep, but the
  loop is made **long enough** that its repetition doesn't fatigue (this project doubled its
  day loop from 4 to 8 bars precisely because a short looped melody *was* audibly repetitive
  — "loop the bed" does not mean "make it short"); (2) the genuinely-stochastic layers —
  **rain drops and keystrokes** — *are* scattered fresh at runtime from a seeded RNG, laid
  over the loop; (3) a **busy-ness stem mixer** fades whole stems (drums, keys…) in and out
  by how active the scene is, so the *arrangement* keeps changing even though the notes
  don't. The result reads as "never the same two minutes" without regenerating harmony.
  (Regenerating melody live is possible but it's the hard, optional part — the shipped
  product froze the melody and leaned on 1–3 instead.)

The one law of the whole method:

> **Measurement is the machine's ears; taste is the human's.** You (or your tooling)
> cannot reliably *hear* whether it sounds good — you can only measure proxies (band
> energies, onset rate, dB deltas) and drive them to a target. A human listens **once per
> major revision** and gives a yes/no. Pick your measurable proxy carefully, iterate on it
> alone, and hand over a finished audition — don't ask the human to babysit each tweak.

---

## The pipeline (five phases)

### Phase 0 — Audition in a scripting language first (numpy), NOT in your ship language

Prototype the whole sound in Python/numpy where the write→hear loop is seconds. Only port
to Rust/C++/wasm once a human has ratified the *sound*. `scripts/synth_audition.py` is a
worked example: deterministic numpy that writes `audio-demos/*.wav`. Iterating synth
recipes in a compiled language first is the classic time sink.

### Phase 1 — Reference-fingerprint each sound

For every distinct sound (the lofi bed, rain, typing, each one-shot):

1. **Get a beloved reference.** An owner-supplied reference beats a "community ideal"
   every time — build to what *they* love, not to what a forum says lofi should be.
   (`yt-dlp` / `curl` for analysis only; for an endless live stream, `yt-dlp -g` gets the
   HLS URL, then `ffmpeg -t 180` grabs a finite slice.)
2. **Confirm the CHARACTER before deep-matching.** One sentence — "gentle rain or heavy
   downpour?" — saves three wasted versions. On this project, three rain versions were
   built to the wrong reference (a heavy wash) before the owner clarified they wanted
   *gentle* rain.
3. **Fingerprint it.** Measure **9 octave-band energies + spectral centroid + rolloff**
   (`analyze_rain.py` / `analyze_typing.py`). For anything *rhythmic or event-bearing*
   ALSO measure the **temporal** fingerprint — onset rate, inter-onset-interval spread,
   per-stroke decay, and the dB level of foreground events vs. the bed
   (`analyze_drops.py`). Spectral averages are blind to events: rain's audible *drops*
   don't show up in an averaged spectrum at all, only in the temporal pass.

### Phase 2 — Shape synthesis to the measured curve

Build your oscillators/noise-shapers and drive their parameters until a re-measurement of
*your* output lands within a few percentage points of the reference fingerprint. The
`LOFI-BIBLE.md` gives you the starting parameter values per voice; the fingerprint tells
you which way to push them. Search the literature for the *physics* (Minnaert resonance
for a water glug, inharmonic bar modes 1 : 2.76 : 5.40 for a chime, tape wow/flutter +
head-bump for the lofi chain) but *tune to the reference*, not to the physics ideal.

### Phase 3 — Human LISTEN gate → freeze the realization

When the numbers converge, a human auditions once (`afplay file.wav` on macOS, or hand
them the wav). On yes:

> **The RNG was the composer. Freeze the one take they blessed.**

The generative script drew notes/velocities/timings from a seeded RNG. That *one seed's
output* is what got ratified — so capture its exact event stream into **constant tables**
(`export_score.py` → a `.rs` / `.h` table), plus a **full-table checksum**. Do NOT re-run
the RNG in production and hope; a later library bump silently redraws and you ship a
different, un-ratified take. **The freeze is the contract.** (Subtlety: your exporter must
reproduce the *exact draw order* of the original — argument-evaluation order, nested draws
inside a `choice()` — or the frozen table desyncs from what was auditioned.)

### Phase 4 — Port + ship (synthesize at launch)

Port the numpy synth to your runtime language reading the frozen tables. Key engineering:

- **The port must be byte-identical to the audition.** Keep the ratified fingerprint/
  checksum tests; passing them verbatim after the port is your oracle that nothing shifted.
- **No wall-clock reads inside the audio math.** Pass `dt` / `now` in as parameters. On
  wasm especially, `SystemTime::now()` isn't available — and a backgrounded tab that jumps
  the clock will otherwise ramp-snap your crossfades and burst-replay every queued event
  (the "stall-clock" bug). Clamp big `dt` gaps.
- **Loop the frozen composition, scatter only the stochastic layers.** All the musical
  stems (pad/drums/keys/sparkle) tile in lockstep on one loop length — make that loop long
  enough to not fatigue; then scatter the truly-random layers (rain drops, keystrokes) fresh
  each frame and fade whole stems by busy-ness (see "Never repeats" above). Tiny RAM: one
  copy of each stem loop.
- **Keep the sub band clear.** The sub-bass register must belong to the bass alone or the
  low end turns to mud. Two ways to get there: the textbook one is to high-pass every
  non-bass stem ~140 Hz (LOFI-BIBLE §4); the cheaper one this project used is to *voice the
  other stems out of that register in the first place* (mid-register EP plucks, and a day
  track with no sub content at all) so there's nothing to filter. Either way, keep ONE
  texture stem carrying all the "medium" noise — per-stem hiss *stacks* (four stems each
  with a little = +6 dB of mud); texture sits 25–35 dB below the music.
- **Perceptual volume.** Map a user volume slider as `amplitude = user²` (loudness is
  logarithmic). Linear volume feels "still too loud at 5%" — the classic trap.

---

## The failure catalog (bugs this method hit, so you don't)

- **Reference outranks literature.** A "thock" keyboard sound built to the ASMR-community
  ideal measured the *opposite* of the owner's clacky reference. Search for physics; tune
  to the reference.
- **Phantom-octave measurement bug.** Fingerprints extracted from the *listening artifacts*
  (stereo wav, soft-clipped) read every frequency halved (interleaved stereo read as mono =
  2× time-stretch). **Pin spectral references on the raw float synthesis chain, never on the
  exported/played wav.**
- **Spectral averages can't see events.** Rain drops, a snare ghost, a keystroke accent —
  measure them *temporally* (level vs. bed, onset rate) or you'll "match" a reference and
  still be missing its soul.
- **Head-only crossfade ≠ a seamless loop.** Fading the *start* of a loop back over its end
  leaves the wrap audible. A real seamless loop synthesizes a genuine *continuation* past the
  loop point and blends that in.
- **Two frozen copies of one truth desync silently.** If a kick-times table duplicates the
  drum table's kicks, add an equality test or a later edit to one drifts from the other with
  no error.
- **Control plane off the data plane.** A mute/volume press must not ride the same queue as
  audio frames — a synthesis burst can saturate the queue and eat the keypress, so the beds
  fade in *unmuted* while the UI says muted. Put mute/volume on an atomic flag the audio
  thread reads, separate from the frame channel.
- **Empty-room-uncanny.** A truly silent "idle" state feels broken, but a full bed under
  nothing feels wrong too. Keep the register (don't octave-drop), voice harmonically, sparse
  EP notes, and let the *texture* bed carry the quiet — don't drop to pure silence.

---

## Quickstart for a fresh project

1. Copy `reference/LOFI-BIBLE.md` and `scripts/` into your repo (or just read them).
2. Install: `python3 -m venv .venv && .venv/bin/pip install numpy scipy` (+ `yt-dlp`,
   `ffmpeg` for grabbing references).
3. Pick and *character-confirm* a reference. Fingerprint it with the matching `analyze_*.py`.
4. Adapt `synth_audition.py`'s voices toward the fingerprint; re-measure to convergence.
5. Human listens once. On yes, `export_score.py` freezes the take + checksum.
6. Port to your runtime; keep the fingerprint tests as the byte-identity oracle; synthesize
   at launch, loop the frozen composition, scatter the stochastic layers, gate stems by
   busy-ness.

(The `scripts/` are the real working prototypes from this project, kept as a concrete
starting point — not a turnkey CLI; `synth_audition.py` runs standalone, and `export_score.py`
imports `phase2_audition.py` — both are bundled. Adapt the voices to your own reference.)

The whole discipline in one line: **fingerprint a reference you love, drive your own
synthesis to the numbers, freeze the one take a human blesses, then let a long-enough loop +
runtime-stochastic layers + busy-ness stem-gating keep it from ever sounding the same.**
