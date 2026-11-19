// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) The Audiform27 contributors.

//! A Schroeder-style reverb: eight damped feedback combs in parallel
//! then four allpasses in series, per channel. Delay lengths are the
//! public-domain Freeverb tunings, scaled to the sample rate.

const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
const SPREAD: usize = 23;

struct Comb {
    buf: Vec<f32>,
    pos: usize,
    store: f32,
}

impl Comb {
    fn tick(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let y = self.buf[self.pos];
        self.store = y * (1.0 - damp) + self.store * damp;
        self.buf[self.pos] = x + self.store * feedback;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

struct Allpass {
    buf: Vec<f32>,
    pos: usize,
}

impl Allpass {
    fn tick(&mut self, x: f32) -> f32 {
        let b = self.buf[self.pos];
        let y = b - x;
        self.buf[self.pos] = x + b * 0.5;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

pub struct Settings {
    /// 0..1, longer tails as it grows.
    pub room: f32,
    /// 0..1, darker tails as it grows.
    pub damping: f32,
    /// 0..1 wet share.
    pub mix: f32,
    /// 0..1 stereo width of the tail.
    pub width: f32,
    pub pre_delay_ms: f32,
}

/// Processes planar channels in place. Adds `tail` seconds of decay
/// when `tail_frames` > 0 (the caller extends the buffers first).
pub fn process(channels: &mut [Vec<f32>], rate: u32, s: &Settings) {
    let scale = rate as f32 / 44_100.0;
    let feedback = 0.7 + 0.28 * s.room.clamp(0.0, 1.0);
    let damp = 0.4 * s.damping.clamp(0.0, 1.0);
    let n = channels.len();
    let input_gain = 0.015;
    let pre = (s.pre_delay_ms.max(0.0) * rate as f32 / 1000.0) as usize;
    // Mono sum feeds every channel's tank, so the tail is decorrelated
    // by the spread between left and right tunings.
    let frames = channels.first().map_or(0, Vec::len);
    let mono: Vec<f32> = (0..frames)
        .map(|i| channels.iter().map(|c| c[i]).sum::<f32>() / n.max(1) as f32)
        .collect();
    let mut wets: Vec<Vec<f32>> = Vec::with_capacity(n);
    for ch in 0..n {
        let off = if ch % 2 == 1 { SPREAD } else { 0 };
        let mut combs: Vec<Comb> = COMBS
            .iter()
            .map(|&d| Comb {
                buf: vec![0.0; (((d + off) as f32) * scale) as usize + 1],
                pos: 0,
                store: 0.0,
            })
            .collect();
        let mut aps: Vec<Allpass> = ALLPASSES
            .iter()
            .map(|&d| Allpass {
                buf: vec![0.0; (((d + off) as f32) * scale) as usize + 1],
                pos: 0,
            })
            .collect();
        let wet: Vec<f32> = (0..frames)
            .map(|i| {
                let x = if i >= pre {
                    mono[i - pre] * input_gain
                } else {
                    0.0
                };
                let mut y: f32 = combs.iter_mut().map(|c| c.tick(x, feedback, damp)).sum();
                for a in &mut aps {
                    y = a.tick(y);
                }
                y
            })
            .collect();
        wets.push(wet);
    }
    let w = s.width.clamp(0.0, 1.0);
    let (wet1, wet2) = (s.mix * (w / 2.0 + 0.5), s.mix * ((1.0 - w) / 2.0));
    let dry = 1.0 - s.mix;
    for (ch, c) in channels.iter_mut().enumerate() {
        let other = if n > 1 { ch ^ 1 } else { ch };
        let other = other.min(n - 1);
        for (i, x) in c.iter_mut().enumerate() {
            *x = *x * dry + wets[ch][i] * wet1 + wets[other][i] * wet2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impulse_leaves_a_tail() {
        let mut ch = vec![vec![0.0f32; 48_000]; 2];
        ch[0][0] = 1.0;
        ch[1][0] = 1.0;
        process(
            &mut ch,
            48_000,
            &Settings {
                room: 0.8,
                damping: 0.5,
                mix: 0.5,
                width: 1.0,
                pre_delay_ms: 0.0,
            },
        );
        let late: f32 = ch[0][10_000..20_000].iter().map(|s| s.abs()).sum();
        assert!(late > 0.01);
        assert!(ch[0].iter().all(|s| s.is_finite() && s.abs() < 2.0));
    }
}
