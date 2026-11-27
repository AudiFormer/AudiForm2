// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The built-in synth's full set of controls, and its models.
//!
//! [`crate::midi::Instrument`] keeps its first nine fields (wave, detune,
//! amp ADSR, cutoff, resonance, gain) as they always were; everything a
//! real synth adds lives in [`SynthControls`], saved only when it is not
//! the default, so older sessions read, sound and save exactly as before.
//!
//! A model is a starting point: picking one sets every control. Their
//! names are our own; each is only inspired by a kind of classic
//! instrument.

use serde::{Deserialize, Serialize};

use crate::midi::{Instrument, Wave};

/// The synth's models (presets that set every control).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SynthModel {
    /// The original tone, unchanged.
    #[default]
    Default,
    /// A fat monophonic bass and lead through a driven 4-pole ladder.
    MonoLadder,
    /// A warm 80s polysynth: saw, pulse with slow width sweep, sub and chorus.
    PolyChorus,
    /// A squelchy acid bass: resonant ladder, accents on loud notes,
    /// slides between overlapping notes.
    AcidLine,
    /// A bright brassy polysynth with a swelling filter.
    BrassPoly,
    /// A 2-operator FM electric piano.
    FmKeys,
    /// A string ensemble: stacked saws and a deep ensemble chorus.
    StringMachine,
    /// A hard-sync lead whose second oscillator sweeps on each note.
    SyncLead,
    // Work-in-progress models matched from reference recordings. They are
    // named after where each was heard so they are easy to tell apart
    // while being tuned; rename them before release.
    /// A bright quarter-width pulse with its fundamental reinforced by a
    /// sine, gently chorused.
    AtlantisVerseLead,
    /// A wide, stacked saw-and-pulse poly with a little glide.
    AtlantisChorusPoly,
    /// Two beating sines with a faint partial three octaves up that
    /// fades as the note decays.
    WildHorsesDemoPad,
    /// A glassy lead: a sine plus an inharmonic partial about 5.37 times
    /// the note, slow to swell.
    WildHorsesIntroLead,
    /// A low buzzing drone, band-passed, pulsed by a square LFO and
    /// bit-crushed.
    WildHorsesCrushDrone,
    /// A crystal bell lead: a bright FM attack over a sine and a glassy
    /// inharmonic partial.
    IceQueen,
}

impl SynthModel {
    pub const ALL: [SynthModel; 14] = [
        SynthModel::Default,
        SynthModel::MonoLadder,
        SynthModel::PolyChorus,
        SynthModel::AcidLine,
        SynthModel::BrassPoly,
        SynthModel::FmKeys,
        SynthModel::StringMachine,
        SynthModel::SyncLead,
        SynthModel::AtlantisVerseLead,
        SynthModel::AtlantisChorusPoly,
        SynthModel::WildHorsesDemoPad,
        SynthModel::WildHorsesIntroLead,
        SynthModel::WildHorsesCrushDrone,
        SynthModel::IceQueen,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SynthModel::Default => "Default",
            SynthModel::MonoLadder => "Mono Ladder",
            SynthModel::PolyChorus => "Poly Chorus",
            SynthModel::AcidLine => "Acid Line",
            SynthModel::BrassPoly => "Brass Poly",
            SynthModel::FmKeys => "FM Keys",
            SynthModel::StringMachine => "String Machine",
            SynthModel::SyncLead => "Sync Lead",
            SynthModel::AtlantisVerseLead => "Atlantis Verse Lead (WIP)",
            SynthModel::AtlantisChorusPoly => "Atlantis Chorus Poly (WIP)",
            SynthModel::WildHorsesDemoPad => "Wild Horses Demo Pad (WIP)",
            SynthModel::WildHorsesIntroLead => "Wild Horses Intro Lead (WIP)",
            SynthModel::WildHorsesCrushDrone => "Wild Horses Crush Drone (WIP)",
            SynthModel::IceQueen => "Ice Queen (WIP)",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            SynthModel::Default => "The original built-in tone: two detuned saws, gentle low-pass",
            SynthModel::MonoLadder => {
                "Fat mono bass and lead through a driven 4-pole ladder filter"
            }
            SynthModel::PolyChorus => {
                "Warm 80s poly: saw, swept pulse and sub through a lush chorus"
            }
            SynthModel::AcidLine => {
                "Squelchy acid bass: loud notes accent, overlapping notes slide"
            }
            SynthModel::BrassPoly => "Bright brassy poly with a filter that swells on each chord",
            SynthModel::FmKeys => "2-operator FM electric piano: bright bell attack, mellow body",
            SynthModel::StringMachine => "String ensemble: stacked saws in a deep ensemble chorus",
            SynthModel::SyncLead => "Hard-sync lead whose second oscillator sweeps on every note",
            SynthModel::AtlantisVerseLead => {
                "Bright 25% pulse with a sine under it, open filter, phasey chorus"
            }
            SynthModel::AtlantisChorusPoly => {
                "Wide stacked saw and pulse poly with a touch of glide"
            }
            SynthModel::WildHorsesDemoPad => {
                "Beating sines with a faint high partial that fades as notes decay"
            }
            SynthModel::WildHorsesIntroLead => {
                "Glassy lead: sine plus an inharmonic partial, slow swell"
            }
            SynthModel::WildHorsesCrushDrone => {
                "Low band-passed buzz, pulsed in eighths and bit-crushed"
            }
            SynthModel::IceQueen => "Crystal bell lead: bright FM strike over a glassy partial",
        }
    }

    /// The instrument this model starts from.
    pub fn preset(self) -> Instrument {
        let mut i = Instrument::default();
        let x = &mut i.ext;
        x.model = self;
        match self {
            SynthModel::Default => {}
            SynthModel::MonoLadder => {
                i.wave = Wave::Saw;
                i.detune = 6.0;
                i.attack = 0.003;
                i.decay = 0.3;
                i.sustain = 0.85;
                i.release = 0.15;
                i.cutoff = 700.0;
                i.resonance = 0.35;
                i.gain_db = -9.0;
                x.osc2_wave = Some(Wave::Square);
                x.osc2_octave = -1;
                x.osc2_level = 0.8;
                x.sub = 0.3;
                x.filter_kind = FilterKind::Ladder;
                x.drive = 0.45;
                x.key_track = 0.5;
                x.filter_env = 3.2;
                x.f_attack = 0.002;
                x.f_decay = 0.35;
                x.f_sustain = 0.25;
                x.f_release = 0.2;
                x.mode = VoiceMode::Mono;
                x.glide = 0.06;
                x.velocity = 0.5;
            }
            SynthModel::PolyChorus => {
                i.wave = Wave::Saw;
                i.detune = 0.0;
                i.attack = 0.01;
                i.decay = 0.6;
                i.sustain = 0.8;
                i.release = 0.5;
                i.cutoff = 2200.0;
                i.resonance = 0.2;
                i.gain_db = -12.0;
                x.osc2_wave = Some(Wave::Pulse);
                x.osc2_level = 0.8;
                x.pw = 0.5;
                x.sub = 0.35;
                x.filter_env = 1.5;
                x.f_attack = 0.005;
                x.f_decay = 0.7;
                x.f_sustain = 0.35;
                x.f_release = 0.5;
                x.lfo_shape = LfoShape::Triangle;
                x.lfo_rate = 0.8;
                x.lfo_depth = 0.7;
                // Vibrato at 30% of the shared default (0.5 st): the warble
                // was too strong. The chorus stays as it was.
                x.lfo_pitch = 0.15;
                x.lfo_pwm = 0.3;
                x.voices = 8;
                x.velocity = 0.6;
                x.chorus = 0.6;
                x.chorus_rate = 0.5;
                x.chorus_depth = 0.5;
            }
            SynthModel::AcidLine => {
                i.wave = Wave::Saw;
                i.detune = 0.0;
                i.attack = 0.002;
                i.decay = 1.2;
                i.sustain = 0.55;
                i.release = 0.06;
                i.cutoff = 320.0;
                i.resonance = 0.82;
                i.gain_db = -10.0;
                x.osc2_level = 0.0;
                x.filter_kind = FilterKind::Ladder;
                x.drive = 0.5;
                x.key_track = 0.3;
                x.filter_env = 4.0;
                x.f_attack = 0.001;
                x.f_decay = 0.2;
                x.f_sustain = 0.0;
                x.f_release = 0.1;
                x.mode = VoiceMode::Legato;
                x.glide = 0.06;
                x.accent = 0.7;
                x.velocity = 0.3;
            }
            SynthModel::BrassPoly => {
                i.wave = Wave::Saw;
                i.detune = 9.0;
                i.attack = 0.05;
                i.decay = 0.3;
                i.sustain = 0.85;
                i.release = 0.3;
                i.cutoff = 650.0;
                i.resonance = 0.15;
                i.gain_db = -13.0;
                x.filter_kind = FilterKind::Ladder;
                x.drive = 0.15;
                x.filter_env = 3.0;
                x.vel_filter = 1.0;
                x.f_attack = 0.09;
                x.f_decay = 0.5;
                x.f_sustain = 0.5;
                x.f_release = 0.3;
                x.unison = 2;
                x.spread = 12.0;
                x.width = 0.6;
                x.voices = 8;
                x.velocity = 0.6;
            }
            SynthModel::FmKeys => {
                i.wave = Wave::Sine;
                i.detune = 3.0;
                i.attack = 0.002;
                i.decay = 1.8;
                i.sustain = 0.2;
                i.release = 0.5;
                i.cutoff = 16_000.0;
                i.resonance = 0.0;
                i.gain_db = -7.0;
                x.osc2_wave = Some(Wave::Sine);
                x.osc2_octave = 1;
                x.osc2_level = 0.25;
                x.fm = 1.8;
                x.fm_ratio = 1.0;
                x.fm_decay = 0.7;
                x.velocity = 0.8;
                x.chorus = 0.25;
                x.chorus_rate = 0.6;
                x.chorus_depth = 0.35;
            }
            SynthModel::StringMachine => {
                i.wave = Wave::Saw;
                i.detune = 4.0;
                i.attack = 0.35;
                i.decay = 0.5;
                i.sustain = 1.0;
                i.release = 0.9;
                i.cutoff = 4500.0;
                i.resonance = 0.05;
                i.gain_db = -13.0;
                x.osc2_octave = 1;
                x.osc2_level = 0.6;
                x.velocity = 0.2;
                x.chorus = 1.0;
                x.chorus_rate = 0.7;
                x.chorus_depth = 0.9;
            }
            SynthModel::SyncLead => {
                i.wave = Wave::Saw;
                i.detune = 0.0;
                i.attack = 0.003;
                i.decay = 0.3;
                i.sustain = 0.9;
                i.release = 0.2;
                i.cutoff = 5000.0;
                i.resonance = 0.25;
                i.gain_db = -13.0;
                x.osc1_level = 0.35;
                x.osc2_semi = 7;
                x.osc2_level = 1.0;
                x.sync = true;
                x.osc2_env = 14.0;
                x.filter_env = 1.0;
                x.f_attack = 0.002;
                x.f_decay = 0.6;
                x.f_sustain = 0.2;
                x.f_release = 0.3;
                x.mode = VoiceMode::Mono;
                x.glide = 0.04;
                x.velocity = 0.5;
            }
            SynthModel::AtlantisVerseLead => {
                // Measured: every harmonic but each fourth (a 25% pulse),
                // the fundamental about 12 dB over the rest, all of them
                // rising together in about 90 ms (no filter sweep).
                i.wave = Wave::Pulse;
                i.detune = 0.0;
                i.attack = 0.08;
                i.decay = 0.6;
                i.sustain = 0.8;
                i.release = 0.35;
                // A resonant peak riding about the 10th harmonic lifts the
                // upper partials as in the recording, then rolls off.
                i.cutoff = 1900.0;
                i.resonance = 0.8;
                i.gain_db = -12.0;
                x.key_track = 1.0;
                x.pw = 0.25;
                x.osc2_wave = Some(Wave::Sine);
                x.osc2_level = 0.45;
                x.voices = 8;
                x.velocity = 0.4;
                x.chorus = 0.55;
                x.chorus_rate = 0.4;
                x.chorus_depth = 0.45;
            }
            SynthModel::AtlantisChorusPoly => {
                // Measured: a full harmonic series well past the tenth,
                // mostly in the sides (wide), notes leaning into pitch.
                i.wave = Wave::Saw;
                i.detune = 6.0;
                i.attack = 0.12;
                i.decay = 0.8;
                i.sustain = 0.85;
                i.release = 0.5;
                i.cutoff = 5000.0;
                i.resonance = 0.1;
                i.gain_db = -15.0;
                x.osc2_wave = Some(Wave::Pulse);
                x.osc2_level = 0.7;
                x.pw = 0.4;
                x.key_track = 0.3;
                x.unison = 3;
                x.spread = 14.0;
                x.width = 0.9;
                x.glide = 0.05;
                x.voices = 8;
                x.velocity = 0.5;
                x.chorus = 0.5;
                x.chorus_rate = 0.5;
                x.chorus_depth = 0.5;
            }
            SynthModel::WildHorsesDemoPad => {
                // Measured: nearly a pure sine, the 2nd harmonic 11-23 dB
                // down, the 8th 21-26 dB down and dying faster than the
                // note; the note beats about 3.4 times a second (two
                // copies some 10 cents apart); 60 ms in, ~3 s to fade.
                i.wave = Wave::Sine;
                i.detune = 0.0;
                i.attack = 0.05;
                i.decay = 3.5;
                i.sustain = 0.25;
                i.release = 0.6;
                i.cutoff = 2000.0;
                i.resonance = 0.0;
                i.gain_db = -8.0;
                x.osc2_wave = Some(Wave::Sine);
                x.osc2_octave = 3;
                x.osc2_level = 0.09;
                x.fm = 0.5;
                x.fm_ratio = 1.0;
                x.fm_decay = 2.5;
                x.key_track = 1.0;
                x.filter_env = 1.3;
                x.f_attack = 0.005;
                x.f_decay = 2.5;
                x.f_sustain = 0.0;
                x.f_release = 0.6;
                x.unison = 2;
                x.spread = 10.0;
                x.width = 0.25;
                x.voices = 8;
                x.velocity = 0.5;
                x.chorus = 0.2;
                x.chorus_rate = 0.4;
                x.chorus_depth = 0.3;
            }
            SynthModel::WildHorsesIntroLead => {
                // Measured: a sine with a partial at 5.37 times it (not a
                // harmonic: a glassy, bell-like edge) about 9 dB down and
                // the 2nd harmonic about 19 dB down; ~250 ms to swell.
                // Oscillator 2 sits 2 octaves, 5 semitones and 12 cents up
                // (x5.37).
                i.wave = Wave::Sine;
                i.detune = 12.0;
                i.attack = 0.22;
                i.decay = 0.5;
                i.sustain = 0.85;
                i.release = 0.35;
                i.cutoff = 16_000.0;
                i.resonance = 0.0;
                i.gain_db = -9.0;
                x.osc2_wave = Some(Wave::Sine);
                x.osc2_octave = 2;
                x.osc2_semi = 5;
                x.osc2_level = 0.35;
                x.fm = 0.25;
                x.fm_ratio = 1.0;
                x.fm_decay = 5.0;
                x.voices = 8;
                x.velocity = 0.4;
                x.chorus = 0.4;
                x.chorus_rate = 0.6;
                x.chorus_depth = 0.35;
            }
            SynthModel::WildHorsesCrushDrone => {
                // Measured: a low drone whose 4th to 12th harmonics carry
                // it (a band round 300-650 Hz), pulsing about 2.8 times a
                // second (eighths at 83 BPM), with a gritty, crushed edge.
                i.wave = Wave::Saw;
                i.detune = 0.0;
                i.attack = 0.01;
                i.decay = 0.3;
                i.sustain = 1.0;
                i.release = 0.25;
                i.cutoff = 450.0;
                i.resonance = 0.35;
                i.gain_db = -8.0;
                x.osc2_wave = Some(Wave::Square);
                x.osc2_octave = -1;
                x.osc2_level = 0.6;
                x.filter_type = FilterType::BandPass;
                x.lfo_shape = LfoShape::Square;
                x.lfo_rate = 2.78;
                x.lfo_depth = 1.0;
                x.lfo_pitch = 0.0;
                x.lfo_filter = 1.0;
                x.voices = 8;
                x.velocity = 0.3;
                x.crush_bits = 6.0;
                x.crush_down = 6.0;
            }
            SynthModel::IceQueen => {
                // A sine with the same glassy x5.37 partial, struck
                // brighter: an FM edge that rings off over a second.
                i.wave = Wave::Sine;
                i.detune = 12.0;
                i.attack = 0.008;
                i.decay = 1.2;
                i.sustain = 0.6;
                i.release = 0.5;
                i.cutoff = 16_000.0;
                i.resonance = 0.0;
                i.gain_db = -9.0;
                x.osc2_wave = Some(Wave::Sine);
                x.osc2_octave = 2;
                x.osc2_semi = 5;
                x.osc2_level = 0.45;
                x.fm = 1.2;
                x.fm_ratio = 1.0;
                x.fm_decay = 0.8;
                x.voices = 8;
                x.velocity = 0.6;
                x.chorus = 0.3;
                x.chorus_rate = 0.5;
                x.chorus_depth = 0.3;
            }
        }
        i
    }
}

/// The filter's response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FilterType {
    #[default]
    LowPass,
    HighPass,
    BandPass,
}

/// How the filter is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FilterKind {
    /// The original 2-pole (12 dB/oct) state-variable filter.
    #[default]
    Classic,
    /// A 4-pole (24 dB/oct) ladder with drive.
    Ladder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LfoShape {
    #[default]
    Sine,
    Triangle,
    Square,
    Saw,
    /// A new random level each cycle.
    SampleHold,
}

/// How notes take voices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VoiceMode {
    /// Every note its own voice (up to the voice count).
    #[default]
    Poly,
    /// One voice; each note starts the envelopes again, glide always.
    Mono,
    /// One voice; a note played while another is held keeps the
    /// envelopes going and glides to it (slide); others start afresh.
    Legato,
}

/// Most voices the synth plays at once, and most unison voices per note.
pub const MAX_VOICES: u8 = 16;
pub const MAX_UNISON: u8 = 5;

/// Everything the built-in synth has beyond its original controls. The
/// default is the original tone.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SynthControls {
    /// The model these settings started from (a label; the sound is the
    /// controls).
    pub model: SynthModel,
    // Oscillator 1 (its wave is `Instrument::wave`).
    pub osc1_octave: i8,
    pub osc1_semi: i8,
    /// Cents.
    pub osc1_fine: f32,
    /// 0..1.
    pub osc1_level: f32,
    // Oscillator 2 (its fine tune is `Instrument::detune`).
    /// None: the same wave as oscillator 1.
    pub osc2_wave: Option<Wave>,
    pub osc2_octave: i8,
    pub osc2_semi: i8,
    pub osc2_level: f32,
    /// Semitones the filter envelope raises oscillator 2 (a sync sweep).
    pub osc2_env: f32,
    /// Oscillator 2 restarts with each cycle of oscillator 1.
    pub sync: bool,
    /// Pulse width, 0.05..0.95.
    pub pw: f32,
    /// A square an octave under oscillator 1, 0..1.
    pub sub: f32,
    /// White noise, 0..1.
    pub noise: f32,
    /// FM: how hard a sine modulator bends oscillator 1 (0 = off).
    pub fm: f32,
    /// The modulator's frequency over oscillator 1's.
    pub fm_ratio: f32,
    /// Seconds for the FM amount to fall to about a third.
    pub fm_decay: f32,
    // Filter (cutoff and resonance are the instrument's).
    pub filter_type: FilterType,
    pub filter_kind: FilterKind,
    /// 0..1.
    pub drive: f32,
    /// How far the cutoff follows the key (1 = one octave per octave).
    pub key_track: f32,
    /// Octaves the filter envelope moves the cutoff (-6..6).
    pub filter_env: f32,
    /// Octaves a full-velocity note opens the filter.
    pub vel_filter: f32,
    pub f_attack: f32,
    pub f_decay: f32,
    pub f_sustain: f32,
    pub f_release: f32,
    // LFO.
    pub lfo_shape: LfoShape,
    /// Hz.
    pub lfo_rate: f32,
    /// How much LFO is always applied, 0..1.
    pub lfo_depth: f32,
    /// How much the mod wheel adds to that, 0..1.
    pub wheel_depth: f32,
    /// Semitones at full depth (vibrato).
    pub lfo_pitch: f32,
    /// Octaves at full depth.
    pub lfo_filter: f32,
    /// Pulse width swing at full depth, 0..0.45.
    pub lfo_pwm: f32,
    // Voice.
    /// Glide time in seconds.
    pub glide: f32,
    pub mode: VoiceMode,
    /// Voices in Poly, 1..=16.
    pub voices: u8,
    /// Stacked copies of each note, 1..=5.
    pub unison: u8,
    /// Cents between the outer unison voices.
    pub spread: f32,
    /// Unison stereo width, 0..1.
    pub width: f32,
    /// How much velocity changes loudness, 0..1.
    pub velocity: f32,
    /// Accent on loud notes (velocity 100 and up): louder and brighter.
    pub accent: f32,
    /// Pitch bend range in semitones.
    pub bend_range: f32,
    // Effects.
    /// Chorus mix, 0..1 (0 = off).
    pub chorus: f32,
    /// Hz.
    pub chorus_rate: f32,
    /// 0..1.
    pub chorus_depth: f32,
    /// Hardware imperfection, 0..1 (0 = off): each note a little off
    /// pitch and brightness, oscillators drifting, free-running phases,
    /// a soft saturation and a faint noise floor.
    pub analog: f32,
    /// Bit-crusher depth in bits, 1..16 (0 = off).
    pub crush_bits: f32,
    /// Bit-crusher sample-rate reduction: hold every sample this many
    /// times, 1..32 (1 = off).
    pub crush_down: f32,
}

impl Default for SynthControls {
    fn default() -> Self {
        SynthControls {
            model: SynthModel::Default,
            osc1_octave: 0,
            osc1_semi: 0,
            osc1_fine: 0.0,
            osc1_level: 1.0,
            osc2_wave: None,
            osc2_octave: 0,
            osc2_semi: 0,
            osc2_level: 1.0,
            osc2_env: 0.0,
            sync: false,
            pw: 0.5,
            sub: 0.0,
            noise: 0.0,
            fm: 0.0,
            fm_ratio: 1.0,
            fm_decay: 1.0,
            filter_type: FilterType::LowPass,
            filter_kind: FilterKind::Classic,
            drive: 0.0,
            key_track: 0.0,
            filter_env: 0.0,
            vel_filter: 0.0,
            f_attack: 0.005,
            f_decay: 0.4,
            f_sustain: 0.0,
            f_release: 0.3,
            lfo_shape: LfoShape::Sine,
            lfo_rate: 5.0,
            lfo_depth: 0.0,
            wheel_depth: 1.0,
            lfo_pitch: 0.5,
            lfo_filter: 0.0,
            lfo_pwm: 0.0,
            glide: 0.0,
            mode: VoiceMode::Poly,
            voices: MAX_VOICES,
            unison: 1,
            spread: 15.0,
            width: 0.5,
            velocity: 1.0,
            accent: 0.0,
            bend_range: 2.0,
            chorus: 0.0,
            chorus_rate: 0.5,
            chorus_depth: 0.5,
            analog: 0.0,
            crush_bits: 0.0,
            crush_down: 1.0,
        }
    }
}

impl SynthControls {
    pub fn is_default(&self) -> bool {
        *self == SynthControls::default()
    }
}

impl Instrument {
    /// Plays through the original engine, bit for bit as it always did:
    /// none of the newer controls has been touched.
    pub fn is_classic(&self) -> bool {
        self.ext.is_default()
    }

    /// The model this instrument started from, and whether it has been
    /// changed since.
    pub fn model_edited(&self) -> bool {
        self.ext.model.preset() != *self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_model_is_the_original_instrument() {
        assert_eq!(SynthModel::Default.preset(), Instrument::default());
        assert!(Instrument::default().is_classic());
        for m in SynthModel::ALL {
            assert_eq!(m.preset().ext.model, m);
            assert!(!m.preset().model_edited());
            if m != SynthModel::Default {
                assert!(!m.preset().is_classic(), "{m:?}");
            }
        }
    }

    #[test]
    fn old_instruments_read_and_write_as_before() {
        // What the instrument always saved.
        let old = r#"{"wave":"Saw","detune":7.0,"attack":0.005,"decay":0.25,"sustain":0.7,"release":0.25,"cutoff":6000.0,"resonance":0.15,"gain_db":-10.0}"#;
        assert_eq!(serde_json::to_string(&Instrument::default()).unwrap(), old);
        let back: Instrument = serde_json::from_str(old).unwrap();
        assert_eq!(back, Instrument::default());
        let sq = r#"{"wave":"Square","detune":0.0,"attack":0.1,"decay":0.25,"sustain":0.7,"release":0.25,"cutoff":900.0,"resonance":0.5,"gain_db":-4.0}"#;
        let i: Instrument = serde_json::from_str(sq).unwrap();
        assert!(i.is_classic());
        assert_eq!(serde_json::to_string(&i).unwrap(), sq);
        // New controls round-trip.
        for m in SynthModel::ALL {
            let p = m.preset();
            let j = serde_json::to_string(&p).unwrap();
            assert_eq!(serde_json::from_str::<Instrument>(&j).unwrap(), p);
        }
        // A file with only some of the new controls.
        let part: Instrument =
            serde_json::from_str(r#"{"wave":"Saw","ext":{"model":"AcidLine","glide":0.1}}"#)
                .unwrap();
        assert_eq!(part.ext.model, SynthModel::AcidLine);
        assert_eq!(part.ext.glide, 0.1);
        assert_eq!(part.ext.voices, MAX_VOICES);
        assert_eq!(part.cutoff, 6000.0);
    }
}
