#!/usr/bin/env python3
"""Fingerprint the typing reference (yt 2BUNHd7ENZk) vs our typing track.

Two fingerprints:
  1. spectral — octave-band energies + centroid + rolloff (as analyze_rain)
  2. temporal — onset rate, inter-onset intervals, per-stroke decay time
Analysis only; reference bytes never ship.
"""

import wave
from pathlib import Path

import numpy as np

HERE = Path(__file__).parent
SR_TARGET = 44100

BANDS = [(20, 60), (60, 125), (125, 250), (250, 500), (500, 1000),
         (1000, 2000), (2000, 4000), (4000, 8000), (8000, 16000)]


def read_wav(path):
    with wave.open(str(path), "rb") as w:
        sr = w.getframerate()
        ch = w.getnchannels()
        raw = np.frombuffer(w.readframes(w.getnframes()), dtype="<i2").astype(np.float64)
    if ch > 1:
        raw = raw.reshape(-1, ch).mean(axis=1)
    return raw / 32768.0, sr


def spectral_fp(x, sr, label):
    win = int(2 * sr)
    hop = win // 2
    acc, count = None, 0
    for s in range(0, len(x) - win, hop):
        seg = x[s:s + win] * np.hanning(win)
        mag = np.abs(np.fft.rfft(seg)) ** 2
        acc = mag if acc is None else acc + mag
        count += 1
    psd = acc / max(count, 1)
    f = np.fft.rfftfreq(win, 1 / sr)
    total = psd.sum()
    print(f"\n=== spectral: {label} ===")
    out = []
    for lo, hi in BANDS:
        e = psd[(f >= lo) & (f < hi)].sum() / total * 100
        out.append(e)
        print(f"{lo:>6}-{hi:<7} {e:>7.1f}%  {'#' * int(e * 1.2)}")
    centroid = (f * psd).sum() / total
    cum = np.cumsum(psd)
    r85 = f[np.searchsorted(cum, 0.85 * cum[-1])]
    print(f"centroid {centroid:.0f} Hz | rolloff85 {r85:.0f} Hz")
    return out


def temporal_fp(x, sr, label):
    # envelope: rectified, 2ms-smoothed
    env = np.abs(x)
    k = max(1, int(0.002 * sr))
    env = np.convolve(env, np.ones(k) / k, "same")
    # onset = env crosses 4x its median with 40ms refractory
    thr = np.median(env) * 4 + 1e-6
    onsets = []
    refractory = int(0.04 * sr)
    i = 0
    while i < len(env):
        if env[i] > thr:
            onsets.append(i)
            i += refractory
        else:
            i += 1
    onsets = np.array(onsets)
    dur_s = len(x) / sr
    rate = len(onsets) / dur_s
    print(f"\n=== temporal: {label} ===")
    print(f"strokes {len(onsets)} over {dur_s:.1f}s → {rate:.1f}/s")
    if len(onsets) > 3:
        ioi = np.diff(onsets) / sr
        print(f"inter-onset: median {np.median(ioi)*1000:.0f}ms | p25 {np.percentile(ioi,25)*1000:.0f} | p75 {np.percentile(ioi,75)*1000:.0f}")
        # per-stroke decay: time for env to fall to 20% of its local peak
        decays = []
        for o in onsets[:80]:
            seg = env[o:o + int(0.15 * sr)]
            if not len(seg):
                continue
            pk_i = np.argmax(seg[:int(0.02 * sr)] if len(seg) > int(0.02 * sr) else seg)
            pk = seg[pk_i]
            below = np.where(seg[pk_i:] < 0.2 * pk)[0]
            if len(below):
                decays.append(below[0] / sr * 1000)
        if decays:
            print(f"stroke decay→20%: median {np.median(decays):.0f}ms | p75 {np.percentile(decays,75):.0f}ms")
    return onsets


ref, sr = read_wav(HERE / "typing_ref.wav")
# trim lead/tail silence
env = np.abs(ref)
nz = np.where(env > env.max() * 0.02)[0]
ref = ref[nz[0]:nz[-1]]
e_ref = spectral_fp(ref, sr, "reference (yt 2BUNHd7ENZk)")
temporal_fp(ref, sr, "reference")

ours, sr2 = read_wav(HERE / "audio-demos" / "oneshot_typing_burst.wav")
e_our = spectral_fp(ours, sr2, "ours (v3 thock)")
temporal_fp(ours, sr2, "ours")

print("\n=== spectral delta (ours - ref, pp) ===")
for (lo, hi), a, b in zip(BANDS, e_our, e_ref):
    print(f"{lo:>6}-{hi:<7} {a - b:>+7.1f}")
