// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Dynamics, Limiter, De-Esser and Gain, written fresh for AudiForm 2.
//!
//! Audiform's versions of these (its `dsp/effects/dynamics.rs` and the
//! Gain in its effect framework) grew out of Altered Audio Editor, so no
//! code from them was read or used. Only their control lists (keys,
//! names, units, ranges, defaults and descriptions) were read, as a format
//! description, so `altaudio.dynamics`, `altaudio.limiter`,
//! `altaudio.deesser` and `altaudio.gain` slots in Audiform sessions load
//! here with their settings. The sound is written from textbook methods:
//!
//! * **Dynamics**: a compressor. The level of each sample (the louder
//!   side when the channels are linked) goes through a gain curve with a
//!   soft knee; the reduction, in dB, follows with separate attack and
//!   release times; Makeup gain is added after.
//! * **Limiter**: a look-ahead brick-wall limiter. The gain each sample
//!   needs to stay under the ceiling is held at its lowest over the
//!   look-ahead window and then averaged over the same window, so the
//!   gain is already down when the peak arrives, with no step in it.
//!   Release lets it back up. True peak also estimates the peaks a player
//!   draws between samples (a cubic through four samples, read at
//!   quarter steps). A last hard stop catches anything left over.
//! * **De-Esser**: listens to the highs above Frequency; when they flare
//!   over the threshold, only the highs are turned down, by how far they
//!   went over, up to Most reduction.
//! * **Gain**: a fixed level change, eased so moving it never clicks.

use std::collections::VecDeque;

use super::{choice, db_to_gain, num, toggle, EffectInfo, Params, Processor};
use crate::biquad::{Biquad, Coeffs, Shape};

pub const DYNAMICS: &str = "af.dynamics";
pub const LIMITER: &str = "af.limiter";
pub const DEESSER: &str = "af.deesser";
pub const GAIN: &str = "af.gain";

pub fn dynamics_info() -> EffectInfo {
    EffectInfo {
        id: DYNAMICS,
        name: "Dynamics",
        category: "Dynamics",
        description: "A compressor: evens out loud and quiet moments above the threshold.",
        params: vec![
            num("threshold", "Threshold", "dB", -60.0, 0.0, -20.0, 0.1),
            num("ratio", "Ratio", ":1", 1.0, 20.0, 3.0, 0.1).log(),
            num("attack", "Attack", "ms", 0.1, 200.0, 10.0, 0.1).log(),
            num("release", "Release", "ms", 5.0, 2000.0, 150.0, 1.0).log(),
            num("knee", "Knee", "dB", 0.0, 24.0, 6.0, 0.1),
            num("makeup", "Makeup gain", "dB", 0.0, 24.0, 0.0, 0.1),
            toggle("linked", "Link channels", true),
        ],
        sidechain: false,
    }
}

pub fn limiter_info() -> EffectInfo {
    EffectInfo {
        id: LIMITER,
        name: "Limiter",
        category: "Dynamics",
        description: "A brick-wall limiter: nothing goes over the ceiling. It looks ahead so it never clips, and in True peak mode it also catches the peaks between samples that players and converters make.",
        params: vec![
            choice("detect", "Detect", &["Peak", "True peak"], 0),
            num("ceiling", "Maximum level", "dB", -60.0, 0.0, -6.0, 0.1),
            num("input_boost", "Input boost", "dB", -24.0, 24.0, 0.0, 0.1),
            num("lookahead", "Look-ahead", "ms", 0.0, 20.0, 5.0, 0.1),
            num("release", "Release", "ms", 10.0, 1000.0, 100.0, 1.0).log(),
            toggle("linked", "Link channels", true),
        ],
        sidechain: false,
    }
}

pub fn deesser_info() -> EffectInfo {
    EffectInfo {
        id: DEESSER,
        name: "De-Esser",
        category: "Dynamics",
        description: "Softens harsh S and SH sounds by turning down only the highs when they flare.",
        params: vec![
            num("freq", "Frequency", "Hz", 2000.0, 12_000.0, 6000.0, 10.0).log(),
            num("threshold", "Threshold", "dB", -60.0, 0.0, -30.0, 0.1),
            num("reduction", "Most reduction", "dB", 0.0, 24.0, 8.0, 0.1),
        ],
        sidechain: false,
    }
}

pub fn gain_info() -> EffectInfo {
    EffectInfo {
        id: GAIN,
        name: "Gain",
        category: "Amplitude",
        description: "Turns the level up or down by a fixed amount.",
        params: vec![num("gain", "Gain", "dB", -48.0, 24.0, 0.0, 0.1)],
        sidechain: false,
    }
}

/// The one-pole coefficient for a time constant of `ms`.
fn coef(ms: f64, rate: u32) -> f32 {
    let n = ms.max(0.001) / 1000.0 * f64::from(rate);
    (-1.0 / n.max(0.5)).exp() as f32
}

fn amp_db(x: f32) -> f32 {
    20.0 * x.abs().max(1e-10).log10()
}

// ------------------------------------------------------------ Dynamics

/// The compressor's reduction in dB (positive) for a level of `db`.
pub fn reduction_db(db: f32, threshold: f32, ratio: f32, knee: f32) -> f32 {
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    let over = db - threshold;
    if knee > 0.0 && over.abs() <= knee * 0.5 {
        let t = over + knee * 0.5;
        slope * t * t / (2.0 * knee)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

pub struct Dynamics {
    rate: u32,
    threshold: f32,
    ratio: f32,
    knee: f32,
    makeup_db: f32,
    attack: f32,
    release: f32,
    linked: bool,
    /// The reduction now, in dB, per channel (only [0] when linked).
    gr: [f32; 2],
}

impl Dynamics {
    pub fn new(p: &Params, rate: u32) -> Self {
        let mut d = Self {
            rate,
            threshold: 0.0,
            ratio: 1.0,
            knee: 0.0,
            makeup_db: 0.0,
            attack: 0.0,
            release: 0.0,
            linked: true,
            gr: [0.0; 2],
        };
        d.set(p);
        d
    }

    fn set(&mut self, p: &Params) {
        self.threshold = p.get("threshold") as f32;
        self.ratio = p.get("ratio") as f32;
        self.knee = p.get("knee") as f32;
        self.makeup_db = p.get("makeup") as f32;
        self.attack = coef(p.get("attack"), self.rate);
        self.release = coef(p.get("release"), self.rate);
        self.linked = p.on("linked");
    }

    /// The reduction now, in dB (the larger side).
    pub fn reduction(&self) -> f32 {
        self.gr[0].max(self.gr[1])
    }

    fn follow(&mut self, c: usize, level_db: f32) -> f32 {
        let want = reduction_db(level_db, self.threshold, self.ratio, self.knee);
        let k = if want > self.gr[c] { self.attack } else { self.release };
        self.gr[c] = want + (self.gr[c] - want) * k;
        db_to_gain(f64::from(self.makeup_db - self.gr[c]))
    }
}

impl Processor for Dynamics {
    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.as_chunks_mut::<2>().0 {
            if self.linked {
                let g = self.follow(0, amp_db(f[0].abs().max(f[1].abs())));
                self.gr[1] = self.gr[0];
                f[0] *= g;
                f[1] *= g;
            } else {
                for (c, x) in f.iter_mut().enumerate() {
                    let g = self.follow(c, amp_db(*x));
                    *x *= g;
                }
            }
        }
    }

    fn reset(&mut self) {
        self.gr = [0.0; 2];
    }

    fn update(&mut self, p: &Params) -> bool {
        self.set(p);
        true
    }
}

// ------------------------------------------------------------ Limiter

/// The highest a cubic through `y` (four samples) reaches between the
/// middle two, read at quarter steps.
fn between_peak(y: [f32; 4]) -> f32 {
    let [y0, y1, y2, y3] = y;
    let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c = -0.5 * y0 + 0.5 * y2;
    [0.25f32, 0.5, 0.75]
        .iter()
        .map(|&t| (((a * t + b) * t + c) * t + y1).abs())
        .fold(0.0, f32::max)
}

/// One gain path of the limiter (one per channel, or one for both).
struct Lane {
    /// (sample number, needed gain), rising, for the lowest in the window.
    lowest: VecDeque<(u64, f32)>,
    /// The last `window` held gains, and their sum, for the average.
    held: Vec<f32>,
    pos: usize,
    sum: f64,
    released: f32,
    /// True peak: the last four samples and the last between-peak.
    hist: [f32; 4],
    prev_between: f32,
}

impl Lane {
    fn new(window: usize) -> Self {
        Self {
            lowest: VecDeque::with_capacity(window + 1),
            held: vec![1.0; window],
            pos: 0,
            sum: window as f64,
            released: 1.0,
            hist: [0.0; 4],
            prev_between: 0.0,
        }
    }

    fn reset(&mut self) {
        self.lowest.clear();
        self.held.iter_mut().for_each(|g| *g = 1.0);
        self.pos = 0;
        self.sum = self.held.len() as f64;
        self.released = 1.0;
        self.hist = [0.0; 4];
        self.prev_between = 0.0;
    }

    /// Takes the newest sample's peak (as a gain it needs) and gives the
    /// gain for the sample leaving the delay now.
    fn gain(&mut self, n: u64, need: f32, release: f32) -> f32 {
        let window = self.held.len() as u64;
        while self.lowest.back().is_some_and(|&(_, g)| g >= need) {
            self.lowest.pop_back();
        }
        self.lowest.push_back((n, need));
        while self.lowest.front().is_some_and(|&(i, _)| i + window <= n) {
            self.lowest.pop_front();
        }
        let low = self.lowest.front().map_or(1.0, |&(_, g)| g);
        let up = 1.0 + (self.released - 1.0) * release;
        self.released = low.min(up);
        self.sum += f64::from(self.released) - f64::from(self.held[self.pos]);
        self.held[self.pos] = self.released;
        self.pos = (self.pos + 1) % self.held.len();
        // Every held gain in the window is at most the one this sample
        // needed, so their average is too.
        ((self.sum / window as f64) as f32).min(1.0)
    }
}

pub struct Limiter {
    rate: u32,
    true_peak: bool,
    ceiling: f32,
    boost: f32,
    release: f32,
    linked: bool,
    extra: usize,
    lanes: [Lane; 2],
    /// The boosted audio, delayed to meet its gain.
    delay: Vec<[f32; 2]>,
    dpos: usize,
    n: u64,
    lookahead_ms: f64,
}

impl Limiter {
    pub fn new(p: &Params, rate: u32) -> Self {
        let lookahead_ms = p.get("lookahead");
        let window = ((lookahead_ms / 1000.0 * f64::from(rate)).round() as usize).max(1);
        let true_peak = p.index("detect") == 1;
        let extra = if true_peak { 2 } else { 0 };
        let mut l = Self {
            rate,
            true_peak,
            ceiling: 1.0,
            boost: 1.0,
            release: 0.0,
            linked: true,
            lanes: [Lane::new(window), Lane::new(window)],
            delay: vec![[0.0; 2]; window],
            extra,
            dpos: 0,
            n: 0,
            lookahead_ms,
        };
        l.set(p);
        l
    }

    fn set(&mut self, p: &Params) {
        self.ceiling = db_to_gain(p.get("ceiling"));
        self.boost = db_to_gain(p.get("input_boost"));
        self.release = coef(p.get("release"), self.rate);
        self.linked = p.on("linked");
    }

    /// The peak of the sample now being judged (in True peak mode, the
    /// one two samples back, with the peaks either side of it).
    fn peak(lane: &mut Lane, x: f32, true_peak: bool) -> (f32, f32) {
        if !true_peak {
            return (x.abs(), x);
        }
        lane.hist = [lane.hist[1], lane.hist[2], lane.hist[3], x];
        let between = between_peak(lane.hist);
        let judged = lane.hist[1];
        let p = judged.abs().max(between).max(lane.prev_between);
        lane.prev_between = between;
        (p, judged)
    }
}

impl Processor for Limiter {
    fn process(&mut self, buf: &mut [f32]) {
        let ceil = self.ceiling;
        let need = |peak: f32| if peak > ceil { ceil / peak } else { 1.0 };
        for f in buf.as_chunks_mut::<2>().0 {
            let (p0, s0) = Self::peak(&mut self.lanes[0], f[0] * self.boost, self.true_peak);
            let (p1, s1) = Self::peak(&mut self.lanes[1], f[1] * self.boost, self.true_peak);
            let (g0, g1) = if self.linked {
                let g = self.lanes[0].gain(self.n, need(p0.max(p1)), self.release);
                (g, g)
            } else {
                (
                    self.lanes[0].gain(self.n, need(p0), self.release),
                    self.lanes[1].gain(self.n, need(p1), self.release),
                )
            };
            self.n += 1;
            let len = self.delay.len();
            self.delay[self.dpos] = [s0, s1];
            // The oldest sample in the line is window - 1 behind the
            // newest judged one: the one this gain was made for.
            let out = self.delay[(self.dpos + 1) % len];
            self.dpos = (self.dpos + 1) % len;
            f[0] = (out[0] * g0).clamp(-ceil, ceil);
            f[1] = (out[1] * g1).clamp(-ceil, ceil);
        }
    }

    fn latency(&self) -> usize {
        self.delay.len() - 1 + self.extra
    }

    fn reset(&mut self) {
        self.lanes.iter_mut().for_each(Lane::reset);
        self.delay.iter_mut().for_each(|f| *f = [0.0; 2]);
        self.dpos = 0;
        self.n = 0;
    }

    fn update(&mut self, p: &Params) -> bool {
        // The look-ahead and the detector set the latency.
        if p.get("lookahead") != self.lookahead_ms || (p.index("detect") == 1) != self.true_peak {
            return false;
        }
        self.set(p);
        true
    }
}

// ------------------------------------------------------------ De-Esser

/// One band of a Linkwitz-Riley crossover: two Butterworth sections.
#[derive(Clone)]
struct Lr4([Biquad; 2]);

impl Lr4 {
    fn new(c: Coeffs) -> Self {
        Self([Biquad::new(c), Biquad::new(c)])
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.0[0].run(x);
        self.0[1].run(y)
    }

    fn set(&mut self, c: Coeffs) {
        self.0.iter_mut().for_each(|b| b.set(c));
    }

    fn reset(&mut self) {
        self.0.iter_mut().for_each(Biquad::reset);
    }
}

fn split(rate: u32, freq: f64) -> (Coeffs, Coeffs) {
    let q = std::f64::consts::FRAC_1_SQRT_2;
    (
        Coeffs::new(Shape::LowPass, rate, freq, q, 0.0),
        Coeffs::new(Shape::HighPass, rate, freq, q, 0.0),
    )
}

pub struct DeEsser {
    rate: u32,
    threshold: f32,
    most: f32,
    /// Per channel: the lows and the highs of a crossover at Frequency,
    /// which add back up to the whole sound (an all-pass, flat).
    lows: [Lr4; 2],
    highs: [Lr4; 2],
    /// The highs' level, in dB.
    level: f32,
    attack: f32,
    release: f32,
    freq: f64,
}

impl DeEsser {
    pub fn new(p: &Params, rate: u32) -> Self {
        let freq = p.get("freq");
        let (lo, hi) = split(rate, freq);
        let mut d = Self {
            rate,
            threshold: 0.0,
            most: 0.0,
            lows: [Lr4::new(lo), Lr4::new(lo)],
            highs: [Lr4::new(hi), Lr4::new(hi)],
            level: -120.0,
            attack: coef(1.0, rate),
            release: coef(60.0, rate),
            freq,
        };
        d.set(p);
        d
    }

    fn set(&mut self, p: &Params) {
        self.threshold = p.get("threshold") as f32;
        self.most = p.get("reduction") as f32;
        let f = p.get("freq");
        if f != self.freq {
            self.freq = f;
            let (lo, hi) = split(self.rate, f);
            self.lows.iter_mut().for_each(|b| b.set(lo));
            self.highs.iter_mut().for_each(|b| b.set(hi));
        }
    }

    /// How far the highs are turned down now, in dB.
    pub fn reduction(&self) -> f32 {
        (self.level - self.threshold).clamp(0.0, self.most)
    }
}

impl Processor for DeEsser {
    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.as_chunks_mut::<2>().0 {
            let l = [self.lows[0].run(f[0]), self.lows[1].run(f[1])];
            let h = [self.highs[0].run(f[0]), self.highs[1].run(f[1])];
            let now = amp_db(h[0].abs().max(h[1].abs()));
            let k = if now > self.level { self.attack } else { self.release };
            self.level = now + (self.level - now) * k;
            let g = db_to_gain(-f64::from(self.reduction()));
            // The second-order high-pass is out of phase with the
            // low-pass; LR4's two match, so the highs add back as they are.
            f[0] = l[0] + h[0] * g;
            f[1] = l[1] + h[1] * g;
        }
    }

    fn reset(&mut self) {
        self.lows.iter_mut().for_each(Lr4::reset);
        self.highs.iter_mut().for_each(Lr4::reset);
        self.level = -120.0;
    }

    fn update(&mut self, p: &Params) -> bool {
        self.set(p);
        true
    }
}

// ------------------------------------------------------------ Gain

pub struct Gain {
    target: f32,
    now: f32,
    ease: f32,
}

impl Gain {
    pub fn new(p: &Params, rate: u32) -> Self {
        let g = db_to_gain(p.get("gain"));
        Self {
            target: g,
            now: g,
            ease: coef(10.0, rate),
        }
    }
}

impl Processor for Gain {
    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.as_chunks_mut::<2>().0 {
            self.now = self.target + (self.now - self.target) * self.ease;
            f[0] *= self.now;
            f[1] *= self.now;
        }
    }

    fn reset(&mut self) {
        self.now = self.target;
    }

    fn update(&mut self, p: &Params) -> bool {
        self.target = db_to_gain(p.get("gain"));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params_from;

    fn sine(rate: u32, hz: f32, amp: f32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .flat_map(|i| {
                let v = amp * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin();
                [v, v]
            })
            .collect()
    }

    fn peak(b: &[f32]) -> f32 {
        b.iter().fold(0.0, |m, v| m.max(v.abs()))
    }

    fn run(fx: &mut dyn Processor, b: &mut [f32]) {
        for c in b.chunks_mut(512) {
            fx.process(c);
        }
    }

    #[test]
    fn the_curve_has_a_soft_knee() {
        assert_eq!(reduction_db(-30.0, -20.0, 4.0, 6.0), 0.0);
        assert!((reduction_db(-10.0, -20.0, 4.0, 6.0) - 7.5).abs() < 1e-5);
        let at = reduction_db(-20.0, -20.0, 4.0, 6.0);
        assert!(at > 0.0 && at < 1.0, "{at}");
        assert!((reduction_db(-10.0, -20.0, 4.0, 0.0) - 7.5).abs() < 1e-5);
    }

    #[test]
    fn dynamics_turns_a_loud_tone_down_by_its_ratio() {
        // -6 dBFS peak, threshold -20, 4:1, hard knee: 14 dB over -> 10.5 dB down.
        let p = params_from(
            DYNAMICS,
            &[("ratio".into(), 4.0), ("knee".into(), 0.0), ("release".into(), 2000.0)],
        )
        .unwrap();
        let mut fx = Dynamics::new(&p, 48_000);
        let mut b = sine(48_000, 1000.0, 0.5, 1.0);
        run(&mut fx, &mut b);
        let out = amp_db(peak(&b[b.len() / 2..]));
        assert!((out - (-6.02 - 10.5)).abs() < 1.0, "{out}");
        // Quiet sound passes untouched, plus makeup.
        let p = params_from(DYNAMICS, &[("makeup".into(), 6.0)]).unwrap();
        let mut fx = Dynamics::new(&p, 48_000);
        let mut b = sine(48_000, 1000.0, 0.01, 0.5);
        run(&mut fx, &mut b);
        assert!((peak(&b[b.len() / 2..]) / 0.01 - 2.0).abs() < 0.05);
    }

    #[test]
    fn the_limiter_never_passes_its_ceiling() {
        for detect in [0.0, 1.0] {
            for la in [0.0, 5.0, 20.0] {
                let p = params_from(
                    LIMITER,
                    &[
                        ("detect".into(), detect),
                        ("lookahead".into(), la),
                        ("input_boost".into(), 12.0),
                    ],
                )
                .unwrap();
                let mut fx = Limiter::new(&p, 48_000);
                let mut b = sine(48_000, 997.0, 0.9, 0.5);
                // A sudden spike too.
                b[20_000] = 1.0;
                run(&mut fx, &mut b);
                let ceil = db_to_gain(-6.0);
                assert!(peak(&b) <= ceil + 1e-6, "{detect} {la}: {}", peak(&b));
                // And it is not clipping: peaks touch the ceiling, but
                // no flat tops (a sample at the ceiling with its
                // neighbours there too).
                let at = |i: usize| b[i].abs() >= ceil - 1e-6;
                let flat = (2..b.len() - 2).filter(|&i| at(i) && at(i - 2) && at(i + 2)).count();
                // (With no look-ahead it can only catch a peak as it
                // comes, so the two samples either side of a top may both
                // touch.)
                if la > 0.0 {
                    assert!(flat < b.len() / 200, "{detect} {la}: {flat}");
                }
            }
        }
    }

    #[test]
    fn the_limiter_reports_its_look_ahead_and_lines_up() {
        let p = params_from(LIMITER, &[("lookahead".into(), 5.0), ("ceiling".into(), 0.0)]).unwrap();
        let mut fx = Limiter::new(&p, 48_000);
        assert_eq!(fx.latency(), 239);
        let mut b = vec![0.0f32; 2048];
        b[100] = 0.5;
        fx.process(&mut b);
        assert!((b[100 + 2 * 239] - 0.5).abs() < 1e-6);
        let p = params_from(LIMITER, &[("lookahead".into(), 5.0), ("detect".into(), 1.0)]).unwrap();
        assert_eq!(Limiter::new(&p, 48_000).latency(), 241);
        let mut fx = Limiter::new(&p, 48_000);
        let q = params_from(LIMITER, &[("lookahead".into(), 10.0)]).unwrap();
        assert!(!fx.update(&q), "new look-ahead needs a rebuild");
    }

    #[test]
    fn the_de_esser_turns_down_highs_only() {
        let p = params_from(DEESSER, &[("threshold".into(), -40.0), ("reduction".into(), 12.0)]).unwrap();
        let mut fx = DeEsser::new(&p, 48_000);
        let mut hi = sine(48_000, 9000.0, 0.5, 0.5);
        run(&mut fx, &mut hi);
        let cut = amp_db(peak(&hi[hi.len() / 2..]) / 0.5);
        assert!(cut < -9.0, "{cut}");
        let mut fx = DeEsser::new(&p, 48_000);
        let mut lo = sine(48_000, 200.0, 0.5, 0.5);
        run(&mut fx, &mut lo);
        let kept = amp_db(peak(&lo[lo.len() / 2..]) / 0.5);
        assert!(kept > -0.5, "{kept}");
    }

    #[test]
    fn gain_eases_to_its_level() {
        let p = params_from(GAIN, &[("gain".into(), -6.0)]).unwrap();
        let mut fx = Gain::new(&p, 48_000);
        let mut b = vec![1.0f32; 9600];
        fx.process(&mut b);
        assert!((b[9599] - db_to_gain(-6.0)).abs() < 1e-4);
        let q = params_from(GAIN, &[("gain".into(), 0.0)]).unwrap();
        assert!(fx.update(&q));
        let mut b = vec![1.0f32; 4];
        fx.process(&mut b);
        assert!(b[0] < 0.6, "eased, not a step: {}", b[0]);
    }
}
