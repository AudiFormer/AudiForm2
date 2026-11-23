// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.
// Ported from Audiform (Audiform/src-tauri/src/dsp/effects/amp.rs), written by its author (AudiFormer)
// for Audiform and relicensed here by them. See docs/clean-room.md.

//! Amp Model: plays a neural amp capture (a `.nam` file, see
//! `crate::nam`) with a noise gate before it, a three-band tone stack and
//! level matching after it, and an optional cabinet impulse response at
//! the end. The capture and cabinet live in the slot's `data` (JSON), so
//! a session or preset carries them with it.
//!
//! Captures are mono: the two channels are summed going in. Around the
//! capture runs a full rig (see `super::amp_rig`): a boost with a tight
//! low cut before it, and after it a built-in cabinet with two mics (or
//! the loaded cabinet response), a small room and a doubler, which are
//! what make the sound stereo.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use super::amp_rig::{Boost, Cab, CabSet, Doubler, Gate, Room, CABS, MAX_DIST_CM, MICS};
use super::{choice, db_to_gain, num, toggle, EffectInfo, Params, Processor};
use crate::biquad::{Biquad, Coeffs, Shape};
use crate::convolve::{resample, Convolver};
use crate::nam::{self, Model, ModelInfo};

pub const AMP: &str = "af.amp_model";

/// Captures are leveled to this when Level Match is on, dB.
const TARGET_DB: f64 = -18.0;
/// Cabinet responses are cut to this long, seconds.
pub const IR_MAX_SECONDS: f64 = 1.0;
/// Cabinet convolution block: also the delay it adds, in frames.
const PART: usize = 128;

pub fn amp_info() -> EffectInfo {
    EffectInfo {
        id: AMP,
        sidechain: false,
        name: "Amp Model",
        category: "Amplitude",
        description: "Plays a neural amp capture (a .nam file) of a real amp, pedal or full rig as a whole rig: a noise gate and boost before it, Bass, Middle and Treble after it, then a built-in cabinet with two blended mics (or your own cabinet impulse response), a small room and a stereo doubler. Load captures in its window.",
        params: vec![
            num("input", "Input", "dB", -24.0, 24.0, 0.0, 0.1),
            toggle("gate", "Gate", true),
            num("gate_db", "Gate Threshold", "dB", -100.0, -20.0, -80.0, 0.5),
            num("bass", "Bass", "dB", -12.0, 12.0, 0.0, 0.1),
            num("middle", "Middle", "dB", -12.0, 12.0, 0.0, 0.1),
            num("treble", "Treble", "dB", -12.0, 12.0, 0.0, 0.1),
            num("output", "Output", "dB", -40.0, 12.0, 0.0, 0.1),
            toggle("level", "Level Match", true),
            toggle("cab", "Cabinet", true),
            // Added for the rig; kept after the first nine so saved
            // slots (values by position) still line up.
            num("boost", "Boost", "dB", 0.0, 24.0, 0.0, 0.1).group("Amp"),
            num("tight", "Tight", "Hz", 20.0, 400.0, 40.0, 1.0).log().group("Amp"),
            choice("cab_type", "Cabinet Type", CABS, CabSet::default().cab).group("Cab"),
            toggle("use_ir", "Use Loaded IR", true).group("Cab"),
            choice("mic_a", "Mic A", MICS, 0).group("Mic"),
            num("pos_a", "Position A", "%", 0.0, 100.0, 30.0, 1.0).group("Mic"),
            num("dist_a", "Distance A", "cm", 0.0, MAX_DIST_CM, 2.0, 0.5).group("Mic"),
            choice("mic_b", "Mic B", MICS, 1).group("Mic"),
            num("pos_b", "Position B", "%", 0.0, 100.0, 45.0, 1.0).group("Mic"),
            num("dist_b", "Distance B", "cm", 0.0, MAX_DIST_CM, 4.0, 0.5).group("Mic"),
            num("mic_blend", "Mic Blend", "%", 0.0, 100.0, 30.0, 1.0).group("Mic"),
            num("room", "Room", "%", 0.0, 100.0, 12.0, 1.0).group("Room"),
            num("room_size", "Room Size", "%", 0.0, 100.0, 35.0, 1.0).group("Room"),
            num("double", "Double", "%", 0.0, 100.0, 0.0, 1.0).group("Room"),
            // Added later; kept last so saved slots still line up.
            num("pan_a", "Pan A", "", -100.0, 100.0, 0.0, 1.0).group("Mic"),
            num("pan_b", "Pan B", "", -100.0, 100.0, 0.0, 1.0).group("Mic"),
        ],
    }
}

/// One built-in preset: a name and the settings it changes (the rest
/// are the defaults).
pub type FactoryPreset = (&'static str, &'static [(&'static str, f64)]);

/// The rig settings that come with the app (the capture is your own).
pub const PRESETS: &[FactoryPreset] = &[
    (
        "Clean Fender-ish",
        &[
            ("gate_db", -85.0),
            ("tight", 30.0),
            ("bass", 1.5),
            ("middle", -2.5),
            ("treble", 1.5),
            ("cab_type", 0.0),
            ("mic_a", 0.0),
            ("pos_a", 20.0),
            ("dist_a", 3.0),
            ("mic_b", 2.0),
            ("pos_b", 40.0),
            ("dist_b", 15.0),
            ("mic_blend", 35.0),
            ("room", 20.0),
            ("room_size", 45.0),
        ],
    ),
    (
        "Crunch",
        &[
            ("gate_db", -75.0),
            ("boost", 4.0),
            ("tight", 90.0),
            ("middle", 1.0),
            ("cab_type", 4.0),
            ("mic_a", 0.0),
            ("pos_a", 30.0),
            ("dist_a", 2.0),
            ("mic_b", 1.0),
            ("pos_b", 50.0),
            ("dist_b", 5.0),
            ("mic_blend", 30.0),
            ("room", 12.0),
            ("room_size", 35.0),
            ("double", 20.0),
        ],
    ),
    (
        "Modern High Gain",
        &[
            ("gate_db", -60.0),
            ("boost", 8.0),
            ("tight", 140.0),
            ("bass", 1.0),
            ("middle", -1.0),
            ("cab_type", 3.0),
            ("mic_a", 0.0),
            ("pos_a", 25.0),
            ("dist_a", 1.0),
            ("mic_b", 1.0),
            ("pos_b", 55.0),
            ("dist_b", 3.0),
            ("mic_blend", 40.0),
            ("room", 6.0),
            ("room_size", 25.0),
            ("double", 45.0),
        ],
    ),
];

/// What the slot's `data` holds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AmpData {
    /// The capture file's JSON, as read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<serde_json::Value>,
    /// The capture's file name, for showing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ir: Option<Ir>,
}

/// A cabinet impulse response, mono.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ir {
    pub name: String,
    pub rate: u32,
    pub samples: Vec<f32>,
}

/// A slot's data, read once and kept while the app runs (models are big).
struct Loaded {
    model: Option<(Model, ModelInfo)>,
    ir: Option<Ir>,
}

fn cache() -> &'static Mutex<HashMap<u64, Arc<Loaded>>> {
    static C: OnceLock<Mutex<HashMap<u64, Arc<Loaded>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn hash(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn load(data: Option<&str>) -> Arc<Loaded> {
    let Some(text) = data.filter(|t| !t.is_empty()) else {
        return Arc::new(Loaded {
            model: None,
            ir: None,
        });
    };
    let key = hash(text);
    if let Some(l) = cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
        return l;
    }
    let d: AmpData = serde_json::from_str(text).unwrap_or_default();
    let loaded = Arc::new(Loaded {
        model: d.model.as_ref().and_then(|m| nam::parse_value(m).ok()),
        ir: d.ir,
    });
    if let Ok(mut c) = cache().lock() {
        // Keep the cache small: a handful of captures in use at once.
        if c.len() > 16 {
            c.clear();
        }
        c.insert(key, loaded.clone());
    }
    loaded
}

/// Checks a capture file's text and says what it is.
pub fn read_model(text: &str) -> Result<(serde_json::Value, ModelInfo), String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|_| "This is not a model file.".to_string())?;
    let (_, info) = nam::parse_value(&v)?;
    Ok((v, info))
}

/// What a slot holds, for its window (after Audiform's `ampSummary`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    /// The capture.
    pub model: Option<ModelSummary>,
    /// The cabinet: (name, length in ms, rate).
    pub ir: Option<(String, u32, u32)>,
    /// Why a loaded capture will not play, if it will not.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelSummary {
    pub name: String,
    pub kind: String,
    pub rate: Option<f64>,
    pub gear: String,
    pub detail: String,
    /// The capture says it has a cabinet in it already.
    pub has_cab: bool,
}

/// Reads a slot's data for its window.
pub fn summary(data: Option<&str>) -> Summary {
    let mut s = Summary::default();
    let Some(text) = data.filter(|t| !t.is_empty()) else {
        return s;
    };
    let Ok(d) = serde_json::from_str::<AmpData>(text) else {
        s.problem = Some("The amp's saved data could not be read.".into());
        return s;
    };
    if let Some(m) = &d.model {
        let meta = &m["metadata"];
        let text = |k: &str| meta[k].as_str().unwrap_or_default().to_string();
        let gear = [text("gear_make"), text("gear_model")]
            .into_iter()
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let arch = m["architecture"].as_str().unwrap_or("Unknown").to_string();
        let kind = if arch == "SlimmableContainer" || arch == "Container" {
            "WaveNet (sized)".to_string()
        } else {
            arch.clone()
        };
        let by = text("modeled_by");
        let file = d.model_name.clone().unwrap_or_default();
        let name = Some(text("name"))
            .filter(|n| !n.is_empty())
            .or_else(|| d.model_name.clone())
            .unwrap_or_else(|| "Capture".into());
        let detail = [file, gear.clone(), if by.is_empty() { by } else { format!("captured by {by}") }]
            .into_iter()
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        if nam::parse_value(m).is_err() {
            s.problem = Some(format!(
                "{kind} captures are not supported yet, so sound passes through without the amp. WaveNet and LSTM captures play."
            ));
        }
        s.model = Some(ModelSummary {
            name,
            kind,
            rate: m["sample_rate"].as_f64(),
            gear,
            detail,
            has_cab: text("gear_type").to_lowercase().contains("cab"),
        });
    }
    if let Some(ir) = d.ir.as_ref().filter(|i| i.rate > 0) {
        let ms = (ir.samples.len() as f64 / f64::from(ir.rate) * 1000.0).round() as u32;
        s.ir = Some((ir.name.clone(), ms, ir.rate));
    }
    s
}

fn data_of(prev: Option<&str>) -> AmpData {
    prev.and_then(|t| serde_json::from_str(t).ok()).unwrap_or_default()
}

fn data_text(d: &AmpData) -> Option<String> {
    (d.model.is_some() || d.ir.is_some())
        .then(|| serde_json::to_string(d).ok())
        .flatten()
}

/// The slot's data with the capture file at `path` in it (the cabinet kept).
pub fn with_model(prev: Option<&str>, path: &std::path::Path) -> Result<Option<String>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let (v, _) = read_model(&text)?;
    let mut d = data_of(prev);
    d.model = Some(v);
    d.model_name = path.file_stem().map(|n| n.to_string_lossy().into_owned());
    Ok(data_text(&d))
}

/// The slot's data with the audio file at `path` as its cabinet (mono,
/// cut to [`IR_MAX_SECONDS`], at the file's own rate; the capture kept).
pub fn with_ir(prev: Option<&str>, path: &std::path::Path) -> Result<Option<String>, String> {
    let mut file = af2_io::decode_file(path)?;
    if file.num_channels() != 1 {
        file = af2_dsp::resample::remix(&file, 1);
    }
    let rate = file.rate.max(1);
    let keep = (IR_MAX_SECONDS * f64::from(rate)) as usize;
    let mut samples = file.channels.into_iter().next().unwrap_or_default();
    samples.truncate(keep);
    if samples.iter().all(|v| *v == 0.0) {
        return Err("This file has no sound in it.".into());
    }
    let mut d = data_of(prev);
    d.ir = Some(Ir {
        name: path
            .file_stem()
            .map_or_else(|| "Cabinet".into(), |n| n.to_string_lossy().into_owned()),
        rate,
        samples,
    });
    Ok(data_text(&d))
}

/// The slot's data with the capture (`model`) or the cabinet taken out.
pub fn without(prev: Option<&str>, model: bool) -> Option<String> {
    let mut d = data_of(prev);
    if model {
        d.model = None;
        d.model_name = None;
    } else {
        d.ir = None;
    }
    data_text(&d)
}

pub struct AmpModel {
    model: Option<Model>,
    /// The capture's loudness, for Level Match.
    loudness: Option<f64>,
    input: f32,
    gate: Gate,
    boost: Boost,
    tone: [Biquad; 3],
    output: f32,
    /// The built-in cabinet with its mics.
    rig: Cab,
    /// Cabinet on.
    cab_on: bool,
    /// The loaded cabinet response, used in place of the built-in one.
    ir: Option<Convolver>,
    use_ir: bool,
    /// The response's delay, kept whenever one is loaded, so switching
    /// it in or out never moves the sound in time.
    delay: Option<Vec<f32>>,
    delay_at: usize,
    room: Room,
    double: Doubler,
    mono: Vec<f32>,
    /// The right side when the mics are panned (else `mono` is both).
    right: Vec<f32>,
    /// The right side's copy of the response delay.
    delay_r: Option<Vec<f32>>,
    rate: u32,
}

fn tone_coeffs(p: &Params, rate: u32) -> [Coeffs; 3] {
    [
        Coeffs::new(Shape::LowShelf, rate, 120.0, 0.7, p.get("bass")),
        Coeffs::new(Shape::Peak, rate, 700.0, 0.8, p.get("middle")),
        Coeffs::new(Shape::HighShelf, rate, 3_200.0, 0.7, p.get("treble")),
    ]
}

fn cab_set(p: &Params) -> CabSet {
    let mic = |k: &str, pos: &str, dist: &str| {
        (
            p.get(k).round().max(0.0) as usize,
            p.get(pos) / 100.0,
            p.get(dist),
        )
    };
    CabSet {
        cab: p.get("cab_type").round().max(0.0) as usize,
        a: mic("mic_a", "pos_a", "dist_a"),
        b: mic("mic_b", "pos_b", "dist_b"),
        blend: p.get("mic_blend") / 100.0,
        pan_a: p.get("pan_a") / 100.0,
        pan_b: p.get("pan_b") / 100.0,
    }
}

impl AmpModel {
    pub fn new(p: &Params, rate: u32, data: Option<&str>) -> AmpModel {
        let loaded = load(data);
        let model = loaded.model.as_ref().map(|(m, _)| {
            let mut m = m.clone();
            m.reset();
            m
        });
        let ir = loaded.ir.as_ref().filter(|i| !i.samples.is_empty()).map(|i| {
            let max = (IR_MAX_SECONDS * f64::from(i.rate)) as usize;
            let cut = &i.samples[..i.samples.len().min(max)];
            Convolver::new(&resample(cut, i.rate, rate), PART)
        });
        let delay = ir.is_some().then(|| vec![0.0; PART]);
        let mut a = AmpModel {
            model,
            loudness: loaded.model.as_ref().and_then(|(_, i)| i.loudness),
            input: 1.0,
            gate: Gate::new(rate),
            boost: Boost::new(rate, p.get("tight"), p.get("boost")),
            tone: tone_coeffs(p, rate).map(Biquad::new),
            output: 1.0,
            rig: Cab::new(cab_set(p), rate),
            cab_on: true,
            ir,
            use_ir: true,
            delay_r: delay.clone(),
            delay,
            delay_at: 0,
            room: Room::new(rate, p.get("room") / 100.0, p.get("room_size") / 100.0),
            double: Doubler::new(rate, p.get("double") / 100.0),
            // Room for any block the engine sends, so the first one after a
            // rebuild does not allocate on the audio thread.
            mono: Vec::with_capacity(8_192),
            right: Vec::with_capacity(8_192),
            rate,
        };
        a.set(p);
        a
    }

    /// Takes the controls in. Never allocates (runs on the audio thread).
    fn set(&mut self, p: &Params) {
        self.input = db_to_gain(p.get("input"));
        self.gate.set(p.on("gate"), db_to_gain(p.get("gate_db")));
        self.boost.set(self.rate, p.get("tight"), p.get("boost"));
        let level = match self.loudness {
            Some(l) if p.on("level") && self.model.is_some() => TARGET_DB - l,
            _ => 0.0,
        };
        self.output = db_to_gain(p.get("output") + level);
        for (b, c) in self.tone.iter_mut().zip(tone_coeffs(p, self.rate)) {
            b.set(c);
        }
        self.cab_on = p.on("cab");
        self.use_ir = p.on("use_ir");
        self.rig.set(cab_set(p), self.rate);
        self.room.set(p.get("room") / 100.0, p.get("room_size") / 100.0);
        self.double.set(p.get("double") / 100.0);
    }
}

impl Processor for AmpModel {
    fn process(&mut self, buf: &mut [f32]) {
        let frames = buf.len() / 2;
        self.mono.clear();
        self.mono.extend(
            buf.as_chunks::<2>()
                .0
                .iter()
                .map(|f| (f[0] + f[1]) * 0.5 * self.input),
        );
        for s in &mut self.mono {
            *s = self.boost.run(self.gate.run(*s));
        }
        if let Some(m) = &mut self.model {
            m.process(&mut self.mono);
        }
        let use_ir = self.cab_on && self.use_ir && self.ir.is_some();
        // Panned mics make the built-in cabinet stereo.
        let stereo = !use_ir && self.cab_on && self.rig.panned();
        self.right.clear();
        if stereo {
            self.right.resize(frames, 0.0);
        }
        for (i, s) in self.mono.iter_mut().enumerate() {
            let mut y = *s;
            for b in &mut self.tone {
                y = b.run(y);
            }
            y *= self.output;
            if use_ir {
                if let Some(c) = &mut self.ir {
                    y = c.run(y);
                }
            } else {
                let mut yr = y;
                if self.cab_on {
                    if stereo {
                        (y, yr) = self.rig.run_stereo(y);
                    } else {
                        y = self.rig.run(y);
                        yr = y;
                    }
                }
                // Keep the loaded response's delay even when it is not used.
                if let Some(d) = &mut self.delay {
                    let out = d[self.delay_at];
                    d[self.delay_at] = y;
                    y = out;
                    if let Some(dr) = &mut self.delay_r {
                        let out_r = dr[self.delay_at];
                        dr[self.delay_at] = yr;
                        yr = out_r;
                    }
                    self.delay_at = (self.delay_at + 1) % d.len();
                }
                if stereo {
                    self.right[i] = yr;
                }
            }
            *s = y;
        }
        let room = self.room.active();
        let double = self.double.active();
        for i in 0..frames {
            let (l0, r0) = if stereo {
                (self.mono[i], self.right[i])
            } else {
                (self.mono[i], self.mono[i])
            };
            // Room and doubler hear the middle.
            let y = (l0 + r0) * 0.5;
            let (mut l, mut r) = (l0, r0);
            if room {
                let (a, b) = self.room.run(y);
                l += a;
                r += b;
            }
            if double {
                let (a, b) = self.double.run(y);
                l += a;
                r += b;
            }
            buf[i * 2] = l;
            buf[i * 2 + 1] = r;
        }
    }

    fn latency(&self) -> usize {
        if self.ir.is_some() {
            PART
        } else {
            0
        }
    }

    fn reset(&mut self) {
        if let Some(m) = &mut self.model {
            m.reset();
        }
        self.gate.reset();
        self.boost.reset();
        for b in &mut self.tone {
            b.reset();
        }
        self.rig.reset();
        if let Some(c) = &mut self.ir {
            c.reset();
        }
        if let Some(d) = &mut self.delay {
            d.fill(0.0);
        }
        if let Some(d) = &mut self.delay_r {
            d.fill(0.0);
        }
        self.room.reset();
        self.double.reset();
    }

    fn update(&mut self, p: &Params) -> bool {
        // The capture's loudness is kept, so every control retunes in
        // place; the latency only depends on the loaded data.
        self.set(p);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::info;

    fn params(set: &[(&str, f64)]) -> Params {
        let i = info(AMP).unwrap();
        let saved = set.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        Params::resolve(&i, &saved)
    }

    fn stereo_sine(f: f64, amp: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = (std::f64::consts::TAU * f * i as f64 / 48_000.0).sin() as f32 * amp;
                [v, v]
            })
            .collect()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// The rig's extras off: no gate, cabinet, room or low cut.
    const BARE: &[(&str, f64)] = &[("gate", 0.0), ("cab", 0.0), ("room", 0.0), ("tight", 20.0)];

    #[test]
    fn with_no_capture_it_passes_sound_through_its_tone_and_gain() {
        let mut set = BARE.to_vec();
        set.push(("output", -6.0206));
        let mut a = AmpModel::new(&params(&set), 48_000, None);
        let mut buf = stereo_sine(1_000.0, 0.5, 4_800);
        a.process(&mut buf);
        // Flat tone, mono sum of equal channels, half the level.
        let p = peak(&buf[4_800..]);
        assert!((p - 0.25).abs() < 5e-3, "{p}");
        assert_eq!(a.latency(), 0);
    }

    #[test]
    fn the_default_rig_sounds_like_a_mic_d_cab() {
        // Defaults (a capture is not needed for the rig): the highs above
        // the cab's corner are far down on the mids, and nothing blows up.
        let run = |f: f64| {
            let mut a = AmpModel::new(&params(&[("room", 0.0)]), 48_000, None);
            let mut buf = stereo_sine(f, 0.5, 24_000);
            for b in buf.chunks_mut(512) {
                a.process(b);
            }
            assert!(buf.iter().all(|v| v.is_finite()));
            peak(&buf[24_000..])
        };
        let mid = run(1_000.0);
        assert!(run(7_000.0) < mid * 0.3, "{} vs {mid}", run(7_000.0));
        assert!(run(12_000.0) < mid * 0.05, "{} vs {mid}", run(12_000.0));
    }

    #[test]
    fn the_whole_rig_stays_finite_when_pushed() {
        let set = [
            ("input", 24.0),
            ("boost", 24.0),
            ("tight", 400.0),
            ("bass", 12.0),
            ("treble", 12.0),
            ("output", 12.0),
            ("room", 100.0),
            ("room_size", 100.0),
            ("double", 100.0),
            ("dist_a", 30.0),
            ("dist_b", 0.0),
        ];
        for cab in 0..CABS.len() {
            let mut s = set.to_vec();
            s.push(("cab_type", cab as f64));
            let mut a = AmpModel::new(&params(&s), 44_100, None);
            let mut buf = stereo_sine(110.0, 1.0, 20_000);
            for b in buf.chunks_mut(256) {
                a.process(b);
            }
            assert!(buf.iter().all(|v| v.is_finite() && v.abs() < 1e3), "{cab}");
            // Room and double make it stereo.
            let side: f32 = buf.as_chunks::<2>().0.iter().map(|f| (f[0] - f[1]).abs()).sum();
            assert!(side > 1.0, "{cab}: {side}");
        }
    }

    #[test]
    fn the_gate_shuts_out_hiss_between_notes() {
        let mut a = AmpModel::new(&params(&[("room", 0.0), ("gate_db", -70.0)]), 48_000, None);
        let mut buf = stereo_sine(220.0, 0.5, 9_600);
        a.process(&mut buf);
        assert!(peak(&buf[9_600..]) > 0.05);
        // Hiss at -100 dB for half a second: the end is silent.
        let mut hiss: Vec<f32> = (0..48_000).flat_map(|i| {
            let v = if i % 3 == 0 { 1e-5 } else { -1e-5 };
            [v, v]
        }).collect();
        for b in hiss.chunks_mut(512) {
            a.process(b);
        }
        assert!(peak(&hiss[60_000..]) < 1e-8, "{}", peak(&hiss[60_000..]));
    }

    #[test]
    fn controls_retune_without_a_rebuild() {
        let mut a = AmpModel::new(&params(&[]), 48_000, None);
        assert!(a.update(&params(&[("cab_type", 0.0), ("room", 60.0), ("double", 50.0)])));
        let mut buf = stereo_sine(440.0, 0.5, 2_048);
        a.process(&mut buf);
        assert!(buf.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn the_presets_name_real_controls_in_range() {
        let i = info(AMP).unwrap();
        assert!(PRESETS.len() >= 3);
        for (name, set) in PRESETS {
            for (k, v) in *set {
                let p = i.params.iter().find(|p| p.key == *k);
                let p = p.unwrap_or_else(|| panic!("{name}: {k}"));
                assert!(*v >= p.min && *v <= p.max, "{name}: {k}");
            }
            let mut a = AmpModel::new(&params(set), 48_000, None);
            let mut buf = stereo_sine(330.0, 0.5, 4_096);
            a.process(&mut buf);
            assert!(buf.iter().all(|v| v.is_finite()), "{name}");
        }
    }

    #[test]
    fn a_cabinet_convolves_after_its_delay() {
        // A response of a single tap at 3 frames, half height.
        let mut ir = vec![0f32; 300];
        ir[3] = 0.5;
        let data = serde_json::to_string(&AmpData {
            ir: Some(Ir {
                name: "test".into(),
                rate: 48_000,
                samples: ir,
            }),
            ..AmpData::default()
        })
        .unwrap();
        let set = [("gate", 0.0), ("room", 0.0), ("tight", 20.0)];
        let mut a = AmpModel::new(&params(&set), 48_000, Some(&data));
        assert_eq!(a.latency(), PART);
        let mut buf = vec![0f32; 1_024];
        buf[0] = 1.0;
        buf[1] = 1.0;
        a.process(&mut buf);
        let at = |f: usize| buf[f * 2];
        // (The 20 Hz low cut before the amp leaves the click all but whole.)
        assert!((at(PART + 3) - 0.5).abs() < 1e-2, "{}", at(PART + 3));
        assert!(at(PART + 2).abs() < 1e-2);
        // Cabinet off keeps the same delay, without the response.
        let mut b = AmpModel::new(&params(&[set.as_slice(), &[("cab", 0.0)]].concat()), 48_000, Some(&data));
        assert_eq!(b.latency(), PART);
        let mut buf = vec![0f32; 1_024];
        buf[0] = 1.0;
        buf[1] = 1.0;
        b.process(&mut buf);
        assert!((buf[PART * 2] - 1.0).abs() < 1e-2);
        // The built-in cabinet in its place keeps the delay too.
        let c = AmpModel::new(&params(&[set.as_slice(), &[("use_ir", 0.0)]].concat()), 48_000, Some(&data));
        assert_eq!(c.latency(), PART);
    }

    #[test]
    fn resampling_keeps_a_smooth_response() {
        let x: Vec<f32> = (0..441).map(|i| (i as f32 * 0.01).sin()).collect();
        let y = resample(&x, 44_100, 48_000);
        assert_eq!(y.len(), 480);
        // The middle follows the same curve, at the new rate.
        let want = (240.0f32 * 44_100.0 / 48_000.0 * 0.01).sin();
        assert!((y[240] - want).abs() < 0.01, "{} {want}", y[240]);
    }
}
