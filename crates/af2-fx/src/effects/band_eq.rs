// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Band EQ: fixed bands across the range, each turned up or down by up
//! to 15 dB, with an Output level. It has 10 bands an octave apart, 20 a
//! half-octave apart or 30 a third-octave apart (the `bands` control,
//! switched with the tabs in its window). Switching keeps the curve's
//! shape: see [`carry`].
//!
//! Written for AF2 from the behaviour of Audiform's Band EQ (its band
//! counts, centres, ranges and how a new count keeps the curve). No
//! Audiform DSP was read for the sound: each band is a peaking section
//! from the public Audio EQ Cookbook ([`crate::biquad`]), its Q set by
//! the band spacing with the standard bandwidth formula.

use super::{choice, db_to_gain, num, EffectInfo, ParamInfo, Params, Processor};
use crate::biquad::{Coeffs, Shape, StereoBiquad};

pub const ID: &str = "af.band_eq";

/// The most bands an EQ has.
pub const MAX_BANDS: usize = 30;

/// The band counts, in the order of the `bands` control's options.
pub const COUNTS: [usize; 3] = [10, 20, 30];

/// Each band's gain range, in dB.
pub const RANGE: f64 = 15.0;

/// The keys of the bands' gains, lowest band first. A key's frequency
/// depends on the band count: see [`centres`].
pub const GAIN_KEYS: [&str; MAX_BANDS] = [
    "g01", "g02", "g03", "g04", "g05", "g06", "g07", "g08", "g09", "g10", "g11", "g12", "g13",
    "g14", "g15", "g16", "g17", "g18", "g19", "g20", "g21", "g22", "g23", "g24", "g25", "g26",
    "g27", "g28", "g29", "g30",
];

const NAMES: [&str; MAX_BANDS] = [
    "Band 1", "Band 2", "Band 3", "Band 4", "Band 5", "Band 6", "Band 7", "Band 8", "Band 9",
    "Band 10", "Band 11", "Band 12", "Band 13", "Band 14", "Band 15", "Band 16", "Band 17",
    "Band 18", "Band 19", "Band 20", "Band 21", "Band 22", "Band 23", "Band 24", "Band 25",
    "Band 26", "Band 27", "Band 28", "Band 29", "Band 30",
];

/// ISO octave centres.
const OCTAVES: [f64; 10] = [
    31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16_000.0,
];

/// ISO third-octave centres.
const THIRDS: [f64; 30] = [
    25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0, 500.0,
    630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0, 6300.0, 8000.0,
    10_000.0, 12_500.0, 16_000.0, 20_000.0,
];

/// Half-octave centres: 31.5 Hz and every half-octave above it.
fn halves() -> [f64; 20] {
    std::array::from_fn(|k| 31.5 * 2f64.powf(k as f64 / 2.0))
}

/// The band count for the `bands` control's value (an index into
/// [`COUNTS`]).
pub fn count_of(index: usize) -> usize {
    COUNTS[index.min(COUNTS.len() - 1)]
}

/// The `bands` control's value for a band count (the nearest count).
pub fn index_of(count: usize) -> usize {
    match count {
        0..=14 => 0,
        15..=24 => 1,
        _ => 2,
    }
}

/// The centre of each band, in Hz, lowest first, for `n` bands (10, 20
/// or 30; other counts take the nearest). Only the first `n` entries are
/// used.
pub fn centres(n: usize) -> [f64; MAX_BANDS] {
    let mut out = [0.0; MAX_BANDS];
    match count_of(index_of(n)) {
        10 => out[..10].copy_from_slice(&OCTAVES),
        20 => out[..20].copy_from_slice(&halves()),
        _ => out.copy_from_slice(&THIRDS),
    }
    out
}

/// The Q of a peaking band one band-spacing wide: for a bandwidth of
/// `b` octaves, Q = sqrt(2^b) / (2^b - 1) (octave 1.41, half-octave
/// 2.87, third-octave 4.32).
pub fn q_for(n: usize) -> f64 {
    let b = match count_of(index_of(n)) {
        10 => 1.0,
        20 => 0.5,
        _ => 1.0 / 3.0,
    };
    let r = 2f64.powf(b);
    r.sqrt() / (r - 1.0)
}

/// The highest centre a band may have at `rate`; bands above it are left
/// flat (a peak too near Nyquist warps and can ring).
fn top(rate: u32) -> f64 {
    f64::from(rate) * 0.45
}

/// Band `i`'s section for `n` bands at `rate` with `gain_db`; flat when
/// the band is above [`top`] or its gain is nil.
pub fn band_coeffs(n: usize, i: usize, rate: u32, gain_db: f64) -> Coeffs {
    let f = centres(n)[i];
    if i >= n || f >= top(rate) || gain_db.abs() < 1e-3 {
        Coeffs::FLAT
    } else {
        Coeffs::new(Shape::Peak, rate, f, q_for(n), gain_db)
    }
}

/// The EQ's gain at `hz`, in dB: every band's section, plus Output.
/// What is drawn is what is heard: these are the sections that run.
pub fn response_db(n: usize, gains: &[f64], output_db: f64, rate: u32, hz: f64) -> f64 {
    let n = count_of(index_of(n));
    (0..n)
        .map(|i| {
            let g = gains.get(i).copied().unwrap_or(0.0);
            band_coeffs(n, i, rate, g).response_db(rate, hz)
        })
        .sum::<f64>()
        + output_db
}

/// The gains for `to` bands that keep the shape of `gains` (for `from`
/// bands): each new band takes the level the old bands have at its
/// centre, read between the two nearest evenly in octaves (and the end
/// bands' levels beyond them), rounded to 0.1 dB and kept in range.
pub fn carry(from: usize, gains: &[f64], to: usize) -> [f64; MAX_BANDS] {
    let from = count_of(index_of(from));
    let to = count_of(index_of(to));
    let fc = centres(from);
    let old: Vec<(f64, f64)> = (0..from)
        .map(|i| (fc[i], gains.get(i).copied().unwrap_or(0.0)))
        .collect();
    let at = |hz: f64| -> f64 {
        let (first, last) = (old[0], old[old.len() - 1]);
        if hz <= first.0 {
            return first.1;
        }
        if hz >= last.0 {
            return last.1;
        }
        for w in old.windows(2) {
            let ((fa, ga), (fb, gb)) = (w[0], w[1]);
            if hz <= fb {
                let t = (hz / fa).ln() / (fb / fa).ln();
                return ga + (gb - ga) * t;
            }
        }
        last.1
    };
    let tc = centres(to);
    let mut out = [0.0; MAX_BANDS];
    for i in 0..to {
        let v = (at(tc[i]) * 10.0).round() / 10.0;
        out[i] = if v.abs() < 1e-9 { 0.0 } else { v.clamp(-RANGE, RANGE) };
    }
    out
}

pub fn info() -> EffectInfo {
    let mut params: Vec<ParamInfo> = vec![choice(
        "bands",
        "Bands",
        &["10 bands", "20 bands", "30 bands"],
        0,
    )
    .hidden()];
    params.extend(
        GAIN_KEYS
            .iter()
            .zip(NAMES)
            .map(|(k, n)| num(k, n, "dB", -RANGE, RANGE, 0.0, 0.1).group("Bands")),
    );
    params.push(num("output", "Output", "dB", -RANGE, RANGE, 0.0, 0.1));
    EffectInfo {
        id: ID,
        name: "Band EQ",
        category: "EQ",
        description: "Fixed bands across the range, each turned up or down: quick broad tone \
                      shaping. Tabs pick 10, 20 or 30 bands; a new count keeps the curve.",
        params,
        sidechain: false,
    }
}

pub struct BandEq {
    /// One section per band; only the first `n` run, and flat ones are
    /// skipped.
    bands: [StereoBiquad; MAX_BANDS],
    live: [bool; MAX_BANDS],
    n: usize,
    gain: f32,
    rate: u32,
}

impl BandEq {
    pub fn new(p: &Params, rate: u32) -> Self {
        let mut eq = BandEq {
            bands: [StereoBiquad::new(Coeffs::FLAT); MAX_BANDS],
            live: [false; MAX_BANDS],
            n: count_of(p.index("bands")),
            gain: 1.0,
            rate: rate.max(1),
        };
        eq.tune(p);
        eq
    }

    /// New coefficients in place, keeping each section's memory.
    fn tune(&mut self, p: &Params) {
        for i in 0..MAX_BANDS {
            let c = band_coeffs(self.n, i, self.rate, p.get(GAIN_KEYS[i]));
            self.live[i] = c != Coeffs::FLAT;
            self.bands[i].set(c);
            if !self.live[i] {
                self.bands[i].reset();
            }
        }
        self.gain = db_to_gain(p.get("output"));
    }
}

impl Processor for BandEq {
    fn process(&mut self, buf: &mut [f32]) {
        for (b, live) in self.bands.iter_mut().zip(self.live).take(self.n) {
            if live {
                b.process(buf);
            }
        }
        if self.gain != 1.0 {
            for s in buf.iter_mut() {
                *s *= self.gain;
            }
        }
    }

    fn reset(&mut self) {
        for b in &mut self.bands {
            b.reset();
        }
    }

    /// Retunes in place (no allocation). A new band count is a different
    /// EQ: it asks to be built again.
    fn update(&mut self, p: &Params) -> bool {
        if count_of(p.index("bands")) != self.n {
            return false;
        }
        self.tune(p);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(set: &[(&'static str, f64)]) -> Params {
        let mut p = Params::from_pairs(&info(), &[]);
        for (k, v) in set {
            p.set(k, *v);
        }
        p
    }

    /// The level of a sine at `hz` after the EQ, in dB, measured.
    fn measured(p: &Params, hz: f64) -> f64 {
        let rate = 48_000;
        let mut eq = BandEq::new(p, rate);
        let frames = rate as usize;
        let mut buf: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let v = (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin() as f32;
                [v, v]
            })
            .collect();
        for b in buf.chunks_mut(512) {
            eq.process(b);
        }
        let tail = &buf[frames..frames + 2 * 24_000];
        // RMS of a sine, as its amplitude (a sampled peak can miss the
        // true one at high frequencies).
        let ms = tail.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / tail.len() as f64;
        20.0 * (ms * 2.0).sqrt().log10()
    }

    #[test]
    fn centres_and_q_follow_the_band_spacing() {
        assert_eq!(centres(10)[5], 1000.0);
        assert_eq!(centres(30)[16], 1000.0);
        let h = centres(20);
        assert!((h[10] - 1008.0).abs() < 0.1, "{}", h[10]);
        for n in COUNTS {
            let c = centres(n);
            assert!(c[..n].windows(2).all(|w| w[0] < w[1]), "{n} rising");
        }
        assert!((q_for(10) - 1.414).abs() < 0.01);
        assert!((q_for(20) - 2.871).abs() < 0.01);
        assert!((q_for(30) - 4.318).abs() < 0.01);
        assert_eq!((count_of(0), count_of(2), count_of(9)), (10, 30, 30));
        assert_eq!((index_of(10), index_of(20), index_of(30)), (0, 1, 2));
    }

    #[test]
    fn flat_passes_everything() {
        let p = params(&[]);
        for hz in [50.0, 1000.0, 9000.0] {
            assert!(measured(&p, hz).abs() < 0.05, "{hz}");
            assert!(response_db(10, &[0.0; 30], 0.0, 48_000, hz).abs() < 1e-9);
        }
    }

    #[test]
    fn each_band_lifts_its_centre() {
        // 10 bands: band 6 is 1 kHz.
        let p = params(&[("g06", 6.0)]);
        assert!((measured(&p, 1000.0) - 6.0).abs() < 0.3);
        // Two octaves away it has almost no effect.
        assert!(measured(&p, 250.0).abs() < 0.5);
        // 30 bands: band 17 is 1 kHz; a cut.
        let p = params(&[("bands", 2.0), ("g17", -9.0)]);
        assert!((measured(&p, 1000.0) + 9.0).abs() < 0.5);
        // 20 bands: band 11 is about 1 kHz.
        let p = params(&[("bands", 1.0), ("g11", 12.0)]);
        assert!((measured(&p, 1008.0) - 12.0).abs() < 0.6);
        // Output moves everything.
        let p = params(&[("output", -6.0)]);
        assert!((measured(&p, 300.0) + 6.0).abs() < 0.05);
    }

    #[test]
    fn the_drawn_curve_is_the_heard_one() {
        let set = [("bands", 1.0), ("g05", 7.0), ("g06", -4.0), ("g15", 10.0), ("output", -2.0)];
        let p = params(&set);
        let gains: Vec<f64> = GAIN_KEYS.iter().map(|k| p.get(k)).collect();
        for hz in [80.0, 125.0, 700.0, 5000.0, 6000.0] {
            let drawn = response_db(20, &gains, -2.0, 48_000, hz);
            let heard = measured(&p, hz);
            assert!((drawn - heard).abs() < 0.15, "{hz}: {drawn} vs {heard}");
        }
    }

    #[test]
    fn bands_above_nyquist_stay_flat() {
        // At 32 kHz the top third-octave bands (16 and 20 kHz) are past
        // 0.45 x rate and are left flat.
        assert_eq!(band_coeffs(30, 28, 32_000, 6.0), Coeffs::FLAT);
        assert_eq!(band_coeffs(30, 29, 32_000, 6.0), Coeffs::FLAT);
        assert_ne!(band_coeffs(30, 27, 32_000, 6.0), Coeffs::FLAT);
        let mut gains = [0.0; MAX_BANDS];
        gains[29] = 15.0;
        let mut eq = BandEq::new(&params(&[("bands", 2.0), ("g30", 15.0)]), 32_000);
        let mut b = vec![0.25f32; 2048];
        eq.process(&mut b);
        assert!(b.iter().all(|v| v.is_finite()));
        assert!(response_db(30, &gains, 0.0, 32_000, 1000.0).abs() < 1e-9);
    }

    #[test]
    fn a_new_count_keeps_the_curve() {
        // Same count: unchanged.
        let mut g = [0.0; MAX_BANDS];
        g[3] = 4.5;
        g[7] = -3.2;
        assert_eq!(carry(10, &g, 10), g);
        // 10 -> 30: the third-octave bands on the octave centres take the
        // octave bands' levels; those between are read between them.
        let up = carry(10, &g, 30);
        let thirds = centres(30);
        let octaves = centres(10);
        for (i, f) in octaves.iter().take(10).enumerate() {
            let j = thirds.iter().position(|t| t == f).unwrap();
            assert!((up[j] - g[i]).abs() < 1e-9, "{f} Hz");
        }
        // 250 Hz is +4.5, 500 Hz is 0: 315 Hz a third of the way down.
        let j315 = thirds.iter().position(|&t| t == 315.0).unwrap();
        assert!((up[j315] - 3.0).abs() < 0.11, "{}", up[j315]);
        // Below the lowest band: the lowest band's level.
        let mut low = [0.0; MAX_BANDS];
        low[0] = 6.0;
        assert_eq!(carry(10, &low, 30)[0], 6.0);
        // And back: 30 -> 10 lands on the same octave levels.
        let back = carry(30, &up, 10);
        for i in 0..10 {
            assert!((back[i] - g[i]).abs() < 1e-9, "band {i}");
        }
        // Bands past the new count are cleared.
        assert!(carry(30, &[9.0; 30], 10)[10..].iter().all(|v| *v == 0.0));
        // 20 bands' outer centres keep the range.
        let wide = carry(10, &[15.0; 30], 20);
        assert!(wide[..20].iter().all(|v| *v == 15.0));
    }

    #[test]
    fn retuning_in_place_and_rebuild_on_a_new_count() {
        let mut eq = BandEq::new(&params(&[]), 48_000);
        assert!(eq.update(&params(&[("g02", 3.0)])));
        assert!(eq.live[1] && !eq.live[0]);
        assert!(!eq.update(&params(&[("bands", 2.0)])));
    }
}
