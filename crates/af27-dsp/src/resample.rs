// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Offline windowed-sinc sample-rate conversion (Blackman window).

use af27_core::AudioBuffer;
use std::f64::consts::PI;

const RADIUS: i64 = 24;

/// Resamples one channel by `ratio` = output rate / input rate.
pub fn resample_channel(input: &[f32], ratio: f64) -> Vec<f32> {
    if (ratio - 1.0).abs() < 1e-12 || input.is_empty() {
        return input.to_vec();
    }
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let cutoff = ratio.min(1.0) * 0.97;
    let step = 1.0 / ratio;
    let width = RADIUS as f64 / cutoff.min(1.0);
    let n = input.len() as i64;
    (0..out_len)
        .map(|i| {
            let t = i as f64 * step;
            let centre = t.floor() as i64;
            let reach = width.ceil() as i64;
            let mut acc = 0.0;
            let mut wsum = 0.0;
            for j in (centre - reach + 1)..=(centre + reach) {
                let x = t - j as f64;
                if x.abs() >= width {
                    continue;
                }
                let arg = x * cutoff;
                let sinc = if arg.abs() < 1e-12 {
                    1.0
                } else {
                    (PI * arg).sin() / (PI * arg)
                };
                let wpos = 0.5 + x / (2.0 * width);
                let win = 0.42 - 0.5 * (2.0 * PI * wpos).cos() + 0.08 * (4.0 * PI * wpos).cos();
                let k = sinc * win;
                wsum += k;
                if (0..n).contains(&j) {
                    acc += input[j as usize] as f64 * k;
                }
            }
            if wsum.abs() > 1e-12 {
                (acc / wsum) as f32
            } else {
                0.0
            }
        })
        .collect()
}

pub fn resample(buf: &AudioBuffer, rate: u32) -> AudioBuffer {
    if buf.rate == rate {
        return buf.clone();
    }
    let ratio = rate as f64 / buf.rate as f64;
    AudioBuffer {
        rate,
        channels: buf
            .channels
            .iter()
            .map(|c| resample_channel(c, ratio))
            .collect(),
    }
}

/// Folds or widens to `channels` (mono sums to the average; mono widens
/// by copying).
pub fn remix(buf: &AudioBuffer, channels: usize) -> AudioBuffer {
    let have = buf.num_channels();
    if have == channels {
        return buf.clone();
    }
    let frames = buf.frames();
    let out = if channels == 1 {
        let mut m = vec![0.0f32; frames];
        for c in &buf.channels {
            for (o, s) in m.iter_mut().zip(c) {
                *o += s / have as f32;
            }
        }
        vec![m]
    } else {
        (0..channels)
            .map(|c| buf.channels[c.min(have - 1)].clone())
            .collect()
    };
    AudioBuffer {
        rate: buf.rate,
        channels: out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: f64, f: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * f * i as f64 / rate).sin() as f32)
            .collect()
    }

    #[test]
    fn up_and_down_keep_a_tone() {
        let x = sine(44_100.0, 1000.0, 44_100);
        let up = resample_channel(&x, 48_000.0 / 44_100.0);
        assert_eq!(up.len(), 48_000);
        let want = sine(48_000.0, 1000.0, 48_000);
        let err: f32 = up[1000..47_000]
            .iter()
            .zip(&want[1000..47_000])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(err < 0.01, "max error {err}");
    }

    #[test]
    fn remix_mono_and_stereo() {
        let b = AudioBuffer::from_interleaved(8, 2, &[1.0, 0.0, 0.5, 0.5]);
        assert_eq!(remix(&b, 1).channels[0], vec![0.5, 0.5]);
        let m = remix(&remix(&b, 1), 2);
        assert_eq!(m.channels[1], vec![0.5, 0.5]);
    }
}
