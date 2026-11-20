// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Tube Master: a stereo valve mastering compressor, written for
//! AudiForm 2.
//!
//! Two channel rows (left and right, or mid and side), each with a
//! stepped Input (drive into the valve stage), Threshold, Attack,
//! Release (program-dependent), Ratio and Output, a Channel In switch, a
//! sidechain high-pass (so the bass does not pump the gain) and a meter
//! that shows its input level or its gain reduction. Ctrl Link makes the
//! second row follow the first; the detectors run on their own (Dual
//! Mono) or linked. Below: Mix, Headroom (how hot the valve stage runs,
//! so how much harmonic colour it adds), Mono Fold (lows below a
//! frequency summed to mono through a Linkwitz-Riley crossover), Gain and
//! an extra Saturator with its Shape.
//!
//! The signal path, per channel row:
//!
//! ```text
//! fold -> [M/S] -> up -> valve(Input, Headroom) -> gain cell -> Output
//!       -> Saturator -> down -> DC block -> [L/R] -> Mix -> Gain
//! ```
//!
//! The valve stage and the saturator are nonlinear, so they run at two or
//! four times the sample rate between linear-phase low-pass filters; the
//! dry path is delayed to match, and the delay is reported as latency.
//! The gain cell is a feed-forward compressor with a wide soft knee whose
//! release slows down the longer it has been working.
//!
//! Its readings (each row's input level and gain reduction, the output's
//! momentary loudness and peak) go to a [`TubeMeter`] the window reads,
//! picked by the hidden `meter` value (see [`meter`]).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

use super::{choice, db_to_gain, num, toggle, EffectInfo, Params, Processor};
use crate::biquad::{Biquad, Coeffs, Shape};

pub const ID: &str = "af.tube_master";

/// Input steps: drive into the valve stage, in dB.
pub const INPUT_DB: [f32; 5] = [0.0, 3.0, 6.0, 9.0, 12.0];
/// Threshold steps, in dBFS of the driven signal.
pub const THRESHOLD_DB: [f32; 11] = [
    -30.0, -27.0, -24.0, -21.0, -18.0, -15.0, -12.0, -9.0, -6.0, -3.0, 0.0,
];
/// Attack steps, slow to fast, in milliseconds.
pub const ATTACK_MS: [f32; 5] = [60.0, 30.0, 12.0, 4.0, 1.0];
/// Release steps, slow to fast, in seconds (the middle of the
/// program-dependent range).
pub const RELEASE_S: [f32; 5] = [1.2, 0.6, 0.3, 0.15, 0.08];
/// The slope above the knee for each Ratio step (1 - 1/ratio): 1 holds
/// the level (Limit), more than 1 turns louder input down (Negative).
pub const RATIO_SLOPE: [f32; 9] = [
    1.0 - 1.0 / 1.5,
    1.0 - 1.0 / 2.0,
    1.0 - 1.0 / 3.0,
    1.0 - 1.0 / 4.0,
    1.0 - 1.0 / 6.0,
    1.0 - 1.0 / 10.0,
    1.0 - 1.0 / 20.0,
    1.0,
    1.25,
];

const INPUT_NAMES: &[&str] = &["1", "2", "3", "4", "5"];
const THRESHOLD_NAMES: &[&str] = &[
    "-30 dB", "-27 dB", "-24 dB", "-21 dB", "-18 dB", "-15 dB", "-12 dB", "-9 dB", "-6 dB",
    "-3 dB", "0 dB",
];
const SPEED_NAMES: &[&str] = &["Slow", "Medium-slow", "Medium", "Medium-fast", "Fast"];
const RATIO_NAMES: &[&str] = &[
    "1.5:1", "2:1", "3:1", "4:1", "6:1", "10:1", "20:1", "Limit", "Negative",
];

/// Each row's control keys: Input, Threshold, Attack, Release, Ratio,
/// Output, Channel In, SC filter, VU mode.
pub const ROW_KEYS: [[&str; 9]; 2] = [
    [
        "in1", "thr1", "att1", "rel1", "ratio1", "out1", "on1", "sc1", "vu1",
    ],
    [
        "in2", "thr2", "att2", "rel2", "ratio2", "out2", "on2", "sc2", "vu2",
    ],
];

/// How many meters there are to pick from (the hidden `meter` value).
pub const METERS: usize = 256;

fn row(k: usize, g: &'static str) -> Vec<super::ParamInfo> {
    let keys = ROW_KEYS[k];
    vec![
        choice(keys[0], "Input", INPUT_NAMES, 0).group(g),
        choice(keys[1], "Threshold", THRESHOLD_NAMES, 6).group(g),
        choice(keys[2], "Attack", SPEED_NAMES, 2).group(g),
        choice(keys[3], "Release", SPEED_NAMES, 2).group(g),
        choice(keys[4], "Ratio", RATIO_NAMES, 1).group(g),
        num(keys[5], "Output", "dB", -12.0, 12.0, 0.0, 1.0).group(g),
        toggle(keys[6], "Channel In", true).group(g),
        toggle(keys[7], "SC Filter", false).group(g),
        choice(keys[8], "VU", &["Input", "Reduction"], 1).group(g),
    ]
}

pub fn info() -> EffectInfo {
    let mut params = row(0, "Channel 1");
    params.extend(row(1, "Channel 2"));
    params.extend([
        toggle("link", "Ctrl Link", true).group("Stereo"),
        choice("ms", "Mode", &["Left-Right", "Mid-Side"], 0).group("Stereo"),
        choice("detect", "Detectors", &["Dual Mono", "Sidechain Link"], 1).group("Stereo"),
        num("mix", "Mix", "%", 0.0, 100.0, 100.0, 1.0).group("Output"),
        num("headroom", "Headroom", "dB", 4.0, 24.0, 18.0, 1.0).group("Output"),
        num("fold", "Mono Fold", "Hz", 0.0, 200.0, 0.0, 5.0).group("Output"),
        num("gain", "Gain", "dB", -12.0, 12.0, 0.0, 0.1).group("Output"),
        toggle("sat", "Saturator", false).group("Output"),
        num("shape", "Shape", "%", 0.0, 100.0, 50.0, 1.0).group("Output"),
        choice("os", "Oversampling", &["2x", "4x"], 0).group("Output"),
        num("meter", "Meter", "", 0.0, (METERS - 1) as f64, 0.0, 1.0).hidden(),
    ]);
    EffectInfo {
        id: ID,
        sidechain: true,
        name: "Tube Master",
        category: "Dynamics",
        description: "A stereo valve mastering compressor: two channel rows (left and right, or mid and side) with stepped Input, Threshold, Attack, Release, Ratio and Output, a smooth soft knee and program-dependent release, valve colour set by Headroom, Mono Fold for the lows, a Saturator, Mix and Gain.",
        params,
    }
}

// ------------------------------------------------------------- meters

/// What the window reads: each row's input level (dBFS, RMS since the
/// last reading) and gain reduction (dB, 0 or more), and the output's
/// momentary loudness (LUFS) and peak (dBFS). `beat` counts readings,
/// so the window can tell whether sound is running.
#[derive(Debug, Default)]
pub struct TubeMeter {
    input: [AtomicU32; 2],
    gr: [AtomicU32; 2],
    lufs: AtomicU32,
    peak: AtomicU32,
    beat: AtomicU32,
}

/// One set of readings from a [`TubeMeter`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TubeReading {
    pub input_db: [f32; 2],
    pub gr_db: [f32; 2],
    pub lufs: f32,
    pub peak_db: f32,
    pub beat: u32,
}

impl TubeMeter {
    fn store(&self, input_db: [f32; 2], gr_db: [f32; 2], lufs: f32, peak_db: f32) {
        for k in 0..2 {
            self.input[k].store(input_db[k].to_bits(), Ordering::Relaxed);
            self.gr[k].store(gr_db[k].to_bits(), Ordering::Relaxed);
        }
        self.lufs.store(lufs.to_bits(), Ordering::Relaxed);
        self.peak.store(peak_db.to_bits(), Ordering::Relaxed);
        self.beat.fetch_add(1, Ordering::Relaxed);
    }

    pub fn read(&self) -> TubeReading {
        let f = |a: &AtomicU32| f32::from_bits(a.load(Ordering::Relaxed));
        TubeReading {
            input_db: [f(&self.input[0]), f(&self.input[1])],
            gr_db: [f(&self.gr[0]), f(&self.gr[1])],
            lufs: f(&self.lufs),
            peak_db: f(&self.peak),
            beat: self.beat.load(Ordering::Relaxed),
        }
    }
}

/// Meter `idx` (the slot's hidden `meter` value; 0 is a spare nobody
/// reads). Lock-free once made; the first call makes them all, so call
/// it off the audio thread first (building a processor does).
pub fn meter(idx: usize) -> &'static TubeMeter {
    static ALL: OnceLock<Box<[TubeMeter]>> = OnceLock::new();
    let all = ALL.get_or_init(|| (0..METERS).map(|_| TubeMeter::default()).collect());
    &all[idx.min(METERS - 1)]
}

// ------------------------------------------------------- oversampling

/// A Kaiser-windowed sinc low-pass of `n` taps (odd) with cutoff `fc`
/// (a fraction of the rate it runs at), scaled by `gain`.
fn lowpass_taps(n: usize, fc: f64, beta: f64, gain: f64) -> Vec<f32> {
    fn i0(x: f64) -> f64 {
        let mut sum = 1.0;
        let mut term = 1.0;
        let q = x * x / 4.0;
        for k in 1..64 {
            term *= q / (k * k) as f64;
            sum += term;
            if term < sum * 1e-16 {
                break;
            }
        }
        sum
    }
    let m = (n - 1) as f64 / 2.0;
    let norm = i0(beta);
    let raw: Vec<f64> = (0..n)
        .map(|k| {
            let t = k as f64 - m;
            let sinc = if t == 0.0 {
                2.0 * fc
            } else {
                (std::f64::consts::TAU * fc * t).sin() / (std::f64::consts::PI * t)
            };
            let r = t / m;
            sinc * i0(beta * (1.0 - r * r).max(0.0).sqrt()) / norm
        })
        .collect();
    let dc: f64 = raw.iter().sum();
    raw.iter().map(|h| (h / dc * gain) as f32).collect()
}

/// One halving/doubling stage's filter, split for the polyphase upsampler.
struct Stage {
    /// Every tap (for the downsampler).
    full: Vec<f32>,
    /// Even and odd taps, doubled (for the upsampler).
    even: Vec<f32>,
    odd: Vec<f32>,
}

impl Stage {
    fn new(n: usize, fc: f64) -> Self {
        let full = lowpass_taps(n, fc, 9.0, 1.0);
        let even = full.iter().step_by(2).map(|h| h * 2.0).collect();
        let odd = full.iter().skip(1).step_by(2).map(|h| h * 2.0).collect();
        Self { full, even, odd }
    }

    /// Delay of an up and down pass, in samples at the lower rate.
    fn delay(&self) -> usize {
        (self.full.len() - 1) / 2
    }
}

/// A history read newest first without wrapping: every sample is written
/// twice, `len` apart.
struct History {
    buf: Vec<f32>,
    len: usize,
    pos: usize,
}

impl History {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len * 2],
            len,
            pos: 0,
        }
    }
    #[inline]
    fn push(&mut self, x: f32) {
        self.pos = if self.pos == 0 {
            self.len - 1
        } else {
            self.pos - 1
        };
        self.buf[self.pos] = x;
        self.buf[self.pos + self.len] = x;
    }
    #[inline]
    fn dot(&self, taps: &[f32]) -> f32 {
        self.buf[self.pos..self.pos + taps.len()]
            .iter()
            .zip(taps)
            .map(|(a, b)| a * b)
            .sum()
    }
    fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

/// One channel's filter memories for a stage.
struct StageMem {
    up: History,
    down: History,
}

impl StageMem {
    fn new(s: &Stage) -> Self {
        Self {
            up: History::new(s.even.len()),
            down: History::new(s.full.len()),
        }
    }
    /// One sample in, two out at the doubled rate.
    #[inline]
    fn up(&mut self, s: &Stage, x: f32) -> [f32; 2] {
        self.up.push(x);
        [self.up.dot(&s.even), self.up.dot(&s.odd)]
    }
    /// Two samples at the doubled rate in, one out.
    #[inline]
    fn down(&mut self, s: &Stage, y: [f32; 2]) -> f32 {
        self.down.push(y[0]);
        let out = self.down.dot(&s.full);
        self.down.push(y[1]);
        out
    }
    fn clear(&mut self) {
        self.up.clear();
        self.down.clear();
    }
}

/// Taps for the outer stage (the sample rate and twice it) and the inner
/// one (twice and four times).
const OUTER_TAPS: usize = 95;
const INNER_TAPS: usize = 33;

// ---------------------------------------------------------- the curves

/// The valve stage, for a signal scaled so 1 is where it starts to
/// flatten: a lopsided grid (so even harmonics), a soft top and a firmer
/// bottom (odd harmonics), unity gain for small signals.
#[inline]
fn valve(u: f32) -> f32 {
    // Held under 3 smoothly, so the lopsided term never turns over.
    let u = u / (1.0 + u * u * (1.0 / 9.0)).sqrt();
    let a = u + 0.06 * u * u;
    if a >= 0.0 {
        a / (1.0 + a * a).sqrt()
    } else {
        a / (1.0 + 1.5 * a * a).sqrt()
    }
}

/// The Saturator: a cubic soft clip with a little lean, `k` the drive.
#[inline]
fn saturate(x: f32, k: f32, lean: f32) -> f32 {
    let a = (k * x).clamp(-1.5, 1.5);
    let a = a + lean * a * a * (1.0 / 1.5);
    let a = a.clamp(-1.5, 1.5);
    (a - (4.0 / 27.0) * a * a * a) / k
}

// ------------------------------------------------------------ smoothing

#[derive(Clone, Copy, Default)]
struct Smooth {
    cur: f32,
    target: f32,
}

impl Smooth {
    fn snap(v: f32) -> Self {
        Self { cur: v, target: v }
    }
    #[inline]
    fn next(&mut self, a: f32) -> f32 {
        self.cur += (self.target - self.cur) * a;
        self.cur
    }
}

/// One channel row's settings and memories.
struct Row {
    gin: Smooth,
    thr: Smooth,
    slope: Smooth,
    out: Smooth,
    on: Smooth,
    att: f32,
    rel_fast: f32,
    rel_slow: f32,
    sc: bool,
    sc_hp: [Biquad; 2],
    /// Peak follower on the detector signal.
    env: f32,
    /// Gain reduction, dB.
    gr: f32,
    /// How long it has been working, 0..1 (slows the release).
    memory: f32,
    dc_x: f32,
    dc_y: f32,
    stage1: StageMem,
    stage2: StageMem,
    power: f32,
}

/// The stereo processor (one per speaker pair on a bed).
struct Core {
    rate: f32,
    four: bool,
    outer: Stage,
    inner: Stage,
    latency: usize,
    rows: [Row; 2],
    ms: bool,
    linked: bool,
    clip: Smooth,
    mix: Smooth,
    trim: Smooth,
    sat_w: Smooth,
    sat_k: Smooth,
    sat_lean: f32,
    smooth_a: f32,
    env_rise: f32,
    env_fall: f32,
    mem_up: f32,
    mem_down: f32,
    dc_r: f32,
    fold_target: f32,
    fold_hz: f32,
    fold: [[Biquad; 4]; 2],
    delay: [Vec<f32>; 2],
    delay_pos: usize,
    // Metering.
    meter: usize,
    metering: bool,
    k_weight: [[Biquad; 2]; 2],
    lufs_ring: [f32; 20],
    lufs_at: usize,
    lufs_acc: f32,
    lufs_n: usize,
    lufs_len: usize,
    peak: f32,
    power_n: usize,
    tick: usize,
}

/// Two sections in a row.
#[inline]
fn pair(b: &mut [Biquad], x: f32) -> f32 {
    let y = b[0].run(x);
    b[1].run(y)
}

fn fold_coeffs(rate: u32, hz: f32) -> [Coeffs; 2] {
    let q = std::f64::consts::FRAC_1_SQRT_2;
    let f = f64::from(hz.max(10.0));
    [
        Coeffs::new(Shape::LowPass, rate, f, q, 0.0),
        Coeffs::new(Shape::HighPass, rate, f, q, 0.0),
    ]
}

impl Core {
    fn new(p: &Params, rate: u32) -> Self {
        let r = rate as f32;
        let four = p.index("os") == 1;
        let outer = Stage::new(OUTER_TAPS, 0.235);
        let inner = Stage::new(INNER_TAPS, 0.25);
        let latency = outer.delay() + if four { inner.delay() / 2 } else { 0 };
        let sc = Coeffs::new(
            Shape::HighPass,
            rate,
            90.0,
            std::f64::consts::FRAC_1_SQRT_2,
            0.0,
        );
        let mk_row = || Row {
            gin: Smooth::default(),
            thr: Smooth::default(),
            slope: Smooth::default(),
            out: Smooth::default(),
            on: Smooth::default(),
            att: 0.0,
            rel_fast: 0.0,
            rel_slow: 0.0,
            sc: false,
            sc_hp: [Biquad::new(sc); 2],
            env: 0.0,
            gr: 0.0,
            memory: 0.0,
            dc_x: 0.0,
            dc_y: 0.0,
            stage1: StageMem::new(&outer),
            stage2: StageMem::new(&inner),
            power: 0.0,
        };
        let fc = fold_coeffs(rate, 100.0);
        let fold_bank = [
            Biquad::new(fc[0]),
            Biquad::new(fc[0]),
            Biquad::new(fc[1]),
            Biquad::new(fc[1]),
        ];
        let kw = [
            Biquad::new(Coeffs::new(Shape::HighShelf, rate, 1681.0, 0.707, 4.0)),
            Biquad::new(Coeffs::new(Shape::HighPass, rate, 38.0, 0.5, 0.0)),
        ];
        let mut c = Self {
            rate: r,
            four,
            rows: [mk_row(), mk_row()],
            outer,
            inner,
            latency,
            ms: false,
            linked: true,
            clip: Smooth::default(),
            mix: Smooth::default(),
            trim: Smooth::default(),
            sat_w: Smooth::default(),
            sat_k: Smooth::default(),
            sat_lean: 0.0,
            smooth_a: 1.0 - (-1.0 / (0.02 * r)).exp(),
            env_rise: 1.0 - (-1.0 / (0.0003 * r)).exp(),
            env_fall: (-1.0 / (0.04 * r)).exp(),
            mem_up: 1.0 - (-1.0 / (2.0 * r)).exp(),
            mem_down: 1.0 - (-1.0 / (5.0 * r)).exp(),
            dc_r: (-std::f32::consts::TAU * 5.0 / r).exp(),
            fold_target: 0.0,
            fold_hz: 0.0,
            fold: [fold_bank; 2],
            delay: [vec![0.0; latency.max(1)], vec![0.0; latency.max(1)]],
            delay_pos: 0,
            meter: 0,
            metering: true,
            k_weight: [kw; 2],
            lufs_ring: [0.0; 20],
            lufs_at: 0,
            lufs_acc: 0.0,
            lufs_n: 0,
            lufs_len: ((0.02 * r) as usize).max(1),
            peak: 0.0,
            power_n: 0,
            tick: 0,
        };
        let _ = meter(0);
        c.set(p);
        c.snap();
        c
    }

    /// Takes new settings. False when the oversampling changed (the
    /// latency would change: build again).
    fn set(&mut self, p: &Params) -> bool {
        let link = p.on("link");
        for (k, row) in self.rows.iter_mut().enumerate() {
            // With Ctrl Link the second row follows the first's knobs.
            let src = if link { 0 } else { k };
            let kk = ROW_KEYS[src];
            let at = |key: &str, n: usize| p.index(key).min(n - 1);
            row.gin.target = db_to_gain(f64::from(INPUT_DB[at(kk[0], INPUT_DB.len())]));
            row.thr.target = THRESHOLD_DB[at(kk[1], THRESHOLD_DB.len())];
            let att = ATTACK_MS[at(kk[2], ATTACK_MS.len())] / 1000.0;
            row.att = (-1.0 / (att * self.rate)).exp();
            let rel = RELEASE_S[at(kk[3], RELEASE_S.len())];
            row.rel_fast = (-1.0 / (rel * 0.35 * self.rate)).exp();
            row.rel_slow = (-1.0 / (rel * 2.0 * self.rate)).exp();
            row.slope.target = RATIO_SLOPE[at(kk[4], RATIO_SLOPE.len())];
            row.out.target = db_to_gain(p.get(kk[5]).round());
            row.sc = p.on(kk[7]);
            // Channel In and the meter mode are each row's own.
            row.on.target = if p.on(ROW_KEYS[k][6]) { 1.0 } else { 0.0 };
        }
        self.ms = p.index("ms") == 1;
        self.linked = p.index("detect") == 1;
        // The valve starts to flatten `headroom` dB above -18 dBFS.
        self.clip.target = db_to_gain(p.get("headroom") - 18.0);
        self.mix.target = (p.get("mix") / 100.0).clamp(0.0, 1.0) as f32;
        self.trim.target = db_to_gain(p.get("gain"));
        let shape = (p.get("shape") / 100.0).clamp(0.0, 1.0) as f32;
        self.sat_w.target = if p.on("sat") { 1.0 } else { 0.0 };
        self.sat_k.target = 1.0 + 5.0 * shape * shape;
        self.sat_lean = 0.05 + 0.3 * shape;
        let fold = p.get("fold") as f32;
        self.fold_target = if fold >= 10.0 { fold } else { 0.0 };
        self.meter = (p.get("meter").round().max(0.0) as usize).min(METERS - 1);
        (p.index("os") == 1) == self.four
    }

    /// Jumps every smoothed value to its target (on build and reset).
    fn snap(&mut self) {
        for r in &mut self.rows {
            for s in [&mut r.gin, &mut r.thr, &mut r.slope, &mut r.out, &mut r.on] {
                *s = Smooth::snap(s.target);
            }
        }
        for s in [
            &mut self.clip,
            &mut self.mix,
            &mut self.trim,
            &mut self.sat_w,
            &mut self.sat_k,
        ] {
            *s = Smooth::snap(s.target);
        }
        self.fold_hz = self.fold_target;
        self.retune_fold();
    }

    fn retune_fold(&mut self) {
        if self.fold_hz <= 0.0 {
            return;
        }
        let c = fold_coeffs(self.rate as u32, self.fold_hz);
        for ch in &mut self.fold {
            ch[0].set(c[0]);
            ch[1].set(c[0]);
            ch[2].set(c[1]);
            ch[3].set(c[1]);
        }
    }

    fn reset(&mut self) {
        for r in &mut self.rows {
            r.env = 0.0;
            r.gr = 0.0;
            r.memory = 0.0;
            r.dc_x = 0.0;
            r.dc_y = 0.0;
            r.power = 0.0;
            r.stage1.clear();
            r.stage2.clear();
            for b in &mut r.sc_hp {
                b.reset();
            }
        }
        for ch in &mut self.fold {
            for b in ch {
                b.reset();
            }
        }
        for ch in &mut self.k_weight {
            for b in ch {
                b.reset();
            }
        }
        for d in &mut self.delay {
            d.fill(0.0);
        }
        self.lufs_ring = [0.0; 20];
        self.lufs_acc = 0.0;
        self.lufs_n = 0;
        self.peak = 0.0;
        self.snap();
    }

    /// Gain reduction (dB) wanted for a level `over` dB above threshold:
    /// a 10 dB soft knee into `slope`.
    #[inline]
    fn curve(over: f32, slope: f32) -> f32 {
        const W: f32 = 10.0;
        if over <= -W / 2.0 {
            0.0
        } else if over >= W / 2.0 {
            over * slope
        } else {
            let t = over + W / 2.0;
            slope * t * t / (2.0 * W)
        }
    }

    /// Processes interleaved stereo; `key`, when given, drives the
    /// detectors in place of the input.
    fn run(&mut self, buf: &mut [f32], key: Option<&[f32]>) {
        // The Mono Fold frequency glides to a new setting.
        if self.fold_hz != self.fold_target {
            if self.fold_target <= 0.0 || self.fold_hz <= 0.0 {
                self.fold_hz = self.fold_target;
            } else {
                let d = self.fold_target - self.fold_hz;
                self.fold_hz = if d.abs() < 0.5 {
                    self.fold_target
                } else {
                    self.fold_hz + d * 0.3
                };
            }
            self.retune_fold();
        }
        let a = self.smooth_a;
        let lat = self.latency;
        for (i, f) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let (mut l, mut r) = (f[0], f[1]);
            if self.fold_hz > 0.0 {
                let [fl, fr] = &mut self.fold;
                let lo_l = pair(&mut fl[..2], l);
                let lo_r = pair(&mut fr[..2], r);
                let hi_l = pair(&mut fl[2..], l);
                let hi_r = pair(&mut fr[2..], r);
                let lo = 0.5 * (lo_l + lo_r);
                l = lo + hi_l;
                r = lo + hi_r;
            }
            let x = if self.ms {
                [0.5 * (l + r), 0.5 * (l - r)]
            } else {
                [l, r]
            };
            let k = match key {
                Some(k) => {
                    let (kl, kr) = (
                        k.get(i * 2).copied().unwrap_or(0.0),
                        k.get(i * 2 + 1).copied().unwrap_or(0.0),
                    );
                    if self.ms {
                        [0.5 * (kl + kr), 0.5 * (kl - kr)]
                    } else {
                        [kl, kr]
                    }
                }
                None => x,
            };
            // The dry sound, delayed to line up with the oversampled path.
            let dry = if lat > 0 {
                let p = self.delay_pos;
                let d = [self.delay[0][p], self.delay[1][p]];
                self.delay[0][p] = x[0];
                self.delay[1][p] = x[1];
                self.delay_pos = if p + 1 == lat { 0 } else { p + 1 };
                d
            } else {
                x
            };
            let clip = self.clip.next(a);
            let mix = self.mix.next(a);
            let trim = self.trim.next(a);
            let sat_w = self.sat_w.next(a);
            let sat_k = self.sat_k.next(a);
            let sat_lean = self.sat_lean;
            // Detectors and gain computers.
            let mut want = [0.0f32; 2];
            let mut level = [0.0f32; 2];
            for (c, row) in self.rows.iter_mut().enumerate() {
                let gin = row.gin.next(a);
                row.thr.next(a);
                row.slope.next(a);
                let mut d = k[c] * gin;
                let hp = pair(&mut row.sc_hp, d);
                if row.sc {
                    d = hp;
                }
                let m = d.abs();
                row.env = if m > row.env {
                    row.env + (m - row.env) * self.env_rise
                } else {
                    row.env * self.env_fall
                };
                level[c] = 20.0 * row.env.max(1e-6).log10();
                row.power += x[c] * x[c];
            }
            if self.linked {
                let lv = level[0].max(level[1]);
                level = [lv, lv];
            }
            for (c, row) in self.rows.iter_mut().enumerate() {
                let target = Self::curve(level[c] - row.thr.cur, row.slope.cur).min(40.0);
                if target > row.gr {
                    row.gr = target + (row.gr - target) * row.att;
                } else {
                    let coef = row.rel_fast + (row.rel_slow - row.rel_fast) * row.memory;
                    row.gr = target + (row.gr - target) * coef;
                }
                let busy = if row.gr > 2.0 { 1.0 } else { 0.0 };
                row.memory += (busy - row.memory)
                    * if busy > row.memory {
                        self.mem_up
                    } else {
                        self.mem_down
                    };
                want[c] = row.gr;
            }
            if self.linked {
                let g = want[0].max(want[1]);
                want = [g, g];
            }
            // The oversampled valve, gain cell and saturator.
            let mut wet = [0.0f32; 2];
            for (c, row) in self.rows.iter_mut().enumerate() {
                let gin = row.gin.cur;
                let out = row.out.next(a);
                let on = row.on.next(a);
                let g = 10f32.powf(-want[c] / 20.0) * out;
                let inv_clip = 1.0 / clip;
                let shape = |v: f32| -> f32 {
                    // Driven into the valve, then brought back down, so
                    // Input changes the colour and the squeeze, not the level.
                    let y = clip * valve(v * gin * inv_clip) / gin * g;
                    if sat_w > 0.0 {
                        y + (saturate(y, sat_k, sat_lean) - y) * sat_w
                    } else {
                        y
                    }
                };
                let up = row.stage1.up(&self.outer, x[c]);
                let y = if self.four {
                    let a0 = row.stage2.up(&self.inner, up[0]);
                    let a1 = row.stage2.up(&self.inner, up[1]);
                    let d0 = row.stage2.down(&self.inner, [shape(a0[0]), shape(a0[1])]);
                    let d1 = row.stage2.down(&self.inner, [shape(a1[0]), shape(a1[1])]);
                    row.stage1.down(&self.outer, [d0, d1])
                } else {
                    row.stage1.down(&self.outer, [shape(up[0]), shape(up[1])])
                };
                // The lopsided valve leaves an offset: take it out.
                let yd = y - row.dc_x + self.dc_r * row.dc_y;
                row.dc_x = y;
                row.dc_y = if yd.abs() < 1e-20 { 0.0 } else { yd };
                wet[c] = dry[c] + (yd - dry[c]) * on;
            }
            let (wl, wr, dl, dr) = if self.ms {
                (
                    wet[0] + wet[1],
                    wet[0] - wet[1],
                    dry[0] + dry[1],
                    dry[0] - dry[1],
                )
            } else {
                (wet[0], wet[1], dry[0], dry[1])
            };
            let ol = (dl + (wl - dl) * mix) * trim;
            let or = (dr + (wr - dr) * mix) * trim;
            f[0] = ol;
            f[1] = or;
            if self.metering {
                self.peak = self.peak.max(ol.abs()).max(or.abs());
                let kl = pair(&mut self.k_weight[0], ol);
                let kr = pair(&mut self.k_weight[1], or);
                self.lufs_acc += kl * kl + kr * kr;
                self.lufs_n += 1;
                if self.lufs_n >= self.lufs_len {
                    self.lufs_ring[self.lufs_at] = self.lufs_acc / self.lufs_n as f32;
                    self.lufs_at = (self.lufs_at + 1) % self.lufs_ring.len();
                    self.lufs_acc = 0.0;
                    self.lufs_n = 0;
                }
            }
        }
        if self.metering {
            let frames = buf.len() / 2;
            self.power_n += frames;
            self.tick += frames;
            if self.tick as f32 > self.rate / 60.0 {
                let n = self.power_n.max(1) as f32;
                let db = |p: f32| 10.0 * (p / n).max(1e-12).log10();
                let input = [db(self.rows[0].power), db(self.rows[1].power)];
                let gr = [self.rows[0].gr, self.rows[1].gr];
                let ms: f32 = self.lufs_ring.iter().sum::<f32>() / self.lufs_ring.len() as f32;
                let lufs = -0.691 + 10.0 * ms.max(1e-12).log10();
                let peak = 20.0 * self.peak.max(1e-6).log10();
                meter(self.meter).store(input, gr, lufs, peak);
                for r in &mut self.rows {
                    r.power = 0.0;
                }
                self.power_n = 0;
                self.peak = 0.0;
                self.tick = 0;
            }
        }
    }
}

// ------------------------------------------------------------ the effect

/// A bed's layout: the speaker groups (pairs, lone speakers), each with
/// its own [`Core`], and the channels passed through (the LFE), held back
/// by the latency.
struct Bed {
    channels: usize,
    groups: Vec<Vec<usize>>,
    cores: Vec<Core>,
    passed: Vec<usize>,
    ring: Vec<f32>,
    ring_at: usize,
    scratch: Vec<f32>,
    key: Vec<f32>,
}

/// Frames a bed processes at a time.
const BED_BLOCK: usize = 256;

pub struct TubeMaster {
    core: Core,
    bed: Option<Bed>,
}

impl TubeMaster {
    pub fn new(p: &Params, rate: u32) -> Self {
        Self {
            core: Core::new(p, rate),
            bed: None,
        }
    }

    /// The form for a speaker bed of channels named `names`: each pair
    /// (and lone speaker) through its own copy, every copy keyed by the
    /// loudest channel of the bed so they all move together; the LFE
    /// passes untouched (only delayed to stay in time).
    pub fn wide(p: &Params, rate: u32, names: &[String]) -> Self {
        let groups = af27_dsp::wide::groups(names, true);
        let channels = names.len().max(1);
        let mut cores: Vec<Core> = groups.iter().map(|_| Core::new(p, rate)).collect();
        for c in cores.iter_mut().skip(1) {
            // The front pair alone feeds the window's meters.
            c.metering = false;
        }
        let passed: Vec<usize> = (0..channels)
            .filter(|c| !groups.iter().any(|g| g.contains(c)))
            .collect();
        let core = Core::new(p, rate);
        let lat = core.latency.max(1);
        Self {
            core,
            bed: Some(Bed {
                channels,
                groups,
                cores,
                ring: vec![0.0; lat * passed.len()],
                passed,
                ring_at: 0,
                scratch: vec![0.0; BED_BLOCK * 2],
                key: vec![0.0; BED_BLOCK * 2],
            }),
        }
    }

    #[cfg(test)]
    fn gr(&self) -> [f32; 2] {
        [self.core.rows[0].gr, self.core.rows[1].gr]
    }

    fn run_bed(bed: &mut Bed, lat: usize, buf: &mut [f32]) {
        let ch = bed.channels;
        for part in buf.chunks_mut(BED_BLOCK * ch) {
            let n = part.len() / ch;
            // The key: the loudest channel the effect works on.
            for i in 0..n {
                let fr = &part[i * ch..i * ch + ch];
                let m = bed
                    .groups
                    .iter()
                    .flatten()
                    .fold(0.0f32, |m, &c| m.max(fr[c].abs()));
                bed.key[i * 2] = m;
                bed.key[i * 2 + 1] = m;
            }
            for (g, core) in bed.groups.iter().zip(&mut bed.cores) {
                let (a, b) = (g[0], g.get(1).copied().unwrap_or(g[0]));
                for i in 0..n {
                    bed.scratch[i * 2] = part[i * ch + a];
                    bed.scratch[i * 2 + 1] = part[i * ch + b];
                }
                core.run(&mut bed.scratch[..n * 2], Some(&bed.key[..n * 2]));
                for i in 0..n {
                    if a == b {
                        part[i * ch + a] = 0.5 * (bed.scratch[i * 2] + bed.scratch[i * 2 + 1]);
                    } else {
                        part[i * ch + a] = bed.scratch[i * 2];
                        part[i * ch + b] = bed.scratch[i * 2 + 1];
                    }
                }
            }
            if lat > 0 && !bed.passed.is_empty() {
                let np = bed.passed.len();
                for i in 0..n {
                    for (j, &c) in bed.passed.iter().enumerate() {
                        let slot = bed.ring_at * np + j;
                        std::mem::swap(&mut bed.ring[slot], &mut part[i * ch + c]);
                    }
                    bed.ring_at = (bed.ring_at + 1) % lat;
                }
            }
        }
    }
}

impl Processor for TubeMaster {
    fn process(&mut self, buf: &mut [f32]) {
        match &mut self.bed {
            Some(bed) => Self::run_bed(bed, self.core.latency, buf),
            None => self.core.run(buf, None),
        }
    }

    fn process_keyed(&mut self, buf: &mut [f32], key: &[f32]) {
        match &mut self.bed {
            Some(bed) => Self::run_bed(bed, self.core.latency, buf),
            None => self.core.run(buf, Some(key)),
        }
    }

    fn latency(&self) -> usize {
        self.core.latency
    }

    fn reset(&mut self) {
        self.core.reset();
        if let Some(bed) = &mut self.bed {
            for c in &mut bed.cores {
                c.reset();
            }
            bed.ring.fill(0.0);
        }
    }

    fn update(&mut self, p: &Params) -> bool {
        let mut same = self.core.set(p);
        if let Some(bed) = &mut self.bed {
            for c in &mut bed.cores {
                same &= c.set(p);
            }
        }
        same
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params_from;
    use std::f32::consts::TAU;

    const SR: u32 = 48_000;

    fn params(set: &[(&str, f64)]) -> Params {
        let pairs: Vec<(String, f64)> = set.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        params_from(ID, &pairs).unwrap()
    }

    fn sines(frames: usize, fl: f32, fr: f32, amp: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let t = i as f32 / SR as f32;
                [amp * (TAU * fl * t).sin(), amp * (TAU * fr * t).sin()]
            })
            .collect()
    }

    fn run(fx: &mut TubeMaster, buf: &mut [f32]) {
        for b in buf.chunks_mut(512) {
            fx.process(b);
        }
    }

    fn rms(buf: &[f32], ch: usize, chans: usize) -> f32 {
        let v: Vec<f32> = buf.iter().skip(ch).step_by(chans).copied().collect();
        (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
    }

    /// Amplitude of frequency `hz` in one channel (Goertzel, Hann window).
    fn tone(buf: &[f32], ch: usize, hz: f32) -> f32 {
        let v: Vec<f32> = buf.iter().skip(ch).step_by(2).copied().collect();
        let n = v.len();
        let w = TAU * hz / SR as f32;
        let (mut re, mut im) = (0.0f64, 0.0f64);
        let mut wsum = 0.0f64;
        for (i, x) in v.iter().enumerate() {
            let win = 0.5 - 0.5 * (TAU * i as f32 / n as f32).cos();
            let a = (w * i as f32) as f64;
            re += (x * win) as f64 * a.cos();
            im += (x * win) as f64 * a.sin();
            wsum += win as f64;
        }
        (2.0 * (re * re + im * im).sqrt() / wsum) as f32
    }

    #[test]
    fn latency_lines_up_and_mix_zero_is_the_input() {
        for os in [0.0, 1.0] {
            let p = params(&[("mix", 0.0), ("os", os)]);
            let mut fx = TubeMaster::new(&p, SR);
            let lat = fx.latency();
            assert!(lat > 0);
            let input = sines(8000, 440.0, 1000.0, 0.3);
            let mut out = input.clone();
            run(&mut fx, &mut out);
            for i in 0..(8000 - lat) {
                for c in 0..2 {
                    assert!((out[(i + lat) * 2 + c] - input[i * 2 + c]).abs() < 1e-6);
                }
            }
            // The wet path lines up too: an impulse at low level comes out
            // at the reported latency.
            let p = params(&[("os", os), ("thr1", 10.0), ("headroom", 24.0)]);
            let mut fx = TubeMaster::new(&p, SR);
            let mut imp = vec![0.0f32; 400];
            imp[20] = 0.01;
            imp[21] = 0.01;
            run(&mut fx, &mut imp);
            let at = (0..200)
                .max_by(|a, b| imp[a * 2].abs().total_cmp(&imp[b * 2].abs()))
                .unwrap();
            assert_eq!(at, 10 + lat, "os {os}");
        }
    }

    #[test]
    fn quiet_input_passes_near_unity_wet() {
        let p = params(&[("thr1", 10.0), ("headroom", 24.0)]);
        let mut fx = TubeMaster::new(&p, SR);
        let mut b = sines(24_000, 300.0, 500.0, 0.01);
        let inr = rms(&b, 0, 2);
        run(&mut fx, &mut b);
        let outr = rms(&b[24_000..], 0, 2);
        let db = 20.0 * (outr / inr).log10();
        assert!(db.abs() < 0.2, "{db} dB");
    }

    #[test]
    fn gain_reduction_rises_above_threshold() {
        let mut last = f32::INFINITY;
        let mut gains = Vec::new();
        for amp_db in [-40.0f32, -24.0, -12.0, -3.0] {
            let p = params(&[("thr1", 4.0), ("ratio1", 3.0)]);
            let mut fx = TubeMaster::new(&p, SR);
            let amp = 10f32.powf(amp_db / 20.0);
            let mut b = sines(48_000, 200.0, 200.0, amp);
            run(&mut fx, &mut b);
            let out = rms(&b[48_000..], 0, 2) * 2f32.sqrt();
            let g = 20.0 * (out / amp).log10();
            gains.push((amp_db, g, fx.gr()[0]));
            assert!(g <= last + 0.1, "{gains:?}");
            last = g;
        }
        assert!(gains[0].1.abs() < 0.5, "{gains:?}");
        assert!(gains[0].2 < 0.1, "{gains:?}");
        assert!(gains[3].2 > 6.0, "{gains:?}");
        assert!(gains[3].1 < -5.0, "{gains:?}");
    }

    /// Frames from a step up until the gain reduction is 2/3 of the way
    /// there, and after the step down until it is back under 1/3.
    fn timing(att: f64, rel: f64) -> (usize, usize) {
        let p = params(&[("thr1", 2.0), ("att1", att), ("rel1", rel), ("ratio1", 4.0)]);
        let mut fx = TubeMaster::new(&p, SR);
        let loud = sines(SR as usize * 2, 150.0, 150.0, 0.7);
        let mut grs = Vec::new();
        for c in loud.chunks(64) {
            let mut b = c.to_vec();
            fx.process(&mut b);
            grs.push(fx.gr()[0]);
        }
        let top = *grs.last().unwrap();
        let up = grs.iter().position(|g| *g > top * 0.66).unwrap() * 32;
        let quiet = vec![0.0f32; SR as usize * 8];
        let mut down = None;
        for (k, c) in quiet.chunks(64).enumerate() {
            let mut b = c.to_vec();
            fx.process(&mut b);
            if down.is_none() && fx.gr()[0] < top * 0.33 {
                down = Some(k * 32);
            }
        }
        (up, down.unwrap())
    }

    #[test]
    fn attack_and_release_steps_differ() {
        let (fast_up, fast_down) = timing(4.0, 4.0);
        let (slow_up, slow_down) = timing(0.0, 0.0);
        assert!(slow_up > fast_up * 5, "{slow_up} vs {fast_up}");
        assert!(slow_down > fast_down * 5, "{slow_down} vs {fast_down}");
        let (mid_up, mid_down) = timing(2.0, 2.0);
        assert!(fast_up < mid_up && mid_up < slow_up);
        assert!(fast_down < mid_down && mid_down < slow_down);
    }

    #[test]
    fn release_is_program_dependent() {
        // A long squeeze lets go more slowly than a short one.
        let rel_after = |secs: f32| {
            let p = params(&[("thr1", 2.0), ("rel1", 2.0), ("att1", 4.0)]);
            let mut fx = TubeMaster::new(&p, SR);
            let mut b = sines((SR as f32 * secs) as usize, 150.0, 150.0, 0.7);
            run(&mut fx, &mut b);
            let top = fx.gr()[0];
            let mut n = 0;
            while fx.gr()[0] > top * 0.33 && n < SR as usize * 10 {
                let mut z = [0.0f32; 128];
                fx.process(&mut z);
                n += 64;
            }
            n
        };
        let short = rel_after(0.2);
        let long = rel_after(6.0);
        assert!(long > short * 3 / 2, "{long} vs {short}");
    }

    #[test]
    fn drive_adds_harmonics_without_aliasing() {
        for os in [0.0, 1.0] {
            let colour = |hr: f64, input: f64| {
                let p = params(&[
                    ("os", os),
                    ("thr1", 10.0),
                    ("ratio1", 0.0),
                    ("headroom", hr),
                    ("in1", input),
                ]);
                let mut fx = TubeMaster::new(&p, SR);
                let mut b = sines(SR as usize, 5000.0, 5000.0, 0.25);
                run(&mut fx, &mut b);
                let tail = &b[SR as usize..];
                let f = tone(tail, 0, 5000.0);
                let rel = |hz: f32| 20.0 * (tone(tail, 0, hz) / f).log10();
                // Where the 6th to 9th harmonics would fold back without
                // oversampling, and where the 2x path's own folds land.
                let alias = [18_000.0, 13_000.0, 8000.0, 3000.0].map(rel);
                let inner = [6000.0, 11_000.0, 16_000.0, 1000.0].map(rel);
                (rel(10_000.0), rel(15_000.0), alias, inner)
            };
            let (h2c, h3c, _, _) = colour(24.0, 0.0);
            let (h2, h3, alias, inner) = colour(4.0, 2.0);
            assert!(h2 > -40.0 && h3 > -40.0, "os {os}: {h2} {h3}");
            assert!(
                h2 > h2c + 10.0 && h3 > h3c + 15.0,
                "os {os}: {h2c} {h3c} vs {h2} {h3}"
            );
            for a in alias {
                assert!(a < -75.0, "os {os}: alias {alias:?}");
            }
            if os == 1.0 {
                for a in inner {
                    assert!(a < -75.0, "4x: {inner:?}");
                }
            }
        }
    }

    #[test]
    fn mono_fold_makes_lows_mono_and_keeps_highs_wide() {
        let p = params(&[("mix", 0.0), ("fold", 150.0)]);
        let mut fx = TubeMaster::new(&p, SR);
        let n = SR as usize;
        let mut b: Vec<f32> = (0..n)
            .flat_map(|i| {
                let t = i as f32 / SR as f32;
                let lo = 0.4 * (TAU * 50.0 * t).sin();
                let hi = 0.3 * (TAU * 3000.0 * t).sin();
                [lo + hi, -lo - hi]
            })
            .collect();
        run(&mut fx, &mut b);
        let side: Vec<f32> = b[n..]
            .chunks(2)
            .flat_map(|f| [0.5 * (f[0] - f[1]), 0.0])
            .collect();
        let lo_side = tone(&side, 0, 50.0);
        let hi_side = tone(&side, 0, 3000.0);
        assert!(lo_side < 0.4 * 0.02, "lows left in the side: {lo_side}");
        assert!((hi_side - 0.3).abs() < 0.03, "highs stay wide: {hi_side}");
    }

    #[test]
    fn mid_side_round_trips() {
        // Both rows out: the wet path is the input, delayed.
        let p = params(&[("ms", 1.0), ("on1", 0.0), ("on2", 0.0)]);
        let mut fx = TubeMaster::new(&p, SR);
        let lat = fx.latency();
        let input = sines(6000, 440.0, 770.0, 0.5);
        let mut out = input.clone();
        run(&mut fx, &mut out);
        for i in 0..(6000 - lat) {
            for c in 0..2 {
                assert!((out[(i + lat) * 2 + c] - input[i * 2 + c]).abs() < 1e-5);
            }
        }
        // Working, a mono input stays mono (the side row has nothing).
        let p = params(&[("ms", 1.0), ("thr1", 2.0), ("headroom", 6.0), ("in1", 3.0)]);
        let mut fx = TubeMaster::new(&p, SR);
        let mut b = sines(12_000, 300.0, 300.0, 0.6);
        run(&mut fx, &mut b);
        assert!(b.chunks(2).all(|f| (f[0] - f[1]).abs() < 1e-5));
        assert!(fx.gr()[0] > 3.0);
    }

    #[test]
    fn dual_mono_and_link_behave() {
        // Loud left, quiet right.
        let input = sines(SR as usize, 200.0, 200.0, 0.0)
            .chunks(2)
            .enumerate()
            .flat_map(|(i, _)| {
                let s = (TAU * 200.0 * i as f32 / SR as f32).sin();
                [0.8 * s, 0.05 * s]
            })
            .collect::<Vec<f32>>();
        let grs = |detect: f64| {
            let p = params(&[("thr1", 3.0), ("detect", detect)]);
            let mut fx = TubeMaster::new(&p, SR);
            let mut b = input.clone();
            run(&mut fx, &mut b);
            fx.gr()
        };
        let dual = grs(0.0);
        let linked = grs(1.0);
        assert!(dual[0] > 3.0 && dual[1] < 0.1, "{dual:?}");
        assert!((linked[0] - linked[1]).abs() < 1e-4 && linked[1] > 3.0);
    }

    #[test]
    fn ctrl_link_and_the_key_drive_the_second_row() {
        let input = sines(SR as usize, 200.0, 200.0, 0.5);
        let gr2 = |set: &[(&str, f64)]| {
            let mut s = vec![("detect", 0.0)];
            s.extend_from_slice(set);
            let mut fx = TubeMaster::new(&params(&s), SR);
            let mut b = input.clone();
            run(&mut fx, &mut b);
            fx.gr()[1]
        };
        // Row 2 at 0 dB threshold does nothing, unless linked to row 1.
        let free = gr2(&[("link", 0.0), ("thr1", 0.0), ("thr2", 10.0)]);
        let tied = gr2(&[("link", 1.0), ("thr1", 0.0), ("thr2", 10.0)]);
        assert!(free < 0.5 && tied > 5.0, "{free} {tied}");
        // A silent key leaves it alone.
        let p = params(&[("thr1", 0.0)]);
        let mut fx = TubeMaster::new(&p, SR);
        let mut b = input.clone();
        let z = vec![0.0f32; 512];
        for c in b.chunks_mut(512) {
            let n = c.len();
            fx.process_keyed(c, &z[..n]);
        }
        assert!(fx.gr()[0] < 0.01);
    }

    #[test]
    fn saturator_and_gain_work() {
        let base = [("thr1", 10.0), ("headroom", 24.0)];
        let level = |extra: &[(&str, f64)]| {
            let mut s = base.to_vec();
            s.extend_from_slice(extra);
            let mut fx = TubeMaster::new(&params(&s), SR);
            let mut b = sines(SR as usize / 2, 1000.0, 1000.0, 0.5);
            run(&mut fx, &mut b);
            let tail = &b[SR as usize / 2..];
            (
                rms(tail, 0, 2),
                tone(tail, 0, 3000.0) / tone(tail, 0, 1000.0),
            )
        };
        let (plain, h_plain) = level(&[]);
        let (up, _) = level(&[("gain", 6.0)]);
        let (_, h_sat) = level(&[("sat", 1.0), ("shape", 100.0)]);
        assert!((20.0 * (up / plain).log10() - 6.0).abs() < 0.2);
        assert!(h_sat > h_plain * 10.0, "{h_sat} vs {h_plain}");
    }

    #[test]
    fn meters_report() {
        let p = params(&[("thr1", 2.0), ("meter", 201.0), ("vu1", 0.0)]);
        let mut fx = TubeMaster::new(&p, SR);
        let before = meter(201).read().beat;
        let mut b = sines(SR as usize, 200.0, 200.0, 0.5);
        run(&mut fx, &mut b);
        let r = meter(201).read();
        assert!(r.beat > before);
        assert!(r.gr_db[0] > 3.0, "{r:?}");
        assert!((r.input_db[0] - (-9.03)).abs() < 0.5, "{r:?}");
        assert!(r.peak_db < 0.0 && r.peak_db > -20.0, "{r:?}");
        assert!(r.lufs < 0.0 && r.lufs > -30.0, "{r:?}");
    }

    #[test]
    fn bed_form_keeps_channels_and_leaves_the_lfe() {
        let names = af27_dsp::wide::labels(12);
        let p = params(&[("thr1", 2.0), ("headroom", 6.0)]);
        let mut fx = TubeMaster::wide(&p, SR, &names);
        let lat = fx.latency();
        let frames = 4800;
        let input: Vec<f32> = (0..frames)
            .flat_map(|i| {
                (0..12).map(move |c| {
                    0.5 * (TAU * (100.0 + 50.0 * c as f32) * i as f32 / SR as f32).sin()
                })
            })
            .collect();
        let mut b = input.clone();
        for c in b.chunks_mut(12 * 300) {
            fx.process(c);
        }
        assert_eq!(b.len(), input.len());
        assert!(b.iter().all(|v| v.is_finite()));
        let lfe = 3;
        for i in 0..frames - lat {
            assert_eq!(b[(i + lat) * 12 + lfe], input[i * 12 + lfe]);
        }
        // Everything else was worked on.
        let moved = (0..frames - lat).any(|i| (b[(i + lat) * 12] - input[i * 12]).abs() > 0.01);
        assert!(moved);
    }

    #[test]
    fn oversampling_change_asks_for_a_rebuild() {
        let mut fx = TubeMaster::new(&params(&[]), SR);
        assert!(fx.update(&params(&[("mix", 50.0)])));
        assert!(!fx.update(&params(&[("os", 1.0)])));
    }
}
