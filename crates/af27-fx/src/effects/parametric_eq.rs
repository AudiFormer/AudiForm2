// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Parametric EQ: seven gain bands (a low shelf, five peaking bells and a
//! high shelf) between a high-pass and a low-pass filter, plus an overall
//! output gain.
//!
//! Every band is built from the public "Audio EQ Cookbook" sections in
//! [`crate::biquad`]. The pass filters are Butterworth: 6 dB/oct is one
//! first-order section, 12 to 48 dB/oct are cascades of second-order
//! sections with the Butterworth Qs (18 dB/oct, order 3, is a first-order
//! section and one with Q 1).
//!
//! Settings glide: frequency (in pitch), gain and Q move toward a new
//! value over about 20 ms, the coefficients following in steps of 16
//! frames, so drags and automation never zipper. A band switched off
//! glides to 0 dB (a pass filter fades out) and is then skipped, so it
//! is exactly transparent. The keys are the ones Audiform saves for its
//! Parametric EQ (`altaudio.parametric_eq`); the shelves' Q is new.

use super::{choice, num, toggle, EffectInfo, Params, Processor};
use crate::biquad::{Biquad, Coeffs, Shape};
use std::f64::consts::PI;

pub const ID: &str = "af.parametric_eq";

/// What a band does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    HighPass,
    LowShelf,
    Bell,
    HighShelf,
    LowPass,
}

/// One band: its kind, names and the keys of its controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub kind: Kind,
    /// The short label drawn on its node (HP, L, 1 to 5, H, LP).
    pub label: &'static str,
    /// Its name, as its controls are grouped.
    pub name: &'static str,
    pub on: &'static str,
    pub freq: &'static str,
    /// Shelves and bells.
    pub gain: Option<&'static str>,
    /// Shelves and bells.
    pub q: Option<&'static str>,
    /// The pass filters: an index into [`SLOPES`].
    pub slope: Option<&'static str>,
}

const fn pass(kind: Kind, label: &'static str, name: &'static str, k: [&'static str; 3]) -> Band {
    Band {
        kind,
        label,
        name,
        on: k[0],
        freq: k[1],
        gain: None,
        q: None,
        slope: Some(k[2]),
    }
}

const fn gained(kind: Kind, label: &'static str, name: &'static str, k: [&'static str; 4]) -> Band {
    Band {
        kind,
        label,
        name,
        on: k[0],
        freq: k[1],
        gain: Some(k[2]),
        q: Some(k[3]),
        slope: None,
    }
}

/// Every band, low to high.
pub const BANDS: [Band; 9] = [
    pass(
        Kind::HighPass,
        "HP",
        "High-pass",
        ["hp_on", "hp_freq", "hp_slope"],
    ),
    gained(
        Kind::LowShelf,
        "L",
        "Low shelf",
        ["ls_on", "ls_freq", "ls_gain", "ls_q"],
    ),
    gained(
        Kind::Bell,
        "1",
        "Band 1",
        ["b1_on", "b1_freq", "b1_gain", "b1_q"],
    ),
    gained(
        Kind::Bell,
        "2",
        "Band 2",
        ["b2_on", "b2_freq", "b2_gain", "b2_q"],
    ),
    gained(
        Kind::Bell,
        "3",
        "Band 3",
        ["b3_on", "b3_freq", "b3_gain", "b3_q"],
    ),
    gained(
        Kind::Bell,
        "4",
        "Band 4",
        ["b4_on", "b4_freq", "b4_gain", "b4_q"],
    ),
    gained(
        Kind::Bell,
        "5",
        "Band 5",
        ["b5_on", "b5_freq", "b5_gain", "b5_q"],
    ),
    gained(
        Kind::HighShelf,
        "H",
        "High shelf",
        ["hs_on", "hs_freq", "hs_gain", "hs_q"],
    ),
    pass(
        Kind::LowPass,
        "LP",
        "Low-pass",
        ["lp_on", "lp_freq", "lp_slope"],
    ),
];

/// The pass filters' slopes, by saved index. The first three are the ones
/// Audiform saves (0, 1, 2); the rest were added after them.
pub const SLOPES: [&str; 6] = [
    "12 dB/oct",
    "24 dB/oct",
    "48 dB/oct",
    "6 dB/oct",
    "18 dB/oct",
    "36 dB/oct",
];

/// The filter order (6 dB/oct each) of each slope in [`SLOPES`].
const ORDERS: [usize; 6] = [2, 4, 8, 1, 3, 6];

/// The slope options in steepness order: indices into [`SLOPES`].
pub const SLOPE_ORDER: [usize; 6] = [3, 0, 4, 1, 5, 2];

/// The overall output gain's key.
pub const GAIN: &str = "gain";

pub fn info() -> EffectInfo {
    let mut params = vec![num(GAIN, "Output", "dB", -40.0, 20.0, 0.0, 0.1).group("Output")];
    let defaults: [(f64, f64); 9] = [
        (80.0, 0.0),
        (100.0, 0.0),
        (200.0, 0.0),
        (500.0, 0.0),
        (1200.0, 0.0),
        (3000.0, 0.0),
        (7000.0, 0.0),
        (10_000.0, 0.0),
        (18_000.0, 0.0),
    ];
    for (b, (f, _)) in BANDS.iter().zip(defaults) {
        let g = b.name;
        match b.kind {
            Kind::HighPass | Kind::LowPass => {
                let (lo, hi) = if b.kind == Kind::HighPass {
                    (10.0, 2000.0)
                } else {
                    (1000.0, 22_000.0)
                };
                params.push(toggle(b.on, "On", false).group(g));
                params.push(
                    num(b.freq, "Frequency", "Hz", lo, hi, f, 1.0)
                        .log()
                        .group(g),
                );
                params.push(choice(b.slope.unwrap(), "Slope", &SLOPES, 1).group(g));
            }
            _ => {
                let (lo, hi) = if b.kind == Kind::HighShelf {
                    (1000.0, 20_000.0)
                } else {
                    (20.0, 20_000.0)
                };
                let (qlo, qhi, qd) = if b.kind == Kind::Bell {
                    (0.1, 100.0, 1.0)
                } else {
                    (0.1, 10.0, std::f64::consts::FRAC_1_SQRT_2)
                };
                params.push(toggle(b.on, "On", true).group(g));
                params.push(
                    num(b.freq, "Frequency", "Hz", lo, hi, f, 1.0)
                        .log()
                        .group(g),
                );
                params.push(num(b.gain.unwrap(), "Gain", "dB", -24.0, 24.0, 0.0, 0.1).group(g));
                params.push(
                    num(b.q.unwrap(), "Q", "", qlo, qhi, qd, 0.01)
                        .log()
                        .group(g),
                );
            }
        }
    }
    EffectInfo {
        id: ID,
        sidechain: false,
        name: "Parametric EQ",
        category: "EQ",
        description: "Seven bands (low shelf, five bells, high shelf) between a high-pass and a low-pass filter (6 to 48 dB/oct), with an output gain. Opens on a graph to drag.",
        params,
    }
}

/// The most sections a band uses (a 48 dB/oct pass filter).
const MAX_SECTIONS: usize = 4;

/// One band's sections at these settings: up to four, and how many.
fn sections(
    kind: Kind,
    rate: u32,
    freq: f64,
    gain: f64,
    q: f64,
    slope: usize,
) -> ([Coeffs; MAX_SECTIONS], usize) {
    let mut out = [Coeffs::FLAT; MAX_SECTIONS];
    match kind {
        Kind::LowShelf => out[0] = Coeffs::new(Shape::LowShelf, rate, freq, q, gain),
        Kind::Bell => out[0] = Coeffs::new(Shape::Peak, rate, freq, q, gain),
        Kind::HighShelf => out[0] = Coeffs::new(Shape::HighShelf, rate, freq, q, gain),
        Kind::HighPass | Kind::LowPass => {
            let high = kind == Kind::HighPass;
            let order = ORDERS[slope.min(ORDERS.len() - 1)];
            let mut n = 0;
            if order % 2 == 1 {
                out[n] = first_order(high, rate, freq);
                n += 1;
            }
            let pairs = order / 2;
            let shape = if high {
                Shape::HighPass
            } else {
                Shape::LowPass
            };
            for k in 0..pairs {
                // Butterworth pole pairs: Q = 1 / (2 sin(theta)), with the
                // angles of an order-`order` filter's complex poles.
                let q = if order % 2 == 1 {
                    1.0 / (2.0 * ((k + 1) as f64 * PI / order as f64).cos())
                } else {
                    1.0 / (2.0 * ((2 * k + 1) as f64 * PI / (2.0 * order as f64)).sin())
                };
                out[n] = Coeffs::new(shape, rate, freq, q, 0.0);
                n += 1;
            }
            return (out, n);
        }
    }
    (out, 1)
}

/// A first-order (6 dB/oct) Butterworth high- or low-pass, by the
/// bilinear transform with the cutoff prewarped.
fn first_order(high: bool, rate: u32, freq: f64) -> Coeffs {
    let fs = f64::from(rate.max(1));
    let k = (PI * freq.clamp(1e-3, fs * 0.4999) / fs).tan();
    let a1 = (k - 1.0) / (k + 1.0);
    let (b0, b1) = if high {
        (1.0 / (1.0 + k), -1.0 / (1.0 + k))
    } else {
        (k / (1.0 + k), k / (1.0 + k))
    };
    Coeffs {
        b0,
        b1,
        b2: 0.0,
        a1,
        a2: 0.0,
    }
}

/// Band `b`'s settled response at `freq` Hz in dB (0 when it is off).
pub fn band_response_db(p: &Params, rate: u32, b: usize, freq: f64) -> f64 {
    let Some(band) = BANDS.get(b) else {
        return 0.0;
    };
    if !p.on(band.on) {
        return 0.0;
    }
    let gain = band.gain.map_or(0.0, |k| p.get(k));
    let q = band.q.map_or(1.0, |k| p.get(k));
    let slope = band.slope.map_or(0, |k| p.index(k));
    let (cs, n) = sections(band.kind, rate, p.get(band.freq), gain, q, slope);
    cs[..n].iter().map(|c| c.response_db(rate, freq)).sum()
}

/// The whole EQ's response at `freq` Hz in dB: every band on, plus the
/// output gain.
pub fn response_db(p: &Params, rate: u32, freq: f64) -> f64 {
    (0..BANDS.len())
        .map(|b| band_response_db(p, rate, b, freq))
        .sum::<f64>()
        + p.get(GAIN)
}

/// Frames between coefficient updates while a setting glides.
const STEP: usize = 16;

/// One band while running.
struct Running {
    band: Band,
    /// Wanted: frequency (log2 Hz), gain (dB, 0 when off), Q, the pass
    /// filter's level (1 on, 0 off) and slope.
    want: [f64; 4],
    slope: usize,
    /// Where the glide is now.
    cur: [f64; 4],
    /// The slope the sections were made for.
    built_slope: usize,
    /// The sections, left and right, and how many are used.
    l: [Biquad; MAX_SECTIONS],
    r: [Biquad; MAX_SECTIONS],
    n: usize,
    /// Running (false once a band has settled off or flat: skipped).
    live: bool,
    dirty: bool,
}

impl Running {
    fn new(band: Band) -> Self {
        Running {
            band,
            want: [0.0; 4],
            slope: 0,
            cur: [0.0; 4],
            built_slope: usize::MAX,
            l: [Biquad::new(Coeffs::FLAT); MAX_SECTIONS],
            r: [Biquad::new(Coeffs::FLAT); MAX_SECTIONS],
            n: 0,
            live: false,
            dirty: true,
        }
    }

    fn pass(&self) -> bool {
        matches!(self.band.kind, Kind::HighPass | Kind::LowPass)
    }

    fn take(&mut self, p: &Params) {
        let b = &self.band;
        let on = p.on(b.on);
        let freq = p.get(b.freq).max(1.0).log2();
        let gain = match b.gain {
            Some(k) if on => p.get(k),
            _ => 0.0,
        };
        let q = b.q.map_or(1.0, |k| p.get(k).max(1e-3));
        let level = if self.pass() && on { 1.0 } else { 0.0 };
        let want = [freq, gain, q, level];
        if want != self.want {
            self.want = want;
            self.dirty = true;
        }
        let slope = b.slope.map_or(0, |k| p.index(k));
        if slope != self.slope {
            self.slope = slope;
            self.dirty = true;
        }
    }

    /// Jumps to the wanted settings.
    fn settle(&mut self, rate: u32) {
        self.cur = self.want;
        self.dirty = true;
        self.glide(rate, 1.0);
    }

    /// Moves `k` of the way to the wanted settings and remakes the
    /// sections when anything moved.
    fn glide(&mut self, rate: u32, k: f64) {
        if !self.dirty {
            return;
        }
        let mut moving = false;
        // The pass filters' level (the last) moves per frame, in `run`.
        for (c, w) in self.cur.iter_mut().zip(self.want).take(3) {
            let d = w - *c;
            // Close enough to snap: 0.001 octave, 0.001 dB, 0.1% of Q.
            if d.abs() < 1e-3 * w.abs().max(1.0) {
                *c = w;
            } else {
                *c += d * k;
                moving = true;
            }
        }
        let was_live = self.live;
        self.live = if self.pass() {
            moving |= self.cur[3] != self.want[3];
            self.cur[3] > 0.0 || self.want[3] > 0.0
        } else {
            self.cur[1] != 0.0
        };
        if self.live {
            let (cs, n) = sections(
                self.band.kind,
                rate,
                self.cur[0].exp2(),
                self.cur[1],
                self.cur[2],
                self.slope,
            );
            if self.built_slope != self.slope || !was_live {
                for s in self.l.iter_mut().chain(self.r.iter_mut()) {
                    s.reset();
                }
                self.built_slope = self.slope;
            }
            for ((l, r), c) in self.l.iter_mut().zip(self.r.iter_mut()).zip(&cs[..n]) {
                l.set(*c);
                r.set(*c);
            }
            self.n = n;
        }
        self.dirty = moving;
    }

    #[inline]
    fn run(&mut self, buf: &mut [f32], level_step: f64) {
        if !self.live {
            return;
        }
        let n = self.n;
        if self.pass() {
            // The filter fades in and out against the dry sound.
            let mut m = self.cur[3];
            let want = self.want[3];
            for f in buf.as_chunks_mut::<2>().0 {
                m += (want - m) * level_step;
                let (mut l, mut r) = (f[0], f[1]);
                for (sl, sr) in self.l[..n].iter_mut().zip(self.r[..n].iter_mut()) {
                    l = sl.run(l);
                    r = sr.run(r);
                }
                let m32 = m as f32;
                f[0] += (l - f[0]) * m32;
                f[1] += (r - f[1]) * m32;
            }
            if (want - m).abs() < 1e-4 {
                m = want;
            }
            self.cur[3] = m;
            if m != want || m == 0.0 {
                // Still fading (or faded out: settle, then skip).
                self.dirty = true;
            }
        } else {
            for f in buf.as_chunks_mut::<2>().0 {
                let (mut l, mut r) = (f[0], f[1]);
                for (sl, sr) in self.l[..n].iter_mut().zip(self.r[..n].iter_mut()) {
                    l = sl.run(l);
                    r = sr.run(r);
                }
                f[0] = l;
                f[1] = r;
            }
        }
    }
}

pub struct ParametricEq {
    rate: u32,
    bands: [Running; 9],
    gain: f32,
    gain_want: f32,
    /// Per coefficient step and per frame glide factors (about 20 ms).
    k_step: f64,
    k_frame: f64,
}

impl ParametricEq {
    pub fn new(p: &Params, rate: u32) -> Self {
        let rate = rate.max(1);
        let tau = 0.02 * f64::from(rate);
        let mut eq = ParametricEq {
            rate,
            bands: BANDS.map(Running::new),
            gain: 1.0,
            gain_want: 1.0,
            k_step: 1.0 - (-(STEP as f64) / tau).exp(),
            k_frame: 1.0 - (-1.0 / tau).exp(),
        };
        eq.take(p);
        for b in &mut eq.bands {
            b.settle(rate);
        }
        eq.gain = eq.gain_want;
        eq
    }

    fn take(&mut self, p: &Params) {
        for b in &mut self.bands {
            b.take(p);
        }
        self.gain_want = super::db_to_gain(p.get(GAIN));
    }
}

impl Processor for ParametricEq {
    fn process(&mut self, buf: &mut [f32]) {
        let k_frame = self.k_frame as f32;
        for block in buf.chunks_mut(STEP * 2) {
            for b in &mut self.bands {
                b.glide(self.rate, self.k_step);
                b.run(block, self.k_frame);
            }
            let want = self.gain_want;
            if self.gain == want {
                if want != 1.0 {
                    for s in block.iter_mut() {
                        *s *= want;
                    }
                }
            } else {
                for f in block.as_chunks_mut::<2>().0 {
                    self.gain += (want - self.gain) * k_frame;
                    f[0] *= self.gain;
                    f[1] *= self.gain;
                }
                if (want - self.gain).abs() < 1e-6 {
                    self.gain = want;
                }
            }
        }
    }

    fn reset(&mut self) {
        for b in &mut self.bands {
            for s in b.l.iter_mut().chain(b.r.iter_mut()) {
                s.reset();
            }
            b.settle(self.rate);
        }
        self.gain = self.gain_want;
    }

    fn update(&mut self, p: &Params) -> bool {
        self.take(p);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build, params_from};

    const RATE: u32 = 48_000;

    fn params(set: &[(&str, f64)]) -> Params {
        let pairs: Vec<(String, f64)> = set.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        params_from(ID, &pairs).unwrap()
    }

    /// The level change (dB) a sine at `freq` gets, measured after the
    /// filter has settled.
    fn measured_db(p: &Params, freq: f64) -> f64 {
        let mut fx = build(ID, RATE, p).unwrap();
        let n = RATE as usize;
        let mut buf: Vec<f32> = (0..n)
            .flat_map(|i| {
                let s = (2.0 * PI * freq * i as f64 / f64::from(RATE)).sin() as f32 * 0.25;
                [s, s]
            })
            .collect();
        for b in buf.chunks_mut(1024) {
            fx.process(b);
        }
        let tail = &buf[n..];
        let rms =
            (tail.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / tail.len() as f64).sqrt();
        20.0 * (rms / (0.25 / 2f64.sqrt())).log10()
    }

    fn all_off() -> Vec<(&'static str, f64)> {
        BANDS.iter().map(|b| (b.on, 0.0)).collect()
    }

    fn only(set: &[(&'static str, f64)]) -> Params {
        let mut v = all_off();
        v.extend_from_slice(set);
        params(&v)
    }

    #[test]
    fn info_lists_every_band_key_with_audiform_names() {
        let i = info();
        for b in BANDS {
            for k in [Some(b.on), Some(b.freq), b.gain, b.q, b.slope]
                .into_iter()
                .flatten()
            {
                assert!(i.param(k).is_some(), "{k}");
            }
        }
        assert!(i.param("gain").is_some());
        assert_eq!(i.params.len(), 1 + 2 * 3 + 7 * 4);
    }

    #[test]
    fn a_bell_reaches_its_gain_at_its_frequency() {
        for (g, q) in [(9.0, 1.0), (-12.0, 4.0), (18.0, 0.3)] {
            let p = only(&[
                ("b3_on", 1.0),
                ("b3_freq", 1000.0),
                ("b3_gain", g),
                ("b3_q", q),
            ]);
            let want = response_db(&p, RATE, 1000.0);
            assert!((want - g).abs() < 0.01, "{want} vs {g}");
            let got = measured_db(&p, 1000.0);
            assert!((got - g).abs() < 0.1, "bell {g} dB Q {q}: measured {got}");
            // Elsewhere the sound follows the curve the graph draws.
            for f in [40.0, 400.0, 2500.0, 12_000.0] {
                let (m, r) = (measured_db(&p, f), response_db(&p, RATE, f));
                assert!((m - r).abs() < 0.1, "{g} {q} at {f}: {m} vs {r}");
            }
        }
    }

    #[test]
    fn shelves_reach_half_their_gain_at_their_frequency() {
        let p = only(&[("ls_on", 1.0), ("ls_freq", 200.0), ("ls_gain", 10.0)]);
        assert!((response_db(&p, RATE, 200.0) - 5.0).abs() < 0.01);
        assert!((measured_db(&p, 200.0) - 5.0).abs() < 0.1);
        assert!((measured_db(&p, 20.0) - 10.0).abs() < 0.3);
        assert!(measured_db(&p, 8000.0).abs() < 0.1);
        let p = only(&[("hs_on", 1.0), ("hs_freq", 4000.0), ("hs_gain", -8.0)]);
        assert!((response_db(&p, RATE, 4000.0) + 4.0).abs() < 0.01);
        assert!((measured_db(&p, 4000.0) + 4.0).abs() < 0.1);
        assert!((measured_db(&p, 20_000.0) + 8.0).abs() < 0.4);
        assert!(measured_db(&p, 100.0).abs() < 0.1);
    }

    #[test]
    fn pass_filters_are_3_db_down_at_the_cutoff_at_every_slope() {
        for (s, order) in ORDERS.iter().enumerate() {
            let hp = only(&[("hp_on", 1.0), ("hp_freq", 200.0), ("hp_slope", s as f64)]);
            let at = measured_db(&hp, 200.0);
            assert!((at + 3.01).abs() < 0.15, "HP order {order}: {at}");
            // An octave below: about 6 dB per order down (Butterworth).
            let below = response_db(&hp, RATE, 100.0);
            let want = -10.0 * (1.0 + 4f64.powi(*order as i32)).log10();
            assert!(
                (below - want).abs() < 0.3,
                "HP order {order}: {below} vs {want}"
            );
            assert!(measured_db(&hp, 5000.0).abs() < 0.1);
            let lp = only(&[("lp_on", 1.0), ("lp_freq", 2000.0), ("lp_slope", s as f64)]);
            let at = measured_db(&lp, 2000.0);
            assert!((at + 3.01).abs() < 0.15, "LP order {order}: {at}");
            assert!(measured_db(&lp, 100.0).abs() < 0.1);
        }
    }

    #[test]
    fn bands_off_or_flat_are_transparent() {
        let input = crate::testing::stereo_sine(RATE, 4800);
        for p in [
            params(&[]),
            only(&[]),
            // Settings on bands switched off are ignored.
            params(&[
                ("hp_on", 0.0),
                ("hp_freq", 1500.0),
                ("ls_on", 0.0),
                ("ls_gain", 12.0),
                ("b2_on", 0.0),
                ("b2_gain", -20.0),
                ("lp_on", 0.0),
                ("lp_freq", 1000.0),
            ]),
        ] {
            let mut fx = build(ID, RATE, &p).unwrap();
            let mut buf = input.clone();
            fx.process(&mut buf);
            assert_eq!(buf, input);
        }
        // Switching a band off glides it away, then it is exactly out.
        let mut p = only(&[("b1_on", 1.0), ("b1_gain", 12.0)]);
        let mut fx = build(ID, RATE, &p).unwrap();
        let mut buf = input.clone();
        fx.process(&mut buf);
        p.set("b1_on", 0.0);
        assert!(fx.update(&p));
        for _ in 0..10 {
            let mut b = input.clone();
            fx.process(&mut b);
        }
        let mut b = input.clone();
        fx.process(&mut b);
        assert_eq!(b, input);
    }

    #[test]
    fn a_pass_filter_switched_on_while_running_fades_in_fully() {
        let mut p = only(&[("hp_freq", 1000.0), ("hp_slope", 1.0)]);
        let mut fx = build(ID, RATE, &p).unwrap();
        let tone = |n: usize| -> Vec<f32> {
            (0..n)
                .flat_map(|i| {
                    let s = (2.0 * PI * 1000.0 * i as f64 / f64::from(RATE)).sin() as f32;
                    [s, s]
                })
                .collect()
        };
        let mut b = tone(4800);
        fx.process(&mut b);
        p.set("hp_on", 1.0);
        fx.update(&p);
        let mut b = tone(48_000);
        for blk in b.chunks_mut(512) {
            fx.process(blk);
        }
        let tail = &b[b.len() - 9600..];
        let rms =
            (tail.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / tail.len() as f64).sqrt();
        let db = 20.0 * (rms * 2f64.sqrt()).log10();
        assert!((db + 3.01).abs() < 0.15, "{db}");
    }

    #[test]
    fn output_gain_scales() {
        let p = only(&[("gain", -6.0)]);
        assert!((measured_db(&p, 1000.0) + 6.0).abs() < 0.01);
    }

    #[test]
    fn changes_glide_without_jumps() {
        // A sweep of a big bell while a sine plays: no sample-to-sample
        // step larger than the sine itself can make.
        let mut p = only(&[("b2_on", 1.0), ("b2_freq", 200.0), ("b2_gain", 18.0)]);
        let mut fx = build(ID, RATE, &p).unwrap();
        let mut buf = crate::testing::stereo_sine(RATE, RATE as usize);
        for (i, blk) in buf.chunks_mut(512).enumerate() {
            if i % 4 == 0 {
                let f = 200.0 * 2f64.powf(i as f64 / 20.0);
                p.set("b2_freq", f.min(15_000.0));
                p.set("b2_gain", if i % 8 == 0 { 18.0 } else { -18.0 });
                fx.update(&p);
            }
            fx.process(blk);
        }
        let max_step = buf
            .chunks(2)
            .zip(buf.chunks(2).skip(1))
            .map(|(a, b)| (b[0] - a[0]).abs())
            .fold(0.0f32, f32::max);
        // 440 Hz at 0.5 boosted 18 dB (x8): the steepest a sine can be.
        let bound = 0.5 * 8.0 * 2.0 * std::f32::consts::PI * 440.0 / RATE as f32;
        assert!(max_step < bound * 1.5, "{max_step} vs {bound}");
    }

    #[test]
    fn stable_at_extreme_settings() {
        for rate in [22_050, 44_100, 48_000, 96_000, 192_000] {
            let mut set: Vec<(String, f64)> = vec![("gain".into(), 20.0)];
            for b in BANDS {
                set.push((b.on.into(), 1.0));
            }
            for (k, f, g, q) in [
                ("ls", 20.0, 24.0, 10.0),
                ("b1", 20.0, 24.0, 100.0),
                ("b2", 20_000.0, -24.0, 100.0),
                ("b3", 20_000.0, 24.0, 0.1),
                ("b4", 20.0, -24.0, 0.1),
                ("b5", 11_000.0, 24.0, 100.0),
                ("hs", 20_000.0, 24.0, 10.0),
            ] {
                set.extend([
                    (format!("{k}_freq"), f),
                    (format!("{k}_gain"), g),
                    (format!("{k}_q"), q),
                ]);
            }
            for (hp, lp, slope) in [(10.0, 22_000.0, 2.0), (2000.0, 1000.0, 5.0)] {
                let mut s = set.clone();
                for (k, v) in [
                    ("hp_freq", hp),
                    ("lp_freq", lp),
                    ("hp_slope", slope),
                    ("lp_slope", slope),
                ] {
                    s.push((k.into(), v));
                }
                let p = params_from(ID, &s).unwrap();
                let mut fx = build(ID, rate, &p).unwrap();
                let mut buf = crate::testing::stereo_sine(rate, rate as usize);
                // An impulse too.
                buf[0] = 1.0;
                for blk in buf.chunks_mut(1000) {
                    fx.process(blk);
                }
                assert!(buf.iter().all(|v| v.is_finite()), "{rate}");
                let tail = &buf[buf.len() - 2000..];
                assert!(
                    tail.iter().all(|v| v.abs() < 1e4),
                    "{rate}: {:?}",
                    &tail[..4]
                );
            }
        }
    }
}
