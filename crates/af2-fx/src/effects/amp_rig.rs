// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The rig around the Amp Model's capture, written from scratch for
//! Audiform2 (no code or responses taken from any product): a noise
//! gate, a boost with a tight low cut before the amp, and after it a
//! cabinet with two microphones, a small room and a doubler.
//!
//! * **Cabinets** are designed responses, not recordings: a low cut
//!   where the box stops working, the box's low resonance, a scoop where
//!   it gets boxy, the cone's breakup peaks and its dip, and a steep fall
//!   above about 5–6 kHz, which is what takes the fizz off an amp head.
//! * **Microphones** add their own colour on top: proximity bass that
//!   fades with distance, a presence peak (dynamic), a dark top
//!   (ribbon) or an open, airy one (condenser). Off centre toward the
//!   cone's edge is darker; farther away arrives later, loses bass and
//!   picks up a reflection off the baffle.
//! * **Two mics** run on the same cabinet and are blended; each path is
//!   brought to about the same level, so blending changes the tone and
//!   not the loudness (their arrival times differ, as on a real cab).
//! * **Room** is a few early reflections and a short damped tail, left
//!   and right different, added under the dry sound.
//! * **Double** adds two slowly drifting, slightly delayed copies, one
//!   each side, so a mono amp opens out to stereo with its centre kept.

use crate::biquad::{Biquad, Coeffs, Shape};

/// The built-in cabinets, in the order of the Cabinet control.
pub const CABS: &[&str] = &[
    "1x12 Open Back",
    "2x12 Open Back",
    "2x12 Closed",
    "4x12 Closed Modern",
    "4x12 Closed Vintage",
    "4x10 Open Tweed",
];

/// The microphones, in the order of the Mic controls.
pub const MICS: &[&str] = &["Dynamic", "Ribbon", "Condenser"];

/// The farthest a mic can be from the grille, cm.
pub const MAX_DIST_CM: f64 = 30.0;

const CAB_N: usize = 8;
const MIC_N: usize = 7;

/// A section that changes nothing.
fn flat() -> Coeffs {
    Coeffs::new(Shape::Peak, 48_000, 1_000.0, 1.0, 0.0)
}

/// (shape, Hz, Q, dB) of one designed section.
type Sec = (Shape, f64, f64, f64);

/// Cabinet `i`'s design.
fn cab_design(i: usize) -> [Sec; CAB_N] {
    use Shape::{HighPass as Hp, LowPass as Lp, Peak as Pk};
    match i {
        // Light, open low end (the back wave cancels some of it), a
        // bright chime from the single cone.
        0 => [
            (Hp, 95.0, 0.7, 0.0),
            (Pk, 115.0, 1.2, 2.0),
            (Pk, 420.0, 1.0, -2.0),
            (Pk, 2_200.0, 1.4, 3.0),
            (Pk, 3_600.0, 2.0, 2.5),
            (Pk, 4_400.0, 3.0, -4.0),
            (Lp, 5_400.0, 0.8, 0.0),
            (Lp, 6_200.0, 0.6, 0.0),
        ],
        1 => [
            (Hp, 82.0, 0.7, 0.0),
            (Pk, 120.0, 1.1, 3.0),
            (Pk, 500.0, 0.9, -1.5),
            (Pk, 1_600.0, 1.2, 2.0),
            (Pk, 3_000.0, 1.6, 3.0),
            (Pk, 4_200.0, 3.0, -3.0),
            (Lp, 5_500.0, 0.75, 0.0),
            (Lp, 6_200.0, 0.6, 0.0),
        ],
        // Closed boxes: a firmer resonance and a boxy scoop to tame.
        2 => [
            (Hp, 72.0, 0.7, 0.0),
            (Pk, 105.0, 1.4, 4.0),
            (Pk, 350.0, 1.0, -2.5),
            (Pk, 2_000.0, 1.3, 2.5),
            (Pk, 3_400.0, 2.0, 3.0),
            (Pk, 4_600.0, 3.0, -3.5),
            (Lp, 5_400.0, 0.8, 0.0),
            (Lp, 6_000.0, 0.6, 0.0),
        ],
        3 => [
            (Hp, 65.0, 0.7, 0.0),
            (Pk, 95.0, 1.5, 5.0),
            (Pk, 400.0, 1.1, -3.0),
            (Pk, 1_400.0, 1.0, 1.5),
            (Pk, 2_600.0, 1.8, 4.0),
            (Pk, 4_000.0, 2.5, -4.0),
            (Lp, 5_000.0, 0.85, 0.0),
            (Lp, 5_800.0, 0.6, 0.0),
        ],
        4 => [
            (Hp, 70.0, 0.7, 0.0),
            (Pk, 100.0, 1.3, 4.0),
            (Pk, 600.0, 0.9, -1.5),
            (Pk, 1_800.0, 1.2, 3.0),
            (Pk, 3_200.0, 2.0, 1.5),
            (Pk, 4_400.0, 3.0, -3.0),
            (Lp, 4_800.0, 0.75, 0.0),
            (Lp, 5_600.0, 0.6, 0.0),
        ],
        // Four small cones in an open box: quick, bright, light lows.
        _ => [
            (Hp, 85.0, 0.7, 0.0),
            (Pk, 130.0, 1.0, 2.0),
            (Pk, 300.0, 1.0, -1.0),
            (Pk, 1_200.0, 1.0, 1.5),
            (Pk, 3_800.0, 1.5, 3.0),
            (Pk, 5_000.0, 3.0, -3.0),
            (Lp, 5_800.0, 0.75, 0.0),
            (Lp, 6_400.0, 0.6, 0.0),
        ],
    }
}

fn coeffs<const N: usize>(d: [Sec; N], rate: u32) -> [Coeffs; N] {
    d.map(|(s, f, q, g)| Coeffs::new(s, rate, f, q, g))
}

/// Cabinet `i`'s sections at `rate`.
pub fn cab_coeffs(i: usize, rate: u32) -> [Coeffs; CAB_N] {
    coeffs(cab_design(i.min(CABS.len() - 1)), rate)
}

/// One mic's sections: `kind` from [`MICS`], `pos` 0 (centre of the
/// cone) to 1 (its edge), `dist` in cm from the grille.
pub fn mic_coeffs(kind: usize, pos: f64, dist: f64, rate: u32) -> [Coeffs; MIC_N] {
    use Shape::{HighPass as Hp, HighShelf as Hs, LowPass as Lp, LowShelf as Ls, Peak as Pk};
    let pos = pos.clamp(0.0, 1.0);
    let far = (dist / MAX_DIST_CM).clamp(0.0, 1.0);
    // Proximity bass: strong right on the grille, gone by a hand away.
    let prox = (-dist.max(0.0) / 6.0).exp();
    let (prox_db, colour): (f64, [Sec; 3]) = match kind {
        // Presence peak, a little thin below.
        0 => (
            6.0,
            [(Hp, 70.0, 0.7, 0.0), (Pk, 4_800.0, 1.1, 4.0), (Pk, 200.0, 0.9, -1.0)],
        ),
        // Dark and round: a soft top, more proximity.
        1 => (
            9.0,
            [(Hs, 3_000.0, 0.6, -5.0), (Lp, 8_000.0, 0.6, 0.0), (Pk, 900.0, 0.8, 0.8)],
        ),
        // Open and airy.
        _ => (
            3.0,
            [(Pk, 250.0, 1.0, -1.0), (Pk, 3_000.0, 1.0, 1.0), (Pk, 10_000.0, 0.8, 3.0)],
        ),
    };
    let d: [Sec; MIC_N] = [
        (Ls, 170.0, 0.7, prox_db * prox - 2.5 * far),
        colour[0],
        colour[1],
        colour[2],
        // Toward the edge: darker, a touch more low-mid.
        (Hs, 2_200.0, 0.7, -10.0 * pos - 3.0 * far),
        (Pk, 3_500.0, 1.2, 2.0 * (1.0 - pos)),
        (Pk, 260.0, 1.0, 1.5 * pos),
    ];
    coeffs(d, rate)
}

/// The gain of a chain of sections at `f` Hz, dB.
pub fn chain_db(c: &[Coeffs], rate: u32, f: f64) -> f64 {
    c.iter().map(|s| s.response_db(rate, f)).sum()
}

/// The level, dB, a chain's 150 Hz–4 kHz band averages (by power).
fn band_db(c: &[Coeffs], rate: u32) -> f64 {
    let n = 24;
    let p: f64 = (0..n)
        .map(|k| {
            let f = 150.0 * (4_000.0f64 / 150.0).powf(f64::from(k) / f64::from(n - 1));
            10f64.powf(chain_db(c, rate, f) / 10.0)
        })
        .sum::<f64>()
        / f64::from(n);
    10.0 * p.max(1e-12).log10()
}

// ------------------------------------------------------------ Gate

/// A noise gate: opens fast above the threshold, holds, and closes
/// smoothly once the level is 6 dB under it (so it does not chatter).
pub struct Gate {
    pub on: bool,
    open_at: f32,
    close_at: f32,
    env: f32,
    gain: f32,
    open: bool,
    hold: usize,
    held: usize,
    env_up: f32,
    env_down: f32,
    attack: f32,
    release: f32,
}

impl Gate {
    pub fn new(rate: u32) -> Gate {
        let k = |ms: f64| (-1.0 / (ms * 0.001 * f64::from(rate.max(1)))).exp() as f32;
        Gate {
            on: true,
            open_at: 1e-4,
            close_at: 5e-5,
            env: 0.0,
            gain: 1.0,
            open: true,
            hold: (0.04 * f64::from(rate)) as usize,
            held: 0,
            env_up: k(0.5),
            env_down: k(20.0),
            attack: k(1.0),
            release: k(50.0),
        }
    }

    pub fn set(&mut self, on: bool, threshold: f32) {
        self.on = on;
        self.open_at = threshold;
        self.close_at = threshold * 0.5;
    }

    pub fn run(&mut self, x: f32) -> f32 {
        if !self.on {
            return x;
        }
        let a = x.abs();
        let k = if a > self.env { self.env_up } else { self.env_down };
        self.env = a + k * (self.env - a);
        if self.env > self.open_at {
            self.open = true;
            self.held = 0;
        } else if self.open && self.env < self.close_at {
            self.held += 1;
            if self.held > self.hold {
                self.open = false;
            }
        }
        let want = if self.open { 1.0 } else { 0.0 };
        let k = if want > self.gain { self.attack } else { self.release };
        self.gain = want + k * (self.gain - want);
        x * self.gain
    }

    /// How far open it is, 0 to 1.
    pub fn gain(&self) -> f32 {
        self.gain
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
        self.gain = 1.0;
        self.open = true;
        self.held = 0;
    }
}

// ------------------------------------------------------------ Boost

/// The pedal before the amp: a steep low cut (`tight`), then `boost` dB
/// of gain with a mid hump and a softened top, which is how a mid boost
/// makes a driven amp tighter and less fizzy.
pub struct Boost {
    secs: [Biquad; 4],
    gain: f32,
}

impl Boost {
    pub fn new(rate: u32, tight: f64, boost: f64) -> Boost {
        let mut b = Boost {
            secs: [Biquad::new(flat()); 4],
            gain: 1.0,
        };
        b.set(rate, tight, boost);
        b
    }

    pub fn set(&mut self, rate: u32, tight: f64, boost: f64) {
        let boost = boost.max(0.0);
        let c = [
            Coeffs::new(Shape::HighPass, rate, tight, 0.54, 0.0),
            Coeffs::new(Shape::HighPass, rate, tight, 1.31, 0.0),
            Coeffs::new(Shape::Peak, rate, 720.0, 0.7, boost * 0.35),
            Coeffs::new(Shape::HighShelf, rate, 4_500.0, 0.7, -boost * 0.25),
        ];
        for (s, c) in self.secs.iter_mut().zip(c) {
            s.set(c);
        }
        self.gain = 10f32.powf((boost * 0.65) as f32 / 20.0);
    }

    pub fn run(&mut self, x: f32) -> f32 {
        let mut y = x;
        for s in &mut self.secs {
            y = s.run(y);
        }
        y * self.gain
    }

    pub fn reset(&mut self) {
        for s in &mut self.secs {
            s.reset();
        }
    }
}

// ------------------------------------------------------------ Cabinet

/// The longest a mic's arrival delay plus its baffle reflection can be.
const MIC_BUF: usize = 256;

struct MicPath {
    secs: [Biquad; MIC_N],
    gain: f32,
    buf: [f32; MIC_BUF],
    at: usize,
    arrive: usize,
    bounce: usize,
    bounce_gain: f32,
}

impl MicPath {
    fn new() -> MicPath {
        MicPath {
            secs: [Biquad::new(flat()); MIC_N],
            gain: 1.0,
            buf: [0.0; MIC_BUF],
            at: 0,
            arrive: 0,
            bounce: 0,
            bounce_gain: 0.0,
        }
    }

    fn set(&mut self, cab: &[Coeffs; CAB_N], kind: usize, pos: f64, dist: f64, rate: u32) {
        let c = mic_coeffs(kind, pos, dist, rate);
        for (s, c) in self.secs.iter_mut().zip(c) {
            s.set(c);
        }
        let all: Vec<Coeffs> = cab.iter().chain(c.iter()).copied().collect();
        self.gain = 10f32.powf(-band_db(&all, rate) as f32 / 20.0);
        let r = f64::from(rate);
        let far = (dist / MAX_DIST_CM).clamp(0.0, 1.0);
        // Sound travels about 34.3 cm a millisecond.
        self.arrive = ((dist.max(0.0) / 34_300.0 * r) as usize).min(MIC_BUF / 2);
        self.bounce = (self.arrive + ((0.25 + 1.2 * far) * 0.001 * r) as usize).min(MIC_BUF - 1);
        self.bounce_gain = (0.32 * far) as f32;
    }

    fn run(&mut self, x: f32) -> f32 {
        self.buf[self.at] = x;
        let n = MIC_BUF;
        let mut y = self.buf[(self.at + n - self.arrive) % n]
            + self.bounce_gain * self.buf[(self.at + n - self.bounce) % n];
        self.at = (self.at + 1) % n;
        for s in &mut self.secs {
            y = s.run(y);
        }
        y * self.gain / (1.0 + self.bounce_gain)
    }

    fn reset(&mut self) {
        for s in &mut self.secs {
            s.reset();
        }
        self.buf.fill(0.0);
    }
}

/// What the cabinet stage is set to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CabSet {
    pub cab: usize,
    /// (kind, position 0–1, distance cm) of mics A and B.
    pub a: (usize, f64, f64),
    pub b: (usize, f64, f64),
    /// 0: mic A alone, 1: mic B alone.
    pub blend: f64,
    /// Where each mic sits left to right, -1..1 (0 centre).
    pub pan_a: f64,
    pub pan_b: f64,
}

impl Default for CabSet {
    fn default() -> Self {
        CabSet {
            cab: 4,
            a: (0, 0.3, 2.0),
            b: (1, 0.45, 4.0),
            blend: 0.3,
            pan_a: 0.0,
            pan_b: 0.0,
        }
    }
}

/// A built-in cabinet with two mics on it.
pub struct Cab {
    secs: [Biquad; CAB_N],
    a: MicPath,
    b: MicPath,
    blend: f32,
    /// Each mic's (left, right) gains: equal power, 1 and 1 at centre.
    pan_a: (f32, f32),
    pan_b: (f32, f32),
}

/// Equal-power pan gains for `p` (-1 left .. 1 right), both 1 at centre.
pub fn pan_gains(p: f64) -> (f32, f32) {
    let a = (p.clamp(-1.0, 1.0) + 1.0) * std::f64::consts::FRAC_PI_4;
    let k = std::f64::consts::SQRT_2;
    ((a.cos() * k) as f32, (a.sin() * k) as f32)
}

impl Cab {
    pub fn new(set: CabSet, rate: u32) -> Cab {
        let mut c = Cab {
            secs: [Biquad::new(flat()); CAB_N],
            a: MicPath::new(),
            b: MicPath::new(),
            blend: 0.0,
            pan_a: (1.0, 1.0),
            pan_b: (1.0, 1.0),
        };
        c.set(set, rate);
        c
    }

    pub fn set(&mut self, set: CabSet, rate: u32) {
        let cab = cab_coeffs(set.cab, rate);
        for (s, c) in self.secs.iter_mut().zip(cab) {
            s.set(c);
        }
        self.a.set(&cab, set.a.0, set.a.1, set.a.2, rate);
        self.b.set(&cab, set.b.0, set.b.1, set.b.2, rate);
        self.blend = set.blend.clamp(0.0, 1.0) as f32;
        self.pan_a = pan_gains(set.pan_a);
        self.pan_b = pan_gains(set.pan_b);
    }

    /// Whether the mics sit anywhere but the middle (the cab is stereo).
    pub fn panned(&self) -> bool {
        self.pan_a != (1.0, 1.0) || self.pan_b != (1.0, 1.0)
    }

    /// One sample through the cabinet, each mic placed left to right.
    pub fn run_stereo(&mut self, x: f32) -> (f32, f32) {
        let mut y = x;
        for s in &mut self.secs {
            y = s.run(y);
        }
        let (wa, wb) = (1.0 - self.blend, self.blend);
        let a = if wa > 0.0 { self.a.run(y) * wa } else { 0.0 };
        let b = if wb > 0.0 { self.b.run(y) * wb } else { 0.0 };
        (
            a * self.pan_a.0 + b * self.pan_b.0,
            a * self.pan_a.1 + b * self.pan_b.1,
        )
    }

    pub fn run(&mut self, x: f32) -> f32 {
        let mut y = x;
        for s in &mut self.secs {
            y = s.run(y);
        }
        let (wa, wb) = (1.0 - self.blend, self.blend);
        let a = if wa > 0.0 { self.a.run(y) } else { 0.0 };
        let b = if wb > 0.0 { self.b.run(y) } else { 0.0 };
        a * wa + b * wb
    }

    pub fn reset(&mut self) {
        for s in &mut self.secs {
            s.reset();
        }
        self.a.reset();
        self.b.reset();
    }
}

/// The gain, dB, of a cabinet with one mic at `f` Hz (level-matched as
/// it plays), for tests and drawing.
pub fn cab_mic_db(cab: usize, mic: (usize, f64, f64), rate: u32, f: f64) -> f64 {
    let c = cab_coeffs(cab, rate);
    let m = mic_coeffs(mic.0, mic.1, mic.2, rate);
    let all: Vec<Coeffs> = c.iter().chain(m.iter()).copied().collect();
    chain_db(&all, rate, f) - band_db(&all, rate)
}

// ------------------------------------------------------------ Room

const ER_L: [f64; 6] = [3.1, 5.3, 7.9, 11.2, 14.7, 19.3];
const ER_R: [f64; 6] = [4.2, 6.6, 9.4, 12.5, 16.8, 21.9];
const COMBS: [f64; 4] = [23.3, 27.1, 31.7, 35.9];
const APS: [f64; 2] = [5.1, 1.7];

struct Line {
    buf: Vec<f32>,
    at: usize,
}

impl Line {
    fn new(len: usize) -> Line {
        Line {
            buf: vec![0.0; len.max(2)],
            at: 0,
        }
    }
    fn push(&mut self, x: f32) {
        self.buf[self.at] = x;
        self.at = (self.at + 1) % self.buf.len();
    }
    /// The sample written `d` pushes ago (1 = the last one).
    fn tap(&self, d: usize) -> f32 {
        let n = self.buf.len();
        self.buf[(self.at + n - d.clamp(1, n)) % n]
    }
    /// Linearly between samples.
    fn tap_frac(&self, d: f32) -> f32 {
        let i = d.floor();
        let t = d - i;
        let i = i as usize;
        self.tap(i) * (1.0 - t) + self.tap(i + 1) * t
    }
    fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

struct Comb {
    line: Line,
    len: usize,
    fb: f32,
    damp: f32,
    lp: f32,
}

/// A small room: early reflections and a short damped tail.
pub struct Room {
    mix: f32,
    er: Line,
    er_l: [(usize, f32); 6],
    er_r: [(usize, f32); 6],
    combs: [Comb; 4],
    aps: [[(Line, usize); 2]; 2],
    rate: u32,
}

impl Room {
    pub fn new(rate: u32, mix: f64, size: f64) -> Room {
        let r = f64::from(rate);
        let ms = |v: f64| (v * 0.001 * r) as usize + 2;
        let comb = |m: f64| Comb {
            line: Line::new(ms(m * 1.25)),
            len: 1,
            fb: 0.0,
            damp: 0.3,
            lp: 0.0,
        };
        let ap = |m: f64| (Line::new(ms(m)), ms(m) - 1);
        let mut room = Room {
            mix: 0.0,
            er: Line::new(ms(22.0 * 2.0)),
            er_l: [(1, 0.0); 6],
            er_r: [(1, 0.0); 6],
            combs: COMBS.map(comb),
            aps: [APS.map(ap), APS.map(|m| ap(m * 1.13))],
            rate,
        };
        room.set(mix, size);
        room
    }

    /// `mix` and `size` 0 to 1.
    pub fn set(&mut self, mix: f64, size: f64) {
        let r = f64::from(self.rate);
        let s = size.clamp(0.0, 1.0);
        let scale = 0.5 + 1.5 * s;
        for (k, (l, rr)) in ER_L.iter().zip(ER_R).enumerate() {
            let g = 0.8f32.powi(k as i32) * if k % 2 == 0 { 1.0 } else { -1.0 } * 0.5;
            self.er_l[k] = ((l * scale * 0.001 * r) as usize + 1, g);
            self.er_r[k] = ((rr * scale * 0.001 * r) as usize + 1, -g);
        }
        let rt60 = 0.25 + 0.6 * s;
        let cs = 0.5 + 0.75 * s;
        for (c, m) in self.combs.iter_mut().zip(COMBS) {
            let len = ((m * cs * 0.001 * r) as usize).clamp(1, c.line.buf.len() - 1);
            c.len = len;
            let secs = len as f64 / r;
            c.fb = 10f64.powf(-3.0 * secs / rt60) as f32;
            c.damp = 0.35 - 0.15 * s as f32;
        }
        self.mix = (mix.clamp(0.0, 1.0) * 0.8) as f32;
    }

    pub fn active(&self) -> bool {
        self.mix > 0.0
    }

    /// Adds the room to a mono sound: (left, right) of the wet part.
    pub fn run(&mut self, x: f32) -> (f32, f32) {
        self.er.push(x);
        let mut l = 0.0;
        let mut r = 0.0;
        for k in 0..6 {
            l += self.er.tap(self.er_l[k].0) * self.er_l[k].1;
            r += self.er.tap(self.er_r[k].0) * self.er_r[k].1;
        }
        let feed = (x + (l + r) * 0.5) * 0.25;
        let mut out = [0f32; 4];
        for (o, c) in out.iter_mut().zip(&mut self.combs) {
            let y = c.line.tap(c.len);
            c.lp = y + c.damp * (c.lp - y);
            c.line.push(feed + c.lp * c.fb);
            *o = y;
        }
        let mut tail = [out[0] - out[3] * 0.5 + out[2], out[1] - out[2] * 0.5 + out[3]];
        for (t, aps) in tail.iter_mut().zip(&mut self.aps) {
            for (line, d) in aps.iter_mut() {
                let z = line.tap(*d);
                let v = *t + 0.6 * z;
                line.push(v);
                *t = z - 0.6 * v;
            }
        }
        ((l + tail[0] * 0.6) * self.mix, (r + tail[1] * 0.6) * self.mix)
    }

    pub fn reset(&mut self) {
        self.er.clear();
        for c in &mut self.combs {
            c.line.clear();
            c.lp = 0.0;
        }
        for aps in &mut self.aps {
            for (line, _) in aps.iter_mut() {
                line.clear();
            }
        }
    }
}

// ------------------------------------------------------------ Double

/// Two slowly drifting delayed copies, one each side.
pub struct Doubler {
    amount: f32,
    line: Line,
    base: [f32; 2],
    depth: f32,
    phase: [f32; 2],
    step: [f32; 2],
    tone: [Biquad; 2],
}

impl Doubler {
    pub fn new(rate: u32, amount: f64) -> Doubler {
        let r = rate as f32;
        let ms = |v: f32| v * 0.001 * r;
        let mut d = Doubler {
            amount: 0.0,
            line: Line::new(ms(30.0) as usize),
            base: [ms(11.0), ms(17.0)],
            depth: ms(0.6),
            phase: [0.0, 0.37],
            step: [0.31 / r, 0.43 / r],
            tone: [
                Biquad::new(Coeffs::new(Shape::Peak, rate, 1_800.0, 0.8, -2.0)),
                Biquad::new(Coeffs::new(Shape::Peak, rate, 900.0, 0.8, -2.0)),
            ],
        };
        d.set(amount);
        d
    }

    /// `amount` 0 to 1.
    pub fn set(&mut self, amount: f64) {
        self.amount = amount.clamp(0.0, 1.0) as f32;
    }

    pub fn active(&self) -> bool {
        self.amount > 0.0
    }

    /// (left, right) to add.
    pub fn run(&mut self, x: f32) -> (f32, f32) {
        self.line.push(x);
        let mut out = [0f32; 2];
        for k in 0..2 {
            self.phase[k] = (self.phase[k] + self.step[k]).fract();
            let wob = (self.phase[k] * std::f32::consts::TAU).sin();
            let d = self.base[k] + self.depth * wob;
            out[k] = self.tone[k].run(self.line.tap_frac(d)) * self.amount * 0.7;
        }
        (out[0], out[1])
    }

    pub fn reset(&mut self) {
        self.line.clear();
        for t in &mut self.tone {
            t.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(f: f64, rate: u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (std::f64::consts::TAU * f * i as f64 / f64::from(rate)).sin() as f32 * 0.5)
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn every_cab_and_mic_rolls_off_the_highs() {
        for cab in 0..CABS.len() {
            for mic in 0..MICS.len() {
                for pos in [0.0, 0.5, 1.0] {
                    let m = (mic, pos, 2.0);
                    let at = |f| cab_mic_db(cab, m, 48_000, f);
                    let mid = at(1_000.0);
                    assert!(at(8_000.0) < mid - 10.0, "{cab} {mic} {pos}: {} vs {mid}", at(8_000.0));
                    assert!(at(12_000.0) < mid - 24.0, "{cab} {mic} {pos}: {}", at(12_000.0));
                    // And the low cut of the box.
                    assert!(at(30.0) < mid - 6.0, "{cab} {mic} {pos}: {}", at(30.0));
                }
            }
        }
    }

    #[test]
    fn the_cab_stage_cuts_fizz_on_real_signals() {
        let rate = 48_000;
        let run = |f: f64| {
            let mut c = Cab::new(CabSet::default(), rate);
            let y: Vec<f32> = sine(f, rate, 24_000).into_iter().map(|v| c.run(v)).collect();
            rms(&y[12_000..])
        };
        let mid = run(1_000.0);
        let high = run(10_000.0);
        assert!(high < mid * 0.1, "{high} vs {mid}");
        // Level-matched: the mids come through at about unity.
        assert!(mid > 0.15 && mid < 0.8, "{mid}");
    }

    #[test]
    fn position_toward_the_edge_is_darker() {
        let centre = cab_mic_db(3, (0, 0.0, 2.0), 48_000, 3_500.0);
        let edge = cab_mic_db(3, (0, 1.0, 2.0), 48_000, 3_500.0);
        assert!(edge < centre - 4.0, "{edge} vs {centre}");
    }

    #[test]
    fn the_gate_closes_on_silence_and_opens_on_playing() {
        let rate = 48_000;
        let mut g = Gate::new(rate);
        g.set(true, 10f32.powf(-60.0 / 20.0));
        for v in sine(220.0, rate, 4_800) {
            g.run(v);
        }
        assert!(g.gain() > 0.99);
        // Hiss far under the threshold: fully shut well within a second.
        let mut last = 1.0;
        for i in 0..36_000 {
            let hiss = if i % 2 == 0 { 1e-5 } else { -1e-5 };
            last = g.run(hiss).abs();
        }
        assert!(g.gain() < 1e-3, "{}", g.gain());
        assert!(last < 1e-8);
        // A note opens it again at once.
        for v in sine(220.0, rate, 480) {
            g.run(v);
        }
        assert!(g.gain() > 0.9, "{}", g.gain());
    }

    #[test]
    fn tight_cuts_lows_and_boost_adds_gain() {
        let rate = 48_000;
        let run = |f: f64, tight: f64, boost: f64| {
            let mut b = Boost::new(rate, tight, boost);
            let y: Vec<f32> = sine(f, rate, 24_000).into_iter().map(|v| b.run(v)).collect();
            rms(&y[12_000..])
        };
        assert!(run(60.0, 200.0, 0.0) < run(60.0, 20.0, 0.0) * 0.2);
        assert!(run(720.0, 40.0, 12.0) > run(720.0, 40.0, 0.0) * 2.0);
    }

    #[test]
    fn room_and_double_stay_finite_and_add_width() {
        let rate = 44_100;
        let mut room = Room::new(rate, 1.0, 1.0);
        let mut dbl = Doubler::new(rate, 1.0);
        let mut diff = 0f32;
        for (i, v) in sine(330.0, rate, 44_100).into_iter().enumerate() {
            let v = if i > 30_000 { 0.0 } else { v };
            let (rl, rr) = room.run(v);
            let (dl, dr) = dbl.run(v);
            for y in [rl, rr, dl, dr] {
                assert!(y.is_finite() && y.abs() < 4.0, "{y}");
            }
            diff += (rl + dl - rr - dr).abs();
        }
        assert!(diff > 10.0, "{diff}");
    }
}

#[cfg(test)]
mod pan_tests {
    use super::*;

    fn noise(n: usize) -> Vec<f32> {
        let mut x: u32 = 7;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as f32 / u32::MAX as f32 - 0.5
            })
            .collect()
    }

    #[test]
    fn panned_mics_move_left_and_right_and_centre_is_unchanged() {
        let input = noise(4_800);
        // Centre: the stereo path is the mono path on both sides.
        let mut mono = Cab::new(CabSet::default(), 48_000);
        let mut st = Cab::new(CabSet::default(), 48_000);
        assert!(!st.panned());
        for &x in &input {
            let m = mono.run(x);
            let (l, r) = st.run_stereo(x);
            assert!((l - m).abs() < 1e-6 && (r - m).abs() < 1e-6);
        }
        // Mic A hard left, mic B hard right, half each.
        let set = CabSet { blend: 0.5, pan_a: -1.0, pan_b: 1.0, ..CabSet::default() };
        let mut c = Cab::new(set, 48_000);
        assert!(c.panned());
        let only_a = CabSet { blend: 0.0, pan_a: -1.0, ..CabSet::default() };
        let mut a = Cab::new(only_a, 48_000);
        let (mut el, mut er) = (0.0f32, 0.0f32);
        for &x in &input {
            let (l, r) = a.run_stereo(x);
            el += l * l;
            er += r * r;
            let (l2, r2) = c.run_stereo(x);
            assert!(l2.is_finite() && r2.is_finite());
        }
        assert!(el > 1.0 && er < 1e-6, "mic A alone, hard left: {el} {er}");
        let (gl, gr) = pan_gains(0.0);
        assert!((gl - 1.0).abs() < 1e-6 && (gr - 1.0).abs() < 1e-6);
    }
}
