// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) The Audiform27 contributors.

//! Spectral noise reduction by STFT gain masking.
//!
//! The noise floor per frequency bin is either learned from a "noise
//! print" (a stretch of only noise the user picked) or estimated as a
//! low percentile of each bin's level over the whole selection. Each
//! bin then gets a gain that falls towards `floor` as its level nears the
//! noise floor, smoothed over time and frequency to avoid warbling.

use crate::fft::{fft, hann};

const SIZE: usize = 2048;
const HOP: usize = SIZE / 4;

/// Per-bin noise magnitudes, from audio known to be only noise.
#[derive(Debug, Clone, PartialEq)]
pub struct NoisePrint {
    pub bins: Vec<f64>,
}

fn frames(data: &[f32]) -> impl Iterator<Item = (usize, Vec<f64>, Vec<f64>)> + '_ {
    let win = hann(SIZE);
    let count = data.len().div_ceil(HOP) + 1;
    (0..count).map(move |f| {
        let start = f as i64 * HOP as i64 - SIZE as i64 / 2;
        let mut re = vec![0.0; SIZE];
        let mut im = vec![0.0; SIZE];
        for k in 0..SIZE {
            let idx = start + k as i64;
            if idx >= 0 && (idx as usize) < data.len() {
                re[k] = data[idx as usize] as f64 * win[k];
            }
        }
        fft(&mut re, &mut im, false);
        (f, re, im)
    })
}

pub fn learn(data: &[f32]) -> NoisePrint {
    let mut sum = vec![0.0; SIZE / 2 + 1];
    let mut n = 0.0;
    for (_, re, im) in frames(data) {
        for (b, s) in sum.iter_mut().enumerate() {
            *s += re[b].hypot(im[b]);
        }
        n += 1.0;
    }
    NoisePrint {
        bins: sum.into_iter().map(|s| s / f64::max(n, 1.0)).collect(),
    }
}

/// Estimates the noise floor as the 10th percentile of each bin.
pub fn estimate(data: &[f32]) -> NoisePrint {
    let mut per_bin: Vec<Vec<f64>> = vec![Vec::new(); SIZE / 2 + 1];
    for (_, re, im) in frames(data) {
        for (b, v) in per_bin.iter_mut().enumerate() {
            v.push(re[b].hypot(im[b]));
        }
    }
    NoisePrint {
        bins: per_bin
            .into_iter()
            .map(|mut v| {
                if v.is_empty() {
                    return 0.0;
                }
                v.sort_by(f64::total_cmp);
                v[v.len() / 10]
            })
            .collect(),
    }
}

/// Reduces noise in `data` in place.
/// `reduction_db`: how far noise is pushed down (e.g. 12).
/// `sensitivity`: multiplier on the noise floor before masking (1..4).
pub fn reduce(data: &mut [f32], print: &NoisePrint, reduction_db: f64, sensitivity: f64) {
    if data.is_empty() {
        return;
    }
    let floor = 10f64.powf(-reduction_db.abs() / 20.0);
    let win = hann(SIZE);
    let mut out = vec![0.0f64; data.len() + SIZE];
    let mut norm = vec![0.0f64; data.len() + SIZE];
    let mut prev_gain = vec![1.0f64; SIZE / 2 + 1];
    let input = data.to_vec();
    for (f, mut re, mut im) in frames(&input) {
        let mut gain: Vec<f64> = (0..=SIZE / 2)
            .map(|b| {
                let mag = re[b].hypot(im[b]);
                let thr = print.bins.get(b).copied().unwrap_or(0.0) * sensitivity;
                if mag <= 1e-12 {
                    floor
                } else {
                    // Soft knee: full gain at 2x the threshold, floor at 1x.
                    let r = ((mag / thr.max(1e-12)) - 1.0).clamp(0.0, 1.0);
                    floor + (1.0 - floor) * r * r
                }
            })
            .collect();
        // Smooth across frequency, then attack fast / release slow in time.
        let smooth: Vec<f64> = (0..gain.len())
            .map(|b| {
                let lo = b.saturating_sub(2);
                let hi = (b + 3).min(gain.len());
                gain[lo..hi].iter().sum::<f64>() / (hi - lo) as f64
            })
            .collect();
        for b in 0..gain.len() {
            let g = smooth[b];
            gain[b] = if g > prev_gain[b] {
                g
            } else {
                prev_gain[b] * 0.6 + g * 0.4
            };
            prev_gain[b] = gain[b];
        }
        for b in 0..=SIZE / 2 {
            re[b] *= gain[b];
            im[b] *= gain[b];
            if b > 0 && b < SIZE / 2 {
                re[SIZE - b] *= gain[b];
                im[SIZE - b] *= gain[b];
            }
        }
        fft(&mut re, &mut im, true);
        let start = f as i64 * HOP as i64 - SIZE as i64 / 2;
        for k in 0..SIZE {
            let idx = start + k as i64;
            if idx >= 0 && (idx as usize) < data.len() {
                out[idx as usize] += re[k] * win[k];
                norm[idx as usize] += win[k] * win[k];
            }
        }
    }
    for (i, s) in data.iter_mut().enumerate() {
        if norm[i] > 1e-9 {
            *s = (out[i] / norm[i]) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiss_drops_and_tone_stays() {
        let rate = 48_000.0;
        let mut seed = 1u32;
        let mut rng = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
        };
        let n = 48_000;
        let noise: Vec<f32> = (0..n).map(|_| rng() * 0.02).collect();
        let tone: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate).sin() * 0.5)
            .collect();
        let print = learn(&noise);
        let mut only_noise = noise.clone();
        reduce(&mut only_noise, &print, 20.0, 1.5);
        let rms = |v: &[f32]| (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt();
        assert!(rms(&only_noise[4000..44_000]) < rms(&noise[4000..44_000]) * 0.3);
        let mut mixed: Vec<f32> = tone.iter().zip(&noise).map(|(a, b)| a + b).collect();
        reduce(&mut mixed, &print, 20.0, 1.5);
        let err = rms(&mixed[4000..44_000]
            .iter()
            .zip(&tone[4000..44_000])
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>());
        assert!(err < 0.02, "tone damaged: {err}");
    }
}
