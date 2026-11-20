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

/// One note in seconds.
#[derive(Debug, Clone, Copy)]
pub struct SfNote {
    pub start: f64,
    pub end: f64,
    pub key: u8,
    pub velocity: u8,
    pub pan: f32,
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
}

impl SfPlayer {
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
        while let Some(e) = self.events.get(self.cursor).copied() {
            if e.t >= t1 {
                break;
            }
            let at = (((e.t - t0) / dt).floor().max(0.0) as usize).min(n);
            if at > done {
                self.synth
                    .render(&mut self.sl[done..at], &mut self.sr[done..at]);
                done = at;
            }
            if e.on {
                self.start(e.note);
            } else {
                self.stop(e.note);
            }
            self.cursor += 1;
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

/// Notes played by hand (the piano roll), through a track's instrument,
/// whether or not the transport runs.
pub struct Audition {
    kind: AuditionKind,
    t: f64,
    l: Vec<f32>,
    r: Vec<f32>,
}

enum AuditionKind {
    Synth(crate::synth::Synth),
    Font(Box<Synthesizer>, f32),
}

/// Notes the built-in synth's audition can hold at once.
const AUDITION_NOTES: usize = 32;

impl Audition {
    /// Through the built-in synth.
    pub fn synth(inst: af27_core::midi::Instrument, rate: u32) -> Audition {
        let mut s = crate::synth::Synth::new(inst, Vec::new(), rate);
        s.notes.reserve(AUDITION_NOTES);
        Audition {
            kind: AuditionKind::Synth(s),
            t: 0.0,
            l: vec![0.0; BLOCK],
            r: vec![0.0; BLOCK],
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
        })
    }

    pub fn note_on(&mut self, pitch: u8, velocity: u8, pan: f32) {
        match &mut self.kind {
            AuditionKind::Synth(s) => {
                if s.notes.len() < s.notes.capacity() {
                    let t = self.t;
                    s.notes
                        .push(crate::synth::note(t, t + 3600.0, pitch, velocity, pan));
                }
            }
            AuditionKind::Font(s, _) => {
                s.process_midi_message(0, 0xB0, 0x0A, pan_cc(pan));
                s.note_on(0, pitch as i32, velocity as i32);
            }
        }
    }

    pub fn note_off(&mut self, pitch: u8) {
        match &mut self.kind {
            AuditionKind::Synth(s) => {
                let f = af27_core::midi::note_freq(pitch as f32);
                let t = self.t;
                for n in s.notes.iter_mut().filter(|n| n.freq == f && n.end > t) {
                    n.end = t;
                }
            }
            AuditionKind::Font(s, _) => s.note_off(0, pitch as i32),
        }
    }

    pub fn all_off(&mut self) {
        match &mut self.kind {
            AuditionKind::Synth(s) => {
                let t = self.t;
                for n in s.notes.iter_mut().filter(|n| n.end > t) {
                    n.end = t;
                }
            }
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
                    let rel = s.inst.release.max(0.001) as f64;
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
