// SPDX-License-Identifier: GPL-3.0-or-later
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
            .or((!st.session.tracks.is_empty()).then_some(0))
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
    // The track's (or the master's) own settings, folded above the rack.
    if target != FILE {
        egui::CollapsingHeader::new(
            RichText::new(if target == MASTER {
                "Master".to_string()
            } else {
                format!("Track: {}", target_name(st, target))
            })
            .strong(),
        )
        .id_salt("rack-track-settings")
        .default_open(true)
        .show(ui, |ui| {
            if target == MASTER {
                crate::inspector::master_settings(ui, st);
            } else {
                crate::inspector::track_settings(ui, st, target as usize);
            }
        });
        ui.separator();
    }
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
    let bottom_h = if target == FILE { 130.0 } else { 100.0 };
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

/// Which slot sits in each of the sixteen rows (slots keep their row, so
/// a rack can have gaps). Slots with no row follow the one before.
pub fn rows(list: &[EffectSlot]) -> [Option<usize>; CHAIN_SLOTS] {
    let mut out = [None; CHAIN_SLOTS];
    let mut next = 0usize;
    for (i, s) in list.iter().enumerate() {
        let want = s.position.map_or(next, |p| (p as usize).max(next));
        let mut r = want.min(CHAIN_SLOTS - 1);
        while r < CHAIN_SLOTS && out[r].is_some() {
            r += 1;
        }
        if r >= CHAIN_SLOTS {
            // No room below: the first free row.
            r = out
                .iter()
                .position(Option::is_none)
                .unwrap_or(CHAIN_SLOTS - 1);
        }
        out[r] = Some(i);
        next = r + 1;
    }
    out
}

/// Puts slots in row order with their rows written down, after a move:
/// `placed` is (slot index, wanted row) for the slot that moved, which
/// pushes whatever sits there (and after it) down a row.
fn place(list: &mut Vec<EffectSlot>, placed: Option<(usize, usize)>) -> Vec<usize> {
    let mut cur: Vec<(usize, usize)> = rows(list)
        .iter()
        .enumerate()
        .filter_map(|(r, i)| i.map(|i| (i, r)))
        .collect();
    if let Some((mi, mr)) = placed {
        cur.retain(|(i, _)| *i != mi);
        // Push down from the wanted row.
        let mut taken: Vec<(usize, usize)> = Vec::new();
        let row_for = |r: usize, taken: &Vec<(usize, usize)>| {
            let mut r = r;
            while taken.iter().any(|(_, x)| *x == r) && r < CHAIN_SLOTS - 1 {
                r += 1;
            }
            r
        };
        taken.push((mi, mr.min(CHAIN_SLOTS - 1)));
        cur.sort_by_key(|(_, r)| *r);
        for (i, r) in cur {
            let nr = if r >= mr { row_for(r, &taken) } else { r };
            let nr = if taken.iter().any(|(_, x)| *x == nr) {
                row_for(nr, &taken)
            } else {
                nr
            };
            taken.push((i, nr));
        }
        cur = taken;
    }
    cur.sort_by_key(|(_, r)| *r);
    // Old index -> new index, for the effect windows.
    let order: Vec<usize> = cur.iter().map(|(i, _)| *i).collect();
    let old = std::mem::take(list);
    let mut slots: Vec<Option<EffectSlot>> = old.into_iter().map(Some).collect();
    for (i, r) in &cur {
        if let Some(mut sl) = slots[*i].take() {
            sl.position = Some(*r as u8);
            list.push(sl);
        }
    }
    order
}

/// The sixteen rows, as the leading editor lays them out: number, power,
/// name, and a menu on every row (empty ones add an effect there).
fn slots(ui: &mut egui::Ui, st: &mut State, target: u16, th: &Theme) {
    let before = st.session.clone();
    let list: Vec<EffectSlot> = chain_ref(st, target).cloned().unwrap_or_default();
    let map = rows(&list);
    // (slot, to row), slot to remove, (row, effect id) to add, power flips.
    let mut moved: Option<(usize, usize)> = None;
    let mut removed: Option<usize> = None;
    let mut add_at: Option<(usize, String)> = None;
    let mut toggled: Option<usize> = None;
    let mut open: Option<usize> = None;
    let row_h = 22.0;
    let frame = ui.available_rect_before_wrap();
    ui.painter().rect_stroke(
        Rect::from_min_size(frame.min, vec2(frame.width(), row_h * CHAIN_SLOTS as f32)),
        Rounding::ZERO,
        Stroke::new(1.0_f32, ca(th.border, 200)),
    );
    for (row, slot_i) in map.iter().enumerate() {
        let slot = slot_i.and_then(|i| list.get(i).map(|s| (i, s)));
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, row_h), Sense::click());
        let p = ui.painter_at(rect);
        let selected = st.rack_row == Some((target, row));
        p.rect_filled(
            rect,
            Rounding::ZERO,
            if selected {
                ca(th.accent, 40)
            } else if row % 2 == 1 {
                ca(th.panel_raised, 110)
            } else {
                ca(th.panel_raised, 55)
            },
        );
        p.line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(1.0_f32, ca(th.border, 90)),
        );
        // Number column.
        let num = Rect::from_min_size(rect.min, vec2(20.0, row_h));
        p.line_segment(
            [num.right_top(), num.right_bottom()],
            Stroke::new(1.0_f32, ca(th.border, 120)),
        );
        p.text(
            num.center(),
            Align2::CENTER_CENTER,
            format!("{}", row + 1),
            FontId::proportional(11.0),
            c(th.text_dim),
        );
        // Power column.
        let pow_cell = Rect::from_min_size(pos2(num.right(), rect.top()), vec2(22.0, row_h));
        p.line_segment(
            [pow_cell.right_top(), pow_cell.right_bottom()],
            Stroke::new(1.0_f32, ca(th.border, 120)),
        );
        let pr = ui.interact(
            pow_cell,
            ui.id().with(("rack-pow", target, row)),
            Sense::click(),
        );
        match slot {
            Some((_, s)) => {
                power_icon(&p, pow_cell.shrink(2.0), !s.bypass, pr.hovered(), th);
                if pr.on_hover_text("On or bypassed").clicked() {
                    toggled = *slot_i;
                }
            }
            None => {
                // An empty slot's switch, dim.
                let dim = Theme {
                    accent: th.text_dim,
                    ..th.clone()
                };
                power_icon(&p, pow_cell.shrink(3.0), false, false, &dim);
            }
        }
        // Menu column on the right.
        let arrow = Rect::from_min_max(pos2(rect.right() - 22.0, rect.top()), rect.max);
        p.line_segment(
            [arrow.left_top(), arrow.left_bottom()],
            Stroke::new(1.0_f32, ca(th.border, 120)),
        );
        // The name.
        let name_r = Rect::from_min_max(
            pos2(pow_cell.right() + 6.0, rect.top()),
            pos2(arrow.left() - 4.0, rect.bottom()),
        );
        if let Some((i, s)) = slot {
            let id = ui.id().with(("rack-slot", target, i));
            let is_open = st.fx_windows.contains(&(target, i));
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(name_r), |ui| {
                let r = ui
                    .dnd_drag_source(id, i, |ui| {
                        let (r, resp) = ui.allocate_exact_size(name_r.size(), Sense::click());
                        ui.painter().text(
                            r.left_center(),
                            Align2::LEFT_CENTER,
                            crate::plugins::slot_name(s),
                            FontId::proportional(12.5),
                            if s.bypass { c(th.text_dim) } else { c(th.text) },
                        );
                        if s.sidechain.is_some() {
                            ui.painter().text(
                                r.right_center(),
                                Align2::RIGHT_CENTER,
                                "key",
                                FontId::proportional(9.5),
                                c(th.accent_alt),
                            );
                        }
                        if is_open {
                            ui.painter().line_segment(
                                [
                                    r.left_bottom() + vec2(0.0, -2.0),
                                    r.right_bottom() + vec2(0.0, -2.0),
                                ],
                                Stroke::new(1.0_f32, ca(th.accent, 160)),
                            );
                        }
                        resp
                    })
                    .inner;
                if r.clicked() {
                    st.rack_row = Some((target, row));
                }
                if r.double_clicked() {
                    open = Some(i);
                }
                r.on_hover_text("Double-click to open. Drag to another row to move it.");
            });
        } else if resp.clicked() {
            st.rack_row = Some((target, row));
        }
        if resp.double_clicked() {
            if let Some(i) = *slot_i {
                open = Some(i);
            }
        }
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(arrow), |ui| {
            ui.menu_button(RichText::new("▸").color(c(th.text)), |ui| match slot {
                Some((i, _)) => {
                    if ui.button("Open").clicked() {
                        open = Some(i);
                        ui.close_menu();
                    }
                    ui.menu_button("Replace With", |ui| {
                        if let Some(id) = effect_picker(ui) {
                            removed = Some(i);
                            add_at = Some((row, id));
                        }
                    });
                    ui.separator();
                    if ui.button("Remove").clicked() {
                        removed = Some(i);
                        ui.close_menu();
                    }
                }
                None => {
                    ui.label(RichText::new(format!("Slot {}", row + 1)).small());
                    if let Some(id) = effect_picker(ui) {
                        add_at = Some((row, id));
                    }
                }
            })
            .response
            .on_hover_text(if slot.is_some() {
                "Effect menu"
            } else {
                "Add an effect here"
            });
        });
        // Dropped on this row: the slot moves here (pushing down what is
        // here and after it).
        let drop = ui.interact(
            rect,
            ui.id().with(("rack-drop", target, row)),
            Sense::hover(),
        );
        if let Some(from) = drop.dnd_hover_payload::<usize>() {
            if map.get(row).copied().flatten() != Some(*from) {
                p.line_segment(
                    [rect.left_top(), rect.right_top()],
                    Stroke::new(2.0_f32, c(th.accent)),
                );
            }
        }
        if let Some(from) = drop.dnd_release_payload::<usize>() {
            moved = Some((*from, row));
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
    }
    if let Some(i) = open {
        open_window(st, target, i);
    }
    let mut changed = false;
    if let Some(i) = removed {
        if let Some(c) = chain_of(st, target) {
            if i < c.len() {
                c.remove(i);
                changed = true;
            }
        }
        remap_windows(st, target, i, None);
    }
    if let Some((row, id)) = add_at {
        let mut made = None;
        if let Some(c) = chain_of(st, target) {
            if c.len() < CHAIN_SLOTS {
                let mut sl = new_slot(&id);
                sl.position = Some(row as u8);
                c.push(sl);
                let new_i = c.len() - 1;
                let order = place(c, Some((new_i, row)));
                made = order.iter().position(|&o| o == new_i);
                changed = true;
            }
        }
        st.fx_windows.retain(|(t, _)| *t != target);
        if let Some(at) = made {
            open_window(st, target, at);
            st.rack_row = Some((target, row));
        }
    }
    if let Some((from, row)) = moved {
        let mut order = None;
        if let Some(c) = chain_of(st, target) {
            if from < c.len() {
                order = Some(place(c, Some((from, row))));
                changed = true;
            }
        }
        if let Some(order) = order {
            // Effect windows follow their slots to the new order.
            st.fx_windows = st
                .fx_windows
                .iter()
                .map(|&(t, i)| {
                    if t == target {
                        (t, order.iter().position(|&o| o == i).unwrap_or(i))
                    } else {
                        (t, i)
                    }
                })
                .collect();
        }
        st.rack_row = Some((target, row));
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
        position: None,
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

/// A level meter across, -48..0 dB, with its scale under it on the last.
fn meter(ui: &mut egui::Ui, level: f32, th: &Theme, scale: bool) {
    let w = ui.available_width().max(40.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, if scale { 22.0 } else { 12.0 }), Sense::hover());
    let bar = Rect::from_min_size(r.min, vec2(r.width(), 10.0));
    let p = ui.painter();
    p.rect_filled(bar, Rounding::ZERO, c(th.wave_background));
    p.rect_stroke(
        bar,
        Rounding::ZERO,
        Stroke::new(1.0_f32, ca(th.border, 200)),
    );
    let db = 20.0 * level.max(1e-9).log10();
    let f = ((db + 48.0) / 48.0).clamp(0.0, 1.0);
    if f > 0.0 {
        let lit = Rect::from_min_size(bar.min, vec2(bar.width() * f, bar.height()));
        let col = if db > -0.1 {
            c(th.meter_high)
        } else if db > -12.0 {
            c(th.meter_mid)
        } else {
            c(th.meter_low)
        };
        p.rect_filled(lit.shrink(1.0), Rounding::ZERO, col);
    }
    if scale {
        for d in [-48.0f32, -36.0, -24.0, -12.0, 0.0] {
            let x = bar.left() + bar.width() * (d + 48.0) / 48.0;
            p.line_segment(
                [pos2(x, bar.bottom()), pos2(x, bar.bottom() + 3.0)],
                Stroke::new(1.0_f32, ca(th.text_dim, 160)),
            );
            p.text(
                pos2(x, bar.bottom() + 4.0),
                if d == 0.0 {
                    Align2::RIGHT_TOP
                } else {
                    Align2::CENTER_TOP
                },
                format!("{d:.0}"),
                FontId::proportional(9.0),
                c(th.text_dim),
            );
        }
        p.text(
            pos2(bar.left(), bar.bottom() + 4.0),
            Align2::LEFT_TOP,
            "dB",
            FontId::proportional(9.0),
            c(th.text_dim),
        );
    }
}

/// The rack's input and output (dial and meter each), dry/wet mix, and
/// the rack's power at the bottom left.
fn rack_controls(ui: &mut egui::Ui, st: &mut State, target: u16, th: &Theme) {
    let before = st.session.clone();
    let Some(mut r) = rack_of(st, target).copied() else {
        return;
    };
    let (lin, lout) = st.rack_levels(target);
    let mut changed = false;
    let mut done = false;
    for (label, which) in [("Input", 0), ("Output", 1)] {
        ui.horizontal(|ui| {
            ui.add_sized(
                vec2(46.0, 18.0),
                egui::Label::new(RichText::new(format!("{label}:")).color(c(th.text_dim))),
            );
            let v = if which == 0 {
                &mut r.input_db
            } else {
                &mut r.output_db
            };
            let resp =
                crate::track_views::dial(ui, v, -24.0..=24.0, 0.0, |v| format!("{v:+.0}"), th)
                    .on_hover_text("dB. Double-click for 0.");
            changed |= resp.changed();
            done |= resp.drag_stopped() || (resp.changed() && !resp.dragged());
            meter(ui, if which == 0 { lin } else { lout }, th, which == 1);
        });
    }
    ui.horizontal(|ui| {
        ui.add_sized(
            vec2(46.0, 18.0),
            egui::Label::new(RichText::new("Mix:").color(c(th.text_dim))),
        );
        ui.label(RichText::new("Dry").size(11.0).color(c(th.text_dim)));
        let mut pct = r.mix * 100.0;
        ui.spacing_mut().slider_width = (ui.available_width() - 90.0).max(60.0);
        let resp = ui.add(crate::widgets::Slider::new(&mut pct, 0.0..=100.0).show_value(false));
        ui.label(RichText::new("Wet").size(11.0).color(c(th.text_dim)));
        ui.label(RichText::new(format!("{pct:.0} %")).color(c(th.accent)));
        if resp.changed() {
            r.mix = pct / 100.0;
            changed = true;
        }
        if resp.double_clicked() {
            r.mix = 1.0;
            changed = true;
            done = true;
        }
        done |= resp.drag_stopped() || (resp.changed() && !resp.dragged());
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let (pr, resp) = ui.allocate_exact_size(vec2(24.0, 22.0), Sense::click());
        ui.painter().rect_filled(
            pr,
            Rounding::same(2.0),
            if r.bypass {
                c(th.panel_raised)
            } else {
                ca(th.accent, 50)
            },
        );
        power_icon(ui.painter(), pr, !r.bypass, resp.hovered(), th);
        if resp.on_hover_text("The whole rack on or off").clicked() {
            r.bypass = !r.bypass;
            changed = true;
            done = true;
        }
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
    let names = preset_names();
    let mut load: Option<String> = None;
    let w = ui.available_width();
    // Laid out from the right: the buttons always fit whole, and the list
    // takes the room that is left.
    ui.allocate_ui_with_layout(
        vec2(w, 22.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.menu_button(RichText::new("🗑").size(14.0), |ui| {
                if names.is_empty() {
                    ui.label("No presets saved yet.");
                }
                for n in &names {
                    if ui.button(n).clicked() {
                        let _ = std::fs::remove_file(presets_dir().join(format!("{n}.json")));
                        ui.close_menu();
                    }
                }
            })
            .response
            .on_hover_text("Delete a saved preset");
            if ui
                .button(RichText::new("💾").size(14.0))
                .on_hover_text("Save this rack as a preset")
                .clicked()
            {
                st.rack_preset_name = Some(format!("{} rack", target_name(st, target)));
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.label(RichText::new("Presets:").color(c(th.text_dim)));
                egui::ComboBox::from_id_salt("rack-preset")
                    .selected_text("(Default)")
                    .width((ui.available_width() - 8.0).max(40.0))
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
            });
        },
    );
    {
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
    }
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
        crate::ui::popup(
            ctx,
            egui::Id::new(("fx-window", target, i)),
            RichText::new(&title).strong(),
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(pos: Option<u8>, id: &str) -> EffectSlot {
        let mut s = new_slot(id);
        s.position = pos;
        s
    }

    #[test]
    fn slots_keep_their_rows_with_gaps() {
        let list = vec![slot(Some(2), "a"), slot(Some(4), "b"), slot(None, "c")];
        let r = rows(&list);
        assert_eq!(r[2], Some(0));
        assert_eq!(r[4], Some(1));
        assert_eq!(r[5], Some(2), "no row: right after the one before");
        assert_eq!(r[0], None);
    }

    #[test]
    fn a_drop_on_a_taken_row_pushes_down() {
        let mut list = vec![slot(Some(0), "a"), slot(Some(1), "b"), slot(Some(5), "c")];
        // Drop "c" on row 1: "b" moves to row 2.
        let order = place(&mut list, Some((2, 1)));
        let names: Vec<(String, Option<u8>)> = list
            .iter()
            .map(|s| (s.effect.clone(), s.position))
            .collect();
        assert_eq!(
            names,
            vec![
                ("a".into(), Some(0)),
                ("c".into(), Some(1)),
                ("b".into(), Some(2))
            ]
        );
        assert_eq!(order, vec![0, 2, 1]);
    }
}
