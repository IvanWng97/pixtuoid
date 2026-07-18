#!/usr/bin/env python3
"""Measure RainyMood's spectral fingerprint vs our synthesized stem_rain.

Octave-band energy distribution + spectral centroid/rolloff — the tuning
targets for mimicking the reference. ANALYSIS ONLY: the reference audio is
copyrighted; we match measured parameters, never ship their bytes.
"""

import sys
import wave
from pathlib import Path

import numpy as np

HERE = Path(__file__).parent


def read_wav(path, max_s=120.0, offset_s=30.0):
    with wave.open(str(path), "rb") as w:
        sr = w.getframerate()
        ch = w.getnchannels()
        w.setpos(int(min(offset_s, max(0, w.getnframes() / sr - max_s)) * sr))
        n = min(int(max_s * sr), w.getnframes() - w.tell())
        raw = np.frombuffer(w.readframes(n), dtype="<i2").astype(np.float64)
    if ch > 1:
        raw = raw.reshape(-1, ch).mean(axis=1)
    return raw / 32768.0, sr


BANDS = [(20, 60), (60, 125), (125, 250), (250, 500), (500, 1000),
         (1000, 2000), (2000, 4000), (4000, 8000), (8000, 16000)]


def fingerprint(x, sr, label):
    # Welch-ish: average magnitude spectrum over 4s windows
    win = int(4 * sr)
    hop = win // 2
    acc = None
    count = 0
    for start in range(0, len(x) - win, hop):
        seg = x[start:start + win] * np.hanning(win)
        mag = np.abs(np.fft.rfft(seg)) ** 2
        acc = mag if acc is None else acc + mag
        count += 1
    psd = acc / count
    f = np.fft.rfftfreq(win, 1 / sr)

    total = psd.sum()
    print(f"\n=== {label} ===")
    print(f"{'band':>14} {'energy %':>9}  bar")
    energies = []
    for lo, hi in BANDS:
        e = psd[(f >= lo) & (f < hi)].sum() / total * 100
        energies.append(e)
        print(f"{lo:>6}-{hi:<7} {e:>8.1f}%  {'#' * int(e * 1.5)}")
    centroid = (f * psd).sum() / psd.sum()
    cum = np.cumsum(psd)
    rolloff85 = f[np.searchsorted(cum, 0.85 * cum[-1])]
    rolloff95 = f[np.searchsorted(cum, 0.95 * cum[-1])]
    print(f"centroid {centroid:.0f} Hz | rolloff85 {rolloff85:.0f} Hz | rolloff95 {rolloff95:.0f} Hz")
    return energies


ref, sr1 = read_wav(HERE / "rainymood.wav")
ours, sr2 = read_wav(HERE / "audio-demos" / "stem_rain.wav", max_s=30, offset_s=0)
e_ref = fingerprint(ref, sr1, "RainyMood (reference)")
e_our = fingerprint(ours, sr2, "our stem_rain (v5)")

print("\n=== delta (ours - ref, percentage points) ===")
for (lo, hi), a, b in zip(BANDS, e_our, e_ref):
    print(f"{lo:>6}-{hi:<7} {a - b:>+8.1f}")
