// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! 3D Space session data: the speaker layout and where each track sits.
//! The room is a box: x runs left (-1) to right (1), y back (-1) to front
//! (1), z ear level (0) to ceiling (1). The panning itself is in
//! `af27-space`.

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
}

/// How a 3D session is heard on a stereo output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Monitor {
    /// Each speaker heard around a head, for headphones.
    #[default]
    Binaural,
    /// Each speaker folded left or right by where it stands.
    StereoFold,
}

/// Where a track sits in the room, and how big it sounds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpacePos {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// 0 is a point; 1 fills every speaker of its layer.
    pub size: f64,
    /// A stereo source's sides sit this far either side of its spot.
    pub width: f64,
    /// Turns a stereo pair, degrees clockwise seen from above.
    pub rotation: f64,
    /// Send to the low-frequency channel, dB (below -60 is off).
    pub lfe_db: f64,
}

impl Default for SpacePos {
    fn default() -> Self {
        SpacePos {
            x: 0.0,
            y: 1.0,
            z: 0.0,
            size: 0.0,
            width: 0.5,
            rotation: 0.0,
            lfe_db: -96.0,
        }
    }
}
