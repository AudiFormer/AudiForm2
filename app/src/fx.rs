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
    id: String,
    p: Box<dyn af27_fx::Processor>,
    buf: Vec<f32>,
    info: af27_fx::EffectInfo,
    pairs: Vec<(String, f64)>,
}

impl Live for FxLive {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(self.buf.len() / 2);
        for i in 0..n {
            self.buf[i * 2] = l[i];
            self.buf[i * 2 + 1] = r[i];
        }
        self.p.process(&mut self.buf[..n * 2]);
        for i in 0..n {
            l[i] = self.buf[i * 2];
            r[i] = self.buf[i * 2 + 1];
        }
    }

    fn set(&mut self, vals: &[f32]) {
        // The pairs vector was sized when built, so this does not allocate
        // beyond what the effect's own update does.
        for (pair, v) in self.pairs.iter_mut().zip(vals) {
            pair.1 = *v as f64;
        }
        let params = af27_fx::Params::from_pairs(&self.info, &self.pairs);
        let _ = self.p.update(&params);
        let _ = &self.id;
    }

    fn reset(&mut self) {
        self.p.reset();
    }

    fn latency(&self) -> usize {
        self.p.latency()
    }
}

fn factory(slot: &EffectSlot, rate: u32) -> Option<Box<dyn Live>> {
    let info = af27_fx::info(&slot.effect)?;
    let params = params_for(&slot.effect, &slot.values)?;
    let p = af27_fx::build_with(&slot.effect, rate, &params, slot.data.as_deref(), None)?;
    let pairs = info
        .params
        .iter()
        .zip(&slot.values)
        .map(|(p, v)| (p.key.to_string(), *v as f64))
        .collect();
    Some(Box::new(FxLive {
        id: slot.effect.clone(),
        p,
        buf: vec![0.0; af27_engine::BLOCK * 2],
        info,
        pairs,
    }))
}

/// Lets every chain build the ported Audiform effects.
pub fn register() {
    af27_dsp::live::register_factory(factory);
}

/// Whether an effect takes new settings while playing; others are rebuilt
/// when a drag ends.
pub fn is_fx(id: &str) -> bool {
    af27_fx::info(id).is_some()
}

/// Runs a slot over a whole buffer (Waveform editor Apply).
pub fn process_offline(
    slot: &EffectSlot,
    input: &af27_core::AudioBuffer,
) -> Option<af27_core::AudioBuffer> {
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
