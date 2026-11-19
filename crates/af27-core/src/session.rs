// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) The Audiform27 contributors.

//! The multitrack session: tracks hold clips that point at media items.
//! Editing is non-destructive: clips only reference a span of a source.
//! Positions are in seconds so sources of any sample rate can mix.
//!
//! Saved as versioned JSON (`.af27`). Audio data is never stored in the
//! session file, only the paths of the media it uses.

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

pub type MediaId = u64;
pub type TrackId = u64;
pub type ClipId = u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: MediaId,
    pub name: String,
    /// Where the audio lives on disk. None for audio made in the app
    /// that has not been saved yet.
    pub path: Option<std::path::PathBuf>,
    pub duration: f64,
    pub rate: u32,
    pub channels: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub media: MediaId,
    pub name: String,
    /// Where the clip starts on the timeline, in seconds.
    pub start: f64,
    /// Where in the source the clip's audio starts, in seconds.
    pub offset: f64,
    /// How long the clip is, in seconds.
    pub length: f64,
    #[serde(default)]
    pub gain_db: f32,
    #[serde(default)]
    pub fade_in: f64,
    #[serde(default)]
    pub fade_out: f64,
    #[serde(default)]
    pub muted: bool,
}

impl Clip {
    pub fn end(&self) -> f64 {
        self.start + self.length
    }

    /// Clip gain at timeline time `t` (seconds), fades included.
    pub fn gain_at(&self, t: f64) -> f32 {
        if self.muted {
            return 0.0;
        }
        let local = t - self.start;
        let mut g = 1.0f64;
        if self.fade_in > 0.0 && local < self.fade_in {
            g *= (local / self.fade_in).clamp(0.0, 1.0);
        }
        let to_end = self.length - local;
        if self.fade_out > 0.0 && to_end < self.fade_out {
            g *= (to_end / self.fade_out).clamp(0.0, 1.0);
        }
        (g as f32) * crate::time::db_to_gain(self.gain_db)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub name: String,
    #[serde(default)]
    pub volume_db: f32,
    /// -1 (left) to 1 (right).
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub armed: bool,
    /// RGB colour of the track's clips.
    pub color: [u8; 3],
    #[serde(default)]
    pub clips: Vec<Clip>,
    #[serde(default = "default_height")]
    pub height: f32,
}

fn default_height() -> f32 {
    84.0
}

impl Track {
    /// Constant-power pan gains (left, right) times the track volume.
    pub fn gains(&self) -> (f32, f32) {
        let v = crate::time::db_to_gain(self.volume_db);
        let p = (self.pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
        (
            v * p.cos() * std::f32::consts::SQRT_2,
            v * p.sin() * std::f32::consts::SQRT_2,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub name: String,
    pub time: f64,
    #[serde(default)]
    pub length: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub schema_version: u32,
    pub name: String,
    pub rate: u32,
    #[serde(default)]
    pub master_db: f32,
    pub media: Vec<MediaItem>,
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub markers: Vec<Marker>,
    #[serde(default)]
    pub next_id: u64,
}

const PALETTE: [[u8; 3]; 8] = [
    [86, 182, 194],
    [222, 142, 86],
    [146, 196, 104],
    [196, 120, 186],
    [232, 196, 92],
    [120, 140, 222],
    [214, 102, 112],
    [110, 200, 160],
];

impl Default for Session {
    fn default() -> Self {
        Session::new("Untitled Session", 48_000, 6)
    }
}

impl Session {
    pub fn new(name: &str, rate: u32, tracks: usize) -> Self {
        let mut s = Session {
            schema_version: SCHEMA_VERSION,
            name: name.into(),
            rate,
            master_db: 0.0,
            media: Vec::new(),
            tracks: Vec::new(),
            markers: Vec::new(),
            next_id: 1,
        };
        for _ in 0..tracks {
            s.add_track();
        }
        s
    }

    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        id
    }

    pub fn add_track(&mut self) -> TrackId {
        let id = self.alloc_id();
        let n = self.tracks.len();
        self.tracks.push(Track {
            id,
            name: format!("Track {}", n + 1),
            volume_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            armed: false,
            color: PALETTE[n % PALETTE.len()],
            clips: Vec::new(),
            height: default_height(),
        });
        id
    }

    pub fn media(&self, id: MediaId) -> Option<&MediaItem> {
        self.media.iter().find(|m| m.id == id)
    }

    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    /// Finds a clip by id: (track index, clip index).
    pub fn find_clip(&self, id: ClipId) -> Option<(usize, usize)> {
        self.tracks
            .iter()
            .enumerate()
            .find_map(|(ti, t)| t.clips.iter().position(|c| c.id == id).map(|ci| (ti, ci)))
    }

    /// Places a clip of the whole of `media` on `track` at `start`.
    pub fn add_clip(&mut self, track: TrackId, media: MediaId, start: f64) -> Option<ClipId> {
        let m = self.media(media)?.clone();
        let id = self.alloc_id();
        let t = self.track_mut(track)?;
        t.clips.push(Clip {
            id,
            media,
            name: m.name.clone(),
            start: start.max(0.0),
            offset: 0.0,
            length: m.duration,
            gain_db: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
        });
        Some(id)
    }

    /// Splits the clip at timeline time `t`. Returns the new right part.
    pub fn split_clip(&mut self, id: ClipId, t: f64) -> Option<ClipId> {
        let (ti, ci) = self.find_clip(id)?;
        let c = self.tracks[ti].clips[ci].clone();
        if t <= c.start + 1e-6 || t >= c.end() - 1e-6 {
            return None;
        }
        let new_id = self.alloc_id();
        let left_len = t - c.start;
        let right = Clip {
            id: new_id,
            start: t,
            offset: c.offset + left_len,
            length: c.length - left_len,
            fade_in: 0.0,
            ..c.clone()
        };
        let left = &mut self.tracks[ti].clips[ci];
        left.length = left_len;
        left.fade_out = 0.0;
        self.tracks[ti].clips.insert(ci + 1, right);
        Some(new_id)
    }

    pub fn remove_clip(&mut self, id: ClipId) -> Option<Clip> {
        let (ti, ci) = self.find_clip(id)?;
        Some(self.tracks[ti].clips.remove(ci))
    }

    /// End of the last clip, in seconds.
    pub fn duration(&self) -> f64 {
        self.tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .map(Clip::end)
            .fold(0.0, f64::max)
    }

    pub fn any_solo(&self) -> bool {
        self.tracks.iter().any(|t| t.solo)
    }

    /// Whether a track is heard, given mute and solo states.
    pub fn audible(&self, t: &Track) -> bool {
        !t.mute && (!self.any_solo() || t.solo)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("session serialises")
    }

    pub fn from_json(text: &str) -> Result<Session, String> {
        let v: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("Not a session file: {e}"))?;
        let version = v
            .get("schema_version")
            .and_then(|x| x.as_u64())
            .unwrap_or(0);
        if version > SCHEMA_VERSION as u64 {
            return Err(format!(
                "This session was saved by a newer Audiform27 (format {version})."
            ));
        }
        serde_json::from_value(v).map_err(|e| format!("Session file is damaged: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_media() -> (Session, MediaId) {
        let mut s = Session::new("t", 48_000, 2);
        let id = s.alloc_id();
        s.media.push(MediaItem {
            id,
            name: "voice".into(),
            path: Some("voice.wav".into()),
            duration: 10.0,
            rate: 44_100,
            channels: 1,
        });
        (s, id)
    }

    #[test]
    fn split_keeps_audio_continuous() {
        let (mut s, m) = with_media();
        let t = s.tracks[0].id;
        let c = s.add_clip(t, m, 2.0).unwrap();
        let r = s.split_clip(c, 5.0).unwrap();
        let (ti, ci) = s.find_clip(r).unwrap();
        let right = &s.tracks[ti].clips[ci];
        assert_eq!(right.start, 5.0);
        assert_eq!(right.offset, 3.0);
        assert_eq!(right.length, 7.0);
        assert_eq!(s.tracks[0].clips[0].length, 3.0);
        assert!(s.split_clip(c, 1.0).is_none());
    }

    #[test]
    fn json_round_trip() {
        let (mut s, m) = with_media();
        let t = s.tracks[1].id;
        s.add_clip(t, m, 1.5).unwrap();
        s.markers.push(Marker {
            name: "Intro".into(),
            time: 1.0,
            length: 0.0,
        });
        let back = Session::from_json(&s.to_json()).unwrap();
        assert_eq!(back, s);
        assert!((back.duration() - 11.5).abs() < 1e-9);
    }

    #[test]
    fn newer_files_are_refused() {
        let text =
            r#"{"schema_version": 99, "name": "x", "rate": 48000, "media": [], "tracks": []}"#;
        assert!(Session::from_json(text).is_err());
    }

    #[test]
    fn fades_and_pan() {
        let c = Clip {
            id: 1,
            media: 1,
            name: String::new(),
            start: 0.0,
            offset: 0.0,
            length: 4.0,
            gain_db: 0.0,
            fade_in: 1.0,
            fade_out: 1.0,
            muted: false,
        };
        assert!((c.gain_at(0.5) - 0.5).abs() < 1e-6);
        assert!((c.gain_at(2.0) - 1.0).abs() < 1e-6);
        assert!((c.gain_at(3.5) - 0.5).abs() < 1e-6);
        let (s, _) = with_media();
        let (l, r) = s.tracks[0].gains();
        assert!((l - 1.0).abs() < 1e-5 && (r - 1.0).abs() < 1e-5);
    }
}
