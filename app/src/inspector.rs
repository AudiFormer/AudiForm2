// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The Inspector: everything about the selected track (output, sends, its
//! effect chain, 3D position, MIDI instrument), or the master chain when
//! no track is selected. Setting changes reach the audio thread at once,
//! without rebuilding the mix; each drag is one undo step.

use egui::RichText;

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
    file_info(ui, st);
    let ti = st.selected_track.and_then(|id| st.session.track_index(id));
    match ti {
        Some(ti) => track(ui, st, ti),
        None => {
            ui.label(RichText::new("MASTER").strong().color(c(th.accent)));
            let before = st.session.clone();
            let changed = ui
                .add(
                    crate::widgets::Slider::new(&mut st.session.master_db, -60.0..=12.0)
                        .suffix(" dB")
                        .text("Master"),
                )
                .changed();
            if changed {
                st.live_master();
            }
            live_commit(st, ui, "Master level", &before, changed);
            ui.separator();
            rack_link(ui, st, MASTER);
            ui.label(
                RichText::new("Select a track to see its settings.")
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
            TrackKind::Audio => "AUDIO",
            TrackKind::Bus => "BUS",
            TrackKind::Midi => "MIDI",
        };
        ui.label(RichText::new(badge).small().strong().color(c(t.color)));
        ui.add(egui::TextEdit::singleline(&mut t.name).desired_width(ui.available_width()));
    });
    if ui.small_button("Show Master").clicked() {
        st.selected_track = None;
        return;
    }
    // Fader and pan.
    let mut changed = false;
    {
        let t = &mut st.session.tracks[ti];
        changed |= ui
            .add(
                crate::widgets::Slider::new(&mut t.volume_db, -60.0..=12.0)
                    .suffix(" dB")
                    .text("Volume"),
            )
            .changed();
        if st.session.space.is_none() {
            let mut pan = t.pan * 100.0;
            if ui
                .add(crate::widgets::Slider::new(&mut pan, -100.0..=100.0).text("Pan"))
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
    live_commit(st, ui, "Track level", &before, changed);

    // Output and sends (structural: the mix is rebuilt).
    // Only buses that would not feed back into this track.
    let buses: Vec<(af27_core::TrackId, String)> = st
        .session
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Bus && t.id != tid && st.session.can_route(tid, t.id))
        .map(|t| (t.id, t.name.clone()))
        .collect();
    let mut structural = false;
    ui.horizontal_wrapped(|ui| {
        let t = &mut st.session.tracks[ti];
        structural |= ui
            .checkbox(&mut t.mono, "Mono")
            .on_hover_text("Fold left and right together after the effects")
            .changed();
        if t.kind != TrackKind::Bus {
            structural |= ui.checkbox(&mut t.polarity, "Invert polarity").changed();
        }
        structural |= ui
            .checkbox(&mut t.solo_safe, "Solo safe")
            .on_hover_text("Stays heard while other tracks are soloed")
            .changed();
    });
    egui::ComboBox::from_label("Automation")
        .selected_text(st.session.tracks[ti].automation.label())
        .show_ui(ui, |ui| {
            let t = &mut st.session.tracks[ti];
            for m in af27_core::automation::AutomationMode::ALL {
                structural |= ui
                    .selectable_value(&mut t.automation, m, m.label())
                    .changed();
            }
        });
    let out_name = st.session.tracks[ti]
        .output
        .and_then(|o| buses.iter().find(|b| b.0 == o))
        .map_or("Master".to_string(), |b| b.1.clone());
    egui::ComboBox::from_label("Output")
        .selected_text(out_name)
        .show_ui(ui, |ui| {
            let t = &mut st.session.tracks[ti];
            structural |= ui.selectable_value(&mut t.output, None, "Master").changed();
            for (id, name) in &buses {
                structural |= ui
                    .selectable_value(&mut t.output, Some(*id), name)
                    .changed();
            }
        });
    ui.collapsing(
        format!("Sends ({})", st.session.tracks[ti].sends.len()),
        |ui| {
            let mut remove = None;
            for (i, s) in st.session.tracks[ti].sends.iter_mut().enumerate() {
                let name = buses
                    .iter()
                    .find(|b| b.0 == s.bus)
                    .map_or("(missing bus)".into(), |b| b.1.clone());
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
                    let mut pan = s.pan * 100.0;
                    if ui
                        .add(
                            egui::DragValue::new(&mut pan)
                                .speed(0.5)
                                .range(-100.0..=100.0)
                                .prefix("Pan "),
                        )
                        .changed()
                    {
                        s.pan = pan / 100.0;
                        structural = true;
                    }
                    structural |= ui.checkbox(&mut s.pre_fader, "Pre").changed();
                    structural |= ui.checkbox(&mut s.enabled, "On").changed();
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
                    RichText::new("Add a bus track to send to it.")
                        .size(10.0)
                        .color(c(th.text_dim)),
                );
            } else {
                ui.menu_button("➕ Add Send", |ui| {
                    for (id, name) in &buses {
                        if ui.button(name).clicked() {
                            st.session.tracks[ti].sends.push(af27_core::Send {
                                bus: *id,
                                level_db: -6.0,
                                pre_fader: false,
                                pan: 0.0,
                                enabled: true,
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
        st.commit_session("Routing", before.clone());
    }

    // 3D position.
    if let Some(layout) = st.session.space {
        ui.collapsing(format!("3D position ({})", layout.label()), |ui| {
            crate::space::spot_controls(ui, st, ti);
        });
    }

    // MIDI instrument.
    if kind == TrackKind::Midi {
        instrument_choice(ui, st, ti);
    }
    if kind == TrackKind::Midi && st.session.tracks[ti].soundfont.is_none() {
        ui.collapsing("Instrument (built-in synth)", |ui| {
            let b2 = st.session.clone();
            let mut ch = false;
            {
                let i = &mut st.session.tracks[ti].instrument;
                ui.horizontal(|ui| {
                    for (w, n) in [
                        (Wave::Sine, "Sine"),
                        (Wave::Triangle, "Triangle"),
                        (Wave::Saw, "Saw"),
                        (Wave::Square, "Square"),
                    ] {
                        ch |= ui.selectable_value(&mut i.wave, w, n).changed();
                    }
                });
                ch |= ui
                    .add(
                        crate::widgets::Slider::new(&mut i.detune, 0.0..=50.0)
                            .suffix(" ct")
                            .text("Detune"),
                    )
                    .changed();
                ch |= ui
                    .add(
                        crate::widgets::Slider::new(&mut i.attack, 0.001..=2.0)
                            .logarithmic(true)
                            .suffix(" s")
                            .text("Attack"),
                    )
                    .changed();
                ch |= ui
                    .add(
                        crate::widgets::Slider::new(&mut i.decay, 0.01..=4.0)
                            .logarithmic(true)
                            .suffix(" s")
                            .text("Decay"),
                    )
                    .changed();
                ch |= ui
                    .add(crate::widgets::Slider::new(&mut i.sustain, 0.0..=1.0).text("Sustain"))
                    .changed();
                ch |= ui
                    .add(
                        crate::widgets::Slider::new(&mut i.release, 0.01..=6.0)
                            .logarithmic(true)
                            .suffix(" s")
                            .text("Release"),
                    )
                    .changed();
                ch |= ui
                    .add(
                        crate::widgets::Slider::new(&mut i.cutoff, 40.0..=18_000.0)
                            .logarithmic(true)
                            .suffix(" Hz")
                            .text("Cutoff"),
                    )
                    .changed();
                ch |= ui
                    .add(crate::widgets::Slider::new(&mut i.resonance, 0.0..=1.0).text("Resonance"))
                    .changed();
                ch |= ui
                    .add(
                        crate::widgets::Slider::new(&mut i.gain_db, -40.0..=6.0)
                            .suffix(" dB")
                            .text("Level"),
                    )
                    .changed();
            }
            if ch {
                st.live_instrument(ti);
            }
            live_commit(st, ui, "Instrument", &b2, ch);
        });
    }
    ui.separator();
    rack_link(ui, st, ti as u16);
}

/// The rack lives in the Effects panel; here, how many and a way there.
fn rack_link(ui: &mut egui::Ui, st: &mut State, track: u16) {
    let th = st.theme.clone();
    let n = st.chain_mut(track).map_or(0, |c| c.len());
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "Effects rack: {n} effect{}",
                if n == 1 { "" } else { "s" }
            ))
            .color(c(th.text_dim)),
        );
        if ui.button("Open Rack").clicked() {
            st.rack_tab = if track == MASTER {
                crate::rack::RackTab::Master
            } else {
                crate::rack::RackTab::Track
            };
            st.want_effects = true;
        }
    });
}

/// Picks a SoundFont file.
fn pick_sf2() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new()
        .add_filter("SoundFont", &["sf2"])
        .pick_file()
}

/// A MIDI track's instrument: the built-in synth or a SoundFont, with its
/// file, preset and level.
fn instrument_choice(ui: &mut egui::Ui, st: &mut State, ti: usize) {
    let th = st.theme.clone();
    let current = st.session.tracks[ti].soundfont.clone();
    let mut want_font = false;
    let mut want_builtin = false;
    ui.horizontal(|ui| {
        ui.label("Instrument");
        egui::ComboBox::from_id_salt(("instrument", ti))
            .selected_text(if current.is_some() {
                "SoundFont (.sf2)"
            } else {
                "Built-in synth"
            })
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(current.is_none(), "Built-in synth")
                    .clicked()
                {
                    want_builtin = true;
                }
                if ui
                    .selectable_label(current.is_some(), "SoundFont (.sf2)…")
                    .clicked()
                {
                    want_font = true;
                }
            });
    });
    if want_builtin && current.is_some() {
        let before = st.session.clone();
        st.session.tracks[ti].soundfont = None;
        st.commit_session("Instrument", before);
    }
    let set_file =
        |st: &mut State, path: std::path::PathBuf| match af27_engine::soundfont::load(&path) {
            Ok(font) => {
                let list = af27_engine::soundfont::presets(&font);
                let keep = st.session.tracks[ti].soundfont.clone();
                // Keep the preset when it is in the new file; else the first.
                let pick = keep
                    .as_ref()
                    .and_then(|k| {
                        list.iter()
                            .find(|p| p.bank == k.bank && p.program == k.program)
                    })
                    .or_else(|| list.first())
                    .cloned();
                let before = st.session.clone();
                st.session.tracks[ti].soundfont = Some(af27_core::midi::SoundFontRef {
                    path,
                    bank: pick.as_ref().map_or(0, |p| p.bank),
                    program: pick.as_ref().map_or(0, |p| p.program),
                    preset_name: pick.map(|p| p.name).unwrap_or_default(),
                    gain_db: keep.map_or(0.0, |k| k.gain_db),
                });
                st.commit_session("Instrument", before);
            }
            Err(e) => st.error(e),
        };
    if want_font && current.is_none() {
        if let Some(p) = pick_sf2() {
            set_file(st, p);
        }
    }
    let Some(sf) = st.session.tracks[ti].soundfont.clone() else {
        return;
    };
    ui.horizontal(|ui| {
        let name = sf
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        ui.label(RichText::new(name).strong())
            .on_hover_text(sf.path.display().to_string());
        if ui.small_button("Change File…").clicked() {
            if let Some(p) = pick_sf2() {
                set_file(st, p);
            }
        }
    });
    match af27_engine::soundfont::load(&sf.path) {
        Ok(font) => {
            let list = af27_engine::soundfont::presets(&font);
            let mut pick = None;
            egui::ComboBox::from_id_salt(("preset", ti))
                .selected_text(format!(
                    "{:03}:{:03} {}",
                    sf.bank, sf.program, sf.preset_name
                ))
                .width(ui.available_width().min(260.0))
                .height(320.0)
                .show_ui(ui, |ui| {
                    for p in &list {
                        let on = p.bank == sf.bank && p.program == sf.program;
                        if ui
                            .selectable_label(
                                on,
                                format!("{:03}:{:03} {}", p.bank, p.program, p.name),
                            )
                            .clicked()
                        {
                            pick = Some(p.clone());
                        }
                    }
                });
            if let Some(p) = pick {
                let before = st.session.clone();
                if let Some(x) = st.session.tracks[ti].soundfont.as_mut() {
                    x.bank = p.bank;
                    x.program = p.program;
                    x.preset_name = p.name;
                }
                st.commit_session("Instrument preset", before);
            }
        }
        Err(e) => {
            ui.label(RichText::new(format!("Silent: {e}")).color(c(th.text_dim)));
        }
    }
    let before = st.session.clone();
    let mut db = sf.gain_db;
    let ch = ui
        .add(
            crate::widgets::Slider::new(&mut db, -40.0..=12.0)
                .suffix(" dB")
                .text("Level"),
        )
        .changed();
    if ch {
        if let Some(x) = st.session.tracks[ti].soundfont.as_mut() {
            x.gain_db = db;
        }
        st.live_instrument(ti);
    }
    live_commit(st, ui, "Instrument level", &before, ch);
}

/// An effect chain: up to 16 slots, each with bypass, order and settings.
#[allow(dead_code)]
fn chain(ui: &mut egui::Ui, st: &mut State, track: u16) {
    let th = st.theme.clone();
    let title = if track == MASTER {
        "Master effects"
    } else {
        "Track effects"
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
                .on_hover_text("On or bypassed")
                .changed()
            {
                if let Some(s) = st.chain_mut(track).and_then(|c| c.get_mut(i)) {
                    s.bypass = !on;
                }
                st.live_slot(track, i);
                st.commit_live("Bypass", before.clone());
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
                if ui.small_button("✕").on_hover_text("Remove").clicked() {
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
                            let mut s = crate::widgets::Slider::new(&mut vals[k], p.min..=p.max)
                                .text(&p.name);
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
        ui.menu_button("➕ Add Effect", |ui| {
            for (group, list) in crate::fx::catalog() {
                ui.menu_button(group, |ui| {
                    for (id, name) in list {
                        if ui.button(&name).clicked() {
                            if let Some(c) = st.chain_mut(track) {
                                c.push(EffectSlot {
                                    sidechain: None,
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
        st.commit_session("Effect chain", before);
    }
}

/// Read-only tags of the open file and what was found in it (markers,
/// chapters), shown in Waveform view.
fn file_info(ui: &mut egui::Ui, st: &State) {
    if st.mode != crate::state::Mode::Waveform {
        return;
    }
    let Some(d) = st.doc() else { return };
    let rows = d.info.tags.rows();
    let found = d.info.markers.len();
    if rows.is_empty() && found == 0 {
        return;
    }
    let th = &st.theme;
    egui::CollapsingHeader::new(RichText::new("FILE INFO").strong().color(c(th.accent)))
        .id_salt(("file-info", d.id))
        .default_open(true)
        .show(ui, |ui| {
            egui::Grid::new(("file-info-grid", d.id))
                .num_columns(2)
                .spacing([8.0, 2.0])
                .show(ui, |ui| {
                    for (k, v) in &rows {
                        ui.label(RichText::new(k).size(10.0).color(c(th.text_dim)));
                        ui.add(egui::Label::new(RichText::new(v).size(11.0)).wrap());
                        ui.end_row();
                    }
                });
            if found > 0 {
                use af27_io::meta::MarkerKind;
                let n = |k| d.info.markers.iter().filter(|m| m.kind == k).count();
                let mut parts = Vec::new();
                for (k, one, many) in [
                    (MarkerKind::Cue, "cue", "cues"),
                    (MarkerKind::Range, "range", "ranges"),
                    (MarkerKind::Chapter, "chapter", "chapters"),
                ] {
                    match n(k) {
                        0 => {}
                        1 => parts.push(format!("1 {one}")),
                        x => parts.push(format!("{x} {many}")),
                    }
                }
                ui.label(
                    RichText::new(format!("From the file: {}", parts.join(", ")))
                        .size(10.0)
                        .color(c(th.text_dim)),
                );
            }
        });
    ui.separator();
}
