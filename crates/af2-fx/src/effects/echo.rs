// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.
// Written for AudiForm 2. Only the control list (keys, ranges, defaults
// and pick lists) follows Audiform's Delay and Echo, so `.audfrm`
// sessions load into it; the sound code is our own. See docs/clean-room.md.

//! Delay and Echo: the sound again after a set time, each repeat fed
//! back for the next, in four kinds.
//!
//! - Digital: clean repeats, each a copy of the last (only Tone, Low cut
//!   and Drive colour them).
//! - Tape: the delay is read through a wobbling tape path (wow at the
//!   Rate, a quicker flutter on top), the loop saturates softly and every
//!   pass loses a little more top.
//! - Analog: a bucket-brigade feel: a darker loop (two poles) and a
//!   softer clip.
//! - Simple: no repeats. Each side is moved by its own time, early or
//!   late (up to half a second either way), with its own mix of moved and
//!   in-place sound and its own polarity. Early needs look-ahead, so the
//!   effect reports the earliest side's time as its latency and everything
//!   else is held back to match.
//!
//! The Analog dial (Tape and Analog, Off then 1 to 4) adds more of the
//! machine at each step: more saturation, a softer top and more of its
//! own noise riding on the repeats (hiss for tape, the darker hash of a
//! bucket brigade); step 4 adds a faint mains hum. Off is the clean form
//! of the kind.
//!
//! The time is in ms or, with Follow tempo, a note value at the slot's
//! tempo. Ping-pong sends each repeat to the other side (the input summed
//! into the left line first). Every control is smoothed: a new time
//! glides (as a tape machine would) rather than jumping, so moving it
//! never clicks.
//!
//! On a speaker bed every speaker has its own line ([`Echo::wide`]):
//! Ping-pong passes each repeat on to the next speaker round the room's
//! ring, Room spread lets each repeat leak into the speakers either side,
//! Simple moves the left-hand speakers by the left time and the
//! right-hand ones by the right time, and the LFE is left dry, held back
//! by the latency.
//!
//! Everything is allocated when the effect is built; processing never
//! allocates or locks.

use super::{choice, num, toggle, EffectInfo, Params, Processor};

pub const ID: &str = "af.echo";
/// The longest time.
pub const MAX_SECONDS: f64 = 4.0;
pub const KINDS: [&str; 4] = ["Digital", "Tape", "Analog", "Simple"];
/// Tape and Analog: how much of the machine comes with the kind.
pub const ANALOG: [&str; 5] = ["Off", "1", "2", "3", "4"];
/// Simple: how far either side can move, early or late.
pub const SIMPLE_MS: f64 = 500.0;
/// The note values a time that follows the tempo can take.
pub const NOTES: [&str; 16] = [
    "1/64", "1/32T", "1/32", "1/16T", "1/16", "1/16D", "1/8T", "1/8", "1/8D", "1/4T", "1/4",
    "1/4D", "1/2", "1/2D", "1 bar", "2 bars",
];
/// Each note's length in beats (quarter notes): T is two thirds, D one
/// and a half times the plain note.
pub const BEATS: [f64; 16] = [
    1.0 / 16.0,
    1.0 / 12.0,
    1.0 / 8.0,
    1.0 / 6.0,
    1.0 / 4.0,
    3.0 / 8.0,
    1.0 / 3.0,
    1.0 / 2.0,
    3.0 / 4.0,
    2.0 / 3.0,
    1.0,
    1.5,
    2.0,
    3.0,
    4.0,
    8.0,
];

pub const DIGITAL: usize = 0;
pub const TAPE: usize = 1;
pub const BBD: usize = 2;
pub const SIMPLE: usize = 3;

pub fn echo_info() -> EffectInfo {
    EffectInfo {
        id: ID,
        sidechain: false,
        name: "Delay and Echo",
        category: "Delay and Reverb",
        description: "Repeats that fade away: clean Digital, wobbly saturated Tape or dark Analog, with the time in ms or following the tempo, ping-pong between the sides, filters, modulation and drive on the repeats. Simple moves each side early or late by its own time, with its own mix and polarity, to line up or widen a sound.",
        params: vec![
            choice("kind", "Type", &KINDS, 0),
            num("time", "Delay time", "ms", 1.0, MAX_SECONDS * 1000.0, 350.0, 1.0)
                .log()
                .group("Time"),
            toggle("sync", "Follow tempo", false).group("Time"),
            choice("note", "Note", &NOTES, 7).group("Time"),
            num("tempo", "Tempo", "BPM", 30.0, 300.0, 120.0, 0.1).group("Time"),
            num("feedback", "Feedback", "%", 0.0, 120.0, 35.0, 1.0).group("Repeats"),
            toggle("pingpong", "Ping-pong", false).group("Repeats"),
            num("spread", "Room spread", "%", 0.0, 100.0, 50.0, 1.0).group("Repeats"),
            num("low_cut", "Low cut", "Hz", 20.0, 2000.0, 20.0, 1.0)
                .log()
                .group("Repeats"),
            num("tone", "Tone", "Hz", 500.0, 16_000.0, 6000.0, 10.0)
                .log()
                .group("Repeats"),
            num("depth", "Modulation", "%", 0.0, 100.0, 0.0, 1.0).group("Color"),
            num("rate", "Rate", "Hz", 0.05, 10.0, 0.6, 0.01)
                .log()
                .group("Color"),
            num("drive", "Drive", "%", 0.0, 100.0, 0.0, 1.0).group("Color"),
            choice("analog", "Analog", &ANALOG, 2).group("Color"),
            num("dry", "Dry", "%", 0.0, 100.0, 100.0, 1.0).group("Output"),
            num("wet", "Echo level", "%", 0.0, 100.0, 30.0, 1.0).group("Output"),
            num("l_time", "Left time", "ms", -SIMPLE_MS, SIMPLE_MS, 0.0, 0.01).group("Simple"),
            num("l_mix", "Left mix", "%", 0.0, 100.0, 100.0, 1.0).group("Simple"),
            toggle("l_invert", "Left invert", false).group("Simple"),
            num("r_time", "Right time", "ms", -SIMPLE_MS, SIMPLE_MS, 0.0, 0.01).group("Simple"),
            num("r_mix", "Right mix", "%", 0.0, 100.0, 100.0, 1.0).group("Simple"),
            toggle("r_invert", "Right invert", false).group("Simple"),
            choice("units", "Time in", &["ms", "Samples"], 0).group("Simple"),
        ],
    }
}

/// The delay time in ms for these settings: the time, or the note value
/// at the slot's tempo (kept within 1 ms to the longest time).
pub fn time_ms(p: &Params) -> f64 {
    let ms = if p.on("sync") {
        let beats = BEATS[p.index("note").min(BEATS.len() - 1)];
        beats * 60_000.0 / p.get("tempo").clamp(30.0, 300.0)
    } else {
        p.get("time")
    };
    ms.clamp(1.0, MAX_SECONDS * 1000.0)
}

/// The look-ahead Simple needs for these settings, in frames at `rate`:
/// the earliest side's time (zero for the other kinds).
pub fn latency_for(p: &Params, rate: u32) -> usize {
    if p.index("kind").min(SIMPLE) != SIMPLE {
        return 0;
    }
    let early = -(p.get("l_time").min(p.get("r_time"))).min(0.0);
    (early.min(SIMPLE_MS) / 1000.0 * f64::from(rate.max(1))).round() as usize
}

/// A value eased toward its target, a step per frame.
#[derive(Clone, Copy, Default)]
struct Smooth {
    cur: f32,
    to: f32,
}

impl Smooth {
    #[inline]
    fn next(&mut self, a: f32) -> f32 {
        let d = self.to - self.cur;
        if d.abs() < 1e-7 {
            self.cur = self.to;
        } else {
            self.cur += d * a;
        }
        self.cur
    }

    fn snap(&mut self) {
        self.cur = self.to;
    }
}

/// One-pole low-pass coefficient for a corner at `hz`.
fn coef(hz: f64, sr: f64) -> f32 {
    let hz = hz.clamp(1.0, sr * 0.45);
    (1.0 - (-std::f64::consts::TAU * hz / sr).exp()) as f32
}

#[inline]
fn flush(x: f32) -> f32 {
    if x.abs() < 1e-20 {
        0.0
    } else {
        x
    }
}

/// The loop's saturation: unity for small signals, rounding off toward
/// 1/k. Tape's is a tanh; the bucket brigade's a softer algebraic knee.
#[inline]
fn shape(x: f32, k: f32, bbd: bool) -> f32 {
    if k < 1e-3 {
        x
    } else if bbd {
        x / (1.0 + (k * x) * (k * x)).sqrt()
    } else {
        (k * x).tanh() / k
    }
}

/// Keeps a loop running hot (feedback over 100%) from growing without
/// bound: as is up to full scale, then bent toward twice that.
#[inline]
fn guard(x: f32) -> f32 {
    let a = x.abs();
    if a <= 1.0 {
        x
    } else {
        let t = a - 1.0;
        (1.0 + t / (1.0 + t)).copysign(x)
    }
}

#[inline]
fn hermite(xm1: f32, x0: f32, x1: f32, x2: f32, t: f32) -> f32 {
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * t + c2) * t + c1) * t + x0
}

/// One speaker's delay line and the state of its repeat's filters.
struct Voice {
    line: Vec<f32>,
    tone1: f32,
    tone2: f32,
    low: f32,
    dark1: f32,
    dark2: f32,
    noise_lp: f32,
    seed: u32,
    /// This frame's repeat.
    rep: f32,
    /// Simple: -1 a left-hand speaker, 1 a right-hand one, 0 neither.
    side: i8,
    /// Where its modulation starts in the turn (0..1).
    phase_off: f64,
}

impl Voice {
    fn new(len: usize, side: i8, k: usize, n: usize) -> Self {
        Self {
            line: vec![0.0; len],
            tone1: 0.0,
            tone2: 0.0,
            low: 0.0,
            dark1: 0.0,
            dark2: 0.0,
            noise_lp: 0.0,
            seed: 0x9e37_79b9 ^ (k as u32 + 1).wrapping_mul(0x85eb_ca6b),
            rep: 0.0,
            side,
            phase_off: k as f64 / n.max(1) as f64,
        }
    }

    /// The line `dist` frames behind index `w` (fractional).
    #[inline]
    fn tap(&self, w: usize, dist: f32) -> f32 {
        let len = self.line.len();
        let p = (w + len) as f64 - f64::from(dist.max(0.0));
        let i = p.floor();
        let t = (p - i) as f32;
        let i = i as usize;
        let at = |k: usize| self.line[k % len];
        if dist < 2.0 {
            at(i) + (at(i + 1) - at(i)) * t
        } else {
            hermite(at(i + len - 1), at(i), at(i + 1), at(i + 2), t)
        }
    }

    #[inline]
    fn white(&mut self) -> f32 {
        let mut s = self.seed;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.seed = s;
        (s as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    fn clear(&mut self) {
        self.line.fill(0.0);
        self.tone1 = 0.0;
        self.tone2 = 0.0;
        self.low = 0.0;
        self.dark1 = 0.0;
        self.dark2 = 0.0;
        self.noise_lp = 0.0;
        self.rep = 0.0;
    }
}

/// Simple's settings for one side.
#[derive(Clone, Copy, Default)]
struct Side {
    /// Its move in frames (negative early).
    time: Smooth,
    mix: Smooth,
    /// +1, or -1 with Invert (eased through zero, so flipping is quiet).
    pol: Smooth,
}

pub struct Echo {
    sr: f64,
    /// Channels in an interleaved frame.
    channels: usize,
    /// The channel each voice works on.
    chans: Vec<usize>,
    voices: Vec<Voice>,
    /// On a speaker bed (Ping-pong goes round the ring, Spread works).
    bed: bool,
    /// Channels left alone (the LFE), held back by the latency: a ring of
    /// `pass_len` frames of them.
    passed: Vec<usize>,
    pass: Vec<f32>,
    pass_len: usize,
    pass_at: usize,
    /// Where this frame is written in every line.
    w: usize,
    kind: usize,
    pingpong: bool,
    /// The latency the effect was built with.
    lat: usize,
    delay: Smooth,
    feedback: Smooth,
    dry: Smooth,
    wet: Smooth,
    depth: Smooth,
    flutter: Smooth,
    sat: Smooth,
    tone: Smooth,
    low: Smooth,
    dark: Smooth,
    dark2: Smooth,
    noise: Smooth,
    hum: Smooth,
    spread: Smooth,
    sides: [Side; 2],
    lfo: f64,
    lfo_step: f64,
    flutter_ph: f64,
    hum_ph: f64,
    /// Per-frame easing for the controls (about 20 ms) and the time
    /// (about 60 ms).
    a_fast: f32,
    a_time: f32,
}

impl Echo {
    /// A stereo Delay and Echo at `rate`.
    pub fn new(p: &Params, rate: u32) -> Self {
        Self::build(p, rate, 2, vec![0, 1], vec![-1, 1], Vec::new(), false)
    }

    /// A Delay and Echo over a speaker bed with channels named `names`
    /// (interleaved frames of `names.len()` channels).
    pub fn wide(p: &Params, rate: u32, names: &[String]) -> Self {
        let chans = af2_dsp::wide::ring(names);
        let sides = chans
            .iter()
            .map(|&c| {
                let n = names[c].as_str();
                if n.starts_with('L') {
                    -1
                } else if n.starts_with('R') {
                    1
                } else {
                    0
                }
            })
            .collect();
        let passed = (0..names.len()).filter(|c| !chans.contains(c)).collect();
        let bed = chans.len() > 2;
        Self::build(p, rate, names.len().max(1), chans, sides, passed, bed)
    }

    fn build(
        p: &Params,
        rate: u32,
        channels: usize,
        chans: Vec<usize>,
        sides: Vec<i8>,
        passed: Vec<usize>,
        bed: bool,
    ) -> Self {
        let sr = f64::from(rate.max(1));
        // The longest time, the widest wobble and room for the reader.
        let len = ((MAX_SECONDS + 0.02) * sr).ceil() as usize + 8;
        let n = chans.len();
        let voices = sides
            .iter()
            .enumerate()
            .map(|(k, &s)| Voice::new(len, s, k, n))
            .collect();
        let lat = latency_for(p, rate);
        let pass_len = lat + 1;
        let mut e = Self {
            sr,
            channels,
            chans,
            voices,
            bed,
            pass: vec![0.0; pass_len * passed.len()],
            passed,
            pass_len,
            pass_at: 0,
            w: 0,
            kind: 0,
            pingpong: false,
            lat,
            delay: Smooth::default(),
            feedback: Smooth::default(),
            dry: Smooth::default(),
            wet: Smooth::default(),
            depth: Smooth::default(),
            flutter: Smooth::default(),
            sat: Smooth::default(),
            tone: Smooth::default(),
            low: Smooth::default(),
            dark: Smooth::default(),
            dark2: Smooth::default(),
            noise: Smooth::default(),
            hum: Smooth::default(),
            spread: Smooth::default(),
            sides: [Side::default(); 2],
            lfo: 0.0,
            lfo_step: 0.0,
            flutter_ph: 0.0,
            hum_ph: 0.0,
            a_fast: (1.0 - (-1.0 / (0.02 * sr)).exp()) as f32,
            a_time: (1.0 - (-1.0 / (0.06 * sr)).exp()) as f32,
        };
        e.set(p);
        e.snap();
        e
    }

    fn snap(&mut self) {
        for s in [
            &mut self.delay,
            &mut self.feedback,
            &mut self.dry,
            &mut self.wet,
            &mut self.depth,
            &mut self.flutter,
            &mut self.sat,
            &mut self.tone,
            &mut self.low,
            &mut self.dark,
            &mut self.dark2,
            &mut self.noise,
            &mut self.hum,
            &mut self.spread,
        ] {
            s.snap();
        }
        for s in &mut self.sides {
            s.time.snap();
            s.mix.snap();
            s.pol.snap();
        }
    }

    fn ms(&self, ms: f64) -> f32 {
        (ms / 1000.0 * self.sr) as f32
    }

    /// Takes new settings as targets (eased in by `process`).
    fn set(&mut self, p: &Params) {
        let sr = self.sr;
        self.kind = p.index("kind").min(SIMPLE);
        self.pingpong = p.on("pingpong");
        let machine = self.kind == TAPE || self.kind == BBD;
        let a = if machine {
            p.index("analog").min(ANALOG.len() - 1) as f64
        } else {
            0.0
        };
        self.delay.to = self.ms(time_ms(p));
        self.feedback.to = (p.get("feedback") / 100.0).clamp(0.0, 1.2) as f32;
        self.dry.to = (p.get("dry") / 100.0).clamp(0.0, 1.0) as f32;
        self.wet.to = (p.get("wet") / 100.0).clamp(0.0, 1.0) as f32;
        let depth = (p.get("depth") / 100.0).clamp(0.0, 1.0);
        // The widest swing at full depth: tape wow is the deepest.
        let swing_ms = match self.kind {
            TAPE => 4.0,
            BBD => 3.0,
            _ => 2.0,
        };
        self.depth.to = self.ms(depth * swing_ms);
        self.flutter.to = if self.kind == TAPE {
            self.ms(0.04 + 0.03 * a)
        } else {
            0.0
        };
        self.lfo_step = p.get("rate").clamp(0.05, 10.0) / sr;
        let drive = (p.get("drive") / 100.0).clamp(0.0, 1.0);
        let base = match self.kind {
            TAPE => 0.7 + 0.4 * a,
            BBD => 0.9 + 0.5 * a,
            _ => 0.0,
        };
        self.sat.to = (base + 4.0 * drive) as f32;
        self.tone.to = coef(p.get("tone").clamp(500.0, 16_000.0), sr);
        self.low.to = coef(p.get("low_cut").clamp(20.0, 2000.0), sr);
        let (dark, dark2) = match self.kind {
            TAPE => (coef(9000.0 * (1.0 - 0.12 * a), sr), 1.0),
            BBD => {
                let c = coef(5200.0 * (1.0 - 0.14 * a), sr);
                (c, c)
            }
            _ => (1.0, 1.0),
        };
        self.dark.to = dark;
        self.dark2.to = dark2;
        let noise_db = [f64::NEG_INFINITY, -84.0, -76.0, -68.0, -60.0][a as usize];
        self.noise.to = if machine && a > 0.0 {
            10f64.powf(noise_db / 20.0) as f32
        } else {
            0.0
        };
        self.hum.to = if machine && a >= 4.0 {
            10f64.powf(-72.0 / 20.0) as f32
        } else {
            0.0
        };
        self.spread.to = (p.get("spread") / 100.0).clamp(0.0, 1.0) as f32;
        const KEYS: [[&str; 3]; 2] = [
            ["l_time", "l_mix", "l_invert"],
            ["r_time", "r_mix", "r_invert"],
        ];
        for (s, [time, mix, invert]) in self.sides.iter_mut().zip(KEYS) {
            let t = p.get(time).clamp(-SIMPLE_MS, SIMPLE_MS);
            s.time.to = (t / 1000.0 * sr) as f32;
            s.mix.to = (p.get(mix) / 100.0).clamp(0.0, 1.0) as f32;
            s.pol.to = if p.on(invert) { -1.0 } else { 1.0 };
        }
    }

    fn run_simple(&mut self, f: &mut [f32]) {
        let a = self.a_fast;
        let lat = self.lat as f32;
        let mut moves = [(0.0f32, 1.0f32, 1.0f32); 2];
        for (m, s) in moves.iter_mut().zip(self.sides.iter_mut()) {
            let t = s.time.next(self.a_time);
            *m = ((lat + t).max(0.0), s.mix.next(a), s.pol.next(a));
        }
        let w = self.w;
        for (v, &c) in self.voices.iter_mut().zip(&self.chans) {
            v.line[w] = f[c];
            let held = v.tap(w, lat);
            f[c] = match v.side {
                0 => held,
                s => {
                    let (dist, mix, pol) = moves[usize::from(s > 0)];
                    let moved = v.tap(w, dist);
                    pol * (mix * moved + (1.0 - mix) * held)
                }
            };
        }
    }

    fn run_echo(&mut self, f: &mut [f32]) {
        let a = self.a_fast;
        // The time glides, but never faster than 4 frames a frame.
        let before = self.delay.cur;
        let d = self.delay.next(self.a_time);
        let d = before + (d - before).clamp(-4.0, 4.0);
        self.delay.cur = d;
        let fb = self.feedback.next(a);
        let dry = self.dry.next(a);
        let wet = self.wet.next(a);
        let depth = self.depth.next(a);
        let flutter = self.flutter.next(a);
        let k = self.sat.next(a);
        let tone = self.tone.next(a);
        let low = self.low.next(a);
        let dark = self.dark.next(a);
        let dark2 = self.dark2.next(a);
        let noise = self.noise.next(a);
        let hum_g = self.hum.next(a);
        let spread = if self.bed { self.spread.next(a) } else { 0.0 };
        let bbd = self.kind == BBD;
        let tape = self.kind == TAPE;
        self.lfo = (self.lfo + self.lfo_step).fract();
        self.flutter_ph = (self.flutter_ph + 6.5 / self.sr).fract();
        let hum = if hum_g > 0.0 {
            self.hum_ph = (self.hum_ph + 60.0 / self.sr).fract();
            let x = std::f64::consts::TAU * self.hum_ph;
            hum_g * (x.sin() + 0.5 * (2.0 * x).sin() + 0.25 * (3.0 * x).sin()) as f32
        } else {
            0.0
        };
        let w = self.w;
        let tau = std::f64::consts::TAU;
        // Each line's repeat, read through its wobble and its filters.
        for v in &mut self.voices {
            let ph = (self.lfo + v.phase_off).fract();
            let wob = if bbd {
                // A triangle, as a bucket brigade's clock sweep.
                (4.0 * (ph - 0.5).abs() - 1.0) as f32
            } else {
                (tau * ph).sin() as f32
            };
            let mut m = depth * wob;
            if flutter > 0.0 {
                m += flutter * (tau * (self.flutter_ph + v.phase_off * 0.37)).sin() as f32;
            }
            let x = v.tap(w, (d + m).max(3.0));
            v.tone1 = flush(v.tone1 + tone * (x - v.tone1));
            v.tone2 = flush(v.tone2 + tone * (v.tone1 - v.tone2));
            v.low = flush(v.low + low * (v.tone2 - v.low));
            let y = v.tone2 - v.low;
            v.dark1 = flush(v.dark1 + dark * (y - v.dark1));
            v.dark2 = flush(v.dark2 + dark2 * (v.dark1 - v.dark2));
            v.rep = flush(shape(v.dark2, k, bbd));
        }
        // Into the lines: the input and the repeat fed back.
        let n = self.voices.len();
        let mono = if self.pingpong && !self.bed && n == 2 {
            0.5 * (f[self.chans[0]] + f[self.chans[1]])
        } else {
            0.0
        };
        for i in 0..n {
            let x = f[self.chans[i]];
            let (input, from) = if self.pingpong {
                let from = (i + n - 1) % n;
                if self.bed || n != 2 {
                    (x, from)
                } else if i == 0 {
                    (mono, from)
                } else {
                    (0.0, from)
                }
            } else {
                (x, i)
            };
            let back = self.voices[from].rep;
            self.voices[i].line[w] = guard(input + fb * back);
        }
        // Out: the dry sound and the repeats (spread round the room on a
        // bed), with the machine's noise riding on them.
        for i in 0..n {
            let v = &self.voices[i];
            let mut rep = v.rep;
            if spread > 0.0 {
                let prev = self.voices[(i + n - 1) % n].rep;
                let next = self.voices[(i + 1) % n].rep;
                rep = (1.0 - 0.5 * spread) * rep + 0.25 * spread * (prev + next);
            }
            if noise > 0.0 {
                let v = &mut self.voices[i];
                let wn = v.white();
                let colored = if tape {
                    // Hiss: the bright part of white noise.
                    v.noise_lp += 0.3 * (wn - v.noise_lp);
                    wn - v.noise_lp
                } else {
                    // The brigade's darker hash.
                    v.noise_lp += 0.2 * (wn - v.noise_lp);
                    2.0 * v.noise_lp
                };
                rep += noise * colored;
            }
            rep += hum;
            let c = self.chans[i];
            f[c] = dry * f[c] + wet * rep;
        }
    }
}

impl Processor for Echo {
    fn process(&mut self, buf: &mut [f32]) {
        let ch = self.channels;
        let len = self.voices.first().map_or(1, |v| v.line.len());
        let np = self.passed.len();
        for f in buf.chunks_exact_mut(ch) {
            if self.kind == SIMPLE {
                self.run_simple(f);
            } else {
                self.run_echo(f);
            }
            self.w = (self.w + 1) % len;
            if np > 0 && self.lat > 0 {
                let wr = self.pass_at * np;
                let rd = ((self.pass_at + 1) % self.pass_len) * np;
                for (j, &c) in self.passed.iter().enumerate() {
                    self.pass[wr + j] = f[c];
                    f[c] = self.pass[rd + j];
                }
                self.pass_at = (self.pass_at + 1) % self.pass_len;
            }
        }
    }

    fn latency(&self) -> usize {
        self.lat
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            v.clear();
        }
        self.pass.fill(0.0);
        self.snap();
    }

    fn update(&mut self, p: &Params) -> bool {
        self.set(p);
        // A new look-ahead needs a rebuild; until then Simple keeps
        // within the one it has.
        latency_for(p, self.sr as u32) == self.lat
    }

    fn vary(&mut self, k: usize) {
        self.lfo = (k as f64 * 0.382).fract();
        for (c, v) in self.voices.iter_mut().enumerate() {
            v.seed = 0x51ed_270b ^ ((k * 8 + c) as u32 + 1).wrapping_mul(0x2545_f491);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const R: u32 = 48_000;

    fn params(over: &[(&str, f64)]) -> Params {
        let saved: BTreeMap<String, f64> =
            over.iter().map(|(k, v)| ((*k).to_string(), *v)).collect();
        Params::resolve(&echo_info(), &saved)
    }

    /// Runs `input` (interleaved stereo) through an Echo in blocks.
    fn run(p: &Params, input: &[f32]) -> (Vec<f32>, usize) {
        let mut e = Echo::new(p, R);
        let mut b = input.to_vec();
        for blk in b.chunks_mut(512) {
            e.process(blk);
        }
        (b, e.latency())
    }

    fn impulse(frames: usize) -> Vec<f32> {
        let mut v = vec![0.0; frames * 2];
        v[0] = 1.0;
        v[1] = 1.0;
        v
    }

    fn side(b: &[f32], c: usize) -> Vec<f32> {
        b.iter().skip(c).step_by(2).copied().collect()
    }

    fn peak_at(x: &[f32], from: usize, to: usize) -> (usize, f32) {
        (from..to.min(x.len())).fold((from, 0.0f32), |(i, m), k| {
            if x[k].abs() > m {
                (k, x[k].abs())
            } else {
                (i, m)
            }
        })
    }

    /// Only the repeats, clean, at 100 ms.
    fn wet_only(kind: usize) -> Vec<(&'static str, f64)> {
        vec![
            ("kind", kind as f64),
            ("time", 100.0),
            ("feedback", 0.0),
            ("dry", 0.0),
            ("wet", 100.0),
            ("analog", 0.0),
            ("tone", 16_000.0),
        ]
    }

    #[test]
    fn each_kind_repeats_on_time() {
        for kind in [DIGITAL, TAPE, BBD] {
            let (b, lat) = run(&params(&wet_only(kind)), &impulse(R as usize / 2));
            assert_eq!(lat, 0);
            for c in 0..2 {
                let x = side(&b, c);
                let (at, m) = peak_at(&x, 0, x.len());
                assert!(at.abs_diff(4800) <= 12, "kind {kind}: repeat at {at}");
                assert!(m > 0.2, "kind {kind}: repeat level {m}");
                // Nothing before it.
                assert!(x[..4700].iter().all(|v| v.abs() < 1e-6), "kind {kind}");
            }
        }
    }

    #[test]
    fn a_followed_tempo_sets_the_time() {
        // An eighth note at 100 BPM is 300 ms.
        let mut set = wet_only(DIGITAL);
        set.extend([("sync", 1.0), ("note", 7.0), ("tempo", 100.0)]);
        let p = params(&set);
        assert!((time_ms(&p) - 300.0).abs() < 1e-9);
        let (b, _) = run(&p, &impulse(R as usize / 2));
        let (at, _) = peak_at(&side(&b, 0), 0, R as usize / 2);
        assert!(at.abs_diff(14_400) <= 2, "{at}");
    }

    #[test]
    fn feedback_brings_each_repeat_down_by_its_amount() {
        let mut set = wet_only(DIGITAL);
        set.push(("feedback", 50.0));
        // A short 1 kHz burst, well inside the filters.
        let mut input = vec![0.0f32; R as usize * 2];
        for i in 0..480 {
            let v = 0.5 * (std::f32::consts::TAU * 1000.0 * i as f32 / R as f32).sin();
            input[i * 2] = v;
            input[i * 2 + 1] = v;
        }
        let (b, _) = run(&params(&set), &input);
        let x = side(&b, 0);
        let rep: Vec<f32> = (1..5)
            .map(|k| peak_at(&x, k * 4800, k * 4800 + 900).1)
            .collect();
        for k in 1..rep.len() {
            let ratio = rep[k] / rep[k - 1];
            assert!((ratio - 0.5).abs() < 0.03, "repeat {k}: {ratio}");
        }
        // And the loop dies away.
        let tail = peak_at(&x, 40_000, 48_000).1;
        assert!(tail < 0.01, "{tail}");
    }

    #[test]
    fn ping_pong_alternates_sides() {
        let mut set = wet_only(DIGITAL);
        set.extend([("feedback", 80.0), ("pingpong", 1.0)]);
        let (b, _) = run(&params(&set), &impulse(R as usize));
        let (l, r) = (side(&b, 0), side(&b, 1));
        for k in 1..6 {
            let win = (k * 4800 - 100, k * 4800 + 200);
            let pl = peak_at(&l, win.0, win.1).1;
            let pr = peak_at(&r, win.0, win.1).1;
            let (on, off) = if k % 2 == 1 { (pl, pr) } else { (pr, pl) };
            assert!(on > 0.05 && off < 1e-4, "repeat {k}: {pl} {pr}");
        }
    }

    #[test]
    fn simple_moves_each_side_by_its_own_time() {
        let p = params(&[("kind", SIMPLE as f64), ("l_time", 10.0), ("r_time", 20.0)]);
        let (b, lat) = run(&p, &impulse(4800));
        assert_eq!(lat, 0);
        let (l, r) = (side(&b, 0), side(&b, 1));
        assert_eq!(peak_at(&l, 0, 4800), (480, 1.0));
        assert_eq!(peak_at(&r, 0, 4800), (960, 1.0));
        // Half mix: half in place, half moved; Invert flips the side.
        let p = params(&[
            ("kind", SIMPLE as f64),
            ("l_time", 10.0),
            ("l_mix", 50.0),
            ("r_invert", 1.0),
        ]);
        let (b, _) = run(&p, &impulse(4800));
        let (l, r) = (side(&b, 0), side(&b, 1));
        assert!((l[0] - 0.5).abs() < 1e-6 && (l[480] - 0.5).abs() < 1e-6);
        assert!((r[0] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn an_early_side_makes_latency_and_the_earliest_lines_up() {
        let p = params(&[("kind", SIMPLE as f64), ("l_time", -10.0), ("r_time", 5.0)]);
        assert_eq!(latency_for(&p, R), 480);
        let (b, lat) = run(&p, &impulse(4800));
        assert_eq!(lat, 480);
        let (l, r) = (side(&b, 0), side(&b, 1));
        // Left (the earliest) comes out at once; right 15 ms after it.
        assert_eq!(peak_at(&l, 0, 4800).0, 0);
        assert_eq!(peak_at(&r, 0, 4800).0, 720);
        // The other kinds never add latency.
        let mut e = Echo::new(&params(&[("kind", 0.0), ("l_time", -10.0)]), R);
        assert_eq!(e.latency(), 0);
        // Asking for more look-ahead than it was built with needs a rebuild.
        assert!(!e.update(&p));
    }

    #[test]
    fn silence_stays_silent_on_digital_and_simple() {
        for kind in [DIGITAL, SIMPLE] {
            let p = params(&[
                ("kind", kind as f64),
                ("feedback", 110.0),
                ("drive", 100.0),
                ("depth", 100.0),
                ("analog", 4.0),
            ]);
            let (b, _) = run(&p, &vec![0.0; 48_000]);
            assert!(b.iter().all(|v| *v == 0.0), "kind {kind}");
        }
        // The machine's noise is there on Tape and Analog above Off, and
        // only there.
        for (kind, analog, noisy) in [(TAPE, 0.0, false), (TAPE, 2.0, true), (BBD, 4.0, true)] {
            let p = params(&[("kind", kind as f64), ("analog", analog)]);
            let (b, _) = run(&p, &vec![0.0; 48_000]);
            let m = b.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert_eq!(m > 0.0, noisy, "kind {kind} analog {analog}: {m}");
            assert!(m < 0.01, "{m}");
        }
    }

    #[test]
    fn no_nans_or_denormals_and_hot_feedback_stays_bounded() {
        for kind in [DIGITAL, TAPE, BBD, SIMPLE] {
            for pp in [0.0, 1.0] {
                let p = params(&[
                    ("kind", kind as f64),
                    ("feedback", 120.0),
                    ("pingpong", pp),
                    ("time", 7.0),
                    ("depth", 100.0),
                    ("rate", 10.0),
                    ("drive", 50.0),
                    ("l_time", -3.0),
                ]);
                let mut input = impulse(R as usize * 2);
                input[2] = -1.0;
                let (b, _) = run(&p, &input);
                assert!(
                    b.iter().all(|v| v.is_finite() && v.abs() < 4.0),
                    "kind {kind}"
                );
                assert!(
                    b.iter().all(|v| *v == 0.0 || v.is_normal()),
                    "kind {kind}: a denormal"
                );
            }
            // A decaying tail into silence leaves no denormals behind.
            let mut set = wet_only(kind);
            set.push(("feedback", 30.0));
            let (b, _) = run(&params(&set), &impulse(R as usize * 3));
            assert!(b.iter().all(|v| *v == 0.0 || v.is_normal()), "kind {kind}");
        }
    }

    #[test]
    fn moving_the_time_does_not_click() {
        // A steady sine through the repeats while the time jumps: the
        // output never steps by more than a sine of that level could.
        let p0 = params(&[
            ("time", 200.0),
            ("feedback", 0.0),
            ("dry", 0.0),
            ("wet", 100.0),
        ]);
        let p1 = params(&[
            ("time", 260.0),
            ("feedback", 0.0),
            ("dry", 0.0),
            ("wet", 100.0),
        ]);
        let mut e = Echo::new(&p0, R);
        let mut b: Vec<f32> = (0..R as usize)
            .flat_map(|i| {
                let v = 0.5 * (std::f32::consts::TAU * 200.0 * i as f32 / R as f32).sin();
                [v, v]
            })
            .collect();
        for (i, blk) in b.chunks_mut(512).enumerate() {
            if i == 40 {
                assert!(e.update(&p1));
            }
            e.process(blk);
        }
        let l = side(&b, 0);
        let worst = l[12_000..]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        // A 200 Hz sine at 0.5 moves at most 0.013 a frame; allow the
        // glide's pitch bend and the tone filter.
        assert!(worst < 0.05, "{worst}");
    }

    #[test]
    fn a_bed_echoes_every_speaker_and_leaves_the_lfe_dry() {
        let names: Vec<String> = ["L", "R", "C", "LFE", "Ls", "Rs"]
            .iter()
            .map(|n| (*n).to_string())
            .collect();
        let p = params(&[
            ("pingpong", 1.0),
            ("feedback", 60.0),
            ("dry", 0.0),
            ("wet", 100.0),
        ]);
        let mut e = Echo::wide(&p, R, &names);
        let mut b = vec![0.0f32; 6 * R as usize];
        for c in 0..6 {
            b[c] = 1.0;
        }
        for blk in b.chunks_mut(6 * 256) {
            e.process(blk);
        }
        assert!(b.iter().all(|v| v.is_finite()));
        let ch = |c: usize| -> Vec<f32> { b.iter().skip(c).step_by(6).copied().collect() };
        assert_eq!(ch(3)[0], 1.0, "LFE as it was");
        assert!(ch(3)[1..].iter().all(|v| *v == 0.0));
        for c in [0, 1, 2, 4, 5] {
            let e: f32 = ch(c).iter().map(|v| v * v).sum();
            assert!(e > 1e-3, "speaker {c} echoes");
        }
        // Simple on a bed: the LFE held back by the look-ahead.
        let p = params(&[("kind", SIMPLE as f64), ("l_time", -10.0)]);
        let mut e = Echo::wide(&p, R, &names);
        assert_eq!(e.latency(), 480);
        let mut b = vec![0.0f32; 6 * 4800];
        for c in 0..6 {
            b[c] = 1.0;
        }
        e.process(&mut b);
        let ch = |c: usize| -> Vec<f32> { b.iter().skip(c).step_by(6).copied().collect() };
        assert_eq!(ch(3)[480], 1.0);
        assert_eq!(ch(0)[0], 1.0, "L moved early by its time");
        assert_eq!(ch(1)[480], 1.0, "R held back to match");
    }
}
