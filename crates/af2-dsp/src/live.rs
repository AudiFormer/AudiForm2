// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Streaming ("live") effects: they process stereo blocks in place, keep
//! their state between blocks, and never allocate after they are built,
//! so they can run on the audio thread inside effect chains. The offline
//! `Effect::process` runs the same processors over a whole buffer, so each
//! effect has one implementation.

use crate::biquad::{Biquad, Kind};
use crate::effects::Effect;
use af2_core::time::db_to_gain;

const BW_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

pub trait Live: Send {
    /// Processes one stereo block in place (`l` and `r` the same length).
    fn process(&mut self, l: &mut [f32], r: &mut [f32]);
    /// New settings. Must not allocate; filters are redesigned in place.
    fn set(&mut self, vals: &[f32]);
    /// Clears tails and envelopes (on seek or stop).
    fn reset(&mut self) {}
    /// How many samples late the output is (look-ahead).
    fn latency(&self) -> usize {
        0
    }
    /// Processes with a level key: one value per frame, the loudest of
    /// every channel the effect's copies work on (a speaker bed), so
    /// copies on different pairs move their gain together. Effects with
    /// no level detector ignore it.
    fn process_keyed(&mut self, l: &mut [f32], r: &mut [f32], _key: &[f32]) {
        self.process(l, r);
    }
    /// Processes with a sidechain: another track's sound (`kl`, `kr`, the
    /// same length as `l`) drives the effect's level detector in place of
    /// its own input. Effects with no detector ignore it. Must not allocate.
    fn process_sidechain(&mut self, l: &mut [f32], r: &mut [f32], _kl: &[f32], _kr: &[f32]) {
        self.process(l, r);
    }
    /// Whether [`Live::process_sidechain`] does anything (the effect has a
    /// level detector a sidechain can drive).
    fn takes_key(&self) -> bool {
        false
    }
    /// Sets this copy apart from other copies of the same effect on a
    /// bed's other speaker pairs (copy `k`), so sweeps and tails do not
    /// move in lockstep. Called once, after building, off the audio thread.
    fn vary(&mut self, _k: usize) {}
    /// This copy will run in an offline render (an export, Apply), not
    /// live: it may take its time on each block but must not leave any
    /// out. Effects that run in another process (VST3 plug-ins) start a
    /// copy of their own here. Called once, after building, off the
    /// audio thread.
    fn set_offline(&mut self, _offline: bool) {}
    /// A plug-in control (VST3 parameter `id`, normalized 0..1) moved by
    /// automation: it goes with the next block. Other effects ignore it.
    /// Must not allocate.
    fn set_plugin_param(&mut self, _id: u32, _v: f64) {}
    /// What runs behind this effect, for [`Chain::adopt`]: a rebuilt
    /// chain keeps the playing effect only when both say the same. 0 for
    /// effects that run here; a VST3 slot gives its running copy, so a
    /// copy started afresh (after it stopped, or one that was not
    /// installed when the chain was first built) is not swapped back out
    /// for the old one.
    fn runs(&self) -> usize {
        0
    }
}

fn v(vals: &[f32], i: usize, d: f32) -> f32 {
    vals.get(i).copied().unwrap_or(d)
}

fn coeff(ms: f32, rate: f32) -> f32 {
    if ms <= 0.0 {
        0.0
    } else {
        (-1.0 / (ms * 0.001 * rate)).exp()
    }
}

/// The louder side of a stereo key at frame `i` (0 past its end).
#[inline(always)]
fn stereo_level(kl: &[f32], kr: &[f32], i: usize) -> f32 {
    let a = kl.get(i).copied().unwrap_or(0.0).abs();
    let b = kr.get(i).copied().unwrap_or(0.0).abs();
    a.max(b)
}

fn level_db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

/// log2 for positive normal numbers, to about 1e-5 (well under 0.001 dB):
/// the exponent from the bits, the mantissa by a short odd series.
#[inline]
pub fn fast_log2(x: f32) -> f32 {
    let bits = x.max(1e-30).to_bits();
    let e = ((bits >> 23) & 0xff) as i32 - 127;
    let m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    let ln = 2.0 * s * (1.0 + s2 * (1.0 / 3.0 + s2 * (0.2 + s2 * (1.0 / 7.0 + s2 / 9.0))));
    e as f32 + ln * std::f32::consts::LOG2_E
}

/// 2^x to about 2e-6 relative, for x in -126..127.
#[inline]
pub fn fast_exp2(x: f32) -> f32 {
    let x = x.clamp(-126.0, 126.0);
    // Round to the nearest integer by truncating a positive number (one
    // instruction; f32::round is a library call), so the series only
    // covers -0.5..0.5, where it is tightest.
    let i = ((x + 256.5) as i32 - 256) as f32;
    let f = x - i;
    let p = 1.0
        + f * (std::f32::consts::LN_2
            + f * (0.240_226_5 + f * (0.055_504_1 + f * (0.009_618_1 + f * 0.001_333_6))));
    p * f32::from_bits(((i as i32 + 127) as u32) << 23)
}

/// Builds the live processor for `e`, or None for effects that need the
/// whole selection (Normalize, Reverse, Speed and the like).
pub fn build(e: Effect, rate: u32, vals: &[f32]) -> Option<Box<dyn Live>> {
    let defaults = e.defaults();
    let vals: Vec<f32> = (0..defaults.len())
        .map(|i| v(vals, i, defaults[i]))
        .collect();
    let mut p: Box<dyn Live> = match e {
        Effect::Amplify => Box::new(Gain { g: 1.0 }),
        Effect::Invert => Box::new(Gain { g: -1.0 }),
        Effect::DcOffset => Box::new(Filters::new(rate, FilterKind::DcBlock)),
        Effect::ParametricEq => Box::new(Filters::new(rate, FilterKind::Eq)),
        Effect::HighPass => Box::new(Filters::new(rate, FilterKind::HighPass)),
        Effect::LowPass => Box::new(Filters::new(rate, FilterKind::LowPass)),
        Effect::Notch => Box::new(Filters::new(rate, FilterKind::Notch)),
        Effect::HumRemover => Box::new(Filters::new(rate, FilterKind::Hum)),
        Effect::Compressor => Box::new(Compressor::new(rate)),
        Effect::Limiter => Box::new(Limiter::new(rate)),
        Effect::NoiseGate => Box::new(Gate::new(rate)),
        Effect::DeEsser => Box::new(DeEsser::new(rate)),
        Effect::Echo => Box::new(Echo::new(rate)),
        Effect::Reverb => Box::new(crate::reverb::Reverb::new(rate)),
        Effect::StereoWidth => Box::new(Width { w: 1.0 }),
        _ => return None,
    };
    p.set(&vals);
    Some(p)
}

struct Gain {
    g: f32,
}

impl Live for Gain {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for s in l.iter_mut().chain(r.iter_mut()) {
            *s *= self.g;
        }
    }
    fn set(&mut self, vals: &[f32]) {
        if self.g >= 0.0 {
            self.g = db_to_gain(v(vals, 0, 0.0));
        }
    }
}

struct Width {
    w: f32,
}

impl Live for Width {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let mid = (*a + *b) * 0.5;
            let side = (*a - *b) * 0.5 * self.w;
            *a = mid + side;
            *b = mid - side;
        }
    }
    fn set(&mut self, vals: &[f32]) {
        self.w = v(vals, 0, 1.0);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum FilterKind {
    DcBlock,
    Eq,
    HighPass,
    LowPass,
    Notch,
    Hum,
}

/// Up to eight biquads in series, per channel.
struct Filters {
    rate: f64,
    kind: FilterKind,
    n: usize,
    l: [Biquad; 8],
    r: [Biquad; 8],
}

impl Filters {
    fn new(rate: u32, kind: FilterKind) -> Filters {
        Filters {
            rate: rate as f64,
            kind,
            n: 0,
            l: [Biquad::default(); 8],
            r: [Biquad::default(); 8],
        }
    }
    fn put(&mut self, i: usize, b: Biquad) {
        // Keep each section's state so a change of settings does not click.
        self.l[i].set_coeffs_from(&b);
        self.r[i].set_coeffs_from(&b);
    }
}

impl Live for Filters {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for i in 0..self.n {
            // A band left at 0 dB changes nothing: skip its work.
            if self.l[i].is_identity() {
                continue;
            }
            Biquad::run_pair(&mut self.l[i], &mut self.r[i], l, r);
        }
    }
    fn set(&mut self, vals: &[f32]) {
        let fs = self.rate;
        let g = |i: usize, d: f32| v(vals, i, d) as f64;
        match self.kind {
            FilterKind::DcBlock => {
                self.put(0, Biquad::new(Kind::HighPass, fs, 5.0, BW_Q, 0.0));
                self.n = 1;
            }
            FilterKind::Eq => {
                self.put(
                    0,
                    Biquad::new(Kind::LowShelf, fs, g(0, 100.0), BW_Q, g(1, 0.0)),
                );
                self.put(
                    1,
                    Biquad::new(Kind::Peak, fs, g(2, 1000.0), g(4, 1.0), g(3, 0.0)),
                );
                self.put(
                    2,
                    Biquad::new(Kind::HighShelf, fs, g(5, 8000.0), BW_Q, g(6, 0.0)),
                );
                self.n = 3;
            }
            FilterKind::HighPass | FilterKind::LowPass => {
                let k = if self.kind == FilterKind::HighPass {
                    Kind::HighPass
                } else {
                    Kind::LowPass
                };
                let stages = g(1, 2.0).round().clamp(1.0, 4.0) as usize;
                let b = Biquad::new(k, fs, g(0, 1000.0), BW_Q, 0.0);
                for i in 0..stages {
                    self.put(i, b);
                }
                self.n = stages;
            }
            FilterKind::Notch => {
                self.put(
                    0,
                    Biquad::new(Kind::Notch, fs, g(0, 1000.0), g(1, 20.0), 0.0),
                );
                self.n = 1;
            }
            FilterKind::Hum => {
                let base = g(0, 60.0);
                let n = g(1, 4.0).round().clamp(1.0, 8.0) as usize;
                let mut k = 0;
                for h in 1..=n {
                    let f = base * h as f64;
                    if f < fs * 0.45 {
                        self.put(k, Biquad::new(Kind::Notch, fs, f, g(2, 30.0), 0.0));
                        k += 1;
                    }
                }
                self.n = k;
            }
        }
    }
    fn reset(&mut self) {
        for b in self.l.iter_mut().chain(self.r.iter_mut()) {
            b.clear();
        }
    }
}

/// Feed-forward compressor, peak detector, channels linked.
struct Compressor {
    rate: f32,
    thr: f32,
    ratio: f32,
    ca: f32,
    cr: f32,
    knee: f32,
    makeup: f32,
    env: f32,
    /// Below this envelope (the knee's start, linear) the gain is just
    /// the makeup, so the logarithms can be skipped.
    quiet: f32,
}

impl Compressor {
    fn new(rate: u32) -> Compressor {
        Compressor {
            rate: rate as f32,
            thr: -18.0,
            ratio: 3.0,
            ca: 0.0,
            cr: 0.0,
            knee: 6.0,
            makeup: 1.0,
            env: 0.0,
            quiet: 0.0,
        }
    }
}

impl Compressor {
    /// `key`: a side-chain level per frame (the loudest speaker of a bed),
    /// or None to listen to this pair itself.
    fn run(&mut self, l: &mut [f32], r: &mut [f32], key: Option<&[f32]>) {
        // Two copies of the loop, so the common case has no per-sample test.
        match key {
            Some(k) => self.run_with(l, r, |i, _, _| k.get(i).copied().unwrap_or(0.0).abs()),
            None => self.run_with(l, r, |_, a, b| a.abs().max(b.abs())),
        }
    }

    #[inline(always)]
    fn run_with(&mut self, l: &mut [f32], r: &mut [f32], level: impl Fn(usize, f32, f32) -> f32) {
        for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let x = level(i, *a, *b);
            let c = if x > self.env { self.ca } else { self.cr };
            self.env = c * self.env + (1.0 - c) * x;
            if self.env <= self.quiet {
                *a *= self.makeup;
                *b *= self.makeup;
                continue;
            }
            // 20·log10(x) = 6.0206·log2(x), with the fast log2 and exp2.
            let over = 6.020_6 * fast_log2(self.env) - self.thr;
            let k = self.knee;
            let reduce = if k > 0.0 && over.abs() <= k / 2.0 {
                (1.0 / self.ratio - 1.0) * (over + k / 2.0).powi(2) / (2.0 * k)
            } else if over > 0.0 {
                (1.0 / self.ratio - 1.0) * over
            } else {
                0.0
            };
            let g = fast_exp2(reduce * 0.166_096_4) * self.makeup;
            *a *= g;
            *b *= g;
        }
    }
}

impl Live for Compressor {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        self.run(l, r, None);
    }
    fn process_keyed(&mut self, l: &mut [f32], r: &mut [f32], key: &[f32]) {
        self.run(l, r, Some(key));
    }
    fn takes_key(&self) -> bool {
        true
    }
    fn process_sidechain(&mut self, l: &mut [f32], r: &mut [f32], kl: &[f32], kr: &[f32]) {
        self.run_with(l, r, |i, _, _| stereo_level(kl, kr, i));
    }
    fn set(&mut self, vals: &[f32]) {
        self.thr = v(vals, 0, -18.0);
        self.ratio = v(vals, 1, 3.0).max(1.0);
        self.ca = coeff(v(vals, 2, 10.0), self.rate);
        self.cr = coeff(v(vals, 3, 120.0), self.rate);
        self.knee = v(vals, 4, 6.0).max(0.0);
        self.makeup = db_to_gain(v(vals, 5, 0.0));
        self.quiet = db_to_gain(self.thr - self.knee / 2.0);
    }
    fn reset(&mut self) {
        self.env = 0.0;
    }
}

/// Look-ahead limiter. The sound is delayed by the look-ahead so the gain
/// can come down before a peak arrives; a final clamp keeps the ceiling.
struct Limiter {
    rate: f32,
    ceiling: f32,
    boost: f32,
    cr: f32,
    look: usize,
    dl: [f32; 2048],
    dr: [f32; 2048],
    pos: usize,
    /// The lowest gain needed in the window, and how long to hold it.
    target: f32,
    hold: usize,
    g: f32,
}

impl Limiter {
    fn new(rate: u32) -> Limiter {
        Limiter {
            rate: rate as f32,
            ceiling: 1.0,
            boost: 1.0,
            cr: 0.0,
            look: 1,
            dl: [0.0; 2048],
            dr: [0.0; 2048],
            pos: 0,
            target: 1.0,
            hold: 0,
            g: 1.0,
        }
    }
}

impl Limiter {
    fn run(&mut self, l: &mut [f32], r: &mut [f32], key: Option<&[f32]>) {
        match key {
            Some(k) => self.run_with(l, r, |i, _, _| k.get(i).copied().unwrap_or(0.0).abs()),
            None => self.run_with(l, r, |_, a, b| a.abs().max(b.abs())),
        }
    }

    /// `level(i, a, b)`: the detector's level for frame `i` (`a`, `b` the
    /// frame before the boost); the boost is applied to it here.
    #[inline(always)]
    fn run_with(&mut self, l: &mut [f32], r: &mut [f32], level: impl Fn(usize, f32, f32) -> f32) {
        let mask = self.dl.len() - 1;
        let step = 1.0 / self.look as f32;
        for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let (xa, xb) = (*a * self.boost, *b * self.boost);
            let peak = level(i, *a, *b) * self.boost;
            let need = if peak > self.ceiling {
                self.ceiling / peak
            } else {
                1.0
            };
            if need <= self.target {
                self.target = need;
                self.hold = self.look;
            } else if self.hold > 0 {
                self.hold -= 1;
            } else {
                self.target = need;
            }
            if self.target < self.g {
                // Reach the target within the look-ahead.
                self.g = (self.g - (self.g - self.target).max(1e-6) * step * 4.0).max(self.target);
            } else {
                self.g = self.cr * self.g + (1.0 - self.cr) * self.target;
            }
            self.dl[self.pos] = xa;
            self.dr[self.pos] = xb;
            let read = (self.pos + self.dl.len() - self.look) & mask;
            let c = self.ceiling;
            *a = (self.dl[read] * self.g).clamp(-c, c);
            *b = (self.dr[read] * self.g).clamp(-c, c);
            self.pos = (self.pos + 1) & mask;
        }
    }
}

impl Live for Limiter {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        self.run(l, r, None);
    }
    fn process_keyed(&mut self, l: &mut [f32], r: &mut [f32], key: &[f32]) {
        self.run(l, r, Some(key));
    }
    fn takes_key(&self) -> bool {
        true
    }
    fn process_sidechain(&mut self, l: &mut [f32], r: &mut [f32], kl: &[f32], kr: &[f32]) {
        self.run_with(l, r, |i, _, _| stereo_level(kl, kr, i));
    }
    fn set(&mut self, vals: &[f32]) {
        self.ceiling = db_to_gain(v(vals, 0, -1.0));
        self.boost = db_to_gain(v(vals, 1, 0.0));
        self.look = ((v(vals, 2, 5.0) * 0.001 * self.rate) as usize).clamp(1, 2000);
        self.cr = coeff(v(vals, 3, 100.0), self.rate);
    }
    fn reset(&mut self) {
        self.dl.fill(0.0);
        self.dr.fill(0.0);
        self.g = 1.0;
        self.target = 1.0;
    }
    fn latency(&self) -> usize {
        self.look
    }
}

struct Gate {
    rate: f32,
    thr: f32,
    ca: f32,
    cr: f32,
    hold_n: usize,
    floor: f32,
    det: f32,
    env: f32,
    g: f32,
    held: usize,
}

impl Gate {
    fn new(rate: u32) -> Gate {
        Gate {
            rate: rate as f32,
            thr: 0.01,
            ca: 0.0,
            cr: 0.0,
            hold_n: 0,
            floor: 0.001,
            det: coeff(5.0, rate as f32),
            env: 0.0,
            g: 0.001,
            held: 0,
        }
    }
}

impl Gate {
    fn run(&mut self, l: &mut [f32], r: &mut [f32], key: Option<&[f32]>) {
        match key {
            Some(k) => self.run_with(l, r, |i, _, _| k.get(i).copied().unwrap_or(0.0).abs()),
            None => self.run_with(l, r, |_, a, b| a.abs().max(b.abs())),
        }
    }

    #[inline(always)]
    fn run_with(&mut self, l: &mut [f32], r: &mut [f32], level: impl Fn(usize, f32, f32) -> f32) {
        for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let x = level(i, *a, *b);
            self.env = self.det * self.env + (1.0 - self.det) * x;
            let open = if self.env >= self.thr {
                self.held = self.hold_n;
                true
            } else if self.held > 0 {
                self.held -= 1;
                true
            } else {
                false
            };
            let t = if open { 1.0 } else { self.floor };
            let c = if t > self.g { self.ca } else { self.cr };
            self.g = c * self.g + (1.0 - c) * t;
            *a *= self.g;
            *b *= self.g;
        }
    }
}

impl Live for Gate {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        self.run(l, r, None);
    }
    fn process_keyed(&mut self, l: &mut [f32], r: &mut [f32], key: &[f32]) {
        self.run(l, r, Some(key));
    }
    fn takes_key(&self) -> bool {
        true
    }
    fn process_sidechain(&mut self, l: &mut [f32], r: &mut [f32], kl: &[f32], kr: &[f32]) {
        self.run_with(l, r, |i, _, _| stereo_level(kl, kr, i));
    }
    fn set(&mut self, vals: &[f32]) {
        self.thr = db_to_gain(v(vals, 0, -40.0));
        self.ca = coeff(v(vals, 1, 2.0), self.rate);
        self.hold_n = (v(vals, 2, 50.0) * 0.001 * self.rate) as usize;
        self.cr = coeff(v(vals, 3, 200.0), self.rate);
        self.floor = db_to_gain(v(vals, 4, -60.0));
    }
    fn reset(&mut self) {
        self.env = 0.0;
        self.g = self.floor;
    }
}

/// Split-band de-esser: a band-pass detector turns down only the top band.
struct DeEsser {
    rate: f64,
    thr: f32,
    max_red: f32,
    det: [Biquad; 2],
    hp: [Biquad; 2],
    env: f32,
    ca: f32,
    cr: f32,
}

impl DeEsser {
    fn new(rate: u32) -> DeEsser {
        DeEsser {
            rate: rate as f64,
            thr: -30.0,
            max_red: 8.0,
            det: [Biquad::default(); 2],
            hp: [Biquad::default(); 2],
            env: 0.0,
            ca: coeff(1.0, rate as f32),
            cr: coeff(60.0, rate as f32),
        }
    }
}

impl Live for DeEsser {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let x = (self.det[0].process(*a as f64).abs()).max(self.det[1].process(*b as f64).abs())
                as f32;
            let c = if x > self.env { self.ca } else { self.cr };
            self.env = c * self.env + (1.0 - c) * x;
            let over = (level_db(self.env) - self.thr).max(0.0);
            let g = db_to_gain(-over.min(self.max_red));
            for (s, hp) in [(&mut *a, 0usize), (&mut *b, 1usize)] {
                let high = self.hp[hp].process(*s as f64) as f32;
                *s = (*s - high) + high * g;
            }
        }
    }
    fn set(&mut self, vals: &[f32]) {
        let f = v(vals, 0, 6000.0) as f64;
        self.thr = v(vals, 1, -30.0);
        self.max_red = v(vals, 2, 8.0);
        let d = Biquad::new(Kind::BandPass, self.rate, f, 1.5, 0.0);
        let h = Biquad::new(Kind::HighPass, self.rate, f * 0.7, BW_Q, 0.0);
        for i in 0..2 {
            self.det[i].set_coeffs_from(&d);
            self.hp[i].set_coeffs_from(&h);
        }
    }
    fn reset(&mut self) {
        self.env = 0.0;
        for b in self.det.iter_mut().chain(self.hp.iter_mut()) {
            b.clear();
        }
    }
}

/// Feedback echo with a low-pass in the loop. Holds up to two seconds.
struct Echo {
    rate: f64,
    lines: [Vec<f32>; 2],
    pos: usize,
    d: usize,
    fb: f32,
    mix: f32,
    tone: [Biquad; 2],
}

impl Echo {
    fn new(rate: u32) -> Echo {
        let max = rate as usize * 2 + 1;
        Echo {
            rate: rate as f64,
            lines: [vec![0.0; max], vec![0.0; max]],
            pos: 0,
            d: 1,
            fb: 0.35,
            mix: 0.3,
            tone: [Biquad::default(); 2],
        }
    }
}

impl Live for Echo {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = self.lines[0].len();
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let read = (self.pos + n - self.d) % n;
            for (c, s) in [(0usize, &mut *a), (1usize, &mut *b)] {
                let delayed = self.lines[c][read];
                self.lines[c][self.pos] =
                    self.tone[c].process((*s + delayed * self.fb) as f64) as f32;
                *s = *s * (1.0 - self.mix) + delayed * self.mix;
            }
            self.pos = (self.pos + 1) % n;
        }
    }
    fn set(&mut self, vals: &[f32]) {
        let n = self.lines[0].len();
        self.d = (((v(vals, 0, 350.0) / 1000.0) as f64 * self.rate) as usize).clamp(1, n - 1);
        self.fb = v(vals, 1, 0.35);
        self.mix = v(vals, 2, 0.3);
        let t = Biquad::new(
            Kind::LowPass,
            self.rate,
            v(vals, 3, 6000.0) as f64,
            BW_Q,
            0.0,
        );
        self.tone[0].set_coeffs_from(&t);
        self.tone[1].set_coeffs_from(&t);
    }
    fn reset(&mut self) {
        for l in &mut self.lines {
            l.fill(0.0);
        }
    }
}

/// Multiplies both sides by `g`, skipping the work at unity.
#[inline]
fn scale(l: &mut [f32], r: &mut [f32], g: f32) {
    if g != 1.0 {
        for s in l.iter_mut().chain(r.iter_mut()) {
            *s *= g;
        }
    }
}

/// A fixed stereo delay (a ring of `d` frames), preallocated.
#[derive(Default)]
pub struct Delay {
    l: Vec<f32>,
    r: Vec<f32>,
    pos: usize,
}

impl Delay {
    pub fn new(frames: usize) -> Delay {
        Delay {
            l: vec![0.0; frames],
            r: vec![0.0; frames],
            pos: 0,
        }
    }

    pub fn frames(&self) -> usize {
        self.l.len()
    }

    /// Delays `l` and `r` in place. No allocation.
    pub fn run(&mut self, l: &mut [f32], r: &mut [f32]) {
        let d = self.l.len();
        if d == 0 {
            return;
        }
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let p = self.pos;
            *a = std::mem::replace(&mut self.l[p], *a);
            *b = std::mem::replace(&mut self.r[p], *b);
            self.pos = if p + 1 == d { 0 } else { p + 1 };
        }
    }

    pub fn clear(&mut self) {
        self.l.fill(0.0);
        self.r.fill(0.0);
        self.pos = 0;
    }
}

/// A chain of live effects, plus its own input and output gain and mix
/// (the rack's controls).
///
/// The chain's delay never changes with bypass: a bypassed effect that
/// looks ahead still holds the sound back by its latency, and the dry
/// part of the mix is held back by the whole chain's, so the mix graph
/// can line every path up once.
pub struct Chain {
    pub slots: Vec<(Box<dyn Live>, bool)>,
    /// Per slot: the delay a bypassed look-ahead effect leaves in place.
    pads: Vec<Delay>,
    /// The dry sound, held back by the chain's latency.
    dry_pad: Delay,
    dry_l: Vec<f32>,
    dry_r: Vec<f32>,
    /// Dry/wet: the dry sound is the chain's input after the input gain.
    pub mix: f32,
    /// Gain before the first effect (linear).
    pub input: f32,
    /// Gain after the mix (linear).
    pub output: f32,
    /// The whole rack is off: the sound passes untouched (no gains either).
    pub bypass: bool,
}

/// The stereo sidechain key one slot hears, if any.
pub type Key<'k> = Option<(&'k [f32], &'k [f32])>;

impl Chain {
    pub fn new(slots: Vec<(Box<dyn Live>, bool)>, max_block: usize) -> Chain {
        let pads: Vec<Delay> = slots.iter().map(|(p, _)| Delay::new(p.latency())).collect();
        let total = pads.iter().map(Delay::frames).sum();
        Chain {
            slots,
            pads,
            dry_pad: Delay::new(total),
            dry_l: vec![0.0; max_block],
            dry_r: vec![0.0; max_block],
            mix: 1.0,
            input: 1.0,
            output: 1.0,
            bypass: false,
        }
    }

    /// Takes a rack's settings (no allocation: safe on the audio thread).
    /// Takes `old`'s running effects in place of this chain's fresh ones
    /// (the caller checks they are the same effects in the same order), so
    /// reverb tails, compressor envelopes and plug-in streams carry on
    /// when the mix is rebuilt. This chain's settings stay: each effect is
    /// given `vals`. No allocation.
    pub fn adopt(&mut self, old: &mut Chain, vals: &[Vec<f32>]) {
        if self.slots.len() != old.slots.len() {
            return;
        }
        for (k, ((p, _), (q, _))) in self.slots.iter_mut().zip(old.slots.iter_mut()).enumerate() {
            // Another copy behind it (a plug-in started afresh): the new
            // effect stays.
            if p.runs() != q.runs() {
                continue;
            }
            std::mem::swap(p, q);
            if let Some(v) = vals.get(k) {
                p.set(v);
            }
        }
        for (a, b) in self.pads.iter_mut().zip(old.pads.iter_mut()) {
            if a.frames() == b.frames() {
                std::mem::swap(a, b);
            }
        }
        if self.dry_pad.frames() == old.dry_pad.frames() {
            std::mem::swap(&mut self.dry_pad, &mut old.dry_pad);
        }
    }

    pub fn set_rack(&mut self, rack: &af2_core::Rack) {
        self.bypass = rack.bypass;
        self.input = db_to_gain(rack.input_db);
        self.output = db_to_gain(rack.output_db);
        self.mix = rack.mix.clamp(0.0, 1.0);
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|(_, bypass)| *bypass)
    }

    /// How many frames late the chain's output is (as built).
    pub fn latency(&self) -> usize {
        self.pads.iter().map(Delay::frames).sum()
    }

    /// The delay of the slots before `slot` (a key for that slot is held
    /// back by this much so it meets the sound in step).
    pub fn latency_before(&self, slot: usize) -> usize {
        self.pads.iter().take(slot).map(Delay::frames).sum()
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        self.process_keyed(l, r, |_| None);
    }

    /// Processes with sidechain keys: `key(slot)` is the key that slot
    /// hears, or None for its own input.
    pub fn process_keyed<'k>(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        key: impl Fn(usize) -> Key<'k>,
    ) {
        let n = l.len().min(r.len()).min(self.dry_l.len());
        let (l, r) = (&mut l[..n], &mut r[..n]);
        if self.bypass {
            // Untouched, but as late as the chain always is.
            for pad in &mut self.pads {
                pad.run(l, r);
            }
            return;
        }
        scale(l, r, self.input);
        if self.is_empty() {
            for pad in &mut self.pads {
                pad.run(l, r);
            }
        } else {
            let wet_only = self.mix >= 0.999;
            if !wet_only {
                self.dry_l[..n].copy_from_slice(l);
                self.dry_r[..n].copy_from_slice(r);
                self.dry_pad.run(&mut self.dry_l[..n], &mut self.dry_r[..n]);
            }
            for (i, (p, bypass)) in self.slots.iter_mut().enumerate() {
                if *bypass {
                    self.pads[i].run(l, r);
                    continue;
                }
                match key(i) {
                    Some((kl, kr)) if kl.len() >= n && kr.len() >= n => {
                        p.process_sidechain(l, r, &kl[..n], &kr[..n])
                    }
                    _ => p.process(l, r),
                }
            }
            if !wet_only {
                let m = self.mix.max(0.0);
                for i in 0..n {
                    l[i] = l[i] * m + self.dry_l[i] * (1.0 - m);
                    r[i] = r[i] * m + self.dry_r[i] * (1.0 - m);
                }
            }
        }
        scale(l, r, self.output);
    }

    pub fn set(&mut self, slot: usize, vals: &[f32]) {
        if let Some((p, _)) = self.slots.get_mut(slot) {
            p.set(vals);
        }
    }

    /// A plug-in control in `slot` moved by automation (see
    /// [`Live::set_plugin_param`]). No allocation.
    pub fn set_plugin_param(&mut self, slot: usize, id: u32, v: f64) {
        if let Some((p, _)) = self.slots.get_mut(slot) {
            p.set_plugin_param(id, v);
        }
    }

    pub fn set_bypass(&mut self, slot: usize, bypass: bool) {
        if let Some((_, b)) = self.slots.get_mut(slot) {
            if bypass && !*b {
                if let Some(pad) = self.pads.get_mut(slot) {
                    pad.clear();
                }
            }
            *b = bypass;
        }
    }

    pub fn reset(&mut self) {
        for (p, _) in &mut self.slots {
            p.reset();
        }
        for pad in &mut self.pads {
            pad.clear();
        }
        self.dry_pad.clear();
    }

    /// Marks every effect as running in an offline render (see
    /// [`Live::set_offline`]).
    pub fn set_offline(&mut self, offline: bool) {
        for (p, _) in &mut self.slots {
            p.set_offline(offline);
        }
        // Effects may be late by another amount offline (a plug-in's own
        // copy has no live buffer): the delays follow. Allocates; called
        // off the audio thread.
        self.pads = self
            .slots
            .iter()
            .map(|(p, _)| Delay::new(p.latency()))
            .collect();
        self.dry_pad = Delay::new(self.latency());
    }
}

/// Builds a live processor for a slot this crate does not know (another
/// crate's effects register one with [`register_factory`]).
pub type Factory = fn(&af2_core::EffectSlot, u32) -> Option<Box<dyn Live>>;

static FACTORY: std::sync::OnceLock<Factory> = std::sync::OnceLock::new();

pub fn register_factory(f: Factory) {
    let _ = FACTORY.set(f);
}

/// The live processor for one stored slot, from the built-in effects or
/// a registered factory.
pub fn build_slot(s: &af2_core::EffectSlot, rate: u32) -> Option<Box<dyn Live>> {
    Effect::from_id(&s.effect)
        .and_then(|e| build(e, rate, &s.values))
        .or_else(|| FACTORY.get().and_then(|f| f(s, rate)))
}

/// Builds a chain from stored slots (effect id, values, bypass). A slot
/// whose effect cannot run live passes the sound through.
pub fn build_chain(slots: &[af2_core::EffectSlot], rate: u32, max_block: usize) -> Chain {
    let built = slots
        .iter()
        .map(|s| {
            let p = build_slot(s, rate).unwrap_or_else(|| Box::new(Gain { g: 1.0 }));
            (p, s.bypass)
        })
        .collect();
    Chain::new(built, max_block)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_log_and_exp_are_close() {
        for k in 0..2000 {
            let x = 1e-6 * 1.01f32.powi(k);
            assert!((fast_log2(x) - x.log2()).abs() < 2e-5, "{x}");
        }
        for k in -2000..2000 {
            let x = k as f32 * 0.01;
            let rel = (fast_exp2(x) - x.exp2()).abs() / x.exp2();
            assert!(rel < 1e-5, "{x}: {rel}");
        }
    }

    #[test]
    fn live_filters_keep_state_across_blocks() {
        // One block or many: the same output.
        let input: Vec<f32> = (0..4800)
            .map(|i| ((i * 37) % 101) as f32 / 101.0 - 0.5)
            .collect();
        let vals = Effect::ParametricEq
            .defaults()
            .iter()
            .enumerate()
            .map(|(i, d)| if i == 3 { 9.0 } else { *d })
            .collect::<Vec<_>>();
        let run = |block: usize| {
            let mut p = build(Effect::ParametricEq, 48_000, &vals).unwrap();
            let (mut l, mut r) = (input.clone(), input.clone());
            for (a, b) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
                p.process(a, b);
            }
            l
        };
        let whole = run(4800);
        let small = run(64);
        for (a, b) in whole.iter().zip(&small) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn dynamics_follow_a_sidechain_key() {
        let quiet = vec![0.2f32; 4800]; // -14 dBFS
        let loud = vec![0.9f32; 4800];
        let silent = vec![0.0f32; 4800];
        let cases = [
            (Effect::Compressor, vec![-10.0, 10.0, 1.0, 20.0, 0.0, 0.0]),
            (Effect::Limiter, vec![-12.0, 0.0, 1.0, 20.0]),
        ];
        for (e, vals) in cases {
            assert!(e.takes_key());
            let run = |key: Option<&[f32]>| {
                let mut p = build(e, 48_000, &vals).unwrap();
                assert!(p.takes_key());
                let (mut l, mut r) = (quiet.clone(), quiet.clone());
                match key {
                    Some(k) => p.process_sidechain(&mut l, &mut r, k, k),
                    None => p.process(&mut l, &mut r),
                }
                l[4000]
            };
            let own = run(None);
            assert!((own - 0.2).abs() < 1e-3, "{e:?} leaves quiet sound alone");
            assert!(run(Some(&loud)) < 0.15, "{e:?} ducks under a loud key");
            assert!((run(Some(&silent)) - own).abs() < 1e-6, "{e:?}");
        }
        // The gate opens for a loud key and stays shut for a quiet one.
        let gate = vec![-20.0, 0.5, 0.0, 5.0, -60.0];
        let run = |key: &[f32]| {
            let mut p = build(Effect::NoiseGate, 48_000, &gate).unwrap();
            let (mut l, mut r) = (vec![0.05f32; 4800], vec![0.05f32; 4800]);
            p.process_sidechain(&mut l, &mut r, key, key);
            l[4000]
        };
        assert!(run(&loud) > 0.04);
        assert!(run(&silent) < 0.001);
        assert!(!Effect::Echo.takes_key());
        assert!(!build(Effect::Echo, 48_000, &[]).unwrap().takes_key());
    }

    #[test]
    fn chain_rack_gains_mix_and_bypass() {
        let slots = vec![af2_core::EffectSlot {
            position: None,
            sidechain: None,
            effect: "amplify".into(),
            values: vec![6.0206],
            bypass: false,
            data: None,
            ext_id: None,
        }];
        let mut c = build_chain(&slots, 48_000, 512);
        let run = |c: &mut Chain| {
            let (mut l, mut r) = (vec![1.0f32; 16], vec![1.0f32; 16]);
            c.process(&mut l, &mut r);
            l[0]
        };
        c.set_rack(&af2_core::Rack {
            bypass: false,
            input_db: -6.0206,
            output_db: 6.0206,
            mix: 0.5,
        });
        // In 0.5, wet 1.0, dry 0.5, mixed 0.75, out 1.5.
        assert!((run(&mut c) - 1.5).abs() < 1e-3);
        c.set_bypass(0, true);
        assert!((run(&mut c) - 1.0).abs() < 1e-3);
        c.bypass = true;
        assert_eq!(run(&mut c), 1.0);
    }

    #[test]
    fn chain_bypass_and_mix() {
        let slots = vec![af2_core::EffectSlot {
            position: None,
            sidechain: None,
            effect: "amplify".into(),
            values: vec![-6.0206],
            bypass: false,
            data: None,
            ext_id: None,
        }];
        let mut c = build_chain(&slots, 48_000, 512);
        let (mut l, mut r) = (vec![1.0f32; 16], vec![1.0f32; 16]);
        c.process(&mut l, &mut r);
        assert!((l[0] - 0.5).abs() < 1e-3);
        c.mix = 0.5;
        let (mut l, mut r) = (vec![1.0f32; 16], vec![1.0f32; 16]);
        c.process(&mut l, &mut r);
        assert!((l[0] - 0.75).abs() < 1e-3);
        c.set_bypass(0, true);
        let (mut l, mut r) = (vec![1.0f32; 16], vec![1.0f32; 16]);
        c.process(&mut l, &mut r);
        assert_eq!(l[0], 1.0);
    }
}
