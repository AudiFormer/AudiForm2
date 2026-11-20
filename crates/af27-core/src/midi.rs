// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! MIDI data: the session's tempo map, MIDI clips and their notes (in
//! beats, so they follow tempo changes), and the instruments' settings.
//!
//! The tempo map: a starting tempo and time signature, plus changes that
//! each take over at a bar line (no ramps). Beats are always quarter
//! notes, so a bar of 6/8 is three of them. Each stretch of the map starts
//! where the previous one's bars end, so a change never leaves a later one
//! partway through a bar. This is the model Audiform's sessions carry
//! (`tempo.changes[{bar, bpm, beats_per_bar, beat_unit}]`).

use serde::{Deserialize, Serialize};

use crate::ClipId;

/// Ticks to a beat in bar.beat.tick and in exported `.mid` files.
pub const TICKS_PER_BEAT: u32 = 960;
/// Lowest and highest tempos.
pub const BPM_MIN: f64 = 20.0;
pub const BPM_MAX: f64 = 400.0;
/// Bottoms a time signature can have.
pub const BEAT_UNITS: [u32; 6] = [1, 2, 4, 8, 16, 32];

fn quarter() -> u32 {
    4
}

fn clamp_bpm(b: f64) -> f64 {
    if b.is_finite() {
        b.clamp(BPM_MIN, BPM_MAX)
    } else {
        120.0
    }
}

fn clamp_unit(u: u32) -> u32 {
    if BEAT_UNITS.contains(&u) {
        u
    } else {
        4
    }
}

/// A tempo and time signature change at the start of a bar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TempoChange {
    /// Bars from the session start where it takes over (1 is the second
    /// bar). Always 1 or more.
    pub bar: u32,
    /// Quarter notes per minute.
    pub bpm: f64,
    pub beats_per_bar: u32,
    #[serde(default = "quarter")]
    pub beat_unit: u32,
    /// The change's id in an `.audfrm` file it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tempo {
    /// Quarter notes per minute at the start.
    pub bpm: f64,
    /// The top of the starting time signature.
    pub beats_per_bar: u32,
    /// The bottom of the starting time signature (4 in 4/4, 8 in 6/8).
    pub beat_unit: u32,
    /// Click on every beat while playing or recording.
    pub metronome: bool,
    /// Changes at later bars, in bar order.
    pub changes: Vec<TempoChange>,
}

impl Default for Tempo {
    fn default() -> Self {
        Tempo {
            bpm: 120.0,
            beats_per_bar: 4,
            beat_unit: 4,
            metronome: false,
            changes: Vec::new(),
        }
    }
}

/// The metronome's sound. Kept in the app's settings, not the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ClickSound {
    #[default]
    Tick,
    Wood,
    Beep,
    Shaker,
}

/// Clicks between the beats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Subdivide {
    #[default]
    None,
    Halves,
    Triplets,
    Quarters,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetronomeSettings {
    pub muted: bool,
    /// Click only while recording.
    pub recording_only: bool,
    pub level_db: f32,
    pub sound: ClickSound,
    /// A higher click on the first beat of each bar.
    pub accent: bool,
    pub subdivide: Subdivide,
}

impl Default for MetronomeSettings {
    fn default() -> Self {
        MetronomeSettings {
            muted: false,
            recording_only: false,
            level_db: -6.0,
            sound: ClickSound::Tick,
            accent: true,
            subdivide: Subdivide::None,
        }
    }
}

/// One stretch of the tempo map with a single tempo and time signature.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    /// Bars from the session start where it begins.
    pub bar: u32,
    /// Quarter notes from the session start where it begins.
    pub beat: f64,
    /// Seconds from the session start where it begins.
    pub secs: f64,
    pub bpm: f64,
    pub beats_per_bar: u32,
    pub beat_unit: u32,
    /// Index into `Tempo::changes`, None for the start.
    pub change: Option<usize>,
}

impl Segment {
    /// Quarter notes in one bar (3 in 6/8).
    pub fn bar_beats(&self) -> f64 {
        self.beats_per_bar.max(1) as f64 * 4.0 / self.beat_unit.max(1) as f64
    }
    /// Quarter notes in one beat of its time signature (0.5 in 6/8).
    pub fn unit_beats(&self) -> f64 {
        4.0 / self.beat_unit.max(1) as f64
    }
    /// Seconds per quarter note.
    pub fn spq(&self) -> f64 {
        60.0 / self.bpm
    }
    /// Quarter notes to seconds, inside or beyond this stretch.
    pub fn secs_at(&self, beats: f64) -> f64 {
        self.secs + (beats - self.beat) * self.spq()
    }
    pub fn beats_at(&self, secs: f64) -> f64 {
        self.beat + (secs - self.secs) / self.spq()
    }
    /// "4/4", "6/8".
    pub fn meter_text(&self) -> String {
        format!("{}/{}", self.beats_per_bar, self.beat_unit)
    }
}

/// Walks the map's stretches in order without allocating.
pub struct Segments<'a> {
    tempo: &'a Tempo,
    next: usize,
    prev: Option<Segment>,
}

impl Iterator for Segments<'_> {
    type Item = Segment;
    fn next(&mut self) -> Option<Segment> {
        let Some(p) = self.prev else {
            let t = self.tempo;
            let s = Segment {
                bar: 0,
                beat: 0.0,
                secs: 0.0,
                bpm: clamp_bpm(t.bpm),
                beats_per_bar: t.beats_per_bar.max(1),
                beat_unit: clamp_unit(t.beat_unit),
                change: None,
            };
            self.prev = Some(s);
            return Some(s);
        };
        while let Some(c) = self.tempo.changes.get(self.next) {
            let i = self.next;
            self.next += 1;
            // Out of order or at the start: ignored, as Audiform does.
            if c.bar <= p.bar {
                continue;
            }
            let beat = p.beat + (c.bar - p.bar) as f64 * p.bar_beats();
            let s = Segment {
                bar: c.bar,
                beat,
                secs: p.secs_at(beat),
                bpm: clamp_bpm(c.bpm),
                beats_per_bar: c.beats_per_bar.max(1),
                beat_unit: clamp_unit(c.beat_unit),
                change: Some(i),
            };
            self.prev = Some(s);
            return Some(s);
        }
        None
    }
}

impl Tempo {
    /// A plain map: one tempo, one time signature.
    pub fn constant(bpm: f64, beats_per_bar: u32, beat_unit: u32) -> Tempo {
        Tempo {
            bpm,
            beats_per_bar,
            beat_unit,
            ..Tempo::default()
        }
    }

    pub fn segments(&self) -> Segments<'_> {
        Segments {
            tempo: self,
            next: 0,
            prev: None,
        }
    }

    fn last_where(&self, ok: impl Fn(&Segment) -> bool) -> Segment {
        let mut it = self.segments();
        let mut seg = it.next().expect("a map has a start");
        for s in it {
            if !ok(&s) {
                break;
            }
            seg = s;
        }
        seg
    }

    pub fn seg_at_beat(&self, beats: f64) -> Segment {
        self.last_where(|s| s.beat <= beats)
    }
    pub fn seg_at_secs(&self, secs: f64) -> Segment {
        self.last_where(|s| s.secs <= secs)
    }
    pub fn seg_at_bar(&self, bar: u32) -> Segment {
        self.last_where(|s| s.bar <= bar)
    }

    /// Quarter notes from the session start to seconds. Before 0 the
    /// starting tempo carries on backwards.
    pub fn secs(&self, beats: f64) -> f64 {
        self.seg_at_beat(beats).secs_at(beats)
    }

    /// Seconds to quarter notes.
    pub fn beats(&self, secs: f64) -> f64 {
        self.seg_at_secs(secs).beats_at(secs)
    }

    /// Where a bar (from 0) starts, in quarter notes.
    pub fn bar_start(&self, bar: u32) -> f64 {
        let s = self.seg_at_bar(bar);
        s.beat + (bar - s.bar) as f64 * s.bar_beats()
    }

    /// The bar (from 0) holding a time in quarter notes, how far into it
    /// in beats of its time signature, and its stretch.
    pub fn bar_at(&self, beats: f64) -> (u32, f64, Segment) {
        let seg = self.seg_at_beat(beats);
        let into = (beats - seg.beat).max(0.0);
        let per = seg.bar_beats();
        // A hair of slack so a bar line computed a little short counts.
        let bars = (into / per + 1e-9).floor();
        let beat = ((into - bars * per) / seg.unit_beats()).max(0.0);
        (seg.bar + bars as u32, beat, seg)
    }

    /// The nearest bar line to a time in quarter notes, as a bar from 0.
    pub fn nearest_bar(&self, beats: f64) -> u32 {
        let (bar, _, _) = self.bar_at(beats);
        let a = self.bar_start(bar);
        let b = self.bar_start(bar + 1);
        if beats - a <= b - beats {
            bar
        } else {
            bar + 1
        }
    }

    /// The bar line at or before a time in quarter notes, in quarter notes.
    pub fn bar_floor(&self, beats: f64) -> f64 {
        self.bar_start(self.bar_at(beats).0)
    }

    /// "bar.beat.tick" (960 ticks per beat of the time signature), bars
    /// and beats from 1.
    pub fn format(&self, secs: f64) -> String {
        let (bar, beat, _) = self.bar_at(self.beats(secs.max(0.0)));
        let whole = (beat + 1e-9).floor();
        let tick = ((beat - whole).max(0.0) * TICKS_PER_BEAT as f64).floor() as u32;
        format!(
            "{}.{}.{:03}",
            bar + 1,
            whole as u64 + 1,
            tick.min(TICKS_PER_BEAT - 1)
        )
    }

    /// Puts the map in its tidy form: values in range, changes in bar
    /// order, one per bar (the later one wins), none at bar 0.
    pub fn normalize(&mut self) {
        self.bpm = clamp_bpm(self.bpm);
        self.beats_per_bar = self.beats_per_bar.clamp(1, 32);
        self.beat_unit = clamp_unit(self.beat_unit);
        for c in &mut self.changes {
            c.bpm = clamp_bpm(c.bpm);
            c.beats_per_bar = c.beats_per_bar.clamp(1, 32);
            c.beat_unit = clamp_unit(c.beat_unit);
        }
        self.changes.retain(|c| c.bar >= 1);
        // Stable: of two at one bar the later stays.
        self.changes.sort_by_key(|c| c.bar);
        let mut out: Vec<TempoChange> = Vec::with_capacity(self.changes.len());
        for c in self.changes.drain(..) {
            if out.last().is_some_and(|l| l.bar == c.bar) {
                out.pop();
            }
            out.push(c);
        }
        self.changes = out;
    }

    /// Adds a change at `bar` (from 1) with the values in effect there, or
    /// finds the one already there. Returns its index.
    pub fn add_change(&mut self, bar: u32) -> usize {
        let bar = bar.max(1);
        if let Some(i) = self.changes.iter().position(|c| c.bar == bar) {
            return i;
        }
        let s = self.seg_at_bar(bar);
        self.changes.push(TempoChange {
            bar,
            bpm: s.bpm,
            beats_per_bar: s.beats_per_bar,
            beat_unit: s.beat_unit,
            ext_id: None,
        });
        self.normalize();
        self.changes.iter().position(|c| c.bar == bar).unwrap_or(0)
    }

    /// The tempo, top and bottom of a stretch (the start for None).
    pub fn set_values(&mut self, change: Option<usize>, bpm: f64, top: u32, unit: u32) {
        match change.and_then(|i| self.changes.get_mut(i)) {
            Some(c) => {
                c.bpm = bpm;
                c.beats_per_bar = top;
                c.beat_unit = unit;
            }
            None => {
                self.bpm = bpm;
                self.beats_per_bar = top;
                self.beat_unit = unit;
            }
        }
        self.normalize();
    }

    /// Whether the map has more than one stretch.
    pub fn has_changes(&self) -> bool {
        self.segments().nth(1).is_some()
    }
}

/// Reads "6/8" or "3" (over 4).
pub fn parse_meter(text: &str) -> Option<(u32, u32)> {
    let mut it = text.split('/');
    let top: u32 = it.next()?.trim().parse().ok()?;
    let bottom: u32 = match it.next() {
        Some(b) => b.trim().parse().ok()?,
        None => 4,
    };
    if it.next().is_some() || !(1..=32).contains(&top) || !BEAT_UNITS.contains(&bottom) {
        return None;
    }
    Some((top, bottom))
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MidiNote {
    /// 0..127, 60 is middle C.
    pub pitch: u8,
    /// Beats from the clip's start.
    pub start: f64,
    /// Beats.
    pub length: f64,
    /// 1..127.
    pub velocity: u8,
    /// 0..15, shown as the note's colour.
    #[serde(default)]
    pub channel: u8,
    /// -1 (left) to 1 (right).
    #[serde(default)]
    pub pan: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiClip {
    pub id: ClipId,
    pub name: String,
    /// Beats from the session start.
    pub start: f64,
    /// Beats.
    pub length: f64,
    pub notes: Vec<MidiNote>,
    #[serde(default)]
    pub muted: bool,
    /// Clip level in dB: scales every note's velocity (-96..24).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gain_db: f32,
    /// Controller moves (sustain pedal, mod wheel, pitch bend...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<ControlEvent>,
    /// The object's id in an `.audfrm` file it came from, so saving back
    /// keeps everything Audiform27 does not use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext_id: Option<String>,
}

fn is_zero(x: &f32) -> bool {
    *x == 0.0
}

/// Controller number for channel aftertouch in [`ControlEvent`].
pub const AFTERTOUCH: u8 = 128;
/// Controller number for pitch bend in [`ControlEvent`].
pub const PITCH_BEND: u8 = 129;
/// The sustain pedal.
pub const SUSTAIN: u8 = 64;

/// One controller move in a MIDI clip.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ControlEvent {
    /// Beats from the clip's start.
    pub at: f64,
    /// 0..127 as in MIDI, 128 channel aftertouch, 129 pitch bend.
    pub controller: u8,
    /// 0..1 (pitch bend: 0.5 is the middle).
    pub value: f32,
    /// 0..15.
    #[serde(default)]
    pub channel: u8,
}

impl ControlEvent {
    /// Pedals (64..=69) are only up or down.
    pub fn is_switch(controller: u8) -> bool {
        (64..=69).contains(&controller)
    }

    /// The controller's name, for lane menus.
    pub fn name(controller: u8) -> String {
        match controller {
            1 => "Mod Wheel".into(),
            2 => "Breath".into(),
            7 => "Volume".into(),
            10 => "Pan".into(),
            11 => "Expression".into(),
            64 => "Sustain Pedal".into(),
            66 => "Sostenuto".into(),
            67 => "Soft Pedal".into(),
            AFTERTOUCH => "Aftertouch".into(),
            PITCH_BEND => "Pitch Bend".into(),
            n => format!("CC {n}"),
        }
    }

    /// The value as shown: pedals up or down, pitch bend -8192..8191,
    /// others 0..127.
    pub fn value_text(controller: u8, value: f32) -> String {
        let v = value.clamp(0.0, 1.0);
        if Self::is_switch(controller) {
            if v >= 0.5 { "down" } else { "up" }.into()
        } else if controller == PITCH_BEND {
            format!("{}", (v * 16383.0).round() as i32 - 8192)
        } else {
            format!("{}", (v * 127.0).round() as i32)
        }
    }
}

/// The controllers controller lanes offer first.
pub const LANE_CONTROLLERS: [u8; 10] = [64, 1, 11, 7, 10, 2, 66, 67, PITCH_BEND, AFTERTOUCH];

impl MidiClip {
    pub fn end(&self) -> f64 {
        self.start + self.length
    }

    /// The clip's level as a velocity scale.
    pub fn level(&self) -> f32 {
        crate::time::db_to_gain(self.gain_db.clamp(-96.0, 24.0))
    }

    /// A note's velocity after the clip level, 1/127..1.
    pub fn velocity(&self, n: &MidiNote) -> f32 {
        (n.velocity.clamp(1, 127) as f32 / 127.0 * self.level()).clamp(1.0 / 127.0, 1.0)
    }
}

/// Oscillator shapes of the built-in synth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Wave {
    Sine,
    Triangle,
    #[default]
    Saw,
    Square,
}

/// The built-in synth: one oscillator plus a detuned second, an ADSR
/// envelope and a resonant low-pass filter.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Instrument {
    pub wave: Wave,
    /// Second oscillator detune in cents (0 = off).
    pub detune: f32,
    pub attack: f32,
    pub decay: f32,
    /// 0..1.
    pub sustain: f32,
    pub release: f32,
    /// Filter cutoff in Hz.
    pub cutoff: f32,
    /// 0..1.
    pub resonance: f32,
    pub gain_db: f32,
}

impl Default for Instrument {
    fn default() -> Self {
        Instrument {
            wave: Wave::Saw,
            detune: 7.0,
            attack: 0.005,
            decay: 0.25,
            sustain: 0.7,
            release: 0.25,
            cutoff: 6000.0,
            resonance: 0.15,
            gain_db: -10.0,
        }
    }
}

/// A SoundFont (.sf2) instrument for a MIDI track: the file and the
/// preset its notes play.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SoundFontRef {
    pub path: std::path::PathBuf,
    /// 0..16383; 128 is the drum kits' bank in General MIDI fonts.
    #[serde(default)]
    pub bank: u16,
    /// 0..127.
    #[serde(default)]
    pub program: u8,
    /// The preset's name, for showing when the file is missing.
    #[serde(default)]
    pub preset_name: String,
    #[serde(default)]
    pub gain_db: f32,
}

pub fn note_name(pitch: u8) -> String {
    const N: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", N[pitch as usize % 12], pitch as i32 / 12 - 1)
}

pub fn note_freq(pitch: f32) -> f32 {
    440.0 * 2f32.powf((pitch - 69.0) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(bar: u32, bpm: f64, top: u32, unit: u32) -> TempoChange {
        TempoChange {
            bar,
            bpm,
            beats_per_bar: top,
            beat_unit: unit,
            ext_id: None,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn tempo_and_names() {
        let t = Tempo::default();
        assert_eq!(t.secs(4.0), 2.0);
        assert_eq!(t.format(2.0), "2.1.000");
        assert_eq!(t.format(0.75), "1.2.480");
        assert_eq!(note_name(60), "C4");
        assert!((note_freq(69.0) - 440.0).abs() < 1e-3);
    }

    #[test]
    fn seconds_and_beats_across_changes() {
        // Two bars of 4/4 at 120 (4 s), then 60 BPM.
        let mut t = Tempo::default();
        t.changes.push(change(2, 60.0, 4, 4));
        assert!(close(t.secs(8.0), 4.0));
        assert!(close(t.secs(9.0), 5.0));
        assert!(close(t.secs(4.0), 2.0));
        assert!(close(t.beats(5.0), 9.0));
        assert!(close(t.beats(1.0), 2.0));
        assert_eq!(t.format(4.0), "3.1.000");
        assert_eq!(t.format(5.5), "3.2.480");
        // Round trip everywhere.
        for i in 0..200 {
            let b = i as f64 * 0.37;
            assert!(close(t.beats(t.secs(b)), b), "at {b}");
        }
    }

    #[test]
    fn meter_changes_move_bar_lines() {
        // One bar of 4/4 at 120, then 6/8 at 60: a 6/8 bar is 3 quarter
        // notes, its beats are eighths.
        let mut t = Tempo::default();
        t.changes.push(change(1, 60.0, 6, 8));
        assert!(close(t.bar_start(1), 4.0));
        assert!(close(t.bar_start(2), 7.0));
        assert!(close(t.bar_start(3), 10.0));
        assert!(close(t.secs(7.0), 2.0 + 3.0));
        let (bar, beat, seg) = t.bar_at(7.5);
        assert_eq!((bar, seg.beat_unit), (2, 8));
        assert!(close(beat, 1.0));
        assert_eq!(t.format(t.secs(7.5)), "3.2.000");
        assert_eq!(t.nearest_bar(5.4), 1);
        assert_eq!(t.nearest_bar(5.6), 2);
        // A later change counts its bars in the meter before it.
        t.changes.push(change(3, 90.0, 3, 4));
        assert!(close(t.bar_start(3), 10.0));
        assert!(close(t.bar_start(4), 13.0));
    }

    #[test]
    fn out_of_order_changes_are_skipped_and_normalized() {
        let mut t = Tempo::default();
        t.changes.push(change(4, 90.0, 4, 4));
        t.changes.push(change(2, 100.0, 3, 4));
        // Read as Audiform does: the second is behind the first, ignored.
        assert_eq!(t.segments().count(), 2);
        t.normalize();
        assert_eq!(t.changes[0].bar, 2);
        assert_eq!(t.segments().count(), 3);
        let i = t.add_change(3);
        assert_eq!(t.changes[i].bpm, 100.0);
        assert_eq!(t.changes[i].beats_per_bar, 3);
        assert_eq!(t.add_change(3), i);
        assert_eq!(parse_meter("6/8"), Some((6, 8)));
        assert_eq!(parse_meter("3"), Some((3, 4)));
        assert_eq!(parse_meter("5/6"), None);
    }

    #[test]
    fn old_files_read_with_defaults() {
        let t: Tempo = serde_json::from_str(r#"{"bpm": 90, "beats_per_bar": 3}"#).unwrap();
        assert_eq!(t.beat_unit, 4);
        assert!(t.changes.is_empty());
    }
}
