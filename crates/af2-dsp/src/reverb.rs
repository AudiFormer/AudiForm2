// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! A Schroeder-style reverb: eight damped feedback combs in parallel
//! then four allpasses in series, per side. Delay lengths are the
//! public-domain Freeverb tunings, scaled to the sample rate; the right
//! side's lines are a little longer so the tail is wide.

use crate::live::Live;

const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
const SPREAD: usize = 23;

struct Comb {
    buf: Vec<f32>,
    pos: usize,
    store: f32,
}

impl Comb {
    #[inline]
    fn tick(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let y = self.buf[self.pos];
        self.store = y * (1.0 - damp) + self.store * damp;
        self.buf[self.pos] = x + self.store * feedback;
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        y
    }
}

struct Allpass {
    buf: Vec<f32>,
    pos: usize,
}

impl Allpass {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let b = self.buf[self.pos];
        let y = b - x;
        self.buf[self.pos] = x + b * 0.5;
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        y
    }
}

struct Tank {
    combs: Vec<Comb>,
    aps: Vec<Allpass>,
}

impl Tank {
    fn new(rate: u32, offset: usize) -> Tank {
        let scale = rate as f32 / 44_100.0;
        let len = |d: usize| (((d + offset) as f32) * scale) as usize + 1;
        Tank {
            combs: COMBS
                .iter()
                .map(|&d| Comb {
                    buf: vec![0.0; len(d)],
                    pos: 0,
                    store: 0.0,
                })
                .collect(),
            aps: ALLPASSES
                .iter()
                .map(|&d| Allpass {
                    buf: vec![0.0; len(d)],
                    pos: 0,
                })
                .collect(),
        }
    }

    #[inline]
    fn tick(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let mut y: f32 = self
            .combs
            .iter_mut()
            .map(|c| c.tick(x, feedback, damp))
            .sum();
        for a in &mut self.aps {
            y = a.tick(y);
        }
        y
    }

    fn clear(&mut self) {
        for c in &mut self.combs {
            c.buf.fill(0.0);
            c.store = 0.0;
        }
        for a in &mut self.aps {
            a.buf.fill(0.0);
        }
    }
}

pub struct Reverb {
    rate: u32,
    tanks: [Tank; 2],
    pre: Vec<f32>,
    pre_pos: usize,
    pre_len: usize,
    feedback: f32,
    damp: f32,
    wet1: f32,
    wet2: f32,
    dry: f32,
}

impl Reverb {
    pub fn new(rate: u32) -> Reverb {
        Reverb {
            rate,
            tanks: [Tank::new(rate, 0), Tank::new(rate, SPREAD)],
            pre: vec![0.0; rate as usize / 4 + 1],
            pre_pos: 0,
            pre_len: 0,
            feedback: 0.85,
            damp: 0.2,
            wet1: 0.25,
            wet2: 0.0,
            dry: 0.75,
        }
    }
}

impl Live for Reverb {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        const INPUT: f32 = 0.015;
        let n = self.pre.len();
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            self.pre[self.pre_pos] = (*a + *b) * 0.5 * INPUT;
            let x = self.pre[(self.pre_pos + n - self.pre_len) % n];
            self.pre_pos = (self.pre_pos + 1) % n;
            let wl = self.tanks[0].tick(x, self.feedback, self.damp);
            let wr = self.tanks[1].tick(x, self.feedback, self.damp);
            *a = *a * self.dry + wl * self.wet1 + wr * self.wet2;
            *b = *b * self.dry + wr * self.wet1 + wl * self.wet2;
        }
    }

    /// Room size, damping, mix, width, pre-delay (ms).
    fn set(&mut self, vals: &[f32]) {
        let g = |i: usize, d: f32| vals.get(i).copied().unwrap_or(d);
        self.feedback = 0.7 + 0.28 * g(0, 0.6).clamp(0.0, 1.0);
        self.damp = 0.4 * g(1, 0.5).clamp(0.0, 1.0);
        let mix = g(2, 0.25).clamp(0.0, 1.0);
        let w = g(3, 1.0).clamp(0.0, 1.0);
        self.wet1 = mix * (w / 2.0 + 0.5);
        self.wet2 = mix * ((1.0 - w) / 2.0);
        self.dry = 1.0 - mix;
        self.pre_len =
            ((g(4, 10.0).max(0.0) * self.rate as f32 / 1000.0) as usize).min(self.pre.len() - 1);
    }

    /// Copy `k` on a speaker bed gets lines a little longer than the
    /// copy before it, so every speaker pair hears its own tail.
    fn vary(&mut self, k: usize) {
        if k == 0 {
            return;
        }
        let off = k * (2 * SPREAD + 13);
        self.tanks = [
            Tank::new(self.rate, off),
            Tank::new(self.rate, off + SPREAD),
        ];
    }

    fn reset(&mut self) {
        self.tanks[0].clear();
        self.tanks[1].clear();
        self.pre.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impulse_leaves_a_tail() {
        let mut rv = Reverb::new(48_000);
        rv.set(&[0.8, 0.5, 0.5, 1.0, 0.0]);
        let mut l = vec![0.0f32; 48_000];
        let mut r = vec![0.0f32; 48_000];
        l[0] = 1.0;
        r[0] = 1.0;
        rv.process(&mut l, &mut r);
        let late: f32 = l[10_000..20_000].iter().map(|s| s.abs()).sum();
        assert!(late > 0.01);
        assert!(l.iter().all(|s| s.is_finite() && s.abs() < 2.0));
    }
}
