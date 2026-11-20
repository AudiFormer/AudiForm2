// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The tempo map in the interface: Multitrack > Tempo and Time
//! Signature... (the start and every change, add at the playhead's bar,
//! edit, delete), the bar grid the Multitrack editor draws, and `.mid`
//! import and export.

use std::path::PathBuf;

use egui::{pos2, Align2, FontId, Rect, RichText, Stroke};

use crate::state::State;
use crate::theme::{c, ca};
use af27_core::midi::{Tempo, BEAT_UNITS, BPM_MAX, BPM_MIN};

/// Tempo window state.
#[derive(Default)]
pub struct TempoUi {
    pub open: bool,
    /// An imported `.mid` waiting for an answer: replace the session's
    /// tempo map with the file's?
    pub pending: Option<(PathBuf, af27_interop::smf::SmfImport)>,
}

/// One undo step per gesture, and the mix rebuilt to the new map.
fn tempo_commit(st: &mut State, ui: &egui::Ui, before: &af27_core::Session, changed: bool) {
    let down = ui.input(|i| i.pointer.any_down());
    if changed {
        st.session.tempo.normalize();
        st.session_version += 1;
        st.live_metronome();
        if !st.gesture {
            st.commit_live("Tempo map", before.clone());
            st.gesture = down;
        }
    }
    if !down {
        st.gesture = false;
    }
}

/// The bar (from 0) the playhead is in.
pub fn playhead_bar(st: &State) -> u32 {
    let t = &st.session.tempo;
    t.bar_at(t.beats(st.position().max(0.0))).0
}

pub fn window(ctx: &egui::Context, st: &mut State) {
    let mut open = st.tempo_ui.open;
    crate::ui::popup(ctx, egui::Id::new(("af2-popup", "tempo.rs", 0)), "Tempo and Time Signature")
        .open(&mut open)
        .resizable(false)
        .default_width(360.0)
        .show(ctx, |ui| {
            let th = st.theme.clone();
            ui.label(
                RichText::new(
                    "Changes start on a bar line and hold until the next one. Beats are quarter notes; a bar of 6/8 is three of them.",
                )
                .color(c(th.text_dim)),
            );
            ui.add_space(4.0);
            let before = st.session.clone();
            let mut changed = false;
            let mut delete = None;
            let here = st
                .session
                .tempo
                .segments()
                .filter(|s| s.secs <= st.position() + 1e-9)
                .last()
                .and_then(|s| s.change);
            egui::Grid::new("tempo-map")
                .num_columns(5)
                .spacing([10.0, 6.0])
                .striped(true)
                .show(ui, |ui| {
                    ui.label(RichText::new("Bar").strong());
                    ui.label(RichText::new("Tempo").strong());
                    ui.label(RichText::new("Time signature").strong());
                    ui.label(RichText::new("Starts at").strong());
                    ui.label("");
                    ui.end_row();
                    let n = st.session.tempo.changes.len();
                    for row in 0..=n {
                        let idx = row.checked_sub(1);
                        let secs = st
                            .session
                            .tempo
                            .segments()
                            .find(|s| s.change == idx)
                            .map(|s| s.secs);
                        let mut zero = 0u32;
                        let t = &mut st.session.tempo;
                        let (bar, bpm, top, unit) = match idx {
                            None => (&mut zero, &mut t.bpm, &mut t.beats_per_bar, &mut t.beat_unit),
                            Some(i) => {
                                let c = &mut t.changes[i];
                                (&mut c.bar, &mut c.bpm, &mut c.beats_per_bar, &mut c.beat_unit)
                            }
                        };
                        let mark = if idx == here { "▶ " } else { "" };
                        if idx.is_none() {
                            ui.label(format!("{mark}1"));
                        } else {
                            ui.horizontal(|ui| {
                                ui.label(mark);
                                let mut shown = *bar + 1;
                                if ui
                                    .add(egui::DragValue::new(&mut shown).range(2..=9999).speed(0.1))
                                    .on_hover_text("The bar it starts on")
                                    .changed()
                                {
                                    *bar = shown - 1;
                                    changed = true;
                                }
                            });
                        }
                        changed |= ui
                            .add(
                                egui::DragValue::new(bpm)
                                    .range(BPM_MIN..=BPM_MAX)
                                    .speed(0.25)
                                    .max_decimals(2)
                                    .suffix(" BPM"),
                            )
                            .changed();
                        ui.horizontal(|ui| {
                            changed |= ui
                                .add(egui::DragValue::new(top).range(1..=32).speed(0.05))
                                .changed();
                            ui.label("/");
                            egui::ComboBox::from_id_salt(("unit", row))
                                .width(44.0)
                                .selected_text(unit.to_string())
                                .show_ui(ui, |ui| {
                                    for u in BEAT_UNITS {
                                        changed |=
                                            ui.selectable_value(unit, u, u.to_string()).changed();
                                    }
                                });
                        });
                        ui.label(
                            RichText::new(secs.map_or("-".into(), af27_core::time::format_time))
                                .monospace()
                                .color(c(th.text_dim)),
                        );
                        if idx.is_some() {
                            if ui.small_button("Delete").clicked() {
                                delete = idx;
                            }
                        } else {
                            ui.label("");
                        }
                        ui.end_row();
                    }
                });
            if let Some(i) = delete {
                st.session.tempo.changes.remove(i);
                st.session.tempo.normalize();
                st.commit_session("Delete tempo change", before.clone());
                st.live_metronome();
            } else {
                tempo_commit(st, ui, &before, changed);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let bar = playhead_bar(st).max(1);
                if ui
                    .button(format!("Add at Playhead (bar {})", bar + 1))
                    .on_hover_text("A change at the bar the playhead is in (bar 2 or later), starting with the values there")
                    .clicked()
                {
                    let before = st.session.clone();
                    st.session.tempo.add_change(bar);
                    st.commit_session("Add tempo change", before);
                    st.live_metronome();
                }
                let mut m = st.session.tempo.metronome;
                if ui.checkbox(&mut m, "Metronome").changed() {
                    st.session.tempo.metronome = m;
                    st.live_metronome();
                }
            });
        });
    st.tempo_ui.open = open && st.tempo_ui.open;
}

/// Asks whether an imported file's tempo map replaces the session's.
pub fn replace_prompt(ctx: &egui::Context, st: &mut State) {
    if st.tempo_ui.pending.is_none() {
        return;
    }
    let mut answer = None;
    crate::ui::popup(
        ctx,
        egui::Id::new(("af2-popup", "tempo.rs", 1)),
        "Import MIDI File",
    )
    .collapsible(false)
    .resizable(false)
    .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
    .show(ctx, |ui| {
        ui.label("The session already has tempo or time signature changes.");
        ui.label("Replace its tempo map with the one in the MIDI file?");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Replace Tempo Map").clicked() {
                answer = Some(true);
            }
            if ui.button("Keep Session Tempo").clicked() {
                answer = Some(false);
            }
            if ui.button("Cancel").clicked() {
                answer = Some(false);
                st.tempo_ui.pending = None;
            }
        });
    });
    if let Some(replace) = answer {
        if let Some((path, imp)) = st.tempo_ui.pending.take() {
            finish_import(st, &path, &imp, replace);
        }
    }
}

/// File > Import > MIDI File...
pub fn import_midi(st: &mut State, path: PathBuf) {
    let imp = match std::fs::read(&path)
        .map_err(|e| format!("Could not open {}: {e}", path.display()))
        .and_then(|b| af27_interop::smf::read(&b))
    {
        Ok(i) => i,
        Err(e) => {
            st.error(e);
            return;
        }
    };
    if imp.tracks.is_empty() {
        st.error("The MIDI file has no notes.");
        return;
    }
    let differs = imp.tempo.segments().ne(st.session.tempo.segments());
    if imp.has_tempo && st.session.tempo.has_changes() && differs {
        st.tempo_ui.pending = Some((path, imp));
        return;
    }
    // A session without changes takes the file's map (its first tempo too).
    finish_import(st, &path, &imp, imp.has_tempo);
}

fn finish_import(
    st: &mut State,
    path: &std::path::Path,
    imp: &af27_interop::smf::SmfImport,
    replace: bool,
) {
    let before = st.session.clone();
    let ids = af27_interop::smf::add_to_session(&mut st.session, imp, replace);
    let n = st.session.tracks.len();
    for (k, id) in ids.iter().enumerate() {
        let colors = &st.theme.track_colors;
        let col = colors[(n - ids.len() + k) % colors.len().max(1)];
        if let Some(t) = st.session.track_mut(*id) {
            t.color = col;
        }
    }
    st.commit_session("Import MIDI File", before);
    st.live_metronome();
    st.mode = crate::state::Mode::Multitrack;
    st.selected_track = ids.first().copied();
    st.toast(format!(
        "Imported {} track(s) from {}",
        ids.len(),
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    if replace && imp.moved > 0 {
        st.toast(format!(
            "{} tempo or time signature event(s) were inside a bar and moved to the nearest bar line",
            imp.moved
        ));
    }
}

/// Multitrack > Export > MIDI File...
pub fn export_midi(st: &mut State, path: PathBuf) {
    if !st
        .session
        .tracks
        .iter()
        .any(|t| t.kind == af27_core::TrackKind::Midi)
    {
        st.error("The session has no MIDI tracks to export.");
        return;
    }
    match af27_interop::smf::write(&st.session)
        .and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string()))
    {
        Ok(()) => st.toast(format!("Exported {}", path.display())),
        Err(e) => st.error(format!("Could not export the MIDI file: {e}")),
    }
}

/// Bar lines (and beat lines when there is room) across `area`, for the
/// Multitrack lanes; or, given a ruler, bar numbers and change flags on it.
pub fn draw_grid(
    p: &egui::Painter,
    ruler: Option<Rect>,
    area: Rect,
    tempo: &Tempo,
    x_of: &dyn Fn(f64) -> f32,
    secs_at: &dyn Fn(f32) -> f64,
    th: &crate::theme::Theme,
) {
    let b0 = tempo.beats(secs_at(area.left()).max(0.0));
    let b1 = tempo.beats(secs_at(area.right()).max(0.0));
    let first = tempo.bar_at(b0).0;
    let mut last_label = f32::MIN;
    // Never more than a few hundred lines.
    for bar in first..first + 600 {
        let start = tempo.bar_start(bar);
        if start > b1 {
            break;
        }
        let seg = tempo.seg_at_beat(start);
        let x = x_of(tempo.secs(start));
        let bar_px = x_of(tempo.secs(start + seg.bar_beats())) - x;
        match ruler {
            None => {
                if bar_px >= 3.0 || bar & 3 == 0 {
                    p.line_segment(
                        [pos2(x, area.top()), pos2(x, area.bottom())],
                        Stroke::new(1.0_f32, ca(th.grid, 200)),
                    );
                }
                // Beats inside the bar when they are far enough apart.
                let beats = seg.beats_per_bar.max(1);
                if bar_px / beats as f32 > 14.0 {
                    for k in 1..beats {
                        let bx = x_of(tempo.secs(start + k as f64 * seg.unit_beats()));
                        p.line_segment(
                            [pos2(bx, area.top()), pos2(bx, area.bottom())],
                            Stroke::new(1.0_f32, ca(th.grid, 90)),
                        );
                    }
                }
            }
            Some(ruler) => {
                if x - last_label > 26.0 && x >= ruler.left() - 1.0 {
                    p.text(
                        pos2(x + 2.0, ruler.bottom() - 1.0),
                        Align2::LEFT_BOTTOM,
                        format!("{}", bar + 1),
                        FontId::proportional(8.5),
                        c(th.accent),
                    );
                    last_label = x;
                }
                if seg.bar == bar && seg.change.is_some() {
                    let tri = vec![
                        pos2(x, ruler.bottom() - 9.0),
                        pos2(x + 6.0, ruler.bottom() - 12.0),
                        pos2(x, ruler.bottom() - 15.0),
                    ];
                    p.add(egui::Shape::convex_polygon(tri, c(th.accent), Stroke::NONE));
                }
            }
        }
    }
}
