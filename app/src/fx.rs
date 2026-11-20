// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! One view of every effect a chain can hold: Audiform27's built-in
//! effects (`af27-dsp`) and the ported Audiform effects (`af27-fx`), plus
//! the adapter that runs an `af27-fx` processor inside a live chain.

use af27_core::EffectSlot;
use af27_dsp::live::Live;

/// One control as the Inspector draws it.
pub struct Control {
    pub name: String,
    pub unit: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub log: bool,
    pub choices: Vec<String>,
    pub toggle: bool,
    pub hidden: bool,
    pub group: String,
}

/// The effects a chain can add, grouped for the Add Effect menu: (group,
/// [(id, name)]).
pub fn catalog() -> Vec<(String, Vec<(String, String)>)> {
    let mut out: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for (group, list) in af27_dsp::effects::GROUPS {
        let live: Vec<_> = list
            .iter()
            .filter(|e| e.realtime())
            .map(|e| (e.id().to_string(), e.name().to_string()))
            .collect();
        if !live.is_empty() {
            out.push((group.to_string(), live));
        }
    }
    let mut fx: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for info in af27_fx::registry() {
        let g = format!("{} (Audiform)", info.category);
        match fx.iter_mut().find(|(n, _)| *n == g) {
            Some((_, v)) => v.push((info.id.to_string(), info.name.to_string())),
            None => fx.push((g, vec![(info.id.to_string(), info.name.to_string())])),
        }
    }
    out.extend(fx);
    out
}

pub fn name(id: &str) -> String {
    if let Some(e) = af27_dsp::Effect::from_id(id) {
        return e.name().to_string();
    }
    af27_fx::info(id).map_or_else(|| id.to_string(), |i| i.name.to_string())
}

pub fn controls(id: &str) -> Vec<Control> {
    if let Some(e) = af27_dsp::Effect::from_id(id) {
        return e
            .params()
            .iter()
            .map(|p| Control {
                name: p.name.into(),
                unit: p.unit.into(),
                min: p.min,
                max: p.max,
                default: p.default,
                log: p.log,
                choices: Vec::new(),
                toggle: false,
                hidden: false,
                group: String::new(),
            })
            .collect();
    }
    af27_fx::info(id).map_or_else(Vec::new, |i| {
        i.params
            .iter()
            .map(|p| Control {
                name: p.name.into(),
                unit: p.unit.into(),
                min: p.min as f32,
                max: p.max as f32,
                default: p.default as f32,
                log: p.log,
                choices: p.choices.iter().map(|c| c.to_string()).collect(),
                toggle: p.toggle,
                hidden: p.hidden,
                group: p.group.into(),
            })
            .collect()
    })
}

pub fn defaults(id: &str) -> Vec<f32> {
    controls(id).iter().map(|c| c.default).collect()
}

fn params_for(id: &str, values: &[f32]) -> Option<af27_fx::Params> {
    let info = af27_fx::info(id)?;
    let pairs: Vec<(String, f64)> = info
        .params
        .iter()
        .zip(values)
        .map(|(p, v)| (p.key.to_string(), *v as f64))
        .collect();
    Some(af27_fx::Params::from_pairs(&info, &pairs))
}

/// An `af27-fx` processor (interleaved stereo) inside a live chain.
struct FxLive {
    p: Box<dyn af27_fx::Processor>,
    buf: Vec<f32>,
    info: af27_fx::EffectInfo,
    /// The current values, every key present, so a new setting replaces
    /// a value in place and never allocates on the audio thread.
    params: af27_fx::Params,
}

/// Takes new values into `params` (in place) and hands them to `p`.
fn take(
    p: &mut dyn af27_fx::Processor,
    info: &af27_fx::EffectInfo,
    params: &mut af27_fx::Params,
    vals: &[f32],
) {
    for (c, v) in info.params.iter().zip(vals) {
        params.set(c.key, c.clamp(*v as f64));
    }
    let _ = p.update(params);
}

impl Live for FxLive {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        // Blocks longer than the buffer go through in parts.
        let len = l.len().min(r.len());
        let step = (self.buf.len() / 2).max(1);
        let mut at = 0;
        while at < len {
            let n = (len - at).min(step);
            for i in 0..n {
                self.buf[i * 2] = l[at + i];
                self.buf[i * 2 + 1] = r[at + i];
            }
            self.p.process(&mut self.buf[..n * 2]);
            for i in 0..n {
                l[at + i] = self.buf[i * 2];
                r[at + i] = self.buf[i * 2 + 1];
            }
            at += n;
        }
    }

    fn set(&mut self, vals: &[f32]) {
        take(self.p.as_mut(), &self.info, &mut self.params, vals);
    }

    fn reset(&mut self) {
        self.p.reset();
    }

    fn latency(&self) -> usize {
        self.p.latency()
    }

    fn vary(&mut self, k: usize) {
        self.p.vary(k);
    }
}

fn factory(slot: &EffectSlot, rate: u32) -> Option<Box<dyn Live>> {
    let info = af27_fx::info(&slot.effect)?;
    let params = params_for(&slot.effect, &slot.values)?;
    let p = af27_fx::build_with(&slot.effect, rate, &params, slot.data.as_deref(), None)?;
    Some(Box::new(FxLive {
        p,
        buf: vec![0.0; af27_engine::BLOCK * 2],
        info,
        params,
    }))
}

/// An `af27-fx` effect's own form on a speaker bed (the Reverb, the
/// Convolution Reverb, Modulation): interleaved frames of every channel.
struct FxWide {
    p: Box<dyn af27_fx::Processor>,
    info: af27_fx::EffectInfo,
    params: af27_fx::Params,
}

impl af27_dsp::wide::Wide for FxWide {
    fn process(&mut self, buf: &mut [f32]) {
        self.p.process(buf);
    }

    fn set(&mut self, vals: &[f32]) {
        take(self.p.as_mut(), &self.info, &mut self.params, vals);
    }

    fn reset(&mut self) {
        self.p.reset();
    }

    fn latency(&self) -> usize {
        self.p.latency()
    }
}

fn wide_factory(
    slot: &EffectSlot,
    rate: u32,
    names: &[String],
    _block: usize,
) -> Option<Box<dyn af27_dsp::wide::Wide>> {
    let info = af27_fx::info(&slot.effect)?;
    let params = params_for(&slot.effect, &slot.values)?;
    let p = af27_fx::build_wide(&slot.effect, rate, &params, slot.data.as_deref(), names)?;
    Some(Box::new(FxWide { p, info, params }))
}

/// Lets every chain build the ported Audiform effects, on stereo and on
/// a speaker bed.
pub fn register() {
    af27_dsp::live::register_factory(factory);
    af27_dsp::wide::register_wide_factory(wide_factory);
}

/// Whether an effect takes new settings while playing; others are rebuilt
/// when a drag ends.
pub fn is_fx(id: &str) -> bool {
    af27_fx::info(id).is_some()
}

/// Runs a slot over a whole buffer (Waveform editor Apply). A file of
/// more than two channels is processed on every channel by its speaker
/// (pairs by name, lone speakers alone, the LFE left alone by delays and
/// tone shapers, Width and the mixer on L and R only; see
/// `af27_dsp::wide`).
pub fn process_offline(
    slot: &EffectSlot,
    input: &af27_core::AudioBuffer,
) -> Option<af27_core::AudioBuffer> {
    if input.num_channels() > 2 {
        return af27_dsp::wide::process_buffer(slot, input, af27_engine::BLOCK);
    }
    let mut p = af27_dsp::live::build_slot(slot, input.rate)?;
    let mut out = input.clone();
    let frames = out.frames();
    let lat = p.latency();
    let n = out.num_channels();
    let mut c = 0;
    while c < n {
        let mut l = out.channels[c].clone();
        let mut r = if c + 1 < n {
            out.channels[c + 1].clone()
        } else {
            l.clone()
        };
        l.resize(frames + lat, 0.0);
        r.resize(frames + lat, 0.0);
        for (x, y) in l
            .chunks_mut(af27_engine::BLOCK)
            .zip(r.chunks_mut(af27_engine::BLOCK))
        {
            p.process(x, y);
        }
        out.channels[c].copy_from_slice(&l[lat..lat + frames]);
        if c + 1 < n {
            out.channels[c + 1].copy_from_slice(&r[lat..lat + frames]);
        }
        c += 2;
        if c < n {
            p = af27_dsp::live::build_slot(slot, input.rate)?;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(id: &str) -> EffectSlot {
        EffectSlot {
            sidechain: None,
            effect: id.into(),
            values: defaults(id),
            bypass: false,
            data: None,
            ext_id: None,
        }
    }

    fn noise(frames: usize, ch: usize) -> af27_core::AudioBuffer {
        let mut seed = 1u32;
        let mut b = af27_core::AudioBuffer::new(48_000, ch, frames);
        for c in &mut b.channels {
            for s in c.iter_mut() {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *s = (seed as f32 / u32::MAX as f32 - 0.5) * 0.5;
            }
        }
        b
    }

    /// Every effect a chain can hold, built-in and Audiform's, runs on a
    /// 7.1.4 bed (a live wide chain and a Waveform Apply) and stays finite.
    #[test]
    fn every_effect_runs_finite_on_a_7_1_4_bed() {
        register();
        let names = af27_dsp::wide::labels(12);
        let input = noise(4800, 12);
        let inter = input.to_interleaved();
        let mut count = 0;
        for (_, list) in catalog() {
            for (id, _) in list {
                let s = slot(&id);
                let mut w = af27_dsp::wide::build_wide(&s, 48_000, &names, af27_engine::BLOCK)
                    .unwrap_or_else(|| panic!("{id} has a multichannel form"));
                let mut b = inter.clone();
                for c in b.chunks_mut(af27_engine::BLOCK * 12) {
                    w.process(c);
                }
                assert!(b.iter().all(|v| v.is_finite()), "{id} stays finite");
                w.set(&defaults(&id));
                w.reset();
                let out = process_offline(&s, &input).unwrap();
                assert_eq!(out.num_channels(), 12, "{id}");
                assert_eq!(out.frames(), 4800, "{id}");
                assert!(
                    out.channels.iter().flatten().all(|v| v.is_finite()),
                    "{id} applied stays finite"
                );
                count += 1;
            }
        }
        assert!(count >= 25, "{count}");
    }

    #[test]
    fn applying_on_a_surround_file_reaches_every_channel_but_mono_still_works() {
        register();
        let input = noise(4800, 6);
        // The Tube Preamp (a tone shaper) works on every speaker but the LFE.
        let out = process_offline(&slot(af27_fx::effects::tube::PREAMP), &input).unwrap();
        for c in 0..6 {
            let same = out.channels[c] == input.channels[c];
            assert_eq!(same, c == 3, "channel {c}");
        }
        // Stereo Width stays on L and R.
        let mut s = slot(af27_fx::effects::width::WIDTH);
        s.values = vec![0.0; s.values.len()];
        let out = process_offline(&s, &input).unwrap();
        for c in 2..6 {
            assert_eq!(out.channels[c], input.channels[c], "channel {c}");
        }
        // The Reverb fills the room but leaves the LFE alone.
        let out = process_offline(&slot(af27_fx::effects::verb::ID), &input).unwrap();
        assert_eq!(out.channels[3], input.channels[3]);
        assert_ne!(out.channels[4], input.channels[4]);
        // Mono: run as one side fed to both.
        let mono = noise(4800, 1);
        let out = process_offline(&slot(af27_fx::effects::tube::PREAMP), &mono).unwrap();
        assert_eq!(out.num_channels(), 1);
        assert!(out.channels[0].iter().all(|v| v.is_finite()));
        assert_ne!(out.channels[0], mono.channels[0]);
    }
}
