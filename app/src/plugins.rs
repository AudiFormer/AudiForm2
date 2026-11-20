// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

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

use af27_core::plugin::{is_plugin, new_inst, PluginData};
use af27_core::{EffectSlot, Session};
use af27_engine::MASTER;
use af27_vst3::live::{Instance, Notice, Status};
use af27_vst3::registry::ScanProgress;
use af27_vst3::{manager, Found};

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

// ------------------------------------------------------------ names

fn cid(effect: &str) -> &str {
    af27_vst3::cid_of(effect).unwrap_or("")
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

/// The inside of the VST3 menu.
pub fn picker_items(ui: &mut egui::Ui) -> Option<String> {
    let mut picked = None;
    let tree: BTreeMap<String, BTreeMap<String, Vec<(String, String)>>> = {
        let reg = manager().registry();
        let mut t: BTreeMap<String, BTreeMap<String, Vec<(String, String)>>> = BTreeMap::new();
        for f in reg.effects() {
            t.entry(f.category())
                .or_default()
                .entry(f.maker())
                .or_default()
                .push((
                    format!("{}{}", af27_vst3::PREFIX, f.info.cid),
                    f.info.name.clone(),
                ));
        }
        t
    };
    if tree.is_empty() {
        ui.label("No VST3 plug-ins found yet.");
    }
    for (cat, makers) in &tree {
        ui.menu_button(cat, |ui| {
            for (maker, list) in makers {
                ui.menu_button(maker, |ui| {
                    for (id, name) in list {
                        if ui.button(name).clicked() {
                            picked = Some(id.clone());
                            ui.close_menu();
                        }
                    }
                });
            }
        });
    }
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
    if let Some(inst) = &inst {
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
    ui.separator();
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
        if let Some(inst) = &inst {
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
    match af27_vst3::presets::read(path) {
        Ok((pcid, blob)) if af27_vst3::presets::matches(&pcid, cid(&slot.effect)) => {
            let b64 = af27_vst3::protocol::base64_encode(&blob);
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

fn presets_row(
    ui: &mut egui::Ui,
    st: &mut State,
    target: u16,
    i: usize,
    slot: &EffectSlot,
    f: &Found,
    inst: Option<&Arc<Instance>>,
) {
    let id = f.info.cid.clone();
    let list = UI.with(|u| {
        u.borrow_mut()
            .presets
            .entry(id.clone())
            .or_insert_with(|| af27_vst3::presets::list(&id))
            .clone()
    });
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
            let mut dlg = rfd::FileDialog::new().add_filter("VST3 preset", &["vstpreset"]);
            if let Some(d) = af27_vst3::presets::user_folder(&f.maker(), &f.info.name) {
                dlg = dlg.set_directory(d);
            }
            load = dlg.pick_file();
        }
        if ui
            .small_button("Save...")
            .on_hover_text("Save the plug-in's settings as a .vstpreset")
            .clicked()
        {
            let state = inst
                .and_then(|x| x.state_now())
                .or_else(|| slot.plugin_data().state);
            match state.and_then(|s| af27_vst3::protocol::base64_decode(&s)) {
                Some(blob) => {
                    let mut dlg = rfd::FileDialog::new()
                        .add_filter("VST3 preset", &["vstpreset"])
                        .set_file_name(format!("{}.vstpreset", f.info.name));
                    if let Some(d) = af27_vst3::presets::user_folder(&f.maker(), &f.info.name) {
                        let _ = std::fs::create_dir_all(&d);
                        dlg = dlg.set_directory(d);
                    }
                    if let Some(p) = dlg.save_file() {
                        match af27_vst3::presets::save(&p, &f.info.cid, &blob) {
                            Ok(()) => {
                                UI.with(|u| u.borrow_mut().presets.remove(&id));
                            }
                            Err(e) => st.toast(format!("The preset could not be saved: {e}")),
                        }
                    }
                }
                None => st.toast("The plug-in has no settings to save yet."),
            }
        }
    });
    if let Some(p) = load {
        load_preset(st, target, i, &p);
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
    let version = st.session_version;
    if UI.with(|u| u.borrow().synced_version) != version {
        UI.with(|u| u.borrow_mut().synced_version = version);
        let mut chains: Vec<&mut Vec<EffectSlot>> = Vec::new();
        let (tracks, master) = (&mut st.session.tracks, &mut st.session.master_chain);
        for t in tracks.iter_mut() {
            chains.push(&mut t.chain);
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
            .flat_map(|t| t.chain.iter())
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
                .flat_map(|t| t.chain.iter())
                .chain(p.session.master_chain.iter())
            {
                if is_plugin(&s.effect) {
                    keep.insert(s.plugin_data().inst);
                }
            }
        }
        manager().retain(&keep);
    }
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
    for (inst, n) in notices {
        let place = at.get(&inst).copied();
        match n {
            Notice::Edited { id, value } => {
                if let Some((t, i)) = place {
                    begin_pending(st);
                    edit_slot(st, t, i, |d| d.set_param(id, value));
                }
            }
            Notice::State(s) => {
                if let Some((t, i)) = place {
                    begin_pending(st);
                    edit_slot(st, t, i, |d| d.state = Some(s.clone()));
                    commit = true;
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
            Notice::Latency(_) | Notice::BeginEdit(_) | Notice::Started => {}
        }
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
    egui::Window::new(RichText::new("Audio Plug-In Manager").strong())
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
        af27_vst3::registry::scan_into(m.registry_mutex(), &exe, bundles, &progress);
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
        for f in af27_vst3::registry::standard_folders() {
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
            if let Some(p) = rfd::FileDialog::new().pick_folder() {
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
