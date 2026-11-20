// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The Effects Rack: sixteen numbered slots for a track, a bus, the master
//! or (in the Waveform editor) the open file. Each slot has a power
//! button and opens its effect in a window of its own; slots drag to a new
//! place in the order. Under the slots: the rack's power, input and output
//! gain and dry/wet mix. Rack presets save and load whole chains.

use egui::{pos2, vec2, Align2, Color32, FontId, Rect, RichText, Rounding, Sense, Stroke};

use af27_core::session::CHAIN_SLOTS;
use af27_core::{EffectSlot, Rack, TrackKind};
use af27_engine::MASTER;

use crate::state::{data_dir, Mode, State};
use crate::theme::{c, ca, lerp, Theme};

/// The rack of the open file in the Waveform editor (applied, not live).
pub const FILE: u16 = u16::MAX - 1;

/// Which rack the panel shows in the Multitrack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RackTab {
    #[default]
    Track,
    Master,
}

/// The slots of a rack.
pub fn chain_of(st: &mut State, target: u16) -> Option<&mut Vec<EffectSlot>> {
    if target == FILE {
        Some(&mut st.file_chain)
    } else {
        st.chain_mut(target)
    }
}

fn chain_ref(st: &State, target: u16) -> Option<&Vec<EffectSlot>> {
    match target {
        FILE => Some(&st.file_chain),
        MASTER => Some(&st.session.master_chain),
        i => st.session.tracks.get(i as usize).map(|t| &t.chain),
    }
}

fn rack_of(st: &mut State, target: u16) -> Option<&mut Rack> {
    match target {
        FILE => Some(&mut st.file_rack),
        MASTER => Some(&mut st.session.master_rack),
        i => st.session.tracks.get_mut(i as usize).map(|t| &mut t.rack),
    }
}

/// What a rack is called in titles.
pub fn target_name(st: &State, target: u16) -> String {
    match target {
        FILE => st.doc().map_or("No file".into(), |d| d.name.clone()),
        MASTER => "Master".into(),
        i => st
            .session
            .tracks
            .get(i as usize)
            .map_or("?".into(), |t| t.name.clone()),
    }
}

/// The rack the panel shows now.
pub fn current_target(st: &State) -> Option<u16> {
    if st.mode == Mode::Waveform {
        return Some(FILE);
    }
    match st.rack_tab {
        RackTab::Master => Some(MASTER),
        RackTab::Track => st
            .selected_track
            .and_then(|id| st.session.track_index(id))
            .map(|i| i as u16),
    }
}

/// A rack edit is done: one undo step (session racks) and the mix hears it.
fn structural(st: &mut State, target: u16, label: &str, before: af27_core::Session) {
    if target != FILE {
        st.commit_session(label, before);
    }
}

/// Opens (or brings back) a slot's window.
/// A plug-in slot also opens the plug-in's own window. Every way of
/// opening a slot comes here.
pub fn open_window(st: &mut State, target: u16, slot: usize) {
    if !st.fx_windows.contains(&(target, slot)) {
        st.fx_windows.push((target, slot));
    }
    crate::plugins::open_editor(st, target, slot);
}

/// A slot's name as the rack shows it.
fn slot_name(s: &EffectSlot) -> String {
    if af27_core::plugin::is_plugin(&s.effect) {
        crate::plugins::slot_name(s)
    } else {
        crate::fx::name(&s.effect)
    }
}

/// Window indexes follow a slot that moved from `from` to `to`, and close
/// for a slot that went (`to` None).
fn remap_windows(st: &mut State, target: u16, from: usize, to: Option<usize>) {
    st.fx_windows.retain_mut(|(t, s)| {
        if *t != target {
            return true;
        }
        match to {
            None => {
                if *s == from {
                    return false;
                }
                if *s > from {
                    *s -= 1;
                }
            }
            Some(to) => {
                if *s == from {
                    *s = to;
                } else if from < to && *s > from && *s <= to {
                    *s -= 1;
                } else if to < from && *s >= to && *s < from {
                    *s += 1;
                }
            }
        }
        true
    });
}

/// The Effects tab.
pub fn panel(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    if st.mode == Mode::Multitrack {
        ui.horizontal(|ui| {
            ui.add_enabled(false, egui::SelectableLabel::new(false, "Clip Effects"))
                .on_disabled_hover_text("Effects on single clips are coming.");
            if ui
                .selectable_label(st.rack_tab == RackTab::Track, "Track Effects")
                .clicked()
            {
                st.rack_tab = RackTab::Track;
            }
            if ui
                .selectable_label(st.rack_tab == RackTab::Master, "Master")
                .clicked()
            {
                st.rack_tab = RackTab::Master;
            }
        });
    }
    let Some(target) = current_target(st) else {
        ui.add_space(8.0);
        ui.label(RichText::new("Select a track to see its effects rack.").color(c(th.text_dim)));
        return;
    };
    presets_row(ui, st, target, &th);
    let kind = match target {
        FILE => "File",
        MASTER => "Mix",
        i => match st.session.tracks.get(i as usize).map(|t| t.kind) {
            Some(TrackKind::Bus) => "Bus",
            _ => "Track",
        },
    };
    ui.label(
        RichText::new(format!("{kind}: {}", target_name(st, target)))
            .strong()
            .color(c(th.text)),
    );
    ui.add_space(2.0);
    let bottom_h = if target == FILE { 92.0 } else { 64.0 };
    let avail = (ui.available_height() - bottom_h).max(120.0);
    egui::ScrollArea::vertical()
        .id_salt("rack-slots")
        .max_height(avail)
        .auto_shrink([false, true])
        .show(ui, |ui| slots(ui, st, target, &th));
    ui.separator();
    rack_controls(ui, st, target, &th);
    if target == FILE {
        file_apply(ui, st, &th);
    }
}

/// The sixteen rows.
fn slots(ui: &mut egui::Ui, st: &mut State, target: u16, th: &Theme) {
    let before = st.session.clone();
    let list: Vec<EffectSlot> = chain_ref(st, target).cloned().unwrap_or_default();
    let mut moved: Option<(usize, usize)> = None;
    let mut removed: Option<usize> = None;
    let mut add_at: Option<(usize, String)> = None;
    let mut toggled: Option<usize> = None;
    let row_h = 22.0;
    for i in 0..CHAIN_SLOTS {
        let slot = list.get(i);
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(w, row_h), Sense::hover());
        let p = ui.painter_at(rect);
        let odd = i % 2 == 1;
        p.rect_filled(
            rect,
            Rounding::ZERO,
            if odd {
                ca(th.panel_raised, 120)
            } else {
                ca(th.panel_raised, 60)
            },
        );
        p.text(
            pos2(rect.left() + 12.0, rect.center().y),
            Align2::CENTER_CENTER,
            format!("{}", i + 1),
            FontId::proportional(11.0),
            c(th.text_dim),
        );
        let power =
            Rect::from_center_size(pos2(rect.left() + 34.0, rect.center().y), vec2(18.0, 18.0));
        let name_r = Rect::from_min_max(
            pos2(power.right() + 6.0, rect.top()),
            pos2(rect.right() - 24.0, rect.bottom()),
        );
        let arrow = Rect::from_min_max(pos2(name_r.right(), rect.top()), rect.max);
        match slot {
            Some(s) => {
                let pr = ui.interact(power, ui.id().with(("rack-pow", target, i)), Sense::click());
                power_icon(&p, power, !s.bypass, pr.hovered(), th);
                if pr.on_hover_text("On or bypassed").clicked() {
                    toggled = Some(i);
                }
                // The name: click to open, drag to move.
                let id = ui.id().with(("rack-slot", target, i));
                let open = st.fx_windows.contains(&(target, i));
                let nr = ui
                    .allocate_new_ui(egui::UiBuilder::new().max_rect(name_r), |ui| {
                        ui.dnd_drag_source(id, i, |ui| {
                            let text = RichText::new(slot_name(s)).color(if s.bypass {
                                c(th.text_dim)
                            } else {
                                c(th.text)
                            });
                            let (r, resp) = ui.allocate_exact_size(name_r.size(), Sense::click());
                            if open || resp.hovered() {
                                ui.painter().rect_filled(
                                    r,
                                    Rounding::same(2.0),
                                    ca(th.accent, if open { 40 } else { 20 }),
                                );
                            }
                            ui.painter().text(
                                r.left_center() + vec2(4.0, 0.0),
                                Align2::LEFT_CENTER,
                                text.text(),
                                FontId::proportional(12.5),
                                if s.bypass { c(th.text_dim) } else { c(th.text) },
                            );
                            if s.sidechain.is_some() {
                                ui.painter().text(
                                    r.right_center() - vec2(4.0, 0.0),
                                    Align2::RIGHT_CENTER,
                                    "key",
                                    FontId::proportional(9.5),
                                    c(th.accent_alt),
                                );
                            }
                            resp
                        })
                    })
                    .inner;
                if nr.inner.clicked() {
                    if open {
                        st.fx_windows.retain(|w| *w != (target, i));
                    } else {
                        open_window(st, target, i);
                    }
                }
                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(arrow), |ui| {
                    ui.menu_button("▸", |ui| {
                        if ui.button("Open").clicked() {
                            open_window(st, target, i);
                            ui.close_menu();
                        }
                        ui.menu_button("Replace With", |ui| {
                            if let Some(id) = effect_picker(ui) {
                                add_at = Some((i, id));
                                removed = Some(i);
                            }
                        });
                        ui.separator();
                        if ui
                            .add_enabled(i > 0, egui::Button::new("Move Up"))
                            .clicked()
                        {
                            moved = Some((i, i - 1));
                            ui.close_menu();
                        }
                        if ui
                            .add_enabled(i + 1 < list.len(), egui::Button::new("Move Down"))
                            .clicked()
                        {
                            moved = Some((i, i + 1));
                            ui.close_menu();
                        }
                        if ui.button("Remove").clicked() {
                            removed = Some(i);
                            ui.close_menu();
                        }
                    });
                });
            }
            None if i == list.len() => {
                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(name_r), |ui| {
                    ui.menu_button(RichText::new("+ Add effect").color(c(th.text_dim)), |ui| {
                        if let Some(id) = effect_picker(ui) {
                            add_at = Some((i, id));
                        }
                    });
                });
            }
            None => {}
        }
        // Dropped here: the slot moves to this row (an empty row past the
        // end: to the end).
        let resp = ui.interact(rect, ui.id().with(("rack-drop", target, i)), Sense::hover());
        if let Some(from) = resp.dnd_hover_payload::<usize>() {
            if *from != i {
                p.line_segment(
                    [rect.left_top(), rect.right_top()],
                    Stroke::new(2.0_f32, c(th.accent)),
                );
            }
        }
        if let Some(from) = resp.dnd_release_payload::<usize>() {
            let to = i.min(list.len().saturating_sub(1));
            if *from != to {
                moved = Some((*from, to));
            }
        }
    }
    if let Some(i) = toggled {
        if let Some(s) = chain_of(st, target).and_then(|c| c.get_mut(i)) {
            s.bypass = !s.bypass;
        }
        if target != FILE {
            st.live_slot(target, i);
            st.commit_live("Bypass", before.clone());
        }
        crate::plugins::show_bypassed(st, target, i);
    }
    let mut changed = false;
    if let Some((from, to)) = moved {
        if let Some(c) = chain_of(st, target) {
            if from < c.len() && to < c.len() {
                let s = c.remove(from);
                c.insert(to, s);
                changed = true;
            }
        }
        remap_windows(st, target, from, Some(to));
    }
    if let Some(i) = removed {
        if let Some(c) = chain_of(st, target) {
            if i < c.len() {
                c.remove(i);
                changed = true;
            }
        }
        remap_windows(st, target, i, None);
    }
    if let Some((i, id)) = add_at {
        if let Some(c) = chain_of(st, target) {
            if c.len() < CHAIN_SLOTS {
                let at = i.min(c.len());
                c.insert(at, new_slot(&id));
                changed = true;
                open_window(st, target, at);
            }
        }
    }
    if changed {
        structural(st, target, "Effects rack", before);
    }
}

pub fn new_slot(id: &str) -> EffectSlot {
    if af27_core::plugin::is_plugin(id) {
        return crate::plugins::new_slot(id);
    }
    EffectSlot {
        values: crate::fx::defaults(id),
        effect: id.to_string(),
        bypass: false,
        sidechain: None,
        data: None,
        ext_id: None,
    }
}

/// The Add Effect menu: families, then effects. Returns the id picked.
pub fn effect_picker(ui: &mut egui::Ui) -> Option<String> {
    let mut picked = None;
    for (group, list) in crate::fx::catalog() {
        ui.menu_button(group, |ui| {
            for (id, name) in list {
                if ui.button(&name).clicked() {
                    picked = Some(id.clone());
                    ui.close_menu();
                }
            }
        });
    }
    ui.separator();
    if let Some(id) = crate::plugins::picker(ui) {
        picked = Some(id);
    }
    picked
}

/// A drawn power symbol: lit when on.
pub fn power_icon(p: &egui::Painter, r: Rect, on: bool, hot: bool, th: &Theme) {
    let col = if on {
        c(th.accent)
    } else if hot {
        c(th.text)
    } else {
        c(th.text_dim)
    };
    let ctr = r.center();
    let rad = r.width().min(r.height()) * 0.32;
    // An open ring with the bar through its gap.
    let n = 20;
    let pts: Vec<egui::Pos2> = (0..=n)
        .map(|k| {
            let a = -std::f32::consts::FRAC_PI_2
                + 0.6
                + (k as f32 / n as f32) * (std::f32::consts::TAU - 1.2);
            ctr + vec2(a.cos(), a.sin()) * rad
        })
        .collect();
    p.add(egui::Shape::line(pts, Stroke::new(1.6_f32, col)));
    p.line_segment(
        [ctr - vec2(0.0, rad + 1.5), ctr - vec2(0.0, rad * 0.15)],
        Stroke::new(1.6_f32, col),
    );
    if on && th.glow > 0.0 {
        p.circle_filled(ctr, rad + 3.0, ca(th.accent, (22.0 * th.glow) as u8));
    }
}

/// The rack's own power, input, output and mix.
fn rack_controls(ui: &mut egui::Ui, st: &mut State, target: u16, th: &Theme) {
    let before = st.session.clone();
    let Some(mut r) = rack_of(st, target).copied() else {
        return;
    };
    let mut changed = false;
    let mut done = false;
    ui.horizontal(|ui| {
        let (pr, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
        power_icon(ui.painter(), pr, !r.bypass, resp.hovered(), th);
        if resp.on_hover_text("The whole rack on or off").clicked() {
            r.bypass = !r.bypass;
            changed = true;
            done = true;
        }
        ui.add_space(4.0);
        for (label, v, lo, hi, dflt) in [
            ("Input", &mut r.input_db, -24.0f32, 24.0f32, 0.0f32),
            ("Output", &mut r.output_db, -24.0, 24.0, 0.0),
        ] {
            ui.label(RichText::new(label).size(11.0).color(c(th.text_dim)));
            let resp =
                crate::track_views::dial(ui, v, lo..=hi, dflt, |v| format!("{v:+.1} dB"), th);
            changed |= resp.changed();
            done |= resp.drag_stopped() || (resp.changed() && !resp.dragged());
        }
        ui.label(RichText::new("Mix").size(11.0).color(c(th.text_dim)));
        let mut pct = r.mix * 100.0;
        let resp =
            crate::track_views::dial(ui, &mut pct, 0.0..=100.0, 100.0, |v| format!("{v:.0}%"), th);
        if resp.changed() {
            r.mix = pct / 100.0;
            changed = true;
        }
        done |= resp.drag_stopped() || (resp.changed() && !resp.dragged());
    });
    if changed {
        if let Some(x) = rack_of(st, target) {
            *x = r;
        }
        if target != FILE {
            st.live_rack(target);
            if !st.gesture {
                st.commit_live("Rack levels", before);
                st.gesture = !done;
            }
        }
    }
    if done {
        st.gesture = false;
    }
}

/// Waveform: run the selection (or the whole file) through the rack.
fn file_apply(ui: &mut egui::Ui, st: &mut State, th: &Theme) {
    let target = match st.doc() {
        Some(d) if d.sel.is_some() => "the selection",
        Some(_) => "the whole file",
        None => "no file open",
    };
    ui.horizontal(|ui| {
        let apply = egui::Button::new(RichText::new("Apply").strong().color(c(th.background)))
            .fill(c(th.accent));
        if ui
            .add_enabled(st.doc().is_some() && !st.file_chain.is_empty(), apply)
            .on_hover_text(format!("Process {target} through the rack"))
            .clicked()
        {
            apply_file_rack(st);
        }
        ui.label(
            RichText::new(format!("Applies to {target}"))
                .size(10.0)
                .color(c(th.text_dim)),
        );
    });
}

pub fn apply_file_rack(st: &mut State) {
    let chain = st.file_chain.clone();
    let rack = st.file_rack;
    if rack.bypass {
        return;
    }
    let Some(d) = st.doc_mut() else { return };
    let r = d.target();
    if r.is_empty() {
        return;
    }
    let dry = d.buf.slice(r);
    let gin = af27_core::time::db_to_gain(rack.input_db);
    let gout = af27_core::time::db_to_gain(rack.output_db);
    let mut seg = dry.clone();
    for ch in &mut seg.channels {
        for s in ch.iter_mut() {
            *s *= gin;
        }
    }
    let dry_in = seg.clone();
    for slot in chain.iter().filter(|s| !s.bypass) {
        if let Some(out) = crate::fx::process_offline(slot, &seg) {
            seg = out;
        }
    }
    for (ch, dch) in seg.channels.iter_mut().zip(&dry_in.channels) {
        for (s, d) in ch.iter_mut().zip(dch) {
            *s = (*s * rack.mix + *d * (1.0 - rack.mix)) * gout;
        }
    }
    let mut b = (*d.buf).clone();
    b.replace(r, &seg);
    d.set_buffer("Effects Rack", b);
}

// ------------------------------------------------------------ presets

fn presets_dir() -> std::path::PathBuf {
    data_dir().join("rack-presets")
}

fn preset_names() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(presets_dir())
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let p = e.path();
                    (p.extension().is_some_and(|x| x == "json"))
                        .then(|| p.file_stem().map(|s| s.to_string_lossy().to_string()))
                        .flatten()
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| crate::sessions::natural_cmp(a, b));
    v
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Preset {
    chain: Vec<EffectSlot>,
    #[serde(default)]
    rack: Rack,
}

fn presets_row(ui: &mut egui::Ui, st: &mut State, target: u16, th: &Theme) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Presets:").color(c(th.text_dim)));
        let names = preset_names();
        let mut load: Option<String> = None;
        egui::ComboBox::from_id_salt("rack-preset")
            .selected_text("(Default)")
            .width((ui.available_width() - 70.0).max(80.0))
            .show_ui(ui, |ui| {
                if ui.selectable_label(false, "(Default)").clicked() {
                    load = Some(String::new());
                }
                for n in &names {
                    if ui.selectable_label(false, n).clicked() {
                        load = Some(n.clone());
                    }
                }
            });
        if ui
            .small_button("Save")
            .on_hover_text("Save this rack as a preset")
            .clicked()
        {
            st.rack_preset_name = Some(format!("{} rack", target_name(st, target)));
        }
        ui.menu_button("Delete", |ui| {
            if names.is_empty() {
                ui.label("No presets saved yet.");
            }
            for n in &names {
                if ui.button(n).clicked() {
                    let _ = std::fs::remove_file(presets_dir().join(format!("{n}.json")));
                    ui.close_menu();
                }
            }
        });
        if let Some(n) = load {
            let before = st.session.clone();
            let preset = if n.is_empty() {
                Some(Preset {
                    chain: Vec::new(),
                    rack: Rack::default(),
                })
            } else {
                std::fs::read_to_string(presets_dir().join(format!("{n}.json")))
                    .ok()
                    .and_then(|s| serde_json::from_str::<Preset>(&s).ok())
            };
            if let Some(p) = preset {
                if let Some(c) = chain_of(st, target) {
                    *c = p.chain;
                }
                if let Some(r) = rack_of(st, target) {
                    *r = p.rack;
                }
                st.fx_windows.retain(|(t, _)| *t != target);
                structural(st, target, "Rack preset", before);
            }
        }
    });
    // Naming a new preset.
    if let Some(mut name) = st.rack_preset_name.take() {
        let mut keep = true;
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(160.0));
            r.request_focus();
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.small_button("Save").clicked() || enter {
                let clean: String = name
                    .chars()
                    .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
                    .collect();
                if !clean.trim().is_empty() {
                    let p = Preset {
                        chain: chain_ref(st, target).cloned().unwrap_or_default(),
                        rack: match target {
                            FILE => st.file_rack,
                            MASTER => st.session.master_rack,
                            i => st
                                .session
                                .tracks
                                .get(i as usize)
                                .map(|t| t.rack)
                                .unwrap_or_default(),
                        },
                    };
                    let _ = std::fs::create_dir_all(presets_dir());
                    if let Ok(s) = serde_json::to_string_pretty(&p) {
                        let _ =
                            std::fs::write(presets_dir().join(format!("{}.json", clean.trim())), s);
                    }
                }
                keep = false;
            }
            if ui.small_button("Cancel").clicked() {
                keep = false;
            }
        });
        if keep {
            st.rack_preset_name = Some(name);
        }
    }
}

// ------------------------------------------------------------ windows

/// Every open effect window.
pub fn windows(ctx: &egui::Context, st: &mut State) {
    let th = st.theme.clone();
    let open = st.fx_windows.clone();
    for (target, i) in open {
        let Some(slot) = chain_ref(st, target).and_then(|c| c.get(i)).cloned() else {
            st.fx_windows.retain(|w| *w != (target, i));
            continue;
        };
        let mut keep = true;
        let title = format!("{}: {}", target_name(st, target), slot_name(&slot));
        egui::Window::new(RichText::new(&title).strong())
            .id(egui::Id::new(("fx-window", target, i)))
            .open(&mut keep)
            .resizable(false)
            .collapsible(true)
            .default_width(320.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (pr, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                    power_icon(ui.painter(), pr, !slot.bypass, resp.hovered(), &th);
                    if resp.on_hover_text("On or bypassed").clicked() {
                        let before = st.session.clone();
                        if let Some(s) = chain_of(st, target).and_then(|c| c.get_mut(i)) {
                            s.bypass = !s.bypass;
                        }
                        if target != FILE {
                            st.live_slot(target, i);
                            st.commit_live("Bypass", before);
                        }
                        crate::plugins::show_bypassed(st, target, i);
                    }
                    ui.label(RichText::new(format!("Slot {}", i + 1)).color(c(th.text_dim)));
                });
                if target != FILE && target != MASTER && crate::fx::takes_key(&slot.effect) {
                    listen_to(ui, st, target, i, &th);
                }
                ui.separator();
                if af27_core::plugin::is_plugin(&slot.effect) {
                    crate::plugins::slot_controls(ui, st, target, i, &th);
                } else {
                    slot_controls(ui, st, target, i, &th);
                }
            });
        if !keep {
            st.fx_windows.retain(|w| *w != (target, i));
        }
    }
}

/// "Listen to": what keys a sidechain-capable effect.
pub fn listen_to(ui: &mut egui::Ui, st: &mut State, target: u16, i: usize, th: &Theme) {
    let Some(me) = st.session.tracks.get(target as usize).map(|t| t.id) else {
        return;
    };
    let cur = chain_ref(st, target)
        .and_then(|c| c.get(i))
        .and_then(|s| s.sidechain);
    let name_of = |st: &State, id| {
        st.session
            .tracks
            .iter()
            .find(|t| t.id == id)
            .map_or("(gone)".to_string(), |t| {
                if t.kind == TrackKind::Bus {
                    format!("Bus {}", t.name)
                } else {
                    t.name.clone()
                }
            })
    };
    let shown = cur.map_or("Own input".to_string(), |id| name_of(st, id));
    let mut pick: Option<Option<af27_core::TrackId>> = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Listen to").color(c(th.text_dim)));
        egui::ComboBox::from_id_salt(("listen", target, i))
            .selected_text(shown)
            .width(180.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(cur.is_none(), "Own input").clicked() {
                    pick = Some(None);
                }
                let ids: Vec<af27_core::TrackId> = st
                    .session
                    .tracks
                    .iter()
                    .map(|t| t.id)
                    .filter(|id| *id != me)
                    .collect();
                for id in ids {
                    // The key may not come back round to this track.
                    let ok = st.session.can_route(id, me);
                    let r = ui.add_enabled(
                        ok,
                        egui::SelectableLabel::new(cur == Some(id), name_of(st, id)),
                    );
                    if r.clicked() {
                        pick = Some(Some(id));
                    }
                }
            });
    });
    if let Some(p) = pick {
        let before = st.session.clone();
        if let Some(s) = chain_of(st, target).and_then(|c| c.get_mut(i)) {
            s.sidechain = p;
        }
        st.commit_session("Sidechain", before);
    }
}

/// A slot's settings.
pub fn slot_controls(ui: &mut egui::Ui, st: &mut State, target: u16, i: usize, th: &Theme) {
    let Some(slot) = chain_ref(st, target).and_then(|c| c.get(i)).cloned() else {
        return;
    };
    let before = st.session.clone();
    let controls = crate::fx::controls(&slot.effect);
    let mut vals = slot.values.clone();
    vals.resize(controls.len(), 0.0);
    let mut ch = false;
    let mut released = false;
    let mut group = String::new();
    for (k, p) in controls.iter().enumerate() {
        if p.hidden {
            continue;
        }
        if !p.group.is_empty() && p.group != group {
            group = p.group.clone();
            ui.label(RichText::new(&group).small().strong().color(c(th.text_dim)));
        }
        let r = if p.toggle {
            let mut on = vals[k] >= 0.5;
            let r = ui.checkbox(&mut on, &p.name);
            vals[k] = if on { 1.0 } else { 0.0 };
            r
        } else if !p.choices.is_empty() {
            let cur = (vals[k].round() as usize).min(p.choices.len() - 1);
            let mut pick = cur;
            egui::ComboBox::from_label(&p.name)
                .selected_text(&p.choices[cur])
                .show_ui(ui, |ui| {
                    for (ci, cname) in p.choices.iter().enumerate() {
                        ui.selectable_value(&mut pick, ci, cname);
                    }
                });
            if pick != cur {
                vals[k] = (p.min + pick as f32).min(p.max);
                ch = true;
                released = true;
            }
            continue;
        } else {
            let mut s = crate::widgets::Slider::new(&mut vals[k], p.min..=p.max).text(&p.name);
            if !p.unit.is_empty() {
                s = s.suffix(format!(" {}", p.unit));
            }
            if p.log && p.min > 0.0 {
                s = s.logarithmic(true);
            }
            ui.add(s)
        };
        ch |= r.changed();
        released |= r.drag_stopped() || (r.changed() && !r.dragged());
    }
    if ch {
        if let Some(s) = chain_of(st, target).and_then(|c| c.get_mut(i)) {
            s.values = vals;
        }
        if target != FILE {
            st.live_slot(target, i);
            let down = ui.input(|i| i.pointer.any_down());
            if !st.gesture {
                st.commit_live(&crate::fx::name(&slot.effect), before);
                st.gesture = down;
            }
        }
    }
    if released {
        st.gesture = false;
        // Effects that cannot retune while playing are rebuilt when a
        // change is finished.
        if target != FILE && crate::fx::is_fx(&slot.effect) {
            st.session_version += 1;
        }
    }
}

#[allow(dead_code)]
fn faint(th: &Theme) -> Color32 {
    lerp(th.panel, th.text, 0.1)
}
