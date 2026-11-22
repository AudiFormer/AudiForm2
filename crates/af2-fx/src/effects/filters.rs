// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.
// Written fresh for AudiForm 2 from a behaviour description of Audiform's
// Notch Filter, Hum Cut, Precision Filter and DC Offset Fix (whose source carries the GeekCast Radio Network / Altered
// Audio Editor copyright line). The author did not read that source: the
// controls (keys, ranges, defaults, pick names) come from the description
// as a file format, so Audiform's `altaudio.*` slots load, and the
// processing from public filter design. See docs/clean-room.md.

//! Four filters for cleaning up and shaping sound.
//!
//! - Notch Filter: up to six narrow dips, each a peaking section from the
//!   Audio EQ Cookbook, all sharing one width.
//! - Hum Cut: full-depth notches on the mains frequency (60 or 50 Hz) and
//!   its first few overtones.
//! - Precision Filter: low-pass, high-pass, band-pass or band-stop built
//!   from second-order sections, in four classic designs: Butterworth,
//!   Chebyshev type I, Bessel and elliptic (Cauer). The elliptic design is
//!   worked out here with the textbook method from Orfanidis' lecture
//!   notes (Landen's transformation for the Jacobi elliptic functions),
//!   then moved to digital with the bilinear transform.
//! - DC Offset Fix: the classic one-pole DC blocker.
//!
//! [`response_db`] gives the gain the window draws, computed from the very
//! sections the processors run.

use std::f64::consts::PI;

use realfft::num_complex::Complex64;

use super::{choice, num, toggle, EffectInfo, ParamInfo, Params, Processor};
use crate::biquad::{butterworth_qs, Coeffs, Shape, StereoBiquad};

pub const NOTCH: &str = "af.notch";
pub const HUM: &str = "af.hum_cut";
pub const PRECISION: &str = "af.precision_filter";
pub const DC: &str = "af.dc_offset";

pub const MAINS: &[&str] = &["60 Hz (Americas)", "50 Hz (most other places)"];
pub const DESIGNS: &[&str] = &[
    "Butterworth (flattest)",
    "Chebyshev (steepest, some ripple)",
    "Bessel (gentlest, cleanest timing)",
    "Elliptic (steepest of all, ripple in both bands)",
];
pub const MODES: &[&str] = &["Low-pass", "High-pass", "Band-pass", "Band-stop"];
pub const ORDERS: &[&str] = &[
    "2nd order (12 dB/oct)",
    "4th order (24 dB/oct)",
    "6th order (36 dB/oct)",
    "8th order (48 dB/oct)",
];

/// How many notches the Notch Filter has.
const NOTCHES: usize = 6;
const NOTCH_GROUPS: [&str; NOTCHES] = [
    "Notch 1", "Notch 2", "Notch 3", "Notch 4", "Notch 5", "Notch 6",
];
const NOTCH_ON: [&str; NOTCHES] = ["n1_on", "n2_on", "n3_on", "n4_on", "n5_on", "n6_on"];
const NOTCH_FREQ: [&str; NOTCHES] = [
    "n1_freq", "n2_freq", "n3_freq", "n4_freq", "n5_freq", "n6_freq",
];
const NOTCH_DEPTH: [&str; NOTCHES] = [
    "n1_depth", "n2_depth", "n3_depth", "n4_depth", "n5_depth", "n6_depth",
];

/// Each notch's heading and the keys of its switch, frequency and depth.
pub fn notch_keys() -> impl Iterator<Item = (&'static str, [&'static str; 3])> {
    (0..NOTCHES).map(|i| (NOTCH_GROUPS[i], [NOTCH_ON[i], NOTCH_FREQ[i], NOTCH_DEPTH[i]]))
}

pub fn notch_info() -> EffectInfo {
    let mut params: Vec<ParamInfo> = Vec::new();
    for (i, (group, [on, freq, depth])) in notch_keys().enumerate() {
        params.push(toggle(on, "On", i == 0).group(group));
        let f = 1000.0 * (i + 1) as f64;
        params.push(
            num(freq, "Frequency", "Hz", 20.0, 20_000.0, f, 1.0)
                .log()
                .group(group),
        );
        params.push(num(depth, "Depth", "dB", -60.0, 0.0, -30.0, 0.5).group(group));
    }
    params.push(num("width", "Width (Q)", "", 1.0, 100.0, 20.0, 0.5).log());
    EffectInfo {
        id: NOTCH,
        name: "Notch Filter",
        category: "Filter",
        description: "Cuts up to six narrow dips at frequencies you choose, for a whistle, a ring or a single resonance.",
        params,
        sidechain: false,
    }
}

pub fn hum_info() -> EffectInfo {
    EffectInfo {
        id: HUM,
        name: "Hum Cut",
        category: "Filter",
        description: "Removes mains hum by notching out 60 or 50 Hz and its overtones, leaving the rest of the sound alone.",
        params: vec![
            choice("mains", "Mains", MAINS, 0),
            num("harmonics", "Overtones", "", 1.0, 10.0, 4.0, 1.0),
            num("q", "Narrowness", "", 2.0, 60.0, 25.0, 1.0).log(),
        ],
        sidechain: false,
    }
}

pub fn precision_info() -> EffectInfo {
    EffectInfo {
        id: PRECISION,
        name: "Precision Filter",
        category: "Filter",
        description: "A steep low-pass, high-pass, band-pass or band-stop filter in a choice of classic designs and slopes.",
        params: vec![
            choice("design", "Design", DESIGNS, 0),
            choice("mode", "Type", MODES, 1),
            choice("order", "Order", ORDERS, 1),
            num("freq", "Frequency", "Hz", 10.0, 20_000.0, 100.0, 1.0).log(),
            num("freq2", "Upper frequency (bands)", "Hz", 20.0, 20_000.0, 4000.0, 1.0).log(),
            num("ripple", "Ripple (Chebyshev, Elliptic)", "dB", 0.1, 3.0, 0.5, 0.05),
            num("stop", "Stopband depth (Elliptic)", "dB", 20.0, 120.0, 60.0, 1.0),
        ],
        sidechain: false,
    }
}

pub fn dc_info() -> EffectInfo {
    EffectInfo {
        id: DC,
        name: "DC Offset Fix",
        category: "Restoration",
        description: "Takes out a constant offset (DC) from the recording while passing everything you can hear.",
        params: Vec::new(),
        sidechain: false,
    }
}

// ---------------------------------------------------------------------------
// Section lists

/// The Notch Filter's sections: one peaking dip per notch that is on.
fn notch_sections(p: &Params, rate: u32) -> Vec<Coeffs> {
    let q = p.get("width");
    notch_keys()
        .filter(|(_, [on, _, _])| p.on(on))
        .map(|(_, [_, f, d])| Coeffs::new(Shape::Peak, rate, p.get(f), q, p.get(d)))
        .collect()
}

/// Hum Cut's sections: a notch on each overtone below 0.45 of the rate.
fn hum_sections(p: &Params, rate: u32) -> Vec<Coeffs> {
    let base = if p.index("mains") == 1 { 50.0 } else { 60.0 };
    let count = p.get("harmonics").round().max(1.0) as usize;
    let q = p.get("q");
    let limit = 0.45 * f64::from(rate.max(1));
    (1..=count)
        .map(|k| base * k as f64)
        .filter(|&f| f < limit)
        .map(|f| Coeffs::new(Shape::Notch, rate, f, q, 0.0))
        .collect()
}

/// The Precision Filter's sections: `series` runs first; then, for a
/// band-stop, `low` and `high` run side by side and are added.
#[derive(Debug, Clone, Default)]
struct Plan {
    series: Vec<Coeffs>,
    low: Vec<Coeffs>,
    high: Vec<Coeffs>,
}

/// Published Bessel sections (natural frequency, Q), -3 dB at 1 rad/s,
/// for orders 2, 4, 6 and 8.
const BESSEL: [&[(f64, f64)]; 4] = [
    &[(1.2736, 0.5773)],
    &[(1.4192, 0.5219), (1.5912, 0.8055)],
    &[(1.6060, 0.5103), (1.6913, 0.6112), (1.9071, 1.0234)],
    &[
        (1.7837, 0.5060),
        (1.8376, 0.5596),
        (1.9591, 0.7109),
        (2.1953, 1.2257),
    ],
];

/// One low-pass (or high-pass, with `high`) of `n` second-order sections.
fn one_side(
    design: usize,
    high: bool,
    f: f64,
    n: usize,
    ripple: f64,
    stop: f64,
    rate: u32,
) -> Vec<Coeffs> {
    let shape = if high { Shape::HighPass } else { Shape::LowPass };
    // A section at `f` scaled by a prototype natural frequency `w0`.
    let at = |w0: f64| if high { f / w0 } else { f * w0 };
    match design {
        1 => {
            // Chebyshev type I: each section unity at the pass end.
            let order = (2 * n) as f64;
            let eps = (10f64.powf(ripple / 10.0) - 1.0).sqrt();
            let mu = (1.0 / eps).asinh() / order;
            (1..=n)
                .map(|k| {
                    let theta = PI * (2 * k - 1) as f64 / (2.0 * order);
                    let sigma = mu.sinh() * theta.sin();
                    let omega = mu.cosh() * theta.cos();
                    let w0 = sigma.hypot(omega);
                    Coeffs::new(shape, rate, at(w0), w0 / (2.0 * sigma), 0.0)
                })
                .collect()
        }
        2 => BESSEL[(n - 1).min(3)]
            .iter()
            .map(|&(w0, q)| Coeffs::new(shape, rate, at(w0), q, 0.0))
            .collect(),
        3 => elliptic::sections(2 * n, ripple, stop, f, rate, high),
        _ => butterworth_qs(n)
            .into_iter()
            .map(|q| Coeffs::new(shape, rate, f, q, 0.0))
            .collect(),
    }
}

fn precision_plan(p: &Params, rate: u32) -> Plan {
    let design = p.index("design").min(3);
    let n = p.index("order").min(3) + 1;
    let (ripple, stop) = (p.get("ripple"), p.get("stop"));
    let (f1, f2) = (p.get("freq"), p.get("freq2"));
    let (lo, hi) = (f1.min(f2), f1.max(f2));
    let side = |high: bool, f: f64| one_side(design, high, f, n, ripple, stop, rate);
    match p.index("mode") {
        0 => Plan {
            series: side(false, f1),
            ..Plan::default()
        },
        2 => {
            let mut series = side(true, lo);
            series.extend(side(false, hi));
            Plan {
                series,
                ..Plan::default()
            }
        }
        3 => Plan {
            series: Vec::new(),
            low: side(false, lo),
            high: side(true, hi),
        },
        _ => Plan {
            series: side(true, f1),
            ..Plan::default()
        },
    }
}

// ---------------------------------------------------------------------------
// Elliptic (Cauer) design

/// Elliptic low-pass and high-pass design after Orfanidis, "Lecture Notes
/// on Elliptic Filter Design": the Jacobi elliptic functions through
/// Landen's descending transformation, the degree equation for the
/// selectivity, zeros and poles from `cd`, and `v0` from the inverse `sn`.
mod elliptic {
    use super::{Coeffs, PI};
    use realfft::num_complex::Complex64 as C;

    /// The descending Landen moduli of `k`, given with its complement
    /// `kp = sqrt(1 - k^2)` so moduli near 1 keep their precision. Stops
    /// once a modulus is negligible.
    fn landen(k: f64, kp: f64) -> Vec<f64> {
        let mut v = Vec::new();
        let (mut k, mut kp) = (k, kp);
        for _ in 0..64 {
            if k <= 1e-16 {
                break;
            }
            let next = (1.0 - kp) / (1.0 + kp);
            kp = 2.0 * kp.sqrt() / (1.0 + kp);
            k = next;
            v.push(k);
        }
        v
    }

    /// cd(u K, k), with `v` the Landen moduli of k.
    fn cde(u: C, v: &[f64]) -> C {
        let mut w = (u * (PI / 2.0)).cos();
        for &vn in v.iter().rev() {
            w = w * (1.0 + vn) / (1.0 + w * w * vn);
        }
        w
    }

    /// sn(u K, k), with `v` the Landen moduli of k.
    fn sne(u: C, v: &[f64]) -> C {
        let mut w = (u * (PI / 2.0)).sin();
        for &vn in v.iter().rev() {
            w = w * (1.0 + vn) / (1.0 + w * w * vn);
        }
        w
    }

    /// The inverse of [`sne`]: u with sn(u K, k) = w.
    fn asne(w: C, k: f64, v: &[f64]) -> C {
        let mut w = w;
        for (n, &vn) in v.iter().enumerate() {
            let prev = if n == 0 { k } else { v[n - 1] };
            w = w / (1.0 + (C::new(1.0, 0.0) - w * w * (prev * prev)).sqrt()) * (2.0 / (1.0 + vn));
        }
        w.asin() * (2.0 / PI)
    }

    /// The analog prototype (passband edge 1 rad/s) of even order `order`:
    /// its DC gain, its selectivity k (the stopband starts at 1/k rad/s)
    /// and, per second-order section, the zero frequency and the pole
    /// (upper half plane).
    pub(super) fn prototype(order: usize, ripple: f64, stop: f64) -> (f64, f64, Vec<(f64, C)>) {
        let order = order.max(2) & !1;
        let ep = (10f64.powf(ripple / 10.0) - 1.0).sqrt();
        let es = (10f64.powf(stop / 10.0) - 1.0).sqrt();
        let k1 = (ep / es).min(1.0 - 1e-12);
        let k1p = ((1.0 - k1) * (1.0 + k1)).sqrt();
        let half = order / 2;
        let u: Vec<f64> = (1..=half)
            .map(|i| (2 * i - 1) as f64 / order as f64)
            .collect();
        // Degree equation: the selectivity k from the order and k1.
        let v1p = landen(k1p, k1);
        let mut kp = k1p.powi(order as i32);
        for &ui in &u {
            kp *= sne(C::new(ui, 0.0), &v1p).re.powi(4);
        }
        let k = ((1.0 - kp) * (1.0 + kp)).sqrt();
        let vk = landen(k, kp);
        // v0 from the inverse sn of j / ep at modulus k1.
        let v1 = landen(k1, k1p);
        let v0 = asne(C::new(0.0, 1.0 / ep), k1, &v1).im / order as f64;
        let sections = u
            .iter()
            .map(|&ui| {
                let zeta = cde(C::new(ui, 0.0), &vk).re;
                let zero = 1.0 / (k * zeta);
                let pole = C::new(0.0, 1.0) * cde(C::new(ui, -v0), &vk);
                (zero, pole)
            })
            .collect();
        (1.0 / (1.0 + ep * ep).sqrt(), k, sections)
    }

    /// An analog section (b2 s^2 + b1 s + b0) / (a2 s^2 + a1 s + a0) moved
    /// to digital by the bilinear transform at rate `fs`.
    fn bilinear(b: [f64; 3], a: [f64; 3], fs: f64) -> Coeffs {
        let c = 2.0 * fs;
        let c2 = c * c;
        let z = |[x2, x1, x0]: [f64; 3]| {
            (
                x2 * c2 + x1 * c + x0,
                2.0 * (x0 - x2 * c2),
                x2 * c2 - x1 * c + x0,
            )
        };
        let (b0, b1, b2) = z(b);
        let (a0, a1, a2) = z(a);
        Coeffs {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// The digital sections of an elliptic low-pass (or high-pass) with
    /// its passband edge at `f` Hz.
    pub(super) fn sections(
        order: usize,
        ripple: f64,
        stop: f64,
        f: f64,
        rate: u32,
        high: bool,
    ) -> Vec<Coeffs> {
        let fs = f64::from(rate.max(1));
        let f = f.clamp(1e-3, 0.49 * fs);
        let wc = 2.0 * fs * (PI * f / fs).tan();
        let (gain, _, protos) = prototype(order, ripple, stop);
        protos
            .iter()
            .enumerate()
            .map(|(i, &(zero, pole))| {
                let m2 = pole.norm_sqr();
                let re = pole.re;
                let z2 = zero * zero;
                // Each section unity in the passband (DC, or the top).
                let (b, a) = if high {
                    (
                        [m2, 0.0, wc * wc * m2 / z2],
                        [m2, -2.0 * re * wc, wc * wc],
                    )
                } else {
                    (
                        [m2 / z2, 0.0, m2 * wc * wc],
                        [1.0, -2.0 * re * wc, m2 * wc * wc],
                    )
                };
                let c = bilinear(b, a, fs);
                if i == 0 {
                    c.scaled(gain)
                } else {
                    c
                }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Response

/// A section's complex gain at `hz`.
fn h(c: &Coeffs, rate: u32, hz: f64) -> Complex64 {
    let w = 2.0 * PI * hz / f64::from(rate.max(1));
    let z1 = Complex64::from_polar(1.0, -w);
    let z2 = z1 * z1;
    (c.b0 + z1 * c.b1 + z2 * c.b2) / (1.0 + z1 * c.a1 + z2 * c.a2)
}

fn product(cs: &[Coeffs], rate: u32, hz: f64) -> Complex64 {
    cs.iter()
        .fold(Complex64::new(1.0, 0.0), |acc, c| acc * h(c, rate, hz))
}

fn db(g: Complex64) -> f64 {
    20.0 * g.norm().max(1e-15).log10()
}

/// The DC blocker's pole.
fn dc_pole(rate: u32) -> f64 {
    1.0 - 2.0 * PI * 5.0 / f64::from(rate.max(1))
}

/// The gain in dB at `hz` of effect `id` with values `p`, from the same
/// sections it runs (for Curve EQ, its curve). None for other effects.
pub fn response_db(id: &str, p: &Params, rate: u32, hz: f64) -> Option<f64> {
    Some(match id {
        NOTCH => db(product(&notch_sections(p, rate), rate, hz)),
        HUM => db(product(&hum_sections(p, rate), rate, hz)),
        PRECISION => {
            let plan = precision_plan(p, rate);
            let mut g = product(&plan.series, rate, hz);
            if !plan.low.is_empty() || !plan.high.is_empty() {
                g *= product(&plan.low, rate, hz) + product(&plan.high, rate, hz);
            }
            db(g)
        }
        DC => {
            let w = 2.0 * PI * hz / f64::from(rate.max(1));
            let z1 = Complex64::from_polar(1.0, -w);
            db((1.0 - z1) / (1.0 - z1 * dc_pole(rate)))
        }
        super::curve_eq::ID => super::curve_eq::curve_db(&super::curve_eq::points(p), hz),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Processors

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Notch,
    Hum,
}

/// Sections in series: the Notch Filter and Hum Cut.
pub struct Series {
    kind: Kind,
    rate: u32,
    sections: Vec<StereoBiquad>,
}

impl Series {
    pub fn notch(p: &Params, rate: u32) -> Self {
        Self::with(Kind::Notch, p, rate)
    }

    pub fn hum(p: &Params, rate: u32) -> Self {
        Self::with(Kind::Hum, p, rate)
    }

    fn with(kind: Kind, p: &Params, rate: u32) -> Self {
        let mut s = Series {
            kind,
            rate,
            sections: Vec::new(),
        };
        s.sections = s.coeffs(p).into_iter().map(StereoBiquad::new).collect();
        s
    }

    fn coeffs(&self, p: &Params) -> Vec<Coeffs> {
        match self.kind {
            Kind::Notch => notch_sections(p, self.rate),
            Kind::Hum => hum_sections(p, self.rate),
        }
    }
}

impl Processor for Series {
    fn process(&mut self, buf: &mut [f32]) {
        for s in &mut self.sections {
            s.process(buf);
        }
    }

    fn reset(&mut self) {
        self.sections.iter_mut().for_each(StereoBiquad::reset);
    }

    /// New values in place while the number of sections stays the same.
    fn update(&mut self, p: &Params) -> bool {
        let cs = self.coeffs(p);
        if cs.len() != self.sections.len() {
            return false;
        }
        for (s, c) in self.sections.iter_mut().zip(cs) {
            s.set(c);
        }
        true
    }
}

#[inline]
fn run(s: &mut StereoBiquad, c: usize, x: f32) -> f32 {
    if c == 0 {
        s.l.run(x)
    } else {
        s.r.run(x)
    }
}

/// The Precision Filter.
pub struct Precision {
    rate: u32,
    series: Vec<StereoBiquad>,
    low: Vec<StereoBiquad>,
    high: Vec<StereoBiquad>,
}

impl Precision {
    pub fn new(p: &Params, rate: u32) -> Self {
        let plan = precision_plan(p, rate);
        let make = |v: Vec<Coeffs>| v.into_iter().map(StereoBiquad::new).collect();
        Precision {
            rate,
            series: make(plan.series),
            low: make(plan.low),
            high: make(plan.high),
        }
    }
}

impl Processor for Precision {
    fn process(&mut self, buf: &mut [f32]) {
        let split = !self.low.is_empty() || !self.high.is_empty();
        for f in buf.as_chunks_mut::<2>().0 {
            for (c, v) in f.iter_mut().enumerate() {
                let mut x = *v;
                for s in &mut self.series {
                    x = run(s, c, x);
                }
                if split {
                    let mut a = x;
                    for s in &mut self.low {
                        a = run(s, c, a);
                    }
                    let mut b = x;
                    for s in &mut self.high {
                        b = run(s, c, b);
                    }
                    x = a + b;
                }
                *v = x;
            }
        }
    }

    fn reset(&mut self) {
        for s in self
            .series
            .iter_mut()
            .chain(&mut self.low)
            .chain(&mut self.high)
        {
            s.reset();
        }
    }

    /// New coefficients in place, keeping the sections' memory, while the
    /// section counts stay the same; otherwise the host rebuilds.
    fn update(&mut self, p: &Params) -> bool {
        let plan = precision_plan(p, self.rate);
        if plan.series.len() != self.series.len()
            || plan.low.len() != self.low.len()
            || plan.high.len() != self.high.len()
        {
            return false;
        }
        for (s, c) in self
            .series
            .iter_mut()
            .chain(&mut self.low)
            .chain(&mut self.high)
            .zip(plan.series.into_iter().chain(plan.low).chain(plan.high))
        {
            s.set(c);
        }
        true
    }
}

/// The DC blocker: y[n] = x[n] - x[n-1] + R y[n-1], per channel.
pub struct DcFix {
    r: f64,
    x1: [f64; 2],
    y1: [f64; 2],
}

impl DcFix {
    pub fn new(rate: u32) -> Self {
        DcFix {
            r: dc_pole(rate),
            x1: [0.0; 2],
            y1: [0.0; 2],
        }
    }
}

impl Processor for DcFix {
    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.as_chunks_mut::<2>().0 {
            for (c, v) in f.iter_mut().enumerate() {
                let x = f64::from(*v);
                let mut y = x - self.x1[c] + self.r * self.y1[c];
                if y.abs() < 1e-30 {
                    y = 0.0;
                }
                self.x1[c] = x;
                self.y1[c] = y;
                *v = y as f32;
            }
        }
    }

    fn reset(&mut self) {
        self.x1 = [0.0; 2];
        self.y1 = [0.0; 2];
    }

    fn update(&mut self, _p: &Params) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn params(id: &str, list: &[(&str, f64)]) -> Params {
        let saved: Vec<(String, f64)> = list.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        crate::params_from(id, &saved).unwrap()
    }

    fn graph(id: &str, p: &Params, hz: f64) -> f64 {
        response_db(id, p, RATE, hz).unwrap()
    }

    /// The level in dB of a sine at `hz` after the built processor,
    /// measured over whole periods once it has settled.
    fn heard_db(id: &str, p: &Params, rate: u32, hz: f64, secs: f64) -> f64 {
        let mut fx = crate::build(id, rate, p).unwrap();
        let frames = (secs * f64::from(rate)) as usize;
        let amp = 0.25;
        let mut buf: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let v = (amp * (2.0 * PI * hz * i as f64 / f64::from(rate)).sin()) as f32;
                [v, v]
            })
            .collect();
        for block in buf.chunks_mut(512) {
            fx.process(block);
        }
        let periods = ((0.4 * hz).floor()).max(1.0);
        let len = (periods * f64::from(rate) / hz).round() as usize;
        let tail = &buf[(frames - len) * 2..];
        let ms: f64 = tail
            .chunks(2)
            .map(|f| f64::from(f[0]) * f64::from(f[0]))
            .sum::<f64>()
            / len as f64;
        20.0 * ((ms * 2.0).sqrt() / amp).max(1e-12).log10()
    }

    // ----- Notch

    #[test]
    fn notch_defaults_dip_at_one_kilohertz() {
        let p = params(NOTCH, &[]);
        assert!((graph(NOTCH, &p, 1000.0) + 30.0).abs() < 0.01);
        assert!(graph(NOTCH, &p, 300.0).abs() < 0.1);
        assert!(graph(NOTCH, &p, 4000.0).abs() < 0.1);
        let heard = heard_db(NOTCH, &p, RATE, 1000.0, 1.0);
        assert!((heard + 30.0).abs() < 0.5, "heard {heard}");
        let p2 = params(NOTCH, &[("n2_on", 1.0), ("n2_freq", 5000.0), ("n2_depth", -12.0)]);
        assert!((graph(NOTCH, &p2, 5000.0) + 12.0).abs() < 0.05);
        assert!((graph(NOTCH, &p2, 1000.0) + 30.0).abs() < 0.05);
    }

    #[test]
    fn notches_all_off_leave_the_sound_untouched() {
        let p = params(NOTCH, &[("n1_on", 0.0)]);
        let mut fx = crate::build(NOTCH, RATE, &p).unwrap();
        let mut buf: Vec<f32> = (0..4000).map(|i| ((i as f32) * 0.37).sin() * 0.4).collect();
        let dry = buf.clone();
        fx.process(&mut buf);
        assert_eq!(buf, dry);
        assert_eq!(graph(NOTCH, &p, 1000.0), 0.0);
        assert_eq!(notch_keys().count(), 6);
    }

    // ----- Hum

    #[test]
    fn hum_cut_takes_out_the_mains_and_overtones() {
        let p = params(HUM, &[]);
        for hz in [60.0, 120.0, 180.0, 240.0] {
            assert!(graph(HUM, &p, hz) < -25.0, "{hz}");
        }
        assert!(graph(HUM, &p, 300.0) > -3.0);
        assert!(graph(HUM, &p, 1000.0).abs() < 0.1);
        let heard = heard_db(HUM, &p, RATE, 60.0, 3.0);
        assert!(heard < -25.0, "heard at 60 Hz: {heard}");
        let p50 = params(HUM, &[("mains", 1.0)]);
        assert!(graph(HUM, &p50, 100.0) < -25.0);
        assert!(graph(HUM, &p50, 60.0).abs() < 0.5);
        assert!(graph(HUM, &p50, 1000.0).abs() < 0.1);
    }

    #[test]
    fn hum_cut_leaves_out_notches_near_nyquist() {
        let p = params(HUM, &[("harmonics", 10.0)]);
        assert_eq!(hum_sections(&p, 1000).len(), 7, "60..420 Hz below 450");
        assert_eq!(hum_sections(&p, RATE).len(), 10);
    }

    // ----- Precision

    fn prec(list: &[(&str, f64)]) -> Params {
        params(PRECISION, list)
    }

    #[test]
    fn butterworth_is_3_db_down_at_the_cutoff_for_every_order() {
        for order in 0..4 {
            for mode in [0.0, 1.0] {
                let p = prec(&[("mode", mode), ("order", order as f64), ("freq", 1000.0)]);
                let at = graph(PRECISION, &p, 1000.0);
                assert!((at + 3.01).abs() < 0.02, "order {order} mode {mode}: {at}");
            }
            // High-pass: 6 dB per octave per order step of 2 (12, 24, 36, 48).
            let p = prec(&[("mode", 1.0), ("order", order as f64), ("freq", 1000.0)]);
            let slope = graph(PRECISION, &p, 100.0) - graph(PRECISION, &p, 50.0);
            let want = 12.0 * (order + 1) as f64;
            assert!((slope - want).abs() < 0.2, "order {order}: {slope}");
        }
    }

    #[test]
    fn bessel_is_about_3_db_down_at_the_cutoff() {
        for order in 0..4 {
            for mode in [0.0, 1.0] {
                let p = prec(&[
                    ("design", 2.0),
                    ("mode", mode),
                    ("order", order as f64),
                    ("freq", 1000.0),
                ]);
                let at = graph(PRECISION, &p, 1000.0);
                assert!((at + 3.0).abs() < 0.3, "order {order} mode {mode}: {at}");
            }
        }
    }

    fn sweep(lo: f64, hi: f64, n: usize) -> impl Iterator<Item = f64> {
        (0..=n).map(move |i| lo * (hi / lo).powf(i as f64 / n as f64))
    }

    #[test]
    fn chebyshev_ripples_above_zero_and_is_steeper() {
        // At 250 Hz: each cookbook section is prewarped at its own
        // frequency rather than at the cutoff, which near 1 kHz shifts the
        // steep edge enough to dip a few hundredths of a dB under 0.
        for order in 0..4 {
            for ripple in [0.5, 2.0] {
                let p = prec(&[
                    ("design", 1.0),
                    ("mode", 0.0),
                    ("order", order as f64),
                    ("freq", 250.0),
                    ("ripple", ripple),
                ]);
                let at = graph(PRECISION, &p, 250.0);
                assert!(at.abs() < 0.05, "0 dB at the cutoff: {at}");
                let (mut lo, mut hi) = (f64::MAX, f64::MIN);
                for f in sweep(2.5, 250.0, 2000) {
                    let g = graph(PRECISION, &p, f);
                    lo = lo.min(g);
                    hi = hi.max(g);
                }
                assert!(lo > -0.01, "order {order} ripple {ripple}: never below 0: {lo}");
                assert!((hi - ripple).abs() < 0.03, "peaks at +ripple: {hi}");
                let butter = prec(&[("mode", 0.0), ("order", order as f64), ("freq", 250.0)]);
                // Steeper from fourth order up (at second order the two
                // share the same 12 dB/oct and differ only near the edge).
                if order > 0 {
                    assert!(graph(PRECISION, &p, 500.0) < graph(PRECISION, &butter, 500.0) - 3.0);
                }
            }
        }
        // The high-pass mirrors it.
        let p = prec(&[("design", 1.0), ("mode", 1.0), ("freq", 250.0)]);
        assert!(graph(PRECISION, &p, 250.0).abs() < 0.05);
        assert!(graph(PRECISION, &p, 2_500.0) > -0.01);
    }

    #[test]
    fn elliptic_keeps_both_bands_in_bounds() {
        for order in 0..4 {
            for (ripple, stop) in [(0.5, 60.0), (1.0, 40.0), (0.1, 90.0), (3.0, 120.0)] {
                for mode in [0.0, 1.0] {
                    let p = prec(&[
                        ("design", 3.0),
                        ("mode", mode),
                        ("order", order as f64),
                        ("freq", 1000.0),
                        ("ripple", ripple),
                        ("stop", stop),
                    ]);
                    let what = format!("order {order} ripple {ripple} stop {stop} mode {mode}");
                    let edge = graph(PRECISION, &p, 1000.0);
                    assert!((edge + ripple).abs() < 0.02, "{what}: edge {edge}");
                    let pass: Vec<f64> = if mode == 0.0 {
                        sweep(5.0, 1000.0, 3000).collect()
                    } else {
                        sweep(1000.0, 23_000.0, 3000).collect()
                    };
                    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
                    for f in pass {
                        let g = graph(PRECISION, &p, f);
                        lo = lo.min(g);
                        hi = hi.max(g);
                    }
                    assert!(lo > -ripple - 0.01, "{what}: passband low {lo}");
                    assert!(hi < 0.01 && hi > -0.02, "{what}: ripple peaks touch 0: {hi}");
                    // The stopband: from its edge on, at least `stop` down,
                    // and touching -stop there.
                    let (_, k, _) = elliptic::prototype(2 * (order + 1), ripple, stop);
                    let fs = f64::from(RATE);
                    let wc = 2.0 * fs * (PI * 1000.0 / fs).tan();
                    let ws = if mode == 0.0 { wc / k } else { wc * k };
                    let edge_hz = fs / PI * (ws / (2.0 * fs)).atan();
                    let far: Vec<f64> = if mode == 0.0 {
                        sweep(edge_hz * 1.0001, 23_990.0, 3000)
                            .filter(|&f| f > edge_hz && f < 23_995.0)
                            .collect()
                    } else {
                        sweep(1.0, edge_hz * 0.9999, 3000).collect()
                    };
                    let mut top = f64::MIN;
                    for f in far {
                        let g = graph(PRECISION, &p, f);
                        top = top.max(g);
                        assert!(g < -stop + 0.01, "{what}: {g} dB at {f} Hz");
                    }
                    if top > f64::MIN {
                        assert!(top > -stop - 0.5, "{what}: stopband peaks reach -stop: {top}");
                    }
                }
            }
        }
    }

    #[test]
    fn elliptic_is_steepest() {
        let at = |design: f64| {
            let p = prec(&[("design", design), ("mode", 0.0), ("freq", 1000.0)]);
            graph(PRECISION, &p, 2000.0)
        };
        assert!(at(3.0) < at(1.0) && at(1.0) < at(0.0) && at(0.0) < at(2.0));
    }

    #[test]
    fn band_pass_and_band_stop() {
        let bp = prec(&[("mode", 2.0), ("freq", 4000.0), ("freq2", 100.0)]);
        assert!(graph(PRECISION, &bp, 632.0).abs() < 0.1);
        assert!(graph(PRECISION, &bp, 20.0) < -20.0);
        assert!(graph(PRECISION, &bp, 16_000.0) < -20.0);
        let bs = prec(&[("mode", 3.0), ("freq", 100.0), ("freq2", 4000.0)]);
        assert!(graph(PRECISION, &bs, 632.0) < -40.0);
        assert!(graph(PRECISION, &bs, 15.0).abs() < 0.1);
        assert!(graph(PRECISION, &bs, 18_000.0).abs() < 0.1);
    }

    #[test]
    fn every_section_is_stable() {
        let mut checked = 0;
        for rate in [44_100u32, 48_000, 96_000] {
            for design in 0..4 {
                for mode in 0..4 {
                    for order in 0..4 {
                        for freq in [10.0, 100.0, 1000.0, 10_000.0, 20_000.0] {
                            for (ripple, stop) in [(0.1, 20.0), (3.0, 120.0), (0.5, 60.0)] {
                                let p = prec(&[
                                    ("design", design as f64),
                                    ("mode", mode as f64),
                                    ("order", order as f64),
                                    ("freq", freq),
                                    ("freq2", 4000.0),
                                    ("ripple", ripple),
                                    ("stop", stop),
                                ]);
                                let plan = precision_plan(&p, rate);
                                for c in plan.series.iter().chain(&plan.low).chain(&plan.high) {
                                    let ok = [c.b0, c.b1, c.b2, c.a1, c.a2]
                                        .iter()
                                        .all(|v| v.is_finite())
                                        && c.a2.abs() < 1.0
                                        && c.a1.abs() < 1.0 + c.a2;
                                    assert!(ok, "{rate} {design} {mode} {order} {freq}: {c:?}");
                                    checked += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(checked > 1000);
    }

    #[test]
    fn what_is_heard_matches_the_graph() {
        let settings: &[&[(&str, f64)]] = &[
            &[],
            &[("mode", 0.0), ("freq", 1000.0), ("order", 3.0)],
            &[("design", 1.0), ("mode", 0.0), ("freq", 2000.0), ("ripple", 2.0)],
            &[("design", 2.0), ("mode", 1.0), ("freq", 500.0), ("order", 2.0)],
            &[("design", 3.0), ("mode", 0.0), ("freq", 1000.0), ("order", 3.0)],
            &[("design", 3.0), ("mode", 1.0), ("freq", 300.0), ("ripple", 1.0)],
            &[("mode", 2.0), ("freq", 200.0), ("freq2", 3000.0)],
            &[("design", 3.0), ("mode", 3.0), ("freq", 300.0), ("freq2", 3000.0)],
            &[("mode", 3.0), ("freq", 100.0), ("freq2", 4000.0)],
        ];
        let freqs = [59.0, 110.0, 290.0, 590.0, 1010.0, 1900.0, 5900.0, 13_100.0];
        for set in settings {
            let p = prec(set);
            for hz in freqs {
                let drawn = graph(PRECISION, &p, hz);
                if drawn < -24.0 {
                    continue;
                }
                let heard = heard_db(PRECISION, &p, RATE, hz, 1.5);
                assert!(
                    (heard - drawn).abs() < 0.15,
                    "{set:?} at {hz}: heard {heard}, drawn {drawn}"
                );
            }
        }
        // Notch and hum too.
        let n = params(NOTCH, &[("n2_on", 1.0), ("n2_depth", -6.0)]);
        for hz in [990.0, 1900.0, 2010.0, 5900.0] {
            let (d, h) = (graph(NOTCH, &n, hz), heard_db(NOTCH, &n, RATE, hz, 1.0));
            if d > -24.0 {
                assert!((d - h).abs() < 0.15, "notch at {hz}: {h} vs {d}");
            }
        }
    }

    #[test]
    fn update_keeps_the_sections_when_the_shape_stays() {
        let p = prec(&[]);
        let mut fx = Precision::new(&p, RATE);
        assert!(fx.update(&prec(&[("freq", 300.0), ("design", 3.0)])));
        assert!(!fx.update(&prec(&[("order", 3.0)])));
        assert!(!fx.update(&prec(&[("mode", 3.0)])));
        let mut bs = Precision::new(&prec(&[("mode", 3.0)]), RATE);
        assert!(bs.update(&prec(&[("mode", 3.0), ("freq", 50.0)])));
        let mut n = Series::notch(&params(NOTCH, &[]), RATE);
        assert!(n.update(&params(NOTCH, &[("n1_freq", 500.0)])));
        assert!(!n.update(&params(NOTCH, &[("n2_on", 1.0)])));
    }

    // ----- DC

    #[test]
    fn dc_offset_is_removed_and_tones_pass() {
        let mut fx = crate::build(DC, RATE, &params(DC, &[])).unwrap();
        let frames = 2 * RATE as usize;
        let mut buf: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let v = 0.3 + 0.5 * (2.0 * PI * 200.0 * i as f64 / f64::from(RATE)).sin();
                [v as f32, v as f32]
            })
            .collect();
        for block in buf.chunks_mut(512) {
            fx.process(block);
        }
        let tail = &buf[RATE as usize * 2..];
        let mean = tail.iter().map(|&v| f64::from(v)).sum::<f64>() / tail.len() as f64;
        assert!(mean.abs() < 1e-3, "offset left: {mean}");
        for hz in [200.0, 30.0] {
            let d = graph(DC, &Params::default(), hz);
            assert!(d.abs() < 0.2, "{hz}: {d}");
            let h = heard_db(DC, &Params::default(), RATE, hz, 1.5);
            assert!(h.abs() < 0.2, "{hz}: heard {h}");
        }
        assert!(response_db("af.nope", &Params::default(), RATE, 100.0).is_none());
    }
}
