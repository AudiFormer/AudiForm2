// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Second-order filter sections with the curves of Robert
//! Bristow-Johnson's public "Audio EQ Cookbook", for the effects' tone
//! controls, crossovers and band limits.

use std::f64::consts::PI;

/// The cookbook's filter shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    LowPass,
    HighPass,
    /// Constant 0 dB peak gain.
    BandPass,
    Notch,
    AllPass,
    Peak,
    LowShelf,
    HighShelf,
}

/// Normalised coefficients (a0 = 1) of one section.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coeffs {
    /// Passes everything as it is.
    pub const FLAT: Coeffs = Coeffs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// A cookbook section at `freq` Hz. `q` is the quality factor (for
    /// the shelves too, as the cookbook's Q form), `gain_db` is used by
    /// Peak and the shelves. The frequency is kept below Nyquist.
    pub fn new(shape: Shape, rate: u32, freq: f64, q: f64, gain_db: f64) -> Coeffs {
        let fs = f64::from(rate.max(1));
        let f = freq.clamp(1e-3, fs * 0.4999);
        let q = if q > 1e-6 { q } else { 1e-6 };
        let w0 = 2.0 * PI * f / fs;
        let (sw, cw) = w0.sin_cos();
        let alpha = sw / (2.0 * q);
        let a = 10f64.powf(gain_db / 40.0);
        let (b0, b1, b2, a0, a1, a2) = match shape {
            Shape::LowPass => (
                (1.0 - cw) / 2.0,
                1.0 - cw,
                (1.0 - cw) / 2.0,
                1.0 + alpha,
                -2.0 * cw,
                1.0 - alpha,
            ),
            Shape::HighPass => (
                (1.0 + cw) / 2.0,
                -(1.0 + cw),
                (1.0 + cw) / 2.0,
                1.0 + alpha,
                -2.0 * cw,
                1.0 - alpha,
            ),
            Shape::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            Shape::Notch => (1.0, -2.0 * cw, 1.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            Shape::AllPass => (
                1.0 - alpha,
                -2.0 * cw,
                1.0 + alpha,
                1.0 + alpha,
                -2.0 * cw,
                1.0 - alpha,
            ),
            Shape::Peak => (
                1.0 + alpha * a,
                -2.0 * cw,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cw,
                1.0 - alpha / a,
            ),
            Shape::LowShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cw + s),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cw),
                    a * ((a + 1.0) - (a - 1.0) * cw - s),
                    (a + 1.0) + (a - 1.0) * cw + s,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cw),
                    (a + 1.0) + (a - 1.0) * cw - s,
                )
            }
            Shape::HighShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cw + s),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cw),
                    a * ((a + 1.0) + (a - 1.0) * cw - s),
                    (a + 1.0) - (a - 1.0) * cw + s,
                    2.0 * ((a - 1.0) - (a + 1.0) * cw),
                    (a + 1.0) - (a - 1.0) * cw - s,
                )
            }
        };
        Coeffs {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// The same section with its output scaled by `g`.
    pub fn scaled(self, g: f64) -> Coeffs {
        Coeffs {
            b0: self.b0 * g,
            b1: self.b1 * g,
            b2: self.b2 * g,
            ..self
        }
    }

    /// The gain at `freq` Hz, in dB.
    pub fn response_db(&self, rate: u32, freq: f64) -> f64 {
        let w = 2.0 * PI * freq / f64::from(rate.max(1));
        // H(z) at z = e^{jw}: evaluate numerator and denominator in z^-1.
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        let mag2 = (nr * nr + ni * ni) / (dr * dr + di * di).max(1e-300);
        10.0 * mag2.max(1e-300).log10()
    }
}

/// One section with its memory (transposed direct form II, in f64).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    c: Coeffs,
    z1: f64,
    z2: f64,
}

impl Biquad {
    pub fn new(c: Coeffs) -> Self {
        Biquad {
            c,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// One sample through the section.
    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        let x = f64::from(x);
        let c = &self.c;
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        // Keep denormals from slowing the processor down.
        if self.z1.abs() < 1e-30 {
            self.z1 = 0.0;
        }
        if self.z2.abs() < 1e-30 {
            self.z2 = 0.0;
        }
        y as f32
    }

    /// Forgets the memory.
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// New coefficients, keeping the memory (for smooth changes).
    pub fn set(&mut self, c: Coeffs) {
        self.c = c;
    }

    pub fn coeffs(&self) -> Coeffs {
        self.c
    }
}

/// The same section on both sides of interleaved stereo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StereoBiquad {
    pub l: Biquad,
    pub r: Biquad,
}

impl StereoBiquad {
    pub fn new(c: Coeffs) -> Self {
        StereoBiquad {
            l: Biquad::new(c),
            r: Biquad::new(c),
        }
    }

    pub fn set(&mut self, c: Coeffs) {
        self.l.set(c);
        self.r.set(c);
    }

    /// Interleaved stereo frames, in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        for f in buf.as_chunks_mut::<2>().0 {
            f[0] = self.l.run(f[0]);
            f[1] = self.r.run(f[1]);
        }
    }

    pub fn reset(&mut self) {
        self.l.reset();
        self.r.reset();
    }
}

/// The Q of each second-order section of a Butterworth filter of order
/// `2 * sections`: 1 / (2 sin((2k + 1) pi / (2n))).
pub fn butterworth_qs(sections: usize) -> Vec<f64> {
    let n = (2 * sections) as f64;
    (0..sections)
        .map(|k| 1.0 / (2.0 * ((2 * k + 1) as f64 * PI / (2.0 * n)).sin()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookbook_curves_land_where_they_should() {
        let r = 48_000;
        let lp = Coeffs::new(
            Shape::LowPass,
            r,
            1000.0,
            std::f64::consts::FRAC_1_SQRT_2,
            0.0,
        );
        assert!((lp.response_db(r, 1000.0) + 3.01).abs() < 0.05);
        assert!(lp.response_db(r, 20.0).abs() < 0.01);
        let pk = Coeffs::new(Shape::Peak, r, 1000.0, 1.0, 6.0);
        assert!((pk.response_db(r, 1000.0) - 6.0).abs() < 0.01);
        let ls = Coeffs::new(Shape::LowShelf, r, 200.0, 0.7, -9.0);
        assert!((ls.response_db(r, 10.0) + 9.0).abs() < 0.1);
        assert!(ls.response_db(r, 15_000.0).abs() < 0.1);
        let hs = Coeffs::new(Shape::HighShelf, r, 3000.0, 0.7, 4.0);
        assert!((hs.response_db(r, 20_000.0) - 4.0).abs() < 0.2);
        let bp = Coeffs::new(Shape::BandPass, r, 2000.0, 2.0, 0.0);
        assert!(bp.response_db(r, 2000.0).abs() < 0.01);
    }

    #[test]
    fn butterworth_qs_match_the_known_values() {
        let q = butterworth_qs(2);
        assert!((q[0] - 1.3066).abs() < 1e-3 && (q[1] - 0.5412).abs() < 1e-3);
        assert!((butterworth_qs(1)[0] - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
    }

    #[test]
    fn a_section_filters_samples() {
        let mut b = Biquad::new(Coeffs::new(Shape::HighPass, 48_000, 100.0, 0.707, 0.0));
        let mut y = 0.0;
        for _ in 0..48_000 {
            y = b.run(1.0);
        }
        assert!(y.abs() < 1e-4, "DC is removed: {y}");
        let mut f = Biquad::new(Coeffs::FLAT);
        assert_eq!(f.run(0.3), 0.3);
    }
}
