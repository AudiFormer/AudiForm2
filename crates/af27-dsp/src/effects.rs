// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The built-in effects. Each has a list of parameters (so the UI can
//! draw a generic dialog and the Effects Rack can store settings) and a
//! `process` that turns an input buffer into an output buffer, which may
//! be a different length (Speed, Reverb tails).

use crate::{noise, resample};
use af27_core::time::db_to_gain;
use af27_core::AudioBuffer;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub unit: &'static str,
    /// Draw the slider on a log scale (frequencies).
    pub log: bool,
}

const fn p(name: &'static str, min: f32, max: f32, default: f32, unit: &'static str) -> ParamSpec {
    ParamSpec {
        name,
        min,
        max,
        default,
        unit,
        log: false,
    }
}

const fn hz(name: &'static str, min: f32, max: f32, default: f32) -> ParamSpec {
    ParamSpec {
        name,
        min,
        max,
        default,
        unit: "Hz",
        log: true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Effect {
    Amplify,
    Normalize,
    MatchLoudness,
    FadeIn,
    FadeOut,
    Invert,
    Reverse,
    Silence,
    DcOffset,
    ParametricEq,
    HighPass,
    LowPass,
    Notch,
    HumRemover,
    Compressor,
    Limiter,
    NoiseGate,
    DeEsser,
    NoiseReduction,
    Echo,
    Reverb,
    StereoWidth,
    Speed,
}

/// Groups shown as submenus in the Effects menu.
pub const GROUPS: &[(&str, &[Effect])] = &[
    (
        "Amplitude",
        &[
            Effect::Amplify,
            Effect::Normalize,
            Effect::MatchLoudness,
            Effect::FadeIn,
            Effect::FadeOut,
        ],
    ),
    (
        "Dynamics",
        &[
            Effect::Compressor,
            Effect::Limiter,
            Effect::NoiseGate,
            Effect::DeEsser,
        ],
    ),
    (
        "Filter and EQ",
        &[
            Effect::ParametricEq,
            Effect::HighPass,
            Effect::LowPass,
            Effect::Notch,
        ],
    ),
    (
        "Restoration",
        &[Effect::NoiseReduction, Effect::HumRemover, Effect::DcOffset],
    ),
    (
        "Delay and Space",
        &[Effect::Echo, Effect::Reverb, Effect::StereoWidth],
    ),
    ("Time and Pitch", &[Effect::Speed]),
    (
        "Utility",
        &[Effect::Invert, Effect::Reverse, Effect::Silence],
    ),
];

impl Effect {
    pub fn all() -> impl Iterator<Item = Effect> {
        GROUPS.iter().flat_map(|(_, e)| e.iter().copied())
    }

    pub fn name(self) -> &'static str {
        match self {
            Effect::Amplify => "Amplify",
            Effect::Normalize => "Normalize",
            Effect::MatchLoudness => "Match Loudness",
            Effect::FadeIn => "Fade In",
            Effect::FadeOut => "Fade Out",
            Effect::Invert => "Invert",
            Effect::Reverse => "Reverse",
            Effect::Silence => "Silence",
            Effect::DcOffset => "Remove DC Offset",
            Effect::ParametricEq => "3-Band EQ",
            Effect::HighPass => "High-Pass Filter",
            Effect::LowPass => "Low-Pass Filter",
            Effect::Notch => "Notch Filter",
            Effect::HumRemover => "Hum Remover",
            Effect::Compressor => "Compressor",
            Effect::Limiter => "Limiter",
            Effect::NoiseGate => "Noise Gate",
            Effect::DeEsser => "De-Esser",
            Effect::NoiseReduction => "Noise Reduction",
            Effect::Echo => "Echo",
            Effect::Reverb => "Reverb",
            Effect::StereoWidth => "Stereo Width",
            Effect::Speed => "Speed",
        }
    }

    /// A stable id for saving.
    pub fn id(self) -> &'static str {
        match self {
            Effect::Amplify => "amplify",
            Effect::Normalize => "normalize",
            Effect::MatchLoudness => "match-loudness",
            Effect::FadeIn => "fade-in",
            Effect::FadeOut => "fade-out",
            Effect::Invert => "invert",
            Effect::Reverse => "reverse",
            Effect::Silence => "silence",
            Effect::DcOffset => "dc-offset",
            Effect::ParametricEq => "parametric-eq",
            Effect::HighPass => "high-pass",
            Effect::LowPass => "low-pass",
            Effect::Notch => "notch",
            Effect::HumRemover => "hum-remover",
            Effect::Compressor => "compressor",
            Effect::Limiter => "limiter",
            Effect::NoiseGate => "noise-gate",
            Effect::DeEsser => "de-esser",
            Effect::NoiseReduction => "noise-reduction",
            Effect::Echo => "echo",
            Effect::Reverb => "reverb",
            Effect::StereoWidth => "stereo-width",
            Effect::Speed => "speed",
        }
    }

    pub fn from_id(id: &str) -> Option<Effect> {
        Effect::all().find(|e| e.id() == id)
    }

    /// Whether the effect can run in the real-time Effects Rack (it keeps
    /// length and needs no look at the whole selection).
    pub fn realtime(self) -> bool {
        !matches!(
            self,
            Effect::Normalize
                | Effect::MatchLoudness
                | Effect::FadeIn
                | Effect::FadeOut
                | Effect::Reverse
                | Effect::Silence
                | Effect::NoiseReduction
                | Effect::Speed
        )
    }

    /// Whether the effect has a level detector another track can key
    /// (a sidechain): the Compressor, Limiter and Noise Gate.
    pub fn takes_key(self) -> bool {
        matches!(
            self,
            Effect::Compressor | Effect::Limiter | Effect::NoiseGate
        )
    }

    pub fn params(self) -> Vec<ParamSpec> {
        match self {
            Effect::Amplify => vec![p("Gain", -40.0, 40.0, 0.0, "dB")],
            Effect::Normalize => vec![p("Peak", -24.0, 0.0, -1.0, "dBFS")],
            Effect::MatchLoudness => vec![
                p("Target", -36.0, -8.0, -16.0, "LUFS"),
                p("Peak ceiling", -6.0, 0.0, -1.0, "dBFS"),
            ],
            Effect::FadeIn | Effect::FadeOut => vec![p("Curve", -1.0, 1.0, 0.0, "")],
            Effect::Invert | Effect::Reverse | Effect::Silence | Effect::DcOffset => Vec::new(),
            Effect::ParametricEq => vec![
                hz("Low freq", 20.0, 1000.0, 100.0),
                p("Low gain", -18.0, 18.0, 0.0, "dB"),
                hz("Mid freq", 100.0, 10_000.0, 1000.0),
                p("Mid gain", -18.0, 18.0, 0.0, "dB"),
                p("Mid Q", 0.1, 12.0, 1.0, ""),
                hz("High freq", 1000.0, 20_000.0, 8000.0),
                p("High gain", -18.0, 18.0, 0.0, "dB"),
            ],
            Effect::HighPass => vec![
                hz("Cutoff", 10.0, 2000.0, 80.0),
                p("Slope", 1.0, 4.0, 2.0, "x12 dB/oct"),
            ],
            Effect::LowPass => vec![
                hz("Cutoff", 200.0, 20_000.0, 12_000.0),
                p("Slope", 1.0, 4.0, 2.0, "x12 dB/oct"),
            ],
            Effect::Notch => vec![
                hz("Frequency", 20.0, 20_000.0, 1000.0),
                p("Q", 1.0, 60.0, 20.0, ""),
            ],
            Effect::HumRemover => vec![
                hz("Base", 40.0, 70.0, 60.0),
                p("Harmonics", 1.0, 8.0, 4.0, ""),
                p("Q", 5.0, 80.0, 30.0, ""),
            ],
            Effect::Compressor => vec![
                p("Threshold", -60.0, 0.0, -18.0, "dB"),
                p("Ratio", 1.0, 20.0, 3.0, ":1"),
                p("Attack", 0.1, 200.0, 10.0, "ms"),
                p("Release", 5.0, 2000.0, 120.0, "ms"),
                p("Knee", 0.0, 24.0, 6.0, "dB"),
                p("Makeup", -12.0, 24.0, 0.0, "dB"),
            ],
            Effect::Limiter => vec![
                p("Ceiling", -24.0, 0.0, -1.0, "dBFS"),
                p("Input boost", 0.0, 24.0, 0.0, "dB"),
                p("Look-ahead", 0.0, 10.0, 5.0, "ms"),
                p("Release", 10.0, 1000.0, 100.0, "ms"),
            ],
            Effect::NoiseGate => vec![
                p("Threshold", -80.0, 0.0, -40.0, "dB"),
                p("Attack", 0.1, 50.0, 2.0, "ms"),
                p("Hold", 0.0, 500.0, 50.0, "ms"),
                p("Release", 5.0, 2000.0, 200.0, "ms"),
                p("Floor", -80.0, 0.0, -60.0, "dB"),
            ],
            Effect::DeEsser => vec![
                hz("Frequency", 2000.0, 12_000.0, 6000.0),
                p("Threshold", -60.0, 0.0, -30.0, "dB"),
                p("Reduction", 0.0, 24.0, 8.0, "dB"),
            ],
            Effect::NoiseReduction => vec![
                p("Reduction", 0.0, 40.0, 12.0, "dB"),
                p("Sensitivity", 1.0, 4.0, 1.6, "x"),
            ],
            Effect::Echo => vec![
                p("Delay", 1.0, 2000.0, 350.0, "ms"),
                p("Feedback", 0.0, 0.95, 0.35, ""),
                p("Mix", 0.0, 1.0, 0.3, ""),
                hz("Tone", 500.0, 20_000.0, 6000.0),
            ],
            Effect::Reverb => vec![
                p("Room size", 0.0, 1.0, 0.6, ""),
                p("Damping", 0.0, 1.0, 0.5, ""),
                p("Mix", 0.0, 1.0, 0.25, ""),
                p("Width", 0.0, 1.0, 1.0, ""),
                p("Pre-delay", 0.0, 200.0, 10.0, "ms"),
            ],
            Effect::StereoWidth => vec![p("Width", 0.0, 2.0, 1.0, "x")],
            Effect::Speed => vec![p("Speed", 0.25, 4.0, 1.0, "x")],
        }
    }

    pub fn defaults(self) -> Vec<f32> {
        self.params().iter().map(|p| p.default).collect()
    }

    /// Runs the effect. `v` holds one value per parameter (missing ones
    /// take defaults).
    pub fn process(self, input: &AudioBuffer, v: &[f32]) -> AudioBuffer {
        let specs = self.params();
        let val = |i: usize| v.get(i).copied().unwrap_or(specs[i].default);
        let mut out = input.clone();
        let frames = out.frames();
        match self {
            Effect::Amplify => gain_all(&mut out, db_to_gain(val(0))),
            Effect::Normalize => {
                let peak = out.peak();
                if peak > 1e-9 {
                    gain_all(&mut out, db_to_gain(val(0)) / peak);
                }
            }
            Effect::MatchLoudness => {
                if let Some(l) = crate::loudness::integrated_loudness(&out) {
                    gain_all(&mut out, db_to_gain((val(0) as f64 - l) as f32));
                    let ceiling = db_to_gain(val(1));
                    if out.peak() > ceiling {
                        out = Effect::Limiter.process(&out, &[val(1), 0.0, 5.0, 100.0]);
                    }
                }
            }
            Effect::FadeIn | Effect::FadeOut => {
                let curve = val(0);
                for c in &mut out.channels {
                    for (i, s) in c.iter_mut().enumerate() {
                        let mut x = i as f32 / frames.max(1) as f32;
                        if self == Effect::FadeOut {
                            x = 1.0 - x;
                        }
                        *s *= shape(x, curve);
                    }
                }
            }
            Effect::Invert => gain_all(&mut out, -1.0),
            Effect::Reverse => out.channels.iter_mut().for_each(|c| c.reverse()),
            Effect::Silence => out.channels.iter_mut().for_each(|c| c.fill(0.0)),
            Effect::DcOffset => {
                for c in &mut out.channels {
                    let mean = c.iter().map(|&s| s as f64).sum::<f64>() / c.len().max(1) as f64;
                    c.iter_mut().for_each(|s| *s -= mean as f32);
                }
            }
            Effect::ParametricEq
            | Effect::HighPass
            | Effect::LowPass
            | Effect::Notch
            | Effect::HumRemover
            | Effect::Compressor
            | Effect::Limiter
            | Effect::NoiseGate
            | Effect::DeEsser => run_live(self, &mut out, v),
            Effect::NoiseReduction => {
                for c in &mut out.channels {
                    let print = noise::estimate(c);
                    noise::reduce(c, &print, val(0) as f64, val(1) as f64);
                }
            }
            Effect::Echo | Effect::Reverb => run_live(self, &mut out, v),
            Effect::StereoWidth => {
                if out.num_channels() >= 2 {
                    run_live(self, &mut out, v)
                }
            }
            Effect::Speed => {
                let s = val(0).clamp(0.25, 4.0) as f64;
                // Play faster = fewer samples at the same rate.
                out = AudioBuffer {
                    rate: input.rate,
                    channels: input
                        .channels
                        .iter()
                        .map(|c| resample::resample_channel(c, 1.0 / s))
                        .collect(),
                };
            }
        }
        out
    }
}

fn gain_all(b: &mut AudioBuffer, g: f32) {
    for c in &mut b.channels {
        c.iter_mut().for_each(|s| *s *= g);
    }
}

/// Fade shape: 0 is linear; negative bends towards a fast start
/// (logarithmic feel), positive towards a slow start (exponential).
fn shape(x: f32, curve: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if curve.abs() < 1e-3 {
        x
    } else {
        let k = 1.0 + curve.abs() * 3.0;
        if curve > 0.0 {
            x.powf(k)
        } else {
            1.0 - (1.0 - x).powf(k)
        }
    }
}

/// Runs an effect's live processor over a whole buffer, a channel pair at
/// a time (a lone last channel is run as both sides), and takes away the
/// processor's latency so the result lines up with the input.
fn run_live(e: Effect, b: &mut AudioBuffer, vals: &[f32]) {
    let n = b.num_channels();
    if n > 2 {
        // A surround file: every channel by its speaker (see `wide`).
        let slot = af27_core::EffectSlot {
            position: None,
            sidechain: None,
            effect: e.id().into(),
            values: vals.to_vec(),
            bypass: false,
            data: None,
            ext_id: None,
        };
        if let Some(out) = crate::wide::process_buffer(&slot, b, 4096) {
            *b = out;
        }
        return;
    }
    let frames = b.frames();
    let mut c = 0;
    while c < n {
        let Some(mut p) = crate::live::build(e, b.rate, vals) else {
            return;
        };
        let lat = p.latency();
        let mut l = b.channels[c].clone();
        let mut r = if c + 1 < n {
            b.channels[c + 1].clone()
        } else {
            l.clone()
        };
        l.resize(frames + lat, 0.0);
        r.resize(frames + lat, 0.0);
        for (x, y) in l.chunks_mut(4096).zip(r.chunks_mut(4096)) {
            p.process(x, y);
        }
        b.channels[c].copy_from_slice(&l[lat..lat + frames]);
        if c + 1 < n {
            b.channels[c + 1].copy_from_slice(&r[lat..lat + frames]);
        }
        c += 2;
    }
}

#[cfg(test)]
fn level_db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, f: f32, secs: f32, amp: f32) -> AudioBuffer {
        let n = (rate as f32 * secs) as usize;
        let d: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * f * i as f32 / rate as f32).sin() * amp)
            .collect();
        AudioBuffer {
            rate,
            channels: vec![d.clone(), d],
        }
    }

    #[test]
    fn every_effect_runs_and_stays_finite() {
        let b = sine(48_000, 440.0, 0.5, 0.5);
        for e in Effect::all() {
            let out = e.process(&b, &e.defaults());
            assert_eq!(out.num_channels(), 2, "{}", e.name());
            assert!(
                out.channels.iter().flatten().all(|s| s.is_finite()),
                "{} made NaN",
                e.name()
            );
            if e != Effect::Speed {
                assert_eq!(out.frames(), b.frames(), "{}", e.name());
            }
            assert_eq!(Effect::from_id(e.id()), Some(e));
        }
    }

    #[test]
    fn normalize_and_limit_hit_their_targets() {
        let b = sine(48_000, 440.0, 0.5, 0.25);
        let n = Effect::Normalize.process(&b, &[-3.0]);
        assert!((n.peak() - db_to_gain(-3.0)).abs() < 1e-3);
        let loud = sine(48_000, 440.0, 0.5, 1.0);
        let l = Effect::Limiter.process(&loud, &[-6.0, 6.0, 5.0, 100.0]);
        assert!(l.peak() <= db_to_gain(-6.0) + 1e-4);
    }

    #[test]
    fn compressor_reduces_loud_parts() {
        let b = sine(48_000, 440.0, 0.5, 1.0);
        let c = Effect::Compressor.process(&b, &[-20.0, 4.0, 1.0, 50.0, 0.0, 0.0]);
        let tail_peak = c.channels[0][20_000..]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()));
        // 0 dB in, -20 threshold, 4:1 → about -15 dB.
        assert!(
            (level_db(tail_peak) + 15.0).abs() < 1.5,
            "got {}",
            level_db(tail_peak)
        );
    }

    #[test]
    fn speed_changes_length() {
        let b = sine(48_000, 440.0, 1.0, 0.5);
        let s = Effect::Speed.process(&b, &[2.0]);
        assert_eq!(s.frames(), 24_000);
    }

    #[test]
    fn fade_in_starts_silent() {
        let b = sine(48_000, 440.0, 0.1, 1.0);
        let f = Effect::FadeIn.process(&b, &[0.0]);
        assert!(f.channels[0][..20].iter().all(|s| s.abs() < 0.01));
    }
}
