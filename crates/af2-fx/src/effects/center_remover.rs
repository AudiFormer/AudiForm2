// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Center Remover: takes out what the two sides share (the centre of
//! the mix, usually the lead vocal), keeping the bass of it so the kick
//! and bass guitar stay. Written for AudiForm 2 to behave as Audiform's
//! Center Remover does (same controls and keys, so its sessions carry
//! over); the code is our own. Retired in Audiform (the Center Channel
//! Extractor does this and more), so it is kept for sessions but not
//! offered.
//!
//! Per sample: the centre is the average of the sides; its bass is the
//! centre through a 24 dB/octave Butterworth low-pass at Keep bass
//! below; Amount of what is left above that is taken from both sides. No
//! delay.

use super::{num, EffectInfo, Params, Processor};
use crate::biquad::{butterworth_qs, Biquad, Coeffs, Shape};

pub const ID: &str = "af.center_remover";

pub fn center_remover_info() -> EffectInfo {
    EffectInfo {
        id: ID,
        sidechain: false,
        name: "Center Remover",
        category: "Stereo",
        description: "Takes out the centre of a stereo mix (usually the lead vocal), keeping its bass.",
        params: vec![
            num("amount", "Amount", "%", 0.0, 100.0, 100.0, 1.0),
            num("keep_below", "Keep bass below", "Hz", 20.0, 500.0, 150.0, 1.0).log(),
        ],
    }
}

pub struct CenterRemover {
    rate: u32,
    amount: f32,
    bass: Vec<Biquad>,
}

impl CenterRemover {
    pub fn new(p: &Params, rate: u32) -> Self {
        let mut v = Self {
            rate: rate.max(1),
            amount: 1.0,
            bass: Vec::new(),
        };
        v.set(p);
        v
    }

    fn set(&mut self, p: &Params) {
        self.amount = (p.get("amount") / 100.0) as f32;
        let f = p.get("keep_below");
        let qs = butterworth_qs(2);
        if self.bass.len() == qs.len() {
            for (b, q) in self.bass.iter_mut().zip(&qs) {
                b.set(Coeffs::new(Shape::LowPass, self.rate, f, *q, 0.0));
            }
        } else {
            self.bass = qs
                .iter()
                .map(|q| Biquad::new(Coeffs::new(Shape::LowPass, self.rate, f, *q, 0.0)))
                .collect();
        }
    }
}

impl Processor for CenterRemover {
    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.as_chunks_mut::<2>().0 {
            let mid = 0.5 * (f[0] + f[1]);
            let mut low = mid;
            for b in &mut self.bass {
                low = b.run(low);
            }
            let cut = (mid - low) * self.amount;
            f[0] -= cut;
            f[1] -= cut;
        }
    }

    fn reset(&mut self) {
        for b in &mut self.bass {
            b.reset();
        }
    }

    fn update(&mut self, p: &Params) -> bool {
        self.set(p);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const R: u32 = 48_000;

    fn run(over: &[(&str, f64)], l: f64, r: f64, hz: f64) -> (f32, f32) {
        let saved: BTreeMap<String, f64> = over.iter().map(|(k, v)| ((*k).to_string(), *v)).collect();
        let mut p = CenterRemover::new(&Params::resolve(&center_remover_info(), &saved), R);
        let mut b: Vec<f32> = (0..R as usize)
            .flat_map(|i| {
                let x = (std::f64::consts::TAU * hz * i as f64 / f64::from(R)).sin();
                [(x * l) as f32, (x * r) as f32]
            })
            .collect();
        p.process(&mut b);
        let rms = |ch: usize| (b.iter().skip(24_000 * 2 + ch).step_by(2).map(|v| v * v).sum::<f32>() / 24_000.0).sqrt();
        (rms(0), rms(1))
    }

    #[test]
    fn the_centre_goes_but_its_bass_and_the_sides_stay() {
        let (l, r) = run(&[], 0.5, 0.5, 2000.0);
        assert!(l < 0.01 && r < 0.01, "a centred voice is gone: {l} {r}");
        let (l, _) = run(&[], 0.5, 0.5, 40.0);
        assert!(l > 0.3, "the bass stays: {l}");
        // Opposite sides share nothing: untouched.
        let (l, r) = run(&[], 0.5, -0.5, 2000.0);
        assert!((l - 0.3536).abs() < 0.01 && (r - 0.3536).abs() < 0.01, "{l} {r}");
        // Half the amount leaves half the centre.
        let (l, _) = run(&[("amount", 50.0)], 0.5, 0.5, 2000.0);
        assert!((l - 0.1768).abs() < 0.01, "{l}");
    }
}
