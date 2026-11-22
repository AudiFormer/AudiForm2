// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The dockable side panels.

use std::sync::Arc;

use egui::{pos2, vec2, Align2, FontId, Rect, RichText, Rounding, Sense, Shape, Stroke};

use crate::state::{Mode, State};
use crate::theme::{c, ca};
use crate::ui::{Caches, Tab};
use af2_core::time::format_time;
use af2_dsp::effects::GROUPS;

// ------------------------------------------------------------------ Files

pub fn files(ui: &mut egui::Ui, st: &mut State, _open: &mut Option<Tab>) {
    crate::files_panel::files(ui, st);
}

// ---------------------------------------------------------------- Markers

pub fn markers(ui: &mut egui::Ui, st: &mut State, caches: &mut Caches) {
    use af2_core::session::MarkerKind;
    ui.horizontal(|ui| {
        if ui.button("➕ Add at Playhead (M)").clicked() {
            st.add_marker();
        }
        egui::ComboBox::from_id_salt("marker-new-kind")
            .selected_text(st.marker_kind.label().to_string())
            .width(90.0)
            .show_ui(ui, |ui| {
                for k in MarkerKind::OFFERED {
                    let l = k.label().to_string();
                    ui.selectable_value(&mut st.marker_kind, k, l);
                }
            })
            .response
            .on_hover_text("The kind new markers get");
        ui.menu_button("Export…", |ui| {
            for (kind, what) in [(MarkerKind::Chapter, "Chapters"), (MarkerKind::AdSlot, "Ad Slots")] {
                ui.menu_button(what, |ui| {
                    for f in crate::chapters::Format::ALL {
                        if ui.button(f.name()).clicked() {
                            ui.close_menu();
                            export_markers(st, &kind, f);
                        }
                    }
                });
            }
        });
    });
    ui.separator();
    let mut jump = None;
    let mut remove = None;
    let mut changed = false;
    let markers: &mut Vec<af2_core::Marker> = match st.mode {
        Mode::Waveform => match st.doc_mut() {
            Some(d) => &mut d.markers,
            None => return,
        },
        Mode::Multitrack => &mut st.session.markers,
    };
    egui::Grid::new("markers")
        .striped(true)
        .num_columns(4)
        .show(ui, |ui| {
            for (i, m) in markers.iter_mut().enumerate() {
                if caches.marker_edit == Some(i) {
                    if ui.text_edit_singleline(&mut m.name).lost_focus() {
                        caches.marker_edit = None;
                        changed = true;
                    }
                } else {
                    let r = ui.selectable_label(false, &m.name);
                    if r.clicked() {
                        jump = Some(m.time);
                    }
                    if r.double_clicked() {
                        caches.marker_edit = Some(i);
                    }
                }
                ui.label(RichText::new(format_time(m.time)).monospace());
                egui::ComboBox::from_id_salt(("marker-kind", i))
                    .selected_text(m.kind.label().to_string())
                    .width(84.0)
                    .show_ui(ui, |ui| {
                        for k in MarkerKind::OFFERED {
                            let l = k.label().to_string();
                            changed |= ui.selectable_value(&mut m.kind, k, l).changed();
                        }
                    });
                if ui.small_button("✕").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
    if let Some(i) = remove {
        markers.remove(i);
        changed = true;
    }
    if changed && st.mode == Mode::Multitrack {
        st.session_dirty = true;
        st.session_version += 1;
    }
    if let Some(t) = jump {
        st.set_cursor(t);
    }
}

/// Saves the chapter or ad list (Markers panel > Export).
fn export_markers(st: &mut State, kind: &af2_core::session::MarkerKind, f: crate::chapters::Format) {
    let (markers, duration, name) = match st.mode {
        Mode::Waveform => match st.doc() {
            Some(d) => (d.markers.clone(), d.duration(), d.name.clone()),
            None => return,
        },
        Mode::Multitrack => (st.session.markers.clone(), st.session.duration(), st.session.name.clone()),
    };
    let list = crate::chapters::of_kind(&markers, kind);
    if list.is_empty() {
        st.toast(format!("There are no {} markers to export.", kind.label()));
        return;
    }
    let stem = std::path::Path::new(&name)
        .file_stem()
        .map_or(name.clone(), |s| s.to_string_lossy().to_string());
    let suffix = if *kind == af2_core::session::MarkerKind::AdSlot { "ads" } else { "chapters" };
    let Some(path) = crate::file_dialog::FileDialog::new()
        .set_file_name(format!("{stem} {suffix}.{}", f.ext()))
        .add_filter(f.name(), &[f.ext()])
        .save_file()
    else {
        return;
    };
    match std::fs::write(&path, crate::chapters::render(&list, f)) {
        Ok(()) => {
            let warn = if *kind == af2_core::session::MarkerKind::Chapter {
                crate::chapters::warnings(&list, duration)
            } else {
                Vec::new()
            };
            if warn.is_empty() {
                st.toast(format!("Saved {} {}", list.len(), suffix));
            } else {
                st.toast(format!("Saved {} {}. Note: {}", list.len(), suffix, warn.join(" ")));
            }
        }
        Err(e) => st.error(format!("Could not save {}: {e}", path.display())),
    }
}

// ---------------------------------------------------------------- Effects

pub fn effects(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    egui::ComboBox::from_id_salt("effect")
        .selected_text(st.effect.name())
        .width(ui.available_width() - 8.0)
        .show_ui(ui, |ui| {
            for (group, list) in GROUPS {
                ui.label(RichText::new(*group).small().color(c(th.text_dim)));
                for e in *list {
                    ui.selectable_value(&mut st.effect, *e, format!("   {}", e.name()));
                }
            }
        });
    let e = st.effect;
    let mut vals = st.effect_values(e);
    let specs = e.params();
    ui.add_space(4.0);
    if specs.is_empty() {
        ui.label(RichText::new("No settings.").color(c(th.text_dim)));
    }
    for (i, p) in specs.iter().enumerate() {
        let mut slider = crate::widgets::Slider::new(&mut vals[i], p.min..=p.max).text(p.name);
        if !p.unit.is_empty() {
            slider = slider.suffix(format!(" {}", p.unit));
        }
        if p.log {
            slider = slider.logarithmic(true);
        }
        ui.add(slider);
    }
    st.effect_values.insert(e, vals);
    ui.add_space(6.0);
    let target = match st.doc() {
        Some(d) if d.sel.is_some() => "selection",
        Some(_) => "whole file",
        None => "no file open",
    };
    ui.horizontal(|ui| {
        let apply = egui::Button::new(RichText::new("Apply").strong().color(c(th.background)))
            .fill(c(th.accent));
        if ui
            .add_enabled(st.doc().is_some(), apply)
            .on_hover_text(format!("Process the {target}"))
            .clicked()
        {
            st.mode = Mode::Waveform;
            st.apply_effect(e);
        }
        if ui
            .button("Preview")
            .on_hover_text("Play the processed selection without changing the file")
            .clicked()
        {
            preview(st);
        }
        if ui.button("Reset").clicked() {
            st.effect_values.remove(&e);
        }
    });
    ui.label(
        RichText::new(format!("Applies to: {target}"))
            .size(10.0)
            .color(c(th.text_dim)),
    );
    if e.realtime() {
        ui.label(
            RichText::new("Also runs live: add it to a track in the Inspector.")
                .size(10.0)
                .color(c(th.text_dim)),
        );
    }
    ui.separator();
    audiform_effects(ui, st);
}

/// The ported Audiform effects, applied to the selection.
fn audiform_effects(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    ui.label(
        RichText::new("Audiform effects")
            .strong()
            .color(c(th.accent)),
    );
    let reg = af2_fx::registry();
    let current = st.fx_pick.clone().unwrap_or_else(|| reg[0].id.to_string());
    egui::ComboBox::from_id_salt("fx-pick")
        .selected_text(crate::fx::name(&current))
        .width(ui.available_width() - 8.0)
        .show_ui(ui, |ui| {
            for info in &reg {
                if ui
                    .selectable_label(
                        current == info.id,
                        format!("{} ({})", info.name, info.category),
                    )
                    .clicked()
                {
                    st.fx_pick = Some(info.id.to_string());
                }
            }
        });
    let id = st.fx_pick.clone().unwrap_or(current);
    let controls = crate::fx::controls(&id);
    let mut vals = st
        .fx_values
        .get(&id)
        .cloned()
        .unwrap_or_else(|| crate::fx::defaults(&id));
    vals.resize(controls.len(), 0.0);
    for (k, p) in controls.iter().enumerate() {
        if p.hidden {
            continue;
        }
        if p.toggle {
            let mut on = vals[k] >= 0.5;
            ui.checkbox(&mut on, &p.name);
            vals[k] = if on { 1.0 } else { 0.0 };
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
            vals[k] = (p.min + pick as f32).min(p.max);
        } else {
            let mut s = crate::widgets::Slider::new(&mut vals[k], p.min..=p.max).text(&p.name);
            if !p.unit.is_empty() {
                s = s.suffix(format!(" {}", p.unit));
            }
            if p.log && p.min > 0.0 {
                s = s.logarithmic(true);
            }
            ui.add(s);
        }
    }
    st.fx_values.insert(id.clone(), vals.clone());
    let apply = egui::Button::new(RichText::new("Apply").strong().color(c(th.background)))
        .fill(c(th.accent));
    if ui.add_enabled(st.doc().is_some(), apply).clicked() {
        let slot = af2_core::EffectSlot {
            position: None,
            sidechain: None,
            effect: id.clone(),
            values: vals,
            bypass: false,
            data: None,
            ext_id: None,
        };
        if let Some(d) = st.doc_mut() {
            let r = d.target();
            if !r.is_empty() {
                let seg = d.buf.slice(r);
                if let Some(out) = crate::fx::process_offline(&slot, &seg) {
                    let out = d.chans.fit_len(out, seg.frames());
                    let mut b = (*d.buf).clone();
                    b.replace(r, &out);
                    d.set_buffer_on(&crate::fx::name(&id), b);
                }
            }
        }
        st.mode = Mode::Waveform;
    }
}

fn preview(st: &mut State) {
    let vals = st.effect_values(st.effect);
    let Some(d) = st.doc() else { return };
    let r = d.target();
    let seg = d.buf.slice(r);
    let out = st.effect.process(&seg, &vals);
    let len = out.duration_secs();
    if let Some(eng) = &mut st.engine {
        eng.set_program(af2_engine::buffer_program(Arc::new(out)));
        eng.play(0.0, None, false, len);
    }
    // Force the normal program back on the next play.
    st.session_version += 1;
}

// ---------------------------------------------------------------- History

pub fn history(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    ui.horizontal(|ui| {
        if ui.button("⟲ Undo").clicked() {
            st.undo();
        }
        if ui.button("⟳ Redo").clicked() {
            st.redo();
        }
    });
    ui.separator();
    let labels: Vec<String> = match st.mode {
        Mode::Waveform => st
            .doc()
            .map(|d| d.history.labels().map(str::to_string).collect())
            .unwrap_or_default(),
        Mode::Multitrack => st.session_history.labels().map(str::to_string).collect(),
    };
    ui.label(
        RichText::new(if st.mode == Mode::Waveform {
            "🗁 Open"
        } else {
            "☰ Session"
        })
        .color(c(th.text_dim)),
    );
    for l in &labels {
        ui.label(format!("• {l}"));
    }
    ui.label(
        RichText::new(format!("{} undo step(s)", labels.len()))
            .size(10.0)
            .color(c(th.text_dim)),
    );
}

// ----------------------------------------------------------------- Levels

pub fn levels(ui: &mut egui::Ui, st: &mut State) {
    use crate::meters::{self, MeterKind, PeakHold, Response};
    let th = st.theme.clone();
    let prefs = st.prefs.meters;
    let rect = ui.available_rect_before_wrap();
    let (rect, resp) = ui.allocate_exact_size(rect.size(), Sense::click());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, Rounding::ZERO, c(th.panel));
    // L on top, the scale between, R below (clear of the panel's corners).
    let latch_w = if prefs.clip { 12.0 } else { 0.0 };
    let bars = Rect::from_min_max(
        rect.min + vec2(18.0, 4.0),
        pos2(rect.right() - 8.0 - latch_w, rect.bottom() - 6.0),
    );
    // A surround file: a bar for each of its channels, before the fold.
    if !st.src_levels.is_empty() {
        let n = st.src_levels.len();
        let names = af2_space::channels::names(n, st.doc().and_then(|d| d.speaker_mask()));
        let gap = 2.0;
        let bar_h = ((bars.height() - gap * (n as f32 - 1.0)) / n as f32).max(2.0);
        for (i, ch) in st.src_levels.iter().enumerate() {
            let r = Rect::from_min_size(
                pos2(bars.left(), bars.top() + i as f32 * (bar_h + gap)),
                vec2(bars.width(), bar_h),
            );
            meters::draw_channel(&p, r, ch, &prefs, &th);
            p.text(
                pos2(rect.left() + 3.0, r.center().y),
                Align2::LEFT_CENTER,
                names.get(i).map_or("", |s| s.as_str()),
                FontId::proportional(if bar_h < 9.0 { 7.5 } else { 9.0 }),
                c(th.text_dim),
            );
            if prefs.clip {
                meters::draw_clip_latch(&p, meters::clip_latch(r), ch.clipped, &th);
            }
        }
        if resp.clicked() {
            for ch in &mut st.src_levels {
                ch.clipped = false;
                ch.reset_hold(prefs.kind);
            }
        }
        resp.on_hover_text(format!(
            "Each of the file's {n} channels before it is folded or placed around \
             the head. Click to clear the clip lights and peak hold"
        ));
        return;
    }
    let gap = if bars.height() > 30.0 { 13.0 } else { 3.0 };
    let bar_h = ((bars.height() - gap) / 2.0).max(2.0);
    for (i, ch) in st.levels.ch.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(bars.left(), bars.top() + i as f32 * (bar_h + gap)),
            vec2(bars.width(), bar_h),
        );
        meters::draw_channel(&p, r, ch, &prefs, &th);
        p.text(
            pos2(rect.left() + 5.0, r.center().y),
            Align2::LEFT_CENTER,
            if i == 0 { "L" } else { "R" },
            FontId::proportional(10.0),
            c(th.text_dim),
        );
        if prefs.clip {
            meters::draw_clip_latch(&p, meters::clip_latch(r), ch.clipped, &th);
        }
    }
    if gap > 10.0 {
        let strip = Rect::from_min_size(
            pos2(bars.left(), bars.top() + bar_h),
            vec2(bars.width(), gap),
        );
        meters::draw_scale(&p, strip, prefs.floor(), &th);
    }
    // A click clears the clip lights and the peak-hold lines.
    if resp.clicked() {
        st.levels.clear_clips();
        st.levels.reset_holds(prefs.kind);
    }
    let hint = if prefs.clip && st.levels.ch.iter().any(|c| c.clipped) {
        "Click to clear the clip lights and peak hold. Right-click for the meter settings"
    } else {
        "Click to reset the peak hold. Right-click for the meter settings"
    };
    let resp = resp.on_hover_text(hint);
    let mut m = prefs;
    resp.context_menu(|ui| {
        ui.menu_button("Range", |ui| {
            for r in meters::RANGES {
                if ui
                    .radio(m.range_db == r, format!("-{r} dB to 0 dB"))
                    .clicked()
                {
                    m.range_db = r;
                    ui.close_menu();
                }
            }
        });
        ui.menu_button("Response", |ui| {
            for r in Response::ALL {
                if ui.radio(m.response == r, r.label()).clicked() {
                    m.response = r;
                    ui.close_menu();
                }
            }
        });
        ui.menu_button("Peak Hold", |ui| {
            for h in PeakHold::ALL {
                if ui.radio(m.hold == h, h.label()).clicked() {
                    m.hold = h;
                    ui.close_menu();
                }
            }
        });
        ui.menu_button("Meter Type", |ui| {
            for k in MeterKind::ALL {
                if ui.radio(m.kind == k, k.label()).clicked() {
                    m.kind = k;
                    ui.close_menu();
                }
            }
        });
        ui.checkbox(&mut m.clip, "Show Clip Indicators");
        ui.separator();
        if ui.button("Reset Peak Hold and Clip Lights").clicked() {
            st.levels.clear_clips();
            st.levels.reset_holds(m.kind);
            ui.close_menu();
        }
    });
    if m != st.prefs.meters {
        if m.kind != st.prefs.meters.kind {
            st.levels.reset_holds(m.kind);
        }
        st.prefs.meters = m;
        st.prefs.save();
    }
}

// ------------------------------------------------------------ Phase scope

// --------------------------------------------------------------- Spectrum

pub fn spectrum(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    let avail = ui.available_rect_before_wrap();
    let (rect, _) = ui.allocate_exact_size(avail.size(), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, Rounding::same(th.rounding), c(th.wave_background));
    let (frames, rate) = st.scope_frames(4096);
    let mono: Vec<f32> = frames.iter().map(|f| (f[0] + f[1]) * 0.5).collect();
    let col = af2_dsp::fft::spectrogram(&mono, 0, mono.len(), 1, 4096);
    let bins = &col[0];
    let ny = rate as f32 / 2.0;
    let x_of = |f: f32| rect.left() + ((f.max(20.0) / 20.0).ln() / (ny / 20.0).ln()) * rect.width();
    let y_of = |db: f32| rect.top() + ((-db) / 100.0).clamp(0.0, 1.0) * rect.height();
    for f in [100.0f32, 1000.0, 10_000.0] {
        let x = x_of(f);
        p.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.0_f32, c(th.grid)),
        );
        p.text(
            pos2(x + 2.0, rect.bottom() - 2.0),
            Align2::LEFT_BOTTOM,
            if f >= 1000.0 {
                format!("{}k", f / 1000.0)
            } else {
                format!("{f}")
            },
            FontId::proportional(9.0),
            c(th.text_dim),
        );
    }
    let mut pts = Vec::new();
    let n = bins.len();
    for (i, db) in bins.iter().enumerate().skip(1) {
        let f = i as f32 / n as f32 * ny;
        pts.push(pos2(x_of(f), y_of(*db)));
    }
    let mut fill = pts.clone();
    fill.push(pos2(rect.right(), rect.bottom()));
    fill.push(pos2(rect.left(), rect.bottom()));
    if th.glow > 0.0 {
        p.add(Shape::line(
            pts.clone(),
            Stroke::new(4.0_f32, ca(th.accent, (40.0 * th.glow) as u8)),
        ));
    }
    p.add(Shape::line(pts, Stroke::new(1.5_f32, c(th.accent))));
}

// -------------------------------------------------------- Level Statistics

pub fn statistics(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    let what = match st.mode {
        Mode::Waveform => st
            .doc()
            .map(|d| if d.sel.is_some() { "selection" } else { "file" }),
        Mode::Multitrack => Some(if st.session_sel.is_some() {
            "mix selection"
        } else {
            "mix"
        }),
    };
    let Some(what) = what else {
        ui.label(RichText::new("Open a file to measure it.").color(c(th.text_dim)));
        return;
    };
    if ui.button(format!("Scan {what}")).clicked() {
        let buf = match st.mode {
            Mode::Waveform => {
                let d = st.doc().unwrap();
                d.buf.slice(d.target())
            }
            Mode::Multitrack => {
                let (a, b) = st.session_sel.unwrap_or((0.0, st.session.duration()));
                st.render_mix(a, b, false)
            }
        };
        st.stats = Some((what.to_string(), af2_dsp::loudness::analyze(&buf)));
    }
    if let Some((label, s)) = &st.stats {
        ui.label(
            RichText::new(format!("Last scan: {label}"))
                .size(10.0)
                .color(c(th.text_dim)),
        );
        egui::Grid::new("stats")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                let row = |ui: &mut egui::Ui, k: &str, v: String| {
                    ui.label(k);
                    ui.label(RichText::new(v).monospace().color(c(th.accent)));
                    ui.end_row();
                };
                row(
                    ui,
                    "Integrated loudness",
                    s.integrated_lufs
                        .map_or("—".into(), |l| format!("{l:.1} LUFS")),
                );
                row(ui, "True peak", format!("{:.1} dBTP", s.true_peak_db));
                row(ui, "Sample peak", format!("{:.1} dBFS", s.sample_peak_db));
                row(ui, "RMS", format!("{:.1} dBFS", s.rms_db));
                row(ui, "DC offset", format!("{:.4} %", s.dc_offset * 100.0));
                row(ui, "Duration", format_time(s.duration));
            });
        ui.add_space(4.0);
        if let Some(l) = s.integrated_lufs {
            for (name, target, tp) in [
                ("Podcast (stereo)", -16.0, -1.0),
                ("Streaming", -14.0, -1.0),
                ("Broadcast (EBU R128)", -23.0, -1.0),
            ] {
                let ok = (l - target).abs() <= 1.0 && s.true_peak_db <= tp;
                ui.label(
                    RichText::new(format!(
                        "{} {name}: {target} LUFS",
                        if ok { "✔" } else { "•" }
                    ))
                    .size(10.0)
                    .color(if ok { c(th.meter_low) } else { c(th.text_dim) }),
                );
            }
        }
    }
}

// ------------------------------------------------------------------ Range

pub fn range(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    let (sel, view) = match st.mode {
        Mode::Waveform => match st.doc() {
            Some(d) => (d.sel, d.view),
            None => return,
        },
        Mode::Multitrack => (st.session_sel, st.session_view),
    };
    let mut changed_sel = None;
    let mut changed_view = None;
    egui::Grid::new("range").num_columns(4).show(ui, |ui| {
        ui.label("");
        for h in ["Start", "End", "Duration"] {
            ui.label(RichText::new(h).size(10.0).color(c(th.text_dim)));
        }
        ui.end_row();
        ui.label("Selection");
        let (mut a, mut b) = sel.unwrap_or((0.0, 0.0));
        let ra = ui.add(
            egui::DragValue::new(&mut a)
                .speed(0.01)
                .range(0.0..=f64::MAX)
                .custom_formatter(|v, _| format_time(v)),
        );
        let rb = ui.add(
            egui::DragValue::new(&mut b)
                .speed(0.01)
                .range(0.0..=f64::MAX)
                .custom_formatter(|v, _| format_time(v)),
        );
        ui.label(RichText::new(format_time((b - a).max(0.0))).monospace());
        if ra.changed() || rb.changed() {
            changed_sel = Some((a.min(b), a.max(b)));
        }
        ui.end_row();
        ui.label("View");
        let (mut s, mut e) = (view.start, view.start + view.len);
        let rs = ui.add(
            egui::DragValue::new(&mut s)
                .speed(0.01)
                .range(0.0..=f64::MAX)
                .custom_formatter(|v, _| format_time(v)),
        );
        let re = ui.add(
            egui::DragValue::new(&mut e)
                .speed(0.01)
                .range(0.0..=f64::MAX)
                .custom_formatter(|v, _| format_time(v)),
        );
        ui.label(RichText::new(format_time(view.len)).monospace());
        if (rs.changed() || re.changed()) && e > s {
            changed_view = Some(crate::state::View {
                start: s,
                len: e - s,
            });
        }
        ui.end_row();
    });
    match st.mode {
        Mode::Waveform => {
            if let Some(d) = st.doc_mut() {
                if let Some(s) = changed_sel {
                    d.sel = Some(s);
                }
                if let Some(v) = changed_view {
                    d.view = v;
                }
            }
        }
        Mode::Multitrack => {
            if let Some(s) = changed_sel {
                st.session_sel = Some(s);
            }
            if let Some(v) = changed_view {
                st.session_view = v;
            }
        }
    }
}
