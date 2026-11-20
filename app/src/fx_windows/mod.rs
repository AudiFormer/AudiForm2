// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The effect windows' own faces, keyed by effect id. The rack's effect
//! window calls [`custom`]; an effect with a panel of its own in
//! [`PANELS`] gets it, every other effect gets the dial columns (a dial
//! and a value box per control, one column per group). Plug-ins keep
//! their own window (see `plugins.rs`).
//!
//! The design of several panels follows the project lead's Audiform
//! windows, rebuilt here in egui (see docs/clean-room.md for the list and
//! each file's provenance): the dial columns (Audiform's effect
//! controls), the Phase Curve graph, the Reverb and Convolution Reverb
//! faceplates, the Delay faceplate (driving AudiForm 2's Echo) and the
//! Opto Leveler / Tube Preamp and Vox Strip panels. The Compressor,
//! Limiter, Noise Gate and De-Esser units are AudiForm 2's own.
//!
//! Cheap to draw: values are read from the slot each frame, graphs are a
//! few dozen points, and the windows ask for frames only while sound
//! plays (for the meters) or a needle is still settling.

mod dynamics;
mod parts;
mod phase;
mod space;

use egui::RichText;

use af27_core::EffectSlot;

use crate::rack::{chain_of, FILE};
use crate::state::State;
use crate::theme::{c, Theme};
use crate::widgets;

/// A panel: draws the effect's face and changes values through `fx`.
pub type Panel = fn(&mut egui::Ui, &mut Fx, &Env);

/// Effects with a face of their own. To add one, add its id and panel
/// function here.
pub const PANELS: &[(&str, Panel)] = &[
    ("compressor", dynamics::compressor),
    ("limiter", dynamics::limiter),
    ("noise-gate", dynamics::gate),
    ("de-esser", dynamics::deesser),
    ("af.opto_leveler", dynamics::opto),
    ("af.tube_preamp", dynamics::preamp),
    ("af.vox_strip", dynamics::vox),
    ("af.reverb", space::verb),
    ("af.convolution_reverb", space::convolution),
    ("reverb", space::room),
    ("echo", space::echo),
    ("af.phase_curve", phase::panel),
];

/// One control of the effect being drawn.
#[derive(Clone)]
pub struct Ctl {
    /// The `af.*` key, or the control's name for built-in effects.
    pub key: String,
    pub name: String,
    pub unit: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub log: bool,
    pub choices: Vec<String>,
    pub toggle: bool,
    pub hidden: bool,
    pub group: String,
}

fn ctls(id: &str) -> Vec<Ctl> {
    let keys: Vec<String> = af27_fx::info(id).map_or_else(Vec::new, |i| {
        i.params.iter().map(|p| p.key.to_string()).collect()
    });
    crate::fx::controls(id)
        .into_iter()
        .enumerate()
        .map(|(k, p)| Ctl {
            key: keys.get(k).cloned().unwrap_or_else(|| p.name.clone()),
            name: p.name,
            unit: p.unit,
            min: f64::from(p.min),
            max: f64::from(p.max),
            default: f64::from(p.default),
            log: p.log && p.min > 0.0,
            choices: p.choices,
            toggle: p.toggle,
            hidden: p.hidden,
            group: p.group,
        })
        .collect()
}

/// The effect being drawn: its controls and values, and what changed.
pub struct Fx {
    pub id: String,
    pub ctls: Vec<Ctl>,
    pub vals: Vec<f32>,
    /// The slot's data (an impulse response's file...).
    pub data: Option<String>,
    /// New data for the slot, when a panel changed it.
    pub new_data: Option<Option<String>>,
    pub changed: bool,
    pub released: bool,
    /// A stable id for this window's widgets.
    pub uid: egui::Id,
}

impl Fx {
    /// The index of a control by key (or name).
    pub fn k(&self, key: &str) -> Option<usize> {
        self.ctls
            .iter()
            .position(|c| c.key == key)
            .or_else(|| self.ctls.iter().position(|c| c.name == key))
    }

    pub fn get(&self, key: &str) -> f64 {
        self.k(key)
            .map_or(0.0, |k| f64::from(self.vals.get(k).copied().unwrap_or(0.0)))
    }

    pub fn ctl(&self, key: &str) -> Option<&Ctl> {
        self.k(key).map(|k| &self.ctls[k])
    }

    /// Sets a control (clamped) as one finished change.
    pub fn set(&mut self, key: &str, v: f64) {
        if let Some(k) = self.k(key) {
            let c = &self.ctls[k];
            let v = v.clamp(c.min.min(c.max), c.max.max(c.min)) as f32;
            if self.vals[k] != v {
                self.vals[k] = v;
                self.changed = true;
                self.released = true;
            }
        }
    }

    /// Sets a control (clamped) mid-gesture: live, finished on release.
    pub fn set_live(&mut self, key: &str, v: f64) {
        if let Some(k) = self.k(key) {
            let c = &self.ctls[k];
            let v = v.clamp(c.min.min(c.max), c.max.max(c.min)) as f32;
            if self.vals[k] != v {
                self.vals[k] = v;
                self.changed = true;
            }
        }
    }

    /// Takes a widget's response into the change flags.
    pub fn track(&mut self, r: &egui::Response) {
        if r.changed() {
            self.changed = true;
            if !r.dragged() {
                self.released = true;
            }
        }
        if r.drag_stopped() {
            self.released = true;
        }
    }

    /// A dial for control `key` (nothing when the effect has no such
    /// control). `label` empty: the control's own name.
    pub fn knob(&mut self, ui: &mut egui::Ui, key: &str, size: f32, label: &str, marks: &[f64]) {
        let fmt = self.ctl(key).map(|c| c.unit.clone()).unwrap_or_default();
        self.knob_fmt(ui, key, size, label, marks, &|v| widgets::fmt_unit(v, &fmt));
    }

    pub fn knob_fmt(
        &mut self,
        ui: &mut egui::Ui,
        key: &str,
        size: f32,
        label: &str,
        marks: &[f64],
        fmt: &dyn Fn(f64) -> String,
    ) {
        let Some(k) = self.k(key) else { return };
        let c = self.ctls[k].clone();
        let mut v = f64::from(self.vals[k]);
        let name = if label.is_empty() { &c.name } else { label };
        let r = widgets::knob(
            ui,
            self.uid.with(("knob", k)),
            &mut v,
            c.min,
            c.max,
            c.default,
            c.log,
            size,
            name,
            marks,
            fmt,
        );
        if r.changed() {
            self.vals[k] = v as f32;
        }
        self.track(&r);
    }

    /// A stepped switch for a choice control.
    pub fn choice(&mut self, ui: &mut egui::Ui, key: &str, tips: &[&str]) {
        let Some(k) = self.k(key) else { return };
        let c = self.ctls[k].clone();
        let mut cur = (f64::from(self.vals[k]) - c.min).round().max(0.0) as usize;
        cur = cur.min(c.choices.len().saturating_sub(1));
        let names: Vec<&str> = c.choices.iter().map(String::as_str).collect();
        let r = widgets::segmented(ui, &mut cur, &names, tips);
        if r.changed() {
            self.set(key, c.min + cur as f64);
        }
    }

    /// A lit switch for a toggle control.
    pub fn toggle(&mut self, ui: &mut egui::Ui, key: &str, label: &str) {
        let Some(k) = self.k(key) else { return };
        let mut on = self.vals[k] >= 0.5;
        let name = if label.is_empty() {
            self.ctls[k].name.clone()
        } else {
            label.to_string()
        };
        if widgets::switch(ui, &mut on, &name).changed() {
            self.set(key, if on { 1.0 } else { 0.0 });
        }
    }

    pub fn on(&self, key: &str) -> bool {
        self.get(key) >= 0.5
    }
}

/// What a panel knows of where it runs.
pub struct Env {
    pub th: Theme,
    /// The rack's input and output levels (linear peaks), when known.
    pub levels: Option<(f32, f32)>,
    pub playing: bool,
    /// On a 3D Space session's speaker bed (or a surround file).
    pub spatial: bool,
    pub rate: u32,
    /// The window's own id, for state that lasts between frames.
    pub key: egui::Id,
}

impl Env {
    /// dBFS of a linear level (floored at -90).
    pub fn db(v: f32) -> f64 {
        if v <= 1e-5 {
            -90.0
        } else {
            20.0 * f64::from(v).log10()
        }
    }
}

fn slot_at(st: &mut State, target: u16, i: usize) -> Option<EffectSlot> {
    chain_of(st, target).and_then(|c| c.get(i)).cloned()
}

/// Draws slot `i` of rack `target` with its own face (or the dial
/// columns) and takes the changes into the session. False when the slot
/// is a plug-in or has no controls (the caller draws its plain list).
pub fn custom(ui: &mut egui::Ui, st: &mut State, target: u16, i: usize, th: &Theme) -> bool {
    let Some(slot) = slot_at(st, target, i) else {
        return false;
    };
    if af27_core::plugin::is_plugin(&slot.effect) {
        return false;
    }
    let ctls = ctls(&slot.effect);
    if ctls.is_empty() {
        return false;
    }
    let mut vals = slot.values.clone();
    vals.resize(ctls.len(), 0.0);
    let uid = egui::Id::new(("fxw", target, i, slot.effect.as_str()));
    let mut fx = Fx {
        id: slot.effect.clone(),
        ctls,
        vals,
        data: slot.data.clone(),
        new_data: None,
        changed: false,
        released: false,
        uid,
    };
    let playing = st.is_playing();
    let levels = (target != FILE).then(|| st.rack_levels(target));
    let spatial = if target == FILE {
        st.doc().is_some_and(|d| d.buf.num_channels() > 2)
    } else {
        st.session.space.is_some()
    };
    let rate = if target == FILE {
        st.doc().map_or(48_000, |d| d.buf.rate)
    } else {
        st.session.rate
    };
    let env = Env {
        th: th.clone(),
        levels,
        playing,
        spatial,
        rate,
        key: uid,
    };
    presets_row(ui, &mut fx, th);
    ui.add_space(4.0);
    let panel = PANELS
        .iter()
        .find(|(id, _)| *id == fx.id)
        .map_or(columns as Panel, |(_, p)| *p);
    panel(ui, &mut fx, &env);
    if playing {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
    finish(ui, st, target, i, &slot, fx);
    true
}

/// Takes a panel's changes into the slot: live while dragging, one undo
/// step per gesture, a rebuild when a change is done for effects that
/// cannot retune while playing.
fn finish(ui: &egui::Ui, st: &mut State, target: u16, i: usize, slot: &EffectSlot, fx: Fx) {
    let data_changed = fx.new_data.is_some();
    if fx.changed || data_changed {
        let before = st.session.clone();
        if let Some(s) = chain_of(st, target).and_then(|c| c.get_mut(i)) {
            s.values = fx.vals.clone();
            if let Some(d) = fx.new_data.clone() {
                s.data = d;
            }
        }
        if target != FILE {
            st.live_slot(target, i);
            let down = ui.input(|i| i.pointer.any_down());
            if !st.gesture || data_changed {
                st.commit_live(&crate::fx::name(&slot.effect), before);
                st.gesture = down && !data_changed;
            }
        }
    }
    if fx.released || data_changed {
        st.gesture = false;
        if target != FILE && (crate::fx::is_fx(&slot.effect) || data_changed) {
            st.session_version += 1;
        }
    }
}

// ------------------------------------------------------------ presets

fn presets_dir(id: &str) -> std::path::PathBuf {
    let safe: String = id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    crate::state::data_dir().join("effect-presets").join(safe)
}

fn preset_names(id: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(presets_dir(id))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension()? != "json" {
                return None;
            }
            Some(p.file_stem()?.to_string_lossy().into_owned())
        })
        .collect();
    v.sort();
    v
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Preset {
    effect: String,
    /// By key (or control name), so presets survive new controls.
    values: std::collections::BTreeMap<String, f32>,
}

/// The effect's presets: Default, the saved ones, and Save / Delete.
fn presets_row(ui: &mut egui::Ui, fx: &mut Fx, th: &Theme) {
    let name_id = fx.uid.with("preset-name");
    let mut name: String = ui.ctx().data(|d| d.get_temp(name_id)).unwrap_or_default();
    let names = preset_names(&fx.id);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Presets").color(c(th.text_dim)));
        egui::ComboBox::from_id_salt(fx.uid.with("presets"))
            .selected_text(if name.is_empty() { "(current)" } else { &name })
            .width(150.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(false, "Default").clicked() {
                    for k in 0..fx.ctls.len() {
                        let key = fx.ctls[k].key.clone();
                        let d = fx.ctls[k].default;
                        fx.set(&key, d);
                    }
                    name.clear();
                }
                for n in &names {
                    if ui.selectable_label(*n == name, n).clicked() {
                        let path = presets_dir(&fx.id).join(format!("{n}.json"));
                        if let Some(p) = std::fs::read_to_string(path)
                            .ok()
                            .and_then(|t| serde_json::from_str::<Preset>(&t).ok())
                        {
                            for (k, v) in p.values {
                                fx.set(&k, f64::from(v));
                            }
                            name = n.clone();
                        }
                    }
                }
            });
        ui.add(
            egui::TextEdit::singleline(&mut name)
                .hint_text("Name")
                .desired_width(90.0),
        );
        let clean: String = name
            .chars()
            .map(|ch| if r#"\/:*?"<>|"#.contains(ch) { '_' } else { ch })
            .collect::<String>()
            .trim()
            .to_string();
        if widgets::button(ui, "Save", false)
            .on_hover_text("Save these settings as a preset of this effect")
            .clicked()
            && !clean.is_empty()
        {
            let p = Preset {
                effect: fx.id.clone(),
                values: fx
                    .ctls
                    .iter()
                    .zip(&fx.vals)
                    .map(|(c, v)| (c.key.clone(), *v))
                    .collect(),
            };
            let dir = presets_dir(&fx.id);
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(t) = serde_json::to_string_pretty(&p) {
                let _ = std::fs::write(dir.join(format!("{clean}.json")), t);
            }
        }
        if names.contains(&clean)
            && widgets::button(ui, "Delete", false)
                .on_hover_text("Delete this saved preset")
                .clicked()
        {
            let _ = std::fs::remove_file(presets_dir(&fx.id).join(format!("{clean}.json")));
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(name_id, name));
}

// ------------------------------------------------------------ columns

/// Controls that only mean something on a speaker bed.
pub fn spatial_only(c: &Ctl) -> bool {
    c.key == "spread" || c.name.to_lowercase().contains("room spread")
}

/// The dial columns: a dial and value box per control, ungrouped controls
/// in rows, each group as a column with its On switch in the heading.
/// Picks are stepped switches; switches are lit lamps.
pub fn columns(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = &env.th;
    // (group, [control index]) in order.
    let mut cols: Vec<(String, Option<usize>, Vec<usize>)> = Vec::new();
    for (k, c) in fx.ctls.iter().enumerate() {
        if c.hidden || (spatial_only(c) && !env.spatial) {
            continue;
        }
        if cols.last().is_none_or(|(g, _, _)| *g != c.group) {
            cols.push((c.group.clone(), None, Vec::new()));
        }
        let col = cols.last_mut().unwrap();
        if !c.group.is_empty() && c.toggle && c.name == "On" && col.1.is_none() {
            col.1 = Some(k);
        } else {
            col.2.push(k);
        }
    }
    let cell = |ui: &mut egui::Ui, fx: &mut Fx, k: usize| {
        let c = fx.ctls[k].clone();
        if c.toggle {
            fx.toggle(ui, &c.key, "");
        } else if !c.choices.is_empty() {
            ui.vertical(|ui| {
                ui.label(RichText::new(&c.name).size(10.5).color(c_dim(th)));
                if c.choices.len() <= 4 && c.choices.iter().map(String::len).sum::<usize>() < 32 {
                    fx.choice(ui, &c.key, &[]);
                } else {
                    let cur = (f64::from(fx.vals[k]) - c.min).round().max(0.0) as usize;
                    let cur = cur.min(c.choices.len() - 1);
                    let mut pick = cur;
                    egui::ComboBox::from_id_salt(fx.uid.with(("pick", k)))
                        .selected_text(&c.choices[cur])
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            for (ci, n) in c.choices.iter().enumerate() {
                                ui.selectable_value(&mut pick, ci, n);
                            }
                        });
                    if pick != cur {
                        fx.set(&c.key, c.min + pick as f64);
                    }
                }
            });
        } else {
            fx.knob(ui, &c.key, 56.0, "", &[]);
        }
    };
    for (name, _, list) in &cols {
        if name.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.set_max_width(560.0);
                for k in list {
                    cell(ui, fx, *k);
                }
            });
        }
    }
    let banded: Vec<_> = cols.iter().filter(|(n, _, _)| !n.is_empty()).collect();
    if !banded.is_empty() {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.set_max_width(620.0);
            for (name, power, list) in banded {
                let off = power.is_some_and(|p| fx.vals[p] < 0.5);
                parts::module(ui, th, name, |ui| {
                    if let Some(p) = power {
                        let key = fx.ctls[*p].key.clone();
                        fx.toggle(ui, &key, "On");
                    }
                    ui.add_enabled_ui(!off, |ui| {
                        for k in list {
                            cell(ui, fx, *k);
                        }
                    });
                });
            }
        });
    }
}

fn c_dim(th: &Theme) -> egui::Color32 {
    c(th.text_dim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_panel_names_a_real_effect_and_its_controls_exist() {
        let all: Vec<String> = crate::fx::catalog()
            .into_iter()
            .flat_map(|(_, l)| l.into_iter().map(|(id, _)| id))
            .collect();
        for (id, _) in PANELS {
            assert!(all.iter().any(|a| a == id), "{id} is not an effect");
        }
        // Keys used by the panels resolve.
        let f = |id: &str, keys: &[&str]| {
            let c = ctls(id);
            for k in keys {
                assert!(
                    c.iter().any(|x| x.key == *k || x.name == *k),
                    "{id} has no {k}"
                );
            }
        };
        f(
            "compressor",
            &["Threshold", "Ratio", "Attack", "Release", "Knee", "Makeup"],
        );
        f(
            "limiter",
            &["Ceiling", "Input boost", "Look-ahead", "Release"],
        );
        f(
            "noise-gate",
            &["Threshold", "Attack", "Hold", "Release", "Floor"],
        );
        f("de-esser", &["Threshold", "Reduction"]);
        f("echo", &["Delay", "Feedback", "Mix"]);
        f(
            "reverb",
            &["Room size", "Damping", "Mix", "Width", "Pre-delay"],
        );
        f(
            "af.reverb",
            &[
                "kind",
                "size",
                "decay",
                "predelay",
                "early",
                "width",
                "damping",
                "diffusion",
                "modulation",
                "low_cut",
                "high_cut",
                "dry",
                "wet",
            ],
        );
        f(
            "af.convolution_reverb",
            &[
                "dry", "wet", "predelay", "length", "width", "low_cut", "high_cut",
            ],
        );
        f(
            "af.opto_leveler",
            &["peak", "gain", "mode", "emphasis", "tube", "mix", "meter"],
        );
        f(
            "af.tube_preamp",
            &[
                "input",
                "drive",
                "low",
                "high",
                "transformer",
                "output",
                "mix",
            ],
        );
        f(
            "af.vox_strip",
            &[
                "input",
                "output",
                "low_cut",
                "dyn_on",
                "dyn_style",
                "comp",
                "deess",
                "tone_on",
                "tone_style",
                "low",
                "mid",
                "high",
                "air",
                "fx_on",
                "fx_style",
                "fx",
            ],
        );
        f(
            "af.phase_curve",
            &[
                "points",
                "spline",
                "range",
                "channels",
                "fft",
                "p1_freq",
                "p16_phase",
            ],
        );
    }

    #[test]
    fn spatial_only_controls_are_found() {
        let c = ctls("af.modulation");
        assert!(c.iter().any(spatial_only));
        assert!(!ctls("compressor").iter().any(spatial_only));
    }
}
