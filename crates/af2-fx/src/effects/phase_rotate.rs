// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Phase Rotate: turns the phase of every frequency by the same angle,
//! on both sides or one, with a polarity flip and a short delay per
//! side for lining up two mics. Written for AudiForm 2 to behave as
//! Audiform's Phase Rotate does (same controls and keys, so its sessions
//! carry over); the code is our own. Retired in Audiform (Phase Curve
//! does the same and more), so it is kept for sessions but not offered.
//!
//! The turn is made on the spectrum: frames of about 40 ms under a
//! square-root Hann window on both sides, three-quarters overlapped; each
//! bin is multiplied by e^(i angle), while the DC and top bins (which
//! hold no phase) are scaled by cos(angle). The frame length is reported
//! as latency. After it, a delay of up to 20 ms (gliding to a new time)
//! and a polarity flip per side that fades over 5 ms.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use super::{choice, num, toggle, EffectInfo, Params, Processor};

pub const ID: &str = "af.phase_rotate";
const MAX_DELAY_MS: f64 = 20.0;

pub fn phase_rotate_info() -> EffectInfo {
    EffectInfo {
        id: ID,
        sidechain: false,
        name: "Phase Rotate",
        category: "Stereo",
        description: "Turns the phase of every frequency by the same angle, on one side or both, with a polarity flip and a short delay per side.",
        params: vec![
            num("angle", "Angle", "°", -180.0, 180.0, 0.0, 1.0),
            choice("channels", "Rotate", &["Both channels", "Left only", "Right only"], 0),
            toggle("invert_left", "Flip left polarity", false).group("Polarity"),
            toggle("invert_right", "Flip right polarity", false).group("Polarity"),
            num("delay", "Delay", "ms", 0.0, MAX_DELAY_MS, 0.0, 0.01).group("Timing"),
        ],
    }
}

fn frame_len(rate: u32) -> usize {
    let target = (f64::from(rate) * 0.04).max(1.0) as usize;
    target.next_power_of_two().clamp(256, 8192).max(512)
}

pub struct PhaseRotate {
    rate: f64,
    n: usize,
    hop: usize,
    window: Vec<f32>,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    input: [Vec<f32>; 2],
    /// Overlapped output, a ring of `2n`.
    acc: [Vec<f32>; 2],
    t: usize,
    since: usize,
    time: Vec<f32>,
    spec: Vec<Complex32>,
    scratch_f: Vec<Complex32>,
    scratch_i: Vec<Complex32>,
    turn: Complex32,
    sides: [bool; 2],
    /// The delay line after the turn, its read distance and target (frames).
    delay: [Vec<f32>; 2],
    delay_at: usize,
    dist: f64,
    dist_target: f64,
    glide: f64,
    pol: [f32; 2],
    pol_target: [f32; 2],
    pol_step: f32,
}

impl PhaseRotate {
    pub fn new(p: &Params, rate: u32) -> Self {
        let n = frame_len(rate);
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let r = f64::from(rate.max(1));
        let window = (0..n)
            .map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos()).sqrt() as f32)
            .collect();
        let dlen = (MAX_DELAY_MS / 1000.0 * r) as usize + 4;
        let mut v = Self {
            rate: r,
            n,
            hop: n / 4,
            window,
            scratch_f: fwd.make_scratch_vec(),
            scratch_i: inv.make_scratch_vec(),
            spec: fwd.make_output_vec(),
            fwd,
            inv,
            input: [vec![0.0; n], vec![0.0; n]],
            acc: [vec![0.0; 2 * n], vec![0.0; 2 * n]],
            t: 0,
            since: 0,
            time: vec![0.0; n],
            turn: Complex32::new(1.0, 0.0),
            sides: [true, true],
            delay: [vec![0.0; dlen], vec![0.0; dlen]],
            delay_at: 0,
            dist: 0.0,
            dist_target: 0.0,
            glide: 1.0 - (-1.0 / (0.03 * r)).exp(),
            pol: [1.0; 2],
            pol_target: [1.0; 2],
            pol_step: (1.0 / (0.005 * r)) as f32,
        };
        v.set(p);
        v.dist = v.dist_target;
        v.pol = v.pol_target;
        v
    }

    fn set(&mut self, p: &Params) {
        let a = p.get("angle").to_radians() as f32;
        self.turn = Complex32::from_polar(1.0, a);
        self.sides = match p.index("channels") {
            1 => [true, false],
            2 => [false, true],
            _ => [true, true],
        };
        self.pol_target = [
            if p.on("invert_left") { -1.0 } else { 1.0 },
            if p.on("invert_right") { -1.0 } else { 1.0 },
        ];
        self.dist_target = p.get("delay").clamp(0.0, MAX_DELAY_MS) / 1000.0 * self.rate;
    }

    fn frame(&mut self) {
        let n = self.n;
        let len = self.acc[0].len();
        // The frame's samples go back in where they came from, `n` later.
        let start = self.t + n - n;
        let scale = 1.0 / (2.0 * n as f32);
        for ch in 0..2 {
            for i in 0..n {
                self.time[i] = self.input[ch][i] * self.window[i];
            }
            let _ = self.fwd.process_with_scratch(&mut self.time, &mut self.spec, &mut self.scratch_f);
            if self.sides[ch] {
                let last = self.spec.len() - 1;
                let c = self.turn.re;
                for (k, b) in self.spec.iter_mut().enumerate() {
                    if k == 0 || k == last {
                        *b = Complex32::new(b.re * c, 0.0);
                    } else {
                        *b *= self.turn;
                    }
                }
            }
            let _ = self.inv.process_with_scratch(&mut self.spec, &mut self.time, &mut self.scratch_i);
            for i in 0..n {
                let at = (start + i) % len;
                self.acc[ch][at] += self.time[i] * self.window[i] * scale;
            }
        }
    }
}

impl Processor for PhaseRotate {
    fn process(&mut self, buf: &mut [f32]) {
        let n = self.n;
        let len = self.acc[0].len();
        let dlen = self.delay[0].len();
        for fr in buf.as_chunks_mut::<2>().0 {
            for ch in 0..2 {
                self.input[ch].copy_within(1.., 0);
                self.input[ch][n - 1] = fr[ch];
            }
            self.t += 1;
            self.since += 1;
            if self.since >= self.hop {
                self.since = 0;
                self.frame();
            }
            // The sample that has had all its frames: `n` back.
            let at = (self.t + len - 1) % len;
            let y = [self.acc[0][at], self.acc[1][at]];
            self.acc[0][at] = 0.0;
            self.acc[1][at] = 0.0;
            // The delay, then the polarity.
            self.dist += (self.dist_target - self.dist) * self.glide;
            for ch in 0..2 {
                self.delay[ch][self.delay_at] = y[ch];
                let pos = self.delay_at as f64 + dlen as f64 - self.dist;
                let i0 = pos.floor();
                let frac = (pos - i0) as f32;
                let i0 = i0 as usize % dlen;
                let i1 = (i0 + 1) % dlen;
                // Reading back from i0 (older) towards i1 (newer).
                let v = self.delay[ch][i0] * (1.0 - frac) + self.delay[ch][i1] * frac;
                let d = self.pol_target[ch] - self.pol[ch];
                self.pol[ch] += d.clamp(-self.pol_step * 2.0, self.pol_step * 2.0);
                fr[ch] = v * self.pol[ch];
            }
            self.delay_at = (self.delay_at + 1) % dlen;
        }
    }

    fn latency(&self) -> usize {
        self.n
    }

    fn reset(&mut self) {
        for ch in 0..2 {
            self.input[ch].fill(0.0);
            self.acc[ch].fill(0.0);
            self.delay[ch].fill(0.0);
        }
        self.since = 0;
        self.dist = self.dist_target;
        self.pol = self.pol_target;
    }

    fn update(&mut self, p: &Params) -> bool {
        self.set(p);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const R: u32 = 48_000;

    fn params(over: &[(&str, f64)]) -> Params {
        let saved: BTreeMap<String, f64> = over.iter().map(|(k, v)| ((*k).to_string(), *v)).collect();
        Params::resolve(&phase_rotate_info(), &saved)
    }

    fn run(over: &[(&str, f64)], input: &[f32]) -> (Vec<f32>, usize) {
        let mut p = PhaseRotate::new(&params(over), R);
        let mut b = input.to_vec();
        for c in b.chunks_mut(256) {
            p.process(c);
        }
        (b, p.latency())
    }

    fn tone(hz: f64) -> Vec<f32> {
        (0..R as usize)
            .flat_map(|i| {
                let x = (std::f64::consts::TAU * hz * i as f64 / f64::from(R)).sin() as f32 * 0.5;
                [x, x]
            })
            .collect()
    }

    #[test]
    fn no_turn_gives_back_the_sound_a_frame_late() {
        let input = tone(300.0);
        let (out, lat) = run(&[], &input);
        for i in (lat + 4800)..(R as usize - 10) {
            assert!((out[2 * i] - input[2 * (i - lat)]).abs() < 1e-3, "frame {i}");
        }
    }

    #[test]
    fn half_a_turn_on_the_left_only_flips_the_left() {
        let input = tone(500.0);
        let (out, lat) = run(&[("angle", 180.0), ("channels", 1.0)], &input);
        for i in (lat + 4800)..(R as usize - 10) {
            assert!((out[2 * i] + input[2 * (i - lat)]).abs() < 2e-3, "left {i}");
            assert!((out[2 * i + 1] - input[2 * (i - lat) + 1]).abs() < 2e-3, "right {i}");
        }
    }

    #[test]
    fn a_quarter_turn_keeps_the_level_and_moves_the_wave() {
        let input = tone(1000.0);
        let (out, lat) = run(&[("angle", 90.0)], &input);
        let s = lat + 4800;
        let e = R as usize - 10;
        let rms = |f: &dyn Fn(usize) -> f32| ((s..e).map(|i| f(i).powi(2)).sum::<f32>() / (e - s) as f32).sqrt();
        let lin = rms(&|i| input[2 * (i - lat)]);
        let lout = rms(&|i| out[2 * i]);
        assert!((lout / lin - 1.0).abs() < 0.01, "{lout} {lin}");
        // A quarter turn: no correlation with the original.
        let corr: f32 = (s..e).map(|i| out[2 * i] * input[2 * (i - lat)]).sum::<f32>() / (e - s) as f32;
        assert!(corr.abs() < 0.01 * lin * lin + 1e-4, "{corr}");
    }

    #[test]
    fn delay_and_polarity_follow_their_controls() {
        let mut input = vec![0.0f32; 2 * 9600];
        input[2 * 100] = 1.0;
        input[2 * 100 + 1] = 1.0;
        let (out, lat) = run(&[("delay", 10.0), ("invert_right", 1.0)], &input);
        let peak = |ch: usize| (0..9600).max_by(|a, b| out[2 * a + ch].abs().total_cmp(&out[2 * b + ch].abs())).unwrap();
        assert_eq!(peak(0), 100 + lat + 480);
        assert!(out[2 * peak(1) + 1] < 0.0, "the right is flipped");
    }
}
