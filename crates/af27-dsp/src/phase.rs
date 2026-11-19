// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.
// Ported from Audiform's Phase Wheel analysis (engine/scope.rs), written
// by its author (nwh212) for Audiform and relicensed here by them. The
// calibration is kept exactly; only the FFT is Audiform27's own.

//! The Phase Wheel's analysis.
//!
//! Compares two signals (left, right, mid or side) in 1,024 frequency
//! bands spaced evenly on a log scale from 20 Hz to 20 kHz. For each band:
//! the phase of the second signal relative to the first (0 when they move
//! together, half a turn when they are opposite), summed over the band's
//! bins so a steady relationship reads as one steady point; how loud the
//! band is; and how consistent its phase is across the band (1 for a plain
//! delay or phase shift, near 0 for unrelated sound). Plus the correlation
//! of the two over the whole window, from +1 (the same) through 0
//! (unrelated) to -1 (opposite).

use crate::fft::fft;

/// Analysis window sizes offered, smallest (lightest) first.
pub const SIZES: [usize; 5] = [1024, 2048, 4096, 8192, 16384];
/// Pictures a second offered.
pub const RATES: [u32; 4] = [15, 30, 60, 120];

/// Which signal a side of the comparison reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Left,
    Right,
    /// (L + R) / 2
    Mid,
    /// (L - R) / 2
    Side,
}

impl Signal {
    pub const ALL: [Signal; 4] = [Signal::Left, Signal::Right, Signal::Mid, Signal::Side];

    pub fn label(self) -> &'static str {
        match self {
            Signal::Left => "Left",
            Signal::Right => "Right",
            Signal::Mid => "Mid (L+R)",
            Signal::Side => "Side (L-R)",
        }
    }

    fn of(self, f: [f32; 2]) -> f32 {
        match self {
            Signal::Left => f[0],
            Signal::Right => f[1],
            Signal::Mid => (f[0] + f[1]) * 0.5,
            Signal::Side => (f[0] - f[1]) * 0.5,
        }
    }
}

/// One frequency band of the comparison.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bin {
    /// Phase of `b` relative to `a`, -pi to pi (0 is in phase; positive
    /// when `b` leads).
    pub phase: f32,
    /// Level, 0 (at or below -72 dBFS) to 1 (full scale).
    pub level: f32,
    /// The band's middle from 20 Hz (0) to 20 kHz (1), on a log scale.
    pub pitch: f32,
    /// How alike the phase is across the band, 0 to 1.
    pub coherence: f32,
}

/// Bands the 20 Hz to 20 kHz range is split into: about one point per
/// analysis bin in the middle range, so the wheel fills in densely.
pub const BANDS: usize = 1024;

#[derive(Debug, Clone, Default)]
pub struct Phase {
    /// -1 to 1; 0 when either side is silent.
    pub correlation: f32,
    pub bins: Vec<Bin>,
}

/// Level range shown: this many dB below full scale is the center.
const FLOOR_DB: f32 = 72.0;
const LOW_HZ: f32 = 20.0;
const HIGH_HZ: f32 = 20_000.0;

fn hann(i: usize, n: usize) -> f32 {
    0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()
}

/// Compares `a` and `b` over `frames` (each [left, right]) at `rate`.
/// The window is rounded down to a power of two.
pub fn analyse(frames: &[[f32; 2]], a: Signal, b: Signal, rate: u32) -> Phase {
    let n = if frames.len() < 4 {
        frames.len()
    } else {
        1 << (usize::BITS - 1 - frames.len().leading_zeros())
    };
    let frames = &frames[frames.len() - n..];
    let sa: Vec<f32> = frames.iter().map(|f| a.of(*f)).collect();
    let sb: Vec<f32> = frames.iter().map(|f| b.of(*f)).collect();
    let (mut ab, mut aa, mut bb) = (0f64, 0f64, 0f64);
    for (x, y) in sa.iter().zip(&sb) {
        ab += f64::from(*x) * f64::from(*y);
        aa += f64::from(*x) * f64::from(*x);
        bb += f64::from(*y) * f64::from(*y);
    }
    let correlation = if aa > 1e-12 && bb > 1e-12 {
        (ab / (aa * bb).sqrt()) as f32
    } else {
        0.0
    };
    if n < 4 {
        return Phase {
            correlation,
            bins: Vec::new(),
        };
    }
    // Hann window; its sum sets full scale.
    let mut sum = 0f32;
    let mut ra = vec![0f64; n];
    let mut rb = vec![0f64; n];
    for i in 0..n {
        let w = hann(i, n);
        ra[i] = (sa[i] * w) as f64;
        rb[i] = (sb[i] * w) as f64;
        sum += w;
    }
    let mut ia = vec![0f64; n];
    let mut ib = vec![0f64; n];
    fft(&mut ra, &mut ia, false);
    fft(&mut rb, &mut ib, false);
    let full = sum / 2.0;
    let hz_per_bin = rate.max(1) as f32 / n as f32;
    let span = (HIGH_HZ / LOW_HZ).ln();
    // Per band: the cross spectrum (b against a) and each side's power, summed.
    let mut cross = vec![(0f32, 0f32); BANDS];
    let mut pa = vec![0f32; BANDS];
    let mut pb = vec![0f32; BANDS];
    let mut count = vec![0u32; BANDS];
    for k in 1..n / 2 {
        let f = k as f32 * hz_per_bin;
        if !(LOW_HZ..HIGH_HZ).contains(&f) {
            continue;
        }
        let band = ((((f / LOW_HZ).ln() / span) * BANDS as f32) as usize).min(BANDS - 1);
        let (xr, xi) = (ra[k] as f32, ia[k] as f32);
        let (yr, yi) = (rb[k] as f32, ib[k] as f32);
        // y * conj(x)
        cross[band].0 += yr * xr + yi * xi;
        cross[band].1 += yi * xr - yr * xi;
        pa[band] += xr * xr + xi * xi;
        pb[band] += yr * yr + yi * yi;
        count[band] += 1;
    }
    let mut bins = Vec::with_capacity(BANDS);
    for band in 0..BANDS {
        if count[band] == 0 {
            continue;
        }
        // The band's whole power, scaled so a sine reads at its own level
        // (the Hann window spreads a sine over 1.5 bins' worth of power).
        let rms = ((pa[band] + pb[band]) * 0.5 / 1.5).sqrt() / full;
        let db = 20.0 * rms.max(1e-9).log10();
        let level = ((db + FLOOR_DB) / FLOOR_DB).clamp(0.0, 1.0);
        if level <= 0.0 {
            continue;
        }
        let (cr, ci) = cross[band];
        let power = (pa[band] * pb[band]).sqrt();
        bins.push(Bin {
            phase: ci.atan2(cr),
            level,
            pitch: (band as f32 + 0.5) / BANDS as f32,
            coherence: if power > 0.0 {
                (cr.hypot(ci) / power).clamp(0.0, 1.0)
            } else {
                0.0
            },
        });
    }
    Phase { correlation, bins }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, shift: f32) -> Vec<[f32; 2]> {
        // 64 cycles over the window: right is left shifted by `shift` radians.
        (0..n)
            .map(|i| {
                let t = std::f32::consts::TAU * 64.0 * i as f32 / n as f32;
                [0.5 * t.sin(), 0.5 * (t - shift).sin()]
            })
            .collect()
    }

    fn loudest(p: &Phase) -> Bin {
        *p.bins
            .iter()
            .max_by(|a, b| a.level.total_cmp(&b.level))
            .unwrap()
    }

    #[test]
    fn in_phase_opposite_and_a_quarter_turn() {
        let same = analyse(&tone(4096, 0.0), Signal::Left, Signal::Right, 48_000);
        assert!((same.correlation - 1.0).abs() < 1e-3);
        assert!(loudest(&same).phase.abs() < 1e-3);
        // -6 dBFS sine: level 1 - 6/72, within 2 dB.
        assert!((loudest(&same).level - (1.0 - 6.02 / 72.0)).abs() < 2.0 / 72.0);

        let flipped = analyse(
            &tone(4096, std::f32::consts::PI),
            Signal::Left,
            Signal::Right,
            48_000,
        );
        assert!((flipped.correlation + 1.0).abs() < 1e-3);
        assert!((loudest(&flipped).phase.abs() - std::f32::consts::PI).abs() < 1e-3);

        let quarter = analyse(
            &tone(4096, std::f32::consts::FRAC_PI_2),
            Signal::Left,
            Signal::Right,
            48_000,
        );
        assert!(quarter.correlation.abs() < 0.01);
        // Right lags left by a quarter turn: right relative to left is -90°.
        assert!((loudest(&quarter).phase + std::f32::consts::FRAC_PI_2).abs() < 1e-2);
        assert!(loudest(&quarter).coherence > 0.99);

        // 64 cycles over 4,096 frames at 48 kHz is 750 Hz.
        let want = (750f32 / 20.0).ln() / 1000f32.ln();
        assert!((loudest(&same).pitch - want).abs() < 0.02);
        assert!(same.bins.len() <= BANDS);

        // Mid against side of an in-phase signal: side is silent.
        let ms = analyse(&tone(4096, 0.0), Signal::Mid, Signal::Side, 48_000);
        assert_eq!(ms.correlation, 0.0);
    }
}
