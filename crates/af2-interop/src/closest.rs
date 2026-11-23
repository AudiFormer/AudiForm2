// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Effects from another program's session that AudiForm 2 cannot run, and
//! their closest AudiForm 2 effect.
//!
//! An imported session keeps such effects in their racks, silent, so they
//! can be written back unchanged: a `.sesx` built-in with no converter
//! (`sesx:<name>`), or an Audiform 1 effect with no AudiForm 2 twin
//! (`altaudio.<id>`). [`convert`] swaps each for the AudiForm 2 effect
//! that does the same job, at its default settings (the other program's
//! settings have no meaning for it), keeping its power and rack row.
//! Effects with no such match stay as they are, and are listed.
//!
//! The match is by the effect's name: a Multiband Compressor becomes
//! Dynamics, a Graphic Equalizer (20 Bands) a 20-band Band EQ, a Flanger
//! Modulation set to Flanger.

use af2_core::automation::{Lane, LaneTarget};
use af2_core::session::{EffectSlot, Session};

use crate::sesx::KEPT;

/// The prefix Audiform 1's own effects carry in its sessions.
const AF1: &str = "altaudio.";

/// What an imported session's effects are.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Census {
    /// Every effect slot that is not a plug-in.
    pub effects: usize,
    /// The names of the effects AudiForm 2 cannot run (one per slot).
    pub silent: Vec<String>,
}

/// What [`convert`] did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Converted {
    /// (the other program's effect, the AudiForm 2 effect now in its place).
    pub replaced: Vec<(String, String)>,
    /// Effects with no close match, left silent in their slots.
    pub unmatched: Vec<String>,
    /// Drawn automation lines on replaced effects, removed (their values
    /// meant the old effect's controls).
    pub lanes_dropped: usize,
}

impl Converted {
    /// Short lines for the user: what changed, and what did not.
    pub fn notices(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.replaced.is_empty() {
            out.push(format!(
                "{} effect(s) became their closest AudiForm 2 effect, at its default settings: {}.",
                self.replaced.len(),
                summarize(self.replaced.iter().map(|(a, b)| format!("{a} → {b}")))
            ));
        }
        if !self.unmatched.is_empty() {
            out.push(format!(
                "{} effect(s) have no close match and stay silent in their racks: {}.",
                self.unmatched.len(),
                summarize(self.unmatched.iter().cloned())
            ));
        }
        if self.lanes_dropped > 0 {
            out.push(format!(
                "{} automation line(s) on replaced effects were removed.",
                self.lanes_dropped
            ));
        }
        out
    }
}

/// Distinct entries with a count where one repeats ("Flanger → Modulation
/// (×3)"), in the order first seen.
fn summarize(items: impl Iterator<Item = String>) -> String {
    let mut seen: Vec<(String, usize)> = Vec::new();
    for it in items {
        match seen.iter_mut().find(|(s, _)| *s == it) {
            Some((_, n)) => *n += 1,
            None => seen.push((it, 1)),
        }
    }
    seen.into_iter()
        .map(|(s, n)| if n > 1 { format!("{s} (×{n})") } else { s })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The name of an effect AudiForm 2 cannot run, as the other program
/// called it; None for one it runs (its own, a converted one, a plug-in).
pub fn silent_name(effect: &str) -> Option<String> {
    if let Some(n) = effect.strip_prefix(KEPT) {
        return Some(n.to_string());
    }
    let id = effect.strip_prefix(AF1)?;
    // A twin is loaded as `af.<id>` on import, so an `altaudio.` slot left
    // in a session is one AudiForm 2 does not have.
    Some(
        id.split('_')
            .filter(|w| !w.is_empty())
            .map(|w| {
                let mut c = w.chars();
                c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Every effect slot of the session: the master's, each track's and each
/// clip's.
fn chains(s: &Session) -> impl Iterator<Item = &Vec<EffectSlot>> {
    std::iter::once(&s.master_chain).chain(s.tracks.iter().flat_map(|t| {
        std::iter::once(&t.chain).chain(t.clips.iter().map(|c| &c.props.chain))
    }))
}

pub fn census(s: &Session) -> Census {
    let mut c = Census::default();
    for slot in chains(s).flatten() {
        if af2_core::plugin::is_plugin(&slot.effect) {
            continue;
        }
        c.effects += 1;
        if let Some(n) = silent_name(&slot.effect) {
            c.silent.push(n);
        }
    }
    c
}

/// The AudiForm 2 effect closest to one called `name`, with the settings
/// that make it that kind (a Flanger is Modulation set to Flanger).
pub fn closest(name: &str) -> Option<(&'static str, Vec<(&'static str, f64)>)> {
    use af2_fx::effects::{
        band_eq, curve_eq, dynamics, echo, filters, modulation, parametric_eq, pitch, restore,
        speech, tube, tube_master, verb, vocal, width,
    };
    let n = format!(" {} ", name.to_lowercase().replace(['_', '-', '(', ')'], " "));
    let has = |k: &str| n.contains(k);
    let any = |ks: &[&str]| ks.iter().any(|k| n.contains(k));
    let none = Vec::new;
    // Most particular first: "noise gate" before "noise", "speech volume
    // leveler" before "volume", "graphic phase" before "graphic".
    let m: (&'static str, Vec<(&'static str, f64)>) = if has("graphic") && any(&["eq", "equaliz"]) {
        let bands = if has(" 30 ") { 2.0 } else if has(" 20 ") { 1.0 } else { 0.0 };
        (band_eq::ID, vec![("bands", bands)])
    } else if has("fft filter") || has("curve") {
        (curve_eq::ID, none())
    } else if has("scientific filter") || any(&["butterworth", "chebyshev", "bessel", "elliptic"]) {
        (filters::PRECISION, none())
    } else if any(&["hum", "dehum"]) {
        (filters::HUM, none())
    } else if has("notch") {
        (filters::NOTCH, none())
    } else if any(&["de ess", "deess", "sibilan"]) {
        (dynamics::DEESSER, none())
    } else if any(&["gate", "expander "]) && !has("stereo") {
        (speech::GATE, none())
    } else if any(&["speech volume", "leveler", "leveller", "auto level"]) {
        (speech::LEVELER, none())
    } else if has("mastering") {
        (tube_master::ID, none())
    } else if has("limit") || has("maximiz") {
        (dynamics::LIMITER, none())
    } else if any(&["compress", "dynamics", "multiband", "multi band"]) {
        (dynamics::DYNAMICS, none())
    } else if any(&["dereverb", "de reverb", "room reduc", "reverb reduc"]) {
        (restore::ROOM, none())
    } else if has("hiss") {
        (restore::HISS, none())
    } else if any(&["click", "crackle", "pop "]) {
        (restore::CLICK, none())
    } else if any(&["clip restor", "declip", "clip repair"]) {
        (restore::CLIP, none())
    } else if has("noise") {
        (restore::NOISE, none())
    } else if any(&["vocal", "vox", "voice"]) {
        (vocal::VOCAL, none())
    } else if any(&["flang"]) {
        (modulation::ID, vec![("kind", 1.0)])
    } else if any(&["phaser", "phase shifter"]) && !has("graphic") {
        (modulation::ID, vec![("kind", 2.0)])
    } else if any(&["chorus", "ensemble"]) {
        (modulation::ID, vec![("kind", 0.0)])
    } else if any(&["doubl", " adt "]) {
        (modulation::ID, vec![("kind", 3.0)])
    } else if any(&["reverb", "room", "hall", "plate", "ambien"]) {
        (verb::ID, none())
    } else if any(&["delay", "echo", "multitap"]) {
        (echo::ID, none())
    } else if any(&["guitar", " amp "]) {
        (af2_fx::effects::amp::AMP, none())
    } else if any(&["distort", "overdrive", "saturat", "tube", "warmth"]) {
        (tube::PREAMP, none())
    } else if any(&["pitch", "shifter", "transpos"]) {
        (pitch::ID, none())
    } else if any(&["stereo", "width", "widen", "imager", "expander"]) {
        (width::WIDTH, none())
    } else if any(&["center channel", "centre channel", "center extract"]) {
        (af2_fx::effects::center::ID, none())
    } else if has("channel mixer") {
        (af2_fx::effects::mixer::ID, none())
    } else if has("phase") {
        (af2_fx::effects::phase_rotate::ID, none())
    } else if any(&["parametric", "equaliz", " eq ", "filter", "tone"]) {
        (parametric_eq::ID, none())
    } else if has("dc offset") {
        (filters::DC, none())
    } else if any(&["gain", "amplif", "volume", "trim"]) {
        (dynamics::GAIN, none())
    } else {
        return None;
    };
    Some(m)
}

/// A slot of AudiForm 2's effect `id`: its defaults, with `set` on top.
fn new_slot(id: &str, set: &[(&str, f64)]) -> Option<EffectSlot> {
    let info = af2_fx::info(id)?;
    let values = info
        .params
        .iter()
        .map(|p| {
            let v = set.iter().find(|(k, _)| *k == p.key).map_or(p.default, |(_, v)| *v);
            p.clamp(v) as f32
        })
        .collect();
    Some(EffectSlot {
        effect: id.to_string(),
        values,
        bypass: false,
        position: None,
        sidechain: None,
        data: None,
        ext_id: None,
    })
}

/// Replaces every effect AudiForm 2 cannot run with its closest match.
pub fn convert(s: &mut Session) -> Converted {
    let mut out = Converted::default();
    let mut gone: Vec<String> = Vec::new();
    let mut one = |chain: &mut Vec<EffectSlot>, gone: &mut Vec<String>| {
        for slot in chain.iter_mut() {
            let Some(name) = silent_name(&slot.effect) else {
                continue;
            };
            match closest(&name).and_then(|(id, set)| new_slot(id, &set)) {
                Some(mut fresh) => {
                    fresh.bypass = slot.bypass;
                    fresh.position = slot.position;
                    if af2_fx::takes_sidechain(&fresh.effect) {
                        fresh.sidechain = slot.sidechain;
                    }
                    if let Some(old) = slot.ext_id.take() {
                        gone.push(old);
                    }
                    let to = af2_fx::info(&fresh.effect).map_or_else(String::new, |i| i.name.to_string());
                    out.replaced.push((name, to));
                    *slot = fresh;
                }
                None => out.unmatched.push(name),
            }
        }
    };
    one(&mut s.master_chain, &mut gone);
    for t in &mut s.tracks {
        one(&mut t.chain, &mut gone);
        for c in &mut t.clips {
            one(&mut c.props.chain, &mut gone);
        }
    }
    // Lines drawn on the old effects' controls.
    let aimed = |l: &Lane| matches!(&l.target, LaneTarget::Effect { slot, .. } if gone.contains(slot));
    let mut drop = |lanes: &mut Vec<Lane>| {
        let before = lanes.len();
        lanes.retain(|l| !aimed(l));
        out.lanes_dropped += before - lanes.len();
    };
    drop(&mut s.master_lanes);
    for t in &mut s.tracks {
        drop(&mut t.lanes);
        for c in &mut t.clips {
            drop(&mut c.props.lanes);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use af2_core::session::TrackKind;

    fn kept(name: &str) -> EffectSlot {
        EffectSlot {
            effect: format!("{KEPT}{name}"),
            values: Vec::new(),
            bypass: false,
            position: None,
            sidechain: None,
            data: Some("{\"xml\":\"<component/>\"}".into()),
            ext_id: Some(format!("id-{name}")),
        }
    }

    #[test]
    fn names_find_their_closest_effect() {
        let id = |n: &str| closest(n).map(|(id, _)| id);
        assert_eq!(id("Multiband Compressor"), Some(af2_fx::effects::dynamics::DYNAMICS));
        assert_eq!(id("Dynamics Processing"), Some(af2_fx::effects::dynamics::DYNAMICS));
        assert_eq!(id("Noise Gate"), Some(af2_fx::effects::speech::GATE));
        assert_eq!(id("Speech Volume Leveler"), Some(af2_fx::effects::speech::LEVELER));
        assert_eq!(id("Hiss Reduction (process)"), Some(af2_fx::effects::restore::HISS));
        assert_eq!(id("DeHummer"), Some(af2_fx::effects::filters::HUM));
        assert_eq!(id("FFT Filter"), Some(af2_fx::effects::curve_eq::ID));
        assert_eq!(id("Full Reverb"), Some(af2_fx::effects::verb::ID));
        assert_eq!(id("Multitap Delay"), Some(af2_fx::effects::echo::ID));
        assert_eq!(id("Guitar Suite"), Some(af2_fx::effects::amp::AMP));
        assert_eq!(id("Distortion"), Some(af2_fx::effects::tube::PREAMP));
        assert_eq!(id("Pitch Shifter"), Some(af2_fx::effects::pitch::ID));
        assert_eq!(id("Mastering"), Some(af2_fx::effects::tube_master::ID));
        assert_eq!(id("Vocal Enhancer"), Some(af2_fx::effects::vocal::VOCAL));
        assert_eq!(id("Noise Print Reducer"), Some(af2_fx::effects::restore::NOISE));
        assert_eq!(id("DeReverb"), Some(af2_fx::effects::restore::ROOM));
        assert_eq!(id("Loudness Radar"), None);
        assert!(id("Doppler Shifter").is_some(), "a shifter of pitch");
        let (eq, set) = closest("Graphic Equalizer (20 Bands)").unwrap();
        assert_eq!(eq, af2_fx::effects::band_eq::ID);
        assert_eq!(set, vec![("bands", 1.0)]);
        let (m, set) = closest("Flanger").unwrap();
        assert_eq!(m, af2_fx::effects::modulation::ID);
        assert_eq!(set, vec![("kind", 1.0)]);
        assert_eq!(closest("Phaser").unwrap().1, vec![("kind", 2.0)]);
    }

    #[test]
    fn audiform_1_ids_read_as_names() {
        assert_eq!(silent_name("altaudio.shaped_noise_reducer").as_deref(), Some("Shaped Noise Reducer"));
        assert_eq!(silent_name("sesx:Multiband Compressor").as_deref(), Some("Multiband Compressor"));
        assert_eq!(silent_name("af.dynamics"), None);
        assert_eq!(silent_name("vst3:abc"), None);
    }

    #[test]
    fn convert_swaps_silent_effects_keeps_the_rest_and_drops_their_lines() {
        let mut s = Session::new("s", 48_000, 0);
        let t = s.add_track_of(TrackKind::Audio);
        let own = new_slot(af2_fx::effects::dynamics::GAIN, &[]).unwrap();
        let mut flanger = kept("Flanger");
        flanger.bypass = true;
        flanger.position = Some(3);
        let tr = s.track_mut(t).unwrap();
        tr.chain = vec![own.clone(), flanger, kept("Loudness Radar")];
        tr.lanes.push(Lane::new(LaneTarget::Effect {
            slot: "id-Flanger".into(),
            param: 0,
            key: None,
        }));
        tr.lanes.push(Lane::new(LaneTarget::Volume));
        s.master_chain.push(kept("Multiband Compressor"));

        assert_eq!(census(&s), Census {
            effects: 4,
            silent: vec!["Multiband Compressor".into(), "Flanger".into(), "Loudness Radar".into()],
        });
        let r = convert(&mut s);
        assert_eq!(r.replaced, vec![
            ("Multiband Compressor".to_string(), "Dynamics".to_string()),
            ("Flanger".to_string(), "Modulation".to_string()),
        ]);
        assert_eq!(r.unmatched, vec!["Loudness Radar".to_string()]);
        assert_eq!(r.lanes_dropped, 1);

        let tr = s.track_mut(t).unwrap();
        assert_eq!(tr.chain[0], own, "AudiForm 2's own effects untouched");
        let f = &tr.chain[1];
        assert_eq!(f.effect, af2_fx::effects::modulation::ID);
        assert!(f.bypass && f.position == Some(3), "power and row kept");
        assert_eq!(f.ext_id, None, "saved as a new slot, not over the old one");
        assert!(is_kept(&tr.chain[2]));
        assert_eq!(tr.lanes.len(), 1);
        assert_eq!(census(&s).silent, vec!["Loudness Radar".to_string()]);
    }

    fn is_kept(s: &EffectSlot) -> bool {
        crate::sesx::is_kept(&s.effect)
    }
}
