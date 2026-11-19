// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Generated audio: tones, noise and silence.

use af27_core::AudioBuffer;
use std::f64::consts::TAU;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wave {
    Sine,
    Square,
    Triangle,
    Saw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Noise {
    White,
    Pink,
    Brown,
}

pub fn tone(
    rate: u32,
    channels: usize,
    secs: f64,
    freq: f64,
    amp_db: f32,
    wave: Wave,
) -> AudioBuffer {
    let frames = (secs * rate as f64) as usize;
    let amp = af27_core::time::db_to_gain(amp_db);
    let data: Vec<f32> = (0..frames)
        .map(|i| {
            let ph = (freq * i as f64 / rate as f64).fract();
            let v = match wave {
                Wave::Sine => (TAU * ph).sin(),
                Wave::Square => {
                    if ph < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                Wave::Triangle => 1.0 - 4.0 * (ph - 0.5).abs(),
                Wave::Saw => 2.0 * ph - 1.0,
            };
            v as f32 * amp
        })
        .collect();
    AudioBuffer {
        rate,
        channels: vec![data; channels.max(1)],
    }
}

pub fn noise(rate: u32, channels: usize, secs: f64, amp_db: f32, kind: Noise) -> AudioBuffer {
    let frames = (secs * rate as f64) as usize;
    let amp = af27_core::time::db_to_gain(amp_db);
    let mut seed = 0x9e37_79b9_u32;
    let chans = (0..channels.max(1))
        .map(|c| {
            seed = seed.wrapping_add(0x6d2b_79f5 * (c as u32 + 1));
            let mut rng = move || {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
            };
            // Pink: Paul Kellet's economy filter on white noise.
            let (mut b0, mut b1, mut b2) = (0.0f32, 0.0f32, 0.0f32);
            let mut brown = 0.0f32;
            (0..frames)
                .map(|_| {
                    let w = rng();
                    let v = match kind {
                        Noise::White => w,
                        Noise::Pink => {
                            b0 = 0.99765 * b0 + w * 0.099_046;
                            b1 = 0.96300 * b1 + w * 0.296_516_4;
                            b2 = 0.57000 * b2 + w * 1.052_691_3;
                            (b0 + b1 + b2 + w * 0.1848) * 0.25
                        }
                        Noise::Brown => {
                            brown = (brown + w * 0.02).clamp(-1.0, 1.0) * 0.998;
                            brown * 3.0
                        }
                    };
                    v.clamp(-1.0, 1.0) * amp
                })
                .collect()
        })
        .collect();
    AudioBuffer {
        rate,
        channels: chans,
    }
}

pub fn silence(rate: u32, channels: usize, secs: f64) -> AudioBuffer {
    AudioBuffer::new(rate, channels, (secs * rate as f64) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_levels() {
        let t = tone(48_000, 2, 1.0, 440.0, -6.0, Wave::Sine);
        assert_eq!(t.frames(), 48_000);
        assert!((t.peak() - 0.501).abs() < 0.01);
        let n = noise(48_000, 1, 1.0, -12.0, Noise::Pink);
        assert!(n.peak() <= 0.252);
    }
}
