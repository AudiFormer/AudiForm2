// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.
// Written fresh for AudiForm 2 from a behaviour description of Audiform's
// Curve EQ (whose source carries the GeekCast Radio Network / Altered
// Audio Editor copyright line). The author did not read that source: the
// controls (keys, ranges, defaults, pick names) come from the description
// as a file format, so Audiform's `altaudio.*` slots load, and the
// processing from public filter design. See docs/clean-room.md.

//! Curve EQ: an equaliser drawn as a smooth curve through eight points
//! (frequency, gain). Between two points the curve eases from one level
//! to the next along a smoothstep on a log frequency scale, so it has no
//! corners; beyond the first and last points it holds their level.
//!
//! The curve is applied as a zero-phase gain in the spectrum: frames of
//! about 80 ms, square-root Hann windows on the way in and out, 75%
//! overlap, and each frequency bin scaled by the curve's gain there. Only
//! the level of each frequency changes, never its timing, at the cost of
//! one frame of delay, which is reported as latency.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use super::{num, EffectInfo, ParamInfo, Params, Processor};

pub const ID: &str = "af.curve_eq";

/// How many points the curve has.
pub const POINTS: usize = 8;

pub const FREQ_KEYS: [&str; POINTS] = [
    "p1_freq", "p2_freq", "p3_freq", "p4_freq", "p5_freq", "p6_freq", "p7_freq", "p8_freq",
];
pub const GAIN_KEYS: [&str; POINTS] = [
    "p1_gain", "p2_gain", "p3_gain", "p4_gain", "p5_gain", "p6_gain", "p7_gain", "p8_gain",
];
pub const GROUPS: [&str; POINTS] = [
    "Point 1", "Point 2", "Point 3", "Point 4", "Point 5", "Point 6", "Point 7", "Point 8",
];
/// Where the points start, in Hz.
pub const START: [f64; POINTS] = [
    40.0, 100.0, 250.0, 600.0, 1500.0, 3500.0, 8000.0, 16_000.0,
];

pub fn info() -> EffectInfo {
    let mut params: Vec<ParamInfo> = Vec::new();
    for i in 0..POINTS {
        params.push(
            num(FREQ_KEYS[i], "Frequency", "Hz", 20.0, 20_000.0, START[i], 1.0)
                .log()
                .group(GROUPS[i]),
        );
        params.push(num(GAIN_KEYS[i], "Gain", "dB", -40.0, 20.0, 0.0, 0.1).group(GROUPS[i]));
    }
    EffectInfo {
        id: ID,
        name: "Curve EQ",
        category: "EQ",
        description: "Shapes the tone with a smooth curve through eight points, changing only the level of each frequency, never its timing.",
        params,
        sidechain: false,
    }
}

/// The curve's points, lowest frequency first, as (Hz, dB).
/// Kept inline (no allocation), as the gains are worked out again on the
/// audio thread when a control moves.
pub fn points(p: &Params) -> crate::biquad::Fixed<(f64, f64), POINTS> {
    let mut pts: crate::biquad::Fixed<(f64, f64), POINTS> = (0..POINTS)
        .map(|i| (p.get(FREQ_KEYS[i]).max(1.0), p.get(GAIN_KEYS[i])))
        .collect();
    pts.sort_stable_by(|a, b| a.0 < b.0);
    pts
}

/// The curve's level in dB at `hz`. `points` must be sorted by frequency
/// (as [`points`] gives them).
pub fn curve_db(points: &[(f64, f64)], hz: f64) -> f64 {
    let Some(&first) = points.first() else {
        return 0.0;
    };
    let last = points[points.len() - 1];
    if hz.is_nan() || hz <= first.0 {
        return first.1;
    }
    if hz >= last.0 {
        return last.1;
    }
    let i = points
        .iter()
        .rposition(|q| q.0 <= hz)
        .unwrap_or(0)
        .min(points.len() - 2);
    let (a, b) = (points[i], points[i + 1]);
    let (xa, xb) = (a.0.log2(), b.0.log2());
    let span = xb - xa;
    if span <= 1e-12 {
        return b.1;
    }
    let t = ((hz.log2() - xa) / span).clamp(0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t);
    a.1 + (b.1 - a.1) * s
}

/// The frame length at `rate`: the power of two at or above 80 ms,
/// between 256 and 8192.
fn frame_len(rate: u32) -> usize {
    let want = (0.08 * f64::from(rate.max(1))).ceil() as usize;
    want.next_power_of_two().clamp(256, 8192)
}

pub struct CurveEq {
    n: usize,
    hop: usize,
    rate: f64,
    win: Vec<f32>,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    input: [Vec<f32>; 2],
    acc: [Vec<f32>; 2],
    out: [Vec<f32>; 2],
    pos: usize,
    frame: Vec<f32>,
    spec: Vec<Complex32>,
    scratch: Vec<Complex32>,
    /// Per bin: the linear gain, with the overlap-add scale folded in.
    gains: Vec<f32>,
}

impl CurveEq {
    pub fn new(p: &Params, rate: u32) -> Self {
        let n = frame_len(rate);
        let hop = n / 4;
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let scratch_len = fwd.get_scratch_len().max(inv.get_scratch_len());
        let win = (0..n)
            .map(|i| {
                let c = (std::f64::consts::TAU * i as f64 / n as f64).cos();
                (0.5 - 0.5 * c).sqrt() as f32
            })
            .collect();
        let mut s = CurveEq {
            n,
            hop,
            rate: f64::from(rate.max(1)),
            win,
            fwd,
            inv,
            input: [vec![0.0; n], vec![0.0; n]],
            acc: [vec![0.0; n], vec![0.0; n]],
            out: [vec![0.0; hop], vec![0.0; hop]],
            pos: 0,
            frame: vec![0.0; n],
            spec: vec![Complex32::default(); n / 2 + 1],
            scratch: vec![Complex32::default(); scratch_len],
            gains: vec![0.0; n / 2 + 1],
        };
        s.set_gains(p);
        s
    }

    fn set_gains(&mut self, p: &Params) {
        let pts = points(p);
        let bin_hz = self.rate / self.n as f64;
        // Hann (the two windows together) sums to 2 at 75% overlap, and the
        // inverse transform is unnormalised: scale by 1 / (2n).
        let scale = 1.0 / (2.0 * self.n as f64);
        for (k, g) in self.gains.iter_mut().enumerate() {
            let db = curve_db(&pts, k as f64 * bin_hz);
            *g = (10f64.powf(db / 20.0) * scale) as f32;
        }
    }

    fn frame(&mut self) {
        let (n, hop) = (self.n, self.hop);
        for c in 0..2 {
            for (o, (x, w)) in self
                .frame
                .iter_mut()
                .zip(self.input[c].iter().zip(&self.win))
            {
                *o = x * w;
            }
            let _ = self
                .fwd
                .process_with_scratch(&mut self.frame, &mut self.spec, &mut self.scratch);
            for (s, g) in self.spec.iter_mut().zip(&self.gains) {
                *s *= *g;
            }
            let last = self.spec.len() - 1;
            self.spec[0].im = 0.0;
            self.spec[last].im = 0.0;
            let _ = self
                .inv
                .process_with_scratch(&mut self.spec, &mut self.frame, &mut self.scratch);
            for ((a, x), w) in self.acc[c].iter_mut().zip(&self.frame).zip(&self.win) {
                *a += x * w;
            }
            self.out[c].copy_from_slice(&self.acc[c][..hop]);
            self.acc[c].copy_within(hop.., 0);
            self.acc[c][n - hop..].fill(0.0);
            self.input[c].copy_within(hop.., 0);
        }
    }
}

impl Processor for CurveEq {
    fn process(&mut self, buf: &mut [f32]) {
        let start = self.n - self.hop;
        for f in buf.as_chunks_mut::<2>().0 {
            for (c, v) in f.iter_mut().enumerate() {
                self.input[c][start + self.pos] = *v;
                *v = self.out[c][self.pos];
            }
            self.pos += 1;
            if self.pos == self.hop {
                self.pos = 0;
                self.frame();
            }
        }
    }

    fn latency(&self) -> usize {
        self.n
    }

    fn reset(&mut self) {
        for c in 0..2 {
            self.input[c].fill(0.0);
            self.acc[c].fill(0.0);
            self.out[c].fill(0.0);
        }
        self.pos = 0;
    }

    /// The points can move while it plays; the frame length depends only
    /// on the rate, so nothing needs a rebuild.
    fn update(&mut self, p: &Params) -> bool {
        self.set_gains(p);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    const RATE: u32 = 48_000;

    fn params(list: &[(&str, f64)]) -> Params {
        let saved: Vec<(String, f64)> = list.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        crate::params_from(ID, &saved).unwrap()
    }

    fn heard_db(p: &Params, hz: f64) -> f64 {
        let mut fx = crate::build(ID, RATE, p).unwrap();
        let frames = 2 * RATE as usize;
        let amp = 0.25;
        let mut buf: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let v = (amp * (2.0 * PI * hz * i as f64 / f64::from(RATE)).sin()) as f32;
                [v, v]
            })
            .collect();
        for block in buf.chunks_mut(512) {
            fx.process(block);
        }
        let periods = (0.4 * hz).floor().max(1.0);
        let len = (periods * f64::from(RATE) / hz).round() as usize;
        let ms: f64 = buf[(frames - len) * 2..]
            .chunks(2)
            .map(|f| f64::from(f[0]).powi(2))
            .sum::<f64>()
            / len as f64;
        20.0 * ((2.0 * ms).sqrt() / amp).log10()
    }

    #[test]
    fn frame_length_follows_the_rate() {
        assert_eq!(frame_len(48_000), 4096);
        assert_eq!(frame_len(44_100), 4096);
        assert_eq!(frame_len(96_000), 8192);
        assert_eq!(frame_len(192_000), 8192);
        assert_eq!(frame_len(2_000), 256);
        let fx = CurveEq::new(&params(&[]), 48_000);
        assert_eq!(fx.latency(), 4096);
    }

    #[test]
    fn the_curve_goes_through_its_points_and_holds_beyond() {
        let pts = [(100.0, 0.0), (400.0, -12.0), (1600.0, 6.0)];
        assert_eq!(curve_db(&pts, 100.0), 0.0);
        assert_eq!(curve_db(&pts, 400.0), -12.0);
        assert_eq!(curve_db(&pts, 1600.0), 6.0);
        assert_eq!(curve_db(&pts, 20.0), 0.0, "holds below");
        assert_eq!(curve_db(&pts, 15_000.0), 6.0, "holds above");
        // Halfway in pitch (200 Hz is an octave above 100, an octave below
        // 400) is halfway in dB.
        assert!((curve_db(&pts, 200.0) + 6.0).abs() < 1e-9);
        assert!((curve_db(&pts, 800.0) + 3.0).abs() < 1e-9);
        // No corner at a point: the slope eases to zero on both sides.
        let near = |d: f64| curve_db(&pts, 400.0 * 2f64.powf(d)) + 12.0;
        assert!(near(1e-3).abs() < 1e-4 && near(-1e-3).abs() < 1e-4);
        assert_eq!(curve_db(&[], 1000.0), 0.0);
    }

    #[test]
    fn points_sort_by_frequency() {
        let p = params(&[("p1_freq", 5000.0), ("p1_gain", -3.0)]);
        let pts = points(&p);
        assert_eq!(pts.len(), POINTS);
        assert!(pts.windows(2).all(|w| w[0].0 <= w[1].0));
        assert!(pts.contains(&(5000.0, -3.0)));
    }

    #[test]
    fn flat_by_default() {
        let p = params(&[]);
        for hz in [20.0, 100.0, 1000.0, 10_000.0] {
            assert_eq!(crate::effects::filters::response_db(ID, &p, RATE, hz), Some(0.0));
        }
        let mut fx = crate::build(ID, RATE, &p).unwrap();
        let mut seed = 1u32;
        let mut buf: Vec<f32> = (0..40_000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect();
        let dry = buf.clone();
        for block in buf.chunks_mut(512) {
            fx.process(block);
        }
        let lat = fx.latency() * 2;
        let err = buf[lat..]
            .iter()
            .zip(&dry)
            .fold(0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(err < 1e-4, "error {err}");
    }

    #[test]
    fn a_point_cuts_by_its_gain() {
        let p = params(&[("p5_gain", -18.0)]);
        let drawn = crate::effects::filters::response_db(ID, &p, RATE, 1500.0).unwrap();
        assert!((drawn + 18.0).abs() < 1e-9);
        let heard = heard_db(&p, 1500.0);
        assert!((heard + 18.0).abs() < 0.3, "heard {heard}");
        // Elsewhere what is heard follows the curve.
        for hz in [59.0, 590.0, 1010.0, 2300.0, 5900.0] {
            let d = curve_db(&points(&p), hz);
            let h = heard_db(&p, hz);
            assert!((h - d).abs() < 0.3, "{hz}: heard {h}, drawn {d}");
        }
    }

    #[test]
    fn a_click_comes_out_at_the_latency() {
        let p = params(&[("p5_gain", -18.0), ("p8_gain", 6.0)]);
        let mut fx = crate::build(ID, RATE, &p).unwrap();
        let at = 10_000;
        let mut buf = vec![0f32; 2 * 30_000];
        buf[at * 2] = 1.0;
        buf[at * 2 + 1] = 1.0;
        for block in buf.chunks_mut(512) {
            fx.process(block);
        }
        let left: Vec<f32> = buf.iter().step_by(2).copied().collect();
        let (peak_at, peak) = left
            .iter()
            .enumerate()
            .fold((0, 0f32), |m, (i, &v)| if v.abs() > m.1 { (i, v.abs()) } else { m });
        assert_eq!(peak_at, at + fx.latency());
        // Not smeared: little energy far from the peak.
        let far = left
            .iter()
            .enumerate()
            .filter(|(i, _)| i.abs_diff(peak_at) > 512)
            .fold(0f32, |m, (_, &v)| m.max(v.abs()));
        assert!(far < 0.01 * peak, "far {far} vs peak {peak}");
    }

    #[test]
    fn update_takes_new_points_in_place() {
        let mut fx = CurveEq::new(&params(&[]), RATE);
        assert!(fx.update(&params(&[("p2_gain", -6.0)])));
    }
}
