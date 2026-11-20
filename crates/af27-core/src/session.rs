// SPDX-License-Identifier: GPL-3.0-or-later
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
    /// The object's id in an `.audfrm` file it came from, so saving back
    /// keeps everything Audiform27 does not use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext_id: Option<String>,
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
    /// Stretch, pitch and the other per-clip settings.
    #[serde(flatten)]
    pub props: ClipProps,
    /// The object's id in an `.audfrm` file it came from, so saving back
    /// keeps everything Audiform27 does not use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext_id: Option<String>,
}

/// How a clip is stretched in time and shifted in pitch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StretchMode {
    #[default]
    Off,
    /// Processed while playing.
    Live,
    /// Processed once and kept (sounds the same as Live).
    Rendered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StretchKind {
    /// One voice or instrument at a time.
    #[default]
    Monophonic,
    /// Chords and full mixes.
    Polyphonic,
    /// Tape-style: speed and pitch change together.
    Varispeed,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipStretch {
    pub mode: StretchMode,
    pub kind: StretchKind,
    /// Timeline length over source length (2.0 plays half speed). The
    /// clip's `length` is on the timeline; it covers `length / ratio`
    /// seconds of the source from `offset`.
    pub ratio: f64,
    /// Pitch shift in semitones (-24..24). Varispeed: follows the ratio.
    pub pitch: f32,
    /// Analysis window in milliseconds (5..200).
    pub window_ms: f32,
    /// Transient sensitivity 0..100.
    pub transients: f32,
    pub preserve_formants: bool,
    pub high_precision: bool,
}

impl Default for ClipStretch {
    fn default() -> Self {
        ClipStretch {
            mode: StretchMode::Off,
            kind: StretchKind::Monophonic,
            ratio: 1.0,
            pitch: 0.0,
            window_ms: 30.0,
            transients: 40.0,
            preserve_formants: true,
            high_precision: true,
        }
    }
}

impl ClipStretch {
    pub fn is_on(&self) -> bool {
        self.mode != StretchMode::Off
            && ((self.ratio - 1.0).abs() > 1e-9 || self.pitch.abs() > 1e-6)
    }
    /// The ratio actually applied (1 when off).
    pub fn applied_ratio(&self) -> f64 {
        if self.mode == StretchMode::Off {
            1.0
        } else {
            self.ratio.clamp(0.25, 4.0)
        }
    }
    /// The pitch actually applied in semitones (Varispeed: from the ratio).
    pub fn applied_pitch(&self) -> f32 {
        match (self.mode, self.kind) {
            (StretchMode::Off, _) => 0.0,
            (_, StretchKind::Varispeed) => (-12.0 * self.applied_ratio().log2()) as f32,
            _ => self.pitch,
        }
    }
}

/// How much of one source channel a clip sends to each side (Source
/// Channels), -1..1 each: 1 is all of it, a negative amount flips its
/// polarity.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChannelSend {
    pub left: f32,
    pub right: f32,
}

impl ChannelSend {
    /// Flipped polarity: both amounts below zero (or one, with the other
    /// silent).
    pub fn flipped(&self) -> bool {
        self.left < 0.0 || self.right < 0.0
    }
}

/// The usual Source Channels of a file with `channels` channels: mono on
/// both sides, stereo left to left and right to right; wider files fold
/// (see the engine) and have no table of their own here, so they list
/// their first two channels left and right and the rest silent.
pub fn usual_channel_map(channels: usize) -> Vec<ChannelSend> {
    match channels {
        0 => Vec::new(),
        1 => vec![ChannelSend {
            left: 1.0,
            right: 1.0,
        }],
        n => (0..n)
            .map(|k| ChannelSend {
                left: if k == 0 { 1.0 } else { 0.0 },
                right: if k == 1 { 1.0 } else { 0.0 },
            })
            .collect(),
    }
}

/// Per-clip settings beyond position and level.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipProps {
    #[serde(skip_serializing_if = "is_default")]
    pub stretch: ClipStretch,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub reversed: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub inverted: bool,
    /// Cannot be moved.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// Its own colour instead of the track's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
    /// Fade shapes: -100..100 skew (0 straight).
    #[serde(skip_serializing_if = "is_default")]
    pub fade_in_skew: f32,
    #[serde(skip_serializing_if = "is_default")]
    pub fade_out_skew: f32,
    /// 3D Space: a clip of more than two channels folds to stereo (its
    /// file had a Source Channels mix) instead of playing each channel
    /// from its speaker.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fold_channels: bool,
    /// Clip automation: volume (dB, on top of the clip gain), pan
    /// (-1..1) and the clip's own effect values. Point times count from
    /// the clip's left edge, so the lanes move with the clip.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lanes: Vec<crate::automation::Lane>,
    /// The clip's own effects, run on its sound before the track's.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub chain: Vec<EffectSlot>,
    /// The clip chain's power, input and output gain and mix.
    #[serde(skip_serializing_if = "is_default")]
    pub rack: Rack,
    /// Source Channels: per channel of the clip's file, how much goes to
    /// the left and the right. None plays the file as usual.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_map: Option<Vec<ChannelSend>>,
}

fn default_bit_depth() -> u16 {
    32
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// A fade's gain at `x` (0 at the silent end, 1 at full level) with a
/// skew of -100..100: 0 is the straight line (exactly `x`), positive
/// bows the curve up (louder sooner), negative bows it down.
#[inline]
pub fn fade_curve(x: f64, skew: f32) -> f64 {
    let x = x.clamp(0.0, 1.0);
    if skew == 0.0 {
        return x;
    }
    // A power curve: skew 100 is x^(1/4), -100 is x^4.
    let e = 2f64.powf(-(skew.clamp(-100.0, 100.0) as f64) / 50.0);
    x.powf(e)
}

impl Clip {
    pub fn end(&self) -> f64 {
        self.start + self.length
    }

    /// How many seconds of the source the clip covers (its timeline
    /// length over the applied stretch ratio).
    pub fn source_len(&self) -> f64 {
        self.length / self.props.stretch.applied_ratio()
    }

    /// The source position (seconds into the media) heard at timeline
    /// time `t` (seconds). Reversed clips run from the end of their span
    /// back to `offset`. Not clamped to the clip.
    pub fn timeline_to_source(&self, t: f64) -> f64 {
        let local = (t - self.start) / self.props.stretch.applied_ratio();
        if self.props.reversed {
            self.offset + self.source_len() - local
        } else {
            self.offset + local
        }
    }

    /// Clip gain at timeline time `t` (seconds), fades included. The
    /// polarity flip of an inverted clip is not part of it.
    pub fn gain_at(&self, t: f64) -> f32 {
        if self.muted {
            return 0.0;
        }
        let local = t - self.start;
        let mut g = 1.0f64;
        if self.fade_in > 0.0 && local < self.fade_in {
            g *= fade_curve(local / self.fade_in, self.props.fade_in_skew);
        }
        let to_end = self.length - local;
        if self.fade_out > 0.0 && to_end < self.fade_out {
            g *= fade_curve(to_end / self.fade_out, self.props.fade_out_skew);
        }
        (g as f32) * crate::time::db_to_gain(self.gain_db)
    }

    /// The clip's playing lane for `target`, if any.
    pub fn playing_lane(
        &self,
        target: &crate::automation::LaneTarget,
    ) -> Option<&crate::automation::Lane> {
        self.props
            .lanes
            .iter()
            .find(|l| &l.target == target && l.plays())
    }

    /// Moves every lane point by `d` seconds (a trim of the left edge by
    /// `-d` keeps the envelope where it was in the audio). Points that
    /// fall before the clip are kept, so trimming back brings them back.
    pub fn shift_lanes(&mut self, d: f64) {
        for l in &mut self.props.lanes {
            for p in &mut l.points {
                p.time += d;
            }
        }
    }

    /// Stretches every lane's times by `k` (the clip was stretched).
    pub fn scale_lanes(&mut self, k: f64) {
        if !(k.is_finite() && k > 0.0) {
            return;
        }
        for l in &mut self.props.lanes {
            for p in &mut l.points {
                p.time *= k;
            }
        }
    }

    /// Every lane cut to the clip's own span (`0..length`), with a point
    /// at each edge where the line crosses it, for files that cannot hold
    /// points outside the clip.
    pub fn cropped_lanes(&self) -> Vec<crate::automation::Lane> {
        self.props
            .lanes
            .iter()
            .map(|l| crate::automation::Lane {
                points: crate::automation::crop(&l.points, 0.0, self.length),
                ..l.clone()
            })
            .collect()
    }

    /// The clip's effect slots with new ids (and plug-ins with running
    /// copies of their own), its effect lanes following: for a copy of
    /// the clip.
    pub fn renew_chain_ids(&mut self) {
        for s in &mut self.props.chain {
            if crate::plugin::is_plugin(&s.effect) {
                let mut d = s.plugin_data();
                d.inst = crate::plugin::new_inst();
                s.set_plugin_data(&d);
            }
            let Some(old) = s.ext_id.take() else {
                continue;
            };
            let new = new_uuid();
            for l in &mut self.props.lanes {
                if let crate::automation::LaneTarget::Effect { slot, .. } = &mut l.target {
                    if *slot == old {
                        *slot = new.clone();
                    }
                }
            }
            s.ext_id = Some(new);
        }
    }
}

/// A fresh random (version 4) UUID, for ids other programs keep.
pub fn new_uuid() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = t
        ^ COUNTER.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed)
        ^ (std::process::id() as u64) << 32;
    let mut next = || {
        // splitmix64
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    let (a, b) = (next(), next());
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&a.to_le_bytes());
    bytes[8..].copy_from_slice(&b.to_le_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
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
    #[serde(default)]
    pub kind: TrackKind,
    /// Where the track's sound goes: None for the master, or a bus track.
    #[serde(default)]
    pub output: Option<TrackId>,
    #[serde(default)]
    pub sends: Vec<Send>,
    /// Effects run in order on the track's sound, before the fader.
    #[serde(default)]
    pub chain: Vec<EffectSlot>,
    /// The chain's power, input and output gain and dry/wet mix.
    #[serde(default, skip_serializing_if = "is_default")]
    pub rack: Rack,
    /// Where the track sits in a 3D Space session's room.
    #[serde(default)]
    pub pos: crate::space::SpacePos,
    /// 3D Space: the 3D Bus flag and spot automation.
    #[serde(default, skip_serializing_if = "crate::space::TrackSpace::is_default")]
    pub spatial: crate::space::TrackSpace,
    /// MIDI tracks: their clips.
    #[serde(default)]
    pub midi: Vec<crate::midi::MidiClip>,
    /// MIDI tracks: the built-in instrument's settings.
    #[serde(default)]
    pub instrument: crate::midi::Instrument,
    /// MIDI tracks: a SoundFont that plays the notes instead of the
    /// built-in synth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soundfont: Option<crate::midi::SoundFontRef>,
    /// Folds left and right into one centred signal after the chain,
    /// before the fader and pan.
    #[serde(default, skip_serializing_if = "is_default")]
    pub mono: bool,
    /// Flips the track's polarity (times -1) on every route.
    #[serde(default, skip_serializing_if = "is_default")]
    pub polarity: bool,
    /// Stays heard while other tracks are soloed (a shared reverb bus).
    #[serde(default, skip_serializing_if = "is_default")]
    pub solo_safe: bool,
    /// How the track's controls write into its lanes while playing.
    #[serde(default, skip_serializing_if = "is_default")]
    pub automation: crate::automation::AutomationMode,
    /// Automation lanes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lanes: Vec<crate::automation::Lane>,
    /// Lanes show under the track.
    #[serde(default, skip_serializing_if = "is_default")]
    pub show_lanes: bool,
    /// MIDI tracks: the MIDI input they listen to when armed. None is
    /// every input, `Some("none")` none, otherwise a port name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi_in: Option<String>,
    /// The object's id in an `.audfrm` file it came from, so saving back
    /// keeps everything Audiform27 does not use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext_id: Option<String>,
}

fn default_height() -> f32 {
    84.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TrackKind {
    #[default]
    Audio,
    /// Sums what other tracks send or output to it.
    Bus,
    /// Plays MIDI clips through an instrument.
    Midi,
}

/// A send from a track to a bus. A post-fader send follows the fader but
/// not the track's pan (it has its own); a pre-fader send ignores both.
/// Every send follows mute and solo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Send {
    pub bus: TrackId,
    pub level_db: f32,
    /// Taken before the track's fader (true) or after it.
    #[serde(default)]
    pub pre_fader: bool,
    /// The send's own pan, -1 (left) to 1 (right).
    #[serde(default, skip_serializing_if = "is_default")]
    pub pan: f32,
    /// Off: plays nowhere (still counts for routing order and loops).
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub enabled: bool,
    /// The object's id in an `.audfrm` file it came from, so saving back
    /// keeps everything Audiform27 does not use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext_id: Option<String>,
}

fn yes() -> bool {
    true
}

fn is_yes(b: &bool) -> bool {
    *b
}

/// Send levels are kept within this range (dB).
pub const SEND_MIN_DB: f32 = -96.0;
pub const SEND_MAX_DB: f32 = 12.0;

impl Send {
    /// A new send: post-fader, 0 dB, centred, on.
    pub fn new(bus: TrackId) -> Send {
        Send {
            bus,
            level_db: 0.0,
            pre_fader: false,
            pan: 0.0,
            enabled: true,
            ext_id: None,
        }
    }

    /// Clamps level and pan into range (NaN to the default).
    pub fn tidy(&mut self) {
        self.level_db = if self.level_db.is_nan() {
            0.0
        } else {
            self.level_db.clamp(SEND_MIN_DB, SEND_MAX_DB)
        };
        self.pan = if self.pan.is_nan() {
            0.0
        } else {
            self.pan.clamp(-1.0, 1.0)
        };
    }
}

/// One slot of an effect chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectSlot {
    /// The effect's stable id (see `af27_dsp::Effect::id`).
    pub effect: String,
    pub values: Vec<f32>,
    #[serde(default)]
    pub bypass: bool,
    /// The rack row it sits in (0..16), so a rack can have gaps. None:
    /// right after the slot before it. The chain runs in row order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u8>,
    /// The track or bus whose sound keys this effect (a compressor
    /// ducking under a voice). None: it listens to its own input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidechain: Option<TrackId>,
    /// Anything else the effect keeps (an amp capture, an impulse
    /// response file), as the effect's own JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// The object's id in an `.audfrm` file it came from, so saving back
    /// keeps everything Audiform27 does not use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext_id: Option<String>,
}

/// An effect chain's own controls.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rack {
    /// The whole chain off.
    pub bypass: bool,
    pub input_db: f32,
    pub output_db: f32,
    /// Dry/wet, 0..1.
    pub mix: f32,
}

impl Default for Rack {
    fn default() -> Self {
        Rack {
            bypass: false,
            input_db: 0.0,
            output_db: 0.0,
            mix: 1.0,
        }
    }
}

/// Up to this many effects per chain.
pub const CHAIN_SLOTS: usize = 16;

/// Pan as a balance (-1 left .. 1 right): centred plays both sides at
/// full level and panning turns the far side down, so a centred voice
/// plays at exactly the level its fader shows.
pub fn balance(pan: f32) -> [f32; 2] {
    let p = if pan.is_nan() {
        0.0
    } else {
        pan.clamp(-1.0, 1.0)
    };
    [
        if p > 0.0 { 1.0 - p } else { 1.0 },
        if p < 0.0 { 1.0 + p } else { 1.0 },
    ]
}

/// A fader level in dB as a gain, clamped to -96..24 dB.
pub fn fader_gain(db: f32) -> f32 {
    crate::time::db_to_gain(db.clamp(-96.0, 24.0))
}

impl Track {
    /// Fader and balance-pan gains (left, right).
    pub fn gains(&self) -> (f32, f32) {
        let v = fader_gain(self.volume_db);
        let [l, r] = balance(self.pan);
        (v * l, v * r)
    }

    /// The playing lane for `target`, if any.
    pub fn playing_lane(
        &self,
        target: &crate::automation::LaneTarget,
    ) -> Option<&crate::automation::Lane> {
        self.lanes.iter().find(|l| &l.target == target && l.plays())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub name: String,
    pub time: f64,
    #[serde(default)]
    pub length: f64,
    /// The object's id in an `.audfrm` file it came from, so saving back
    /// keeps everything Audiform27 does not use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub schema_version: u32,
    pub name: String,
    pub rate: u32,
    #[serde(default)]
    pub master_db: f32,
    /// The master's balance, -1 (left) ..= 1 (right).
    #[serde(default, skip_serializing_if = "is_default")]
    pub master_pan: f32,
    /// The master summed to mono.
    #[serde(default, skip_serializing_if = "is_default")]
    pub master_mono: bool,
    /// The master's polarity flipped.
    #[serde(default, skip_serializing_if = "is_default")]
    pub master_invert: bool,
    pub media: Vec<MediaItem>,
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub markers: Vec<Marker>,
    #[serde(default)]
    pub next_id: u64,
    /// Effects on the whole mix.
    #[serde(default)]
    pub master_chain: Vec<EffectSlot>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub master_rack: Rack,
    /// The master's automation lanes (its volume lane plays).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub master_lanes: Vec<crate::automation::Lane>,
    /// Some(layout) makes this a 3D Space session.
    #[serde(default)]
    pub space: Option<crate::space::SpaceLayout>,
    #[serde(default)]
    pub monitor: crate::space::Monitor,
    #[serde(default)]
    pub tempo: crate::midi::Tempo,
    /// Where the playhead was when playback last stopped (seconds).
    #[serde(default)]
    pub playhead: f64,
    /// Bits per sample new audio in the session is kept at (16, 24 or 32
    /// float). Recordings are written as FLAC at 24 bits for 32.
    #[serde(default = "default_bit_depth")]
    pub bit_depth: u16,
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
            master_pan: 0.0,
            master_mono: false,
            master_invert: false,
            media: Vec::new(),
            tracks: Vec::new(),
            markers: Vec::new(),
            next_id: 1,
            master_chain: Vec::new(),
            master_rack: Default::default(),
            master_lanes: Vec::new(),
            space: None,
            monitor: Default::default(),
            tempo: Default::default(),
            playhead: 0.0,
            bit_depth: 32,
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
        self.add_track_of(TrackKind::Audio)
    }

    pub fn add_track_of(&mut self, kind: TrackKind) -> TrackId {
        let id = self.alloc_id();
        let n = self.tracks.len();
        let count = self.tracks.iter().filter(|t| t.kind == kind).count() + 1;
        let name = match kind {
            TrackKind::Audio => format!("Track {}", n + 1),
            TrackKind::Bus => format!("Bus {count}"),
            TrackKind::Midi => format!("MIDI {count}"),
        };
        self.tracks.push(Track {
            id,
            name,
            volume_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            armed: false,
            color: PALETTE[n % PALETTE.len()],
            clips: Vec::new(),
            height: default_height(),
            kind,
            output: None,
            sends: Vec::new(),
            chain: Vec::new(),
            rack: Default::default(),
            pos: Default::default(),
            spatial: Default::default(),
            midi: Vec::new(),
            instrument: Default::default(),
            soundfont: None,
            mono: false,
            polarity: false,
            solo_safe: false,
            automation: Default::default(),
            lanes: Vec::new(),
            show_lanes: false,
            midi_in: None,
            ext_id: None,
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
            props: Default::default(),
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
            ext_id: None,
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
        let right_len = c.length - left_len;
        let ratio = c.props.stretch.applied_ratio();
        // A reversed clip plays its span backwards, so its right part
        // comes from the earlier source and the left part from the later.
        let (left_off, right_off) = if c.props.reversed {
            (c.offset + right_len / ratio, c.offset)
        } else {
            (c.offset, c.offset + left_len / ratio)
        };
        let mut right = Clip {
            id: new_id,
            start: t,
            offset: right_off,
            length: right_len,
            fade_in: 0.0,
            ext_id: None,
            ..c.clone()
        };
        // Each part keeps its share of the clip's lanes (a point where
        // the cut crosses them) and a copy of its effects.
        for l in &mut right.props.lanes {
            l.points = crate::automation::crop(&l.points, left_len, c.length);
            l.ext_id = None;
        }
        right.renew_chain_ids();
        let left = &mut self.tracks[ti].clips[ci];
        left.offset = left_off;
        left.length = left_len;
        left.fade_out = 0.0;
        for l in &mut left.props.lanes {
            l.points = crate::automation::crop(&l.points, 0.0, left_len);
        }
        self.tracks[ti].clips.insert(ci + 1, right);
        Some(new_id)
    }

    pub fn remove_clip(&mut self, id: ClipId) -> Option<Clip> {
        let (ti, ci) = self.find_clip(id)?;
        Some(self.tracks[ti].clips.remove(ci))
    }

    /// End of the last clip (audio or MIDI), in seconds.
    pub fn duration(&self) -> f64 {
        let audio = self
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .map(Clip::end)
            .fold(0.0, f64::max);
        let midi = self
            .tracks
            .iter()
            .flat_map(|t| t.midi.iter())
            .map(|c| c.end_secs(&self.tempo))
            .fold(0.0, f64::max);
        audio.max(midi)
    }

    /// Finds a MIDI clip by id: (track index, clip index).
    pub fn find_midi(&self, id: ClipId) -> Option<(usize, usize)> {
        self.tracks
            .iter()
            .enumerate()
            .find_map(|(ti, t)| t.midi.iter().position(|c| c.id == id).map(|ci| (ti, ci)))
    }

    pub fn track_index(&self, id: TrackId) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    pub fn any_solo(&self) -> bool {
        self.tracks.iter().any(|t| t.solo)
    }

    /// Whether a track is heard, given mute and solo states (see
    /// [`Session::heard`]).
    pub fn audible(&self, t: &Track) -> bool {
        match self.track_index(t.id) {
            Some(i) => self.heard()[i],
            None => !t.mute,
        }
    }

    /// Which tracks are heard, by index. With nothing soloed, everything
    /// not muted. With any solo, everything reached from a soloed track or
    /// bus through outputs and sends that are on, forward (where it goes)
    /// and backward (what feeds it), plus solo-safe tracks. Mute always
    /// wins. Sidechain keys are not routes.
    pub fn heard(&self) -> Vec<bool> {
        let n = self.tracks.len();
        if !self.any_solo() {
            return self.tracks.iter().map(|t| !t.mute).collect();
        }
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for (a, t) in self.tracks.iter().enumerate() {
            let to = t
                .output
                .into_iter()
                .chain(t.sends.iter().filter(|s| s.enabled).map(|s| s.bus));
            for b in to.filter_map(|id| self.track_index(id)) {
                if b != a {
                    edges.push((a, b));
                }
            }
        }
        let mut reached = vec![false; n];
        for forward in [true, false] {
            let mut seen: Vec<bool> = self.tracks.iter().map(|t| t.solo).collect();
            let mut stack: Vec<usize> = (0..n).filter(|&i| seen[i]).collect();
            while let Some(x) = stack.pop() {
                reached[x] = true;
                for &(a, b) in &edges {
                    let next = match (forward, a == x, b == x) {
                        (true, true, _) => b,
                        (false, _, true) => a,
                        _ => continue,
                    };
                    if !seen[next] {
                        seen[next] = true;
                        stack.push(next);
                    }
                }
            }
        }
        self.tracks
            .iter()
            .zip(reached)
            .map(|(t, r)| !t.mute && (r || t.solo_safe))
            .collect()
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
            ext_id: None,
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
    fn split_follows_stretch_and_reverse() {
        let (mut s, m) = with_media();
        let t = s.tracks[0].id;
        let c = s.add_clip(t, m, 0.0).unwrap();
        {
            let cl = &mut s.tracks[0].clips[0];
            cl.offset = 1.0;
            cl.length = 8.0; // 4 s of source at ratio 2
            cl.props.stretch.mode = StretchMode::Live;
            cl.props.stretch.ratio = 2.0;
            cl.ext_id = Some("x".into());
        }
        assert_eq!(s.tracks[0].clips[0].source_len(), 4.0);
        assert_eq!(s.tracks[0].clips[0].timeline_to_source(2.0), 2.0);
        let r = s.split_clip(c, 2.0).unwrap();
        let (l, rt) = (&s.tracks[0].clips[0], &s.tracks[0].clips[1]);
        assert_eq!((l.offset, l.length), (1.0, 2.0));
        assert_eq!((rt.offset, rt.length, rt.id), (2.0, 6.0, r));
        assert!(rt.ext_id.is_none());
        // Reversed: the right part is the earlier source.
        let cl = &mut s.tracks[0].clips[1];
        cl.props.reversed = true;
        assert_eq!(cl.timeline_to_source(2.0), 5.0);
        assert_eq!(cl.timeline_to_source(8.0), 2.0);
        s.split_clip(r, 4.0).unwrap();
        let (l, rt) = (&s.tracks[0].clips[1], &s.tracks[0].clips[2]);
        assert_eq!((l.offset, l.length), (4.0, 2.0));
        assert_eq!((rt.offset, rt.length), (2.0, 4.0));
        assert_eq!(l.timeline_to_source(2.0), 5.0);
        assert_eq!(rt.timeline_to_source(8.0), 2.0);
    }

    #[test]
    fn split_cuts_clip_lanes_and_copies_the_rack() {
        use crate::automation::{Curve, Lane, LanePoint, LaneTarget};
        let (mut s, m) = with_media();
        let t = s.tracks[0].id;
        let c = s.add_clip(t, m, 2.0).unwrap();
        {
            let cl = &mut s.tracks[0].clips[0];
            let mut vol = Lane::new(LaneTarget::Volume);
            let pt = |time, value| LanePoint {
                time,
                value,
                curve: Curve::Linear,
            };
            vol.points = vec![pt(0.0, 0.0), pt(4.0, -20.0), pt(8.0, -20.0)];
            vol.ext_id = Some("lane".into());
            cl.props.lanes = vec![vol];
            cl.props.chain = vec![EffectSlot {
                effect: "gain".into(),
                values: vec![1.0],
                bypass: false,
                position: Some(2),
                sidechain: None,
                data: None,
                ext_id: Some("slot".into()),
            }];
            let mut fx = Lane::new(LaneTarget::Effect {
                slot: "slot".into(),
                param: 0,
                key: None,
            });
            fx.points = vec![pt(1.0, 0.5)];
            cl.props.lanes.push(fx);
            cl.props.rack.mix = 0.5;
            cl.props.channel_map = Some(usual_channel_map(1));
        }
        // At 2 s into the clip: the line is at -10 dB there.
        let r = s.split_clip(c, 4.0).unwrap();
        let (l, rt) = (&s.tracks[0].clips[0], &s.tracks[0].clips[1]);
        assert_eq!(rt.id, r);
        let lv = &l.props.lanes[0].points;
        assert_eq!(lv.len(), 2);
        assert_eq!((lv[1].time, lv[1].value), (2.0, -10.0));
        let rv = &rt.props.lanes[0].points;
        assert_eq!((rv[0].time, rv[0].value), (0.0, -10.0));
        assert_eq!((rv[1].time, rv[1].value), (2.0, -20.0));
        assert_eq!(rv.last().unwrap().time, 8.0);
        assert_eq!(
            rt.playing_lane(&LaneTarget::Volume).unwrap().value_at(1.0),
            Some(-15.0)
        );
        // Both keep the rack; the copy's slot has an id of its own that
        // its effect lane follows.
        assert_eq!(l.props.chain.len(), 1);
        assert_eq!(rt.props.chain.len(), 1);
        assert_eq!(rt.props.rack.mix, 0.5);
        assert_eq!(l.props.chain[0].ext_id.as_deref(), Some("slot"));
        let new_slot = rt.props.chain[0].ext_id.clone().unwrap();
        assert_ne!(new_slot, "slot");
        assert!(
            matches!(&rt.props.lanes[1].target, LaneTarget::Effect { slot, .. } if *slot == new_slot)
        );
        assert_eq!(rt.props.channel_map, l.props.channel_map);
        assert!(rt.props.lanes[0].ext_id.is_none());
        // Trimming the left edge keeps the line where it was in the audio.
        assert!(s.trim_start_to(r, 5.0));
        let rt = &s.tracks[0].clips[1];
        assert_eq!(
            rt.playing_lane(&LaneTarget::Volume).unwrap().value_at(0.0),
            Some(-15.0)
        );
        let json = Session::from_json(&s.to_json()).unwrap();
        assert_eq!(json, s);
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
            ext_id: None,
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
            props: Default::default(),
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
            ext_id: None,
        };
        assert!((c.gain_at(0.5) - 0.5).abs() < 1e-6);
        assert!((c.gain_at(2.0) - 1.0).abs() < 1e-6);
        assert!((c.gain_at(3.5) - 0.5).abs() < 1e-6);
        // Skew 0 is the straight line exactly; skew bows it up or down.
        for x in [0.0, 0.1, 0.37, 0.5, 0.99, 1.0] {
            assert_eq!(fade_curve(x, 0.0), x);
            assert!(fade_curve(x, 60.0) >= x && fade_curve(x, -60.0) <= x);
        }
        assert_eq!(fade_curve(1.0, 100.0), 1.0);
        assert_eq!(fade_curve(0.0, -100.0), 0.0);
        let mut up = c.clone();
        up.props.fade_in_skew = 50.0;
        up.props.fade_out_skew = -50.0;
        assert!(up.gain_at(0.5) > 0.5 && up.gain_at(3.5) < 0.5);
        let (s, _) = with_media();
        let (l, r) = s.tracks[0].gains();
        assert!((l - 1.0).abs() < 1e-5 && (r - 1.0).abs() < 1e-5);
    }
}
