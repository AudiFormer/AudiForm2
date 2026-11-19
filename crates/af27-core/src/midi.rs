// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! MIDI data: the session tempo, MIDI clips and their notes (in beats, so
//! they follow tempo changes), and the built-in instrument's settings.

use serde::{Deserialize, Serialize};

use crate::ClipId;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tempo {
    pub bpm: f64,
    pub beats_per_bar: u32,
    /// Click on every beat while playing or recording.
    pub metronome: bool,
}

impl Default for Tempo {
    fn default() -> Self {
        Tempo {
            bpm: 120.0,
            beats_per_bar: 4,
            metronome: false,
        }
    }
}

impl Tempo {
    pub fn secs(&self, beats: f64) -> f64 {
        beats * 60.0 / self.bpm.max(1.0)
    }
    pub fn beats(&self, secs: f64) -> f64 {
        secs * self.bpm.max(1.0) / 60.0
    }
    /// "bar.beat.tick" (960 ticks per beat), bars and beats from 1.
    pub fn format(&self, secs: f64) -> String {
        let b = self.beats(secs.max(0.0));
        let bpb = self.beats_per_bar.max(1) as f64;
        let bar = (b / bpb).floor();
        let beat = (b - bar * bpb).floor();
        let tick = ((b - b.floor()) * 960.0).floor();
        format!("{}.{}.{:03}", bar as u64 + 1, beat as u64 + 1, tick as u64)
    }
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
    /// The object's id in an `.audfrm` file it came from, so saving back
    /// keeps everything Audiform27 does not use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext_id: Option<String>,
}

impl MidiClip {
    pub fn end(&self) -> f64 {
        self.start + self.length
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

    #[test]
    fn tempo_and_names() {
        let t = Tempo::default();
        assert_eq!(t.secs(4.0), 2.0);
        assert_eq!(t.format(2.0), "2.1.000");
        assert_eq!(t.format(0.75), "1.2.480");
        assert_eq!(note_name(60), "C4");
        assert!((note_freq(69.0) - 440.0).abs() < 1e-3);
    }
}
