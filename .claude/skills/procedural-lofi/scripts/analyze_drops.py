#!/usr/bin/env python3
"""Temporal DROP-EVENT analysis of RainyMood — the feature the spectral
fingerprint can't see: audible individual raindrops over the wash.

1. Scan the whole 36min for drop-event density (find the drop-rich zones
   the owner means by "往后听").
2. Zoom into the richest minute: per-drop level-above-bed, spectral
   centroid, decay — the rebuild targets.
"""

import wave
from pathlib import Path

import numpy as np

HERE = Path(__file__).parent
F = HERE / "rainymood.wav"


def read_seg(path, start_s, dur_s):
    with wave.open(str(path), "rb") as w:
        sr = w.getframerate()
        ch = w.getnchannels()
        w.setpos(int(start_s * sr))
        n = min(int(dur_s * sr), w.getnframes() - w.tell())
        raw = np.frombuffer(w.readframes(n), dtype="<i2").astype(np.float64)
    if ch > 1:
        raw = raw.reshape(-1, ch).mean(axis=1)
    return raw / 32768.0, sr


def band_env(x, sr, lo=800, hi=6000, smooth_ms=3):
    """Envelope of the band where drop transients live."""
    X = np.fft.rfft(x)
    f = np.fft.rfftfreq(len(x), 1 / sr)
    X[(f < lo) | (f > hi)] = 0
    b = np.fft.irfft(X, len(x))
    env = np.abs(b)
    k = max(1, int(smooth_ms / 1000 * sr))
    return np.convolve(env, np.ones(k) / k, "same")


def detect_drops(x, sr):
    """Transient peaks well above the local (1s rolling) bed level."""
    env = band_env(x, sr)
    med = np.median(env)
    events = []
    thr = med * 3.0
    refr = int(0.08 * sr)
    i = 0
    while i < len(env):
        if env[i] > thr:
            j = min(i + refr, len(env))
            pk = i + int(np.argmax(env[i:j]))
            events.append((pk, env[pk] / med))  # (sample, strength vs bed)
            i = j
        else:
            i += 1
    return events, env, med


# ---- pass 1: density map over the whole file ----
with wave.open(str(F), "rb") as w:
    total_s = w.getnframes() / w.getframerate()
print(f"file: {total_s/60:.1f} min")
print("\ndrop-event density map (events/s per 60s window):")
best = (0, 0.0)
for start in range(0, int(total_s) - 60, 120):
    x, sr = read_seg(F, start, 60)
    ev, _, _ = detect_drops(x, sr)
    rate = len(ev) / 60
    strong = sum(1 for _, s in ev if s > 5) / 60
    bar = "#" * int(rate * 4)
    print(f"  {start//60:3d}min  {rate:5.1f}/s  strong(>5x) {strong:4.1f}/s  {bar}")
    if strong > best[1]:
        best = (start, strong)

# ---- pass 2: zoom into the drop-richest minute ----
start = best[0]
print(f"\n=== zoom: {start//60}min (strongest drop activity) ===")
x, sr = read_seg(F, start, 60)
events, env, med = detect_drops(x, sr)
strengths = np.array([s for _, s in events])
print(f"events: {len(events)/60:.1f}/s | strength-vs-bed: median {np.median(strengths):.1f}x "
      f"p90 {np.percentile(strengths,90):.1f}x max {strengths.max():.1f}x")
print(f"(in dB above bed: median {20*np.log10(np.median(strengths)):.0f} dB, "
      f"p90 {20*np.log10(np.percentile(strengths,90)):.0f} dB)")

# per-drop spectral character: slice 60ms around strong events
cents, decays = [], []
for pk, s in events:
    if s < 4:
        continue
    seg = x[pk - int(0.005 * sr): pk + int(0.055 * sr)]
    if len(seg) < int(0.05 * sr):
        continue
    mag = np.abs(np.fft.rfft(seg * np.hanning(len(seg)))) ** 2
    fr = np.fft.rfftfreq(len(seg), 1 / sr)
    sel = (fr > 300) & (fr < 10000)
    cents.append((fr[sel] * mag[sel]).sum() / mag[sel].sum())
    e = env[pk: pk + int(0.12 * sr)]
    below = np.where(e < 0.25 * e[0])[0]
    if len(below):
        decays.append(below[0] / sr * 1000)
print(f"strong-drop centroid: median {np.median(cents):.0f} Hz "
      f"(p25 {np.percentile(cents,25):.0f}, p75 {np.percentile(cents,75):.0f})")
print(f"strong-drop decay→25%: median {np.median(decays):.0f} ms")

# inter-drop intervals of STRONG drops — is there a quasi-periodic drip?
strong_t = np.array([pk / sr for pk, s in events if s > 5])
if len(strong_t) > 4:
    ioi = np.diff(strong_t)
    print(f"strong-drop IOI: median {np.median(ioi)*1000:.0f} ms "
          f"(p25 {np.percentile(ioi,25)*1000:.0f}, p75 {np.percentile(ioi,75)*1000:.0f})")
