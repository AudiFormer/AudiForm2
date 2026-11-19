// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Second-order IIR sections with the well-known "Audio EQ Cookbook"
//! (Robert Bristow-Johnson) coefficient formulas.

use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    LowPass,
    HighPass,
    Peak,
    LowShelf,
    HighShelf,
    Notch,
    BandPass,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    /// `freq` in Hz, `q` the quality factor, `gain_db` for peak and shelves.
    pub fn new(kind: Kind, rate: f64, freq: f64, q: f64, gain_db: f64) -> Self {
        let freq = freq.clamp(1.0, rate * 0.499);
        let q = q.max(0.01);
        let w0 = 2.0 * PI * freq / rate;
        let (sn, cs) = w0.sin_cos();
        let alpha = sn / (2.0 * q);
        let a = 10f64.powf(gain_db / 40.0);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            Kind::LowPass => (
                (1.0 - cs) / 2.0,
                1.0 - cs,
                (1.0 - cs) / 2.0,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            Kind::HighPass => (
                (1.0 + cs) / 2.0,
                -(1.0 + cs),
                (1.0 + cs) / 2.0,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            Kind::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            Kind::Notch => (1.0, -2.0 * cs, 1.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            Kind::Peak => (
                1.0 + alpha * a,
                -2.0 * cs,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cs,
                1.0 - alpha / a,
            ),
            Kind::LowShelf => {
                let k = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cs + k),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                    a * ((a + 1.0) - (a - 1.0) * cs - k),
                    (a + 1.0) + (a - 1.0) * cs + k,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                    (a + 1.0) + (a - 1.0) * cs - k,
                )
            }
            Kind::HighShelf => {
                let k = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cs + k),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                    a * ((a + 1.0) + (a - 1.0) * cs - k),
                    (a + 1.0) - (a - 1.0) * cs + k,
                    2.0 * ((a - 1.0) - (a + 1.0) * cs),
                    (a + 1.0) - (a - 1.0) * cs - k,
                )
            }
        };
        Biquad::from_coeffs(b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
    }

    /// Normalised coefficients (a0 = 1).
    pub fn from_coeffs(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
        Biquad {
            b0,
            b1,
            b2,
            a1,
            a2,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Takes another section's coefficients, keeping this one's state,
    /// so a filter can be retuned while it plays without a click.
    pub fn set_coeffs_from(&mut self, o: &Biquad) {
        self.b0 = o.b0;
        self.b1 = o.b1;
        self.b2 = o.b2;
        self.a1 = o.a1;
        self.a2 = o.a2;
    }

    pub fn clear(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// Transposed direct form II.
    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    pub fn run(&mut self, data: &mut [f32]) {
        for s in data {
            *s = self.process(*s as f64) as f32;
        }
    }

    /// Magnitude response in dB at `freq`, for drawing EQ curves.
    pub fn response_db(&self, rate: f64, freq: f64) -> f64 {
        let w = 2.0 * PI * freq / rate;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        let mag2 = (nr * nr + ni * ni) / (dr * dr + di * di).max(1e-30);
        10.0 * mag2.max(1e-30).log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_have_the_right_shape() {
        let fs = 48_000.0;
        let lp = Biquad::new(
            Kind::LowPass,
            fs,
            1000.0,
            std::f64::consts::FRAC_1_SQRT_2,
            0.0,
        );
        assert!(lp.response_db(fs, 50.0).abs() < 0.1);
        assert!((lp.response_db(fs, 1000.0) + 3.01).abs() < 0.1);
        assert!(lp.response_db(fs, 10_000.0) < -35.0);
        let pk = Biquad::new(Kind::Peak, fs, 2000.0, 1.0, 6.0);
        assert!((pk.response_db(fs, 2000.0) - 6.0).abs() < 0.05);
        let ls = Biquad::new(
            Kind::LowShelf,
            fs,
            200.0,
            std::f64::consts::FRAC_1_SQRT_2,
            -6.0,
        );
        assert!((ls.response_db(fs, 20.0) + 6.0).abs() < 0.2);
        let nt = Biquad::new(Kind::Notch, fs, 60.0, 10.0, 0.0);
        assert!(nt.response_db(fs, 60.0) < -40.0);
    }
}
