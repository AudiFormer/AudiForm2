// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Place Elements: a stereo source opened out into the room frequency by
//! frequency. Each part of the sound goes where it sits in the stereo
//! picture: centred parts to the middle, panned parts out to the sides,
//! parts in opposite phase behind. What the two sides do not share (room,
//! reverb) goes to the front pair and, with its phase slowly turned, to
//! the speakers beside, behind and above.
//!
//! A short-time Fourier transform: frames of [`STEER_FRAME`] samples, a
//! quarter frame apart, sine windows in and out. Per bin, the two sides'
//! powers and cross power are smoothed over time and over neighbouring
//! bins; how alike the sides are (coherence) splits each bin into a direct
//! part, steered to a direction, and an ambient part, keeping its power.
//! The lows (below 120 Hz) pass to the front pair untouched; high bins
//! lean to the direct part so a voice's air stays with the voice. The
//! output is exactly `STEER_FRAME` samples late.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::pan::{point_gains_into, speakers, Gains, MAX_SPEAKERS};
use af2_core::space::SpaceLayout;

/// Frame length (and the delay it adds), samples.
pub const STEER_FRAME: usize = 2048;
const HOP: usize = STEER_FRAME / 4;
const BINS: usize = STEER_FRAME / 2 + 1;
/// Direction steps around the room.
const DIRS: usize = 72;
/// Share of the ambient part kept by the front pair when there are
/// speakers beside or behind to take the rest.
const FRONT_AMBIENCE: f32 = 0.55;
/// Smoothing of the powers per frame (about 30 ms at 48 kHz).
const SMOOTH: f32 = 0.7;

/// Place Elements for one source.
pub struct Steer {
    channels: usize,
    window: Vec<f32>,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    /// The last frame of input, oldest first (shifted by a hop each frame).
    in_l: Vec<f32>,
    in_r: Vec<f32>,
    count: usize,
    time: Vec<f32>,
    spec_l: Vec<Complex32>,
    spec_r: Vec<Complex32>,
    scratch_f: Vec<Complex32>,
    scratch_i: Vec<Complex32>,
    /// Per speaker: this frame's spectrum, and its overlap-add ring.
    out_spec: Vec<Vec<Complex32>>,
    ola: Vec<Vec<f32>>,
    at: usize,
    pll: Vec<f32>,
    prr: Vec<f32>,
    plr: Vec<Complex32>,
    /// The front pair (left, right) by speaker index.
    front: [Option<usize>; 2],
    /// Speakers beside, behind and above: (index, side 0 or 1, gain).
    round: Vec<(usize, usize, f32)>,
    has_round: [bool; 2],
    /// Unit-power gains for each direction step.
    dirs: Vec<Gains>,
    /// Per round speaker, per bin: the slowly wandering phase turn.
    turn: Vec<Vec<Complex32>>,
    low_bins: usize,
    hf_from: f32,
    hf_span: f32,
    /// The speakers that get anything.
    used: Vec<usize>,
    /// Smoothing of the powers per frame, and how many neighbouring bins
    /// are looked at (Diffusion raises both).
    smooth: f32,
    widen: f32,
}

impl Steer {
    /// Place Elements for `layout` at `rate`; `seed` varies the ambient
    /// phase turns between sources.
    pub fn new(layout: SpaceLayout, rate: u32, seed: u64) -> Steer {
        let sp = speakers(layout);
        let channels = sp.len();
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(STEER_FRAME);
        let inv = planner.plan_fft_inverse(STEER_FRAME);
        let scratch_f = fwd.make_scratch_vec();
        let scratch_i = inv.make_scratch_vec();
        let window = (0..STEER_FRAME)
            .map(|i| (std::f32::consts::PI * (i as f32 + 0.5) / STEER_FRAME as f32).sin())
            .collect();
        // The front pair: the first ear-level front speaker on each side.
        let mut front = [None; 2];
        for (i, s) in sp.iter().enumerate() {
            if let Some(a) = s.at {
                if a[2] < 0.5 && a[1] >= 0.5 && a[0].abs() > 0.1 {
                    let side = usize::from(a[0] > 0.0);
                    front[side].get_or_insert(i);
                }
            }
        }
        let mut round = Vec::new();
        for (i, s) in sp.iter().enumerate() {
            if let Some(a) = s.at {
                if a[0].abs() > 0.1 && (a[1] < 0.5 || a[2] > 0.5) {
                    round.push((i, usize::from(a[0] > 0.0), 0.0));
                }
            }
        }
        let mut has_round = [false; 2];
        for (side, has) in has_round.iter_mut().enumerate() {
            let count = round.iter().filter(|r| r.1 == side).count();
            *has = count > 0;
            let g = ((1.0 - FRONT_AMBIENCE) / count.max(1) as f32).sqrt();
            for r in round.iter_mut().filter(|r| r.1 == side) {
                r.2 = g;
            }
        }
        let dirs = (0..DIRS)
            .map(|d| {
                let th = d as f32 / DIRS as f32 * std::f32::consts::TAU;
                let mut g = [0.0; MAX_SPEAKERS];
                point_gains_into(layout, th.sin(), th.cos(), 0.0, 0.0, &mut g);
                let p = g.iter().map(|v| v * v).sum::<f32>().sqrt();
                if p > 0.0 {
                    g.iter_mut().for_each(|v| *v /= p);
                }
                g
            })
            .collect();
        let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let mut rand = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 11) as f32 / (1u64 << 53) as f32
        };
        let turn = round
            .iter()
            .map(|_| {
                // Random knots every 8 bins, eased between the short way round.
                let knots: Vec<f32> = (0..BINS / 8 + 2)
                    .map(|_| rand() * std::f32::consts::TAU)
                    .collect();
                (0..BINS)
                    .map(|k| {
                        if k < 8 {
                            return Complex32::new(1.0, 0.0);
                        }
                        let (j, f) = (k / 8, (k % 8) as f32 / 8.0);
                        let (a, b) = (knots[j], knots[j + 1]);
                        let mut d = b - a;
                        if d > std::f32::consts::PI {
                            d -= std::f32::consts::TAU;
                        } else if d < -std::f32::consts::PI {
                            d += std::f32::consts::TAU;
                        }
                        let ease = f * f * (3.0 - 2.0 * f);
                        let ph = a + d * ease;
                        Complex32::new(ph.cos(), ph.sin())
                    })
                    .collect()
            })
            .collect();
        let hz = |f: f32| f * STEER_FRAME as f32 / rate.max(1) as f32;
        let mut used: Vec<usize> = (0..channels).filter(|&i| sp[i].at.is_some()).collect();
        used.sort_unstable();
        Steer {
            channels,
            window,
            fwd,
            inv,
            in_l: vec![0.0; STEER_FRAME],
            in_r: vec![0.0; STEER_FRAME],
            count: 0,
            time: vec![0.0; STEER_FRAME],
            spec_l: vec![Complex32::default(); BINS],
            spec_r: vec![Complex32::default(); BINS],
            scratch_f,
            scratch_i,
            out_spec: vec![vec![Complex32::default(); BINS]; channels],
            ola: vec![vec![0.0; STEER_FRAME]; channels],
            at: 0,
            pll: vec![0.0; BINS],
            prr: vec![0.0; BINS],
            plr: vec![Complex32::default(); BINS],
            front,
            round,
            has_round,
            dirs,
            turn,
            low_bins: hz(120.0).ceil() as usize,
            hf_from: hz(4000.0),
            hf_span: hz(6000.0),
            used,
            smooth: SMOOTH,
            widen: 1.0,
        }
    }

    /// Diffusion, 0 to 1: 0 places each part of the sound precisely (as a
    /// session does); towards 1 the directions are averaged over longer
    /// times and wider bands, so placement is smoother and less exact.
    pub fn set_diffusion(&mut self, d: f32) {
        let d = d.clamp(0.0, 1.0);
        self.smooth = SMOOTH + (0.95 - SMOOTH) * d;
        self.widen = 1.0 + 5.0 * d;
    }

    pub fn reset(&mut self) {
        self.in_l.fill(0.0);
        self.in_r.fill(0.0);
        self.count = 0;
        for o in &mut self.ola {
            o.fill(0.0);
        }
        self.pll.fill(0.0);
        self.prr.fill(0.0);
        self.plr.fill(Complex32::default());
    }

    /// Takes one input frame and gives the output, one value per speaker,
    /// `STEER_FRAME` samples late.
    #[inline]
    pub fn run(&mut self, l: f32, r: f32, out: &mut [f32]) {
        let last = STEER_FRAME - HOP + self.count;
        self.in_l[last] = l;
        self.in_r[last] = r;
        self.count += 1;
        for (c, o) in out.iter_mut().enumerate().take(self.channels) {
            *o = std::mem::take(&mut self.ola[c][self.at]);
        }
        if self.count == HOP {
            // Input sample `i` of the frame comes out at slot `at + 1 + i`:
            // exactly a frame after it went in.
            self.frame();
            self.in_l.copy_within(HOP.., 0);
            self.in_r.copy_within(HOP.., 0);
            self.count = 0;
        }
        self.at = (self.at + 1) % STEER_FRAME;
    }

    fn frame(&mut self) {
        for (input, spec) in [
            (&self.in_l, &mut self.spec_l),
            (&self.in_r, &mut self.spec_r),
        ] {
            for ((t, x), w) in self.time.iter_mut().zip(input.iter()).zip(&self.window) {
                *t = x * w;
            }
            let _ = self
                .fwd
                .process_with_scratch(&mut self.time, spec, &mut self.scratch_f);
        }
        for o in &mut self.out_spec {
            o.fill(Complex32::default());
        }
        // Smoothed powers over time.
        for k in 0..BINS {
            let (a, b) = (self.spec_l[k], self.spec_r[k]);
            let sm = self.smooth;
            self.pll[k] = sm * self.pll[k] + (1.0 - sm) * a.norm_sqr();
            self.prr[k] = sm * self.prr[k] + (1.0 - sm) * b.norm_sqr();
            self.plr[k] = self.plr[k] * sm + a * b.conj() * (1.0 - sm);
        }
        let [fl, fr] = self.front;
        for k in 0..BINS {
            let (sl, sr) = (self.spec_l[k], self.spec_r[k]);
            if k < self.low_bins {
                if let Some(i) = fl {
                    self.out_spec[i][k] += sl;
                }
                if let Some(i) = fr {
                    self.out_spec[i][k] += sr;
                }
                continue;
            }
            // ...and over neighbouring bins (wider up high).
            let w = ((k / 96).clamp(1, 3) as f32 * self.widen) as usize;
            let (lo, hi) = (k.saturating_sub(w), (k + w).min(BINS - 1));
            let el: f32 = self.pll[lo..=hi].iter().sum();
            let er: f32 = self.prr[lo..=hi].iter().sum();
            let slr: Complex32 = self.plr[lo..=hi].iter().sum();
            let cross = slr.norm();
            let alike = if el * er > 0.0 {
                (cross / (el * er).sqrt()).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let pan = if el + er > 0.0 {
                ((er - el) / (el + er)).clamp(-1.0, 1.0)
            } else {
                0.0
            };
            let behind = if cross > 0.0 {
                (-slr.re / cross).clamp(0.0, 1.0) * alike
            } else {
                0.0
            };
            let hf = ((k as f32 - self.hf_from) / self.hf_span).clamp(0.0, 1.0);
            let m = alike.powf(1.0 - 0.5 * hf);
            let amb = (1.0 - m * m).max(0.0).sqrt();
            let (dl, dr) = (sl * m, sr * m);
            let unit = if cross > 0.0 {
                slr / cross
            } else {
                Complex32::new(1.0, 0.0)
            };
            // Both sides' direct parts in the left's phase, with their power.
            let sum = dl + dr * unit;
            let want = (dl.norm_sqr() + dr.norm_sqr()).sqrt();
            let n = sum.norm();
            let direct = if n > 1e-20 {
                sum * (want / n)
            } else {
                Complex32::default()
            };
            let front_deg = pan.signum() * pan.abs().powf(0.8) * 80.0;
            let side = if pan.abs() < 0.05 { 1.0 } else { pan.signum() };
            let deg = (front_deg + side * behind * (180.0 - front_deg.abs())).rem_euclid(360.0);
            let pos = deg / 360.0 * DIRS as f32;
            let (d0, frac) = (pos.floor() as usize % DIRS, pos.fract());
            let d1 = (d0 + 1) % DIRS;
            for &c in &self.used {
                let g = self.dirs[d0][c] * (1.0 - frac) + self.dirs[d1][c] * frac;
                if g != 0.0 {
                    self.out_spec[c][k] += direct * g;
                }
            }
            if amb > 0.0 {
                for (s, spec, f) in [(0, sl, fl), (1, sr, fr)] {
                    if let Some(i) = f {
                        let keep = if self.has_round[s] {
                            FRONT_AMBIENCE.sqrt()
                        } else {
                            1.0
                        };
                        self.out_spec[i][k] += spec * (amb * keep);
                    }
                }
                for (j, &(c, s, g)) in self.round.iter().enumerate() {
                    let spec = if s == 0 { sl } else { sr };
                    self.out_spec[c][k] += spec * self.turn[j][k] * (amb * g);
                }
            }
        }
        let scale = 0.5 / STEER_FRAME as f32;
        for &c in &self.used {
            let spec = &mut self.out_spec[c];
            spec[0].im = 0.0;
            spec[BINS - 1].im = 0.0;
            let _ = self
                .inv
                .process_with_scratch(spec, &mut self.time, &mut self.scratch_i);
            for (i, (t, w)) in self.time.iter().zip(&self.window).enumerate() {
                let slot = (self.at + 1 + i) % STEER_FRAME;
                self.ola[c][slot] += t * w * scale;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Energy per speaker after running `l`/`r` through Place Elements.
    fn energy(layout: SpaceLayout, f: impl Fn(usize) -> (f32, f32)) -> Vec<f32> {
        let mut s = Steer::new(layout, 48_000, 7);
        let n = speakers(layout).len();
        let mut e = vec![0.0f32; n];
        let mut out = vec![0.0f32; n];
        for i in 0..48_000 {
            let (l, r) = f(i);
            s.run(l, r, &mut out);
            if i > 8_000 {
                for (a, v) in e.iter_mut().zip(&out) {
                    *a += v * v;
                }
            }
        }
        e
    }

    fn tone(i: usize) -> f32 {
        (std::f32::consts::TAU * 1000.0 * i as f32 / 48_000.0).sin() * 0.5
    }

    #[test]
    fn place_elements_puts_each_part_where_it_sits() {
        let l = SpaceLayout::Surround71;
        // Centred: loudest on the centre speaker.
        let e = energy(l, |i| (tone(i), tone(i)));
        let c = e[2];
        assert!(e.iter().enumerate().all(|(k, v)| k == 2 || *v < c), "{e:?}");
        // Hard left: the left of the room, far more than the right.
        let e = energy(l, |i| (tone(i), 0.0));
        let left = e[0] + e[4] + e[6];
        let right = e[1] + e[5] + e[7];
        assert!(left > 50.0 * right.max(1e-12), "{e:?}");
        // Alike but out of phase: behind.
        let e = energy(l, |i| (tone(i), -tone(i)));
        let back = e[4] + e[5];
        assert!(back > 4.0 * (e[0] + e[1] + e[2]), "{e:?}");
    }

    #[test]
    fn it_keeps_level_and_is_exactly_one_frame_late() {
        let l = SpaceLayout::Surround51;
        let mut s = Steer::new(l, 48_000, 1);
        let mut out = vec![0.0f32; 6];
        let mut best = (0usize, 0.0f32);
        for i in 0..3 * STEER_FRAME {
            let x = if i == 100 { 1.0 } else { 0.0 };
            s.run(x, x, &mut out);
            if out[2].abs() > best.1 {
                best = (i, out[2].abs());
            }
        }
        // A centred impulse peaks on the centre speaker a frame late.
        assert_eq!(best.0, 100 + STEER_FRAME, "{best:?}");
        // A centred tone keeps its power (within 1.5 dB).
        let e = energy(l, |i| (tone(i), tone(i)));
        let total: f32 = e.iter().sum();
        let want = 2.0 * 0.125 * (48_000 - 8_001) as f32;
        let db = 10.0 * (total / want).log10();
        assert!(db.abs() < 1.5, "{db:+.2} dB");
    }
}
