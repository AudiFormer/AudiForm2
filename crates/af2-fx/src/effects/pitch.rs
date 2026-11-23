// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Pitch Shift: changes pitch without changing speed, by up to an octave
//! either way in semitones and cents. Written for AudiForm 2 to behave as
//! Audiform's Pitch Shift does (same controls and keys, so its sessions
//! carry over); the code is our own.
//!
//! Each short frame (about 21 ms, Hann-windowed, seven-eighths overlap)
//! goes through a phase vocoder that stretches it in time by the pitch
//! factor: every frequency's phase is moved on at its own measured rate
//! for the longer (or shorter) hop, with the bins around each spectral
//! peak locked to the peak so partials stay whole. Both sides turn by the
//! same angles, worked out on their sum (or their difference when that
//! is far louder), so the stereo picture holds. A sudden onset starts the
//! phases afresh so drums stay sharp. Each stretched frame is then read
//! back faster or slower (cubic interpolation) to its old length, which
//! moves its pitch, and the frames are overlapped again.
//!
//! At no shift the sound passes untouched with no delay; otherwise the
//! delay is two frames, reported as latency.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use super::{num, EffectInfo, Params, Processor};

pub const ID: &str = "af.pitch_shift";

pub fn pitch_info() -> EffectInfo {
    EffectInfo {
        id: ID,
        sidechain: false,
        name: "Pitch Shift",
        category: "Pitch",
        description: "Changes pitch without changing speed, by up to an octave up or down.",
        params: vec![
            num("semitones", "Semitones", "st", -12.0, 12.0, 0.0, 1.0),
            num("cents", "Fine tune", "cents", -100.0, 100.0, 0.0, 1.0),
        ],
    }
}

/// The pitch factor for these settings (2 is an octave up), kept to an
/// octave either way.
pub fn factor(p: &Params) -> f64 {
    let st = p.get("semitones") + p.get("cents") / 100.0;
    2f64.powf(st / 12.0).clamp(0.5, 2.0)
}

/// The frame length for `rate`: a power of two near 21 ms.
fn frame_len(rate: u32) -> usize {
    let target = f64::from(rate) * 0.015 * 1.4;
    let p = target.max(1.0).log2().round() as u32;
    (1usize << p).clamp(512, 16_384)
}

fn princarg(x: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    x - TAU * ((x + PI) / TAU).floor()
}

/// Catmull-Rom read of `g` at fractional index `x` (zero outside).
fn cubic(g: &[f32], x: f64) -> f32 {
    let i = x.floor() as isize;
    let t = (x - i as f64) as f32;
    let at = |k: isize| {
        if k < 0 || k as usize >= g.len() {
            0.0
        } else {
            g[k as usize]
        }
    };
    let (y0, y1, y2, y3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c = -0.5 * y0 + 0.5 * y2;
    ((a * t + b) * t + c) * t + y1
}

pub struct PitchShift {
    factor: f64,
    /// Built to shift (a factor other than 1): its latency, kept while
    /// the factor is retuned live, even through 1.
    active: bool,
    n: usize,
    hop: usize,
    /// The last `n` input frames, a ring whose oldest frame is at `inw`.
    input: [Vec<f32>; 2],
    inw: usize,
    /// Frames since the last hop.
    since: usize,
    seen: usize,
    /// Overlapped output, a ring of `4n` read at `t`.
    acc: [Vec<f32>; 2],
    t: usize,
    window: Vec<f32>,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    time: Vec<f32>,
    spec: [Vec<Complex32>; 2],
    scratch_f: Vec<Complex32>,
    scratch_i: Vec<Complex32>,
    /// The reference spectrum's phases last frame, and the made phases.
    last_phase: Vec<f32>,
    out_phase: Vec<f32>,
    last_mag: Vec<f32>,
    flux_avg: f32,
    first: bool,
    grain: Vec<f32>,
    /// Bins of each peak's region: the peak each bin follows.
    peak_of: Vec<usize>,
    mag: Vec<f32>,
    phase: Vec<f32>,
    turn: Vec<f32>,
    /// Each frame's peaks (the bins leading a region) and the borders
    /// between them, kept so a frame does not allocate.
    leaders: Vec<usize>,
    borders: Vec<usize>,
}

impl PitchShift {
    pub fn new(p: &Params, rate: u32) -> Self {
        let n = frame_len(rate);
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let bins = n / 2 + 1;
        let window = (0..n)
            .map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos()) as f32)
            .collect();
        let factor = factor(p);
        Self {
            factor,
            active: (factor - 1.0).abs() >= 1e-9,
            n,
            hop: n / 8,
            input: [vec![0.0; n], vec![0.0; n]],
            inw: 0,
            since: 0,
            seen: 0,
            acc: [vec![0.0; 4 * n], vec![0.0; 4 * n]],
            t: 0,
            window,
            scratch_f: fwd.make_scratch_vec(),
            scratch_i: inv.make_scratch_vec(),
            fwd,
            inv,
            time: vec![0.0; n],
            spec: [vec![Complex32::default(); bins], vec![Complex32::default(); bins]],
            last_phase: vec![0.0; bins],
            out_phase: vec![0.0; bins],
            last_mag: vec![0.0; bins],
            flux_avg: 0.0,
            first: true,
            grain: vec![0.0; n],
            peak_of: vec![0; bins],
            mag: vec![0.0; bins],
            phase: vec![0.0; bins],
            turn: vec![0.0; bins],
            leaders: Vec::with_capacity(bins),
            borders: Vec::with_capacity(bins),
        }
    }

    fn bypass(&self) -> bool {
        !self.active
    }

    /// One frame: analyse the last `n` inputs, stretch, read back and add
    /// into the output ring.
    fn frame(&mut self) {
        let n = self.n;
        let bins = n / 2 + 1;
        for ch in 0..2 {
            let w = self.inw;
            for i in 0..n {
                self.time[i] = self.input[ch][(w + i) % n] * self.window[i];
            }
            let _ = self.fwd.process_with_scratch(&mut self.time, &mut self.spec[ch], &mut self.scratch_f);
        }
        // The reference: the sum, or the difference when it is far louder.
        let (mut e_mid, mut e_side) = (0.0f32, 0.0f32);
        for k in 0..bins {
            e_mid += (self.spec[0][k] + self.spec[1][k]).norm_sqr();
            e_side += (self.spec[0][k] - self.spec[1][k]).norm_sqr();
        }
        let sign = if e_side > 10.0 * e_mid { -1.0 } else { 1.0 };
        let mut peak = 0.0f32;
        for k in 0..bins {
            let c = self.spec[0][k] + self.spec[1][k] * sign;
            self.mag[k] = c.norm();
            self.phase[k] = c.arg();
            peak = peak.max(self.mag[k]);
        }
        // A sudden onset: start the phases afresh.
        let total: f32 = self.mag.iter().sum::<f32>().max(1e-12);
        let flux: f32 = self
            .mag
            .iter()
            .zip(&self.last_mag)
            .map(|(m, l)| (m - l).max(0.0))
            .sum::<f32>()
            / total;
        let onset = flux > 0.3 && flux > 2.5 * self.flux_avg;
        self.flux_avg += 0.1 * (flux - self.flux_avg);
        self.last_mag.copy_from_slice(&self.mag);

        let ha = self.hop as f32;
        let hs = (self.hop as f64 * self.factor) as f32;
        let tau = std::f32::consts::TAU;
        // Peaks, and each bin's peak (the nearer one, split at the dip).
        let floor = peak * 1e-5;
        let mut leaders = std::mem::take(&mut self.leaders);
        let mut borders = std::mem::take(&mut self.borders);
        leaders.clear();
        leaders.extend(
            (1..bins - 1)
                .filter(|&k| self.mag[k] > self.mag[k - 1] && self.mag[k] >= self.mag[k + 1] && self.mag[k] > floor),
        );
        if leaders.is_empty() {
            leaders.extend(0..bins);
        }
        // The border between two peaks: the lowest bin between them.
        borders.clear();
        borders.extend(
            leaders
                .windows(2)
                .map(|w| (w[0]..=w[1]).min_by(|x, y| self.mag[*x].total_cmp(&self.mag[*y])).unwrap_or(w[0])),
        );
        let mut j = 0;
        for k in 0..bins {
            while j < borders.len() && k > borders[j] {
                j += 1;
            }
            self.peak_of[k] = leaders[j];
        }
        // The made phase of each peak, then every bin turned by its peak's turn.
        for &k in &leaders {
            let omega = tau * k as f32 / n as f32;
            let made = if self.first || onset {
                self.phase[k]
            } else {
                let dev = princarg(self.phase[k] - self.last_phase[k] - omega * ha);
                self.out_phase[k] + (omega + dev / ha) * hs
            };
            self.turn[k] = princarg(made - self.phase[k]);
        }
        for k in 0..bins {
            let r = self.turn[self.peak_of[k]];
            self.out_phase[k] = self.phase[k] + r;
        }
        self.last_phase.copy_from_slice(&self.phase);
        self.first = false;
        self.leaders = leaders;
        self.borders = borders;

        // Read each stretched frame back at the factor's speed.
        let f = self.factor;
        let m = ((n as f64) / f).round() as usize;
        let scale = 1.0 / (n as f32 * 0.375 * m as f32 / ha);
        let lat = 2 * n;
        let len = self.acc[0].len();
        // The frame's centre is n/2 back; its read-back grain lands
        // centred on the same moment `lat` later.
        let start = self.t + lat - n / 2 - m / 2;
        for ch in 0..2 {
            let mut s = std::mem::take(&mut self.spec[ch]);
            for k in 0..bins {
                let r = self.turn[self.peak_of[k]];
                s[k] *= Complex32::from_polar(1.0, r);
            }
            s[0].im = 0.0;
            s[bins - 1].im = 0.0;
            let _ = self.inv.process_with_scratch(&mut s, &mut self.time, &mut self.scratch_i);
            self.spec[ch] = s;
            for i in 0..n {
                self.grain[i] = self.time[i] * self.window[i];
            }
            for j in 0..m {
                let v = cubic(&self.grain, j as f64 * f) * scale;
                let at = (start + j) % len;
                self.acc[ch][at] += v;
            }
        }
    }
}

impl Processor for PitchShift {
    fn process(&mut self, buf: &mut [f32]) {
        if self.bypass() {
            return;
        }
        let n = self.n;
        let len = self.acc[0].len();
        for fr in buf.as_chunks_mut::<2>().0 {
            let at = self.t % len;
            let out = [self.acc[0][at], self.acc[1][at]];
            self.acc[0][at] = 0.0;
            self.acc[1][at] = 0.0;
            for ch in 0..2 {
                self.input[ch][self.inw] = fr[ch];
            }
            self.inw = (self.inw + 1) % n;
            fr[0] = out[0];
            fr[1] = out[1];
            self.t += 1;
            self.seen += 1;
            self.since += 1;
            if self.since >= self.hop {
                self.since = 0;
                if self.seen >= n {
                    self.frame();
                }
            }
        }
    }

    fn latency(&self) -> usize {
        if self.bypass() {
            0
        } else {
            2 * self.n
        }
    }

    fn reset(&mut self) {
        for ch in 0..2 {
            self.input[ch].fill(0.0);
            self.acc[ch].fill(0.0);
        }
        self.since = 0;
        self.seen = 0;
        self.inw = 0;
        self.last_phase.fill(0.0);
        self.out_phase.fill(0.0);
        self.last_mag.fill(0.0);
        self.flux_avg = 0.0;
        self.first = true;
    }

    /// Retunes in place, so a drag or automation is heard as it moves. One
    /// built at a factor of 1 passes the sound straight through with no
    /// latency, so shifting it needs a rebuild; one already shifting keeps
    /// its latency even when brought back to 1.
    fn update(&mut self, p: &Params) -> bool {
        let f = factor(p);
        if !self.active && (f - 1.0).abs() >= 1e-9 {
            return false;
        }
        self.factor = f;
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
        Params::resolve(&pitch_info(), &saved)
    }

    fn tone(hz: f64, secs: f64) -> Vec<f32> {
        (0..(secs * f64::from(R)) as usize)
            .flat_map(|i| {
                let x = (std::f64::consts::TAU * hz * i as f64 / f64::from(R)).sin() as f32 * 0.5;
                [x, x]
            })
            .collect()
    }

    /// The strongest frequency in the left side of `b` (after `skip` frames).
    fn pitch_of(b: &[f32], skip: usize) -> f64 {
        let x: Vec<f32> = b.iter().step_by(2).skip(skip).take(16_384).copied().collect();
        let n = x.len();
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);
        let mut t: Vec<f32> = x
            .iter()
            .enumerate()
            .map(|(i, v)| v * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()))
            .collect();
        let mut s = fft.make_output_vec();
        fft.process(&mut t, &mut s).unwrap();
        let k = (1..s.len()).max_by(|a, b| s[*a].norm().total_cmp(&s[*b].norm())).unwrap();
        k as f64 * f64::from(R) / n as f64
    }

    #[test]
    fn no_shift_passes_the_sound_untouched() {
        let mut p = PitchShift::new(&params(&[]), R);
        let input = tone(440.0, 0.2);
        let mut b = input.clone();
        p.process(&mut b);
        assert_eq!(b, input);
        assert_eq!(p.latency(), 0);
    }

    #[test]
    fn it_moves_the_pitch_and_keeps_the_level_and_length() {
        for (st, want) in [(12.0, 880.0), (-12.0, 220.0), (7.0, 440.0 * 2f64.powf(7.0 / 12.0)), (-5.0, 440.0 * 2f64.powf(-5.0 / 12.0))] {
            let mut p = PitchShift::new(&params(&[("semitones", st)]), R);
            let mut b = tone(440.0, 1.5);
            let len = b.len();
            for c in b.chunks_mut(512) {
                p.process(c);
            }
            assert_eq!(b.len(), len);
            assert!(b.iter().all(|x| x.is_finite()));
            let got = pitch_of(&b, p.latency() + 4800);
            assert!((got / want - 1.0).abs() < 0.02, "{st} st: {got} Hz, wanted {want}");
            let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
            let lvl = rms(&b[2 * (p.latency() + 4800)..]) / rms(&tone(440.0, 0.5));
            assert!((0.7..1.4).contains(&lvl), "{st} st: level {lvl}");
        }
    }

    #[test]
    fn cents_tune_finely() {
        let a = factor(&params(&[("cents", 100.0)]));
        let b = factor(&params(&[("semitones", 1.0)]));
        assert!((a - b).abs() < 1e-12);
        assert_eq!(factor(&params(&[("semitones", 12.0), ("cents", 100.0)])), 2.0);
    }

    #[test]
    fn retunes_live_and_rebuilds_only_to_start_shifting() {
        let mut p = PitchShift::new(&params(&[("semitones", 3.0)]), R);
        assert!(p.update(&params(&[("semitones", 5.0)])));
        assert_eq!(p.factor, factor(&params(&[("semitones", 5.0)])));
        // Back to no shift: still running, same latency.
        assert!(p.update(&params(&[])));
        assert_eq!(p.latency(), 2 * p.n);
        // Built with no shift: shifting needs a rebuild (latency changes).
        let mut q = PitchShift::new(&params(&[]), R);
        assert!(!q.update(&params(&[("semitones", 2.0)])));
        assert_eq!(q.latency(), 0);
    }
}
