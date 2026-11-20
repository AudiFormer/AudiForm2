// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The low-frequency channel's band: everything that reaches it (sends,
//! multichannel clips, 3D Buses) is kept to the sub band by an 8th-order
//! Butterworth low pass at 120 Hz and a 4th-order Butterworth high pass at
//! 20 Hz, each a cascade of second-order sections.

use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, Default)]
struct Section {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Section {
    /// A second-order low or high pass at `f` Hz with quality `q`.
    fn new(rate: u32, f: f64, q: f64, high: bool) -> Section {
        let w = 2.0 * PI * f.min(rate as f64 * 0.45) / rate as f64;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b = if high {
            [(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0]
        } else {
            [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0]
        };
        Section {
            b: [b[0] / a0, b[1] / a0, b[2] / a0],
            a: [-2.0 * c / a0, (1.0 - alpha) / a0],
            z: [0.0; 2],
        }
    }

    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        // Transposed direct form II.
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// Qualities of the second-order sections of an `order` Butterworth.
fn butterworth_q(order: usize) -> impl Iterator<Item = f64> {
    (1..=order / 2).map(move |k| {
        let theta = (2 * k - 1) as f64 * PI / (2 * order) as f64;
        1.0 / (2.0 * theta.cos())
    })
}

/// The low-frequency channel's band filter.
#[derive(Debug, Clone)]
pub struct LfeFilter {
    sections: [Section; 6],
}

impl LfeFilter {
    pub fn new(rate: u32) -> LfeFilter {
        let mut sections = [Section::default(); 6];
        for (s, q) in sections.iter_mut().zip(butterworth_q(8)) {
            *s = Section::new(rate, 120.0, q, false);
        }
        for (s, q) in sections[4..].iter_mut().zip(butterworth_q(4)) {
            *s = Section::new(rate, 20.0, q, true);
        }
        LfeFilter { sections }
    }

    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        let mut v = x as f64;
        for s in &mut self.sections {
            v = s.run(v);
        }
        v as f32
    }

    /// Filters channel `ch` of an interleaved bed `channels` wide.
    pub fn process(&mut self, bed: &mut [f32], channels: usize, ch: usize) {
        for f in bed.chunks_exact_mut(channels) {
            f[ch] = self.run(f[ch]);
        }
    }

    pub fn reset(&mut self) {
        for s in &mut self.sections {
            s.z = [0.0; 2];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain_db(f: f64) -> f64 {
        let rate = 48_000;
        let mut lf = LfeFilter::new(rate);
        let n = rate as usize * 2;
        let mut peak = 0.0f32;
        for i in 0..n {
            let x = (2.0 * PI * f * i as f64 / rate as f64).sin() as f32;
            let y = lf.run(x);
            if i > n / 2 {
                peak = peak.max(y.abs());
            }
        }
        20.0 * (peak as f64).log10()
    }

    #[test]
    fn lfe_keeps_only_the_low_band() {
        assert!(gain_db(60.0).abs() < 0.5, "60 Hz: {}", gain_db(60.0));
        assert!(gain_db(1000.0) < -60.0, "1 kHz: {}", gain_db(1000.0));
        assert!(gain_db(10.0) < -9.0, "10 Hz: {}", gain_db(10.0));
        // Butterworth: -3 dB at each corner.
        assert!(
            (gain_db(120.0) + 3.0).abs() < 0.6,
            "120 Hz: {}",
            gain_db(120.0)
        );
    }
}
