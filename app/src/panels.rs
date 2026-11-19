// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The dockable side panels.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use egui::{pos2, vec2, Align2, FontId, Rect, RichText, Rounding, Sense, Shape, Stroke};

use crate::state::{Mode, State};
use crate::theme::{c, ca};
use crate::ui::{meter_db, Caches, Tab};
use af27_core::time::format_time;
use af27_dsp::effects::GROUPS;

// ------------------------------------------------------------------ Files

pub fn files(ui: &mut egui::Ui, st: &mut State, _open: &mut Option<Tab>) {
    let th = st.theme.clone();
    ui.horizontal(|ui| {
        if ui
            .button("🗁 Open")
            .on_hover_text("Open audio or a session (Ctrl+O)")
            .clicked()
        {
            crate::ui::open_dialog(st);
        }
        if ui
            .button("➕ New")
            .on_hover_text("New empty audio file")
            .clicked()
        {
            st.new_file(48_000, 2);
        }
    });
    ui.separator();
    let docs: Vec<_> = st
        .docs
        .iter()
        .map(|d| {
            (
                d.id,
                d.name.clone(),
                d.dirty,
                d.duration(),
                d.buf.rate,
                d.buf.num_channels(),
            )
        })
        .collect();
    let mut close = None;
    for (id, name, dirty, dur, rate, ch) in docs {
        let on = st.current == Some(id);
        let drag_id = egui::Id::new(("file", id));
        let r = ui
            .dnd_drag_source(drag_id, id, |ui| {
                ui.vertical(|ui| {
                    let icon = if ch > 2 { "🔊" } else { "🎵" };
                    let text =
                        RichText::new(format!("{icon} {name}{}", if dirty { " *" } else { "" }))
                            .color(if on { c(th.accent) } else { c(th.text) });
                    ui.add(egui::Label::new(text).truncate());
                    ui.label(
                        RichText::new(format!("     {} • {} Hz • {ch} ch", format_time(dur), rate))
                            .size(10.0)
                            .color(c(th.text_dim)),
                    );
                });
            })
            .response;
        let r = r.interact(Sense::click());
        if r.clicked() {
            st.current = Some(id);
            st.mode = Mode::Waveform;
        }
        r.context_menu(|ui| {
            if ui.button("Edit in Waveform").clicked() {
                st.current = Some(id);
                st.mode = Mode::Waveform;
                ui.close_menu();
            }
            if ui.button("Insert into Multitrack at playhead").clicked() {
                if let Some(t) = st
                    .selected_track
                    .or(st.session.tracks.first().map(|t| t.id))
                {
                    let at = st.session_cursor;
                    st.place_doc(id, t, at);
                }
                st.mode = Mode::Multitrack;
                ui.close_menu();
            }
            if ui.button("Close").clicked() {
                close = Some(id);
                ui.close_menu();
            }
        });
    }
    if let Some(id) = close {
        st.close_doc(id);
    }
    for name in st.loading() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new(name).color(c(th.text_dim)));
        });
    }
    if st.docs.is_empty() && st.jobs.is_empty() {
        ui.label(RichText::new("Drop audio files anywhere in the window.").color(c(th.text_dim)));
    }
}

// ---------------------------------------------------------------- Markers

pub fn markers(ui: &mut egui::Ui, st: &mut State, caches: &mut Caches) {
    ui.horizontal(|ui| {
        if ui.button("➕ Add at Playhead (M)").clicked() {
            st.add_marker();
        }
    });
    ui.separator();
    let mut jump = None;
    let mut remove = None;
    let markers: &mut Vec<af27_core::Marker> = match st.mode {
        Mode::Waveform => match st.doc_mut() {
            Some(d) => &mut d.markers,
            None => return,
        },
        Mode::Multitrack => &mut st.session.markers,
    };
    egui::Grid::new("markers")
        .striped(true)
        .num_columns(3)
        .show(ui, |ui| {
            for (i, m) in markers.iter_mut().enumerate() {
                if caches.marker_edit == Some(i) {
                    if ui.text_edit_singleline(&mut m.name).lost_focus() {
                        caches.marker_edit = None;
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
                if ui.small_button("✕").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
    if let Some(i) = remove {
        markers.remove(i);
    }
    if let Some(t) = jump {
        st.set_cursor(t);
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
        let mut slider = egui::Slider::new(&mut vals[i], p.min..=p.max).text(p.name);
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
    let reg = af27_fx::registry();
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
            let mut s = egui::Slider::new(&mut vals[k], p.min..=p.max).text(&p.name);
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
        let slot = af27_core::EffectSlot {
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
                    let mut b = (*d.buf).clone();
                    b.replace(r, &out);
                    d.set_buffer(&crate::fx::name(&id), b);
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
        eng.set_program(af27_engine::buffer_program(Arc::new(out)));
        eng.play(0.0, None, false, len);
    }
    // Force the normal program back on the next play.
    st.session_version += 1;
}

// ------------------------------------------------------------ AI Stems

pub fn separation(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    let dirs = st.sep_dirs.clone();
    let catalog = af27_separation::catalog(&dirs);
    // Engine status.
    let engine = dirs.engine();
    match &engine {
        Some(p) => {
            if st.separation.engine_version.is_none() {
                st.separation.engine_version =
                    Some(af27_separation::engine_version(p).unwrap_or_else(|e| e));
            }
            let v = st.separation.engine_version.clone().unwrap_or_default();
            let ok = af27_separation::engine_is_new_enough(&v);
            let shared = dirs.legacy.as_ref().is_some_and(|l| p.starts_with(l));
            ui.label(
                RichText::new(format!(
                    "Engine {v} {}{}",
                    if ok { "✔" } else { "(too old)" },
                    if shared {
                        " • shared with Audiform"
                    } else {
                        ""
                    }
                ))
                .color(if ok {
                    c(th.meter_low)
                } else {
                    c(th.accent_alt)
                }),
            );
        }
        None => {
            ui.label(
                RichText::new("The separation engine is not installed.").color(c(th.accent_alt)),
            );
            if ui.button("Download Engine").clicked() && st.separation.job.is_none() {
                start_engine_download(st);
            }
        }
    }
    ui.label(
        RichText::new("Runs locally on your computer. Speed depends on your hardware.")
            .size(10.0)
            .color(c(th.text_dim)),
    );
    ui.separator();

    // Running job.
    if let Some(job) = st.separation.job.clone() {
        let g = job.lock().unwrap();
        ui.label(format!("{}: {}", st.separation.job_kind, g.status));
        ui.add(
            egui::ProgressBar::new(g.fraction)
                .show_percentage()
                .animate(!g.done),
        );
        let finished = g.done;
        let err = g.error.clone();
        let outs = g.outputs.clone();
        drop(g);
        if !finished && ui.button("Cancel").clicked() {
            st.separation.cancel.store(true, Ordering::Relaxed);
        }
        if finished {
            st.separation.job = None;
            st.separation.engine_version = None;
            match err {
                Some(e) => st.error(e),
                None => {
                    if !outs.is_empty() {
                        for (_, p) in &outs {
                            st.load_file(p.clone(), None);
                        }
                        st.separation.last_outputs = outs;
                        st.toast("Separation finished. The stems are in the Files panel.");
                    }
                }
            }
        }
        ui.separator();
    }
    if !st.separation.last_outputs.is_empty() && ui.button("Put Last Stems on New Tracks").clicked()
    {
        st.stems_to_tracks();
    }

    // Models.
    let mut cats: Vec<String> = catalog.iter().map(|m| m.category.clone()).collect();
    cats.dedup();
    for cat in cats {
        ui.label(
            RichText::new(&cat)
                .strong()
                .size(11.0)
                .color(c(th.text_dim)),
        );
        for m in catalog.iter().filter(|m| m.category == cat) {
            let installed = dirs.model(m).is_some();
            let on = st.separation.model.as_deref() == Some(m.id.as_str());
            ui.horizontal(|ui| {
                let label = format!(
                    "{} {}",
                    m.name,
                    m.sdr.map(|s| format!("(SDR {s:.1})")).unwrap_or_default()
                );
                let r = ui.selectable_label(
                    on,
                    RichText::new(label).color(if installed {
                        c(th.text)
                    } else {
                        c(th.text_dim)
                    }),
                );
                let r = r.on_hover_text(format!(
                    "{}\nBy {}\n{}\nResults: {}",
                    m.sdr_note,
                    m.author,
                    m.licence,
                    m.output_names().join(", ")
                ));
                if r.clicked() {
                    st.separation.model = Some(m.id.clone());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if installed {
                        ui.label(RichText::new("✔").color(c(th.meter_low)));
                    } else if ui.small_button(format!("⬇ {} MB", m.bytes >> 20)).clicked()
                        && st.separation.job.is_none()
                    {
                        start_model_download(st, m.clone());
                    }
                });
            });
        }
    }
    ui.separator();
    ui.checkbox(&mut st.separation.gpu, "Use the graphics card");
    ui.add(egui::Slider::new(&mut st.separation.overlap, 1..=8).text("Overlap (smoother, slower)"));
    let model = st
        .separation
        .model
        .as_ref()
        .and_then(|id| catalog.iter().find(|m| &m.id == id))
        .cloned();
    let can = model.as_ref().is_some_and(|m| dirs.model(m).is_some())
        && engine.is_some()
        && st.doc().is_some()
        && st.separation.job.is_none();
    let label = match st.doc() {
        Some(d) => format!("Separate \"{}\"", d.name),
        None => "Open a file to separate".into(),
    };
    let b = egui::Button::new(RichText::new(label).strong().color(c(th.background)))
        .fill(c(th.accent_alt));
    if ui.add_enabled(can, b).clicked() {
        let m = model.unwrap();
        let d = st.doc().unwrap();
        let src = d.buf.clone();
        let name = d
            .name
            .rsplit_once('.')
            .map_or(d.name.as_str(), |(a, _)| a)
            .to_string();
        let out_dir = d
            .path
            .as_ref()
            .and_then(|p| p.parent().map(|p| p.join("Separated")))
            .unwrap_or_else(|| crate::state::data_dir().join("Separated"));
        st.separation.cancel = Arc::new(Default::default());
        st.separation.job_kind = m.name.clone();
        st.separation.job = Some(af27_separation::spawn(
            dirs,
            m,
            src,
            name,
            out_dir,
            st.separation.gpu,
            Some(st.separation.overlap),
            st.separation.cancel.clone(),
        ));
    }
}

fn start_model_download(st: &mut State, m: af27_separation::Model) {
    let dirs = st.sep_dirs.clone();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shared: af27_separation::Shared = Default::default();
    st.separation.cancel = cancel.clone();
    st.separation.job_kind = format!("Downloading {}", m.name);
    st.separation.job = Some(shared.clone());
    std::thread::spawn(move || {
        let dest = dirs.own_model_path(&m);
        let r = af27_separation::download(&m.url, &dest, &m.sha256, &cancel, &shared, m.bytes);
        let mut g = shared.lock().unwrap();
        g.done = true;
        match r {
            Ok(()) => g.status = "Downloaded".into(),
            Err(e) => g.error = Some(e),
        }
    });
}

fn start_engine_download(st: &mut State) {
    let dirs = st.sep_dirs.clone();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shared: af27_separation::Shared = Default::default();
    st.separation.cancel = cancel.clone();
    st.separation.job_kind = "Separation engine".into();
    st.separation.job = Some(shared.clone());
    std::thread::spawn(move || {
        let r = af27_separation::install_engine(&dirs, &cancel, &shared);
        let mut g = shared.lock().unwrap();
        g.done = true;
        match r {
            Ok(()) => g.status = "Installed".into(),
            Err(e) => g.error = Some(e),
        }
    });
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
    let th = st.theme.clone();
    let rect = ui.available_rect_before_wrap();
    let (rect, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, Rounding::ZERO, c(th.panel));
    let scale_h = 12.0;
    let bars = Rect::from_min_max(
        rect.min + vec2(28.0, 2.0),
        pos2(rect.right() - 6.0, rect.bottom() - scale_h),
    );
    let bar_h = ((bars.height() - 3.0) / 2.0).max(2.0);
    let x_of = |db: f32| bars.left() + ((db + 60.0) / 60.0).clamp(0.0, 1.0) * bars.width();
    for (i, (v, hold)) in [(st.meter.0, st.meter_hold.0), (st.meter.1, st.meter_hold.1)]
        .into_iter()
        .enumerate()
    {
        let r = Rect::from_min_size(
            pos2(bars.left(), bars.top() + i as f32 * (bar_h + 3.0)),
            vec2(bars.width(), bar_h),
        );
        p.rect_filled(r, Rounding::same(2.0), c(th.wave_background));
        let db = meter_db(v);
        // Segmented gradient fill.
        let mut x = r.left();
        while x < x_of(db) {
            let seg_db = (x - bars.left()) / bars.width() * 60.0 - 60.0;
            p.rect_filled(
                Rect::from_min_max(pos2(x, r.top()), pos2((x + 3.0).min(x_of(db)), r.bottom())),
                Rounding::ZERO,
                th.meter_color(seg_db),
            );
            x += 4.0;
        }
        let hx = x_of(meter_db(hold));
        p.line_segment(
            [pos2(hx, r.top()), pos2(hx, r.bottom())],
            Stroke::new(2.0_f32, th.meter_color(meter_db(hold))),
        );
        p.text(
            pos2(rect.left() + 4.0, r.center().y),
            Align2::LEFT_CENTER,
            if i == 0 { "L" } else { "R" },
            FontId::proportional(10.0),
            c(th.text_dim),
        );
        if hold >= 1.0 {
            p.rect_filled(
                Rect::from_min_size(pos2(bars.right() + 1.0, r.top()), vec2(4.0, r.height())),
                Rounding::ZERO,
                c(th.meter_high),
            );
        }
    }
    let mut db = -60;
    while db <= 0 {
        let x = x_of(db as f32);
        p.text(
            pos2(x, rect.bottom() - 1.0),
            Align2::CENTER_BOTTOM,
            db.to_string(),
            FontId::proportional(9.0),
            c(th.text_dim),
        );
        db += 3;
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
    let col = af27_dsp::fft::spectrogram(&mono, 0, mono.len(), 1, 4096);
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
        st.stats = Some((what.to_string(), af27_dsp::loudness::analyze(&buf)));
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
