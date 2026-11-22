// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! VST3 plug-ins in the app: the VST3 menus, plug-in slots' windows (the
//! plug-in's own window and generic controls), bringing what plug-ins
//! report into the session, and the Audio Plug-In Manager.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::RichText;

use af2_core::plugin::{is_plugin, new_inst, PluginData};
use af2_core::{EffectSlot, Session};
use af2_engine::MASTER;
use af2_vst3::live::{Instance, Notice, Status};
use af2_vst3::registry::ScanProgress;
use af2_vst3::{manager, Found};

use crate::state::{data_dir, State};
use crate::theme::{c, Theme};

/// The app window, which owns plug-in windows (an HWND).
static OWNER: AtomicU64 = AtomicU64::new(0);

/// Interface state that lives only on the UI thread.
#[derive(Default)]
struct Ui {
    manager_open: bool,
    scan: Option<Arc<ScanProgress>>,
    /// Edits from plug-in windows since the last undo step, and the
    /// session before them.
    pending: Option<(Session, Instant)>,
    synced_version: u64,
    filter: HashMap<String, String>,
    new_folder: String,
    /// `.vstpreset` files per plug-in, read when its window first shows.
    presets: HashMap<String, Vec<PathBuf>>,
}

thread_local! {
    static UI: RefCell<Ui> = RefCell::new(Ui::default());
}

/// Loads the plug-in list. Called once at start.
pub fn init() {
    manager().open_registry(data_dir().join("plugins.json"));
    // Puts the fixed plug-in host copy in place off the UI thread, so the
    // first plug-in or scan does not wait for the copy.
    std::thread::spawn(|| {
        let _ = manager().host_exe();
    });
}

/// Keeps the app window's handle, for plug-in windows to sit above it.
pub fn remember_window(frame: &eframe::Frame) {
    if OWNER.load(Ordering::Relaxed) != 0 {
        return;
    }
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(h) = frame.window_handle() {
        if let RawWindowHandle::Win32(h) = h.as_raw() {
            OWNER.store(h.hwnd.get() as u64, Ordering::Relaxed);
        }
    }
}

/// The app window's handle (an HWND), 0 before it is known.
pub fn owner() -> u64 {
    OWNER.load(Ordering::Relaxed)
}

// ------------------------------------------------------------ names

fn cid(effect: &str) -> &str {
    af2_vst3::cid_of(effect).unwrap_or("")
}

pub fn found(effect: &str) -> Option<Found> {
    manager().find(cid(effect))
}

/// A plug-in's name for menus and slots.
pub fn name(effect: &str) -> String {
    found(effect).map_or_else(|| "VST3 plug-in (not installed)".into(), |f| f.info.name)
}

/// A slot's name: the plug-in's, or the saved one with a note when it is
/// not installed here.
pub fn slot_name(s: &EffectSlot) -> String {
    match found(&s.effect) {
        Some(f) => f.info.name,
        None => {
            let d = s.plugin_data();
            if d.name.is_empty() {
                "VST3 plug-in (not installed)".into()
            } else {
                format!("{} (not installed)", d.name)
            }
        }
    }
}

pub fn takes_key(effect: &str) -> bool {
    found(effect).is_some_and(|f| f.info.sidechain)
}

/// A new slot for plug-in `effect` (`vst3:<id>`), with a running copy of
/// its own.
pub fn new_slot(effect: &str) -> EffectSlot {
    let f = found(effect);
    let d = PluginData {
        inst: new_inst(),
        name: f.as_ref().map(|f| f.info.name.clone()).unwrap_or_default(),
        vendor: f.as_ref().map(|f| f.maker()).unwrap_or_default(),
        ..Default::default()
    };
    EffectSlot {
        position: None,
        effect: effect.to_string(),
        values: Vec::new(),
        bypass: false,
        sidechain: None,
        data: Some(d.to_json()),
        ext_id: None,
    }
}

// ------------------------------------------------------------ menus

/// VST3 ▸ category ▸ maker ▸ plug-in. Returns the effect id picked.
pub fn picker(ui: &mut egui::Ui) -> Option<String> {
    let mut picked = None;
    ui.menu_button("VST3", |ui| picked = picker_items(ui));
    picked
}

/// The menu group a plug-in's own category goes in. Plug-ins name their
/// categories many ways ("Dynamics - Multiband", "Spatial + Panner",
/// "Channel Strip"); near-duplicates share one group so the menu stays
/// short. Channel strips go with Effects, as Adobe Audition files them.
pub fn menu_group(raw: &str) -> &'static str {
    let s = raw.to_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| s.contains(x));
    if has(&["dynamic", "drum", "compress", "limit", "gate", "transient"]) {
        "Dynamics"
    } else if has(&["spatial", "stereo", "sound field", "surround", "panner", "imag", "ambison"]) {
        "Spatial and Stereo"
    } else if has(&["channel strip", "effect"]) {
        "Effects"
    } else if has(&["eq", "filter"]) {
        "EQ and Filter"
    } else if has(&["distort", "guitar", "bass", "amp", "saturat"]) {
        "Distortion and Amps"
    } else if has(&["pitch", "time"]) {
        "Pitch and Time"
    } else if has(&["reverb"]) {
        "Reverb"
    } else if has(&["delay", "echo"]) {
        "Delay"
    } else if has(&["modulation", "chorus", "flanger", "phaser"]) {
        "Modulation"
    } else if has(&["restor", "noise", "repair"]) {
        "Restoration"
    } else if has(&["master"]) {
        "Mastering"
    } else if has(&["vocal", "voice"]) {
        "Vocals"
    } else if has(&["analy", "tool", "generator", "meter", "utility"]) {
        "Tools and Analyzers"
    } else {
        "Other"
    }
}

/// A menu list that scrolls instead of running off the bottom of the
/// screen.
pub fn tall_list<R>(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let h = (ui.ctx().screen_rect().height() - 80.0).max(160.0);
    egui::ScrollArea::vertical()
        .id_salt(salt)
        .max_height(h)
        .show(ui, add)
        .inner
}

/// The inside of the VST3 menu.
pub fn picker_items(ui: &mut egui::Ui) -> Option<String> {
    let mut picked = None;
    let tree: BTreeMap<&'static str, BTreeMap<String, Vec<(String, String)>>> = {
        let reg = manager().registry();
        let mut t: BTreeMap<&'static str, BTreeMap<String, Vec<(String, String)>>> =
            BTreeMap::new();
        for f in reg.effects() {
            t.entry(menu_group(&f.category()))
                .or_default()
                .entry(f.maker())
                .or_default()
                .push((
                    format!("{}{}", af2_vst3::PREFIX, f.info.cid),
                    f.info.name.clone(),
                ));
        }
        for makers in t.values_mut() {
            for list in makers.values_mut() {
                list.sort_by_key(|(_, n)| n.to_lowercase());
            }
        }
        t
    };
    if tree.is_empty() {
        ui.label("No VST3 plug-ins found yet.");
    }
    tall_list(ui, "vst3-groups", |ui| {
        for (cat, makers) in &tree {
            ui.menu_button(*cat, |ui| {
                tall_list(ui, ("vst3-makers", cat), |ui| {
                    for (maker, list) in makers {
                        ui.menu_button(maker, |ui| {
                            tall_list(ui, ("vst3-list", cat, maker), |ui| {
                                for (id, name) in list {
                                    if ui.button(name).clicked() {
                                        picked = Some(id.clone());
                                        ui.close_menu();
                                    }
                                }
                            });
                        });
                    }
                });
            });
        }
    });
    ui.separator();
    if ui.button("Audio Plug-In Manager...").clicked() {
        open_manager();
        ui.close_menu();
    }
    picked
}

pub fn open_manager() {
    UI.with(|u| u.borrow_mut().manager_open = true);
}

// ------------------------------------------------------------ slots

fn rate_of(st: &State) -> u32 {
    st.engine.as_ref().map_or(st.session.rate, |e| e.rate)
}

/// The running copy of a slot, started if need be.
fn instance(st: &State, slot: &EffectSlot) -> Option<Arc<Instance>> {
    let d = slot.plugin_data();
    manager()
        .get(&d.inst)
        .or_else(|| manager().live(slot, rate_of(st)))
}

fn slot_at(st: &State, target: u16, i: usize) -> Option<EffectSlot> {
    let chain = match target {
        crate::rack::FILE => &st.file_chain,
        MASTER => &st.session.master_chain,
        t if crate::rack::is_clip(t) => crate::rack::clip_chain(st, t)?,
        t => &st.session.tracks.get(t as usize)?.chain,
    };
    chain.get(i).cloned()
}

/// Opens a plug-in slot's own window (when it has one).
pub fn open_editor(st: &State, target: u16, i: usize) {
    let Some(slot) = slot_at(st, target, i) else {
        return;
    };
    if !is_plugin(&slot.effect) || found(&slot.effect).is_some_and(|f| !f.info.has_editor) {
        return;
    }
    let Some(inst) = instance(st, &slot) else {
        return;
    };
    let title = format!(
        "{}: {}",
        crate::rack::target_name(st, target),
        slot_name(&slot)
    );
    inst.open_editor(&title, OWNER.load(Ordering::Relaxed), slot.bypass);
}

/// A slot's power changed: the plug-in's window shows it.
pub fn show_bypassed(st: &State, target: u16, i: usize) {
    if let Some(slot) = slot_at(st, target, i).filter(|s| is_plugin(&s.effect)) {
        if let Some(inst) = manager().get(&slot.plugin_data().inst) {
            inst.show_bypassed(slot.bypass);
        }
    }
}

/// A plug-in slot's effect window: its own window, its state, and a
/// control for every parameter.
pub fn slot_controls(ui: &mut egui::Ui, st: &mut State, target: u16, i: usize, th: &Theme) {
    let Some(slot) = slot_at(st, target, i) else {
        return;
    };
    let Some(f) = found(&slot.effect) else {
        ui.label(
            RichText::new(
                "This plug-in is not installed here (or not scanned yet). It stays in the chain with its settings and is skipped when playing.",
            )
            .color(c(th.text_dim)),
        );
        if ui.button("Audio Plug-In Manager...").clicked() {
            open_manager();
        }
        return;
    };
    let inst = instance(st, &slot);
    let status = inst.as_ref().map(|i| i.status());
    ui.horizontal(|ui| {
        ui.label(RichText::new(f.maker()).color(c(th.text_dim)));
        match &status {
            Some(Status::Running) => {
                ui.label(RichText::new("Running").color(c(th.accent)));
            }
            Some(Status::Starting) => {
                ui.spinner();
                ui.label("Starting...");
                ui.ctx().request_repaint_after(Duration::from_millis(200));
            }
            Some(Status::Stopped(_)) | None => {
                ui.label(RichText::new("Stopped").color(egui::Color32::from_rgb(230, 90, 80)));
            }
        }
    });
    if let Some(Status::Stopped(why)) = &status {
        ui.label(RichText::new(why).color(c(th.text_dim)));
        if ui
            .button("Restart")
            .on_hover_text("The plug-in starts again with the next playback")
            .clicked()
        {
            manager().restart(&slot.plugin_data().inst);
            st.session_version += 1;
            st.rebuild_program();
        }
    }
    ui.horizontal(|ui| {
        let open = inst
            .as_ref()
            .is_some_and(|i| i.editor_open.load(Ordering::Relaxed));
        if f.info.has_editor {
            if ui
                .add_enabled(
                    !matches!(status, Some(Status::Stopped(_))),
                    egui::Button::new("Show plug-in editor"),
                )
                .clicked()
            {
                open_editor(st, target, i);
            }
            if open && ui.button("Close it").clicked() {
                if let Some(inst) = &inst {
                    inst.close_editor();
                }
            }
        } else {
            ui.label(RichText::new("No window of its own").color(c(th.text_dim)));
        }
    });
    presets_row(ui, st, target, i, &slot, &f, inst.as_ref());
    // The plug-in's own window is the main way in; the generic list waits
    // under a closed heading (open at once for plug-ins with no window).
    egui::CollapsingHeader::new(RichText::new("Controls").color(c(th.text_dim)))
        .id_salt(("vst-controls", target, i))
        .default_open(!f.info.has_editor)
        .show(ui, |ui| {
            generic_controls(ui, st, target, i, th, &slot, &f, &inst)
        });
}

/// A plug-in slot's generic controls: its delay and one control per
/// parameter, with a search box for long lists.
#[allow(clippy::too_many_arguments)]
fn generic_controls(
    ui: &mut egui::Ui,
    st: &mut State,
    target: u16,
    i: usize,
    th: &Theme,
    slot: &EffectSlot,
    f: &Found,
    inst: &Option<Arc<Instance>>,
) {
    if let Some(inst) = inst {
        let own = inst.plugin_latency();
        let text = if inst.offline || inst.pipeline == 0 {
            format!("Delay: {own} samples")
        } else {
            format!(
                "Delay: {own} samples, plus {} buffered while playing",
                inst.pipeline
            )
        };
        ui.label(RichText::new(text).size(10.5).color(c(th.text_dim)));
    }
    // Generic controls (every plug-in has them; they also reach plug-ins
    // with no window, and stopped ones keep their values).
    let params: Vec<_> = inst
        .as_ref()
        .map_or_else(|| f.info.params.clone(), |i| i.params())
        .into_iter()
        .filter(|p| !p.hidden)
        .collect();
    if params.is_empty() {
        ui.label(RichText::new("No controls to show.").color(c(th.text_dim)));
        return;
    }
    let key = slot.plugin_data().inst;
    let mut filter = UI.with(|u| u.borrow().filter.get(&key).cloned().unwrap_or_default());
    if params.len() > 12 {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Find").color(c(th.text_dim)));
            ui.add(egui::TextEdit::singleline(&mut filter).desired_width(160.0));
            ui.label(
                RichText::new(format!("{} controls", params.len()))
                    .size(10.5)
                    .color(c(th.text_dim)),
            );
        });
        UI.with(|u| u.borrow_mut().filter.insert(key.clone(), filter.clone()));
    }
    let needle = filter.to_lowercase();
    let d = slot.plugin_data();
    let before = st.session.clone();
    let mut changed: Option<(u32, f64)> = None;
    let mut released = false;
    egui::ScrollArea::vertical()
        .id_salt(("vst-params", target, i))
        .max_height(360.0)
        .show(ui, |ui| {
            let mut shown = 0;
            for p in &params {
                if !needle.is_empty() && !p.title.to_lowercase().contains(&needle) {
                    continue;
                }
                shown += 1;
                if shown > 400 {
                    ui.label(
                        RichText::new("More controls: narrow the search.").color(c(th.text_dim)),
                    );
                    break;
                }
                let cur = inst
                    .as_ref()
                    .and_then(|i| i.value(p.id))
                    .or_else(|| d.param(p.id))
                    .unwrap_or(p.default);
                let r = if p.steps == 1 {
                    let mut on = cur >= 0.5;
                    let r = ui.checkbox(&mut on, &p.title);
                    if r.changed() {
                        changed = Some((p.id, if on { 1.0 } else { 0.0 }));
                        released = true;
                    }
                    r
                } else {
                    let mut v = cur as f32;
                    let mut s = crate::widgets::Slider::new(&mut v, 0.0..=1.0).text(&p.title);
                    if p.steps > 1 {
                        s = s.step_by(1.0 / p.steps as f64);
                    }
                    if !p.units.is_empty() {
                        s = s.suffix(format!(" ({})", p.units));
                    }
                    let r = ui.add(s);
                    if r.changed() {
                        changed = Some((p.id, v as f64));
                    }
                    released |= r.drag_stopped() || (r.changed() && !r.dragged());
                    r
                };
                let _ = r;
            }
        });
    if let Some((id, v)) = changed {
        if let Some(inst) = inst {
            inst.set_param(id, v);
        }
        if let Some(s) = crate::rack::chain_of(st, target).and_then(|c| c.get_mut(i)) {
            let mut d = s.plugin_data();
            d.set_param(id, v);
            s.set_plugin_data(&d);
        }
        if target != crate::rack::FILE {
            let down = ui.input(|i| i.pointer.any_down());
            if !st.gesture {
                st.commit_live(&f.info.name, before);
                st.gesture = down;
            }
        }
    }
    if released {
        st.gesture = false;
    }
}

/// Loads a `.vstpreset` into a slot (undoable) and the running plug-in.
fn load_preset(st: &mut State, target: u16, i: usize, path: &std::path::Path) {
    let Some(slot) = slot_at(st, target, i) else {
        return;
    };
    match af2_vst3::presets::read(path) {
        Ok((pcid, blob)) if af2_vst3::presets::matches(&pcid, cid(&slot.effect)) => {
            let b64 = af2_vst3::protocol::base64_encode(&blob);
            let before = st.session.clone();
            edit_slot(st, target, i, |d| d.state = Some(b64.clone()));
            if target != crate::rack::FILE {
                st.commit_live("Load Preset", before);
            }
            if let Some(inst) = manager().get(&slot.plugin_data().inst) {
                inst.push_state(&b64);
            }
        }
        Ok(_) => st.toast("That preset is for another plug-in."),
        Err(e) => st.toast(format!("The preset could not be read: {e}")),
    }
}

/// A plug-in's `.vstpreset` files (read once, then kept).
fn preset_list(f: &Found) -> Vec<PathBuf> {
    let id = f.info.cid.clone();
    UI.with(|u| {
        u.borrow_mut()
            .presets
            .entry(id.clone())
            .or_insert_with(|| af2_vst3::presets::list(&id))
            .clone()
    })
}

/// Asks for a `.vstpreset` to load, starting in the plug-in's folder.
fn pick_preset(f: &Found) -> Option<PathBuf> {
    let mut dlg =
        crate::file_dialog::FileDialog::new().add_filter("VST3 preset", &["vstpreset"]);
    if let Some(d) = af2_vst3::presets::user_folder(&f.maker(), &f.info.name) {
        dlg = dlg.set_directory(d);
    }
    dlg.pick_file()
}

/// Saves a slot's plug-in settings as a `.vstpreset`, asking where.
fn save_preset(st: &mut State, slot: &EffectSlot, f: &Found, inst: Option<&Arc<Instance>>) {
    let state = inst
        .and_then(|x| x.state_now())
        .or_else(|| slot.plugin_data().state);
    match state.and_then(|s| af2_vst3::protocol::base64_decode(&s)) {
        Some(blob) => {
            let mut dlg = crate::file_dialog::FileDialog::new()
                .add_filter("VST3 preset", &["vstpreset"])
                .set_file_name(format!("{}.vstpreset", f.info.name));
            if let Some(d) = af2_vst3::presets::user_folder(&f.maker(), &f.info.name) {
                let _ = std::fs::create_dir_all(&d);
                dlg = dlg.set_directory(d);
            }
            if let Some(p) = dlg.save_file() {
                match af2_vst3::presets::save(&p, &f.info.cid, &blob) {
                    Ok(()) => {
                        UI.with(|u| u.borrow_mut().presets.remove(&f.info.cid));
                    }
                    Err(e) => st.toast(format!("The preset could not be saved: {e}")),
                }
            }
        }
        None => st.toast("The plug-in has no settings to save yet."),
    }
}

fn presets_row(
    ui: &mut egui::Ui,
    st: &mut State,
    target: u16,
    i: usize,
    slot: &EffectSlot,
    f: &Found,
    inst: Option<&Arc<Instance>>,
) {
    let list = preset_list(f);
    let mut load: Option<PathBuf> = None;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt(("vst-preset", target, i))
            .selected_text("Presets")
            .width(150.0)
            .show_ui(ui, |ui| {
                if list.is_empty() {
                    ui.label("No presets found.");
                }
                for p in &list {
                    let name = p
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if ui.selectable_label(false, name).clicked() {
                        load = Some(p.clone());
                    }
                }
            });
        if ui.small_button("Load...").clicked() {
            load = pick_preset(f);
        }
        if ui
            .small_button("Save...")
            .on_hover_text("Save the plug-in's settings as a .vstpreset")
            .clicked()
        {
            save_preset(st, slot, f, inst);
        }
    });
    if let Some(p) = load {
        load_preset(st, target, i, &p);
    }
}

/// True when opening this slot opens the plug-in's own window and nothing
/// else: it has a window and is not stopped. Otherwise the slot opens
/// Audiform's window for it (generic controls, or why it is stopped).
pub fn opens_own_window(st: &State, target: u16, i: usize) -> bool {
    let Some(slot) = slot_at(st, target, i).filter(|s| is_plugin(&s.effect)) else {
        return false;
    };
    if !found(&slot.effect).is_some_and(|f| f.info.has_editor) {
        return false;
    }
    instance(st, &slot).is_some_and(|inst| !matches!(inst.status(), Status::Stopped(_)))
}

/// A plug-in slot's own items for its right-click and arrow menus:
/// presets, and Audiform's controls window.
pub fn slot_menu(ui: &mut egui::Ui, st: &mut State, target: u16, i: usize) {
    let Some(slot) = slot_at(st, target, i) else {
        return;
    };
    let Some(f) = found(&slot.effect) else {
        return;
    };
    let mut load: Option<PathBuf> = None;
    ui.menu_button("Presets", |ui| {
        let list = preset_list(&f);
        if list.is_empty() {
            ui.label("No presets found.");
        }
        tall_list(ui, ("vst-preset-menu", target, i), |ui| {
            for p in &list {
                let name = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if ui.button(name).clicked() {
                    load = Some(p.clone());
                    ui.close_menu();
                }
            }
        });
    });
    if ui.button("Load Preset...").clicked() {
        ui.close_menu();
        load = pick_preset(&f);
    }
    if ui
        .button("Save Preset...")
        .on_hover_text("Save the plug-in's settings as a .vstpreset")
        .clicked()
    {
        ui.close_menu();
        let inst = manager().get(&slot.plugin_data().inst);
        save_preset(st, &slot, &f, inst.as_ref());
    }
    if let Some(p) = load {
        load_preset(st, target, i, &p);
    }
    if f.info.has_editor
        && ui
            .button("Generic Controls...")
            .on_hover_text("Audiform's window for this plug-in: a control per parameter")
            .clicked()
    {
        ui.close_menu();
        if !st.fx_windows.contains(&(target, i)) {
            st.fx_windows.push((target, i));
        }
    }
}

// ------------------------------------------------------------ sync

/// Where each running copy's slot is: inst -> (target, slot).
fn slots_by_inst(st: &State) -> HashMap<String, (u16, usize)> {
    let mut m = HashMap::new();
    let mut add = |target: u16, chain: &[EffectSlot]| {
        for (i, s) in chain.iter().enumerate() {
            if is_plugin(&s.effect) {
                m.insert(s.plugin_data().inst, (target, i));
            }
        }
    };
    for (ti, t) in st.session.tracks.iter().enumerate() {
        add(ti as u16, &t.chain);
        for c in t.clips.iter().filter(|c| !c.props.chain.is_empty()) {
            add(crate::rack::clip_target(st, c.id), &c.props.chain);
        }
    }
    add(MASTER, &st.session.master_chain);
    add(crate::rack::FILE, &st.file_chain);
    m
}

/// Gives a copy of a slot (pasted, a preset used twice) a running copy of
/// its own. True when anything changed.
fn unique_insts(chains: &mut [&mut Vec<EffectSlot>]) -> bool {
    let mut seen = HashSet::new();
    let mut changed = false;
    for chain in chains.iter_mut() {
        for s in chain.iter_mut().filter(|s| is_plugin(&s.effect)) {
            let mut d = s.plugin_data();
            if d.inst.is_empty() || !seen.insert(d.inst.clone()) {
                d.inst = new_inst();
                seen.insert(d.inst.clone());
                s.set_plugin_data(&d);
                changed = true;
            }
        }
    }
    changed
}

/// Every frame: what plug-ins reported goes into the session; after an
/// edit or undo, running plug-ins catch up with the session.
pub fn tick(st: &mut State) {
    manager().set_device_buffer(st.engine.as_ref().and_then(|e| e.buffer));
    // What plug-ins reported goes in first, so the catch-up below never
    // sends a plug-in a value older than one it just reported (a knob
    // turned in its window would jump back).
    take_notices(st);
    let version = st.session_version;
    if UI.with(|u| u.borrow().synced_version) != version {
        UI.with(|u| u.borrow_mut().synced_version = version);
        let mut chains: Vec<&mut Vec<EffectSlot>> = Vec::new();
        let (tracks, master) = (&mut st.session.tracks, &mut st.session.master_chain);
        for t in tracks.iter_mut() {
            chains.push(&mut t.chain);
            for c in t.clips.iter_mut() {
                chains.push(&mut c.props.chain);
            }
        }
        chains.push(master);
        chains.push(&mut st.file_chain);
        if unique_insts(&mut chains) {
            st.session_version += 1;
        }
        // Running copies follow the session (undo, presets).
        let mut keep = HashSet::new();
        let all: Vec<&EffectSlot> = st
            .session
            .tracks
            .iter()
            .flat_map(|t| {
                t.chain
                    .iter()
                    .chain(t.clips.iter().flat_map(|c| c.props.chain.iter()))
            })
            .chain(st.session.master_chain.iter())
            .chain(st.file_chain.iter())
            .collect();
        for s in all.into_iter().filter(|s| is_plugin(&s.effect)) {
            let d = s.plugin_data();
            if let Some(inst) = manager().get(&d.inst) {
                inst.sync(&d);
            }
            keep.insert(d.inst);
        }
        for p in &st.sessions.parked {
            for s in p
                .session
                .tracks
                .iter()
                .flat_map(|t| {
                    t.chain
                        .iter()
                        .chain(t.clips.iter().flat_map(|c| c.props.chain.iter()))
                })
                .chain(p.session.master_chain.iter())
            {
                if is_plugin(&s.effect) {
                    keep.insert(s.plugin_data().inst);
                }
            }
        }
        // MIDI tracks' instruments (and the keys' copies).
        crate::instruments::keep(st, &mut keep);
        manager().retain(&keep);
    }
}

fn take_notices(st: &mut State) {
    let notices = manager().take_notices();
    let idle_commit = UI.with(|u| {
        u.borrow()
            .pending
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() > Duration::from_millis(700))
    });
    if notices.is_empty() {
        if idle_commit {
            commit_pending(st);
        }
        return;
    }
    let at = slots_by_inst(st);
    let mut commit = false;
    let mut rebuild = false;
    for (inst, n) in notices {
        let place = at.get(&inst).copied();
        // A MIDI track's instrument (only playback's copy reports edits).
        let instrument = if place.is_none() {
            crate::instruments::track_of(st, &inst)
        } else {
            None
        };
        // Only changes made in the plug-in's own window are the user's
        // edits (an undo step, unsaved changes). A plug-in reporting its
        // settings as it loads just updates the slot quietly.
        let by_user = manager()
            .get(&inst)
            .is_some_and(|i| i.editor_open.load(std::sync::atomic::Ordering::Relaxed));
        match n {
            Notice::Edited { id, value } => {
                if let Some((t, i)) = place {
                    if by_user {
                        begin_pending(st);
                    }
                    edit_slot(st, t, i, |d| d.set_param(id, value));
                } else if let Some(ti) = instrument.filter(|_| !inst.ends_with(af2_vst3::live::KEYS)) {
                    if by_user {
                        begin_pending(st);
                    }
                    crate::instruments::edit(st, ti, |d| d.set_param(id, value));
                }
            }
            Notice::State(s) => {
                if let Some((t, i)) = place {
                    if by_user {
                        begin_pending(st);
                        commit = true;
                    }
                    edit_slot(st, t, i, |d| d.state = Some(s.clone()));
                } else if let Some(ti) = instrument.filter(|_| !inst.ends_with(af2_vst3::live::KEYS)) {
                    if by_user {
                        begin_pending(st);
                        commit = true;
                    }
                    crate::instruments::edit(st, ti, |d| d.state = Some(s.clone()));
                }
            }
            Notice::EndEdit(_) | Notice::EditorClosed => commit = true,
            Notice::BypassPicked(b) => {
                if let Some((t, i)) = place {
                    let before = st.session.clone();
                    if let Some(s) = crate::rack::chain_of(st, t).and_then(|c| c.get_mut(i)) {
                        s.bypass = b;
                    }
                    if t != crate::rack::FILE {
                        st.live_slot(t, i);
                        st.commit_live(if b { "Bypass Effect" } else { "Turn On Effect" }, before);
                    }
                }
            }
            Notice::Stopped(why) => {
                if place.is_some() {
                    st.toast(format!(
                        "A plug-in stopped: {why} Restart it from its effect window."
                    ));
                } else if let Some(ti) = instrument {
                    let track = st.session.tracks[ti].name.clone();
                    st.toast(format!(
                        "The instrument on \"{track}\" stopped: {why} The track is silent; restart it from its instrument menu."
                    ));
                }
            }
            Notice::Error(e) => st.toast(e),
            Notice::Space(record) => {
                if record {
                    if st.is_recording() {
                        st.stop_recording();
                    } else {
                        st.start_recording();
                    }
                } else {
                    st.toggle_play();
                }
            }
            // A copy that finished starting (or changed its latency) gets
            // a fresh build of the mix: the chain was built while it was
            // starting, with the scan's latency, and the delay
            // compensation follows the real one. A MIDI keyboard's copy
            // plays outside the mix.
            Notice::Started | Notice::Latency(_) => {
                if !inst.ends_with(af2_vst3::live::KEYS) {
                    rebuild = true;
                }
            }
            Notice::BeginEdit(_) => {}
        }
    }
    if rebuild {
        st.rebuild_program();
    }
    if commit {
        commit_pending(st);
    }
}

fn begin_pending(st: &State) {
    UI.with(|u| {
        let mut u = u.borrow_mut();
        match &mut u.pending {
            Some((_, t)) => *t = Instant::now(),
            None => u.pending = Some((st.session.clone(), Instant::now())),
        }
    });
}

fn commit_pending(st: &mut State) {
    if let Some((before, _)) = UI.with(|u| u.borrow_mut().pending.take()) {
        st.commit_live("Plug-in Settings", before);
    }
}

fn edit_slot(st: &mut State, target: u16, i: usize, f: impl FnOnce(&mut PluginData)) {
    if let Some(s) = crate::rack::chain_of(st, target).and_then(|c| c.get_mut(i)) {
        let mut d = s.plugin_data();
        f(&mut d);
        s.set_plugin_data(&d);
        if target != crate::rack::FILE {
            st.session_dirty = true;
        }
    }
}

/// Before saving: every running plug-in's settings into its slot.
pub fn capture_all(st: &mut State) {
    let at = slots_by_inst(st);
    for (inst, (t, i)) in at {
        let Some(live) = manager().get(&inst).filter(|l| l.running()) else {
            continue;
        };
        if let Some(s) = live.state_now() {
            edit_slot(st, t, i, |d| {
                if d.state.as_deref() != Some(&s) {
                    d.state = Some(s);
                }
            });
        }
    }
    crate::instruments::capture(st);
}

// ------------------------------------------------------------ manager

/// Plug-in windows of the app (the Plug-In Manager).
pub fn windows(ctx: &egui::Context, st: &mut State) {
    let open = UI.with(|u| u.borrow().manager_open);
    if !open {
        return;
    }
    let th = st.theme.clone();
    let mut keep = true;
    crate::ui::popup(
        ctx,
        egui::Id::new(("af2-popup", "plugins.rs", 0)),
        RichText::new("Audio Plug-In Manager").strong(),
    )
    .id(egui::Id::new("plugin-manager"))
    .open(&mut keep)
    .default_width(560.0)
    .default_height(520.0)
    .resizable(true)
    .show(ctx, |ui| manager_ui(ui, st, &th));
    if !keep {
        UI.with(|u| u.borrow_mut().manager_open = false);
    }
}

fn start_scan(all: bool) {
    let progress = Arc::new(ScanProgress::default());
    UI.with(|u| u.borrow_mut().scan = Some(progress.clone()));
    let m = manager();
    let bundles = {
        let mut reg = m.registry();
        reg.forget_missing();
        reg.due(all)
    };
    let exe = m.host_exe();
    std::thread::spawn(move || {
        af2_vst3::registry::scan_into(m.registry_mutex(), &exe, bundles, &progress);
        let _ = m.save_registry();
    });
}

fn manager_ui(ui: &mut egui::Ui, st: &mut State, th: &Theme) {
    let scan = UI.with(|u| u.borrow().scan.clone());
    let running = scan
        .as_ref()
        .is_some_and(|s| !s.finished.load(Ordering::SeqCst));
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!running, egui::Button::new("Scan for Plug-Ins"))
            .on_hover_text("Reads new and changed plug-ins")
            .clicked()
        {
            start_scan(false);
        }
        if ui
            .add_enabled(!running, egui::Button::new("Rescan All"))
            .clicked()
        {
            start_scan(true);
        }
        if running && ui.button("Stop").clicked() {
            if let Some(s) = &scan {
                s.stop.store(true, Ordering::SeqCst);
            }
        }
    });
    if let Some(s) = &scan {
        let done = s.done.load(Ordering::SeqCst);
        let total = s.total.load(Ordering::SeqCst).max(1);
        if running {
            ui.add(
                egui::ProgressBar::new(done as f32 / total as f32)
                    .text(format!("{done} of {total}: {}", s.current.lock().unwrap())),
            );
            ui.ctx().request_repaint_after(Duration::from_millis(150));
        } else {
            ui.label(RichText::new(format!("Scanned {done} bundles.")).color(c(th.text_dim)));
        }
    }
    if manager().registry().stale && !running {
        ui.label(
            RichText::new("The plug-in list is from an older version: scan again.")
                .color(c(th.accent_alt)),
        );
    }
    ui.separator();
    ui.label(RichText::new("Folders").strong());
    let mut reg_changed = false;
    {
        let mut reg = manager().registry();
        for f in af2_vst3::registry::standard_folders() {
            let mut on = !reg.skipped.contains(&f);
            if ui.checkbox(&mut on, f.display().to_string()).changed() {
                reg.skipped.retain(|x| *x != f);
                if !on {
                    reg.skipped.push(f.clone());
                }
                reg_changed = true;
            }
        }
        let mut remove = None;
        for (k, f) in reg.folders.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(f.display().to_string());
                if ui.small_button("Remove").clicked() {
                    remove = Some(k);
                }
            });
        }
        if let Some(k) = remove {
            reg.folders.remove(k);
            reg_changed = true;
        }
    }
    ui.horizontal(|ui| {
        if ui.button("Add Folder...").clicked() {
            if let Some(p) = crate::file_dialog::FileDialog::new().pick_folder() {
                let mut reg = manager().registry();
                if !reg.folders.contains(&p) {
                    reg.folders.push(p);
                    reg_changed = true;
                }
            }
        }
        let mut typed = UI.with(|u| u.borrow().new_folder.clone());
        ui.add(
            egui::TextEdit::singleline(&mut typed)
                .hint_text("or type a folder")
                .desired_width(220.0),
        );
        if ui.small_button("Add").clicked() && !typed.trim().is_empty() {
            let p = PathBuf::from(typed.trim());
            let mut reg = manager().registry();
            if !reg.folders.contains(&p) {
                reg.folders.push(p);
                reg_changed = true;
            }
            typed.clear();
        }
        UI.with(|u| u.borrow_mut().new_folder = typed);
    });
    ui.separator();
    // The list, by maker.
    let (by_maker, failed, total) = {
        let reg = manager().registry();
        let mut m: BTreeMap<String, Vec<Found>> = BTreeMap::new();
        for f in &reg.plugins {
            m.entry(f.maker()).or_default().push(f.clone());
        }
        (m, reg.failed.clone(), reg.plugins.len())
    };
    ui.label(RichText::new(format!("{total} plug-ins")).strong());
    egui::ScrollArea::vertical()
        .id_salt("plugin-list")
        .max_height(ui.available_height() - 8.0)
        .show(ui, |ui| {
            for (maker, list) in &by_maker {
                let header = egui::CollapsingHeader::new(format!("{maker} ({})", list.len()))
                    .id_salt(("maker", maker));
                header.show(ui, |ui| {
                    let mut reg = manager().registry();
                    let mut together = !reg.runs_separately(maker);
                    if ui
                        .checkbox(&mut together, "Run together")
                        .on_hover_text(
                            "This maker's plug-ins share one process (less memory). Untick to give each its own, so one that crashes stops only itself.",
                        )
                        .changed()
                    {
                        reg.set_separate(maker, !together);
                        drop(reg);
                        // Running ones start again the new way.
                        crate::plugins::capture_all(st);
                        manager().drop_maker(maker);
                        st.session_version += 1;
                        reg_changed = true;
                        reg = manager().registry();
                    }
                    for f in list {
                        ui.horizontal(|ui| {
                            let mut on = !reg.is_disabled(&f.info.cid);
                            if ui.checkbox(&mut on, &f.info.name).changed() {
                                reg.set_disabled(&f.info.cid, !on);
                                reg_changed = true;
                            }
                            let kind = if f.info.instrument {
                                "instrument"
                            } else {
                                "effect"
                            };
                            ui.label(
                                RichText::new(format!("{} · {kind}", f.category()))
                                    .size(10.5)
                                    .color(c(th.text_dim)),
                            );
                        });
                        if let Some(n) = &f.info.note {
                            ui.label(RichText::new(n).size(10.5).color(c(th.accent_alt)));
                        }
                    }
                });
            }
            if !failed.is_empty() {
                ui.separator();
                ui.label(RichText::new("Could not be read").strong());
                for f in &failed {
                    ui.label(
                        RichText::new(format!("{}: {}", f.bundle.display(), f.error))
                            .size(10.5)
                            .color(c(th.text_dim)),
                    );
                }
            }
        });
    if reg_changed {
        let _ = manager().save_registry();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_duplicate_categories_share_a_menu_group() {
        for c in ["Dynamics", "Dynamics - Multiband", "Drums"] {
            assert_eq!(menu_group(c), "Dynamics", "{c}");
        }
        for c in ["Spatial", "Spatial + Panner", "Stereo", "Sound Field", "Surround"] {
            assert_eq!(menu_group(c), "Spatial and Stereo", "{c}");
        }
        assert_eq!(menu_group("Channel Strip"), "Effects");
        assert_eq!(menu_group("Effects"), "Effects");
        assert_eq!(menu_group("EQ"), "EQ and Filter");
        assert_eq!(menu_group("Filter"), "EQ and Filter");
        assert_eq!(menu_group("Pitch Shift"), "Pitch and Time");
        assert_eq!(menu_group("Time"), "Pitch and Time");
        assert_eq!(menu_group("Analyzer"), "Tools and Analyzers");
        assert_eq!(menu_group("Delay"), "Delay");
        assert_eq!(menu_group("Reverb"), "Reverb");
        // The user's 29 categories fit in far fewer groups.
        let raw = [
            "Analyzer", "Bass", "Channel Strip", "Delay", "Distortion", "Drums", "Dynamics",
            "Dynamics - Multiband", "EQ", "Effects", "Filter", "Generator", "Guitar",
            "Mastering", "Modulation", "OnlyARA", "Other", "Pitch Shift", "Restoration",
            "Reverb", "Sound Field", "Spatial", "Spatial + Panner", "Stereo",
            "StudioRack Local-DSPBased", "Surround", "Time", "Tools", "Vocals",
        ];
        let groups: HashSet<&str> = raw.iter().map(|c| menu_group(c)).collect();
        assert!(groups.len() <= 14, "{groups:?}");
    }

    #[test]
    fn copies_of_a_plug_in_slot_get_their_own_running_copy() {
        let mut a = vec![new_slot("vst3:00112233445566778899AABBCCDDEEFF")];
        let mut b = a.clone();
        let mut chains = vec![&mut a, &mut b];
        assert!(unique_insts(&mut chains));
        assert_ne!(a[0].plugin_data().inst, b[0].plugin_data().inst);
        let mut chains = vec![&mut a, &mut b];
        assert!(!unique_insts(&mut chains), "already unique");
        // Other effects are left alone.
        let mut c = vec![crate::rack::new_slot("compressor")];
        assert!(!unique_insts(&mut [&mut c]));
        assert!(c[0].data.is_none());
    }
}
