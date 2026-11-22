// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Dialogue focus: what a stereo recording has dead centre (a voice, most
//! often) taken out of the two sides, so it can be given to the centre
//! speaker on its own.
//!
//! Offline, a short-time Fourier transform: frames of [`FRAME`] samples a
//! quarter frame apart, sine windows in and out, nothing late. Per bin the
//! two sides' powers and cross power are smoothed over time and over the
//! neighbouring bins; the more alike and in phase the sides are, the more
//! of their shared middle is taken. The voice band (about 200 Hz to 6 kHz)
//! is taken fully; the lows are left alone and the very highs taken
//! lightly. What is taken comes off both sides, so left minus taken, right
//! minus taken, and the taken part add back up to the original.

use realfft::num_complex::Complex32;
use realfft::RealFftPlanner;

const FRAME: usize = 2048;
const HOP: usize = FRAME / 4;
const BINS: usize = FRAME / 2 + 1;
/// Smoothing of the powers per frame.
const SMOOTH: f32 = 0.6;

/// How much of each frequency a full Dialogue focus takes.
fn band(hz: f32) -> f32 {
    if hz < 80.0 {
        0.0
    } else if hz < 200.0 {
        (hz - 80.0) / 120.0
    } else if hz <= 6000.0 {
        1.0
    } else if hz < 12_000.0 {
        1.0 - 0.7 * (hz - 6000.0) / 6000.0
    } else {
        0.3
    }
}

/// Splits `l` and `r` into (left without the centre, right without the
/// centre, the centre). `amount` 0 to 1 is how much is taken. The centre
/// part is what both sides had: give it to a centre speaker at √2 to keep
/// its loudness (two speakers' worth of it in one).
pub fn extract(l: &[f32], r: &[f32], rate: u32, amount: f32) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let n = l.len().min(r.len());
    let amount = amount.clamp(0.0, 1.0);
    let mut ol = vec![0.0f32; n];
    let mut or = vec![0.0f32; n];
    let mut oc = vec![0.0f32; n];
    if n == 0 || amount == 0.0 {
        ol.copy_from_slice(&l[..n]);
        or.copy_from_slice(&r[..n]);
        return (ol, or, oc);
    }
    let mut planner = RealFftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(FRAME);
    let inv = planner.plan_fft_inverse(FRAME);
    let mut sf = fwd.make_scratch_vec();
    let mut si = inv.make_scratch_vec();
    let window: Vec<f32> = (0..FRAME)
        .map(|i| (std::f32::consts::PI * (i as f32 + 0.5) / FRAME as f32).sin())
        .collect();
    let weight: Vec<f32> = (0..BINS)
        .map(|k| band(k as f32 * rate as f32 / FRAME as f32))
        .collect();
    let mut time = vec![0.0f32; FRAME];
    let mut spec_l = vec![Complex32::default(); BINS];
    let mut spec_r = vec![Complex32::default(); BINS];
    let mut spec_c = vec![Complex32::default(); BINS];
    let (mut pll, mut prr) = (vec![0.0f32; BINS], vec![0.0f32; BINS]);
    let mut plr = vec![Complex32::default(); BINS];
    let mut gain = vec![0.0f32; BINS];
    // Sine in and out, a quarter frame apart: the windows' squares add to 2.
    let scale = 0.5 / FRAME as f32;
    let mut start = -((FRAME - HOP) as isize);
    while start < n as isize {
        for (src, spec) in [(l, &mut spec_l), (r, &mut spec_r)] {
            for (i, (t, w)) in time.iter_mut().zip(&window).enumerate() {
                let at = start + i as isize;
                *t = if at >= 0 && (at as usize) < n {
                    src[at as usize] * w
                } else {
                    0.0
                };
            }
            let _ = fwd.process_with_scratch(&mut time, spec, &mut sf);
        }
        for k in 0..BINS {
            let (a, b) = (spec_l[k], spec_r[k]);
            pll[k] = SMOOTH * pll[k] + (1.0 - SMOOTH) * a.norm_sqr();
            prr[k] = SMOOTH * prr[k] + (1.0 - SMOOTH) * b.norm_sqr();
            plr[k] = plr[k] * SMOOTH + a * b.conj() * (1.0 - SMOOTH);
        }
        for k in 0..BINS {
            let (lo, hi) = (k.saturating_sub(2), (k + 2).min(BINS - 1));
            let el: f32 = pll[lo..=hi].iter().sum();
            let er: f32 = prr[lo..=hi].iter().sum();
            let cr: f32 = plr[lo..=hi].iter().map(|v| v.re).sum();
            // 1 when the sides are the same, 0 when unrelated, opposite or
            // only on one side.
            let alike = if el + er > 1e-20 {
                (2.0 * cr.max(0.0) / (el + er)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            gain[k] = amount * weight[k] * alike * alike;
        }
        for k in 0..BINS {
            let mid = (spec_l[k] + spec_r[k]) * (0.5 * gain[k]);
            spec_c[k] = mid;
            spec_l[k] -= mid;
            spec_r[k] -= mid;
        }
        for (spec, out) in [(&mut spec_l, &mut ol), (&mut spec_r, &mut or), (&mut spec_c, &mut oc)] {
            spec[0].im = 0.0;
            spec[BINS - 1].im = 0.0;
            let _ = inv.process_with_scratch(spec, &mut time, &mut si);
            for (i, (t, w)) in time.iter().zip(&window).enumerate() {
                let at = start + i as isize;
                if at >= 0 && (at as usize) < n {
                    out[at as usize] += t * w * scale;
                }
            }
        }
        start += HOP as isize;
    }
    (ol, or, oc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, f: f32) -> Vec<f32> {
        (0..n)
            .map(|i| (i as f32 * f * std::f32::consts::TAU / 48_000.0).sin() * 0.5)
            .collect()
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
    }

    #[test]
    fn the_parts_add_back_to_the_original() {
        let n = 24_000;
        let a = tone(n, 1000.0);
        let b: Vec<f32> = tone(n, 333.0).iter().zip(&a).map(|(x, y)| x * 0.5 + y).collect();
        let (l, r, c) = extract(&a, &b, 48_000, 1.0);
        for i in 0..n {
            assert!((l[i] + c[i] - a[i]).abs() < 1e-4, "left at {i}");
            assert!((r[i] + c[i] - b[i]).abs() < 1e-4, "right at {i}");
        }
    }

    #[test]
    fn a_centred_voice_goes_to_the_centre_and_a_panned_one_stays() {
        let n = 48_000;
        let v = tone(n, 800.0);
        let (l, r, c) = extract(&v, &v, 48_000, 1.0);
        let mid = 4096..n - 4096;
        assert!(rms(&c[mid.clone()]) > 0.95 * rms(&v[mid.clone()]));
        assert!(rms(&l[mid.clone()]) < 0.05 && rms(&r[mid.clone()]) < 0.05);
        // Only on the left: nothing shared, nothing taken.
        let z = vec![0.0; n];
        let (l, _, c) = extract(&v, &z, 48_000, 1.0);
        assert!(rms(&c[mid.clone()]) < 1e-3);
        assert!((rms(&l[mid.clone()]) - rms(&v[mid])).abs() < 1e-3);
        // The lows stay where they were.
        let bass = tone(n, 50.0);
        let (_, _, c) = extract(&bass, &bass, 48_000, 1.0);
        assert!(rms(&c[4096..n - 4096]) < 0.02);
    }
}
