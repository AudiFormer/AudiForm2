// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Tube Master: a valve mastering compressor for any number of channels,
//! written for AudiForm 2.
//!
//! One set of controls drives an identical copy of the circuit on every
//! channel that comes in: mono, stereo or a speaker bed (5.1 up to
//! 7.1.4). The controls:
//!
//! | key        | control     | what it does                                         |
//! |------------|-------------|------------------------------------------------------|
//! | `in`       | Input       | stepped drive into the valve stage (+0 to +12 dB)    |
//! | `thr`      | Threshold   | stepped, -30 to 0 dB                                 |
//! | `att`      | Attack      | stepped, slow (60 ms) to fast (1 ms)                 |
//! | `rel`      | Release     | stepped, slow to fast, program-dependent             |
//! | `ratio`    | Ratio       | 1.5:1 to 20:1, Limit, Negative                       |
//! | `out`      | Output      | make-up, -12 to +12 dB in whole dB                   |
//! | `sc`       | SC Filter   | high-passes the detector so the bass does not pump   |
//! | `detect`   | Detection   | Independent (each channel on its own) or Linked      |
//! | `ms`       | Stereo Mode | Left-Right or Mid-Side (stereo only)                 |
//! | `mix`      | Mix         | dry/wet, %                                           |
//! | `headroom` | Headroom    | how hot the valve runs, so how much colour it adds   |
//! | `fold`     | Mono Fold   | lows of each stereo pair summed to mono below this   |
//! | `gain`     | Gain        | final trim                                           |
//! | `sat`      | Saturator   | an extra soft clipper after the gain cell            |
//! | `shape`    | Shape       | the Saturator's drive and lean                       |
//! | `os`       | Oversampling| 2x or 4x for the nonlinear stages                    |
//! | `meter`    | (hidden)    | which [`TubeMeter`] the window reads                 |
//!
//! Detection: Independent lets every channel compress on its own (a loud
//! left does not turn the right down); Linked makes every channel follow
//! the loudest one, so the image does not shift. Stereo Mode Mid-Side
//! runs the two copies on the mid and side of a stereo pair; it is
//! ignored for any other channel count. Mono Fold works on stereo pairs
//! (L with R, Ls with Rs, the height pairs...) and leaves lone channels
//! (C, mono) alone. On a bed the LFE passes untouched, only delayed by
//! the latency so it stays in time.
//!
//! The signal path, per channel:
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
//! Its readings (each channel's input level and gain reduction, the
//! output's momentary loudness and peak) go to a [`TubeMeter`] the window
//! reads, picked by the hidden `meter` value (see [`meter`]).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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

/// How many meters there are to pick from (the hidden `meter` value).
pub const METERS: usize = 256;
/// The most channels a meter reports (7.1.4 has 12).
pub const MAX_CH: usize = 16;

pub fn info() -> EffectInfo {
    let circuit = "Circuit";
    let params = vec![
        choice("in", "Input", INPUT_NAMES, 0).group(circuit),
        choice("thr", "Threshold", THRESHOLD_NAMES, 6).group(circuit),
        choice("att", "Attack", SPEED_NAMES, 2).group(circuit),
        choice("rel", "Release", SPEED_NAMES, 2).group(circuit),
        choice("ratio", "Ratio", RATIO_NAMES, 1).group(circuit),
        num("out", "Output", "dB", -12.0, 12.0, 0.0, 1.0).group(circuit),
        toggle("sc", "SC Filter", false).group(circuit),
        choice("detect", "Detection", &["Independent", "Linked"], 0).group("Channels"),
        choice("ms", "Stereo Mode", &["Left-Right", "Mid-Side"], 0).group("Channels"),
        num("mix", "Mix", "%", 0.0, 100.0, 100.0, 1.0).group("Output"),
        num("headroom", "Headroom", "dB", 4.0, 24.0, 18.0, 1.0).group("Output"),
        num("fold", "Mono Fold", "Hz", 0.0, 200.0, 0.0, 5.0).group("Output"),
        num("gain", "Gain", "dB", -12.0, 12.0, 0.0, 0.1).group("Output"),
        toggle("sat", "Saturator", false).group("Output"),
        num("shape", "Shape", "%", 0.0, 100.0, 50.0, 1.0).group("Output"),
        choice("os", "Oversampling", &["2x", "4x"], 0).group("Output"),
        num("meter", "Meter", "", 0.0, (METERS - 1) as f64, 0.0, 1.0).hidden(),
    ];
    EffectInfo {
        id: ID,
        sidechain: true,
        name: "Tube Master",
        category: "Dynamics",
        description: "A valve mastering compressor for mono, stereo or a speaker bed: one set of stepped Input, Threshold, Attack, Release, Ratio and Output controls drives an identical circuit on every channel, compressing each on its own or all linked (optionally mid and side for stereo), with a smooth soft knee, program-dependent release, valve colour set by Headroom, Mono Fold for the lows, a Saturator, Mix and Gain. The LFE of a bed passes untouched.",
        params,
    }
}

// ------------------------------------------------------------- meters

/// What the window reads: how many channels the effect runs on (and
/// which of them pass untouched, the LFE), each channel's input level
/// (dBFS, RMS since the last reading) and gain reduction (dB, 0 or more),
/// and the output's momentary loudness (LUFS) and peak (dBFS). `beat`
/// counts readings, so the window can tell whether sound is running.
#[derive(Debug, Default)]
pub struct TubeMeter {
    channels: AtomicU32,
    passed: AtomicU32,
    ms: AtomicBool,
    input: [AtomicU32; MAX_CH],
    gr: [AtomicU32; MAX_CH],
    lufs: AtomicU32,
    peak: AtomicU32,
    beat: AtomicU32,
}

/// One set of readings from a [`TubeMeter`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TubeReading {
    /// Channels the effect runs on (0 before it was ever built).
    pub channels: usize,
    /// Bit `c` set: channel `c` passes untouched (the LFE).
    pub passed: u32,
    /// Running mid and side (a stereo pair in Mid-Side).
    pub ms: bool,
    pub input_db: [f32; MAX_CH],
    pub gr_db: [f32; MAX_CH],
    pub lufs: f32,
    pub peak_db: f32,
    pub beat: u32,
}

impl TubeReading {
    /// The channels worked on (not passed), up to [`MAX_CH`].
    pub fn worked(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.channels.min(MAX_CH)).filter(|c| self.passed & (1 << c) == 0)
    }

    /// The loudest worked channel's input level, dBFS.
    pub fn loudest_input(&self) -> f32 {
        self.worked()
            .map(|c| self.input_db[c])
            .fold(-120.0, f32::max)
    }

    /// The most gain reduction on any channel, dB.
    pub fn most_gr(&self) -> f32 {
        self.worked().map(|c| self.gr_db[c]).fold(0.0, f32::max)
    }
}

impl TubeMeter {
    /// The layout, stored when a processor is built (before any sound).
    fn layout(&self, channels: usize, passed: u32, ms: bool) {
        self.channels.store(channels as u32, Ordering::Relaxed);
        self.passed.store(passed, Ordering::Relaxed);
        self.ms.store(ms, Ordering::Relaxed);
    }

    fn store(&self, input_db: &[f32], gr_db: &[f32], lufs: f32, peak_db: f32) {
        for (k, (i, g)) in input_db.iter().zip(gr_db).take(MAX_CH).enumerate() {
            self.input[k].store(i.to_bits(), Ordering::Relaxed);
            self.gr[k].store(g.to_bits(), Ordering::Relaxed);
        }
        self.lufs.store(lufs.to_bits(), Ordering::Relaxed);
        self.peak.store(peak_db.to_bits(), Ordering::Relaxed);
        self.beat.fetch_add(1, Ordering::Relaxed);
    }

    pub fn read(&self) -> TubeReading {
        let f = |a: &AtomicU32| f32::from_bits(a.load(Ordering::Relaxed));
        TubeReading {
            channels: self.channels.load(Ordering::Relaxed) as usize,
            passed: self.passed.load(Ordering::Relaxed),
            ms: self.ms.load(Ordering::Relaxed),
            input_db: std::array::from_fn(|k| f(&self.input[k])),
            gr_db: std::array::from_fn(|k| f(&self.gr[k])),
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

/// One channel's copy of the circuit: its detector, gain computer,
/// oversampling filters, DC blocker, dry delay, Mono Fold crossover and
/// loudness filter. A passed channel (the LFE) uses only its delay.
struct Chan {
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
    /// Mono Fold's low-pass (two sections) then high-pass (two).
    fold: [Biquad; 4],
    k_weight: [Biquad; 2],
    delay: Vec<f32>,
    /// Input power since the last reading.
    power: f32,
}

impl Chan {
    fn reset(&mut self) {
        self.env = 0.0;
        self.gr = 0.0;
        self.memory = 0.0;
        self.dc_x = 0.0;
        self.dc_y = 0.0;
        self.power = 0.0;
        self.stage1.clear();
        self.stage2.clear();
        for b in self
            .sc_hp
            .iter_mut()
            .chain(&mut self.fold)
            .chain(&mut self.k_weight)
        {
            b.reset();
        }
        self.delay.fill(0.0);
    }
}

/// Tube Master over interleaved frames of any number of channels.
pub struct TubeMaster {
    rate: f32,
    four: bool,
    outer: Stage,
    inner: Stage,
    latency: usize,
    /// Channels in a frame.
    ch: usize,
    /// The channels worked on, and those passed untouched (the LFE).
    work: Vec<usize>,
    pass: Vec<usize>,
    /// Stereo pairs, for Mono Fold.
    pairs: Vec<[usize; 2]>,
    /// One stereo pair and nothing else: Mid-Side is possible.
    can_ms: bool,
    chans: Vec<Chan>,
    // Settings, shared by every channel's copy.
    gin: Smooth,
    thr: Smooth,
    slope: Smooth,
    out: Smooth,
    att: f32,
    rel_fast: f32,
    rel_slow: f32,
    sc: bool,
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
    delay_pos: usize,
    // One frame's working values, one per channel (made once).
    x: Vec<f32>,
    key: Vec<f32>,
    dry: Vec<f32>,
    level: Vec<f32>,
    // Metering.
    meter: usize,
    lufs_ring: [f32; 20],
    lufs_at: usize,
    lufs_acc: f32,
    lufs_n: usize,
    lufs_len: usize,
    peak: f32,
    power_n: usize,
    tick: usize,
    read_in: Vec<f32>,
    read_gr: Vec<f32>,
}

impl TubeMaster {
    /// The stereo form (interleaved left and right). A mono source fed to
    /// both sides comes out the same on both, as a mono run would.
    pub fn new(p: &Params, rate: u32) -> Self {
        Self::build(p, rate, &af2_dsp::wide::labels(2))
    }

    /// The form for channels named `names` (mono `M`, stereo `L R`, a
    /// speaker bed up to 7.1.4, or numbered channels): every channel but
    /// the LFE through its own identical copy of the circuit; the LFE
    /// passes untouched, only delayed to stay in time.
    pub fn wide(p: &Params, rate: u32, names: &[String]) -> Self {
        Self::build(p, rate, names)
    }

    fn build(p: &Params, rate: u32, names: &[String]) -> Self {
        let rate = rate.max(1);
        let r = rate as f32;
        let names: Vec<String> = if names.is_empty() {
            af2_dsp::wide::labels(1)
        } else {
            names.to_vec()
        };
        let ch = names.len();
        let work: Vec<usize> = (0..ch).filter(|&c| names[c] != "LFE").collect();
        let pass: Vec<usize> = (0..ch).filter(|&c| names[c] == "LFE").collect();
        let pairs: Vec<[usize; 2]> = af2_dsp::wide::groups(&names, true)
            .into_iter()
            .filter(|g| g.len() == 2)
            .map(|g| [g[0], g[1]])
            .collect();
        let can_ms = ch == 2 && pairs.len() == 1;
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
        let fc = fold_coeffs(rate, 100.0);
        let kw = [
            Biquad::new(Coeffs::new(Shape::HighShelf, rate, 1681.0, 0.707, 4.0)),
            Biquad::new(Coeffs::new(Shape::HighPass, rate, 38.0, 0.5, 0.0)),
        ];
        let chans = (0..ch)
            .map(|_| Chan {
                sc_hp: [Biquad::new(sc); 2],
                env: 0.0,
                gr: 0.0,
                memory: 0.0,
                dc_x: 0.0,
                dc_y: 0.0,
                stage1: StageMem::new(&outer),
                stage2: StageMem::new(&inner),
                fold: [
                    Biquad::new(fc[0]),
                    Biquad::new(fc[0]),
                    Biquad::new(fc[1]),
                    Biquad::new(fc[1]),
                ],
                k_weight: kw,
                delay: vec![0.0; latency.max(1)],
                power: 0.0,
            })
            .collect();
        let mut t = Self {
            rate: r,
            four,
            outer,
            inner,
            latency,
            ch,
            work,
            pass,
            pairs,
            can_ms,
            chans,
            gin: Smooth::default(),
            thr: Smooth::default(),
            slope: Smooth::default(),
            out: Smooth::default(),
            att: 0.0,
            rel_fast: 0.0,
            rel_slow: 0.0,
            sc: false,
            ms: false,
            linked: false,
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
            delay_pos: 0,
            x: vec![0.0; ch],
            key: vec![0.0; ch],
            dry: vec![0.0; ch],
            level: vec![0.0; ch],
            meter: 0,
            lufs_ring: [0.0; 20],
            lufs_at: 0,
            lufs_acc: 0.0,
            lufs_n: 0,
            lufs_len: ((0.02 * r) as usize).max(1),
            peak: 0.0,
            power_n: 0,
            tick: 0,
            read_in: vec![-120.0; ch],
            read_gr: vec![0.0; ch],
        };
        t.set(p);
        t.snap();
        t.publish_layout();
        t
    }

    /// Tells the window how many channels there are, which pass, and
    /// whether it runs mid and side.
    fn publish_layout(&self) {
        let passed = self
            .pass
            .iter()
            .filter(|&&c| c < 32)
            .fold(0u32, |m, &c| m | (1 << c));
        meter(self.meter).layout(self.ch, passed, self.ms_on());
    }

    /// Mid-Side asked for, on a stereo pair.
    fn ms_on(&self) -> bool {
        self.ms && self.can_ms
    }

    /// Takes new settings. False when the oversampling changed (the
    /// latency would change: build again).
    fn set(&mut self, p: &Params) -> bool {
        let at = |key: &str, n: usize| p.index(key).min(n - 1);
        self.gin.target = db_to_gain(f64::from(INPUT_DB[at("in", INPUT_DB.len())]));
        self.thr.target = THRESHOLD_DB[at("thr", THRESHOLD_DB.len())];
        let att = ATTACK_MS[at("att", ATTACK_MS.len())] / 1000.0;
        self.att = (-1.0 / (att * self.rate)).exp();
        let rel = RELEASE_S[at("rel", RELEASE_S.len())];
        self.rel_fast = (-1.0 / (rel * 0.35 * self.rate)).exp();
        self.rel_slow = (-1.0 / (rel * 2.0 * self.rate)).exp();
        self.slope.target = RATIO_SLOPE[at("ratio", RATIO_SLOPE.len())];
        self.out.target = db_to_gain(p.get("out").round());
        self.sc = p.on("sc");
        self.linked = p.index("detect") == 1;
        let ms = p.index("ms") == 1;
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
        let m = (p.get("meter").round().max(0.0) as usize).min(METERS - 1);
        let relayout = m != self.meter || ms != self.ms;
        self.meter = m;
        self.ms = ms;
        if relayout {
            self.publish_layout();
        }
        (p.index("os") == 1) == self.four
    }

    /// Jumps every smoothed value to its target (on build and reset).
    fn snap(&mut self) {
        for s in [
            &mut self.gin,
            &mut self.thr,
            &mut self.slope,
            &mut self.out,
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
        for ch in &mut self.chans {
            ch.fold[0].set(c[0]);
            ch.fold[1].set(c[0]);
            ch.fold[2].set(c[1]);
            ch.fold[3].set(c[1]);
        }
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

    #[cfg(test)]
    fn gr(&self, c: usize) -> f32 {
        self.chans[c].gr
    }

    /// Processes interleaved frames of `ch` channels. `key`, when given,
    /// drives the detectors in place of the input: one key channel per
    /// channel when it has as many, otherwise its frames summed to mono.
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
        let ch = self.ch;
        let frames = buf.len() / ch;
        let kw = key.map_or(0, |k| k.len() / frames.max(1));
        let ms = self.ms_on();
        let a = self.smooth_a;
        let lat = self.latency;
        for i in 0..frames {
            let f = &mut buf[i * ch..i * ch + ch];
            self.x.copy_from_slice(f);
            // Mono Fold: the lows of each pair summed to mono.
            if self.fold_hz > 0.0 {
                for &[l, r] in &self.pairs {
                    let (xl, xr) = (self.x[l], self.x[r]);
                    let (cl, cr) = pick2(&mut self.chans, l, r);
                    let lo = 0.5 * (pair(&mut cl.fold[..2], xl) + pair(&mut cr.fold[..2], xr));
                    self.x[l] = lo + pair(&mut cl.fold[2..], xl);
                    self.x[r] = lo + pair(&mut cr.fold[2..], xr);
                }
            }
            if ms {
                let (l, r) = (self.x[0], self.x[1]);
                self.x[0] = 0.5 * (l + r);
                self.x[1] = 0.5 * (l - r);
            }
            match key {
                Some(k) if kw == ch => {
                    self.key.copy_from_slice(&k[i * ch..i * ch + ch]);
                    if ms {
                        let (l, r) = (self.key[0], self.key[1]);
                        self.key[0] = 0.5 * (l + r);
                        self.key[1] = 0.5 * (l - r);
                    }
                }
                Some(k) if kw > 0 => {
                    let s = &k[i * kw..i * kw + kw];
                    let m = s.iter().sum::<f32>() / kw as f32;
                    self.key.fill(m);
                }
                Some(_) => self.key.fill(0.0),
                None => self.key.copy_from_slice(&self.x),
            }
            // The dry sound, delayed to line up with the oversampled path.
            if lat > 0 {
                let p = self.delay_pos;
                for (c, chan) in self.chans.iter_mut().enumerate() {
                    self.dry[c] = chan.delay[p];
                    chan.delay[p] = self.x[c];
                }
                self.delay_pos = if p + 1 == lat { 0 } else { p + 1 };
            } else {
                self.dry.copy_from_slice(&self.x);
            }
            let clip = self.clip.next(a);
            let mix = self.mix.next(a);
            let trim = self.trim.next(a);
            let sat_w = self.sat_w.next(a);
            let sat_k = self.sat_k.next(a);
            let sat_lean = self.sat_lean;
            let gin = self.gin.next(a);
            let thr = self.thr.next(a);
            let slope = self.slope.next(a);
            let out = self.out.next(a);
            // Detectors.
            let mut loudest = -120.0f32;
            for &c in &self.work {
                let chan = &mut self.chans[c];
                let mut d = self.key[c] * gin;
                let hp = pair(&mut chan.sc_hp, d);
                if self.sc {
                    d = hp;
                }
                let m = d.abs();
                chan.env = if m > chan.env {
                    chan.env + (m - chan.env) * self.env_rise
                } else {
                    chan.env * self.env_fall
                };
                let lv = 20.0 * chan.env.max(1e-6).log10();
                self.level[c] = lv;
                loudest = loudest.max(lv);
                chan.power += self.x[c] * self.x[c];
            }
            // Gain computers, then the oversampled valve, gain cell and
            // saturator: the same circuit on every channel.
            let inv_clip = 1.0 / clip;
            for &c in &self.work {
                let chan = &mut self.chans[c];
                let level = if self.linked { loudest } else { self.level[c] };
                let target = Self::curve(level - thr, slope).min(40.0);
                if target > chan.gr {
                    chan.gr = target + (chan.gr - target) * self.att;
                } else {
                    let coef = self.rel_fast + (self.rel_slow - self.rel_fast) * chan.memory;
                    chan.gr = target + (chan.gr - target) * coef;
                }
                let busy = if chan.gr > 2.0 { 1.0 } else { 0.0 };
                chan.memory += (busy - chan.memory)
                    * if busy > chan.memory {
                        self.mem_up
                    } else {
                        self.mem_down
                    };
                let g = 10f32.powf(-chan.gr / 20.0) * out;
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
                let up = chan.stage1.up(&self.outer, self.x[c]);
                let y = if self.four {
                    let a0 = chan.stage2.up(&self.inner, up[0]);
                    let a1 = chan.stage2.up(&self.inner, up[1]);
                    let d0 = chan.stage2.down(&self.inner, [shape(a0[0]), shape(a0[1])]);
                    let d1 = chan.stage2.down(&self.inner, [shape(a1[0]), shape(a1[1])]);
                    chan.stage1.down(&self.outer, [d0, d1])
                } else {
                    chan.stage1.down(&self.outer, [shape(up[0]), shape(up[1])])
                };
                // The lopsided valve leaves an offset: take it out.
                let yd = y - chan.dc_x + self.dc_r * chan.dc_y;
                chan.dc_x = y;
                chan.dc_y = if yd.abs() < 1e-20 { 0.0 } else { yd };
                // `x` now holds the wet sound.
                self.x[c] = yd;
            }
            if ms {
                let (m, s) = (self.x[0], self.x[1]);
                self.x[0] = m + s;
                self.x[1] = m - s;
                let (m, s) = (self.dry[0], self.dry[1]);
                self.dry[0] = m + s;
                self.dry[1] = m - s;
            }
            for &c in &self.work {
                let o = (self.dry[c] + (self.x[c] - self.dry[c]) * mix) * trim;
                f[c] = o;
                self.peak = self.peak.max(o.abs());
                let k = pair(&mut self.chans[c].k_weight, o);
                self.lufs_acc += k * k;
            }
            // The LFE: untouched, only in time.
            for &c in &self.pass {
                f[c] = self.dry[c];
                self.peak = self.peak.max(f[c].abs());
            }
            self.lufs_n += 1;
            if self.lufs_n >= self.lufs_len {
                self.lufs_ring[self.lufs_at] = self.lufs_acc / self.lufs_n as f32;
                self.lufs_at = (self.lufs_at + 1) % self.lufs_ring.len();
                self.lufs_acc = 0.0;
                self.lufs_n = 0;
            }
        }
        self.power_n += frames;
        self.tick += frames;
        if self.tick as f32 > self.rate / 60.0 {
            let n = self.power_n.max(1) as f32;
            for (c, chan) in self.chans.iter_mut().enumerate() {
                self.read_in[c] = 10.0 * (chan.power / n).max(1e-12).log10();
                self.read_gr[c] = chan.gr;
                chan.power = 0.0;
            }
            let ms: f32 = self.lufs_ring.iter().sum::<f32>() / self.lufs_ring.len() as f32;
            let lufs = -0.691 + 10.0 * ms.max(1e-12).log10();
            let peak = 20.0 * self.peak.max(1e-6).log10();
            meter(self.meter).store(&self.read_in, &self.read_gr, lufs, peak);
            self.power_n = 0;
            self.peak = 0.0;
            self.tick = 0;
        }
    }
}

/// Two different channels, both borrowed mutably.
fn pick2(chans: &mut [Chan], a: usize, b: usize) -> (&mut Chan, &mut Chan) {
    if a < b {
        let (lo, hi) = chans.split_at_mut(b);
        (&mut lo[a], &mut hi[0])
    } else {
        let (lo, hi) = chans.split_at_mut(a);
        (&mut hi[0], &mut lo[b])
    }
}

impl Processor for TubeMaster {
    fn process(&mut self, buf: &mut [f32]) {
        self.run(buf, None);
    }

    fn process_keyed(&mut self, buf: &mut [f32], key: &[f32]) {
        self.run(buf, Some(key));
    }

    fn latency(&self) -> usize {
        self.latency
    }

    fn reset(&mut self) {
        for c in &mut self.chans {
            c.reset();
        }
        self.lufs_ring = [0.0; 20];
        self.lufs_acc = 0.0;
        self.lufs_n = 0;
        self.peak = 0.0;
        self.snap();
    }

    fn update(&mut self, p: &Params) -> bool {
        self.set(p)
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

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    fn sines(frames: usize, fl: f32, fr: f32, amp: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let t = i as f32 / SR as f32;
                [amp * (TAU * fl * t).sin(), amp * (TAU * fr * t).sin()]
            })
            .collect()
    }

    /// `ch` channels of sines, channel `c` at `freq(c)` and `amp(c)`.
    fn bed(frames: usize, ch: usize, freq: fn(usize) -> f32, amp: fn(usize) -> f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                (0..ch).map(move |c| amp(c) * (TAU * freq(c) * i as f32 / SR as f32).sin())
            })
            .collect()
    }

    fn channel(buf: &[f32], c: usize, ch: usize) -> Vec<f32> {
        buf.iter().skip(c).step_by(ch).copied().collect()
    }

    fn run(fx: &mut TubeMaster, buf: &mut [f32]) {
        let ch = fx.ch;
        for b in buf.chunks_mut(256 * ch) {
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
            // Every form reports the same latency.
            for n in [1, 2, 6, 12] {
                let w = TubeMaster::wide(&p, SR, &af2_dsp::wide::labels(n));
                assert_eq!(w.latency(), lat, "{n} channels, os {os}");
            }
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
            let p = params(&[("os", os), ("thr", 10.0), ("headroom", 24.0)]);
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
        let p = params(&[("thr", 10.0), ("headroom", 24.0)]);
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
            let p = params(&[("thr", 4.0), ("ratio", 3.0)]);
            let mut fx = TubeMaster::new(&p, SR);
            let amp = 10f32.powf(amp_db / 20.0);
            let mut b = sines(48_000, 200.0, 200.0, amp);
            run(&mut fx, &mut b);
            let out = rms(&b[48_000..], 0, 2) * 2f32.sqrt();
            let g = 20.0 * (out / amp).log10();
            gains.push((amp_db, g, fx.gr(0)));
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
        let p = params(&[("thr", 2.0), ("att", att), ("rel", rel), ("ratio", 4.0)]);
        let mut fx = TubeMaster::new(&p, SR);
        let loud = sines(SR as usize * 2, 150.0, 150.0, 0.7);
        let mut grs = Vec::new();
        for c in loud.chunks(64) {
            let mut b = c.to_vec();
            fx.process(&mut b);
            grs.push(fx.gr(0));
        }
        let top = *grs.last().unwrap();
        let up = grs.iter().position(|g| *g > top * 0.66).unwrap() * 32;
        let quiet = vec![0.0f32; SR as usize * 8];
        let mut down = None;
        for (k, c) in quiet.chunks(64).enumerate() {
            let mut b = c.to_vec();
            fx.process(&mut b);
            if down.is_none() && fx.gr(0) < top * 0.33 {
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
            let p = params(&[("thr", 2.0), ("rel", 2.0), ("att", 4.0)]);
            let mut fx = TubeMaster::new(&p, SR);
            let mut b = sines((SR as f32 * secs) as usize, 150.0, 150.0, 0.7);
            run(&mut fx, &mut b);
            let top = fx.gr(0);
            let mut n = 0;
            while fx.gr(0) > top * 0.33 && n < SR as usize * 10 {
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
                    ("thr", 10.0),
                    ("ratio", 0.0),
                    ("headroom", hr),
                    ("in", input),
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
    fn mono_fold_leaves_lone_channels_alone() {
        // A mono run with Mono Fold on is the same as with it off.
        let input = bed(12_000, 1, |_| 60.0, |_| 0.5);
        let out = |fold: f64| {
            let p = params(&[("fold", fold), ("thr", 3.0)]);
            let mut fx = TubeMaster::wide(&p, SR, &names(&["M"]));
            let mut b = input.clone();
            run(&mut fx, &mut b);
            b
        };
        assert_eq!(out(0.0), out(150.0));
    }

    #[test]
    fn mid_side_round_trips() {
        // Dry: M/S in and out gives back the input, delayed.
        let p = params(&[("ms", 1.0), ("mix", 0.0)]);
        let mut fx = TubeMaster::new(&p, SR);
        let lat = fx.latency();
        let input = sines(6000, 440.0, 770.0, 0.5);
        let mut out = input.clone();
        run(&mut fx, &mut out);
        for i in 0..(6000 - lat) {
            for c in 0..2 {
                assert!((out[(i + lat) * 2 + c] - input[i * 2 + c]).abs() < 1e-6);
            }
        }
        // Wet but quiet: the same as Left-Right, to a hair.
        let quiet = sines(12_000, 440.0, 770.0, 0.01);
        let wet = |ms: f64| {
            let p = params(&[("ms", ms), ("thr", 10.0), ("headroom", 24.0)]);
            let mut fx = TubeMaster::new(&p, SR);
            let mut b = quiet.clone();
            run(&mut fx, &mut b);
            b
        };
        let (lr, msb) = (wet(0.0), wet(1.0));
        let err = lr
            .iter()
            .zip(&msb)
            .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(err < 1e-4, "{err}");
        // Working, a mono input stays mono (the side copy has nothing).
        let p = params(&[("ms", 1.0), ("thr", 2.0), ("headroom", 6.0), ("in", 3.0)]);
        let mut fx = TubeMaster::new(&p, SR);
        let mut b = sines(12_000, 300.0, 300.0, 0.6);
        run(&mut fx, &mut b);
        assert!(b.chunks(2).all(|f| (f[0] - f[1]).abs() < 1e-5));
        assert!(fx.gr(0) > 3.0 && fx.gr(1) < 0.01);
    }

    #[test]
    fn mid_side_is_ignored_off_stereo() {
        let input = bed(
            6000,
            6,
            |c| 100.0 + 70.0 * c as f32,
            |c| 0.2 + 0.1 * c as f32,
        );
        let out = |ms: f64| {
            let p = params(&[("ms", ms), ("thr", 3.0)]);
            let mut fx = TubeMaster::wide(&p, SR, &af2_dsp::wide::labels(6));
            let mut b = input.clone();
            run(&mut fx, &mut b);
            b
        };
        assert_eq!(out(0.0), out(1.0));
    }

    #[test]
    fn independent_channels_match_a_mono_run() {
        // Each channel at its own level and pitch, so each compresses by
        // its own amount; every one must come out as a mono run of it.
        let frames = 24_000;
        let freq = |c: usize| 90.0 + 55.0 * c as f32;
        let amp = |c: usize| [0.9, 0.01, 0.5, 0.3, 0.7, 0.15][c];
        let input = bed(frames, 6, freq, amp);
        let p = params(&[("thr", 3.0), ("ratio", 4.0), ("in", 2.0), ("headroom", 8.0)]);
        let numbered = names(&["1", "2", "3", "4", "5", "6"]);
        for layout in [numbered, af2_dsp::wide::labels(6)] {
            let mut fx = TubeMaster::wide(&p, SR, &layout);
            let mut b = input.clone();
            run(&mut fx, &mut b);
            for c in 0..6 {
                let got = channel(&b, c, 6);
                if layout[c] == "LFE" {
                    continue;
                }
                let mut mono = TubeMaster::wide(&p, SR, &names(&["M"]));
                let mut m = channel(&input, c, 6);
                run(&mut mono, &mut m);
                let err = got
                    .iter()
                    .zip(&m)
                    .fold(0.0f32, |e, (a, b)| e.max((a - b).abs()));
                assert!(err < 1e-6, "{layout:?} channel {c}: {err}");
                assert!((fx.gr(c) - mono.gr(0)).abs() < 1e-5);
            }
            // They really did compress by different amounts.
            assert!(fx.gr(0) > 6.0 && fx.gr(1) < 0.1, "{layout:?}");
        }
    }

    #[test]
    fn a_mono_source_on_the_stereo_build_matches_a_mono_run() {
        let p = params(&[("thr", 2.0), ("in", 3.0)]);
        let mono_in = bed(12_000, 1, |_| 220.0, |_| 0.6);
        let mut stereo: Vec<f32> = mono_in.iter().flat_map(|&v| [v, v]).collect();
        let mut fx = TubeMaster::new(&p, SR);
        run(&mut fx, &mut stereo);
        let mut mono = TubeMaster::wide(&p, SR, &names(&["M"]));
        let mut m = mono_in.clone();
        run(&mut mono, &mut m);
        for (f, v) in stereo.chunks(2).zip(&m) {
            assert_eq!(f[0], f[1]);
            assert!((f[0] - v).abs() < 1e-6);
        }
    }

    #[test]
    fn linked_moves_every_channel_together() {
        // One loud channel, the rest quiet.
        let amp = |c: usize| if c == 2 { 0.8 } else { 0.03 };
        let input = bed(SR as usize, 8, |c| 120.0 + 40.0 * c as f32, amp);
        let grs = |detect: f64| {
            let p = params(&[("thr", 3.0), ("detect", detect)]);
            let mut fx =
                TubeMaster::wide(&p, SR, &names(&["1", "2", "3", "4", "5", "6", "7", "8"]));
            let mut b = input.clone();
            run(&mut fx, &mut b);
            (0..8).map(|c| fx.gr(c)).collect::<Vec<f32>>()
        };
        let free = grs(0.0);
        let linked = grs(1.0);
        assert!(free[2] > 3.0, "{free:?}");
        assert!(
            free.iter().enumerate().all(|(c, g)| c == 2 || *g < 0.1),
            "{free:?}"
        );
        assert!(
            linked.iter().all(|g| (g - linked[2]).abs() < 1e-5),
            "{linked:?}"
        );
        assert!((linked[2] - free[2]).abs() < 1e-4, "{linked:?} {free:?}");
        // Stereo too: a loud left turns the right down only when linked.
        let lr: Vec<f32> = (0..SR as usize)
            .flat_map(|i| {
                let s = (TAU * 200.0 * i as f32 / SR as f32).sin();
                [0.8 * s, 0.03 * s]
            })
            .collect();
        for (detect, right_works) in [(0.0, false), (1.0, true)] {
            let mut fx = TubeMaster::new(&params(&[("thr", 3.0), ("detect", detect)]), SR);
            let mut b = lr.clone();
            run(&mut fx, &mut b);
            assert!(fx.gr(0) > 3.0);
            assert_eq!(fx.gr(1) > 3.0, right_works, "detect {detect}");
        }
    }

    #[test]
    fn the_key_drives_the_detectors() {
        let input = sines(SR as usize, 200.0, 200.0, 0.5);
        let p = params(&[("thr", 0.0)]);
        // A silent key leaves it alone.
        let mut fx = TubeMaster::new(&p, SR);
        let mut b = input.clone();
        let z = vec![0.0f32; 512];
        for c in b.chunks_mut(512) {
            let n = c.len();
            fx.process_keyed(c, &z[..n]);
        }
        assert!(fx.gr(0) < 0.01 && fx.gr(1) < 0.01);
        // A loud stereo key on a quiet bed squeezes it.
        let quiet = bed(SR as usize, 6, |c| 100.0 + 30.0 * c as f32, |_| 0.01);
        let mut fx = TubeMaster::wide(&p, SR, &af2_dsp::wide::labels(6));
        let mut b = quiet.clone();
        for (k, c) in b.chunks_mut(6 * 256).enumerate() {
            let n = c.len() / 6;
            fx.process_keyed(c, &input[k * 512..k * 512 + n * 2]);
        }
        assert!(fx.gr(0) > 5.0 && fx.gr(5) > 5.0);
        assert_eq!(fx.gr(3), 0.0);
    }

    #[test]
    fn saturator_and_gain_work() {
        let base = [("thr", 10.0), ("headroom", 24.0)];
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
        let p = params(&[("thr", 2.0), ("meter", 201.0)]);
        let mut fx = TubeMaster::new(&p, SR);
        let r = meter(201).read();
        assert_eq!((r.channels, r.passed, r.ms), (2, 0, false));
        let before = r.beat;
        let mut b = sines(SR as usize, 200.0, 200.0, 0.5);
        run(&mut fx, &mut b);
        let r = meter(201).read();
        assert!(r.beat > before);
        assert!(r.gr_db[0] > 3.0, "{r:?}");
        assert!((r.input_db[0] - (-9.03)).abs() < 0.5, "{r:?}");
        assert!((r.most_gr() - r.gr_db[0]).abs() < 1e-6);
        assert!(r.peak_db < 0.0 && r.peak_db > -20.0, "{r:?}");
        assert!(r.lufs < 0.0 && r.lufs > -30.0, "{r:?}");
        // A 5.1 bed: six channels, the LFE marked as passed.
        let p = params(&[("thr", 2.0), ("meter", 202.0)]);
        let mut fx = TubeMaster::wide(&p, SR, &af2_dsp::wide::labels(6));
        let r = meter(202).read();
        assert_eq!((r.channels, r.passed), (6, 1 << 3));
        let mut b = bed(
            SR as usize / 2,
            6,
            |c| 100.0 + 50.0 * c as f32,
            |c| 0.1 * (c + 1) as f32,
        );
        run(&mut fx, &mut b);
        let r = meter(202).read();
        assert_eq!(r.worked().collect::<Vec<_>>(), vec![0, 1, 2, 4, 5]);
        assert!(r.gr_db[5] > r.gr_db[0], "{r:?}");
        assert!((r.most_gr() - r.gr_db[5]).abs() < 1e-6, "{r:?}");
        assert!((r.loudest_input() - r.input_db[5]).abs() < 1e-6, "{r:?}");
    }

    #[test]
    fn bed_form_keeps_channels_and_leaves_the_lfe() {
        for n in [6usize, 8, 10, 12] {
            let names = af2_dsp::wide::labels(n);
            let lfe = names.iter().position(|s| s == "LFE").unwrap();
            let p = params(&[("thr", 2.0), ("headroom", 6.0), ("fold", 120.0)]);
            let mut fx = TubeMaster::wide(&p, SR, &names);
            let lat = fx.latency();
            let frames = 4800;
            let input = bed(frames, n, |c| 100.0 + 50.0 * c as f32, |_| 0.5);
            let mut b = input.clone();
            for c in b.chunks_mut(n * 300) {
                fx.process(c);
            }
            assert_eq!(b.len(), input.len());
            assert!(b.iter().all(|v| v.is_finite()));
            for i in 0..frames {
                let want = if i < lat {
                    0.0
                } else {
                    input[(i - lat) * n + lfe]
                };
                assert_eq!(
                    b[i * n + lfe].to_bits(),
                    want.to_bits(),
                    "{n} ch, frame {i}"
                );
            }
            // Everything else was worked on.
            for c in (0..n).filter(|&c| c != lfe) {
                let moved = (0..frames - lat)
                    .any(|i| (b[(i + lat) * n + c] - input[i * n + c]).abs() > 0.01);
                assert!(moved, "{n} ch, channel {c}");
            }
        }
    }

    #[test]
    fn oversampling_change_asks_for_a_rebuild() {
        let mut fx = TubeMaster::new(&params(&[]), SR);
        assert!(fx.update(&params(&[("mix", 50.0)])));
        assert!(!fx.update(&params(&[("os", 1.0)])));
    }
}
