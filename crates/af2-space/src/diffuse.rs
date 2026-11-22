// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Spread and Spatialize: decorrelated copies of a source for the room.
//!
//! A decorrelator gives four copies of one input that sound alike but do
//! not line up sample for sample: each copy is a short delay followed by
//! three all-pass stages with their own lengths. Speaker `c` takes copy
//! `c % 4`, so neighbouring speakers never share one. The lengths are
//! chosen here (primes in tenths of a millisecond, no two alike).

use crate::place::Placement;
use af2_core::space::SpatializeMode;

/// A plain delay line.
#[derive(Debug, Clone)]
pub struct Delay {
    buf: Vec<f32>,
    at: usize,
}

impl Delay {
    pub fn new(frames: usize) -> Delay {
        Delay {
            buf: vec![0.0; frames.max(1)],
            at: 0,
        }
    }

    /// Puts `x` in and returns what went in `frames` ago.
    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        let y = self.buf[self.at];
        self.buf[self.at] = x;
        self.at += 1;
        if self.at == self.buf.len() {
            self.at = 0;
        }
        y
    }

    pub fn clear(&mut self) {
        self.buf.fill(0.0);
        self.at = 0;
    }
}

/// A Schroeder all-pass stage.
#[derive(Debug, Clone)]
struct Allpass {
    line: Delay,
    g: f32,
}

impl Allpass {
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let d = self.line.buf[self.line.at];
        let v = x + self.g * d;
        self.line.run(v);
        d - self.g * v
    }
}

/// Pre-delays per copy, in tenths of a millisecond.
const PRE: [u32; 8] = [47, 89, 127, 163, 61, 103, 149, 191];
/// All-pass lengths per copy, in hundredths of a millisecond.
const STAGES: [[u32; 3]; 8] = [
    [113, 359, 761],
    [167, 421, 823],
    [223, 487, 887],
    [263, 547, 953],
    [127, 389, 787],
    [193, 449, 859],
    [241, 509, 919],
    [281, 569, 977],
];

/// Four decorrelated copies of one input.
#[derive(Debug, Clone)]
pub struct Decorrelator {
    paths: [(Delay, [Allpass; 3]); 4],
}

impl Decorrelator {
    /// `seed` picks which four of the eight copies (0 and 1 differ fully).
    pub fn new(rate: u32, seed: usize) -> Decorrelator {
        let frames = |hundredths: f64| ((hundredths / 100_000.0) * rate as f64).round() as usize;
        let path = |k: usize| {
            let i = (k + seed * 4) % 8;
            let ap = |j: usize| Allpass {
                line: Delay::new(frames(STAGES[i][j] as f64)),
                g: 0.6,
            };
            (
                Delay::new(frames(PRE[i] as f64 * 10.0)),
                [ap(0), ap(1), ap(2)],
            )
        };
        Decorrelator {
            paths: [path(0), path(1), path(2), path(3)],
        }
    }

    #[inline]
    pub fn run(&mut self, x: f32) -> [f32; 4] {
        let mut out = [0.0; 4];
        for (o, (pre, ap)) in out.iter_mut().zip(self.paths.iter_mut()) {
            let mut v = pre.run(x);
            for a in ap.iter_mut() {
                v = a.run(v);
            }
            *o = v;
        }
        out
    }

    pub fn clear(&mut self) {
        for (pre, ap) in &mut self.paths {
            pre.clear();
            for a in ap {
                a.line.clear();
            }
        }
    }
}

/// One source's Spread and Spatialize state.
#[derive(Debug, Clone)]
pub struct Diffuser {
    spread: Decorrelator,
    side: Decorrelator,
    left: Decorrelator,
    right: Decorrelator,
    /// Matrix: one plain 12 ms delay of the side signal.
    matrix: Delay,
}

impl Diffuser {
    /// `k` (the source's index) varies the copies between sources.
    pub fn new(rate: u32, k: usize) -> Diffuser {
        let a = k % 2;
        Diffuser {
            spread: Decorrelator::new(rate, a),
            side: Decorrelator::new(rate, 1 - a),
            left: Decorrelator::new(rate, a),
            right: Decorrelator::new(rate, 1 - a),
            matrix: Delay::new((rate as f64 * 0.012).round() as usize),
        }
    }

    pub fn clear(&mut self) {
        self.spread.clear();
        self.side.clear();
        self.left.clear();
        self.right.clear();
        self.matrix.clear();
    }

    /// Adds one frame's field (for `p`) to `frame`, one value per speaker.
    #[inline]
    pub fn run(&mut self, l: f32, r: f32, p: &Placement, frame: &mut [f32]) {
        let mid = 0.5 * (l + r);
        let side = 0.5 * (l - r);
        let n = frame.len().min(p.spread.len());
        if p.spread[..n].iter().any(|v| *v != 0.0) {
            let d = self.spread.run(mid);
            for (c, (o, g)) in frame[..n].iter_mut().zip(&p.spread).enumerate() {
                *o += d[c % 4] * g;
            }
        }
        if p.side[..n].iter().any(|v| *v != 0.0) {
            match p.mode {
                SpatializeMode::Matrix => {
                    let d = self.matrix.run(side);
                    for (o, g) in frame[..n].iter_mut().zip(&p.side) {
                        *o += d * g;
                    }
                }
                SpatializeMode::Ambience => {
                    let dl = self.left.run(l);
                    let dr = self.right.run(r);
                    for (c, (o, &w)) in frame[..n].iter_mut().zip(&p.side).enumerate() {
                        if w > 0.0 {
                            *o += dl[c % 4] * w;
                        } else if w < 0.0 {
                            *o -= dr[c % 4] * w;
                        }
                    }
                }
                _ => {
                    let d = self.side.run(side);
                    for (c, (o, g)) in frame[..n].iter_mut().zip(&p.side).enumerate() {
                        *o += d[c % 4] * g;
                    }
                }
            }
        }
        if let Some(c) = p.center_channel.filter(|&c| c < n) {
            frame[c] += mid * p.center;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use af2_core::space::{SpaceLayout, SpacePos};

    fn noise(n: usize) -> Vec<f32> {
        let mut x = 0x1234_5678u32;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn copies_keep_level_but_differ() {
        let input = noise(96_000);
        let mut d = Decorrelator::new(48_000, 0);
        let mut outs = vec![Vec::new(); 4];
        for &x in &input {
            for (o, v) in outs.iter_mut().zip(d.run(x)) {
                o.push(v);
            }
        }
        let skip = 4800;
        let e = |v: &[f32]| v[skip..].iter().map(|x| x * x).sum::<f32>();
        let ein = e(&input);
        for o in &outs {
            let ratio = (e(o) / ein).sqrt();
            assert!((ratio - 1.0).abs() < 0.1, "level {ratio}");
        }
        for a in 0..4 {
            for b in a + 1..4 {
                let dot: f32 = outs[a][skip..]
                    .iter()
                    .zip(&outs[b][skip..])
                    .map(|(x, y)| x * y)
                    .sum();
                let corr = dot / (e(&outs[a]) * e(&outs[b])).sqrt();
                assert!(corr.abs() < 0.1, "copies {a} and {b} correlate {corr}");
            }
        }
    }

    #[test]
    fn the_matrix_delay_is_a_plain_delay() {
        let mut d = Diffuser::new(48_000, 0);
        let p = Placement::new(
            SpaceLayout::Surround51,
            &SpacePos {
                spatialize: 1.0,
                spatialize_mode: SpatializeMode::Matrix,
                ..SpacePos::default()
            },
            true,
        );
        let delay = (48_000.0f64 * 0.012).round() as usize;
        let mut hit = None;
        for i in 0..delay + 10 {
            let mut f = [0.0f32; 6];
            let (l, r) = if i == 0 { (1.0, -1.0) } else { (0.0, 0.0) };
            d.run(l, r, &p, &mut f);
            if f[4] != 0.0 && hit.is_none() {
                hit = Some((i, f[4], f[5]));
            }
        }
        let (at, ls, rs) = hit.unwrap();
        assert_eq!(at, delay);
        // Side of a (1, -1) impulse is 1: left in phase, right inverted.
        assert!(ls > 0.0 && rs < 0.0 && (ls + rs).abs() < 1e-6);
    }

    #[test]
    fn each_spatialize_method_feeds_its_own_speakers() {
        let input = noise(24_000);
        for mode in [
            SpatializeMode::SideSpread,
            SpatializeMode::Matrix,
            SpatializeMode::Ambience,
            SpatializeMode::HeightLift,
        ] {
            let p = Placement::new(
                SpaceLayout::Surround714,
                &SpacePos {
                    spatialize: 1.0,
                    spatialize_mode: mode,
                    ..SpacePos::default()
                },
                true,
            );
            let mut d = Diffuser::new(48_000, 1);
            let mut e = [0.0f32; 12];
            for (i, &x) in input.iter().enumerate() {
                let mut f = [0.0f32; 12];
                // Different sides, so there is a side signal.
                d.run(x, input[(i * 7) % input.len()], &p, &mut f);
                for (a, v) in e.iter_mut().zip(f) {
                    *a += v * v;
                }
            }
            assert_eq!(e[3], 0.0, "{mode:?}: LFE");
            assert!(e[2] > 0.0, "{mode:?}: centre gets the middle");
            assert_eq!((e[0], e[1]), (0.0, 0.0), "{mode:?}: front pair untouched");
            if mode == SpatializeMode::HeightLift {
                assert!(e[4..8].iter().all(|v| *v == 0.0), "{mode:?}");
                assert!(e[8..12].iter().all(|v| *v > 0.0), "{mode:?}");
            } else {
                assert!(e[4..12].iter().all(|v| *v > 0.0), "{mode:?}");
            }
        }
    }
}
