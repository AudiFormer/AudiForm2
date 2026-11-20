// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Time stretch and pitch shift, offline.
//!
//! Written from the textbook phase vocoder (Flanagan & Golden; Dolson's
//! tutorial) with Laroche & Dolson's identity phase locking:
//!
//! * Frames are taken from the input every `Ha` samples and laid down
//!   every `Hs` samples (`Hs / Ha` = the time ratio). Each spectral peak
//!   gets its phase advanced by its measured instantaneous frequency
//!   times `Hs`; every other bin keeps its phase offset from the peak
//!   whose region it is in (identity locking), which keeps partials
//!   coherent and avoids the "phasey" sound of a plain vocoder.
//! * Channels are linked: the phase rotation is worked out once from the
//!   sum of all channels and applied to each, so the stereo image (the
//!   phase differences between channels) is kept exactly.
//! * Transients: a spectral-flux onset detector marks frames where a
//!   sharp attack sits; there the phases are reset to the analysis
//!   phases, so drums and consonants stay crisp instead of smeared.
//! * Pitch: the audio is stretched by an extra factor `2^(st/12)` and
//!   then resampled by that factor (windowed sinc), which leaves the
//!   length right and moves the pitch.
//! * Formants: with `preserve_formants`, each frame's spectral envelope
//!   (from a low-quefrency cepstral lifter) is measured and the frame is
//!   re-weighted so that after the resampling the envelope lands where it
//!   was: the pitch moves, the voice's character does not.
//! * Varispeed is plain resampling: speed and pitch together.

#![allow(clippy::needless_range_loop)]

use std::cell::RefCell;
use std::f64::consts::PI;
use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

/// Planned forward and inverse transforms of one size, used like
/// [`crate::fft::fft`] (split real and imaginary parts, inverse scaled).
struct Ffts {
    fwd: Arc<dyn rustfft::Fft<f64>>,
    inv: Arc<dyn rustfft::Fft<f64>>,
    buf: RefCell<Vec<Complex<f64>>>,
    scratch: RefCell<Vec<Complex<f64>>>,
}

impl Ffts {
    fn new(n: usize) -> Ffts {
        let mut planner = FftPlanner::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let scratch = fwd
            .get_inplace_scratch_len()
            .max(inv.get_inplace_scratch_len());
        Ffts {
            fwd,
            inv,
            buf: RefCell::new(vec![Complex::new(0.0, 0.0); n]),
            scratch: RefCell::new(vec![Complex::new(0.0, 0.0); scratch]),
        }
    }

    fn run(&self, re: &mut [f64], im: &mut [f64], inverse: bool) {
        let mut buf = self.buf.borrow_mut();
        let mut scratch = self.scratch.borrow_mut();
        for ((z, r), i) in buf.iter_mut().zip(re.iter()).zip(im.iter()) {
            *z = Complex::new(*r, *i);
        }
        let plan = if inverse { &self.inv } else { &self.fwd };
        plan.process_with_scratch(&mut buf, &mut scratch);
        let k = if inverse { 1.0 / re.len() as f64 } else { 1.0 };
        for ((z, r), i) in buf.iter().zip(re.iter_mut()).zip(im.iter_mut()) {
            *r = z.re * k;
            *i = z.im * k;
        }
    }
}

/// Which algorithm, by material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// One voice or instrument: shorter frames, tighter timing.
    Monophonic,
    /// Chords and full mixes: longer frames, cleaner tone.
    Polyphonic,
    /// Tape-style resampling.
    Varispeed,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// Output length over input length (2 plays half speed).
    pub ratio: f64,
    /// Pitch shift in semitones (ignored by Varispeed).
    pub pitch: f64,
    pub kind: Kind,
    /// Analysis window, milliseconds.
    pub window_ms: f32,
    /// Transient sensitivity 0..100 (0: never reset phases).
    pub transients: f32,
    pub preserve_formants: bool,
    /// More overlap between frames: smoother, slower.
    pub high_precision: bool,
}

impl Params {
    /// The settings a clip's stretch applies.
    pub fn from_clip(s: &af27_core::session::ClipStretch) -> Params {
        use af27_core::session::StretchKind as K;
        Params {
            ratio: s.applied_ratio(),
            pitch: s.applied_pitch() as f64,
            kind: match s.kind {
                K::Monophonic => Kind::Monophonic,
                K::Polyphonic => Kind::Polyphonic,
                K::Varispeed => Kind::Varispeed,
            },
            window_ms: s.window_ms,
            transients: s.transients,
            preserve_formants: s.preserve_formants,
            high_precision: s.high_precision,
        }
    }

    /// Frame size (a power of two) and overlap for a rate.
    fn frame(&self, rate: u32) -> (usize, usize) {
        let ms = self.window_ms.clamp(5.0, 200.0) as f64;
        let want = ms / 1000.0 * rate as f64;
        let n = match self.kind {
            Kind::Polyphonic => (want * 2.0).ceil() as usize,
            _ => want.ceil() as usize,
        }
        .next_power_of_two()
        .clamp(256, 16_384);
        let overlap = if self.high_precision { 8 } else { 4 };
        (n, overlap)
    }
}

/// Renders `out_len` frames per channel. Output frame `j` is the source
/// heard at source sample `in_start + j / ratio` (source samples outside
/// the slices count as silence, so a span near a file's edge keeps its
/// timing; audio around the span is used as context). `cancel` is asked
/// now and then; when it returns true the work stops and None comes back.
pub fn render(
    src: &[&[f32]],
    rate: u32,
    p: &Params,
    in_start: f64,
    out_len: usize,
    cancel: &dyn Fn() -> bool,
) -> Option<Vec<Vec<f32>>> {
    let ratio = p.ratio.clamp(0.05, 20.0);
    if src.is_empty() {
        return Some(Vec::new());
    }
    if p.kind == Kind::Varispeed {
        let r = Resampler::new(1.0 / ratio);
        return Some(src.iter().map(|c| r.read(c, in_start, out_len)).collect());
    }
    let pf = 2f64.powf(p.pitch.clamp(-36.0, 36.0) / 12.0);
    if (pf - 1.0).abs() < 1e-9 {
        return vocode(src, rate, p, ratio, in_start, 0, out_len, 1.0, cancel);
    }
    // Stretch by `ratio * pf`, then read that back `pf` times as fast.
    // The vocoder's output index 0 sits `pre` samples in, so the
    // resampler has context on both sides.
    let r = Resampler::new(pf);
    let pre = r.reach() + 2;
    let len = (out_len as f64 * pf).ceil() as usize + 2 * pre + 2;
    let y = vocode(
        src,
        rate,
        p,
        ratio * pf,
        in_start,
        pre as i64,
        len,
        pf,
        cancel,
    )?;
    Some(y.iter().map(|c| r.read(c, pre as f64, out_len)).collect())
}

/// Wraps a phase into -pi..pi.
#[inline]
fn princarg(x: f64) -> f64 {
    x - 2.0 * PI * (x / (2.0 * PI)).round()
}

/// The phase vocoder. Output sample `m - pre` (m in 0..len) is the
/// source around `in_start + (m - pre) / r`. `pf` is the pitch factor the
/// caller resamples by afterwards (for the formant correction).
#[allow(clippy::too_many_arguments)]
fn vocode(
    src: &[&[f32]],
    rate: u32,
    p: &Params,
    r: f64,
    in_start: f64,
    pre: i64,
    len: usize,
    pf: f64,
    cancel: &dyn Fn() -> bool,
) -> Option<Vec<Vec<f32>>> {
    let ch = src.len();
    let (n, overlap) = p.frame(rate);
    let half = n / 2;
    let bins = half + 1;
    // Synthesis hop; when compressing, a smaller one keeps the analysis
    // hop (and so the phase-unwrapping range) at its usual size.
    let base_hop = n / overlap;
    let hs = if r >= 1.0 {
        base_hop
    } else {
        ((base_hop as f64 * r).round() as usize).max(16)
    };
    let win: Vec<f64> = (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos())
        .collect();
    let fx = Ffts::new(n);
    // Frame k is centred on output position k*hs (vector index
    // k*hs + pre); it reads the input centred on in_start + k*hs / r.
    let out_first = -pre;
    let out_last = len as i64 - pre;
    let k0 = (out_first - half as i64).div_euclid(hs as i64) - 1;
    let k1 = (out_last + half as i64).div_euclid(hs as i64) + 1;
    let centre_in = |k: i64| -> i64 { (in_start + (k * hs as i64) as f64 / r).round() as i64 };

    // Read a windowed frame of every channel into packed pairs.
    let pairs = ch.div_ceil(2);
    let mut re = vec![vec![0.0f64; n]; pairs];
    let mut im = vec![vec![0.0f64; n]; pairs];
    let load = |re: &mut [Vec<f64>], im: &mut [Vec<f64>], c_in: i64| {
        let s0 = c_in - half as i64;
        for pi in 0..pairs {
            let a = src[pi * 2];
            let b = src.get(pi * 2 + 1).copied();
            for i in 0..n {
                let x = s0 + i as i64;
                let get = |c: &[f32]| {
                    if x >= 0 && (x as usize) < c.len() {
                        c[x as usize] as f64
                    } else {
                        0.0
                    }
                };
                re[pi][i] = get(a) * win[i];
                im[pi][i] = b.map_or(0.0, get) * win[i];
            }
            fx.run(&mut re[pi], &mut im[pi], false);
        }
    };
    // Bin `b` of channel `c` from the packed transforms.
    let unpack = |re: &[Vec<f64>], im: &[Vec<f64>], c: usize, b: usize| -> (f64, f64) {
        let pi = c / 2;
        let (zr, zi) = (re[pi][b], im[pi][b]);
        let nb = (n - b) % n;
        let (wr, wi) = (re[pi][nb], -im[pi][nb]);
        if c & 1 == 0 {
            ((zr + wr) * 0.5, (zi + wi) * 0.5)
        } else {
            // (Z - conj Z[-b]) / 2i
            ((zi - wi) * 0.5, -(zr - wr) * 0.5)
        }
    };

    // Pass 1: onsets, from the spectral flux of the summed spectrum.
    let frames = (k1 - k0 + 1) as usize;
    let sens = p.transients.clamp(0.0, 100.0) as f64 / 100.0;
    let mut onset = vec![false; frames];
    if sens > 0.0 {
        let mut flux = vec![0.0f64; frames];
        let mut energy = vec![0.0f64; frames];
        let mut prev = vec![0.0f64; bins];
        let mut prev_c = i64::MIN;
        for (fi, k) in (k0..=k1).enumerate() {
            if fi % 64 == 0 && cancel() {
                return None;
            }
            let c_in = centre_in(k);
            load(&mut re, &mut im, c_in);
            let mut up = 0.0;
            let mut sum = 0.0;
            for b in 0..bins {
                let (mut mr, mut mi) = (0.0, 0.0);
                for c in 0..ch {
                    let (a, bb) = unpack(&re, &im, c, b);
                    mr += a;
                    mi += bb;
                }
                let m = (mr * mr + mi * mi).sqrt();
                up += (m - prev[b]).max(0.0);
                sum += prev[b];
                prev[b] = m;
            }
            // Per standard hop, so the measure does not depend on the ratio.
            let ha = if prev_c == i64::MIN {
                base_hop as f64
            } else {
                ((c_in - prev_c) as f64).max(1.0)
            };
            prev_c = c_in;
            flux[fi] = up / (sum + 1e-9) * (base_hop as f64 / ha).min(8.0);
            energy[fi] = up + sum;
        }
        // Peaks of the flux above a threshold (lower with more
        // sensitivity), at least a frame length apart in the input.
        let thr = 0.12 + 2.5 * (1.0 - sens) * (1.0 - sens);
        let floor = energy.iter().cloned().fold(0.0, f64::max) * 1e-4;
        let mut last_in = i64::MIN / 2;
        for fi in 1..frames.saturating_sub(1) {
            let f = flux[fi];
            if f > thr && f >= flux[fi - 1] && f > flux[fi + 1] && energy[fi] > floor {
                let c_in = centre_in(k0 + fi as i64);
                if c_in - last_in >= n as i64 {
                    onset[fi] = true;
                    last_in = c_in;
                }
            }
        }
    }

    // Pass 2: synthesis. Per bin only the phase change from analysis to
    // synthesis is kept (`d`); within a peak's region it is the same for
    // every bin, so the trigonometry runs once per peak.
    let mut out = vec![vec![0.0f64; len]; ch];
    let mut wsum = vec![0.0f64; len];
    let mut spec = vec![vec![(0.0f64, 0.0f64); bins]; ch];
    let mut mid = vec![(0.0f64, 0.0f64); bins];
    let mut prev_mid = vec![(0.0f64, 0.0f64); bins];
    let mut mag = vec![0.0f64; bins];
    let mut d = vec![0.0f64; bins];
    let mut prev_d = vec![0.0f64; bins];
    let mut rot = vec![(1.0f64, 0.0f64); bins];
    let mut peaks: Vec<usize> = Vec::with_capacity(bins / 2);
    let mut fgain = vec![1.0f64; bins];
    let formants = p.preserve_formants && (pf - 1.0).abs() > 1e-6;
    let lifter = ((rate as f64 * 0.0012).round() as usize).clamp(8, half / 4);
    let mut cr = vec![0.0f64; n];
    let mut ci = vec![0.0f64; n];
    let mut env = vec![0.0f64; bins];
    let mut ore = vec![vec![0.0f64; n]; pairs];
    let mut oim = vec![vec![0.0f64; n]; pairs];
    let mut first = true;
    let mut prev_c = 0i64;
    let arg = |z: (f64, f64)| z.1.atan2(z.0);
    for (fi, k) in (k0..=k1).enumerate() {
        if fi % 64 == 0 && cancel() {
            return None;
        }
        let c_in = centre_in(k);
        load(&mut re, &mut im, c_in);
        for (c, sp) in spec.iter_mut().enumerate() {
            for (b, z) in sp.iter_mut().enumerate() {
                *z = unpack(&re, &im, c, b);
            }
        }
        for b in 0..bins {
            let (mut mr, mut mi) = (0.0, 0.0);
            for sp in &spec {
                mr += sp[b].0;
                mi += sp[b].1;
            }
            mid[b] = (mr, mi);
            mag[b] = (mr * mr + mi * mi).sqrt();
        }
        let ha = (c_in - prev_c) as f64;
        // Peaks: a bin above both neighbours.
        peaks.clear();
        if !(first || onset[fi] || ha <= 0.0) {
            for b in 1..bins - 1 {
                let m = mag[b];
                if m > mag[b - 1] && m >= mag[b + 1] {
                    peaks.push(b);
                }
            }
        }
        if peaks.is_empty() {
            // First frame, a transient or silence: analysis phases as they are.
            d.fill(0.0);
            rot.fill((1.0, 0.0));
        } else {
            let mut lo = 0usize;
            for (pi, &pk) in peaks.iter().enumerate() {
                // The peak's synthesis phase from its instantaneous frequency.
                let (phi, prev_phi) = (arg(mid[pk]), arg(prev_mid[pk]));
                let omega = 2.0 * PI * pk as f64 / n as f64;
                let dev = princarg(phi - prev_phi - omega * ha);
                let inst = omega + dev / ha;
                let theta = prev_phi + prev_d[pk] + inst * hs as f64;
                let dp = princarg(theta - phi);
                let rp = (dp.cos(), dp.sin());
                // Its region runs to the lowest bin before the next peak;
                // every bin in it keeps its phase offset from the peak.
                let hi = match peaks.get(pi + 1) {
                    Some(&nx) => {
                        let mut m = pk;
                        for b in pk..nx {
                            if mag[b] < mag[m] {
                                m = b;
                            }
                        }
                        m
                    }
                    None => bins,
                };
                d[lo..hi].fill(dp);
                rot[lo..hi].fill(rp);
                lo = hi;
            }
        }
        first = false;
        prev_c = c_in;
        std::mem::swap(&mut prev_mid, &mut mid);
        std::mem::swap(&mut prev_d, &mut d);

        if formants {
            // Real cepstrum of the log magnitude, low quefrencies only.
            for b in 0..n {
                let bb = if b <= half { b } else { n - b };
                cr[b] = (mag[bb] + 1e-9).ln();
                ci[b] = 0.0;
            }
            fx.run(&mut cr, &mut ci, true);
            for q in lifter..=(n - lifter) {
                cr[q] = 0.0;
            }
            for v in ci.iter_mut() {
                *v = 0.0;
            }
            fx.run(&mut cr, &mut ci, false);
            env.copy_from_slice(&cr[..bins]);
            // After resampling, bin b moves to b*pf; it should then carry
            // the envelope found at b*pf.
            for b in 0..bins {
                let x = b as f64 * pf;
                let e = if x >= half as f64 {
                    env[half]
                } else {
                    let i = x as usize;
                    let f = x - i as f64;
                    env[i] + (env[i + 1] - env[i]) * f
                };
                fgain[b] = (e - env[b]).clamp(-3.5, 3.5).exp();
            }
        }

        // Rotate every channel by the shared phase change and invert.
        for pi in 0..pairs {
            ore[pi].fill(0.0);
            oim[pi].fill(0.0);
        }
        for (c, sp) in spec.iter().enumerate() {
            let pi = c / 2;
            let second = c & 1 == 1;
            for b in 0..bins {
                let (xr, xi) = sp[b];
                let (cs, sn) = rot[b];
                let g = if formants { fgain[b] } else { 1.0 };
                let (yr, yi) = if b == 0 || b == half {
                    // DC and Nyquist stay real.
                    (xr * g, 0.0)
                } else {
                    ((xr * cs - xi * sn) * g, (xr * sn + xi * cs) * g)
                };
                // Pack: Z = Y_a + i Y_b, with the mirrored half conjugate.
                let (zr, zi) = if second { (-yi, yr) } else { (yr, yi) };
                ore[pi][b] += zr;
                oim[pi][b] += zi;
                if b != 0 && b != half {
                    let (mr, mi) = if second { (yi, yr) } else { (yr, -yi) };
                    ore[pi][n - b] += mr;
                    oim[pi][n - b] += mi;
                }
            }
        }
        let o0 = k * hs as i64 - half as i64 + pre;
        for pi in 0..pairs {
            fx.run(&mut ore[pi], &mut oim[pi], true);
            for i in 0..n {
                let m = o0 + i as i64;
                if m < 0 || m as usize >= len {
                    continue;
                }
                let m = m as usize;
                let w = win[i];
                out[pi * 2][m] += ore[pi][i] * w;
                if pi * 2 + 1 < ch {
                    out[pi * 2 + 1][m] += oim[pi][i] * w;
                }
                if pi == 0 {
                    wsum[m] += w * w;
                }
            }
        }
    }
    // Overlap-add normalisation.
    let norm_floor = wsum.iter().cloned().fold(0.0, f64::max) * 0.1;
    Some(
        out.into_iter()
            .map(|c| {
                c.iter()
                    .zip(&wsum)
                    .map(|(v, w)| (v / w.max(norm_floor).max(1e-9)) as f32)
                    .collect()
            })
            .collect(),
    )
}

/// Windowed-sinc reader at a fixed step, from a table (fast enough for
/// whole songs): `out[j]` = the input at `origin + j * step`, low-passed
/// below the output's Nyquist when stepping faster than one.
pub struct Resampler {
    step: f64,
    cutoff: f64,
    table: Vec<f32>,
}

const TAPS: usize = 16;
const PHASES: usize = 512;

impl Resampler {
    pub fn new(step: f64) -> Resampler {
        let cutoff = if step > 1.0 { 0.97 / step } else { 0.97 };
        // Blackman-windowed sinc over |x| < TAPS, PHASES points per unit.
        let size = TAPS * PHASES + 2;
        let table = (0..size)
            .map(|i| {
                let x = i as f64 / PHASES as f64;
                if x >= TAPS as f64 {
                    return 0.0;
                }
                let s = if x < 1e-12 {
                    1.0
                } else {
                    (PI * x).sin() / (PI * x)
                };
                let wpos = 0.5 + x / (2.0 * TAPS as f64);
                let w = 0.42 - 0.5 * (2.0 * PI * wpos).cos() + 0.08 * (4.0 * PI * wpos).cos();
                (s * w) as f32
            })
            .collect();
        Resampler {
            step,
            cutoff,
            table,
        }
    }

    /// Input samples read on each side of a point.
    pub fn reach(&self) -> usize {
        (TAPS as f64 / self.cutoff).ceil() as usize + 1
    }

    #[inline]
    fn kernel(&self, x: f64) -> f32 {
        let t = x.abs() * PHASES as f64;
        let i = t as usize;
        if i + 1 >= self.table.len() {
            return 0.0;
        }
        let f = (t - i as f64) as f32;
        self.table[i] + (self.table[i + 1] - self.table[i]) * f
    }

    pub fn read(&self, input: &[f32], origin: f64, out_len: usize) -> Vec<f32> {
        let reach = self.reach() as i64;
        let len = input.len() as i64;
        let c = self.cutoff;
        (0..out_len)
            .map(|j| {
                let t = origin + j as f64 * self.step;
                let centre = t.floor() as i64;
                let mut acc = 0.0f32;
                let mut wsum = 0.0f32;
                for i in (centre - reach + 1)..=(centre + reach) {
                    let k = self.kernel((t - i as f64) * c);
                    wsum += k;
                    if i >= 0 && i < len {
                        acc += input[i as usize] * k;
                    }
                }
                if wsum.abs() > 1e-9 {
                    acc / wsum
                } else {
                    0.0
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fft::fft;

    const RATE: u32 = 48_000;

    fn sine(f: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * f * i as f64 / RATE as f64).sin() as f32 * 0.5)
            .collect()
    }

    fn params(ratio: f64, pitch: f64, kind: Kind) -> Params {
        Params {
            ratio,
            pitch,
            kind,
            window_ms: 30.0,
            transients: 40.0,
            preserve_formants: false,
            high_precision: false,
        }
    }

    /// The strongest frequency in `x` (Hz), by a windowed FFT with
    /// parabolic interpolation.
    fn peak_hz(x: &[f32]) -> f64 {
        let n = 16_384.min(x.len().next_power_of_two() / 2);
        let s = (x.len() - n) / 2;
        let mut re: Vec<f64> = (0..n)
            .map(|i| x[s + i] as f64 * (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()))
            .collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im, false);
        let m: Vec<f64> = (0..n / 2).map(|b| re[b].hypot(im[b])).collect();
        let b = (1..n / 2 - 1)
            .max_by(|a, b| m[*a].partial_cmp(&m[*b]).unwrap())
            .unwrap();
        let (a, c, d) = (m[b - 1].ln(), m[b].ln(), m[b + 1].ln());
        let off = 0.5 * (a - d) / (a - 2.0 * c + d);
        (b as f64 + off) * RATE as f64 / n as f64
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
    }

    #[test]
    fn ratio_two_is_twice_as_long_and_keeps_pitch() {
        let x = sine(440.0, RATE as usize);
        for kind in [Kind::Monophonic, Kind::Polyphonic] {
            let out = render(
                &[&x],
                RATE,
                &params(2.0, 0.0, kind),
                0.0,
                2 * x.len(),
                &|| false,
            )
            .unwrap();
            assert_eq!(out[0].len(), 2 * x.len());
            let mid = &out[0][10_000..86_000];
            let f = peak_hz(mid);
            assert!((f - 440.0).abs() < 2.0, "{kind:?}: {f} Hz");
            // Level kept (a steady sine: about the same RMS).
            let (a, b) = (rms(&x[5_000..43_000]), rms(mid));
            assert!((b / a - 1.0).abs() < 0.15, "{kind:?}: rms {a} -> {b}");
        }
    }

    #[test]
    fn pitch_shift_moves_the_peak() {
        let x = sine(440.0, RATE as usize);
        for (st, want) in [(12.0, 880.0), (-12.0, 220.0), (7.0, 440.0 * 1.4983)] {
            let mut p = params(1.0, st, Kind::Polyphonic);
            for formants in [false, true] {
                p.preserve_formants = formants;
                let out = render(&[&x], RATE, &p, 0.0, x.len(), &|| false).unwrap();
                assert_eq!(out[0].len(), x.len());
                let f = peak_hz(&out[0][4_000..44_000]);
                assert!(
                    (f / want - 1.0).abs() < 0.01,
                    "{st} st (formants {formants}): {f} Hz, want {want}"
                );
            }
        }
    }

    #[test]
    fn varispeed_changes_speed_and_pitch() {
        let x = sine(440.0, RATE as usize);
        let out = render(
            &[&x],
            RATE,
            &params(2.0, 0.0, Kind::Varispeed),
            0.0,
            2 * x.len(),
            &|| false,
        )
        .unwrap();
        assert_eq!(out[0].len(), 2 * x.len());
        let f = peak_hz(&out[0][10_000..86_000]);
        assert!((f - 220.0).abs() < 1.0, "{f} Hz");
    }

    #[test]
    fn stereo_keeps_its_channels_apart() {
        let l = sine(440.0, RATE as usize / 2);
        let r = vec![0.0f32; l.len()];
        let out = render(
            &[&l, &r],
            RATE,
            &params(1.5, 0.0, Kind::Polyphonic),
            0.0,
            (l.len() as f64 * 1.5) as usize,
            &|| false,
        )
        .unwrap();
        assert!(rms(&out[0]) > 0.2);
        assert!(rms(&out[1]) < 1e-4, "right {}", rms(&out[1]));
    }

    #[test]
    fn cancel_stops() {
        let x = sine(440.0, RATE as usize);
        assert!(render(
            &[&x],
            RATE,
            &params(2.0, 0.0, Kind::Monophonic),
            0.0,
            1000,
            &|| true
        )
        .is_none());
    }

    #[test]
    fn a_click_stays_sharp() {
        // One click in silence, stretched 2x: it should land at twice its
        // time and not smear far either side.
        let mut x = vec![0.0f32; RATE as usize];
        for i in 0..64 {
            x[24_000 + i] = (1.0 - i as f32 / 64.0) * if i % 2 == 0 { 1.0 } else { -1.0 };
        }
        let mut p = params(2.0, 0.0, Kind::Monophonic);
        p.transients = 80.0;
        let out = render(&[&x], RATE, &p, 0.0, 2 * x.len(), &|| false).unwrap();
        let y = &out[0];
        let peak = (0..y.len())
            .max_by(|a, b| y[*a].abs().partial_cmp(&y[*b].abs()).unwrap())
            .unwrap();
        assert!((peak as i64 - 48_000).abs() < 1_500, "peak at {peak}");
        let far: f64 = y[..peak.saturating_sub(4_000)]
            .iter()
            .chain(&y[(peak + 4_000).min(y.len())..])
            .map(|v| (*v as f64).abs())
            .fold(0.0, f64::max);
        assert!(far < 0.05, "smear {far}");
    }
}
