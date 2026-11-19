// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The Inspector: everything about the selected track (output, sends, its
//! effect chain, 3D position, MIDI instrument), or the master chain when
//! no track is selected. Setting changes reach the audio thread at once,
//! without rebuilding the mix; each drag is one undo step.

use egui::RichText;

use crate::i18n::tr;
use crate::state::State;
use crate::theme::c;
use af27_core::midi::Wave;
use af27_core::{EffectSlot, TrackKind};
use af27_engine::MASTER;

/// Tracks one pointer gesture so a drag makes a single undo step.
fn live_commit(
    st: &mut State,
    ui: &egui::Ui,
    label: &str,
    before: &af27_core::Session,
    changed: bool,
) {
    let down = ui.input(|i| i.pointer.any_down());
    if changed && !st.gesture {
        st.commit_live(label, before.clone());
        st.gesture = down;
    }
    if !down {
        st.gesture = false;
    }
}

pub fn panel(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    let ti = st.selected_track.and_then(|id| st.session.track_index(id));
    match ti {
        Some(ti) => track(ui, st, ti),
        None => {
            ui.label(RichText::new(tr("MASTER")).strong().color(c(th.accent)));
            let before = st.session.clone();
            let changed = ui
                .add(
                    egui::Slider::new(&mut st.session.master_db, -60.0..=12.0)
                        .suffix(" dB")
                        .text(tr("Master")),
                )
                .changed();
            if changed {
                st.live_master();
            }
            live_commit(st, ui, tr("Master level"), &before, changed);
            ui.separator();
            chain(ui, st, MASTER);
            ui.label(
                RichText::new(tr("Select a track to see its settings."))
                    .size(10.0)
                    .color(c(th.text_dim)),
            );
        }
    }
}

fn track(ui: &mut egui::Ui, st: &mut State, ti: usize) {
    let th = st.theme.clone();
    let before = st.session.clone();
    let kind = st.session.tracks[ti].kind;
    let tid = st.session.tracks[ti].id;
    ui.horizontal(|ui| {
        let t = &mut st.session.tracks[ti];
        let badge = match kind {
            TrackKind::Audio => tr("AUDIO"),
            TrackKind::Bus => tr("BUS"),
            TrackKind::Midi => "MIDI",
        };
        ui.label(RichText::new(badge).small().strong().color(c(t.color)));
        ui.add(egui::TextEdit::singleline(&mut t.name).desired_width(ui.available_width()));
    });
    if ui.small_button(tr("Show Master")).clicked() {
        st.selected_track = None;
        return;
    }
    // Fader and pan.
    let mut changed = false;
    {
        let t = &mut st.session.tracks[ti];
        changed |= ui
            .add(
                egui::Slider::new(&mut t.volume_db, -60.0..=12.0)
                    .suffix(" dB")
                    .text(tr("Volume")),
            )
            .changed();
        if st.session.space.is_none() {
            let mut pan = t.pan * 100.0;
            if ui
                .add(egui::Slider::new(&mut pan, -100.0..=100.0).text(tr("Pan")))
                .changed()
            {
                t.pan = pan / 100.0;
                changed = true;
            }
        }
    }
    if changed {
        st.live_track(ti);
    }
    live_commit(st, ui, tr("Track level"), &before, changed);

    // Output and sends (structural: the mix is rebuilt).
    let buses: Vec<(af27_core::TrackId, String)> = st
        .session
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Bus && t.id != tid)
        .map(|t| (t.id, t.name.clone()))
        .collect();
    let mut structural = false;
    let out_name = st.session.tracks[ti]
        .output
        .and_then(|o| buses.iter().find(|b| b.0 == o))
        .map_or(tr("Master").to_string(), |b| b.1.clone());
    egui::ComboBox::from_label(tr("Output"))
        .selected_text(out_name)
        .show_ui(ui, |ui| {
            let t = &mut st.session.tracks[ti];
            structural |= ui
                .selectable_value(&mut t.output, None, tr("Master"))
                .changed();
            for (id, name) in &buses {
                structural |= ui
                    .selectable_value(&mut t.output, Some(*id), name)
                    .changed();
            }
        });
    ui.collapsing(
        tr!("Sends ({0})", st.session.tracks[ti].sends.len()),
        |ui| {
            let mut remove = None;
            for (i, s) in st.session.tracks[ti].sends.iter_mut().enumerate() {
                let name = buses
                    .iter()
                    .find(|b| b.0 == s.bus)
                    .map_or(tr("(missing bus)").into(), |b| b.1.clone());
                ui.horizontal(|ui| {
                    ui.label(name);
                    structural |= ui
                        .add(
                            egui::DragValue::new(&mut s.level_db)
                                .speed(0.2)
                                .range(-90.0..=12.0)
                                .suffix(" dB"),
                        )
                        .changed();
                    structural |= ui.checkbox(&mut s.pre_fader, tr("Pre")).changed();
                    if ui.small_button("✕").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                st.session.tracks[ti].sends.remove(i);
                structural = true;
            }
            if buses.is_empty() {
                ui.label(
                    RichText::new(tr("Add a bus track to send to it."))
                        .size(10.0)
                        .color(c(th.text_dim)),
                );
            } else {
                ui.menu_button(format!("➕ {}", tr("Add Send")), |ui| {
                    for (id, name) in &buses {
                        if ui.button(name).clicked() {
                            st.session.tracks[ti].sends.push(af27_core::Send {
                                bus: *id,
                                level_db: -6.0,
                                pre_fader: false,
                                ext_id: None,
                            });
                            structural = true;
                            ui.close_menu();
                        }
                    }
                });
            }
        },
    );
    if structural {
        st.commit_session(tr("Routing"), before.clone());
    }

    // 3D position.
    if let Some(layout) = st.session.space {
        ui.collapsing(tr!("3D position ({0})", layout.label()), |ui| {
            let b2 = st.session.clone();
            let mut ch = false;
            {
                let p = &mut st.session.tracks[ti].pos;
                ch |= ui
                    .add(egui::Slider::new(&mut p.x, -1.0..=1.0).text(tr("Left / right")))
                    .changed();
                ch |= ui
                    .add(egui::Slider::new(&mut p.y, -1.0..=1.0).text(tr("Back / front")))
                    .changed();
                ch |= ui
                    .add(egui::Slider::new(&mut p.z, 0.0..=1.0).text(tr("Height")))
                    .changed();
                ch |= ui
                    .add(egui::Slider::new(&mut p.size, 0.0..=1.0).text(tr("Size")))
                    .changed();
                ch |= ui
                    .add(egui::Slider::new(&mut p.width, 0.0..=1.0).text(tr("Width")))
                    .changed();
                ch |= ui
                    .add(
                        egui::Slider::new(&mut p.rotation, -180.0..=180.0)
                            .suffix("°")
                            .text(tr("Rotation")),
                    )
                    .changed();
                ch |= ui
                    .add(
                        egui::Slider::new(&mut p.lfe_db, -96.0..=6.0)
                            .suffix(" dB")
                            .text(tr("LFE send")),
                    )
                    .changed();
            }
            if ch {
                st.live_space(ti);
            }
            live_commit(st, ui, tr("3D position"), &b2, ch);
        });
    }

    // MIDI instrument.
    if kind == TrackKind::Midi {
        ui.collapsing(tr("Instrument (built-in synth)"), |ui| {
            let b2 = st.session.clone();
            let mut ch = false;
            {
                let i = &mut st.session.tracks[ti].instrument;
                ui.horizontal(|ui| {
                    for (w, n) in [
                        (Wave::Sine, tr("Sine")),
                        (Wave::Triangle, tr("Triangle")),
                        (Wave::Saw, tr("Saw")),
                        (Wave::Square, tr("Square")),
                    ] {
                        ch |= ui.selectable_value(&mut i.wave, w, n).changed();
                    }
                });
                ch |= ui
                    .add(
                        egui::Slider::new(&mut i.detune, 0.0..=50.0)
                            .suffix(" ct")
                            .text(tr("Detune")),
                    )
                    .changed();
                ch |= ui
                    .add(
                        egui::Slider::new(&mut i.attack, 0.001..=2.0)
                            .logarithmic(true)
                            .suffix(" s")
                            .text(tr("Attack")),
                    )
                    .changed();
                ch |= ui
                    .add(
                        egui::Slider::new(&mut i.decay, 0.01..=4.0)
                            .logarithmic(true)
                            .suffix(" s")
                            .text(tr("Decay")),
                    )
                    .changed();
                ch |= ui
                    .add(egui::Slider::new(&mut i.sustain, 0.0..=1.0).text(tr("Sustain")))
                    .changed();
                ch |= ui
                    .add(
                        egui::Slider::new(&mut i.release, 0.01..=6.0)
                            .logarithmic(true)
                            .suffix(" s")
                            .text(tr("Release")),
                    )
                    .changed();
                ch |= ui
                    .add(
                        egui::Slider::new(&mut i.cutoff, 40.0..=18_000.0)
                            .logarithmic(true)
                            .suffix(" Hz")
                            .text(tr("Cutoff")),
                    )
                    .changed();
                ch |= ui
                    .add(egui::Slider::new(&mut i.resonance, 0.0..=1.0).text(tr("Resonance")))
                    .changed();
                ch |= ui
                    .add(
                        egui::Slider::new(&mut i.gain_db, -40.0..=6.0)
                            .suffix(" dB")
                            .text(tr("Level")),
                    )
                    .changed();
            }
            if ch {
                st.live_instrument(ti);
            }
            live_commit(st, ui, tr("Instrument"), &b2, ch);
        });
    }
    ui.separator();
    chain(ui, st, ti as u16);
}

/// An effect chain: up to 16 slots, each with bypass, order and settings.
fn chain(ui: &mut egui::Ui, st: &mut State, track: u16) {
    let th = st.theme.clone();
    let title = if track == MASTER {
        tr("Master effects")
    } else {
        tr("Track effects")
    };
    ui.label(RichText::new(title).strong().color(c(th.accent)));
    let before = st.session.clone();
    let slots: Vec<EffectSlot> = st.chain_mut(track).cloned().unwrap_or_default();
    let mut structural = false;
    let mut action: Option<(usize, i32)> = None;
    for (i, slot) in slots.iter().enumerate() {
        let name = crate::fx::name(&slot.effect);
        let open = st.open_slot == Some((track, i));
        ui.horizontal(|ui| {
            let mut on = !slot.bypass;
            if ui
                .checkbox(&mut on, "")
                .on_hover_text(tr("On or bypassed"))
                .changed()
            {
                if let Some(s) = st.chain_mut(track).and_then(|c| c.get_mut(i)) {
                    s.bypass = !on;
                }
                st.live_slot(track, i);
                st.commit_live(tr("Bypass"), before.clone());
            }
            let label = RichText::new(format!("{} {}", i + 1, name)).color(if slot.bypass {
                c(th.text_dim)
            } else {
                c(th.text)
            });
            if ui.selectable_label(open, label).clicked() {
                st.open_slot = if open { None } else { Some((track, i)) };
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("✕").on_hover_text(tr("Remove")).clicked() {
                    action = Some((i, 0));
                }
                if ui.small_button("⬇").clicked() {
                    action = Some((i, 1));
                }
                if ui.small_button("⬆").clicked() {
                    action = Some((i, -1));
                }
            });
        });
        if open {
            let controls = crate::fx::controls(&slot.effect);
            let mut vals = slot.values.clone();
            vals.resize(controls.len(), 0.0);
            let mut ch = false;
            let mut released = false;
            egui::Frame::none()
                .inner_margin(egui::Margin::symmetric(14.0, 2.0))
                .show(ui, |ui| {
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
                            let mut s =
                                egui::Slider::new(&mut vals[k], p.min..=p.max).text(&p.name);
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
                });
            if ch {
                if let Some(s) = st.chain_mut(track).and_then(|c| c.get_mut(i)) {
                    s.values = vals;
                }
                st.live_slot(track, i);
            }
            live_commit(st, ui, &name, &before, ch);
            // Effects that cannot retune while playing are rebuilt when a
            // change is finished.
            if released && crate::fx::is_fx(&slot.effect) {
                st.session_version += 1;
            }
        }
    }
    if let Some((i, dir)) = action {
        if let Some(c) = st.chain_mut(track) {
            match dir {
                0 => {
                    c.remove(i);
                }
                -1 if i > 0 => c.swap(i, i - 1),
                1 if i + 1 < c.len() => c.swap(i, i + 1),
                _ => {}
            }
        }
        st.open_slot = None;
        structural = true;
    }
    if slots.len() < af27_core::session::CHAIN_SLOTS {
        ui.menu_button(format!("➕ {}", tr("Add Effect")), |ui| {
            for (group, list) in crate::fx::catalog() {
                ui.menu_button(group, |ui| {
                    for (id, name) in list {
                        if ui.button(&name).clicked() {
                            if let Some(c) = st.chain_mut(track) {
                                c.push(EffectSlot {
                                    values: crate::fx::defaults(&id),
                                    effect: id.clone(),
                                    bypass: false,
                                    data: None,
                                    ext_id: None,
                                });
                                let n = c.len() - 1;
                                st.open_slot = Some((track, n));
                            }
                            structural = true;
                            ui.close_menu();
                        }
                    }
                });
            }
        });
    }
    if structural {
        st.commit_session(tr("Effect chain"), before);
    }
}
