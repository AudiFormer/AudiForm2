// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Loudness and level statistics. Integrated loudness follows
//! ITU-R BS.1770-4: K-weighting, 400 ms blocks with 75 % overlap, an
//! absolute gate at -70 LUFS and a relative gate 10 LU below.

use crate::biquad::Biquad;
use crate::resample::resample_channel;
use af27_core::AudioBuffer;
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stats {
    pub integrated_lufs: Option<f64>,
    pub sample_peak_db: f64,
    /// Estimated by 4x oversampling.
    pub true_peak_db: f64,
    pub rms_db: f64,
    pub dc_offset: f64,
    pub duration: f64,
}

/// The two K-weighting stages for sample rate `fs`. Constants are the
/// published BS.1770 filter design values, kept at full precision.
#[allow(clippy::excessive_precision, clippy::inconsistent_digit_grouping)]
fn k_weighting(fs: f64) -> [Biquad; 2] {
    // Stage 1: high shelf, about +4 dB above 1.5 kHz.
    let (f0, g, q) = (
        1681.974_450_955_533,
        3.999_843_853_973_347,
        0.707_175_236_955_419_6,
    );
    let k = (PI * f0 / fs).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad::from_coeffs(
        (vh + vb * k / q + k * k) / a0,
        2.0 * (k * k - vh) / a0,
        (vh - vb * k / q + k * k) / a0,
        2.0 * (k * k - 1.0) / a0,
        (1.0 - k / q + k * k) / a0,
    );
    // Stage 2: high pass near 38 Hz.
    let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
    let k = (PI * f0 / fs).tan();
    let a0 = 1.0 + k / q + k * k;
    let hp = Biquad::from_coeffs(
        1.0,
        -2.0,
        1.0,
        2.0 * (k * k - 1.0) / a0,
        (1.0 - k / q + k * k) / a0,
    );
    [shelf, hp]
}

/// Integrated loudness in LUFS, or None when everything is gated out.
pub fn integrated_loudness(buf: &AudioBuffer) -> Option<f64> {
    let fs = buf.rate as f64;
    let block = (0.4 * fs) as usize;
    let hop = (0.1 * fs) as usize;
    if buf.frames() < block || block == 0 {
        return None;
    }
    // Squared, K-weighted signal per channel.
    let weighted: Vec<Vec<f64>> = buf
        .channels
        .iter()
        .map(|c| {
            let mut f = k_weighting(fs);
            c.iter()
                .map(|&s| {
                    let a = f[0].process(s as f64);
                    let y = f[1].process(a);
                    y * y
                })
                .collect()
        })
        .collect();
    let weight = |ch: usize| if ch >= 3 { 1.41 } else { 1.0 };
    let mut blocks = Vec::new();
    let mut start = 0;
    while start + block <= buf.frames() {
        let z: f64 = weighted
            .iter()
            .enumerate()
            .map(|(ch, w)| weight(ch) * w[start..start + block].iter().sum::<f64>() / block as f64)
            .sum();
        blocks.push(z);
        start += hop;
    }
    let lufs = |z: f64| -0.691 + 10.0 * z.max(1e-20).log10();
    let abs: Vec<f64> = blocks.into_iter().filter(|&z| lufs(z) > -70.0).collect();
    if abs.is_empty() {
        return None;
    }
    let rel_gate = lufs(abs.iter().sum::<f64>() / abs.len() as f64) - 10.0;
    let kept: Vec<f64> = abs.into_iter().filter(|&z| lufs(z) > rel_gate).collect();
    if kept.is_empty() {
        return None;
    }
    Some(lufs(kept.iter().sum::<f64>() / kept.len() as f64))
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-10).log10()
}

pub fn analyze(buf: &AudioBuffer) -> Stats {
    let n = (buf.frames() * buf.num_channels()).max(1) as f64;
    let mut peak = 0.0f64;
    let mut sum_sq = 0.0f64;
    let mut sum = 0.0f64;
    for c in &buf.channels {
        for &s in c {
            let s = s as f64;
            peak = peak.max(s.abs());
            sum_sq += s * s;
            sum += s;
        }
    }
    // True peak: oversample a copy of each channel by four (capped at
    // ten minutes per channel to keep the scan quick).
    let mut tp = peak;
    for c in &buf.channels {
        let take = c.len().min(buf.rate as usize * 600);
        let up = resample_channel(&c[..take], 4.0);
        tp = tp.max(up.iter().fold(0.0f64, |m, &s| m.max((s as f64).abs())));
    }
    Stats {
        integrated_lufs: integrated_loudness(buf),
        sample_peak_db: db(peak),
        true_peak_db: db(tp),
        rms_db: db((sum_sq / n).sqrt()),
        dc_offset: sum / n,
        duration: buf.duration_secs(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_full_scale_tone_reads_0_lufs() {
        // A 0 dBFS 1 kHz sine in both channels: each channel's mean
        // square is 0.5, summed 1.0, so about 0 LUFS (the -0.691 offset
        // cancels the K-filter's gain at 1 kHz).
        let fs = 48_000;
        let mut b = AudioBuffer::new(fs, 2, fs as usize * 5);
        for c in &mut b.channels {
            for (i, s) in c.iter_mut().enumerate() {
                *s = (2.0 * PI * 1000.0 * i as f64 / fs as f64).sin() as f32;
            }
        }
        let l = integrated_loudness(&b).unwrap();
        assert!(l.abs() < 0.1, "got {l}");
    }

    #[test]
    fn mono_tone_minus_20_db() {
        let fs = 44_100;
        let mut b = AudioBuffer::new(fs, 1, fs as usize * 4);
        let amp = 0.1; // -20 dBFS peak
        for (i, s) in b.channels[0].iter_mut().enumerate() {
            *s = (amp * (2.0 * PI * 1000.0 * i as f64 / fs as f64).sin()) as f32;
        }
        let l = integrated_loudness(&b).unwrap();
        // -20 dBFS peak sine = -23.01 dB mean square; K-weighting adds
        // about +0.69 at 1 kHz, offset by the -0.691 constant.
        assert!((l + 23.0).abs() < 0.2, "got {l}");
        let st = analyze(&b);
        assert!((st.sample_peak_db + 20.0).abs() < 0.05);
        assert!((st.rms_db + 23.01).abs() < 0.05);
    }

    #[test]
    fn silence_is_gated() {
        let b = AudioBuffer::new(48_000, 2, 48_000);
        assert!(integrated_loudness(&b).is_none());
    }
}
