// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Everything a source needs to go onto the speaker bed: its direct gains
//! per side, its low-frequency send, and the Spread and Spatialize feeds
//! (see `diffuse`). Worked out from a spot without allocating, so the
//! audio thread can redo it as automation moves the spot.

use crate::pan::{lfe_channel, source_gains_into, speakers, Gains, MAX_SPEAKERS};
use af2_core::space::{SpaceLayout, SpacePos, SpatializeMode};

/// A source's placement on the bed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Per side (left, right), per speaker: the placed sound's gain, with
    /// the share Spread and Spatialize take already out.
    pub bed: [Gains; 2],
    /// Gain of (left + right) into the low-frequency channel.
    pub lfe: f32,
    /// Per speaker: the decorrelated middle (Spread).
    pub spread: Gains,
    /// Per speaker, signed: the side signal (Spatialize, all modes but
    /// Place Elements). Positive on the left of the room.
    pub side: Gains,
    /// The middle, plainly, to the front centre speaker (Spatialize).
    pub center: f32,
    pub center_channel: Option<usize>,
    pub mode: SpatializeMode,
    /// Place Elements amount (0 off).
    pub steer: f32,
}

impl Placement {
    pub fn silent() -> Placement {
        Placement {
            bed: [[0.0; MAX_SPEAKERS]; 2],
            lfe: 0.0,
            spread: [0.0; MAX_SPEAKERS],
            side: [0.0; MAX_SPEAKERS],
            center: 0.0,
            center_channel: None,
            mode: SpatializeMode::SideSpread,
            steer: 0.0,
        }
    }

    /// A source at `pos`. `stereo` sources play from two points and can
    /// be spatialized; mono ones play from one.
    pub fn new(layout: SpaceLayout, pos: &SpacePos, stereo: bool) -> Placement {
        let pos = pos.tidy();
        let sp = speakers(layout);
        let has_top = sp.iter().any(|s| s.at.is_some_and(|a| a[2] > 0.5));
        let mut p = Placement::silent();
        p.bed = source_gains_into(layout, &pos, stereo);
        p.mode = pos.spatialize_mode;
        if lfe_channel(layout).is_some() {
            p.lfe = pos.lfe as f32 * 0.5;
        }
        let mut direct = 1.0f32;

        // Spread: the middle, decorrelated, over the speakers of the
        // height the source sits at.
        let spread = pos.spread as f32;
        if spread > 0.0 {
            let lift = if has_top {
                (pos.z as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut w = [0.0f32; MAX_SPEAKERS];
            for (i, s) in sp.iter().enumerate() {
                if let Some(a) = s.at {
                    w[i] = if a[2] > 0.5 {
                        2.0 * lift.min(0.5)
                    } else {
                        1.0 - (lift - 0.5).max(0.0)
                    };
                }
            }
            let norm = w.iter().map(|v| v * v).sum::<f32>().sqrt();
            if norm > 0.0 {
                for (o, v) in p.spread.iter_mut().zip(w) {
                    *o = spread.sqrt() * v / norm;
                }
            }
            direct *= (1.0 - spread).sqrt();
        }

        let spat = if stereo { pos.spatialize as f32 } else { 0.0 };
        if spat > 0.0 {
            if p.mode == SpatializeMode::Steer {
                // Equal power between the placed and the steered sound.
                p.steer = spat.sqrt();
                direct *= (1.0 - spat).sqrt();
            } else {
                let heights_only = p.mode == SpatializeMode::HeightLift && has_top;
                let mut w = [0.0f32; MAX_SPEAKERS];
                for (i, s) in sp.iter().enumerate() {
                    let Some(a) = s.at else { continue };
                    let eligible = if heights_only {
                        a[2] > 0.5
                    } else {
                        a[1] < 0.5 || a[2] > 0.5
                    };
                    if eligible {
                        w[i] = if a[0] < -0.1 {
                            1.0
                        } else if a[0] > 0.1 {
                            -1.0
                        } else {
                            0.0
                        };
                    }
                }
                let norm = w.iter().map(|v| v * v).sum::<f32>().sqrt();
                if norm > 0.0 {
                    for (o, v) in p.side.iter_mut().zip(w) {
                        *o = spat * 0.8 * v / norm;
                    }
                }
                p.center_channel = sp.iter().position(|s| {
                    s.at.is_some_and(|a| a[0].abs() < 0.1 && a[1] >= 0.5 && a[2] < 0.5)
                });
                if p.center_channel.is_some() {
                    p.center = spat * 0.5;
                }
                direct *= 1.0 - 0.3 * spat;
            }
        }
        for side in p.bed.iter_mut() {
            for v in side.iter_mut() {
                *v *= direct;
            }
        }
        p
    }

    /// Whether it feeds anything beyond the placed sound.
    pub fn has_field(&self) -> bool {
        self.spread.iter().any(|v| *v != 0.0)
            || self.side.iter().any(|v| *v != 0.0)
            || self.center != 0.0
    }

    /// Placement of a send into a 3D Bus: the spot only.
    pub fn spot_only(&self) -> Placement {
        Placement {
            spread: [0.0; MAX_SPEAKERS],
            side: [0.0; MAX_SPEAKERS],
            center: 0.0,
            steer: 0.0,
            ..*self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn power(g: &[f32]) -> f32 {
        g.iter().map(|v| v * v).sum()
    }

    #[test]
    fn fields_keep_power_and_spread_where_they_should() {
        let pos = SpacePos {
            spread: 0.5,
            ..SpacePos::default()
        };
        let p = Placement::new(SpaceLayout::Surround714, &pos, false);
        // Direct (two half-gain sides of one point) plus spread: unit power.
        let direct = power(&p.bed[0]) * 4.0;
        assert!((direct + power(&p.spread) - 1.0).abs() < 1e-4);
        // At ear level only the floor speakers, never the LFE.
        assert_eq!(p.spread[3], 0.0);
        assert!(p.spread[8..12].iter().all(|v| *v == 0.0));
        assert!(p.spread[0] > 0.0 && p.spread[4] > 0.0);
        // Raised to the ceiling: the top full, the floor half.
        let up = Placement::new(SpaceLayout::Surround714, &SpacePos { z: 1.0, ..pos }, false);
        assert!((up.spread[8] / up.spread[0] - 2.0).abs() < 1e-4);
    }

    #[test]
    fn spatialize_feeds_the_sides_and_centre() {
        let pos = SpacePos {
            spatialize: 1.0,
            ..SpacePos::default()
        };
        let p = Placement::new(SpaceLayout::Surround71, &pos, true);
        // Not the front row; left of the room in phase, right inverted.
        assert_eq!((p.side[0], p.side[1], p.side[2]), (0.0, 0.0, 0.0));
        assert!(p.side[4] > 0.0 && p.side[6] > 0.0);
        assert!(p.side[5] < 0.0 && p.side[7] < 0.0);
        assert_eq!(p.center_channel, Some(2));
        assert!((p.center - 0.5).abs() < 1e-6);
        // Height Lift: only the raised speakers.
        let h = Placement::new(
            SpaceLayout::Surround714,
            &SpacePos {
                spatialize_mode: SpatializeMode::HeightLift,
                ..pos
            },
            true,
        );
        assert!(h.side[4..8].iter().all(|v| *v == 0.0));
        assert!(h.side[8] > 0.0 && h.side[9] < 0.0);
        // Mono sources are not spatialized.
        let m = Placement::new(SpaceLayout::Surround71, &pos, false);
        assert!(!m.has_field());
        // Place Elements shares power with the placed sound.
        let s = Placement::new(
            SpaceLayout::Surround71,
            &SpacePos {
                spatialize: 0.5,
                spatialize_mode: SpatializeMode::Steer,
                ..pos
            },
            true,
        );
        assert!((s.steer * s.steer + power(&s.bed[0]) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn lfe_send_is_half_per_side_and_absent_in_2_0() {
        let pos = SpacePos {
            lfe: 1.0,
            ..SpacePos::default()
        };
        assert_eq!(
            Placement::new(SpaceLayout::Surround51, &pos, false).lfe,
            0.5
        );
        assert_eq!(Placement::new(SpaceLayout::Stereo20, &pos, false).lfe, 0.0);
    }
}
