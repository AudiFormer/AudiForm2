// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Studio Reverb: a dense, even room in ten controls. Room Size moves the
//! first of the reverb later (about 17 ms at the smallest to 50 ms at the
//! largest) and makes it quieter and leaner in the bass, without
//! lengthening it: Decay alone sets how long it rings. Early Reflections
//! adds strong echoes off the walls, in pairs of alternating polarity,
//! that grow with the room. Damping darkens the very top a little and
//! lifts the level; Diffusion smooths the tail and lowers it. The cuts
//! shape the wet sound only; Dry and Wet are plain levels.
//!
//! It runs the Reverb's engine (`verb.rs`) with a shape of its own,
//! tuned by measuring the project lead's reference recordings (a dry clip
//! and wet versions of it at each control's extremes) and matching
//! timing, level and tone. Nothing else was taken from anywhere.
//!
//! The Studio sound is now the Reverb's Studio kind (its first and
//! default type). This effect is no longer offered in the menus; it stays
//! so the sessions that hold it still play, and sounds exactly as the
//! Reverb's Studio kind does with the same ten settings.

use super::verb::{self, Reverb};
use super::{num, EffectInfo, Params, Processor};

pub const ID: &str = "af.studio_reverb";

pub fn studio_info() -> EffectInfo {
    EffectInfo {
        id: ID,
        sidechain: false,
        name: "Studio Reverb",
        category: "Delay and Reverb",
        description: "A dense, even room in ten controls: size, decay, early reflections, width, the cuts, damping, diffusion, and the dry and wet levels.",
        params: vec![
            num("size", "Room Size", "", 0.0, 100.0, 50.0, 1.0).group("Characteristics"),
            num("decay", "Decay", "ms", 200.0, 10_000.0, 1500.0, 1.0).log().group("Characteristics"),
            num("early", "Early Reflections", "%", 0.0, 100.0, 20.0, 1.0).group("Characteristics"),
            num("width", "Width", "", 0.0, 100.0, 100.0, 1.0).group("Characteristics"),
            num("high_cut", "High Frequency Cut", "Hz", 20.0, 20_000.0, 20_000.0, 1.0).log().group("Characteristics"),
            num("low_cut", "Low Frequency Cut", "Hz", 20.0, 4_000.0, 20.0, 1.0).log().group("Characteristics"),
            num("damping", "Damping", "%", 0.0, 100.0, 50.0, 1.0).group("Characteristics"),
            num("diffusion", "Diffusion", "%", 0.0, 100.0, 50.0, 1.0).group("Characteristics"),
            num("dry", "Dry", "%", 0.0, 100.0, 100.0, 1.0).group("Output Level"),
            num("wet", "Wet", "%", 0.0, 100.0, 35.0, 1.0).group("Output Level"),
        ],
    }
}

/// The engine's settings for these: its own keys, values passed through
/// as they are.
fn engine_params(p: &Params) -> Params {
    let mut v = Params::resolve(&verb::verb_info(), &Default::default());
    v.set("kind", verb::STUDIO as f64);
    for k in [
        "size", "decay", "early", "width", "high_cut", "low_cut", "damping", "diffusion", "dry", "wet",
    ] {
        v.set(key(k), p.get(k));
    }
    v.set("predelay", 0.0);
    v.set("modulation", 15.0);
    v
}

/// The engine's own key of the same name.
fn key(k: &str) -> &'static str {
    match k {
        "size" => "size",
        "decay" => "decay",
        "early" => "early",
        "width" => "width",
        "high_cut" => "high_cut",
        "low_cut" => "low_cut",
        "damping" => "damping",
        "diffusion" => "diffusion",
        "dry" => "dry",
        _ => "wet",
    }
}

pub struct StudioReverb {
    engine: Reverb,
}

impl StudioReverb {
    pub fn new(p: &Params, rate: u32) -> Self {
        Self {
            engine: Reverb::of_kind(&engine_params(p), rate, verb::STUDIO),
        }
    }
}

impl Processor for StudioReverb {
    fn process(&mut self, buf: &mut [f32]) {
        self.engine.process(buf);
    }

    fn reset(&mut self) {
        self.engine.reset();
    }

    fn update(&mut self, p: &Params) -> bool {
        self.engine.update(&engine_params(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const R: u32 = 44_100;

    fn params(over: &[(&str, f64)]) -> Params {
        let saved: BTreeMap<String, f64> = over.iter().map(|(k, v)| ((*k).to_string(), *v)).collect();
        Params::resolve(&studio_info(), &saved)
    }

    /// A click through it, wet only: the response, left channel.
    fn response(over: &[(&str, f64)], seconds: f64) -> Vec<f32> {
        let mut p = over.to_vec();
        p.extend([("dry", 0.0), ("wet", 100.0)]);
        let mut v = StudioReverb::new(&params(&p), R);
        let frames = (seconds * f64::from(R)) as usize;
        let mut b = vec![0.0f32; frames * 2];
        b[0] = 1.0;
        b[1] = 1.0;
        for c in b.chunks_mut(512) {
            v.process(c);
        }
        b.iter().step_by(2).copied().collect()
    }

    fn first_sound_ms(x: &[f32]) -> f64 {
        let peak = x.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        let i = x.iter().position(|v| v.abs() > peak * 0.05).unwrap_or(0);
        i as f64 / f64::from(R) * 1000.0
    }

    #[test]
    fn room_size_moves_the_first_sound_later_and_lowers_the_level() {
        // At the references' Decay (4000 ms), where it was tuned.
        let small = response(&[("size", 0.0), ("early", 0.0), ("decay", 4000.0)], 4.0);
        let big = response(&[("size", 100.0), ("early", 0.0), ("decay", 4000.0)], 4.0);
        let (a, b) = (first_sound_ms(&small), first_sound_ms(&big));
        assert!((10.0..25.0).contains(&a), "smallest room starts at {a} ms");
        assert!((40.0..60.0).contains(&b), "largest room starts at {b} ms");
        let e = |x: &[f32]| x.iter().map(|v| v * v).sum::<f32>();
        let db = 10.0 * (e(&big) / e(&small)).log10();
        assert!((-6.0..-2.0).contains(&db), "largest room {db:+.1} dB against the smallest");
    }

    #[test]
    fn decay_sets_how_long_it_rings() {
        let level_at = |decay: f64, t: f64| {
            let x = response(&[("decay", decay), ("early", 0.0)], t + 0.2);
            let a = (t * f64::from(R)) as usize;
            let w = &x[a..a + 4410];
            (w.iter().map(|v| v * v).sum::<f32>() / w.len() as f32).sqrt()
        };
        let short = level_at(500.0, 1.0);
        let long = level_at(4000.0, 1.0);
        assert!(long > short * 30.0, "a 4 s decay rings on where 0.5 s has died: {short} {long}");
    }

    #[test]
    fn the_reverbs_studio_kind_sounds_exactly_as_this_does() {
        let noise: Vec<f32> = (0..R as usize)
            .flat_map(|i| {
                let x = ((i as f32 * 12.9898).sin() * 43758.547).fract() - 0.5;
                [x, x * 0.5]
            })
            .collect();
        let run = |p: &mut dyn Processor| {
            let mut b = noise.clone();
            for c in b.chunks_mut(512) {
                p.process(c);
            }
            b
        };
        // The defaults: a new Reverb is the Studio kind at Studio's starts,
        // whatever its hidden pre-delay and motion say.
        let mut old = StudioReverb::new(&params(&[]), R);
        let verb_p = |over: &[(&str, f64)]| {
            let saved: BTreeMap<String, f64> = over.iter().map(|(k, v)| ((*k).to_string(), *v)).collect();
            Params::resolve(&verb::verb_info(), &saved)
        };
        let mut new = Reverb::new(&verb_p(&[("predelay", 80.0), ("modulation", 90.0)]), R);
        assert_eq!(run(&mut old), run(&mut new));
        // And at other settings.
        let set = [
            ("size", 80.0),
            ("decay", 4000.0),
            ("early", 70.0),
            ("width", 40.0),
            ("high_cut", 3000.0),
            ("low_cut", 300.0),
            ("damping", 10.0),
            ("diffusion", 90.0),
            ("dry", 60.0),
            ("wet", 80.0),
        ];
        let mut old = StudioReverb::new(&params(&set), R);
        let mut with_kind = set.to_vec();
        with_kind.push(("kind", verb::STUDIO as f64));
        let mut new = Reverb::new(&verb_p(&with_kind), R);
        assert_eq!(run(&mut old), run(&mut new));
    }

    #[test]
    fn it_runs_clean_through_every_extreme() {
        for over in [
            vec![("size", 0.0), ("early", 100.0), ("diffusion", 0.0), ("damping", 0.0)],
            vec![("size", 100.0), ("early", 100.0), ("diffusion", 100.0), ("damping", 100.0)],
            vec![("high_cut", 20.0), ("low_cut", 4000.0)],
            vec![("decay", 10_000.0), ("width", 0.0)],
        ] {
            let x = response(&over, 3.0);
            assert!(x.iter().all(|v| v.is_finite() && v.abs() < 8.0), "{over:?}");
        }
    }
}

/// Renders a file through the Studio Reverb for tuning against reference
/// recordings: `AF2_STUDIO_IN` is raw 32-bit float stereo at 44.1 kHz,
/// `AF2_STUDIO_JOBS` is `name:key=value,key=value;...`, and each result is
/// written to `AF2_STUDIO_OUT/name.raw`. Run with --ignored.
#[cfg(test)]
mod tuning {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    #[ignore]
    fn render_jobs() {
        let Ok(input) = std::env::var("AF2_STUDIO_IN") else { return };
        let out = std::env::var("AF2_STUDIO_OUT").unwrap();
        let bytes = std::fs::read(input).unwrap();
        let src: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        for job in std::env::var("AF2_STUDIO_JOBS").unwrap().split(';').filter(|j| !j.is_empty()) {
            let (name, sets) = job.split_once(':').unwrap();
            let saved: BTreeMap<String, f64> = sets
                .split(',')
                .filter_map(|kv| kv.split_once('='))
                .map(|(k, v)| (k.trim().to_string(), v.trim().parse().unwrap()))
                .collect();
            let mut v = StudioReverb::new(&Params::resolve(&studio_info(), &saved), 44_100);
            let mut buf = src.clone();
            for c in buf.chunks_mut(1024) {
                v.process(c);
            }
            let raw: Vec<u8> = buf.iter().flat_map(|f| f.to_le_bytes()).collect();
            std::fs::write(format!("{out}/{name}.raw"), raw).unwrap();
        }
    }
}
