// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Streaming ("live") effects: they process stereo blocks in place, keep
//! their state between blocks, and never allocate after they are built,
//! so they can run on the audio thread inside effect chains. The offline
//! `Effect::process` runs the same processors over a whole buffer, so each
//! effect has one implementation.

use crate::biquad::{Biquad, Kind};
use crate::effects::Effect;
use af27_core::time::db_to_gain;

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

fn level_db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
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
            self.l[i].run(l);
            self.r[i].run(r);
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
        }
    }
}

impl Live for Compressor {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let x = a.abs().max(b.abs());
            let c = if x > self.env { self.ca } else { self.cr };
            self.env = c * self.env + (1.0 - c) * x;
            let over = level_db(self.env) - self.thr;
            let k = self.knee;
            let reduce = if k > 0.0 && over.abs() <= k / 2.0 {
                (1.0 / self.ratio - 1.0) * (over + k / 2.0).powi(2) / (2.0 * k)
            } else if over > 0.0 {
                (1.0 / self.ratio - 1.0) * over
            } else {
                0.0
            };
            let g = db_to_gain(reduce) * self.makeup;
            *a *= g;
            *b *= g;
        }
    }
    fn set(&mut self, vals: &[f32]) {
        self.thr = v(vals, 0, -18.0);
        self.ratio = v(vals, 1, 3.0).max(1.0);
        self.ca = coeff(v(vals, 2, 10.0), self.rate);
        self.cr = coeff(v(vals, 3, 120.0), self.rate);
        self.knee = v(vals, 4, 6.0);
        self.makeup = db_to_gain(v(vals, 5, 0.0));
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

impl Live for Limiter {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let mask = self.dl.len() - 1;
        let step = 1.0 / self.look as f32;
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let (xa, xb) = (*a * self.boost, *b * self.boost);
            let peak = xa.abs().max(xb.abs());
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

impl Live for Gate {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let x = a.abs().max(b.abs());
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

/// A chain of live effects, plus its own input and output gain and mix.
pub struct Chain {
    pub slots: Vec<(Box<dyn Live>, bool)>,
    dry_l: Vec<f32>,
    dry_r: Vec<f32>,
    pub mix: f32,
}

impl Chain {
    pub fn new(slots: Vec<(Box<dyn Live>, bool)>, max_block: usize) -> Chain {
        Chain {
            slots,
            dry_l: vec![0.0; max_block],
            dry_r: vec![0.0; max_block],
            mix: 1.0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|(_, bypass)| *bypass)
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if self.is_empty() {
            return;
        }
        let n = l.len().min(self.dry_l.len());
        let wet_only = self.mix >= 0.999;
        if !wet_only {
            self.dry_l[..n].copy_from_slice(&l[..n]);
            self.dry_r[..n].copy_from_slice(&r[..n]);
        }
        for (p, bypass) in &mut self.slots {
            if !*bypass {
                p.process(&mut l[..n], &mut r[..n]);
            }
        }
        if !wet_only {
            let m = self.mix;
            for i in 0..n {
                l[i] = l[i] * m + self.dry_l[i] * (1.0 - m);
                r[i] = r[i] * m + self.dry_r[i] * (1.0 - m);
            }
        }
    }

    pub fn set(&mut self, slot: usize, vals: &[f32]) {
        if let Some((p, _)) = self.slots.get_mut(slot) {
            p.set(vals);
        }
    }

    pub fn set_bypass(&mut self, slot: usize, bypass: bool) {
        if let Some((_, b)) = self.slots.get_mut(slot) {
            *b = bypass;
        }
    }

    pub fn reset(&mut self) {
        for (p, _) in &mut self.slots {
            p.reset();
        }
    }
}

/// Builds a live processor for a slot this crate does not know (another
/// crate's effects register one with [`register_factory`]).
pub type Factory = fn(&af27_core::EffectSlot, u32) -> Option<Box<dyn Live>>;

static FACTORY: std::sync::OnceLock<Factory> = std::sync::OnceLock::new();

pub fn register_factory(f: Factory) {
    let _ = FACTORY.set(f);
}

/// The live processor for one stored slot, from the built-in effects or
/// a registered factory.
pub fn build_slot(s: &af27_core::EffectSlot, rate: u32) -> Option<Box<dyn Live>> {
    Effect::from_id(&s.effect)
        .and_then(|e| build(e, rate, &s.values))
        .or_else(|| FACTORY.get().and_then(|f| f(s, rate)))
}

/// Builds a chain from stored slots (effect id, values, bypass). A slot
/// whose effect cannot run live passes the sound through.
pub fn build_chain(slots: &[af27_core::EffectSlot], rate: u32, max_block: usize) -> Chain {
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
    fn chain_bypass_and_mix() {
        let slots = vec![af27_core::EffectSlot {
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
