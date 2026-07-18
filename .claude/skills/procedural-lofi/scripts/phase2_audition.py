#!/usr/bin/env python3
"""Phase 2 (musical stems) audition renders — reuses the Phase 0 synth 1:1.

What Phase 2 must prove BEFORE any engine code (the audio pin-before-coding):
1. SOAK: a 13.33s stem loop tiled for ~2 min per tier — does it wear thin?
2. TRANSITION: tier gains ramped at the mixer's real RAMP_PER_S while the
   "office" goes empty -> moderate -> busy -> moderate — the runtime
   "generative feel from mixing" claim, heard end to end.
Stem solos re-render too (Phase 0's copies died with its scratchpad).

Tier gain tables are pinned COPIES of crates/pixtuoid-scene/src/audio.rs
(PAD_GAIN/SPARKLE_GAIN/KEYS_GAIN/DRUMS_GAIN/TEXTURE_GAIN/TYPING_GAIN) — if
those consts move, re-copy before re-auditioning.
"""

import numpy as np
import synth_audition as s0

# v2 (owner: "感觉可以拉长一些"): 8-bar loops. The progression still cycles
# every 4 bars (normal lofi); the SECOND half's variation comes from the
# melodic foreground — sparkle/keys draw fresh events for bars 4-7. Pad
# repeats its (chord-determined) rendering; drums keep the groove.
LOOP_BARS = 8
s0.LOOP_BARS = LOOP_BARS  # stem_drums/texture read the module global
s0.LOOP_S = s0.BAR * LOOP_BARS

SR = s0.SR
LOOP_S = s0.LOOP_S
BEAT = s0.BEAT
BAR = s0.BAR
RAMP_PER_S = 0.5  # mixer.rs RAMP_PER_S — the real runtime slew


def chords8():
    return [s0.CHORDS[b % len(s0.CHORDS)] for b in range(LOOP_BARS)]


def stem_pad8():
    # stem_pad's body over 8 bars (it iterates enumerate(CHORDS) = 4)
    buf = np.zeros(s0.n_samples(LOOP_S))
    for bar, chord in enumerate(chords8()):
        dur = BAR + 0.9
        t = s0.t_axis(dur)
        chord_sig = np.zeros_like(t)
        for i, m in enumerate(chord):
            f = s0.midi_freq(m)
            tone = (np.sin(2 * np.pi * f * t)
                    + 0.30 * np.sin(2 * np.pi * 2 * f * t)
                    + 0.08 * np.sin(2 * np.pi * 3 * f * t))
            tone *= s0.env_ar(dur, 0.25 + 0.08 * i, 1.2)
            chord_sig += tone
        s0.place(buf, chord_sig, bar * BAR)
    buf = s0.lowpass(buf, 2600)
    slow = 1 + 0.05 * np.sin(2 * np.pi * 0.22 * s0.t_axis(LOOP_S))
    return s0.normalize(buf * slow, 0.7)


def stem_sparkle8():
    buf = np.zeros(s0.n_samples(LOOP_S))
    r = np.random.default_rng(5)
    for bar, chord in enumerate(chords8()):
        for beat in r.choice([0.0, 1.5, 2.0, 3.0], size=r.integers(1, 3), replace=False):
            note = int(r.choice(chord)) + 12
            s0.place(buf, s0.ep_pluck(note, dur=1.6, vel=0.35 + 0.2 * r.random()),
                     bar * BAR + beat * BEAT)
    return s0.normalize(s0.lowpass(buf, 3200), 0.6)


def stem_keys8():
    buf = np.zeros(s0.n_samples(LOOP_S))
    r = np.random.default_rng(7)
    swing = 0.10 * BEAT
    for bar, chord in enumerate(chords8()):
        pool = chord + [chord[0] + 12, chord[2] + 12]
        for eighth in range(8):
            if r.random() > 0.42:
                continue
            at = bar * BAR + eighth * (BEAT / 2) + (swing if eighth % 2 else 0)
            note = int(r.choice(pool))
            s0.place(buf, s0.ep_pluck(note, vel=0.5 + 0.4 * r.random()), at)
    return s0.normalize(s0.lowpass(buf, 2400), 0.8)

# scene/src/audio.rs tier tables [empty, moderate, busy]
PAD_GAIN = [0.75, 0.70, 0.65]
SPARKLE_GAIN = [0.70, 0.0, 0.0]
KEYS_GAIN = [0.0, 0.60, 0.70]
DRUMS_GAIN = [0.0, 0.35, 0.60]
TEXTURE_GAIN = [0.28, 0.30, 0.28]
TYPING_GAIN = [0.0, 0.50, 0.80]
TYPING_BURSTS_PER_MIN_AT_FULL = 28  # mixer.rs BURSTS_PER_MIN_AT_FULL


def render_stems():
    pad = s0.normalize(s0.lofi_post(stem_pad8()), 0.7)
    sparkle = s0.normalize(s0.lofi_post(stem_sparkle8()), 0.6)
    keys = s0.normalize(s0.lofi_post(stem_keys8()), 0.8)
    drums = s0.normalize(s0.lofi_post(s0.stem_drums(), drive=2.2), 0.85)
    texture = s0.stem_texture()
    return {"pad": pad, "sparkle": sparkle, "keys": keys,
            "drums": drums, "texture": texture}


def gains_at(tier):
    return {"pad": PAD_GAIN[tier], "sparkle": SPARKLE_GAIN[tier],
            "keys": KEYS_GAIN[tier], "drums": DRUMS_GAIN[tier],
            "texture": TEXTURE_GAIN[tier]}


def soak(stems, tier, minutes=2.0):
    loops = int(np.ceil(minutes * 60.0 / LOOP_S))
    dur = LOOP_S * loops
    g = gains_at(tier)
    parts = [(s0.tile(stems[k], loops), g[k], 0.0, 11 if k == "pad" else 0)
             for k in stems if g[k] > 0.0]
    if TYPING_GAIN[tier] > 0.0:
        r = np.random.default_rng(30 + tier)
        bpm = TYPING_BURSTS_PER_MIN_AT_FULL * TYPING_GAIN[tier]
        parts.append((s0.typing_track(dur, bpm, r), TYPING_GAIN[tier], 0.3, 0))
    return s0.mix(parts)


def transition(stems, schedule, total_s):
    """schedule: [(at_s, tier)] — per-stem gains slew toward the active
    tier's target at RAMP_PER_S, exactly like mixer.rs."""
    n = s0.n_samples(total_s)
    dt = 1.0 / SR
    out_parts = {}
    for k, stem in stems.items():
        loops = int(np.ceil(total_s / LOOP_S))
        out_parts[k] = s0.tile(stem, loops)[:n]
    gain_curves = {k: np.zeros(n) for k in stems}
    cur = {k: gains_at(schedule[0][1])[k] for k in stems}
    seg_starts = [int(at * SR) for at, _ in schedule] + [n]
    for i, (_, tier) in enumerate(schedule):
        tgt = gains_at(tier)
        for idx in range(seg_starts[i], min(seg_starts[i + 1], n)):
            for k in stems:
                d = tgt[k] - cur[k]
                step = RAMP_PER_S * dt
                cur[k] += np.clip(d, -step, step)
                gain_curves[k][idx] = cur[k]
    mixed = sum(out_parts[k] * gain_curves[k] for k in stems)
    # typing joins per segment (burst scheduler; no slew — density is the knob)
    r = np.random.default_rng(41)
    typing = np.zeros(n)
    for i, (at, tier) in enumerate(schedule):
        if TYPING_GAIN[tier] <= 0.0:
            continue
        seg_end = seg_starts[i + 1] / SR if i + 1 < len(seg_starts) else total_s
        seg_dur = seg_end - at
        bpm = TYPING_BURSTS_PER_MIN_AT_FULL * TYPING_GAIN[tier]
        s0.place(typing, s0.typing_track(seg_dur, bpm, r) * TYPING_GAIN[tier], at)
    return s0.normalize(mixed + typing * 0.9, 0.85)


def main():
    stems = render_stems()
    for k, v in stems.items():
        s0.write_wav(f"p2_stem_{k}.wav", v)
    for tier, name in [(0, "empty"), (1, "moderate"), (2, "busy")]:
        s0.write_wav(f"p2_soak_{name}.wav", soak(stems, tier))
    # a 3-min office day: empty 30s -> moderate 60s -> busy 60s -> moderate
    sched = [(0.0, 0), (30.0, 1), (90.0, 2), (150.0, 1)]
    s0.write_wav("p2_transition_day.wav", transition(stems, sched, 180.0))
    print(f"done -> {s0.OUT}")


if __name__ == "__main__":
    main()
