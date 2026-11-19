// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! 3D Space sessions: tracks are placed in a room instead of panned. Each
//! track's sound is spread over the layout's speakers (`pan`), and the
//! speaker bed is heard on a stereo output either binaurally (`binaural`)
//! or folded left and right ([`Monitor`]).

pub mod binaural;
pub mod pan;

pub use af27_core::space::{Monitor, SpaceLayout, SpacePos};
pub use binaural::Binaural;
pub use pan::{fold_gains, lfe_channel, point_gains, source_gains, speakers};

/// Turns a speaker bed into stereo for listening.
pub enum Monitoring {
    Binaural(Box<Binaural>),
    /// Per speaker: (left, right) gain.
    Fold(Vec<[f32; 2]>),
}

impl Monitoring {
    pub fn new(layout: SpaceLayout, monitor: Monitor, rate: u32) -> Monitoring {
        match monitor {
            Monitor::Binaural => Monitoring::Binaural(Box::new(Binaural::new(layout, rate))),
            Monitor::StereoFold => Monitoring::Fold(fold_bed(layout)),
        }
    }

    /// `bed` is interleaved speaker channels; `out` interleaved stereo.
    pub fn process(&mut self, bed: &[f32], out: &mut [f32]) {
        match self {
            Monitoring::Binaural(b) => b.process(bed, out),
            Monitoring::Fold(g) => {
                let n = g.len();
                for (f, o) in out.chunks_exact_mut(2).enumerate() {
                    let frame = &bed[f * n..(f + 1) * n];
                    let (mut l, mut r) = (0.0, 0.0);
                    for (s, gains) in frame.iter().zip(g.iter()) {
                        l += s * gains[0];
                        r += s * gains[1];
                    }
                    o[0] = l;
                    o[1] = r;
                }
            }
        }
    }
}

/// Fold gains per speaker: by where it stands left to right, sharing
/// power, with the low-frequency channel to both sides at -6 dB.
pub fn fold_bed(layout: SpaceLayout) -> Vec<[f32; 2]> {
    speakers(layout)
        .iter()
        .map(|s| match s.at {
            Some([x, _, _]) => {
                let a = ((x + 1.0) / 2.0).clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
                [a.cos(), a.sin()]
            }
            None => [0.5, 0.5],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_sends_left_speakers_left() {
        let g = fold_bed(SpaceLayout::Surround51);
        assert!((g[0][0] - 1.0).abs() < 1e-6 && g[0][1].abs() < 1e-6);
        assert!((g[2][0] - g[2][1]).abs() < 1e-6);
        let mut m = Monitoring::new(SpaceLayout::Surround51, Monitor::StereoFold, 48_000);
        let bed = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut out = [0.0; 2];
        m.process(&bed, &mut out);
        assert!((out[0] - 1.0).abs() < 1e-6 && out[1].abs() < 1e-6);
    }
}
