// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! 3D Space session data: the speaker layout and where each track sits.
//! The room is a box: x runs left (-1) to right (1), y back (-1) to front
//! (1), z ear level (0) to ceiling (1), down to -1 below ear level. The
//! panning itself is in `af27-space`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpaceLayout {
    Stereo20,
    Surround51,
    Surround71,
    Surround512,
    Surround514,
    Surround712,
    Surround714,
}

impl SpaceLayout {
    pub const ALL: [SpaceLayout; 7] = [
        SpaceLayout::Stereo20,
        SpaceLayout::Surround51,
        SpaceLayout::Surround71,
        SpaceLayout::Surround512,
        SpaceLayout::Surround514,
        SpaceLayout::Surround712,
        SpaceLayout::Surround714,
    ];

    /// The name a file uses for it ("5.1", "7.1.4").
    pub fn label(self) -> &'static str {
        match self {
            SpaceLayout::Stereo20 => "2.0",
            SpaceLayout::Surround51 => "5.1",
            SpaceLayout::Surround71 => "7.1",
            SpaceLayout::Surround512 => "5.1.2",
            SpaceLayout::Surround514 => "5.1.4",
            SpaceLayout::Surround712 => "7.1.2",
            SpaceLayout::Surround714 => "7.1.4",
        }
    }

    pub fn from_label(s: &str) -> Option<SpaceLayout> {
        SpaceLayout::ALL.into_iter().find(|l| l.label() == s)
    }
}

/// How a 3D session is heard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Monitor {
    /// Each speaker heard around a head, for headphones.
    #[default]
    Binaural,
    /// Each speaker folded left or right by where it stands.
    StereoFold,
    /// Each speaker on its own output of the audio device.
    Speakers,
}

/// How Spatialize spreads a stereo source into the room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SpatializeMode {
    /// The difference of the sides, smeared, to the speakers beside and
    /// behind.
    #[default]
    SideSpread,
    /// The difference of the sides, plainly delayed, beside and behind.
    Matrix,
    /// Each side, smeared, to its own side of the room.
    Ambience,
    /// Like Side Spread, to the raised speakers only.
    HeightLift,
    /// Place Elements: every part of the sound to where it sits.
    Steer,
}

impl SpatializeMode {
    pub const ALL: [SpatializeMode; 5] = [
        SpatializeMode::SideSpread,
        SpatializeMode::Matrix,
        SpatializeMode::Ambience,
        SpatializeMode::HeightLift,
        SpatializeMode::Steer,
    ];

    /// The name a file uses for it.
    pub fn key(self) -> &'static str {
        match self {
            SpatializeMode::SideSpread => "side_spread",
            SpatializeMode::Matrix => "matrix",
            SpatializeMode::Ambience => "ambience",
            SpatializeMode::HeightLift => "height_lift",
            SpatializeMode::Steer => "steer",
        }
    }

    /// Unknown names are Side Spread.
    pub fn from_key(s: &str) -> SpatializeMode {
        SpatializeMode::ALL
            .into_iter()
            .find(|m| m.key() == s)
            .unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            SpatializeMode::SideSpread => "Side Spread",
            SpatializeMode::Matrix => "Matrix",
            SpatializeMode::Ambience => "Ambience",
            SpatializeMode::HeightLift => "Height Lift",
            SpatializeMode::Steer => "Place Elements",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            SpatializeMode::SideSpread => {
                "What differs between left and right, smeared, to the speakers beside and behind"
            }
            SpatializeMode::Matrix => {
                "What differs between left and right, delayed a little, beside and behind"
            }
            SpatializeMode::Ambience => "Each side, smeared, to its own side of the room",
            SpatializeMode::HeightLift => {
                "What differs between left and right, smeared, to the raised speakers"
            }
            SpatializeMode::Steer => {
                "Every part of the sound to where it sits in the stereo picture; ambience around the room"
            }
        }
    }
}

/// Where a track sits in the room, and how big it sounds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpacePos {
    pub x: f64,
    pub y: f64,
    /// Ear level 0, ceiling 1, down to -1 below ear level.
    pub z: f64,
    /// 0 is a point; 1 fills every speaker of its layer.
    pub size: f64,
    /// A stereo source's sides sit this far either side of its spot.
    pub width: f64,
    /// Turns a stereo pair, degrees clockwise seen from above.
    pub rotation: f64,
    /// Send to the low-frequency channel, 0..1 (full level at 1).
    pub lfe: f64,
    /// Share sent as a diffuse field, 0..1.
    pub spread: f64,
    /// Stereo sources: how much is spread into the room, 0..1.
    pub spatialize: f64,
    pub spatialize_mode: SpatializeMode,
}

impl Default for SpacePos {
    /// Front centre, as wide as the front speakers.
    fn default() -> Self {
        SpacePos {
            x: 0.0,
            y: 1.0,
            z: 0.0,
            size: 0.0,
            width: 1.0,
            rotation: 0.0,
            lfe: 0.0,
            spread: 0.0,
            spatialize: 0.0,
            spatialize_mode: SpatializeMode::SideSpread,
        }
    }
}

impl SpacePos {
    /// Every value in range; values that are not numbers take the default.
    pub fn tidy(&self) -> SpacePos {
        let d = SpacePos::default();
        let fix = |v: f64, or: f64, lo: f64, hi: f64| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                or
            }
        };
        SpacePos {
            x: fix(self.x, d.x, -1.0, 1.0),
            y: fix(self.y, d.y, -1.0, 1.0),
            z: fix(self.z, d.z, -1.0, 1.0),
            size: fix(self.size, d.size, 0.0, 1.0),
            width: fix(self.width, d.width, 0.0, 1.0),
            rotation: fix(self.rotation, d.rotation, -180.0, 180.0),
            lfe: fix(self.lfe, d.lfe, 0.0, 1.0),
            spread: fix(self.spread, d.spread, 0.0, 1.0),
            spatialize: fix(self.spatialize, d.spatialize, 0.0, 1.0),
            spatialize_mode: self.spatialize_mode,
        }
    }

    pub fn get(&self, p: SpaceParam) -> f64 {
        match p {
            SpaceParam::X => self.x,
            SpaceParam::Y => self.y,
            SpaceParam::Z => self.z,
            SpaceParam::Size => self.size,
            SpaceParam::Width => self.width,
            SpaceParam::Rotation => self.rotation,
            SpaceParam::Lfe => self.lfe,
            SpaceParam::Spread => self.spread,
            SpaceParam::Spatialize => self.spatialize,
        }
    }

    pub fn set(&mut self, p: SpaceParam, v: f64) {
        match p {
            SpaceParam::X => self.x = v,
            SpaceParam::Y => self.y = v,
            SpaceParam::Z => self.z = v,
            SpaceParam::Size => self.size = v,
            SpaceParam::Width => self.width = v,
            SpaceParam::Rotation => self.rotation = v,
            SpaceParam::Lfe => self.lfe = v,
            SpaceParam::Spread => self.spread = v,
            SpaceParam::Spatialize => self.spatialize = v,
        }
    }

    /// The spot with each automated value at time `t` (seconds); values
    /// without a lane keep the spot's own.
    pub fn at_time(&self, lanes: &[SpaceLane], t: f64) -> SpacePos {
        let mut p = *self;
        for l in lanes {
            if let Some(v) = l.value_at(t) {
                p.set(l.param, v);
            }
        }
        p
    }
}

/// A spot value that automation can move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpaceParam {
    X,
    Y,
    Z,
    Size,
    Width,
    Rotation,
    Lfe,
    Spread,
    Spatialize,
}

impl SpaceParam {
    pub const ALL: [SpaceParam; 9] = [
        SpaceParam::X,
        SpaceParam::Y,
        SpaceParam::Z,
        SpaceParam::Size,
        SpaceParam::Width,
        SpaceParam::Rotation,
        SpaceParam::Lfe,
        SpaceParam::Spread,
        SpaceParam::Spatialize,
    ];

    /// The name a file uses for it.
    pub fn key(self) -> &'static str {
        match self {
            SpaceParam::X => "x",
            SpaceParam::Y => "y",
            SpaceParam::Z => "z",
            SpaceParam::Size => "size",
            SpaceParam::Width => "width",
            SpaceParam::Rotation => "rotation",
            SpaceParam::Lfe => "lfe",
            SpaceParam::Spread => "spread",
            SpaceParam::Spatialize => "spatialize",
        }
    }

    pub fn from_key(s: &str) -> Option<SpaceParam> {
        SpaceParam::ALL.into_iter().find(|p| p.key() == s)
    }

    pub fn range(self) -> (f64, f64) {
        match self {
            SpaceParam::X | SpaceParam::Y | SpaceParam::Z => (-1.0, 1.0),
            SpaceParam::Rotation => (-180.0, 180.0),
            _ => (0.0, 1.0),
        }
    }
}

/// How a lane moves from one point to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LaneCurve {
    #[default]
    Linear,
    /// Keeps the point's value until the next point.
    Hold,
}

/// One point of a spot lane: time in seconds, value in the param's units.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LanePoint {
    pub t: f64,
    pub v: f64,
    #[serde(default)]
    pub curve: LaneCurve,
}

/// Automation of one spot value. Kept apart from any other automation so
/// the 3D Space code owns it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpaceLane {
    pub param: SpaceParam,
    /// Sorted by time.
    pub points: Vec<LanePoint>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl SpaceLane {
    /// The lane's value at `t`: straight between points (or held, for a
    /// hold point), flat before the first and after the last. None when
    /// the lane is off or empty.
    pub fn value_at(&self, t: f64) -> Option<f64> {
        if !self.enabled {
            return None;
        }
        let p = &self.points;
        let first = p.first()?;
        if t <= first.t {
            return Some(first.v);
        }
        let last = p.last()?;
        if t >= last.t {
            return Some(last.v);
        }
        let i = p.partition_point(|q| q.t <= t).saturating_sub(1);
        let (a, b) = (p[i], p[(i + 1).min(p.len() - 1)]);
        if a.curve == LaneCurve::Hold || b.t <= a.t {
            return Some(a.v);
        }
        Some(a.v + (b.v - a.v) * (t - a.t) / (b.t - a.t))
    }
}

/// A track's 3D Space settings beyond its spot.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackSpace {
    /// A 3D Bus: its inputs are placed on its own speaker bed and its
    /// effects run on every speaker. A plain stereo bus outside 3D Space.
    pub room: bool,
    /// Spot automation (from an Audiform session's envelopes).
    pub lanes: Vec<SpaceLane>,
    /// The file had a spot for this track (so saving writes it back even
    /// when it is the default).
    pub had_spot: bool,
    /// Whether it is placed as a stereo pair (true) or one point, as the
    /// file says (its `channels` and Mono button). None: by its clips.
    pub stereo: Option<bool>,
}

impl TrackSpace {
    pub fn is_default(&self) -> bool {
        *self == TrackSpace::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidy_clamps_and_replaces_nan() {
        let p = SpacePos {
            x: 3.0,
            y: f64::NAN,
            z: -4.0,
            size: 2.0,
            width: f64::INFINITY,
            rotation: 400.0,
            lfe: -1.0,
            spread: 0.5,
            spatialize: 9.0,
            spatialize_mode: SpatializeMode::Matrix,
        }
        .tidy();
        assert_eq!((p.x, p.y, p.z, p.size, p.width), (1.0, 1.0, -1.0, 1.0, 1.0));
        assert_eq!(
            (p.rotation, p.lfe, p.spread, p.spatialize),
            (180.0, 0.0, 0.5, 1.0)
        );
        assert_eq!(p.spatialize_mode, SpatializeMode::Matrix);
    }

    #[test]
    fn defaults_are_front_centre_full_width() {
        let p = SpacePos::default();
        assert_eq!((p.x, p.y, p.z, p.width), (0.0, 1.0, 0.0, 1.0));
        let q: SpacePos = serde_json::from_str(r#"{"x":0.5}"#).unwrap();
        assert_eq!((q.x, q.y, q.width), (0.5, 1.0, 1.0));
    }

    #[test]
    fn names_round_trip() {
        for l in SpaceLayout::ALL {
            assert_eq!(SpaceLayout::from_label(l.label()), Some(l));
        }
        for m in SpatializeMode::ALL {
            assert_eq!(SpatializeMode::from_key(m.key()), m);
        }
        assert_eq!(
            SpatializeMode::from_key("nonsense"),
            SpatializeMode::SideSpread
        );
        for p in SpaceParam::ALL {
            assert_eq!(SpaceParam::from_key(p.key()), Some(p));
        }
        assert_eq!(SpaceParam::Rotation.range(), (-180.0, 180.0));
    }

    #[test]
    fn lanes_interpolate_and_hold() {
        let mut lane = SpaceLane {
            param: SpaceParam::X,
            points: vec![
                LanePoint {
                    t: 1.0,
                    v: -1.0,
                    curve: LaneCurve::Linear,
                },
                LanePoint {
                    t: 3.0,
                    v: 1.0,
                    curve: LaneCurve::Hold,
                },
                LanePoint {
                    t: 5.0,
                    v: 0.0,
                    curve: LaneCurve::Linear,
                },
            ],
            enabled: true,
        };
        assert_eq!(lane.value_at(0.0), Some(-1.0));
        assert_eq!(lane.value_at(2.0), Some(0.0));
        assert_eq!(lane.value_at(4.0), Some(1.0));
        assert_eq!(lane.value_at(9.0), Some(0.0));
        let p = SpacePos::default().at_time(std::slice::from_ref(&lane), 2.0);
        assert_eq!((p.x, p.y), (0.0, 1.0));
        lane.enabled = false;
        assert_eq!(lane.value_at(2.0), None);
    }
}
