// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! MIDI data: the session's tempo map, MIDI clips and their notes (in
//! beats, so they follow tempo changes), and the instruments' settings.
//!
//! The tempo map: a starting tempo and time signature, plus changes that
//! each take over at a bar line. Beats are always quarter notes, so a bar
//! of 6/8 is three of them. Each stretch of the map starts where the
//! previous one's bars end, so a change never leaves a later one partway
//! through a bar. This is the model Audiform's sessions carry
//! (`tempo.changes[{bar, bpm, beats_per_bar, beat_unit}]`).
//!
//! A change is either immediate (the new tempo starts at its bar) or
//! gradual (a ritardando or accelerando): the tempo glides in a straight
//! line, in beats, from the tempo before it to its own, starting at an
//! earlier bar and arriving at its bar. A time signature always changes
//! at the change's bar. Seconds in a glide are exact: the time across
//! beats `0..x` of a glide from `b0` to `b1` BPM over `L` beats is the
//! integral of `60 / (b0 + k u)` with `k = (b1 - b0) / L`, which is
//! `60 / k * ln(1 + k x / b0)`.

use serde::{Deserialize, Serialize};

use crate::synth_model::SynthControls;

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

/// How a tempo change arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TempoKind {
    /// The new tempo starts at the change's bar.
    #[default]
    Immediate,
    /// A ritardando or accelerando: the tempo glides from the one before
    /// to the change's, starting at `from_bar` (before the change's bar,
    /// not before the previous change's) and arriving at the change's bar.
    Ramp { from_bar: u32 },
}

impl TempoKind {
    pub fn is_immediate(&self) -> bool {
        matches!(self, TempoKind::Immediate)
    }
    /// Where the glide starts, for a ramp.
    pub fn ramp_from(&self) -> Option<u32> {
        match *self {
            TempoKind::Ramp { from_bar } => Some(from_bar),
            TempoKind::Immediate => None,
        }
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
    /// Immediate, or gliding in from an earlier bar.
    #[serde(default, skip_serializing_if = "TempoKind::is_immediate")]
    pub kind: TempoKind,
    /// The change's id in an `.audfrm` file it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext_id: Option<String>,
}

impl TempoChange {
    /// An immediate change.
    pub fn new(bar: u32, bpm: f64, beats_per_bar: u32, beat_unit: u32) -> TempoChange {
        TempoChange {
            bar,
            bpm,
            beats_per_bar,
            beat_unit,
            kind: TempoKind::Immediate,
            ext_id: None,
        }
    }
    /// The first bar it touches: its glide's start, or its own bar.
    pub fn first_bar(&self) -> u32 {
        self.kind.ramp_from().unwrap_or(self.bar).min(self.bar)
    }
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

/// One stretch of the tempo map with one time signature: a steady tempo,
/// or a glide from `bpm` to `end_bpm` over its first `ramp_beats`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    /// Bars from the session start where it begins.
    pub bar: u32,
    /// Quarter notes from the session start where it begins.
    pub beat: f64,
    /// Seconds from the session start where it begins.
    pub secs: f64,
    /// The tempo at its start.
    pub bpm: f64,
    /// The tempo at the end of its glide; `bpm` for a steady stretch.
    pub end_bpm: f64,
    /// Quarter notes the glide lasts (0 for a steady stretch). A glide
    /// runs to the next stretch; past its end the tempo stays `end_bpm`.
    pub ramp_beats: f64,
    pub beats_per_bar: u32,
    pub beat_unit: u32,
    /// Index into `Tempo::changes`, None for the start. A gradual change
    /// has two stretches: its glide, then its steady tempo.
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
    /// Seconds per quarter note at its start.
    pub fn spq(&self) -> f64 {
        60.0 / self.bpm
    }
    /// A glide (ritardando or accelerando) rather than a steady tempo.
    pub fn is_ramp(&self) -> bool {
        self.ramp_beats > 0.0
    }
    /// BPM gained per quarter note in the glide.
    fn slope(&self) -> f64 {
        (self.end_bpm - self.bpm) / self.ramp_beats
    }
    /// Seconds across the glide's first `x` quarter notes (0..=ramp_beats).
    fn glide_secs(&self, x: f64) -> f64 {
        let k = self.slope();
        if k == 0.0 {
            x * self.spq()
        } else {
            60.0 / k * (k * x / self.bpm).ln_1p()
        }
    }
    /// Quarter notes in the glide's first `t` seconds; inverse of
    /// `glide_secs`.
    fn glide_beats(&self, t: f64) -> f64 {
        let k = self.slope();
        if k == 0.0 {
            t / self.spq()
        } else {
            self.bpm * (k * t / 60.0).exp_m1() / k
        }
    }
    /// The tempo at a time in quarter notes, inside or beyond this stretch.
    pub fn bpm_at(&self, beats: f64) -> f64 {
        if !self.is_ramp() {
            return self.bpm;
        }
        let x = beats - self.beat;
        if x <= 0.0 {
            self.bpm
        } else if x >= self.ramp_beats {
            self.end_bpm
        } else {
            self.bpm + self.slope() * x
        }
    }
    /// Quarter notes to seconds, inside or beyond this stretch.
    pub fn secs_at(&self, beats: f64) -> f64 {
        let x = beats - self.beat;
        if !self.is_ramp() || x <= 0.0 {
            self.secs + x * self.spq()
        } else if x < self.ramp_beats {
            self.secs + self.glide_secs(x)
        } else {
            self.secs
                + self.glide_secs(self.ramp_beats)
                + (x - self.ramp_beats) * 60.0 / self.end_bpm
        }
    }
    pub fn beats_at(&self, secs: f64) -> f64 {
        let t = secs - self.secs;
        if !self.is_ramp() || t <= 0.0 {
            return self.beat + t / self.spq();
        }
        let whole = self.glide_secs(self.ramp_beats);
        if t < whole {
            self.beat + self.glide_beats(t)
        } else {
            self.beat + self.ramp_beats + (t - whole) * self.end_bpm / 60.0
        }
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
    /// A gradual change's steady stretch, due after its glide.
    pending: Option<Segment>,
}

impl Iterator for Segments<'_> {
    type Item = Segment;
    fn next(&mut self) -> Option<Segment> {
        if let Some(s) = self.pending.take() {
            self.prev = Some(s);
            return Some(s);
        }
        let Some(p) = self.prev else {
            let t = self.tempo;
            let bpm = clamp_bpm(t.bpm);
            let s = Segment {
                bar: 0,
                beat: 0.0,
                secs: 0.0,
                bpm,
                end_bpm: bpm,
                ramp_beats: 0.0,
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
            let bpm = clamp_bpm(c.bpm);
            let steady = Segment {
                bar: c.bar,
                beat,
                secs: 0.0,
                bpm,
                end_bpm: bpm,
                ramp_beats: 0.0,
                beats_per_bar: c.beats_per_bar.max(1),
                beat_unit: clamp_unit(c.beat_unit),
                change: Some(i),
            };
            if let Some(from) = c.kind.ramp_from() {
                // The glide starts no earlier than the stretch before it
                // and keeps that stretch's time signature.
                let from = from.clamp(p.bar, c.bar - 1);
                let start = p.beat + (from - p.bar) as f64 * p.bar_beats();
                let glide = Segment {
                    bar: from,
                    beat: start,
                    secs: p.secs_at(start),
                    bpm: p.bpm_at(start),
                    end_bpm: bpm,
                    ramp_beats: beat - start,
                    beats_per_bar: p.beats_per_bar,
                    beat_unit: p.beat_unit,
                    change: Some(i),
                };
                self.pending = Some(Segment {
                    secs: glide.secs_at(beat),
                    ..steady
                });
                self.prev = Some(glide);
                return Some(glide);
            }
            let s = Segment {
                secs: p.secs_at(beat),
                ..steady
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
            pending: None,
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

    /// The steady stretch a change (None: the start) sets, the one after
    /// its glide for a gradual change.
    pub fn change_segment(&self, change: Option<usize>) -> Option<Segment> {
        self.segments().filter(|s| s.change == change).last()
    }

    /// A gradual change's glide stretch.
    pub fn ramp_segment(&self, change: usize) -> Option<Segment> {
        self.segments()
            .find(|s| s.change == Some(change) && s.is_ramp())
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

    /// The tempo at a time in quarter notes (between the two ends in a
    /// glide).
    pub fn bpm_at(&self, beats: f64) -> f64 {
        self.seg_at_beat(beats).bpm_at(beats)
    }

    /// The tempo at a time in seconds.
    pub fn bpm_at_secs(&self, secs: f64) -> f64 {
        self.bpm_at(self.beats(secs))
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
    /// order, one per bar (the later one wins), none at bar 0, and every
    /// glide starting before its bar and no earlier than the change
    /// before it.
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
        let mut prev = 0;
        for c in &mut out {
            if let TempoKind::Ramp { from_bar } = &mut c.kind {
                *from_bar = (*from_bar).clamp(prev, c.bar - 1);
            }
            prev = c.bar;
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
        let bpm = s.bpm_at(self.bar_start(bar));
        self.changes
            .push(TempoChange::new(bar, bpm, s.beats_per_bar, s.beat_unit));
        self.normalize();
        self.changes.iter().position(|c| c.bar == bar).unwrap_or(0)
    }

    /// Makes the change at `to` (from 1; added with the values in effect
    /// there when there is none) glide in from bar `from`, to `bpm` when
    /// given. Returns its index.
    pub fn add_ramp(&mut self, from: u32, to: u32, bpm: Option<f64>) -> usize {
        let to = to.max(1);
        let i = self.add_change(to);
        let c = &mut self.changes[i];
        c.kind = TempoKind::Ramp {
            from_bar: from.min(to - 1),
        };
        if let Some(b) = bpm {
            c.bpm = b;
        }
        self.normalize();
        self.changes.iter().position(|c| c.bar == to).unwrap_or(0)
    }

    /// Moves a change to another bar (from 1); a glide keeps its length in
    /// bars. A change already at that bar is replaced. Returns its new
    /// index.
    pub fn move_change(&mut self, i: usize, bar: u32) -> usize {
        if i >= self.changes.len() {
            return i;
        }
        let bar = bar.max(1);
        let mut c = self.changes.remove(i);
        if let TempoKind::Ramp { from_bar } = &mut c.kind {
            let len = c.bar.saturating_sub(*from_bar).max(1);
            *from_bar = bar.saturating_sub(len);
        }
        c.bar = bar;
        // Last, so it wins over one already at the bar.
        self.changes.push(c);
        self.normalize();
        self.changes.iter().position(|c| c.bar == bar).unwrap_or(0)
    }

    /// Immediate (None) or gliding in from bar `from`.
    pub fn set_ramp(&mut self, i: usize, from: Option<u32>) {
        if let Some(c) = self.changes.get_mut(i) {
            c.kind = match from {
                Some(f) => TempoKind::Ramp {
                    from_bar: f.min(c.bar.saturating_sub(1)),
                },
                None => TempoKind::Immediate,
            };
        }
        self.normalize();
    }

    pub fn remove_change(&mut self, i: usize) {
        if i < self.changes.len() {
            self.changes.remove(i);
            self.normalize();
        }
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

    /// Whether any change glides.
    pub fn has_ramps(&self) -> bool {
        self.segments().any(|s| s.is_ramp())
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

/// How a MIDI clip's notes meet the tempo map under it.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum ClipTiming {
    /// Its beats are the session's: it speeds up and slows down with
    /// tempo changes and glides under it, its length in seconds changing.
    #[default]
    FollowTempo,
    /// It plays at its own fixed tempo wherever it sits: its length in
    /// seconds stays put, its length in session beats changes.
    KeepSpeed { bpm: f64 },
}

impl ClipTiming {
    pub fn is_follow(&self) -> bool {
        matches!(self, ClipTiming::FollowTempo)
    }
}

/// Lowest and highest MIDI clip stretch (timeline length over the clip's
/// own length).
pub const MIDI_STRETCH_MIN: f64 = 0.25;
pub const MIDI_STRETCH_MAX: f64 = 4.0;
/// Semitones a MIDI clip can be shifted either way.
pub const MIDI_PITCH_MAX: f32 = 24.0;

fn one() -> f64 {
    1.0
}

fn is_one(x: &f64) -> bool {
    *x == 1.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiClip {
    pub id: ClipId,
    pub name: String,
    /// Beats from the session start.
    pub start: f64,
    /// The clip's own beats (its notes' time), before stretch and timing:
    /// on the timeline it lasts [`MidiClip::end_beats`] minus `start`.
    pub length: f64,
    pub notes: Vec<MidiNote>,
    /// Follow the session's tempo, or keep its own speed.
    #[serde(default, skip_serializing_if = "ClipTiming::is_follow")]
    pub timing: ClipTiming,
    /// Timeline length over its own length (2 plays at half speed, its
    /// notes twice as long), 0.25..4. The notes themselves are unchanged.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub stretch: f64,
    /// Pitch shift in semitones (-24..24): whole semitones transpose the
    /// notes, the fraction detunes the instrument.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pitch: f32,
    #[serde(default)]
    pub muted: bool,
    /// Clip level in dB: scales every note's velocity (-96..24).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gain_db: f32,
    /// Controller moves (sustain pedal, mod wheel, pitch bend...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<ControlEvent>,
    /// The object's id in an `.audfrm` file it came from, so saving back
    /// keeps everything Audiform2 does not use.
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
/// The sostenuto pedal: holds only the notes down when it goes down.
pub const SOSTENUTO: u8 = 66;
/// The mod wheel.
pub const MOD_WHEEL: u8 = 1;

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

/// A note as it will play: timeline seconds, velocity 0..1 after the
/// clip level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlannedNote {
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
    pub velocity: f32,
    pub channel: u8,
    pub pan: f32,
    /// Semitones the instrument plays it off its key (-0.5..0.5), from
    /// the clip's fractional pitch shift.
    pub detune: f32,
}

/// Every note of `clips` as it plays through the tempo map: muted clips
/// are silent, notes starting at or past their clip's end are not played
/// and the rest are cut at it, the clip level scales velocity (within
/// 1/127..1), the clip's timing, stretch and pitch shift apply, and a
/// sustain pedal held down in the clip keeps released
/// notes sounding until it comes up (the built-in instruments have no
/// pedal of their own). The sostenuto pedal holds only the notes whose
/// keys were down when it went down, until it comes up (a sustain pedal
/// still down then holds them on). Sorted by start; a note always lasts a
/// frame.
pub fn plan_notes(clips: &[MidiClip], tempo: &Tempo) -> Vec<PlannedNote> {
    let mut out = Vec::new();
    for c in clips.iter().filter(|c| !c.muted) {
        let mut pedal: Vec<&ControlEvent> = c
            .controls
            .iter()
            .filter(|e| e.controller == SUSTAIN && e.at < c.length)
            .collect();
        pedal.sort_by(|a, b| a.at.total_cmp(&b.at));
        // Where the pedal (on `ch`) next comes up after it was down at `b`.
        let held_until = |ch: u8, b: f64| -> Option<f64> {
            let down = pedal
                .iter()
                .rev()
                .find(|e| e.channel == ch && e.at <= b)
                .is_some_and(|e| e.value >= 0.5);
            if !down {
                return None;
            }
            Some(
                pedal
                    .iter()
                    .find(|e| e.channel == ch && e.at > b && e.value < 0.5)
                    .map_or(c.length, |e| e.at),
            )
        };
        let mut sost: Vec<&ControlEvent> = c
            .controls
            .iter()
            .filter(|e| e.controller == SOSTENUTO && e.at < c.length)
            .collect();
        sost.sort_by(|a, b| a.at.total_cmp(&b.at));
        // Sostenuto presses (on `ch`) while a key was down over `a..b`:
        // where the pedal next comes up.
        let sost_until = |ch: u8, a: f64, b: f64| -> Option<f64> {
            let mut down = false;
            let mut up_at: Option<f64> = None;
            for (k, e) in sost.iter().enumerate().filter(|(_, e)| e.channel == ch) {
                let now = e.value >= 0.5;
                if now && !down && e.at >= a && e.at < b {
                    up_at = Some(
                        sost[k + 1..]
                            .iter()
                            .find(|x| x.channel == ch && x.value < 0.5)
                            .map_or(c.length, |x| x.at),
                    );
                }
                down = now;
            }
            up_at
        };
        for n in &c.notes {
            if n.start >= c.length || n.length <= 0.0 || n.start < 0.0 {
                continue;
            }
            let mut end = (n.start + n.length).min(c.length);
            if let Some(up) = sost_until(n.channel, n.start, end) {
                end = up.max(end);
            }
            if let Some(up) = held_until(n.channel, end) {
                end = up.max(end);
            }
            let start = c.secs_at(n.start, tempo);
            let stop = c.secs_at(end, tempo);
            out.push(PlannedNote {
                start,
                end: stop.max(start + 1e-6),
                pitch: c.shifted(n.pitch),
                detune: c.detune(),
                velocity: c.velocity(n),
                channel: n.channel.min(15),
                pan: if n.pan.is_nan() {
                    0.0
                } else {
                    n.pan.clamp(-1.0, 1.0)
                },
            });
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    out
}

/// A controller move as it will play: timeline seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlannedControl {
    pub t: f64,
    pub controller: u8,
    /// 0..1 (pitch bend: 0.5 is the middle).
    pub value: f32,
}

/// The mod wheel and pitch bend moves of `clips` as they play (muted
/// clips left out), in time order. A clip that moves either puts it back
/// (wheel down, bend centred) where it ends, so the next clip starts
/// clean.
pub fn plan_controls(clips: &[MidiClip], tempo: &Tempo) -> Vec<PlannedControl> {
    let mut out = Vec::new();
    for c in clips.iter().filter(|c| !c.muted) {
        let mut used = [false; 2];
        for e in &c.controls {
            let k = match e.controller {
                MOD_WHEEL => 0,
                PITCH_BEND => 1,
                _ => continue,
            };
            if e.at < 0.0 || e.at >= c.length {
                continue;
            }
            used[k] = true;
            out.push(PlannedControl {
                t: c.secs_at(e.at, tempo),
                controller: e.controller,
                value: if e.value.is_nan() {
                    0.0
                } else {
                    e.value.clamp(0.0, 1.0)
                },
            });
        }
        let end = c.secs_at(c.length, tempo);
        for (k, (controller, value)) in [(MOD_WHEEL, 0.0), (PITCH_BEND, 0.5)]
            .into_iter()
            .enumerate()
        {
            if used[k] {
                out.push(PlannedControl {
                    t: end,
                    controller,
                    value,
                });
            }
        }
    }
    // Stable: a clip's own moves at one time keep their order.
    out.sort_by(|a, b| a.t.total_cmp(&b.t));
    out
}

/// A controller move as a plug-in instrument hears it: timeline seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlannedMove {
    pub at: f64,
    /// 0..127, [`AFTERTOUCH`] or [`PITCH_BEND`].
    pub controller: u8,
    /// 0..1.
    pub value: f32,
    pub channel: u8,
}

/// Every note and controller move of `clips` as an instrument with a
/// pedal of its own (a plug-in) hears them: as [`plan_notes`], except
/// that notes end where they are written (the sustain pedal is sent as
/// controller 64, not folded into the notes). Controller moves inside
/// each unmuted clip come along, in timeline seconds. Both are sorted.
pub fn plan_events(clips: &[MidiClip], tempo: &Tempo) -> (Vec<PlannedNote>, Vec<PlannedMove>) {
    let mut notes = Vec::new();
    let mut controls = Vec::new();
    for c in clips.iter().filter(|c| !c.muted) {
        for n in &c.notes {
            if n.start >= c.length || n.length <= 0.0 || n.start < 0.0 {
                continue;
            }
            let end = (n.start + n.length).min(c.length);
            let start = c.secs_at(n.start, tempo);
            let stop = c.secs_at(end, tempo);
            notes.push(PlannedNote {
                start,
                end: stop.max(start + 1e-6),
                pitch: c.shifted(n.pitch),
                detune: c.detune(),
                velocity: c.velocity(n),
                channel: n.channel.min(15),
                pan: if n.pan.is_nan() {
                    0.0
                } else {
                    n.pan.clamp(-1.0, 1.0)
                },
            });
        }
        for e in c.controls.iter().filter(|e| e.at >= 0.0 && e.at < c.length) {
            controls.push(PlannedMove {
                at: c.secs_at(e.at, tempo),
                controller: e.controller.min(PITCH_BEND),
                value: if e.value.is_finite() {
                    e.value.clamp(0.0, 1.0)
                } else {
                    0.0
                },
                channel: e.channel.min(15),
            });
        }
    }
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    controls.sort_by(|a, b| a.at.total_cmp(&b.at));
    (notes, controls)
}

/// The controllers controller lanes offer first.
pub const LANE_CONTROLLERS: [u8; 10] = [64, 1, 11, 7, 10, 2, 66, 67, PITCH_BEND, AFTERTOUCH];

impl MidiClip {
    /// A new empty clip of `length` beats that follows the tempo,
    /// unstretched and unshifted.
    pub fn new(id: ClipId, name: impl Into<String>, start: f64, length: f64) -> MidiClip {
        MidiClip {
            id,
            name: name.into(),
            start,
            length,
            notes: Vec::new(),
            timing: ClipTiming::FollowTempo,
            stretch: 1.0,
            pitch: 0.0,
            muted: false,
            gain_db: 0.0,
            controls: Vec::new(),
            ext_id: None,
        }
    }

    /// Start plus its own length: its end on the timeline while it
    /// follows the tempo unstretched. [`MidiClip::end_beats`] is the real
    /// end.
    pub fn end(&self) -> f64 {
        self.start + self.length
    }

    /// The stretch applied (0.25..4).
    pub fn ratio(&self) -> f64 {
        if self.stretch.is_finite() {
            self.stretch.clamp(MIDI_STRETCH_MIN, MIDI_STRETCH_MAX)
        } else {
            1.0
        }
    }

    /// The pitch shift applied in semitones.
    pub fn shift(&self) -> f32 {
        if self.pitch.is_finite() {
            self.pitch.clamp(-MIDI_PITCH_MAX, MIDI_PITCH_MAX)
        } else {
            0.0
        }
    }

    /// A key after the whole semitones of the pitch shift.
    pub fn shifted(&self, pitch: u8) -> u8 {
        (pitch.min(127) as i32 + self.shift().round() as i32).clamp(0, 127) as u8
    }

    /// The fraction of the pitch shift the instrument detunes by.
    pub fn detune(&self) -> f32 {
        let s = self.shift();
        s - s.round()
    }

    /// Whether it plays other than as written: kept speed, stretched or
    /// shifted.
    pub fn is_altered(&self) -> bool {
        !self.timing.is_follow() || self.ratio() != 1.0 || self.shift() != 0.0
    }

    /// Timeline seconds of a time in its own beats (inside or past it).
    pub fn secs_at(&self, x: f64, tempo: &Tempo) -> f64 {
        match self.timing {
            ClipTiming::FollowTempo => tempo.secs(self.start + x * self.ratio()),
            ClipTiming::KeepSpeed { bpm } => {
                tempo.secs(self.start) + x * self.ratio() * 60.0 / clamp_bpm(bpm)
            }
        }
    }

    /// Session beats of a time in its own beats.
    pub fn beats_at(&self, x: f64, tempo: &Tempo) -> f64 {
        match self.timing {
            ClipTiming::FollowTempo => self.start + x * self.ratio(),
            ClipTiming::KeepSpeed { .. } => tempo.beats(self.secs_at(x, tempo)),
        }
    }

    /// Its own beats at a time in session beats; inverse of `beats_at`.
    pub fn local_at(&self, beats: f64, tempo: &Tempo) -> f64 {
        match self.timing {
            ClipTiming::FollowTempo => (beats - self.start) / self.ratio(),
            ClipTiming::KeepSpeed { bpm } => {
                (tempo.secs(beats) - tempo.secs(self.start)) * clamp_bpm(bpm) / 60.0 / self.ratio()
            }
        }
    }

    /// Where it ends on the timeline, in session beats.
    pub fn end_beats(&self, tempo: &Tempo) -> f64 {
        self.beats_at(self.length, tempo)
    }

    /// Where it ends on the timeline, in seconds.
    pub fn end_secs(&self, tempo: &Tempo) -> f64 {
        self.secs_at(self.length, tempo)
    }

    /// Switches between following the tempo and keeping its speed (at
    /// the tempo where it starts). Every note, controller move and its
    /// end stay where they sound now: under a steady tempo nothing
    /// changes; across tempo changes the notes are re-timed in its own
    /// beats to match.
    pub fn set_timing(&mut self, keep: bool, tempo: &Tempo) {
        if keep != self.timing.is_follow() {
            return;
        }
        let old = self.clone();
        self.timing = if keep {
            ClipTiming::KeepSpeed {
                bpm: clamp_bpm(tempo.bpm_at(self.start)),
            }
        } else {
            ClipTiming::FollowTempo
        };
        let new = self.clone();
        let remap = |x: f64| new.local_at(old.beats_at(x, tempo), tempo);
        // Float dust is snapped away, so a steady tempo leaves it exactly
        // as it was.
        let tidy = |a: f64, b: f64| if (a - b).abs() < 1e-9 { b } else { a };
        self.length = tidy(remap(old.length), old.length);
        for (n, o) in self.notes.iter_mut().zip(&old.notes) {
            let a = remap(o.start);
            n.start = tidy(a, o.start);
            n.length = tidy(remap(o.start + o.length) - a, o.length);
        }
        for (e, o) in self.controls.iter_mut().zip(&old.controls) {
            e.at = tidy(remap(o.at), o.at);
        }
    }

    /// Drags one edge to `to` (session beats), stretching it so the other
    /// edge stays put, within 25%..400%.
    pub fn stretch_edge(&mut self, left: bool, to: f64, tempo: &Tempo) {
        if self.length <= 0.0 {
            return;
        }
        let end_s = self.end_secs(tempo);
        let start_s = tempo.secs(self.start);
        // The span in seconds it should cover.
        let (a, b) = if left {
            (tempo.secs(to.max(0.0)), end_s)
        } else {
            (start_s, tempo.secs(to))
        };
        let r = match self.timing {
            ClipTiming::FollowTempo => (tempo.beats(b) - tempo.beats(a)) / self.length,
            ClipTiming::KeepSpeed { bpm } => (b - a) * clamp_bpm(bpm) / 60.0 / self.length,
        };
        self.stretch = r.clamp(MIDI_STRETCH_MIN, MIDI_STRETCH_MAX);
        if left {
            // Back from the fixed end at the clamped stretch.
            self.start = match self.timing {
                ClipTiming::FollowTempo => tempo.beats(end_s) - self.length * self.stretch,
                ClipTiming::KeepSpeed { bpm } => {
                    tempo.beats(end_s - self.length * self.stretch * 60.0 / clamp_bpm(bpm))
                }
            }
            .max(0.0);
        }
    }

    /// The badge on its title: "Keep 120 BPM 150% +2.0st", or empty.
    pub fn badge(&self) -> String {
        let mut out = Vec::new();
        if let ClipTiming::KeepSpeed { bpm } = self.timing {
            out.push(format!("Keep {} BPM", (bpm * 10.0).round() / 10.0));
        }
        if self.ratio() != 1.0 {
            out.push(format!("{:.0}%", self.ratio() * 100.0));
        }
        if self.shift() != 0.0 {
            out.push(format!("{:+.1}st", self.shift()));
        }
        out.join(" ")
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
    /// A pulse of the instrument's pulse width (`ext.pw`).
    Pulse,
}

/// The built-in synth: one oscillator plus a detuned second, an ADSR
/// envelope and a resonant low-pass filter, and in `ext` everything else
/// a real synth has (see [`crate::synth_model`]).
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
    /// The newer controls; saved only when changed, so an instrument
    /// that never used them saves exactly as before.
    #[serde(default, skip_serializing_if = "SynthControls::is_default")]
    pub ext: SynthControls,
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
            ext: SynthControls::default(),
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
        TempoChange::new(bar, bpm, top, unit)
    }

    fn ramp(from: u32, bar: u32, bpm: f64) -> TempoChange {
        TempoChange {
            kind: TempoKind::Ramp { from_bar: from },
            ..TempoChange::new(bar, bpm, 4, 4)
        }
    }

    #[test]
    fn a_ritardando_lasts_its_analytic_length() {
        // 120 to 60 BPM over bars 1..5 (16 quarter notes) of 4/4: the
        // time is 60 / k * ln(60 / 120) with k = -60 / 16 per beat.
        let mut t = Tempo::default();
        t.changes.push(ramp(1, 5, 60.0));
        let want = 16.0 * 2f64.ln();
        assert!(close(t.secs(4.0), 2.0));
        assert!(close(t.secs(20.0) - t.secs(4.0), want));
        // Then steady at 60.
        assert!(close(t.secs(21.0) - t.secs(20.0), 1.0));
        // Halfway through in beats the tempo is halfway.
        assert!(close(t.bpm_at(12.0), 90.0));
        assert!(close(t.bpm_at_secs(t.secs(12.0)), 90.0));
        assert!(close(t.bpm_at(2.0), 120.0));
        assert!(close(t.bpm_at(30.0), 60.0));
        // Bars and their numbers are not moved by the glide.
        assert!(close(t.bar_start(5), 20.0));
        assert_eq!(t.format(t.secs(20.0)), "6.1.000");
        let segs: Vec<_> = t.segments().collect();
        assert_eq!(segs.len(), 3);
        assert!(segs[1].is_ramp() && !segs[2].is_ramp());
        assert_eq!(segs[1].change, segs[2].change);
        assert_eq!(t.change_segment(Some(0)).unwrap().bar, 5);
        assert_eq!(t.ramp_segment(0).unwrap().bar, 1);
    }

    #[test]
    fn ramps_round_trip_and_time_only_moves_forward() {
        let mut t = Tempo::default();
        t.changes.push(ramp(1, 3, 200.0));
        t.changes.push(change(4, 90.0, 3, 4));
        t.changes.push(ramp(5, 9, 90.0)); // same tempo: a flat glide
        t.changes.push(ramp(9, 12, 45.0));
        t.normalize();
        let mut last = f64::MIN;
        for i in 0..400 {
            let b = i as f64 * 0.173 - 2.0;
            let s = t.secs(b);
            assert!(s > last, "at {b}");
            last = s;
            assert!((t.beats(s) - b).abs() < 1e-9, "at {b}");
        }
        // An accelerando speeds up: each beat shorter than the last.
        let d1 = t.secs(5.0) - t.secs(4.0);
        let d2 = t.secs(11.0) - t.secs(10.0);
        assert!(d2 < d1);
        // A flat glide is a steady tempo.
        let a = t.bar_start(5);
        assert!(close(t.secs(a + 4.0) - t.secs(a), 4.0 * 60.0 / 90.0));
    }

    #[test]
    fn immediate_changes_are_unchanged_by_ramps_elsewhere() {
        let mut t = Tempo::default();
        t.changes.push(change(2, 60.0, 4, 4));
        let plain: Vec<f64> = (0..50).map(|i| t.secs(i as f64 * 0.5)).collect();
        assert_eq!(t.segments().count(), 2);
        assert!(!t.has_ramps());
        // A glide after the last point checked moves nothing before it.
        t.changes.push(ramp(7, 9, 100.0));
        for (i, s) in plain.iter().enumerate() {
            assert!(close(t.secs(i as f64 * 0.5), *s));
        }
    }

    #[test]
    fn ramp_edits_keep_the_map_tidy() {
        let mut t = Tempo::default();
        t.changes.push(change(2, 100.0, 4, 4));
        // A glide may not start before the change before it.
        let i = t.add_ramp(0, 6, Some(80.0));
        assert_eq!(t.changes[i].kind, TempoKind::Ramp { from_bar: 2 });
        assert_eq!(t.changes[i].bpm, 80.0);
        // Moving keeps the glide's length.
        let i = t.move_change(i, 8);
        assert_eq!(t.changes[i].kind, TempoKind::Ramp { from_bar: 4 });
        // A change added inside a glide starts at the glided tempo there,
        // and shortens the glide.
        let j = t.add_change(5);
        assert!(close(t.changes[j].bpm, 100.0 - 20.0 * 0.25));
        let i = t.changes.iter().position(|c| c.bar == 8).unwrap();
        assert_eq!(t.changes[i].kind, TempoKind::Ramp { from_bar: 5 });
        t.set_ramp(i, None);
        assert!(t.changes[i].kind.is_immediate());
        t.remove_change(i);
        assert_eq!(t.changes.len(), 2);
        // Saved only when it glides.
        let json = serde_json::to_string(&t.changes[0]).unwrap();
        assert!(!json.contains("kind"));
        t.set_ramp(1, Some(3));
        let json = serde_json::to_string(&t).unwrap();
        let back: Tempo = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
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

    fn clip_with(notes: &[(f64, f64)], start: f64, length: f64) -> MidiClip {
        let mut c = MidiClip::new(7, "c", start, length);
        c.notes = notes
            .iter()
            .map(|&(s, l)| MidiNote {
                pitch: 60,
                start: s,
                length: l,
                velocity: 100,
                channel: 0,
                pan: 0.0,
            })
            .collect();
        c
    }

    fn cc(at: f64, controller: u8, value: f32) -> ControlEvent {
        ControlEvent {
            at,
            controller,
            value,
            channel: 0,
        }
    }

    #[test]
    fn sostenuto_holds_only_the_notes_down_when_it_went_down() {
        // 120 BPM: a beat is 0.5 s. Note A 0..1, note B 1.5..2 (beats).
        let t = Tempo::default();
        let mut c = clip_with(&[(0.0, 1.0), (1.5, 0.5)], 0.0, 8.0);
        c.controls = vec![cc(0.5, SOSTENUTO, 1.0), cc(4.0, SOSTENUTO, 0.0)];
        let p = plan_notes(std::slice::from_ref(&c), &t);
        // A was down at the press: held to beat 4. B came after: not held.
        assert_eq!((p[0].start, p[0].end), (0.0, 2.0));
        assert_eq!((p[1].start, p[1].end), (0.75, 1.0));
        // Sustain still down when sostenuto lifts holds A on.
        c.controls.push(cc(3.0, SUSTAIN, 1.0));
        c.controls.push(cc(6.0, SUSTAIN, 0.0));
        let p = plan_notes(std::slice::from_ref(&c), &t);
        assert_eq!(p[0].end, 3.0);
    }

    #[test]
    fn wheel_and_bend_moves_play_in_time_and_reset_at_the_clip_end() {
        let t = Tempo::default();
        let mut c = clip_with(&[(0.0, 1.0)], 2.0, 4.0);
        c.controls = vec![
            cc(1.0, PITCH_BEND, 1.0),
            cc(0.5, MOD_WHEEL, 0.6),
            cc(1.0, SUSTAIN, 1.0),
        ];
        let p = plan_controls(std::slice::from_ref(&c), &t);
        let v: Vec<(f64, u8, f32)> = p.iter().map(|x| (x.t, x.controller, x.value)).collect();
        assert_eq!(
            v,
            vec![
                (1.25, MOD_WHEEL, 0.6),
                (1.5, PITCH_BEND, 1.0),
                (3.0, MOD_WHEEL, 0.0),
                (3.0, PITCH_BEND, 0.5)
            ]
        );
        c.muted = true;
        assert!(plan_controls(&[c], &t).is_empty());
    }

    #[test]
    fn switching_timing_leaves_every_note_where_it_sounds() {
        // Two bars at 120 (4 s), then 60 BPM.
        let mut t = Tempo::default();
        t.changes.push(change(2, 60.0, 4, 4));
        let mut c = clip_with(&[(0.0, 1.0), (6.0, 4.0), (12.0, 2.0)], 0.0, 16.0);
        let before = plan_notes(std::slice::from_ref(&c), &t);
        let end = c.end_secs(&t);
        c.set_timing(true, &t);
        assert_eq!(c.timing, ClipTiming::KeepSpeed { bpm: 120.0 });
        let after = plan_notes(std::slice::from_ref(&c), &t);
        for (a, b) in before.iter().zip(&after) {
            assert!(
                close(a.start, b.start) && close(a.end, b.end),
                "{a:?} {b:?}"
            );
        }
        assert!(close(c.end_secs(&t), end));
        // At its own 120 BPM the note at 12 session beats (8 s) is at 16.
        assert!(close(c.notes[2].start, 16.0));
        c.set_timing(false, &t);
        assert!(c.timing.is_follow());
        assert!(close(c.notes[2].start, 12.0) && close(c.length, 16.0));
        // Under a steady tempo nothing changes at all.
        let steady = Tempo::constant(93.0, 4, 4);
        let mut d = clip_with(&[(0.3, 1.7), (5.1, 0.2)], 3.0, 8.0);
        let was = d.clone();
        d.set_timing(true, &steady);
        assert_eq!((d.notes.clone(), d.length), (was.notes.clone(), was.length));
        d.set_timing(false, &steady);
        assert_eq!(d, was);
    }

    #[test]
    fn a_kept_speed_clip_lasts_the_same_seconds_anywhere() {
        let mut t = Tempo::default();
        t.changes.push(change(2, 60.0, 4, 4));
        t.changes.push(ramp(4, 6, 180.0));
        let mut c = clip_with(&[(0.0, 1.0)], 0.0, 8.0);
        c.set_timing(true, &t);
        for start in [0.0, 6.0, 9.0, 18.0, 30.0] {
            c.start = start;
            assert!(close(c.end_secs(&t) - t.secs(start), 4.0), "at {start}");
            // Its length in session beats follows the map under it.
            let b = c.end_beats(&t);
            assert!(close(t.secs(b), c.end_secs(&t)));
            assert!(close(c.local_at(b, &t), 8.0));
        }
        // Following the tempo, the same clip lasts 8 beats anywhere.
        c.set_timing(false, &t);
        c.start = 9.0;
        assert!(close(c.end_beats(&t) - 9.0, c.length));
    }

    #[test]
    fn stretch_scales_the_notes_and_edges_drag_it() {
        let t = Tempo::default();
        let mut c = clip_with(&[(1.0, 1.0)], 4.0, 4.0);
        c.stretch = 2.0;
        let p = plan_notes(std::slice::from_ref(&c), &t);
        // Beat 1 of the clip at 200% is 2 beats in: 6 beats, 3 s.
        assert!(close(p[0].start, 3.0) && close(p[0].end, 4.0));
        assert!(close(c.end_beats(&t), 12.0));
        assert!(close(c.local_at(6.0, &t), 1.0));
        // The right edge to beat 6: 50%.
        c.stretch_edge(false, 6.0, &t);
        assert!(close(c.ratio(), 0.5));
        // The left edge keeps the end put.
        let end = c.end_beats(&t);
        c.stretch_edge(true, 2.0, &t);
        assert!(close(c.start, 2.0) && close(c.end_beats(&t), end));
        assert!(close(c.ratio(), 1.0));
        // Held to 25%..400%.
        c.stretch_edge(false, 1000.0, &t);
        assert_eq!(c.ratio(), MIDI_STRETCH_MAX);
        c.stretch_edge(true, c.end_beats(&t) - 0.01, &t);
        assert_eq!(c.ratio(), MIDI_STRETCH_MIN);
        assert!(close(c.end_beats(&t), 2.0 + 16.0));
        // A kept-speed clip stretches in seconds.
        let mut k = clip_with(&[], 0.0, 4.0);
        k.timing = ClipTiming::KeepSpeed { bpm: 60.0 };
        k.stretch_edge(false, 4.0, &t);
        assert!(close(k.ratio(), 0.5));
        assert!(close(k.end_secs(&t), 2.0));
    }

    #[test]
    fn pitch_shift_transposes_and_detunes() {
        let t = Tempo::default();
        let mut c = clip_with(&[(0.0, 1.0)], 0.0, 4.0);
        c.pitch = 2.3;
        let p = plan_notes(std::slice::from_ref(&c), &t);
        assert_eq!(p[0].pitch, 62);
        assert!((p[0].detune - 0.3).abs() < 1e-5);
        c.pitch = -1.6;
        assert_eq!(c.shifted(60), 58);
        assert!((c.detune() - 0.4).abs() < 1e-5);
        c.pitch = 24.0;
        assert_eq!(c.shifted(120), 127);
        assert_eq!(c.badge(), "+24.0st");
        c.stretch = 1.5;
        c.timing = ClipTiming::KeepSpeed { bpm: 120.0 };
        assert_eq!(c.badge(), "Keep 120 BPM 150% +24.0st");
        // Saved only when set; old clips read as plain.
        let plain = clip_with(&[], 0.0, 4.0);
        let json = serde_json::to_string(&plain).unwrap();
        assert!(!json.contains("timing") && !json.contains("stretch") && !json.contains("pitch\""));
        let back: MidiClip = serde_json::from_str(&json).unwrap();
        assert_eq!(back, plain);
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<MidiClip>(&json).unwrap(), c);
    }

    #[test]
    fn old_files_read_with_defaults() {
        let t: Tempo = serde_json::from_str(r#"{"bpm": 90, "beats_per_bar": 3}"#).unwrap();
        assert_eq!(t.beat_unit, 4);
        assert!(t.changes.is_empty());
    }
}
