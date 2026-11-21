// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! SoundFont (.sf2) instruments through `rustysynth` (MIT).
//!
//! Fonts are loaded once on the UI thread and shared (`load`). A track's
//! player gets its own synthesizer, made when the mix is built, and its
//! notes as a sorted list of note-on and note-off events. On the audio
//! thread it only walks that list and renders into buffers it already has:
//! nothing allocates. After a jump it lets every note go and starts the
//! notes that are sounding at the new place.
//!
//! The track's preset is set on every channel but 10 (rustysynth's drum
//! channel adds 128 to its bank), and notes take channels in turn, so each
//! note's pan (controller 10) is its own until its channel comes round
//! again. A drum kit is chosen as bank 128, like any other preset.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use af27_core::midi::SoundFontRef;
use af27_core::time::db_to_gain;
use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};

use crate::mix::BLOCK;

pub use rustysynth::SoundFont as Font;

/// Channels notes take in turn (all but the drum channel).
const CHANNELS: [i32; 15] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15];

fn cache() -> &'static Mutex<HashMap<PathBuf, Arc<SoundFont>>> {
    static C: OnceLock<Mutex<HashMap<PathBuf, Arc<SoundFont>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Loads a font, or hands back the one already loaded. UI thread only.
pub fn load(path: &Path) -> Result<Arc<SoundFont>, String> {
    if let Some(f) = cache().lock().ok().and_then(|c| c.get(path).cloned()) {
        return Ok(f);
    }
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    let font = Arc::new(
        SoundFont::new(&mut file)
            .map_err(|e| format!("{} is not a SoundFont: {e:?}", path.display()))?,
    );
    if let Ok(mut c) = cache().lock() {
        c.insert(path.to_path_buf(), font.clone());
    }
    Ok(font)
}

/// One preset of a font.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub bank: u16,
    pub program: u8,
    pub name: String,
}

/// A font's presets by bank, then program.
pub fn presets(font: &SoundFont) -> Vec<Preset> {
    let mut v: Vec<Preset> = font
        .get_presets()
        .iter()
        .map(|p| Preset {
            bank: p.get_bank_number().clamp(0, 16383) as u16,
            program: p.get_patch_number().clamp(0, 127) as u8,
            name: p.get_name().trim().to_string(),
        })
        .collect();
    v.sort_by_key(|p| (p.bank, p.program));
    v
}

fn synthesizer(font: &Arc<SoundFont>, rate: u32, bank: u16, program: u8) -> Option<Synthesizer> {
    let mut settings = SynthesizerSettings::new(rate as i32);
    settings.enable_reverb_and_chorus = false;
    let mut s = Synthesizer::new(font, &settings).ok()?;
    for ch in 0..16 {
        s.process_midi_message(ch, 0xB0, 0x00, bank as i32);
        s.process_midi_message(ch, 0xC0, program as i32, 0);
    }
    Some(s)
}

fn pan_cc(p: f32) -> i32 {
    (64.0 + p.clamp(-1.0, 1.0) * 63.0).round() as i32
}

/// A 14-bit pitch bend (8192 is none) for `semitones` at a bend range
/// of two semitones.
fn bend_value(semitones: f32) -> i32 {
    (8192.0 + semitones.clamp(-2.0, 2.0) / 2.0 * 8192.0)
        .round()
        .clamp(0.0, 16383.0) as i32
}

/// One note in seconds.
#[derive(Debug, Clone, Copy)]
pub struct SfNote {
    pub start: f64,
    pub end: f64,
    pub key: u8,
    pub velocity: u8,
    pub pan: f32,
    /// Semitones off its key (-0.5..0.5), played as pitch bend on its
    /// channel (General MIDI's bend range is two semitones each way).
    pub detune: f32,
}

#[derive(Debug, Clone, Copy)]
struct Event {
    t: f64,
    on: bool,
    note: u32,
}

/// A MIDI track's SoundFont, playing its notes.
pub struct SfPlayer {
    synth: Synthesizer,
    notes: Vec<SfNote>,
    events: Vec<Event>,
    cursor: usize,
    next_t: f64,
    seek: bool,
    gain: f32,
    sl: Vec<f32>,
    sr: Vec<f32>,
    /// Clip mod-wheel and pitch-bend moves (time order), the next one,
    /// the bend now (semitones) and each channel's note detune.
    ctl: Vec<af27_core::midi::PlannedControl>,
    ctl_at: usize,
    bend: f32,
    detune: [f32; 16],
}

impl SfPlayer {
    /// With the clips' mod-wheel and pitch-bend moves (in time order).
    pub fn with_controls(mut self, ctl: Vec<af27_core::midi::PlannedControl>) -> SfPlayer {
        self.ctl = ctl;
        self.seek = true;
        self
    }

    /// Sends the bend (the clip's plus each channel's note detune).
    fn send_bend(&mut self) {
        for ch in CHANNELS {
            let b = bend_value(self.detune[ch as usize] + self.bend);
            self.synth.process_midi_message(ch, 0xE0, b & 0x7F, b >> 7);
        }
    }

    fn apply(&mut self, c: af27_core::midi::PlannedControl) {
        use af27_core::midi::{MOD_WHEEL, PITCH_BEND};
        match c.controller {
            PITCH_BEND => {
                self.bend =
                    ((c.value.clamp(0.0, 1.0) * 16383.0 - 8192.0) / 8192.0).clamp(-1.0, 1.0) * 2.0;
                self.send_bend();
            }
            MOD_WHEEL => {
                let v = (c.value.clamp(0.0, 1.0) * 127.0).round() as i32;
                for ch in CHANNELS {
                    self.synth.process_midi_message(ch, 0xB0, 0x01, v);
                }
            }
            _ => {}
        }
    }

    pub fn new(sf: &SoundFontRef, mut notes: Vec<SfNote>, rate: u32) -> Result<SfPlayer, String> {
        let font = load(&sf.path)?;
        let synth = synthesizer(&font, rate, sf.bank, sf.program)
            .ok_or("The SoundFont cannot play at this sample rate.")?;
        notes.sort_by(|a, b| a.start.total_cmp(&b.start));
        let mut events: Vec<Event> = notes
            .iter()
            .enumerate()
            .flat_map(|(i, n)| {
                [
                    Event {
                        t: n.start,
                        on: true,
                        note: i as u32,
                    },
                    Event {
                        t: n.end.max(n.start),
                        on: false,
                        note: i as u32,
                    },
                ]
            })
            .collect();
        // Note-offs first at the same time, so a repeated note starts again.
        events.sort_by(|a, b| a.t.total_cmp(&b.t).then(a.on.cmp(&b.on)));
        Ok(SfPlayer {
            synth,
            notes,
            events,
            cursor: 0,
            next_t: f64::NAN,
            seek: true,
            gain: db_to_gain(sf.gain_db),
            sl: vec![0.0; BLOCK],
            sr: vec![0.0; BLOCK],
            ctl: Vec::new(),
            ctl_at: 0,
            bend: 0.0,
            detune: [0.0; 16],
        })
    }

    pub fn set_gain(&mut self, db: f32) {
        self.gain = db_to_gain(db);
    }

    /// After a jump: let every note go at the next block.
    pub fn reset(&mut self) {
        self.seek = true;
    }

    fn channel(i: u32) -> i32 {
        CHANNELS[i as usize % CHANNELS.len()]
    }

    fn start(&mut self, i: u32) {
        let n = self.notes[i as usize];
        let ch = Self::channel(i);
        self.synth
            .process_midi_message(ch, 0xB0, 0x0A, pan_cc(n.pan));
        self.detune[ch as usize] = n.detune;
        let bend = bend_value(n.detune + self.bend);
        self.synth
            .process_midi_message(ch, 0xE0, bend & 0x7F, bend >> 7);
        self.synth.note_on(ch, n.key as i32, n.velocity as i32);
    }

    fn stop(&mut self, i: u32) {
        let n = self.notes[i as usize];
        self.synth.note_off(Self::channel(i), n.key as i32);
    }

    /// Adds `n` frames from `t0` into `l` and `r` (n <= BLOCK).
    pub fn render(&mut self, t0: f64, dt: f64, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(BLOCK);
        if self.seek || (t0 - self.next_t).abs() > dt * 0.5 || self.next_t.is_nan() {
            self.seek = false;
            self.synth.note_off_all(true);
            self.cursor = self.events.partition_point(|e| e.t < t0);
            // Controllers as they stand here.
            self.ctl_at = self.ctl.partition_point(|e| e.t < t0);
            if !self.ctl.is_empty() {
                use af27_core::midi::{PlannedControl, MOD_WHEEL, PITCH_BEND};
                let before = &self.ctl[..self.ctl_at];
                let last = |c: u8, v: f32| {
                    before
                        .iter()
                        .rev()
                        .find(|e| e.controller == c)
                        .copied()
                        .unwrap_or(PlannedControl {
                            t: t0,
                            controller: c,
                            value: v,
                        })
                };
                let (w, b) = (last(MOD_WHEEL, 0.0), last(PITCH_BEND, 0.5));
                self.apply(w);
                self.apply(b);
            }
            // Notes already sounding here start now.
            for i in 0..self.notes.len() {
                let x = self.notes[i];
                if x.start >= t0 {
                    break;
                }
                if x.end > t0 {
                    self.start(i as u32);
                }
            }
        }
        let t1 = t0 + dt * n as f64;
        let mut done = 0usize;
        loop {
            let e = self.events.get(self.cursor).copied().filter(|e| e.t < t1);
            let c = self.ctl.get(self.ctl_at).copied().filter(|c| c.t < t1);
            // Whichever comes first (a controller before a note at the
            // same time).
            let te = match (e, c) {
                (None, None) => break,
                (Some(e), Some(c)) => e.t.min(c.t),
                (Some(e), None) => e.t,
                (None, Some(c)) => c.t,
            };
            let at = (((te - t0) / dt).floor().max(0.0) as usize).min(n);
            if at > done {
                self.synth
                    .render(&mut self.sl[done..at], &mut self.sr[done..at]);
                done = at;
            }
            match (e, c) {
                (_, Some(c)) if c.t <= te => {
                    self.apply(c);
                    self.ctl_at += 1;
                }
                (Some(e), _) => {
                    if e.on {
                        self.start(e.note);
                    } else {
                        self.stop(e.note);
                    }
                    self.cursor += 1;
                }
                _ => break,
            }
        }
        if n > done {
            self.synth
                .render(&mut self.sl[done..n], &mut self.sr[done..n]);
        }
        for i in 0..n {
            l[i] += self.sl[i] * self.gain;
            r[i] += self.sr[i] * self.gain;
        }
        self.next_t = t1;
    }
}

/// Notes played by hand (the piano roll, a MIDI keyboard), through a
/// track's instrument, whether or not the transport runs. The sustain
/// and sostenuto pedals are kept here, the same for either instrument:
/// a key let go while the sustain pedal is down sounds on until it comes
/// up; the sostenuto pedal holds just the keys down when it went down.
pub struct Audition {
    kind: AuditionKind,
    t: f64,
    l: Vec<f32>,
    r: Vec<f32>,
    pedals: Pedals,
}

enum AuditionKind {
    Synth(crate::synth::Synth),
    Font(Box<Synthesizer>, f32),
}

/// Notes the built-in synth's audition can hold at once.
const AUDITION_NOTES: usize = 32;

/// The pedals and which keys they hold.
#[derive(Debug, Clone, Copy)]
struct Pedals {
    sustain: bool,
    sostenuto: bool,
    /// Keys down.
    down: [bool; 128],
    /// Let go, still sounding for a pedal.
    held: [bool; 128],
    /// Caught by the sostenuto pedal.
    caught: [bool; 128],
}

impl Default for Pedals {
    fn default() -> Self {
        Pedals {
            sustain: false,
            sostenuto: false,
            down: [false; 128],
            held: [false; 128],
            caught: [false; 128],
        }
    }
}

impl Audition {
    /// Through the built-in synth.
    pub fn synth(inst: af27_core::midi::Instrument, rate: u32) -> Audition {
        let mut s = crate::synth::Synth::live(inst, rate);
        s.notes.reserve(AUDITION_NOTES);
        Audition {
            kind: AuditionKind::Synth(s),
            t: 0.0,
            l: vec![0.0; BLOCK],
            r: vec![0.0; BLOCK],
            pedals: Pedals::default(),
        }
    }

    /// Through a SoundFont.
    pub fn font(sf: &SoundFontRef, rate: u32) -> Result<Audition, String> {
        let font = load(&sf.path)?;
        let synth = synthesizer(&font, rate, sf.bank, sf.program)
            .ok_or("The SoundFont cannot play at this sample rate.")?;
        Ok(Audition {
            kind: AuditionKind::Font(Box::new(synth), db_to_gain(sf.gain_db)),
            t: 0.0,
            l: vec![0.0; BLOCK],
            r: vec![0.0; BLOCK],
            pedals: Pedals::default(),
        })
    }

    /// New settings for the built-in synth, live (held notes carry on).
    pub fn set_instrument(&mut self, inst: af27_core::midi::Instrument) {
        if let AuditionKind::Synth(s) = &mut self.kind {
            s.set(inst);
        }
    }

    /// Whether this plays the built-in synth.
    pub fn is_synth(&self) -> bool {
        matches!(self.kind, AuditionKind::Synth(_))
    }

    fn start(&mut self, pitch: u8, velocity: u8, pan: f32) {
        match &mut self.kind {
            AuditionKind::Synth(s) => s.key_on(self.t, pitch, velocity, pan),
            AuditionKind::Font(s, _) => {
                s.process_midi_message(0, 0xB0, 0x0A, pan_cc(pan));
                s.note_on(0, pitch as i32, velocity as i32);
            }
        }
    }

    fn stop(&mut self, pitch: u8) {
        match &mut self.kind {
            AuditionKind::Synth(s) => s.key_off(self.t, pitch),
            AuditionKind::Font(s, _) => s.note_off(0, pitch as i32),
        }
    }

    pub fn note_on(&mut self, pitch: u8, velocity: u8, pan: f32) {
        let k = (pitch & 0x7f) as usize;
        // Struck again while a pedal holds it: the old one ends first.
        if self.pedals.held[k] {
            self.pedals.held[k] = false;
            self.stop(pitch);
        }
        self.pedals.down[k] = true;
        self.start(pitch, velocity, pan);
    }

    pub fn note_off(&mut self, pitch: u8) {
        let k = (pitch & 0x7f) as usize;
        let p = &mut self.pedals;
        if !p.down[k] && !p.held[k] {
            // Not ours (or already let go): make sure it stops.
            self.stop(pitch);
            return;
        }
        p.down[k] = false;
        if p.sustain || (p.sostenuto && p.caught[k]) {
            p.held[k] = true;
        } else {
            self.stop(pitch);
        }
    }

    /// A controller from a player: 64 sustain, 66 sostenuto, 1 the mod
    /// wheel, pitch bend; value 0..1.
    pub fn control(&mut self, controller: u8, value: f32) {
        use af27_core::midi::{MOD_WHEEL, PITCH_BEND, SOSTENUTO, SUSTAIN};
        let down = value >= 0.5;
        match controller {
            SUSTAIN => {
                self.pedals.sustain = down;
                if !down {
                    for k in 0..128 {
                        let p = &self.pedals;
                        if p.held[k] && !(p.sostenuto && p.caught[k]) {
                            self.pedals.held[k] = false;
                            self.stop(k as u8);
                        }
                    }
                }
            }
            SOSTENUTO => {
                let p = &mut self.pedals;
                if down && !p.sostenuto {
                    p.caught = p.down;
                } else if !down {
                    for k in 0..128 {
                        let p = &mut self.pedals;
                        let was = p.caught[k];
                        p.caught[k] = false;
                        if was && p.held[k] && !p.sustain {
                            p.held[k] = false;
                            self.stop(k as u8);
                        }
                    }
                }
                self.pedals.sostenuto = down;
            }
            MOD_WHEEL => match &mut self.kind {
                AuditionKind::Synth(s) => s.set_wheel(value),
                AuditionKind::Font(s, _) => s.process_midi_message(
                    0,
                    0xB0,
                    0x01,
                    (value.clamp(0.0, 1.0) * 127.0).round() as i32,
                ),
            },
            PITCH_BEND => match &mut self.kind {
                AuditionKind::Synth(s) => s.set_bend(value),
                AuditionKind::Font(s, _) => {
                    let b = (value.clamp(0.0, 1.0) * 16383.0).round() as i32;
                    s.process_midi_message(0, 0xE0, b & 0x7F, b >> 7);
                }
            },
            _ => {}
        }
    }

    pub fn all_off(&mut self) {
        let wheel_bend = |k: &mut AuditionKind| match k {
            AuditionKind::Synth(s) => {
                s.set_bend(0.5);
                s.set_wheel(0.0);
            }
            AuditionKind::Font(s, _) => {
                s.process_midi_message(0, 0xE0, 0, 64);
                s.process_midi_message(0, 0xB0, 0x01, 0);
            }
        };
        wheel_bend(&mut self.kind);
        self.pedals = Pedals::default();
        match &mut self.kind {
            AuditionKind::Synth(s) => s.keys_off(self.t),
            AuditionKind::Font(s, _) => s.note_off_all(false),
        }
    }

    /// Adds the sound into interleaved `out` (`channels` wide).
    pub fn render(&mut self, out: &mut [f32], channels: usize, dt: f64) {
        let channels = channels.max(1);
        let frames = out.len() / channels;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(BLOCK);
            let (l, r) = (&mut self.l[..n], &mut self.r[..n]);
            match &mut self.kind {
                AuditionKind::Synth(s) => {
                    l.fill(0.0);
                    r.fill(0.0);
                    let rel = s.release();
                    let t = self.t;
                    s.notes.retain(|x| x.end + rel > t);
                    s.render(self.t, dt, l, r);
                }
                AuditionKind::Font(s, g) => {
                    s.render(l, r);
                    for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                        *a *= *g;
                        *b *= *g;
                    }
                }
            }
            for i in 0..n {
                let o = &mut out[(done + i) * channels..(done + i + 1) * channels];
                if channels == 1 {
                    o[0] += (self.l[i] + self.r[i]) * 0.5;
                } else {
                    o[0] += self.l[i];
                    o[1] += self.r[i];
                }
            }
            self.t += dt * n as f64;
            done += n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audition_synth_holds_and_lets_go_without_growing() {
        let mut a = Audition::synth(af27_core::midi::Instrument::default(), 48_000);
        let cap = match &a.kind {
            AuditionKind::Synth(s) => s.notes.capacity(),
            _ => unreachable!(),
        };
        let mut out = vec![0.0f32; 4800 * 2];
        a.note_on(60, 100, 0.0);
        a.render(&mut out, 2, 1.0 / 48_000.0);
        assert!(out.iter().any(|v| v.abs() > 0.01));
        a.note_off(60);
        for _ in 0..20 {
            out.fill(0.0);
            a.render(&mut out, 2, 1.0 / 48_000.0);
        }
        assert!(out.iter().all(|v| v.abs() < 1e-6));
        for p in 0..100 {
            a.note_on(p, 100, 0.0);
        }
        match &a.kind {
            AuditionKind::Synth(s) => assert_eq!(s.notes.capacity(), cap),
            _ => unreachable!(),
        }
    }

    fn level(a: &mut Audition, secs: f64) -> f32 {
        let mut out = vec![0.0f32; 480 * 2];
        let mut peak = 0.0f32;
        for _ in 0..(secs * 100.0) as usize {
            out.fill(0.0);
            a.render(&mut out, 2, 1.0 / 48_000.0);
            peak = out.iter().fold(0.0, |m, v| m.max(v.abs()));
        }
        // The level over the last 10 ms.
        peak
    }

    #[test]
    fn the_sustain_pedal_holds_live_notes_until_it_lifts() {
        use af27_core::synth_model::SynthModel;
        for inst in [
            af27_core::midi::Instrument::default(),
            SynthModel::PolyChorus.preset(),
            SynthModel::MonoLadder.preset(),
        ] {
            let mut a = Audition::synth(inst, 48_000);
            a.note_on(60, 100, 0.0);
            assert!(level(&mut a, 0.1) > 0.01);
            a.control(64, 1.0);
            a.note_off(60);
            // Key up, pedal down: still sounding a second later.
            assert!(level(&mut a, 1.0) > 0.01, "{:?}", inst.ext.model);
            a.control(64, 0.0);
            assert!(level(&mut a, 1.5) < 1e-4, "{:?}", inst.ext.model);
            // Without the pedal a key lets go at once.
            a.note_on(62, 100, 0.0);
            level(&mut a, 0.1);
            a.note_off(62);
            assert!(level(&mut a, 1.5) < 1e-4);
        }
    }

    #[test]
    fn sostenuto_holds_only_keys_down_when_it_went_down() {
        let mut a = Audition::synth(af27_core::midi::Instrument::default(), 48_000);
        a.note_on(60, 100, 0.0);
        a.control(66, 1.0);
        a.note_on(64, 100, 0.0);
        level(&mut a, 0.05);
        a.note_off(60);
        a.note_off(64);
        level(&mut a, 0.05);
        let t = a.t;
        let sounding = |a: &Audition, p: u8| match &a.kind {
            AuditionKind::Synth(s) => s.notes.iter().any(|n| n.pitch == p && n.end > t),
            _ => unreachable!(),
        };
        assert!(sounding(&a, 60));
        assert!(!sounding(&a, 64));
        a.control(66, 0.0);
        assert!(!sounding(&a, 60));
        assert!(level(&mut a, 1.0) < 1e-4);
    }

    #[test]
    fn a_new_instrument_keeps_held_live_notes() {
        let mut a = Audition::synth(af27_core::midi::Instrument::default(), 48_000);
        a.note_on(60, 100, 0.0);
        level(&mut a, 0.1);
        // Into the full engine and back while the key is held.
        a.set_instrument(af27_core::synth_model::SynthModel::BrassPoly.preset());
        assert!(level(&mut a, 0.3) > 0.01);
        a.set_instrument(af27_core::midi::Instrument::default());
        assert!(level(&mut a, 0.3) > 0.01);
        a.note_off(60);
        assert!(level(&mut a, 1.0) < 1e-4);
    }

    /// Plays a real font when `AF27_SF2` names one:
    /// `cargo test -p af27-engine soundfont -- --ignored`.
    #[test]
    #[ignore]
    fn plays_a_real_soundfont() {
        let Some(p) = std::env::var_os("AF27_SF2") else {
            return;
        };
        let sf = SoundFontRef {
            path: p.into(),
            bank: 0,
            program: 0,
            preset_name: String::new(),
            gain_db: 0.0,
        };
        let notes = vec![SfNote {
            start: 0.1,
            end: 0.5,
            key: 60,
            velocity: 100,
            pan: 0.0,
            detune: 0.0,
        }];
        let mut pl = SfPlayer::new(&sf, notes, 48_000).unwrap();
        let mut l = vec![0.0; 1024];
        let mut r = vec![0.0; 1024];
        let mut heard = false;
        let mut t = 0.0;
        for _ in 0..40 {
            l.fill(0.0);
            r.fill(0.0);
            pl.render(t, 1.0 / 48_000.0, &mut l, &mut r);
            heard |= l.iter().any(|v| v.abs() > 1e-3);
            t += 1024.0 / 48_000.0;
        }
        assert!(heard);
        assert!(!presets(&load(&sf.path).unwrap()).is_empty());
    }
}
