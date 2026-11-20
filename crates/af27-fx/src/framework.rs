// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The small framework the effects are written against: what an effect
//! and its controls look like, the values a control has in a slot, and
//! the processor that runs on the audio thread.

use std::collections::BTreeMap;

/// One control of an effect.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamInfo {
    /// The key the value is saved under.
    pub key: &'static str,
    /// The label shown.
    pub name: &'static str,
    /// Unit shown after the value ("dB", "ms", "%", or empty).
    pub unit: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    /// The step a drag or arrow key moves by.
    pub step: f64,
    /// Shown on a logarithmic scale (frequencies, times).
    pub log: bool,
    /// For a pick-one control: the names of its options (the value is the
    /// index). Empty for a number or a switch.
    pub choices: &'static [&'static str],
    /// An on/off switch (0 or 1).
    pub toggle: bool,
    /// The heading the control is shown under, or empty.
    pub group: &'static str,
    /// Kept in the saved values but not shown.
    pub hidden: bool,
}

impl ParamInfo {
    /// Shown on a logarithmic scale.
    pub fn log(mut self) -> Self {
        self.log = true;
        self
    }

    /// Shown under the heading `g`.
    pub fn group(mut self, g: &'static str) -> Self {
        self.group = g;
        self
    }

    /// Kept but not shown.
    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    /// `v` brought inside the control's range (and to a whole option for
    /// a pick-one control or a switch).
    pub fn clamp(&self, v: f64) -> f64 {
        if !v.is_finite() {
            return self.default;
        }
        let v = v.clamp(self.min, self.max);
        if self.toggle || !self.choices.is_empty() {
            v.round()
        } else {
            v
        }
    }
}

/// A number control.
#[allow(clippy::too_many_arguments)]
pub fn num(
    key: &'static str,
    name: &'static str,
    unit: &'static str,
    min: f64,
    max: f64,
    default: f64,
    step: f64,
) -> ParamInfo {
    ParamInfo {
        key,
        name,
        unit,
        min,
        max,
        default,
        step,
        log: false,
        choices: &[],
        toggle: false,
        group: "",
        hidden: false,
    }
}

/// A pick-one control; its value is the index into `choices`.
pub fn choice(
    key: &'static str,
    name: &'static str,
    choices: &'static [&'static str],
    default: usize,
) -> ParamInfo {
    ParamInfo {
        key,
        name,
        unit: "",
        min: 0.0,
        max: choices.len().saturating_sub(1) as f64,
        default: default as f64,
        step: 1.0,
        log: false,
        choices,
        toggle: false,
        group: "",
        hidden: false,
    }
}

/// An on/off switch.
pub fn toggle(key: &'static str, name: &'static str, on: bool) -> ParamInfo {
    ParamInfo {
        key,
        name,
        unit: "",
        min: 0.0,
        max: 1.0,
        default: if on { 1.0 } else { 0.0 },
        step: 1.0,
        log: false,
        choices: &[],
        toggle: true,
        group: "",
        hidden: false,
    }
}

/// What an effect is and the controls it has.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectInfo {
    /// Stable id, saved in sessions (e.g. "af.reverb").
    pub id: &'static str,
    pub name: &'static str,
    /// The menu heading it is listed under.
    pub category: &'static str,
    pub description: &'static str,
    pub params: Vec<ParamInfo>,
    /// It has a level detector another track can key (a sidechain): the
    /// host passes that track's sound to [`Processor::process_keyed`].
    pub sidechain: bool,
}

impl EffectInfo {
    /// The control with this key.
    pub fn param(&self, key: &str) -> Option<&ParamInfo> {
        self.params.iter().find(|p| p.key == key)
    }
}

/// The values of an effect's controls: every control's default, with
/// any saved values in its place (kept inside each control's range).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Params(BTreeMap<&'static str, f64>);

impl Params {
    /// The defaults of `info`, overridden by `saved` where it has a value.
    /// Saved keys the effect does not know are ignored.
    pub fn resolve(info: &EffectInfo, saved: &BTreeMap<String, f64>) -> Self {
        Params(
            info.params
                .iter()
                .map(|p| {
                    let v = saved.get(p.key).map_or(p.default, |&v| p.clamp(v));
                    (p.key, v)
                })
                .collect(),
        )
    }

    /// The defaults of `info`, overridden by the pairs in `saved`.
    pub fn from_pairs(info: &EffectInfo, saved: &[(String, f64)]) -> Self {
        let map: BTreeMap<String, f64> = saved.iter().cloned().collect();
        Self::resolve(info, &map)
    }

    /// Every control's key and value.
    pub fn values(&self) -> impl Iterator<Item = (&'static str, f64)> + '_ {
        self.0.iter().map(|(k, v)| (*k, *v))
    }

    /// A control's value; 0 for a key the effect does not have.
    pub fn get(&self, key: &str) -> f64 {
        self.0.get(key).copied().unwrap_or(0.0)
    }

    /// A switch's state.
    pub fn on(&self, key: &str) -> bool {
        self.get(key) >= 0.5
    }

    /// A pick-one control's option, as an index.
    pub fn index(&self, key: &str) -> usize {
        let v = self.get(key);
        if v.is_finite() && v > 0.0 {
            v.round() as usize
        } else {
            0
        }
    }

    /// Sets one value (unchecked: used by tests and hosts that already
    /// clamp).
    pub fn set(&mut self, key: &'static str, value: f64) {
        self.0.insert(key, value);
    }
}

/// Decibels to a linear gain.
#[inline]
pub fn db_to_gain(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

/// An effect running on interleaved stereo audio.
pub trait Processor: Send {
    /// Processes interleaved stereo frames (L, R, L, R, ...) in place.
    fn process(&mut self, buf: &mut [f32]);

    /// The delay the effect adds, in frames.
    fn latency(&self) -> usize {
        0
    }

    /// Forgets all memory (tails, envelopes), as after a seek.
    fn reset(&mut self);

    /// Takes new control values while running. Returns false when the
    /// change needs the processor built again (for example when its
    /// latency would change).
    fn update(&mut self, _p: &Params) -> bool {
        false
    }

    /// Processes with a side-chain key (interleaved stereo, the same
    /// length as `buf`): another track's sound, which drives the level
    /// detector in place of the effect's own input. Effects without a key
    /// input (see [`EffectInfo::sidechain`]) ignore it. Must not allocate.
    fn process_keyed(&mut self, buf: &mut [f32], _key: &[f32]) {
        self.process(buf);
    }

    /// Sets this copy apart from other copies of the same effect running
    /// on a speaker bed's other pairs (copy `k`), so effects that sweep
    /// or drift do not move in lockstep. Called once, after building.
    fn vary(&mut self, _k: usize) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_take_defaults_and_clamp() {
        let info = EffectInfo {
            id: "t",
            sidechain: false,
            name: "T",
            category: "",
            description: "",
            params: vec![
                num("a", "A", "", 0.0, 10.0, 5.0, 1.0),
                choice("c", "C", &["x", "y", "z"], 1),
                toggle("t", "T", true),
            ],
        };
        let p = Params::resolve(&info, &BTreeMap::new());
        assert_eq!((p.get("a"), p.index("c"), p.on("t")), (5.0, 1, true));
        let saved = BTreeMap::from([
            ("a".to_string(), 20.0),
            ("c".to_string(), 1.7),
            ("t".to_string(), 0.0),
            ("zzz".to_string(), 1.0),
        ]);
        let p = Params::resolve(&info, &saved);
        assert_eq!((p.get("a"), p.index("c"), p.on("t")), (10.0, 2, false));
        assert_eq!(p.get("zzz"), 0.0);
        assert!((db_to_gain(-6.0206) - 0.5).abs() < 1e-4);
    }
}
