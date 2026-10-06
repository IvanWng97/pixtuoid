//! Minimal DSP kernel for the procedural synth — a real-input FFT, brickwall
//! band filters, spectral-envelope noise shaping, and a deterministic noise
//! stream. Everything runs ONCE per track to pre-render sample buffers
//! (`synth.rs`), never per audio frame. Dependency-free on purpose: the site's
//! wasm ships this module, and an FFT crate is a download every visitor pays.

/// The one sample rate every buffer in this module uses (CD-standard mono).
pub const SAMPLE_RATE: u32 = 44_100;

/// Deterministic noise stream over the canonical splitmix64 finalizer
/// (`pixtuoid_core::id`) — seedable, so synthesized assets are reproducible
/// run-to-run.
#[derive(Debug)]
pub struct NoiseStream {
    seed: u64,
    counter: u64,
}

impl NoiseStream {
    pub fn new(seed: u64) -> Self {
        Self { seed, counter: 0 }
    }

    fn next_u64(&mut self) -> u64 {
        self.counter = self.counter.wrapping_add(1);
        crate::splitmix_draw(self.seed, self.counter)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Approximately standard-normal (Irwin–Hall n=4, unit variance).
    pub fn norm(&mut self) -> f32 {
        (self.unit() + self.unit() + self.unit() + self.unit() - 2.0) * 1.732_051
    }
}

/// `e^(−2πik/n)` for `k` in `0..n/2` as `(cos, sin)` rows, built per transform
/// pair and dropped: a kept table for the longest beds pins megabytes.
type Twiddles = Vec<(f32, f32)>;

/// [`Twiddles`] for `n`, in f64: a recurrence across a stage drifts by its
/// length.
fn twiddles(n: usize) -> Twiddles {
    (0..n / 2)
        .map(|k| {
            let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
            (a.cos() as f32, a.sin() as f32)
        })
        .collect()
}

/// In-place iterative radix-2 FFT (Cooley–Tukey), unscaled. `re`/`im` len
/// must be a power of two no longer than the table: `tw` is [`twiddles`] for
/// a power of two at least `re.len()`.
fn fft(re: &mut [f32], im: &mut [f32], tw: &[(f32, f32)], inverse: bool) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n && 2 * tw.len() >= n);
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
    let sign = if inverse { -1.0f32 } else { 1.0 };
    let mut len = 2;
    // Two radix-2 stages per pass where two remain (radix-2²): half the passes
    // over a buffer that outgrows the cache, and the second stage's odd
    // twiddles are the even ones turned a quarter.
    while 2 * len <= n {
        let h = len / 2;
        let (step1, step2) = (2 * tw.len() / len, tw.len() / len);
        for (re, im) in re
            .chunks_exact_mut(2 * len)
            .zip(im.chunks_exact_mut(2 * len))
        {
            for k in 0..h {
                let (w1r, w1i) = tw[k * step1];
                let (w2r, w2i) = tw[k * step2];
                let (w1i, w2i) = (sign * w1i, sign * w2i);
                let [i0, i1, i2, i3] = [k, k + h, k + 2 * h, k + 3 * h];
                let (t1r, t1i) = (re[i1] * w1r - im[i1] * w1i, re[i1] * w1i + im[i1] * w1r);
                let (t3r, t3i) = (re[i3] * w1r - im[i3] * w1i, re[i3] * w1i + im[i3] * w1r);
                let (y0r, y0i) = (re[i0] + t1r, im[i0] + t1i);
                let (y1r, y1i) = (re[i0] - t1r, im[i0] - t1i);
                let (y2r, y2i) = (re[i2] + t3r, im[i2] + t3i);
                let (y3r, y3i) = (re[i2] - t3r, im[i2] - t3i);
                let (u2r, u2i) = (y2r * w2r - y2i * w2i, y2r * w2i + y2i * w2r);
                // w2 · e^(∓iπ/2): the twiddle `h` further along the stage
                let (u3r, u3i) = {
                    let (ar, ai) = (y3r * w2r - y3i * w2i, y3r * w2i + y3i * w2r);
                    (sign * ai, -sign * ar)
                };
                (re[i0], im[i0]) = (y0r + u2r, y0i + u2i);
                (re[i2], im[i2]) = (y0r - u2r, y0i - u2i);
                (re[i1], im[i1]) = (y1r + u3r, y1i + u3i);
                (re[i3], im[i3]) = (y1r - u3r, y1i - u3i);
            }
        }
        len <<= 2;
    }
    if len <= n {
        let half = len / 2;
        let step = 2 * tw.len() / len;
        for (re, im) in re.chunks_exact_mut(len).zip(im.chunks_exact_mut(len)) {
            let (ur, vr) = re.split_at_mut(half);
            let (ui, vi) = im.split_at_mut(half);
            let w = tw.iter().step_by(step);
            for ((((ur, ui), vr), vi), &(wr, wi)) in ur.iter_mut().zip(ui).zip(vr).zip(vi).zip(w) {
                let wi = sign * wi;
                let (tr, ti) = (*vr * wr - *vi * wi, *vr * wi + *vi * wr);
                (*vr, *vi) = (*ur - tr, *ui - ti);
                (*ur, *ui) = (*ur + tr, *ui + ti);
            }
        }
    }
}

/// Bins `0..=n/2` of the spectrum of `buf` zero-padded to `n`, `tw` being
/// [`twiddles`]`(n)`, through one `n/2`-point complex FFT of its even and odd
/// samples (a real signal's spectrum is conjugate-symmetric, so the upper half
/// is redundant).
fn spectrum(buf: &[f32], tw: &[(f32, f32)]) -> (Vec<f32>, Vec<f32>) {
    let m = tw.len();
    let (mut zr, mut zi) = (vec![0.0f32; m], vec![0.0f32; m]);
    for (k, pair) in buf.chunks(2).enumerate() {
        zr[k] = pair[0];
        zi[k] = pair.get(1).copied().unwrap_or(0.0);
    }
    fft(&mut zr, &mut zi, tw, false);
    let (mut xr, mut xi) = (vec![0.0f32; m + 1], vec![0.0f32; m + 1]);
    for k in 0..=m {
        let (ar, ai) = (zr[k % m], zi[k % m]);
        let (br, bi) = (zr[(m - k) % m], -zi[(m - k) % m]);
        // the even samples' spectrum, and the odd samples'
        let (er, ei) = (f32::midpoint(ar, br), f32::midpoint(ai, bi));
        let (or, oi) = ((ai - bi) * 0.5, (br - ar) * 0.5);
        let (wr, wi) = tw.get(k).copied().unwrap_or((-1.0, 0.0));
        xr[k] = er + wr * or - wi * oi;
        xi[k] = ei + wr * oi + wi * or;
    }
    (xr, xi)
}

/// The `n`-sample real signal whose bins `0..=n/2` are `xr`/`xi`, `tw` being
/// [`twiddles`]`(n)`: [`spectrum`] inverted, scaled back to its input.
fn signal(xr: &[f32], xi: &[f32], tw: &[(f32, f32)]) -> Vec<f32> {
    let m = tw.len();
    let (mut zr, mut zi) = (vec![0.0f32; m], vec![0.0f32; m]);
    for k in 0..m {
        let (ar, ai) = (xr[k], xi[k]);
        let (br, bi) = (xr[m - k], -xi[m - k]);
        let (er, ei) = (f32::midpoint(ar, br), f32::midpoint(ai, bi));
        let (dr, di) = ((ar - br) * 0.5, (ai - bi) * 0.5);
        let (wr, wi) = tw[k];
        let (or, oi) = (dr * wr + di * wi, di * wr - dr * wi);
        (zr[k], zi[k]) = (er - oi, ei + or);
    }
    fft(&mut zr, &mut zi, tw, true);
    let scale = 1.0 / m as f32;
    zr.iter()
        .zip(&zi)
        .flat_map(|(&e, &o)| [e * scale, o * scale])
        .collect()
}

/// `buf`'s half spectrum at the next power of two, with the twiddles it was
/// taken with (half that length) for the [`signal`] back, and the bin width.
fn forward_spectrum(buf: &[f32]) -> (Vec<f32>, Vec<f32>, Twiddles, f32) {
    let n = buf.len().next_power_of_two().max(2);
    let tw = twiddles(n);
    let (re, im) = spectrum(buf, &tw);
    (re, im, tw, SAMPLE_RATE as f32 / n as f32)
}

/// Mirror-aware frequency of FFT bin `k` in an `n`-point spectrum: bins above
/// `n/2` are NEGATIVE frequencies, folded back to `n − k`. A copy that drops the
/// fold band-passes the wrong half of the spectrum.
fn bin_freq(k: usize, n: usize, hz_per_bin: f32) -> f32 {
    (if k <= n / 2 { k } else { n - k }) as f32 * hz_per_bin
}

/// Brickwall band-pass via FFT bin zeroing. Construction-time only — a
/// linear-phase FIR would be overkill for pre-rendered assets. Keeps
/// `buf.len()` (internally pads to a power of 2).
pub fn bandpass(buf: &[f32], lo_hz: f32, hi_hz: f32) -> Vec<f32> {
    let (mut re, mut im, tw, hz_per_bin) = forward_spectrum(buf);
    let n = 2 * tw.len();
    for (k, (r, i)) in re.iter_mut().zip(&mut im).enumerate() {
        let f = bin_freq(k, n, hz_per_bin);
        if f < lo_hz || f > hi_hz {
            *r = 0.0;
            *i = 0.0;
        }
    }
    let mut out = signal(&re, &im, &tw);
    out.truncate(buf.len());
    out
}

pub fn lowpass(buf: &[f32], cutoff_hz: f32) -> Vec<f32> {
    bandpass(buf, 0.0, cutoff_hz)
}

pub fn highpass(buf: &[f32], cutoff_hz: f32) -> Vec<f32> {
    bandpass(buf, cutoff_hz, SAMPLE_RATE as f32)
}

/// Tape wow/flutter — resample `buf` along a sinusoidally warped time axis
/// (linear interpolation, edge-clamped). Each `(hz, dev)` pair contributes a
/// pitch deviation of ±`dev` (fractional) by displacing the read head
/// `dev·SR/(2π·hz)` samples at rate `hz`.
pub fn warp_resample(buf: &[f32], warps: &[(f32, f32)]) -> Vec<f32> {
    let n = buf.len();
    let amp: Vec<(f32, f32)> = warps
        .iter()
        .map(|&(hz, dev)| {
            (
                hz,
                dev * SAMPLE_RATE as f32 / (2.0 * std::f32::consts::PI * hz),
            )
        })
        .collect();
    (0..n)
        .map(|i| {
            let mut p = i as f32;
            for &(hz, a) in &amp {
                p += a * (2.0 * std::f32::consts::PI * hz * i as f32 / SAMPLE_RATE as f32).sin();
            }
            let p = p.clamp(0.0, (n - 1) as f32);
            let lo = p.floor() as usize;
            let hi = (lo + 1).min(n - 1);
            let frac = p - lo as f32;
            buf[lo] * (1.0 - frac) + buf[hi] * frac
        })
        .collect()
}

/// A power-of-two block of noise FFT-shaped to a measured octave-band
/// envelope — CIRCULARLY seamless by construction (FFT-domain shaping is
/// periodic in the block), so the returned block loops without a click.
/// `bands` are `(lo_hz, hi_hz, energy_percent)` rows.
///
/// # Panics
///
/// If `n_pow2` is not a power of two.
pub fn shaped_noise_loop(
    n_pow2: usize,
    bands: &[(f32, f32, f32)],
    rng: &mut NoiseStream,
) -> Vec<f32> {
    assert!(
        n_pow2.is_power_of_two(),
        "a noise loop is a power-of-two block"
    );
    let noise: Vec<f32> = (0..n_pow2).map(|_| rng.norm()).collect();
    let tw = twiddles(n_pow2);
    let (mut re, mut im) = spectrum(&noise, &tw);
    let hz_per_bin = SAMPLE_RATE as f32 / n_pow2 as f32;
    // per-bin amplitude gain: sqrt(band power share / band bin count)
    let mut gain = vec![0.0f32; n_pow2];
    for &(lo, hi, pct) in bands {
        let bins = ((hi - lo) / hz_per_bin).max(1.0);
        let g = (pct / 100.0 / bins).sqrt();
        for (k, gk) in gain.iter_mut().enumerate() {
            let f = bin_freq(k, n_pow2, hz_per_bin);
            if f >= lo && f < hi {
                *gk = g;
            }
        }
    }
    // smooth the band stairs so edges don't ring
    let smoothed = moving_average(&gain, 201);
    for ((r, i), s) in re.iter_mut().zip(&mut im).zip(&smoothed) {
        *r *= s;
        *i *= s;
    }
    let mut out = signal(&re, &im, &tw);
    let peak = out.iter().fold(0.0f32, |a, &v| a.max(v.abs())).max(1e-9);
    out.iter_mut().for_each(|v| *v /= peak);
    out
}

fn moving_average(x: &[f32], window: usize) -> Vec<f32> {
    let half = window / 2;
    let n = x.len();
    let mut out = vec![0.0f32; n];
    let mut acc = 0.0f32;
    let mut count = 0usize;
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

/// Spectral centroid in Hz. NOT `#[cfg(test)]`-gated: a dependency's test-cfg
/// items are invisible cross-crate, and the binary's `run_loop` composition test
/// reads this (as the web driver's tests will).
pub fn centroid_hz(buf: &[f32]) -> f32 {
    let (re, im, _, hz_per_bin) = forward_spectrum(buf);
    let (mut num, mut den) = (0.0f64, 0.0f64);
    for (k, (r, i)) in re.iter().zip(&im).enumerate() {
        let p = f64::from(r * r + i * i);
        num += k as f64 * f64::from(hz_per_bin) * p;
        den += p;
    }
    (num / den.max(1e-12)) as f32
}

/// Fraction of spectral power inside `[lo_hz, hi_hz)`.
#[cfg(test)]
pub fn band_energy_share(buf: &[f32], lo_hz: f32, hi_hz: f32) -> f32 {
    let (re, im, tw, hz_per_bin) = forward_spectrum(buf);
    let (mut band, mut total) = (0.0f64, 0.0f64);
    for k in 1..=tw.len() {
        let p = f64::from(re[k] * re[k] + im[k] * im[k]);
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
        let tw = twiddles(256);
        let (re, im) = spectrum(&orig, &tw);
        for (a, b) in orig.iter().zip(&signal(&re, &im, &tw)) {
            assert!((a - b).abs() < 1e-4, "{a} vs {b}");
        }
    }

    /// Every power of two, odd and even exponents alike (the radix-2² passes
    /// end on a lone radix-2 stage only for the odd ones), against the DFT's
    /// own sum.
    #[test]
    fn the_spectrum_is_the_direct_dft_at_every_length() {
        for n in (1..=10).map(|e| 1usize << e) {
            let mut rng = NoiseStream::new(n as u64);
            let x: Vec<f32> = (0..n).map(|_| rng.norm()).collect();
            let tw = twiddles(n);
            let (re, im) = spectrum(&x, &tw);
            for k in 0..=n / 2 {
                let (mut dr, mut di) = (0.0f64, 0.0f64);
                for (t, &v) in x.iter().enumerate() {
                    let a = -2.0 * std::f64::consts::PI * (k * t) as f64 / n as f64;
                    dr += f64::from(v) * a.cos();
                    di += f64::from(v) * a.sin();
                }
                let tol = 1e-3 * (n as f64).sqrt();
                assert!(
                    (f64::from(re[k]) - dr).abs() < tol && (f64::from(im[k]) - di).abs() < tol,
                    "n {n} bin {k}: ({}, {}) vs ({dr}, {di})",
                    re[k],
                    im[k]
                );
            }
            let back = signal(&re, &im, &tw);
            assert!(
                x.iter().zip(&back).all(|(a, b)| (a - b).abs() < 1e-4),
                "n {n} round trip"
            );
        }
    }

    #[test]
    fn bandpass_keeps_in_band_and_kills_out_of_band() {
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
    fn warp_resample_preserves_a_tone_and_is_identity_at_zero_dev() {
        let tone: Vec<f32> = (0..44_100)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SAMPLE_RATE as f32).sin())
            .collect();
        // zero deviation is bit-exact identity: the read head never moves
        let same = warp_resample(&tone, &[(0.7, 0.0)]);
        assert_eq!(same, tone);
        // the shipped wow/flutter pair — a deliberately subtle warble
        let warped = warp_resample(&tone, &[(0.7, 0.0025), (8.0, 0.0006)]);
        let c = centroid_hz(&warped);
        assert!((400.0..480.0).contains(&c), "centroid {c} strayed from 440");
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        assert!(
            rms(&warped) > 0.9 * rms(&tone),
            "warble must not lose power"
        );
    }

    #[test]
    fn shaped_noise_matches_its_target_envelope() {
        let mut rng = NoiseStream::new(7);
        let bands = [(100.0, 1000.0, 70.0), (1000.0, 8000.0, 30.0)];
        let buf = shaped_noise_loop(1 << 16, &bands, &mut rng);
        let low = band_energy_share(&buf, 100.0, 1000.0);
        let high = band_energy_share(&buf, 1000.0, 8000.0);
        assert!((low - 0.70).abs() < 0.05, "low band {low}");
        assert!((high - 0.30).abs() < 0.05, "high band {high}");
    }
}
