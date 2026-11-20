// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The time ruler both editors share: its marks in the chosen time format,
//! click-or-drag positioning of the playhead, its right-click menu, the
//! start marker, middle-button panning and the snapping of selection edges.
//!
//! - Stopped (or paused), a click or drag on the ruler moves the playhead
//!   with the mouse. Playing, playback moves to the spot on release (a
//!   ghost line shows where while the button is down).
//! - The middle button drags the view anywhere over the time area; it
//!   never selects.

use egui::{pos2, Align2, FontId, PointerButton, Rect, Response, Rounding, Shape, Stroke};

use crate::state::{State, View};
use crate::theme::{c, ca, Theme};
use crate::transport::TimeCtx;
use crate::waveform::{time_to_x, x_to_time};

/// Pixels from a selection edge that grab it, and that snapping reaches.
pub const EDGE_PX: f32 = 6.0;

pub fn draw_ruler(
    p: &egui::Painter,
    rect: Rect,
    v: &View,
    th: &Theme,
    markers: &[af27_core::Marker],
    tc: &TimeCtx,
) {
    p.rect_filled(rect, Rounding::ZERO, c(th.panel));
    let step = tc.ruler_step(v.len, rect.width());
    let first = (v.start / step).floor() as i64;
    let last = ((v.start + v.len) / step).ceil() as i64;
    for i in first..=last {
        let t = i as f64 * step;
        let x = time_to_x(v, rect, t);
        if x < rect.left() - 1.0 || x > rect.right() {
            continue;
        }
        p.line_segment(
            [pos2(x, rect.bottom() - 7.0), pos2(x, rect.bottom())],
            Stroke::new(1.0_f32, c(th.text_dim)),
        );
        let xm = time_to_x(v, rect, t + step / 2.0);
        p.line_segment(
            [pos2(xm, rect.bottom() - 3.0), pos2(xm, rect.bottom())],
            Stroke::new(1.0_f32, c(th.border)),
        );
        p.text(
            pos2(x + 3.0, rect.top() + 2.0),
            Align2::LEFT_TOP,
            tc.ruler_label(t.max(0.0), step),
            FontId::proportional(10.0),
            c(th.text_dim),
        );
    }
    for m in markers {
        let x = time_to_x(v, rect, m.time);
        if x >= rect.left() && x <= rect.right() {
            let tri = vec![
                pos2(x, rect.bottom()),
                pos2(x - 5.0, rect.bottom() - 8.0),
                pos2(x + 5.0, rect.bottom() - 8.0),
            ];
            p.add(Shape::convex_polygon(tri, c(th.marker), Stroke::NONE));
        }
    }
}

/// `t` snapped to a ruler mark (with Snap to Ruler Marks on) or to one of
/// `extra` (markers, clip edges...) within [`EDGE_PX`].
pub fn snap(st: &State, t: f64, v: &View, rect: Rect, extra: &[f64]) -> f64 {
    let tol = EDGE_PX as f64 * v.len / rect.width().max(1.0) as f64;
    let mut best = (tol, t);
    for &e in extra {
        if (e - t).abs() < best.0 {
            best = ((e - t).abs(), e);
        }
    }
    if best.1 == t && st.prefs.snap_ruler {
        let step = st.time_ctx().ruler_step(v.len, rect.width());
        return crate::transport::snap_to_marks(t, step, tol);
    }
    best.1
}

/// The selection edge under `x`, if any: (that edge, the other edge).
pub fn edge_at(sel: Option<(f64, f64)>, v: &View, rect: Rect, x: f32) -> Option<(f64, f64)> {
    let (a, b) = sel.filter(|(a, b)| b - a > 1e-9)?;
    let da = (time_to_x(v, rect, a) - x).abs();
    let db = (time_to_x(v, rect, b) - x).abs();
    if da.min(db) > EDGE_PX {
        None
    } else if da <= db {
        Some((a, b))
    } else {
        Some((b, a))
    }
}

/// A middle-button drag on `r` this frame: how far it moved (and the
/// grabbing hand shows).
pub fn pan_delta(ui: &egui::Ui, r: &Response) -> Option<egui::Vec2> {
    if r.dragged_by(PointerButton::Middle) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        Some(r.drag_delta())
    } else {
        None
    }
}

/// Clicks, drags and the right-click menu on the ruler `r`, whose times
/// run across `x_rect` with view `v`, up to `max_t`. Returns where playback
/// will move on release while a drag is under way during playback.
#[allow(clippy::too_many_arguments)]
pub fn input(
    ui: &egui::Ui,
    st: &mut State,
    r: &Response,
    x_rect: Rect,
    v: &View,
    max_t: f64,
    markers: &[f64],
    tempo_items: bool,
) -> Option<f64> {
    let id = r.id.with("scrub");
    let pressed = r.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
    let mut ghost = None;
    if let Some(pp) = r.interact_pointer_pos() {
        let t = snap(st, x_to_time(v, x_rect, pp.x), v, x_rect, markers).clamp(0.0, max_t);
        let release =
            r.clicked() || (r.drag_stopped() && ui.data(|d| d.get_temp::<f64>(id)).is_some());
        if st.transport_playing() {
            if release {
                st.jump_to(t);
                ui.data_mut(|d| d.remove::<f64>(id));
            } else if pressed {
                ui.data_mut(|d| d.insert_temp(id, t));
                ghost = Some(t);
            }
        } else if pressed || release {
            st.set_cursor(t);
            ui.data_mut(|d| d.remove::<f64>(id));
        }
    }
    if !pressed && !r.drag_stopped() && !r.clicked() {
        ui.data_mut(|d| d.remove::<f64>(id));
    }
    // Right-click: Time Display, Snap to Ruler Marks, Add Marker (there).
    let at_id = r.id.with("menu-at");
    if r.secondary_clicked() {
        if let Some(pp) = r.interact_pointer_pos() {
            let t = x_to_time(v, x_rect, pp.x).clamp(0.0, max_t);
            ui.data_mut(|d| d.insert_temp(at_id, t));
        }
    }
    r.context_menu(|ui| {
        ui.menu_button("Time Display", |ui| crate::ui::time_display_items(ui, st));
        let mut snap_on = st.prefs.snap_ruler;
        if ui.checkbox(&mut snap_on, "Snap to Ruler Marks").changed() {
            st.prefs.snap_ruler = snap_on;
            st.prefs.save();
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Add Marker").clicked() {
            let t = ui
                .data(|d| d.get_temp::<f64>(at_id))
                .unwrap_or_else(|| st.position());
            st.add_marker_at(t);
            ui.close_menu();
        }
        if tempo_items {
            let t = ui
                .data(|d| d.get_temp::<f64>(at_id))
                .unwrap_or_else(|| st.position());
            ui.separator();
            if ui
                .button("Add Tempo Change Here")
                .on_hover_text("An immediate tempo or time signature change at the nearest bar line")
                .clicked()
            {
                let beats = st.session.tempo.beats(t);
                crate::tempo::add_here(st, beats, false);
                ui.close_menu();
            }
            if ui
                .button("Add Gradual Tempo Change Here")
                .on_hover_text("A ritardando or accelerando starting at the nearest bar line and arriving four bars later")
                .clicked()
            {
                let beats = st.session.tempo.beats(t);
                crate::tempo::add_here(st, beats, true);
                ui.close_menu();
            }
        }
    });
    ghost
}

/// Where playback began (while playing or paused): an outlined triangle on
/// the ruler and a dashed line down the lanes.
pub fn start_marker(p: &egui::Painter, x: f32, ruler: Rect, bottom: f32, th: &Theme) {
    let x = pixel_x(p, x);
    let col = ca(th.playhead, 200);
    let tri = vec![
        pos2(x, ruler.bottom() - 1.0),
        pos2(x - 5.0, ruler.bottom() - 10.0),
        pos2(x + 5.0, ruler.bottom() - 10.0),
    ];
    p.add(Shape::closed_line(tri, Stroke::new(1.5_f32, col)));
    crate::waveform::dashed_v(p, x, ruler.bottom(), bottom, ca(th.playhead, 140));
}

/// Where playback will move on release of a ruler drag.
pub fn ghost(p: &egui::Painter, x: f32, top: f32, bottom: f32, th: &Theme) {
    let x = pixel_x(p, x);
    p.line_segment(
        [pos2(x, top), pos2(x, bottom)],
        Stroke::new(1.0_f32, ca(th.playhead, 110)),
    );
}

/// `x` moved to the middle of its device pixel, so a 1 px line and the
/// shape on top of it land on exactly the same pixel column.
pub fn pixel_x(p: &egui::Painter, x: f32) -> f32 {
    let ppp = p.ctx().pixels_per_point();
    ((x * ppp).floor() + 0.5) / ppp
}
