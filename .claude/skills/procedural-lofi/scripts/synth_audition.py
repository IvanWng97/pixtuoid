#!/usr/bin/env python3
"""Phase-0 audition synth for pixtuoid ambient sound (#633).

Synthesizes PLACEHOLDER lofi stems (pad/keys/drums/texture/rain) and the
REAL candidate procedural one-shots (clacks/ding/glug/whir/chime), then
renders busy-ness-level demo mixes. Everything is deterministic (seeded).

Output: audio-demos/*.wav next to this script.
"""

import wave
from pathlib import Path

import numpy as np

SR = 44100
BPM = 72
BEAT = 60.0 / BPM
BAR = 4 * BEAT
LOOP_BARS = 4
LOOP_S = BAR * LOOP_BARS  # 13.33s
OUT = Path(__file__).parent / "audio-demos"
OUT.mkdir(exist_ok=True)

rng = np.random.default_rng(42)


# ---------- DSP primitives ----------

def n_samples(dur):
    return int(round(dur * SR))


def t_axis(dur):
    return np.arange(n_samples(dur)) / SR


def sinc_lowpass_kernel(cutoff, taps=301):
    t = np.arange(taps) - (taps - 1) / 2
    h = np.sinc(2 * cutoff / SR * t) * np.hanning(taps)
    return h / h.sum()


def lowpass(x, cutoff):
    # kernel must not out-length the signal (np.convolve 'same' returns
    # max(M,N) — a 4ms snippet vs a 301-tap kernel broadcast-errors upstream)
    taps = min(301, len(x) if len(x) % 2 else len(x) - 1)
    return np.convolve(x, sinc_lowpass_kernel(cutoff, taps), "same")


def highpass(x, cutoff):
    return x - lowpass(x, cutoff)


def bandpass(x, lo, hi):
    return lowpass(highpass(x, lo), hi)


def midi_freq(m):
    return 440.0 * 2 ** ((m - 69) / 12)


def env_ar(dur, attack, release):
    """Attack-release envelope over dur seconds."""
    n = n_samples(dur)
    e = np.ones(n)
    na, nr = min(n_samples(attack), n), min(n_samples(release), n)
    if na:
        e[:na] = np.linspace(0, 1, na)
    if nr:
        e[-nr:] = np.minimum(e[-nr:], np.linspace(1, 0, nr))
    return e


def place(buf, snippet, at_s, gain=1.0):
    """Overlap-add snippet into buf at time at_s (clipped to buf)."""
    i = n_samples(at_s)
    if i >= len(buf):
        return
    seg = snippet[: len(buf) - i]
    buf[i : i + len(seg)] += seg * gain


def normalize(x, peak=0.9):
    m = np.max(np.abs(x))
    return x * (peak / m) if m > 0 else x


def pink_noise(n, r=None):
    """1/f-shaped noise via FFT-domain -3dB/oct tilt (rain literature: the
    aggregate of millions of bubble events reads PINK, not white)."""
    r = r or rng
    x = np.fft.rfft(r.standard_normal(n))
    f = np.fft.rfftfreq(n, 1 / SR)
    f[0] = f[1]
    x /= np.sqrt(f)
    return np.fft.irfft(x, n)


def lofi_post(x, wow_hz=0.7, wow_dev=0.0025, flut_hz=8.0, flut_dev=0.0006,
              drive=1.6, hiss=0.0):
    # hiss default 0: per-stem hiss STACKS in the mix (4 stems ≈ +6dB) —
    # the medium noise is stem_texture's job, exactly once.
    """The lofi master chain, per production references: tape wow (slow
    pitch warble) + flutter (fast shimmer) via time-warp resample, tanh
    saturation (highs soften first), an 80-120Hz tape-head bump, gentle
    HF rolloff, and a breath of hiss."""
    n = len(x)
    t = np.arange(n)
    warp = (wow_dev * SR / (2 * np.pi * wow_hz) * np.sin(2 * np.pi * wow_hz * t / SR)
            + flut_dev * SR / (2 * np.pi * flut_hz) * np.sin(2 * np.pi * flut_hz * t / SR))
    x = np.interp(t + warp, t, x)
    x = np.tanh(x * drive) / np.tanh(drive)
    x = x + 0.35 * bandpass(x, 80, 120)  # head bump
    x = lowpass(x, 6500)
    return x + rng.standard_normal(n) * hiss


# ---------- stems (PLACEHOLDERS for the AI-generated ones) ----------

# lofi progression: Dm7 - Gm7 - Bbmaj7 - Am7, one bar each
CHORDS = [
    [50, 53, 57, 60],  # Dm7
    [55, 58, 62, 65],  # Gm7
    [58, 62, 65, 69],  # Bbmaj7
    [57, 60, 64, 67],  # Am7
]


def stem_pad():
    """Warm EP-chord bed at pitch (v2: the -12 octave drop + bare detuned
    sines read as a horror-movie drone — keep the register, voice with
    harmonics, and let each chord breathe like a held Rhodes chord)."""
    buf = np.zeros(n_samples(LOOP_S))
    for bar, chord in enumerate(CHORDS):
        dur = BAR + 0.9
        t = t_axis(dur)
        chord_sig = np.zeros_like(t)
        for i, m in enumerate(chord):
            f = midi_freq(m)  # at pitch — no octave drop
            tone = (
                np.sin(2 * np.pi * f * t)
                + 0.30 * np.sin(2 * np.pi * 2 * f * t)
                + 0.08 * np.sin(2 * np.pi * 3 * f * t)
            )
            # stagger note onsets slightly: a hand, not a machine
            tone *= env_ar(dur, 0.25 + 0.08 * i, 1.2)
            chord_sig += tone
        place(buf, chord_sig, bar * BAR)
    buf = lowpass(buf, 2600)
    slow = 1 + 0.05 * np.sin(2 * np.pi * 0.22 * t_axis(LOOP_S))
    return normalize(buf * slow, 0.7)


def stem_sparkle():
    """Sparse gentle EP notes over the pad — the 'someone left the radio
    on' humanity that keeps an empty office from feeling haunted."""
    buf = np.zeros(n_samples(LOOP_S))
    r = np.random.default_rng(5)
    for bar, chord in enumerate(CHORDS):
        # 1-2 soft high notes per bar, on friendly beats
        for beat in r.choice([0.0, 1.5, 2.0, 3.0], size=r.integers(1, 3), replace=False):
            note = int(r.choice(chord)) + 12
            place(buf, ep_pluck(note, dur=1.6, vel=0.35 + 0.2 * r.random()),
                  bar * BAR + beat * BEAT)
    return normalize(lowpass(buf, 3200), 0.6)


def ep_pluck(midi, dur=0.9, vel=1.0):
    t = t_axis(dur)
    f = midi_freq(midi)
    sig = np.sin(2 * np.pi * f * t) * np.exp(-t * 5.5)
    sig += 0.35 * np.sin(2 * np.pi * 2 * f * t) * np.exp(-t * 9)
    sig += 0.12 * np.sin(2 * np.pi * 3.01 * f * t) * np.exp(-t * 14)
    return sig * vel


def stem_keys():
    buf = np.zeros(n_samples(LOOP_S))
    r = np.random.default_rng(7)
    swing = 0.10 * BEAT
    for bar, chord in enumerate(CHORDS):
        pool = chord + [chord[0] + 12, chord[2] + 12]
        for eighth in range(8):
            if r.random() > 0.42:  # sparse
                continue
            at = bar * BAR + eighth * (BEAT / 2) + (swing if eighth % 2 else 0)
            note = int(r.choice(pool))
            place(buf, ep_pluck(note, vel=0.5 + 0.4 * r.random()), at)
    return normalize(lowpass(buf, 2400), 0.8)


def kick():
    dur = 0.32
    t = t_axis(dur)
    f = 110 * np.exp(-t * 9) + 42
    phase = 2 * np.pi * np.cumsum(f) / SR
    sig = np.sin(phase) * np.exp(-t * 11)
    click = rng.standard_normal(n_samples(0.006)) * 0.25
    sig[: len(click)] += click * np.exp(-t_axis(0.006) * 300)
    return sig


def snare():
    dur = 0.22
    t = t_axis(dur)
    noise = bandpass(rng.standard_normal(len(t)), 400, 3200) * np.exp(-t * 22)
    tone = np.sin(2 * np.pi * 185 * t) * np.exp(-t * 25) * 0.5
    return (noise + tone) * 0.8


def hat(open_=False):
    dur = 0.35 if open_ else 0.06
    t = t_axis(dur)
    return highpass(rng.standard_normal(len(t)), 6000) * np.exp(-t * (9 if open_ else 60)) * 0.5


def stem_drums():
    buf = np.zeros(n_samples(LOOP_S))
    r = np.random.default_rng(3)
    swing = 0.10 * BEAT
    for bar in range(LOOP_BARS):
        b0 = bar * BAR
        place(buf, kick(), b0)
        place(buf, kick(), b0 + 2.5 * BEAT, 0.8)
        place(buf, snare(), b0 + 2 * BEAT)
        for eighth in range(8):
            at = b0 + eighth * (BEAT / 2) + (swing if eighth % 2 else 0)
            open_ = eighth == 7 and bar % 2 == 1
            place(buf, hat(open_), at, 0.5 + 0.25 * r.random())
    return normalize(lowpass(buf, 7500), 0.85)  # lofi: shave the top


def stem_texture():
    """Room tone + light vinyl crackle (v2: crackle density halved and
    softened — dense pops over a dark drone read 'haunted', not 'cozy';
    a faint warm room hum replaces most of it)."""
    n = n_samples(LOOP_S)
    hiss = lowpass(rng.standard_normal(n), 3800) * 0.010
    t = t_axis(LOOP_S)
    room = 0.006 * np.sin(2 * np.pi * 90 * t) * (1 + 0.2 * np.sin(2 * np.pi * 0.4 * t))
    crackle = np.zeros(n)
    n_cracks = int(LOOP_S * 4)  # was 9/s
    for at in rng.uniform(0, LOOP_S - 0.01, n_cracks):
        d = 0.003 + 0.004 * rng.random()
        pop = rng.standard_normal(n_samples(d)) * np.exp(-t_axis(d) * 800)
        place(crackle, pop, at, 0.03 + 0.06 * rng.random())
    return normalize(hiss + room + lowpass(crackle, 4200), 0.45)


def _rain_bed(n):
    """RainyMood-matched wash: noise FFT-shaped to the measured envelope."""
    # GENTLE-rain envelope (owner's chosen reference, yt 42M3esYyHdw live
    # "Gentle Night Rain", measured 2026-07-16): light airy rain — energy
    # lives 500Hz-2k, almost no rumble, real 2-8k air. (RainyMood's
    # 70%-below-250Hz storm wash was the WRONG character: "比较是gentle".)
    ref_bands = [  # (lo_hz, hi_hz, energy %)
        (20, 60, 1.5), (60, 125, 3.5), (125, 250, 6.9), (250, 500, 14.3),
        (500, 1000, 24.4), (1000, 2000, 25.3), (2000, 4000, 10.2),
        (4000, 8000, 8.9), (8000, 16000, 4.0)]
    spec = np.fft.rfft(rng.standard_normal(n))
    f = np.fft.rfftfreq(n, 1 / SR)
    gain = np.zeros_like(f)
    for lo, hi, pct in ref_bands:
        sel = (f >= lo) & (f < hi)
        if sel.any():
            gain[sel] = np.sqrt(pct / 100.0 / sel.sum())
    k = np.hanning(201)
    gain = np.convolve(gain, k / k.sum(), "same")
    bed = np.fft.irfft(spec * gain, n)
    return bed / np.max(np.abs(bed))


def _slow_lfo(n, lo_hz=0.02, hi_hz=0.10, depth=0.18):
    """Aperiodic intensity drift — rain in waves, not a metronome sine."""
    m = np.fft.rfft(rng.standard_normal(n))
    f = np.fft.rfftfreq(n, 1 / SR)
    m[(f < lo_hz) | (f > hi_hz)] = 0
    lfo = np.fft.irfft(m, n)
    peak = np.max(np.abs(lfo))
    return 1 + depth * (lfo / peak if peak > 0 else lfo)


def _one_drop(kind, strength, bed_med):
    """A single drop, `strength`x above the bed's 800-6000Hz envelope.
    Three surface populations per the reference's wide centroid spread
    (p25 634 / median 1469 / p75 1726 Hz)."""
    dp = 0.10
    tp = t_axis(dp)
    if kind == 0:    # dull plop — wood/soil: low, bodied, little splash
        f0 = 320 + 300 * rng.random()
        decay, spl_gain, spl_band = 55, 0.12, (900, 2200)
    elif kind == 1:  # mid plip — water: the classic Minnaert chirp
        f0 = 700 + 700 * rng.random()
        decay, spl_gain, spl_band = 62, 0.25, (1200, 4000)
    else:            # bright ping — metal/glass: small, rare
        f0 = 1800 + 1200 * rng.random()
        decay, spl_gain, spl_band = 80, 0.15, (2500, 6000)
    chirp = f0 * (1 + 0.12 * tp / dp)
    phase = 2 * np.pi * np.cumsum(chirp) / SR
    body = np.sin(phase) * np.exp(-tp * decay)
    splash = bandpass(rng.standard_normal(len(tp)), *spl_band)
    splash *= np.exp(-tp * 180) * spl_gain
    return (body + splash) * (strength * bed_med / 0.64)


def _band_env_med(sig):
    """Median of the 800-6000Hz envelope — the analyzer's own bed measure."""
    X = np.fft.rfft(sig)
    fr = np.fft.rfftfreq(len(sig), 1 / SR)
    X[(fr < 800) | (fr > 6000)] = 0
    e = np.abs(np.fft.irfft(X, len(sig)))
    k = max(1, int(0.003 * SR))
    return np.median(np.convolve(e, np.ones(k) / k, "same"))


def _scatter_drops(dur_s, bed_med, pan=None):
    """Drop field calibrated to the reference: patter ~1.6/s at 2.6-4.2x,
    foreground every ~4s at 4.5-6.5x, 35% fast pairs. kind mix 35/50/15.
    Returns mono (pan=None) or stereo (n,2) with per-drop constant-power pan."""
    n = n_samples(dur_s)
    buf = np.zeros((n, 2)) if pan else np.zeros(n)

    def put(d, at):
        if pan:
            theta = rng.uniform(0.15, np.pi / 2 - 0.15)  # keep off hard edges
            st = np.stack([d * np.cos(theta), d * np.sin(theta)], axis=1)
            i = n_samples(at)
            seg = st[: n - i]
            if i < n:
                buf[i:i + len(seg)] += seg
        else:
            place(buf, d, at)

    def kind():
        return int(rng.choice([0, 1, 2], p=[0.20, 0.55, 0.25]))

    for at in rng.uniform(0, dur_s - 0.15, int(dur_s * 1.0)):
        put(_one_drop(kind(), 2.6 + 1.6 * rng.random(), bed_med), at)
    for at in rng.uniform(0, dur_s - 0.5, max(2, int(dur_s / 5))):
        put(_one_drop(kind(), 4.5 + 2.0 * rng.random(), bed_med), at)
        if rng.random() < 0.35:
            put(_one_drop(kind(), 3.5 + 1.5 * rng.random(), bed_med),
                at + 0.20 + 0.15 * rng.random())
    return buf


def stem_rain(dur_s=LOOP_S, stereo=False):
    """v8. Mono loop for the mixes; stereo long-form for standalone
    listening (decorrelated L/R beds + per-drop pan — and a render longer
    than the loop kills the every-13s drop deja vu. Product note: the
    ENGINE loops only the bed and scatters drops at runtime, so the real
    thing never repeats)."""
    n = n_samples(dur_s)
    und = _slow_lfo(n)
    if stereo:
        bedL = _rain_bed(n) * und
        bedR = _rain_bed(n) * und          # independent noise = wide wash
        bed_med = _band_env_med(bedL)
        drops = _scatter_drops(dur_s, bed_med, pan=True)
        mix_ = np.stack([bedL, bedR], axis=1) + drops
        return mix_ / np.max(np.abs(mix_)) * 0.6
    bed = _rain_bed(n) * und
    drops = _scatter_drops(dur_s, _band_env_med(bed))
    mix_ = bed + drops
    return mix_ / np.max(np.abs(mix_)) * 0.6


# ---------- one-shots (the REAL procedural candidates) ----------

def one_keystroke(r):
    """v4, matched to the OWNER'S reference (yt 2BUNHd7ENZk "Writing on
    keyboard Sound Effect", fingerprinted with analyze_typing.py): a BRIGHT
    office clack — 82.6% of energy in 1-4kHz, centroid ~2.4kHz, stroke
    decays to 20% in ~8ms, almost no low end. (The v3 thock — centroid
    474Hz per the ASMR-community ideal — measured exactly opposite; the
    reference outranks the literature.)"""
    d = 0.05
    n = n_samples(d)
    t = t_axis(d)
    # main click: tight noise burst peaked 2-4k, very fast decay
    f_lo = 1250 + 450 * r.random()
    click = bandpass(r.standard_normal(n), f_lo, f_lo + 2100) * np.exp(-t * 330)
    # body: 1-2k component gives the click its "key" substance
    f_b = 1000 + 400 * r.random()
    body = bandpass(r.standard_normal(n), f_b, f_b + 900) * np.exp(-t * 280) * 1.1
    # spice: a whisper above 4k so the top octaves read natural
    spice = bandpass(r.standard_normal(n), 4200, 7500) * np.exp(-t * 500) * 0.18
    buf = click + body + spice
    # up-stroke tick ~60-90ms later, same family, quieter
    up_at = 0.055 + 0.03 * r.random()
    du = 0.02
    tu = t_axis(du)
    up = bandpass(r.standard_normal(n_samples(du)), 2000, 4500) * np.exp(-tu * 520)
    out = np.zeros(n_samples(up_at) + n_samples(du) + 8)
    out[:n] += buf
    place(out, up, up_at, 0.35)
    return out


def typing_track(dur, bursts_per_min, r):
    """Clustered keystrokes — bursts of fast typing with gaps.
    v4 timing matched to the reference: inter-key median ~80ms with real
    spread (p25-p75 = 59-93ms), occasional think-pause inside a burst."""
    buf = np.zeros(n_samples(dur))
    n_bursts = max(1, int(dur / 60 * bursts_per_min))
    for _ in range(n_bursts):
        start = r.uniform(0, max(0.1, dur - 2.5))
        at = start
        for _k in range(r.integers(8, 22)):
            place(buf, one_keystroke(r), at, 0.5 + 0.4 * r.random())
            at += 0.066 + 0.030 * r.random() + (0.18 if r.random() < 0.08 else 0)
        if r.random() < 0.5:  # spacebar: a rounder, slightly lower clack
            d = 0.05
            td = t_axis(d)
            thud = (bandpass(r.standard_normal(n_samples(d)), 900, 2200) * np.exp(-td * 300)
                    + bandpass(r.standard_normal(n_samples(d)), 400, 900) * np.exp(-td * 250) * 0.5)
            place(buf, thud, at, 0.8)
    return buf


def elevator_ding():
    """v4, chime-bar physics: a struck metal bar's transverse modes are
    INHARMONIC at ~1 : 2.76 : 5.40 (the glockenspiel ratios — v3's 1:2:3
    harmonic stack read 'organ', not 'ding'). Upper partials decay 3-5x
    faster; the fundamental is a detuned pair for a slow beat."""
    dur = 1.6
    t = t_axis(dur)
    f0 = 870.0
    sig = (
        np.sin(2 * np.pi * (f0 - 0.8) * t) * np.exp(-t * 2.2)
        + np.sin(2 * np.pi * (f0 + 0.8) * t) * np.exp(-t * 2.2)  # beating pair
        + 0.55 * np.sin(2 * np.pi * f0 * 2.76 * t) * np.exp(-t * 8)
        + 0.25 * np.sin(2 * np.pi * f0 * 5.40 * t) * np.exp(-t * 16)
    )
    strike = bandpass(rng.standard_normal(len(t)), 2500, 7000) * np.exp(-t * 300) * 0.3
    return normalize(sig * 0.5 + strike, 0.6)


def cooler_glug():
    """v4, Minnaert physics: each glug is ONE large bubble — a damped sine
    near its Minnaert frequency (r≈6-9mm → ~360-540Hz) whose pitch RISES
    slightly as the bubble ascends and shrinks; successive glugs step UP
    (the air column shortens). Plus a quiet pour-noise wash."""
    dur = 0.85
    buf = np.zeros(n_samples(dur))
    f_base = 340.0
    for i, at in enumerate((0.02, 0.28, 0.55)):
        d = 0.16
        t = t_axis(d)
        f0 = f_base * (1 + 0.18 * i)          # the sequence steps up
        chirp = f0 * (1 + 0.12 * t / d)        # each bubble bends up
        phase = 2 * np.pi * np.cumsum(chirp) / SR
        g = np.sin(phase) * np.exp(-t * 22) * env_ar(d, 0.004, 0.05)
        place(buf, g, at, 0.9 - 0.1 * i)
    pour = bandpass(rng.standard_normal(len(buf)), 600, 2000)
    pour *= env_ar(dur, 0.1, 0.3) * 0.06
    return normalize(lowpass(buf + pour, 3500), 0.55)


def printer_whir():
    """v2 — structured like a real office laser printer instead of the v1
    noise blob: motor spin-UP (pitch rises), quasi-regular feed-roller
    ticks, a paper-slide swoosh through the middle, spin-DOWN tail."""
    dur = 1.5
    n = n_samples(dur)
    t = t_axis(dur)
    buf = np.zeros(n)
    # motor: pitch ramps 80→130Hz over the spin-up, holds, sags at the end
    f_motor = 80 + 50 * np.clip(t / 0.25, 0, 1) - 30 * np.clip((t - 1.15) / 0.35, 0, 1)
    phase = 2 * np.pi * np.cumsum(f_motor) / SR
    motor = (np.sin(phase) + 0.45 * np.sin(2 * phase) + 0.2 * np.sin(3 * phase))
    motor *= env_ar(dur, 0.12, 0.3) * 0.5
    # mechanical texture: filtered noise, gated by the same envelope
    texture_n = bandpass(rng.standard_normal(n), 400, 2600) * env_ar(dur, 0.15, 0.35) * 0.16
    # feed-roller ticks: quasi-regular 11/s with jitter, through the middle
    ticks = np.zeros(n)
    at = 0.28
    while at < 1.05:
        d = 0.014
        tk = bandpass(rng.standard_normal(n_samples(d)), 1500, 4200) * np.exp(-t_axis(d) * 260)
        place(ticks, tk, at, 0.5 + 0.2 * rng.random())
        at += 0.09 + 0.02 * rng.random()
    # paper slide: a swoosh — noise through a rising bandpass, mid-file
    sw_d = 0.5
    sw = bandpass(rng.standard_normal(n_samples(sw_d)), 900, 3000)
    sw *= np.hanning(n_samples(sw_d)) * 0.35
    place(buf, sw, 0.55)
    buf += motor + texture_n + ticks
    return normalize(lowpass(buf, 5000), 0.55)


def door_chime():
    """v2 — a DESCENDING ding-dong (real doorbells fall, our v1 rose) with
    warmer, longer bells so it cannot be confused with the elevator's
    single bright inharmonic chime-bar strike (872Hz centroid): lower
    register, harmonic (not chime-bar) partials, slow decay."""
    buf = np.zeros(n_samples(2.0))
    for at, m, g in ((0.0, 76, 0.8), (0.42, 72, 1.0)):   # E5 → C5, falling
        d = 1.5
        t = t_axis(d)
        f = midi_freq(m)
        note = (np.sin(2 * np.pi * f * t) * np.exp(-t * 2.6)
                + 0.35 * np.sin(2 * np.pi * 2 * f * t) * np.exp(-t * 5)
                + 0.10 * np.sin(2 * np.pi * 3 * f * t) * np.exp(-t * 9))
        place(buf, note, at, g * 0.7)
    return normalize(lowpass(buf, 4000), 0.55)


# ---------- mixing + IO ----------

def to_stereo(mono, pan=0.0, width_delay=0):
    """Constant-power pan; optional tiny delay on R for width."""
    theta = (pan + 1) / 2 * np.pi / 2
    left = mono * np.cos(theta)
    right = mono * np.sin(theta)
    if width_delay:
        right = np.concatenate([np.zeros(width_delay), right[:-width_delay]])
    return np.stack([left, right], axis=1)


def write_wav(name, data):
    """data: mono (n,) or stereo (n,2) float; peak-safe soft clip."""
    if data.ndim == 1:
        data = to_stereo(data)
    data = np.tanh(data * 1.1) * 0.85
    pcm = (data * 32767).astype("<i2")
    with wave.open(str(OUT / name), "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(pcm.tobytes())
    print(f"  {name}  ({len(data)/SR:.1f}s)")


def tile(stem, loops):
    return np.tile(stem, loops)


def mix(parts):
    """parts: list of (stereo_or_mono, gain, pan, width). Sums to stereo."""
    n = max(len(p[0]) for p in parts)
    acc = np.zeros((n, 2))
    for sig, gain, pan, width in parts:
        st = to_stereo(sig, pan, width) if sig.ndim == 1 else sig
        acc[: len(st)] += st * gain
    return acc


def main():
    print("stems (placeholder for AI-generated):")
    # the musical stems go through the tape: wow/flutter + saturation +
    # head bump + rolloff (the lofi literature chain). texture/rain don't —
    # they ARE the medium.
    pad = normalize(lofi_post(stem_pad()), 0.7)
    sparkle = normalize(lofi_post(stem_sparkle()), 0.6)
    keys = normalize(lofi_post(stem_keys()), 0.8)
    drums = normalize(lofi_post(stem_drums(), drive=2.2), 0.85)
    texture, rain = stem_texture(), stem_rain()
    rain_solo = stem_rain(60.0, stereo=True)
    for name, s in [("stem_pad", pad), ("stem_sparkle", sparkle), ("stem_keys", keys),
                    ("stem_drums", drums), ("stem_texture", texture), ("stem_rain", rain_solo)]:
        write_wav(f"{name}.wav", s)

    print("one-shots (REAL procedural candidates):")
    r = np.random.default_rng(11)
    write_wav("oneshot_typing_burst.wav", normalize(typing_track(4.0, 30, r), 0.7))
    write_wav("oneshot_elevator_ding.wav", elevator_ding())
    write_wav("oneshot_cooler_glug.wav", cooler_glug())
    write_wav("oneshot_printer_whir.wav", printer_whir())
    write_wav("oneshot_door_chime.wav", door_chime())

    print("busy-ness demo mixes (2 loops each, ~27s):")
    L = 2
    dur = LOOP_S * L
    r_e, r_m, r_b = (np.random.default_rng(s) for s in (21, 22, 23))

    # empty office: warm pad + sparse EP notes + soft room tone —
    # "someone left the radio on", NOT "abandoned building" (v2)
    write_wav("demo_1_empty.wav", mix([
        (tile(pad, L), 0.75, 0.0, 11), (tile(sparkle, L), 0.7, 0.15, 0),
        (tile(texture, L), 0.28, 0.0, 0)]))

    # moderate: 2 agents — keys join, light drums, sparse typing
    write_wav("demo_2_moderate.wav", mix([
        (tile(pad, L), 0.7, 0.0, 11), (tile(texture, L), 0.30, 0.0, 0),
        (tile(keys, L), 0.6, -0.25, 0), (tile(drums, L), 0.35, 0.0, 0),
        (typing_track(dur, 14, r_m), 0.5, 0.3, 0)]))

    # busy: full band + dense typing + appliance moments
    busy_parts = [
        (tile(pad, L), 0.65, 0.0, 11), (tile(texture, L), 0.28, 0.0, 0),
        (tile(keys, L), 0.7, -0.25, 0), (tile(drums, L), 0.6, 0.0, 0),
        (typing_track(dur, 34, r_b), 0.6, 0.35, 0),
        (typing_track(dur, 26, r_e), 0.5, -0.4, 0)]
    appl = np.zeros(n_samples(dur))
    place(appl, printer_whir(), 7.5, 0.8)
    place(appl, cooler_glug(), 14.0, 0.9)
    place(appl, elevator_ding(), 20.5, 0.6)
    place(appl, door_chime(), 24.0, 0.6)
    busy_parts.append((appl, 0.8, 0.15, 0))
    write_wav("demo_3_busy.wav", mix(busy_parts))

    # rainy + busy: the rain stem joins
    write_wav("demo_4_rainy_busy.wav", mix(
        busy_parts + [(tile(rain, L), 0.55, 0.0, 17)]))

    print(f"\nall files in: {OUT}")


if __name__ == "__main__":
    main()
