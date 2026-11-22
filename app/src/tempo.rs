// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The tempo map in the interface: Multitrack > Tempo and Time
//! Signature... (the start and every change, immediate or gradual; add,
//! edit, delete), the tempo markers on the Multitrack ruler (drag to
//! another bar, double-click to edit, right-click for more), the bar grid
//! the Multitrack editor draws, and `.mid` import and export.

use std::path::PathBuf;

use egui::{pos2, Align2, FontId, Rect, RichText, Sense, Stroke};

use crate::state::State;
use crate::theme::{c, ca};
use af2_core::midi::{Tempo, BEAT_UNITS, BPM_MAX, BPM_MIN};

/// Tempo window state.
#[derive(Default)]
pub struct TempoUi {
    pub open: bool,
    /// An imported `.mid` waiting for an answer: replace the session's
    /// tempo map with the file's?
    pub pending: Option<(PathBuf, af2_interop::smf::SmfImport)>,
    /// Add Gradual Change's start and end bars as shown (from 1); 0 until
    /// first set from the playhead.
    pub ramp_from: u32,
    pub ramp_to: u32,
    /// The change being edited in the small Tempo Change editor.
    pub edit: Option<usize>,
    /// The ruler marker under the pointer (a change index).
    pub hot: Option<usize>,
    /// A ruler marker being dragged: the change, whether it is a glide's
    /// start handle, and the session before the drag.
    drag: Option<(usize, bool, Box<af2_core::Session>)>,
}

/// One undo step per gesture, and the mix rebuilt to the new map.
fn tempo_commit(st: &mut State, ui: &egui::Ui, before: &af2_core::Session, changed: bool) {
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

/// The tempo at the playhead (between the two ends in a glide), and
/// whether the playhead is in a glide.
pub fn playhead_bpm(st: &State) -> (f64, bool) {
    let t = &st.session.tempo;
    let beats = t.beats(st.position().max(0.0));
    let seg = t.seg_at_beat(beats);
    (seg.bpm_at(beats), seg.is_ramp())
}

/// Hover text for the BPM fields while the playhead is in a glide.
pub fn glide_text(st: &State) -> String {
    let t = &st.session.tempo;
    let seg = t.seg_at_secs(st.position().max(0.0));
    let word = if seg.end_bpm < seg.bpm {
        "Ritardando"
    } else {
        "Accelerando"
    };
    let to = seg
        .change
        .and_then(|i| t.changes.get(i))
        .map_or(0, |c| c.bar);
    format!(
        "{word}: {} to {} BPM, bar {} to bar {}. Edit it in Tempo and Time Signature, or double-click its marker on the ruler.",
        fmt_bpm(seg.bpm),
        fmt_bpm(seg.end_bpm),
        seg.bar + 1,
        to + 1
    )
}

fn fmt_bpm(b: f64) -> String {
    let r = (b * 100.0).round() / 100.0;
    if r.fract() == 0.0 {
        format!("{r:.0}")
    } else {
        format!("{r}")
    }
}

/// Adds a change at the bar nearest `beats` (bar 2 or later): immediate,
/// or a glide starting there and arriving four bars later. Opens the
/// Tempo Change editor on it.
pub fn add_here(st: &mut State, beats: f64, gradual: bool) {
    let before = st.session.clone();
    let t = &mut st.session.tempo;
    let bar = t.nearest_bar(beats.max(0.0));
    let i = if gradual {
        t.add_ramp(bar, bar + 4, None)
    } else {
        t.add_change(bar.max(1))
    };
    st.commit_session(
        if gradual {
            "Add gradual tempo change"
        } else {
            "Add tempo change"
        },
        before,
    );
    st.live_metronome();
    st.tempo_ui.edit = Some(i);
}

/// The kind picker for change `i`: Immediate, or Gradual from a bar.
/// Returns the new kind's glide start (Some(None) for immediate) when it
/// changes.
fn kind_picker(ui: &mut egui::Ui, t: &Tempo, i: usize, salt: &str) -> Option<Option<u32>> {
    let c = &t.changes[i];
    let mut out = None;
    let mut gradual = !c.kind.is_immediate();
    egui::ComboBox::from_id_salt((salt, "kind", i))
        .width(84.0)
        .selected_text(if gradual { "Gradual" } else { "Immediate" })
        .show_ui(ui, |ui| {
            if ui
                .selectable_value(&mut gradual, false, "Immediate")
                .on_hover_text("The new tempo starts at its bar")
                .changed()
            {
                out = Some(None);
            }
            if ui
                .selectable_value(&mut gradual, true, "Gradual")
                .on_hover_text(
                    "Ritardando or accelerando: the tempo glides from the one before to this one, arriving at its bar",
                )
                .changed()
            {
                let prev = i.checked_sub(1).map_or(0, |p| t.changes[p].bar);
                out = Some(Some(c.bar.saturating_sub(2).max(prev)));
            }
        });
    if let Some(from) = c.kind.ramp_from() {
        let mut shown = from + 1;
        let prev = i.checked_sub(1).map_or(0, |p| t.changes[p].bar);
        if ui
            .add(
                egui::DragValue::new(&mut shown)
                    .range(prev + 1..=c.bar.max(1))
                    .speed(0.1)
                    .prefix("from "),
            )
            .on_hover_text("The bar the glide starts on")
            .changed()
        {
            out = Some(Some(shown - 1));
        }
    }
    out
}

/// BPM and time signature fields for a change (None: the start).
fn value_fields(ui: &mut egui::Ui, t: &mut Tempo, idx: Option<usize>, salt: &str) -> bool {
    let mut changed = false;
    let (bpm, top, unit) = match idx {
        None => (&mut t.bpm, &mut t.beats_per_bar, &mut t.beat_unit),
        Some(i) => {
            let c = &mut t.changes[i];
            (&mut c.bpm, &mut c.beats_per_bar, &mut c.beat_unit)
        }
    };
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
        egui::ComboBox::from_id_salt((salt, "unit", idx))
            .width(44.0)
            .selected_text(unit.to_string())
            .show_ui(ui, |ui| {
                for u in BEAT_UNITS {
                    changed |= ui.selectable_value(unit, u, u.to_string()).changed();
                }
            });
    });
    changed
}

/// What a row asked for besides value edits.
enum RowAct {
    Move(usize, u32),
    Kind(usize, Option<u32>),
    Delete(usize),
}

fn apply_act(st: &mut State, act: RowAct) -> Option<&'static str> {
    let t = &mut st.session.tempo;
    match act {
        RowAct::Move(i, bar) => {
            t.move_change(i, bar);
            None
        }
        RowAct::Kind(i, from) => {
            t.set_ramp(i, from);
            Some(if from.is_some() {
                "Gradual tempo change"
            } else {
                "Immediate tempo change"
            })
        }
        RowAct::Delete(i) => {
            t.remove_change(i);
            Some("Delete tempo change")
        }
    }
}

pub fn window(ctx: &egui::Context, st: &mut State) {
    let mut open = st.tempo_ui.open;
    crate::ui::popup(ctx, egui::Id::new(("af2-popup", "tempo.rs", 0)), "Tempo and Time Signature")
        .open(&mut open)
        .resizable(false)
        .default_width(460.0)
        .show(ctx, |ui| {
            let th = st.theme.clone();
            ui.label(
                RichText::new(
                    "Changes start on a bar line and hold until the next one. A gradual change (ritardando or accelerando) glides from the tempo before it, starting at its \"from\" bar and arriving at its own. Time signatures always change at the bar. Beats are quarter notes; a bar of 6/8 is three of them.",
                )
                .color(c(th.text_dim)),
            );
            ui.add_space(4.0);
            let before = st.session.clone();
            let mut changed = false;
            let mut act = None;
            let here = st
                .session
                .tempo
                .segments()
                .filter(|s| s.secs <= st.position() + 1e-9)
                .last()
                .and_then(|s| s.change);
            egui::Grid::new("tempo-map")
                .num_columns(6)
                .spacing([10.0, 6.0])
                .striped(true)
                .show(ui, |ui| {
                    ui.label(RichText::new("Bar").strong());
                    ui.label(RichText::new("Kind").strong());
                    ui.label(RichText::new("Tempo").strong());
                    ui.label(RichText::new("Time signature").strong());
                    ui.label(RichText::new("Starts at").strong());
                    ui.label("");
                    ui.end_row();
                    let n = st.session.tempo.changes.len();
                    for row in 0..=n {
                        let idx = row.checked_sub(1);
                        let tempo = &st.session.tempo;
                        let secs = match idx {
                            Some(i) => tempo.ramp_segment(i).or(tempo.change_segment(idx)),
                            None => tempo.change_segment(None),
                        }
                        .map(|s| s.secs);
                        let mark = if idx == here { "▶ " } else { "" };
                        match idx {
                            None => {
                                ui.label(format!("{mark}1"));
                                ui.label(RichText::new("Start").color(c(th.text_dim)));
                            }
                            Some(i) => {
                                ui.horizontal(|ui| {
                                    ui.label(mark);
                                    let mut shown = tempo.changes[i].bar + 1;
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut shown)
                                                .range(2..=9999)
                                                .speed(0.1),
                                        )
                                        .on_hover_text("The bar it arrives on")
                                        .changed()
                                    {
                                        act = Some(RowAct::Move(i, shown - 1));
                                    }
                                });
                                ui.horizontal(|ui| {
                                    if let Some(k) = kind_picker(ui, tempo, i, "win") {
                                        act = Some(RowAct::Kind(i, k));
                                    }
                                });
                            }
                        }
                        changed |= value_fields(ui, &mut st.session.tempo, idx, "win");
                        ui.label(
                            RichText::new(secs.map_or("-".into(), af2_core::time::format_time))
                                .monospace()
                                .color(c(th.text_dim)),
                        );
                        match idx {
                            Some(i) => {
                                if ui.small_button("Delete").clicked() {
                                    act = Some(RowAct::Delete(i));
                                }
                            }
                            None => {
                                ui.label("");
                            }
                        }
                        ui.end_row();
                    }
                });
            match act {
                Some(RowAct::Move(i, bar)) => {
                    apply_act(st, RowAct::Move(i, bar));
                    tempo_commit(st, ui, &before, true);
                }
                Some(a) => {
                    let label = apply_act(st, a).unwrap_or("Tempo map");
                    st.commit_session(label, before.clone());
                    st.live_metronome();
                }
                None => tempo_commit(st, ui, &before, changed),
            }
            ui.add_space(6.0);
            let bar = playhead_bar(st).max(1);
            ui.horizontal(|ui| {
                if ui
                    .button(format!("Add Tempo Change (bar {})", bar + 1))
                    .on_hover_text("An immediate change at the bar the playhead is in (bar 2 or later), starting with the values there")
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
            ui.horizontal(|ui| {
                let tu = &mut st.tempo_ui;
                if tu.ramp_from == 0 {
                    tu.ramp_from = bar + 1;
                    tu.ramp_to = bar + 5;
                }
                ui.add(
                    egui::DragValue::new(&mut tu.ramp_from)
                        .range(1..=9998)
                        .speed(0.1)
                        .prefix("from bar "),
                );
                tu.ramp_to = tu.ramp_to.max(tu.ramp_from + 1);
                ui.add(
                    egui::DragValue::new(&mut tu.ramp_to)
                        .range(tu.ramp_from + 1..=9999)
                        .speed(0.1)
                        .prefix("to bar "),
                );
                let (from, to) = (tu.ramp_from - 1, tu.ramp_to - 1);
                if ui
                    .button("Add Gradual Change")
                    .on_hover_text("A ritardando or accelerando: the tempo glides from the one before, starting at the first bar and arriving at the second, where you set the new tempo")
                    .clicked()
                {
                    let before = st.session.clone();
                    st.session.tempo.add_ramp(from, to, None);
                    st.commit_session("Add gradual tempo change", before);
                    st.live_metronome();
                }
            });
        });
    st.tempo_ui.open = open && st.tempo_ui.open;
}

/// The small editor a double-click on a ruler marker opens.
pub fn edit_popup(ctx: &egui::Context, st: &mut State) {
    let Some(i) = st.tempo_ui.edit else {
        return;
    };
    if i >= st.session.tempo.changes.len() {
        st.tempo_ui.edit = None;
        return;
    }
    let mut open = true;
    crate::ui::popup(
        ctx,
        egui::Id::new(("af2-popup", "tempo.rs", 2)),
        "Tempo Change",
    )
    .open(&mut open)
    .collapsible(false)
    .resizable(false)
    .show(ctx, |ui| {
        let before = st.session.clone();
        let mut changed = false;
        let mut act = None;
        egui::Grid::new("tempo-change-edit")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                let t = &st.session.tempo;
                ui.label("Bar");
                let mut shown = t.changes[i].bar + 1;
                if ui
                    .add(egui::DragValue::new(&mut shown).range(2..=9999).speed(0.1))
                    .changed()
                {
                    act = Some(RowAct::Move(i, shown - 1));
                }
                ui.end_row();
                ui.label("Kind");
                ui.horizontal(|ui| {
                    if let Some(k) = kind_picker(ui, t, i, "pop") {
                        act = Some(RowAct::Kind(i, k));
                    }
                });
                ui.end_row();
                ui.label("Tempo and time signature");
                ui.horizontal(|ui| {
                    changed |= value_fields(ui, &mut st.session.tempo, Some(i), "pop");
                });
                ui.end_row();
            });
        ui.add_space(4.0);
        if ui.button("Delete").clicked() {
            act = Some(RowAct::Delete(i));
        }
        match act {
            Some(RowAct::Move(_, bar)) => {
                apply_act(st, RowAct::Move(i, bar));
                // It may have moved in the list.
                let ni = st
                    .session
                    .tempo
                    .changes
                    .iter()
                    .position(|c| c.bar == bar.max(1));
                st.tempo_ui.edit = ni;
                tempo_commit(st, ui, &before, true);
            }
            Some(a) => {
                let deleting = matches!(a, RowAct::Delete(_));
                let label = apply_act(st, a).unwrap_or("Tempo map");
                st.commit_session(label, before.clone());
                st.live_metronome();
                if deleting {
                    st.tempo_ui.edit = None;
                }
            }
            None => tempo_commit(st, ui, &before, changed),
        }
    });
    if !open {
        st.tempo_ui.edit = None;
    }
}

/// A change's marker on the ruler: where its bar is, and its glide's
/// start and direction (true: speeding up).
struct Marker {
    change: usize,
    x: f32,
    glide: Option<(f32, bool)>,
}

fn markers(tempo: &Tempo, x_of: &dyn Fn(f64) -> f32) -> Vec<Marker> {
    let mut out: Vec<Marker> = Vec::new();
    let mut glide = None;
    for s in tempo.segments() {
        let Some(i) = s.change else { continue };
        if s.is_ramp() {
            glide = Some((x_of(s.secs), s.end_bpm > s.bpm));
        } else {
            out.push(Marker {
                change: i,
                x: x_of(s.secs),
                glide: glide.take(),
            });
        }
    }
    out
}

/// The band of the ruler the markers sit in.
fn band(ruler: Rect) -> (f32, f32) {
    (ruler.bottom() - 18.0, ruler.bottom() - 10.0)
}

/// Tempo markers on the ruler: a flag at an immediate change's bar; a
/// wedge across a gradual change's bars (widening when it speeds up,
/// narrowing when it slows down) ending in a flag.
pub fn draw_markers(
    p: &egui::Painter,
    ruler: Rect,
    tempo: &Tempo,
    x_of: &dyn Fn(f64) -> f32,
    th: &crate::theme::Theme,
    hot: Option<usize>,
) {
    let (top, base) = band(ruler);
    for m in markers(tempo, x_of) {
        if m.x < ruler.left() - 400.0 || m.glide.map_or(m.x, |g| g.0) > ruler.right() + 10.0 {
            continue;
        }
        let col = if hot == Some(m.change) {
            c(th.accent_alt)
        } else {
            c(th.accent)
        };
        if let Some((x0, up)) = m.glide {
            let (h0, h1) = if up {
                (1.5, base - top)
            } else {
                (base - top, 1.5)
            };
            let x0 = x0.max(ruler.left() - 2.0);
            if m.x - x0 > 1.0 {
                let wedge = vec![
                    pos2(x0, base),
                    pos2(m.x, base),
                    pos2(m.x, base - h1),
                    pos2(x0, base - h0),
                ];
                p.add(egui::Shape::convex_polygon(
                    wedge,
                    col.gamma_multiply(0.45),
                    Stroke::new(1.0_f32, col),
                ));
            }
            // The glide's start handle.
            p.line_segment(
                [pos2(x0, top - 1.0), pos2(x0, ruler.bottom())],
                Stroke::new(1.0_f32, col.gamma_multiply(0.7)),
            );
        }
        p.line_segment(
            [pos2(m.x, top), pos2(m.x, ruler.bottom())],
            Stroke::new(1.5_f32, col),
        );
        let flag = vec![
            pos2(m.x, top),
            pos2(m.x + 7.0, top + 3.0),
            pos2(m.x, top + 6.0),
        ];
        p.add(egui::Shape::convex_polygon(flag, col, Stroke::NONE));
    }
}

/// Hover, drag, double-click and right-click on the ruler's tempo
/// markers. Drawn on top of the ruler's own widget, so they take the
/// pointer first. `secs_at` maps a ruler x to seconds.
pub fn markers_input(
    ui: &egui::Ui,
    st: &mut State,
    ruler: Rect,
    x_of: &dyn Fn(f64) -> f32,
    secs_at: &dyn Fn(f32) -> f64,
) {
    let (top, _) = band(ruler);
    let ms = markers(&st.session.tempo, x_of);
    st.tempo_ui.hot = None;
    // Bars are found on the map as it was when a drag began, so the bars
    // under the pointer do not shift while the change moves.
    let bar_at_x = |t: &Tempo, x: f32| t.nearest_bar(t.beats(secs_at(x).max(0.0)));
    for m in &ms {
        let i = m.change;
        let handles = std::iter::once((m.x, false)).chain(m.glide.map(|g| (g.0, true)));
        for (x, start) in handles {
            if x < ruler.left() - 6.0 || x > ruler.right() + 6.0 {
                continue;
            }
            let r = Rect::from_min_max(pos2(x - 4.0, top - 2.0), pos2(x + 8.0, ruler.bottom()));
            let dragging = st
                .tempo_ui
                .drag
                .as_ref()
                .is_some_and(|d| d.0 == i && d.1 == start);
            // The one being dragged keeps one id while its index changes.
            let id = if dragging {
                ui.id().with("af2-tempo-marker-drag")
            } else {
                ui.id().with(("af2-tempo-marker", i, start))
            };
            let resp = ui
                .interact(r, id, Sense::click_and_drag())
                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            if resp.hovered() || dragging {
                st.tempo_ui.hot = Some(i);
            }
            if resp.hovered() && !dragging {
                let t = &st.session.tempo;
                if let Some(c) = t.changes.get(i) {
                    let what = match c.kind.ramp_from() {
                        Some(_) if start => format!(
                            "Start of the glide to bar {} (drag to change where it starts)",
                            c.bar + 1
                        ),
                        Some(f) => format!(
                            "Bar {}: gliding from bar {} to {} BPM, {}/{}",
                            c.bar + 1,
                            f + 1,
                            fmt_bpm(c.bpm),
                            c.beats_per_bar,
                            c.beat_unit
                        ),
                        None => format!(
                            "Bar {}: {} BPM, {}/{}",
                            c.bar + 1,
                            fmt_bpm(c.bpm),
                            c.beats_per_bar,
                            c.beat_unit
                        ),
                    };
                    resp.clone().on_hover_text(format!(
                        "{what}\nDrag to another bar; double-click to edit; right-click for more"
                    ));
                }
            }
            if resp.drag_started() {
                st.tempo_ui.drag = Some((i, start, Box::new(st.session.clone())));
            }
            if resp.dragged() {
                if let (Some(pp), Some((di, ds, before))) =
                    (resp.interact_pointer_pos(), st.tempo_ui.drag.as_mut())
                {
                    if *di == i && *ds == start {
                        let bar = bar_at_x(&before.tempo, pp.x);
                        let t = &mut st.session.tempo;
                        let moved = if start {
                            let cur = t.changes[*di].kind.ramp_from();
                            if cur != Some(bar) {
                                t.set_ramp(*di, Some(bar));
                                t.changes[*di].kind.ramp_from() != cur
                            } else {
                                false
                            }
                        } else if t.changes[*di].bar != bar.max(1) {
                            *di = t.move_change(*di, bar);
                            true
                        } else {
                            false
                        };
                        if moved {
                            st.session_version += 1;
                            st.live_metronome();
                        }
                    }
                }
            }
            if resp.drag_stopped() {
                if let Some((_, _, before)) = st.tempo_ui.drag.take() {
                    if before.tempo != st.session.tempo {
                        st.commit_session(
                            if start {
                                "Move glide start"
                            } else {
                                "Move tempo change"
                            },
                            *before,
                        );
                        st.live_metronome();
                    }
                }
            }
            if resp.double_clicked() {
                st.tempo_ui.edit = Some(i);
            }
            resp.context_menu(|ui| {
                if ui.button("Edit...").clicked() {
                    st.tempo_ui.edit = Some(i);
                    ui.close_menu();
                }
                let gradual = st
                    .session
                    .tempo
                    .changes
                    .get(i)
                    .is_some_and(|c| !c.kind.is_immediate());
                let flip = if gradual {
                    "Make Immediate"
                } else {
                    "Make Gradual"
                };
                if ui.button(flip).clicked() {
                    let before = st.session.clone();
                    let t = &st.session.tempo;
                    let k = if gradual {
                        None
                    } else {
                        let prev = i.checked_sub(1).map_or(0, |p| t.changes[p].bar);
                        Some(t.changes[i].bar.saturating_sub(2).max(prev))
                    };
                    let label = apply_act(st, RowAct::Kind(i, k)).unwrap_or("Tempo map");
                    st.commit_session(label, before);
                    st.live_metronome();
                    ui.close_menu();
                }
                if ui.button("Delete").clicked() {
                    let before = st.session.clone();
                    apply_act(st, RowAct::Delete(i));
                    st.commit_session("Delete tempo change", before);
                    st.live_metronome();
                    st.tempo_ui.edit = None;
                    ui.close_menu();
                }
            });
        }
    }
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
        .and_then(|b| af2_interop::smf::read(&b))
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
    imp: &af2_interop::smf::SmfImport,
    replace: bool,
) {
    let before = st.session.clone();
    let ids = af2_interop::smf::add_to_session(&mut st.session, imp, replace);
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
        .any(|t| t.kind == af2_core::TrackKind::Midi)
    {
        st.error("The session has no MIDI tracks to export.");
        return;
    }
    match af2_interop::smf::write(&st.session)
        .and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string()))
    {
        Ok(()) => st.toast(format!("Exported {}", path.display())),
        Err(e) => st.error(format!("Could not export the MIDI file: {e}")),
    }
}

/// Bar lines (and beat lines when there is room) across `area`, for the
/// Multitrack lanes; or, given a ruler, bar numbers on it (the tempo
/// markers are [`draw_markers`]).
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
            }
        }
    }
}
