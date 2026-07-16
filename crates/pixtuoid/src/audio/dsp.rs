//! Minimal DSP kernel for the procedural synth — a radix-2 FFT, brickwall
//! band filters, spectral-envelope noise shaping, and a deterministic noise
//! stream. Deliberately dependency-free: everything runs ONCE at startup to
//! pre-render sample buffers (`synth.rs`), never per audio frame, so clarity
//! beats throughput here.

/// The one sample rate every buffer in this module uses (CD-standard mono).
pub(crate) const SAMPLE_RATE: u32 = 44_100;

/// Deterministic noise stream over the canonical splitmix64 finalizer
/// (`pixtuoid_core::id`) — seedable, so synthesized assets are reproducible
/// run-to-run (the audition prototype's seeded-numpy property, kept).
pub(crate) struct NoiseStream {
    seed: u64,
    counter: u64,
}

impl NoiseStream {
    pub(crate) fn new(seed: u64) -> Self {
        Self { seed, counter: 0 }
    }

    fn next_u64(&mut self) -> u64 {
        self.counter = self.counter.wrapping_add(1);
        pixtuoid_core::id::splitmix64(
            self.seed
                .wrapping_add(self.counter.wrapping_mul(0x9E37_79B9_7F4A_7C15)),
        )
    }

    /// Uniform in [0, 1).
    pub(crate) fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Approximately standard-normal (Irwin–Hall n=4, unit variance) —
    /// plenty gaussian for noise beds; nothing here is statistics.
    pub(crate) fn norm(&mut self) -> f32 {
        (self.unit() + self.unit() + self.unit() + self.unit() - 2.0) * 1.732_051
    }
}

/// In-place iterative radix-2 FFT (Cooley–Tukey). `re`/`im` len must be a
/// power of two. `inverse` includes the 1/n scale.
fn fft(re: &mut [f32], im: &mut [f32], inverse: bool) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
    // bit-reversal permutation
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let sign = if inverse { 1.0f64 } else { -1.0f64 };
    let mut len = 2;
    while len <= n {
        let ang = sign * 2.0 * std::f64::consts::PI / len as f64;
        let (wr, wi) = (ang.cos() as f32, ang.sin() as f32);
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (ur, ui) = (re[start + k], im[start + k]);
                let (vr0, vi0) = (re[start + k + len / 2], im[start + k + len / 2]);
                let vr = vr0 * cr - vi0 * ci;
                let vi = vr0 * ci + vi0 * cr;
                re[start + k] = ur + vr;
                im[start + k] = ui + vi;
                re[start + k + len / 2] = ur - vr;
                im[start + k + len / 2] = ui - vi;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
    if inverse {
        let scale = 1.0 / n as f32;
        for v in re.iter_mut() {
            *v *= scale;
        }
        for v in im.iter_mut() {
            *v *= scale;
        }
    }
}

/// Brickwall band-pass via FFT bin zeroing (the audition prototype's filter).
/// Construction-time only — a linear-phase FIR would be overkill for
/// pre-rendered assets. Keeps `buf.len()` (internally pads to a power of 2).
pub(crate) fn bandpass(buf: &[f32], lo_hz: f32, hi_hz: f32) -> Vec<f32> {
    let n = buf.len().next_power_of_two().max(2);
    let mut re = vec![0.0f32; n];
    re[..buf.len()].copy_from_slice(buf);
    let mut im = vec![0.0f32; n];
    fft(&mut re, &mut im, false);
    let hz_per_bin = SAMPLE_RATE as f32 / n as f32;
    for k in 0..n {
        // mirror-aware bin frequency (bins above n/2 are negative freqs)
        let f = if k <= n / 2 { k } else { n - k } as f32 * hz_per_bin;
        if f < lo_hz || f > hi_hz {
            re[k] = 0.0;
            im[k] = 0.0;
        }
    }
    fft(&mut re, &mut im, true);
    re.truncate(buf.len());
    re
}

pub(crate) fn lowpass(buf: &[f32], cutoff_hz: f32) -> Vec<f32> {
    bandpass(buf, 0.0, cutoff_hz)
}

/// A power-of-two block of noise FFT-shaped to a measured octave-band
/// envelope — CIRCULARLY seamless by construction (FFT-domain shaping is
/// periodic in the block), so the returned block loops without a click.
/// `bands` are `(lo_hz, hi_hz, energy_percent)` rows.
pub(crate) fn shaped_noise_loop(
    n_pow2: usize,
    bands: &[(f32, f32, f32)],
    rng: &mut NoiseStream,
) -> Vec<f32> {
    debug_assert!(n_pow2.is_power_of_two());
    let mut re: Vec<f32> = (0..n_pow2).map(|_| rng.norm()).collect();
    let mut im = vec![0.0f32; n_pow2];
    fft(&mut re, &mut im, false);
    let hz_per_bin = SAMPLE_RATE as f32 / n_pow2 as f32;
    // per-bin amplitude gain: sqrt(band power share / band bin count)
    let mut gain = vec![0.0f32; n_pow2];
    for &(lo, hi, pct) in bands {
        let bins = ((hi - lo) / hz_per_bin).max(1.0);
        let g = (pct / 100.0 / bins).sqrt();
        for (k, gk) in gain.iter_mut().enumerate() {
            let f = if k <= n_pow2 / 2 { k } else { n_pow2 - k } as f32 * hz_per_bin;
            if f >= lo && f < hi {
                *gk = g;
            }
        }
    }
    // smooth the band stairs so edges don't ring (the prototype's hanning-201)
    let smoothed = moving_average(&gain, 201);
    for k in 0..n_pow2 {
        re[k] *= smoothed[k];
        im[k] *= smoothed[k];
    }
    fft(&mut re, &mut im, true);
    let peak = re.iter().fold(0.0f32, |a, &v| a.max(v.abs())).max(1e-9);
    re.iter_mut().for_each(|v| *v /= peak);
    re
}

fn moving_average(x: &[f32], window: usize) -> Vec<f32> {
    let half = window / 2;
    let n = x.len();
    let mut out = vec![0.0f32; n];
    let mut acc = 0.0f32;
    let mut count = 0usize;
    // sliding window over a clamped range; O(n) with incremental updates
    let mut lo = 0usize;
    let mut hi = 0usize; // exclusive
    for (i, o) in out.iter_mut().enumerate() {
        let want_lo = i.saturating_sub(half);
        let want_hi = (i + half + 1).min(n);
        while hi < want_hi {
            acc += x[hi];
            count += 1;
            hi += 1;
        }
        while lo < want_lo {
            acc -= x[lo];
            count -= 1;
            lo += 1;
        }
        *o = acc / count as f32;
    }
    out
}

/// Spectral centroid in Hz — the test oracle for the synth's spectral-sanity
/// pins (the fingerprint metric from the Phase 0 analyzers).
#[cfg(test)]
pub(crate) fn centroid_hz(buf: &[f32]) -> f32 {
    let n = buf.len().next_power_of_two().max(2);
    let mut re = vec![0.0f32; n];
    re[..buf.len()].copy_from_slice(buf);
    let mut im = vec![0.0f32; n];
    fft(&mut re, &mut im, false);
    let hz_per_bin = SAMPLE_RATE as f32 / n as f32;
    let (mut num, mut den) = (0.0f64, 0.0f64);
    for k in 0..=n / 2 {
        let p = (re[k] * re[k] + im[k] * im[k]) as f64;
        num += k as f64 * hz_per_bin as f64 * p;
        den += p;
    }
    (num / den.max(1e-12)) as f32
}

/// Fraction of spectral power inside `[lo_hz, hi_hz)` — the octave-band
/// energy metric of the Phase 0 analyzers, for the rain-envelope pin.
#[cfg(test)]
pub(crate) fn band_energy_share(buf: &[f32], lo_hz: f32, hi_hz: f32) -> f32 {
    let n = buf.len().next_power_of_two().max(2);
    let mut re = vec![0.0f32; n];
    re[..buf.len()].copy_from_slice(buf);
    let mut im = vec![0.0f32; n];
    fft(&mut re, &mut im, false);
    let hz_per_bin = SAMPLE_RATE as f32 / n as f32;
    let (mut band, mut total) = (0.0f64, 0.0f64);
    for k in 1..=n / 2 {
        let p = (re[k] * re[k] + im[k] * im[k]) as f64;
        let f = k as f32 * hz_per_bin;
        total += p;
        if f >= lo_hz && f < hi_hz {
            band += p;
        }
    }
    (band / total.max(1e-12)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_round_trips() {
        let mut rng = NoiseStream::new(1);
        let orig: Vec<f32> = (0..256).map(|_| rng.norm()).collect();
        let mut re = orig.clone();
        let mut im = vec![0.0f32; 256];
        fft(&mut re, &mut im, false);
        fft(&mut re, &mut im, true);
        for (a, b) in orig.iter().zip(&re) {
            assert!((a - b).abs() < 1e-4, "{a} vs {b}");
        }
    }

    #[test]
    fn bandpass_keeps_in_band_and_kills_out_of_band() {
        // a 500Hz tone through a 300-800Hz pass survives; through a
        // 2-4kHz pass it dies (both sides of the band edges)
        let t: Vec<f32> = (0..8192)
            .map(|i| (2.0 * std::f32::consts::PI * 500.0 * i as f32 / SAMPLE_RATE as f32).sin())
            .collect();
        let kept = bandpass(&t, 300.0, 800.0);
        let killed = bandpass(&t, 2000.0, 4000.0);
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        assert!(rms(&kept) > 0.5, "in-band tone survives");
        assert!(rms(&killed) < 0.01, "out-of-band tone dies");
    }

    #[test]
    fn shaped_noise_matches_its_target_envelope() {
        // shaping IS the rain-bed mechanism — pin it against a simple
        // two-band target within a few percentage points
        let mut rng = NoiseStream::new(7);
        let bands = [(100.0, 1000.0, 70.0), (1000.0, 8000.0, 30.0)];
        let buf = shaped_noise_loop(1 << 16, &bands, &mut rng);
        let low = band_energy_share(&buf, 100.0, 1000.0);
        let high = band_energy_share(&buf, 1000.0, 8000.0);
        assert!((low - 0.70).abs() < 0.05, "low band {low}");
        assert!((high - 0.30).abs() < 0.05, "high band {high}");
    }
}
