// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The Multitrack's overview strip, above the time ruler: the whole
//! session in miniature (one thin row per track) with the part in view
//! as a window. Drag the window to scroll, drag one of its ends to zoom
//! with the other end held, click beside it to bring that time into the
//! middle (and keep dragging), double-click for Zoom to Session. The
//! wheel over it zooms at the time under the pointer (see
//! [`crate::zoom::wheel_action`]). The button at its right end is Zoom
//! Out Full (every track fitted, the whole session in view).

use egui::{pos2, vec2, Rect, Rounding, Sense, Stroke};

use crate::state::{State, View};
use crate::theme::{c, ca};

/// The strip's height.
pub const STRIP_H: f32 = 16.0;
/// How close to a window end a press grabs the end.
const EDGE_PX: f32 = 5.0;

/// What a drag on the strip holds.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Grab {
    /// The window, grabbed this many seconds after its start.
    Body(f64),
    /// The window's left end (the right end stays).
    Left,
    /// The window's right end (the left end stays).
    Right,
}

/// The time the strip spans: the session (with a little room) or the
/// view, whichever reaches further.
pub fn span(st: &State) -> f64 {
    let v = st.session_view;
    (st.session.duration() * 1.05).max(v.start + v.len).max(1.0)
}

/// The session time under `x` on the strip `r`.
pub fn time_at(st: &State, r: Rect, x: f32) -> f64 {
    ((x - r.left()) / r.width().max(1.0)).clamp(0.0, 1.0) as f64 * span(st)
}

/// Where the strip's view of `t` sits in screen x.
fn x_of(r: Rect, total: f64, t: f64) -> f32 {
    r.left() + (t / total) as f32 * r.width()
}

/// A drag that moves one end of the window, the other held: the view
/// from `start` to the time `t` under the pointer, within the limits.
pub fn drag_end(v: &View, left: bool, t: f64, lim: &crate::zoom::TimeLimits) -> View {
    let end = v.start + v.len;
    let mut out = if left {
        let start = t.min(end - lim.min_len).max(0.0);
        let len = (end - start).clamp(lim.min_len, lim.max_len);
        View {
            start: end - len,
            len,
        }
    } else {
        let len = (t - v.start).clamp(lim.min_len, lim.max_len);
        View {
            start: v.start,
            len,
        }
    };
    out.limit(lim);
    out
}

/// Clicks, drags and double-clicks on the strip `r`, and the Zoom Out
/// Full button `button`.
pub fn input(ui: &egui::Ui, st: &mut State, r: Rect, button: Rect) {
    let id = ui.id().with("af2-overview");
    let resp = ui.interact(r, id, Sense::click_and_drag());
    let total = span(st);
    let v = st.session_view;
    let (x0, x1) = (x_of(r, total, v.start), x_of(r, total, v.start + v.len));
    let near = |x: f32| -> Option<Grab> {
        if (x - x0).abs() <= EDGE_PX && (x - x0).abs() <= (x - x1).abs() {
            Some(Grab::Left)
        } else if (x - x1).abs() <= EDGE_PX {
            Some(Grab::Right)
        } else if x > x0 && x < x1 {
            Some(Grab::Body(0.0))
        } else {
            None
        }
    };
    if let Some(hp) = resp.hover_pos() {
        let icon = match near(hp.x) {
            Some(Grab::Left | Grab::Right) => egui::CursorIcon::ResizeHorizontal,
            Some(Grab::Body(_)) if resp.dragged() => egui::CursorIcon::Grabbing,
            Some(Grab::Body(_)) => egui::CursorIcon::Grab,
            None => egui::CursorIcon::Default,
        };
        ui.ctx().set_cursor_icon(icon);
    }
    let lim = st.session_limits();
    let grab_id = id.with("grab");
    if resp.double_clicked() {
        crate::ui::zoom_session(st);
        return;
    }
    if resp.drag_started_by(egui::PointerButton::Primary) || resp.clicked() {
        if let Some(pp) = resp.interact_pointer_pos() {
            let t = time_at(st, r, pp.x);
            let grab = match near(pp.x) {
                Some(Grab::Body(_)) => Grab::Body(t - v.start),
                Some(g) => g,
                None => {
                    // Beside the window: centre it there, then drag it.
                    st.session_view.start = t - v.len / 2.0;
                    st.session_view.limit(&lim);
                    st.hand_moved_view();
                    Grab::Body(t - st.session_view.start)
                }
            };
            ui.data_mut(|d| d.insert_temp(grab_id, grab));
        }
    }
    if resp.dragged_by(egui::PointerButton::Primary) {
        let grab: Option<Grab> = ui.data(|d| d.get_temp(grab_id));
        if let (Some(g), Some(pp)) = (grab, resp.interact_pointer_pos()) {
            // While dragging, the strip keeps the span it had at the start
            // so the window does not run away under the pointer.
            let t = ((pp.x - r.left()) / r.width().max(1.0)) as f64 * total;
            let nv = match g {
                Grab::Body(off) => {
                    let mut n = View {
                        start: t - off,
                        len: v.len,
                    };
                    n.limit(&lim);
                    n
                }
                Grab::Left => drag_end(&v, true, t, &lim),
                Grab::Right => drag_end(&v, false, t, &lim),
            };
            if nv.start != v.start || nv.len != v.len {
                st.session_view = nv;
                st.hand_moved_view();
            }
        }
    }
    if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<Grab>(grab_id));
    }
    let b = ui
        .interact(button, id.with("full"), Sense::click())
        .on_hover_text("Zoom Out Full: every track fitted, the whole session in view (Shift+\\)");
    if b.clicked() {
        crate::ui::zoom_out_full(st);
    }
}

/// Draws the strip `r` and the Zoom Out Full button `button`.
pub fn draw(p: &egui::Painter, st: &State, r: Rect, button: Rect, th: &crate::theme::Theme) {
    p.rect_filled(r, Rounding::ZERO, c(th.panel));
    let total = span(st);
    let n = st.session.tracks.len().max(1);
    let inner = r.shrink2(vec2(0.0, 2.0));
    let row_h = (inner.height() / n as f32).max(1.0);
    for (i, tr) in st.session.tracks.iter().enumerate() {
        let y = inner.top() + (i as f32 * inner.height() / n as f32).floor();
        if y >= inner.bottom() {
            break;
        }
        for clip in &tr.clips {
            let a = x_of(r, total, clip.start);
            let b = x_of(r, total, clip.end()).max(a + 1.0);
            p.rect_filled(
                Rect::from_min_max(pos2(a, y), pos2(b, (y + row_h).min(inner.bottom()))),
                Rounding::ZERO,
                ca(clip.props.color.unwrap_or(tr.color), 150),
            );
        }
        for clip in &tr.midi {
            let a = x_of(r, total, st.session.tempo.secs(clip.start));
            let b = x_of(r, total, clip.end_secs(&st.session.tempo)).max(a + 1.0);
            p.rect_filled(
                Rect::from_min_max(pos2(a, y), pos2(b, (y + row_h).min(inner.bottom()))),
                Rounding::ZERO,
                ca(tr.color, 150),
            );
        }
    }
    let v = st.session_view;
    let win = Rect::from_min_max(
        pos2(x_of(r, total, v.start).max(r.left()), r.top()),
        pos2(
            x_of(r, total, v.start + v.len)
                .min(r.right())
                .max(x_of(r, total, v.start) + 3.0),
            r.bottom(),
        ),
    );
    p.rect_filled(win, Rounding::same(2.0), ca(th.accent, 40));
    p.rect_stroke(win, Rounding::same(2.0), Stroke::new(1.0_f32, c(th.accent)));
    let x = x_of(r, total, st.position());
    if x >= r.left() && x <= r.right() {
        p.line_segment(
            [pos2(x, r.top()), pos2(x, r.bottom())],
            Stroke::new(1.0_f32, c(th.playhead)),
        );
    }
    p.line_segment(
        [r.left_bottom(), r.right_bottom()],
        Stroke::new(1.0_f32, c(th.border)),
    );
    // Zoom Out Full: a frame with arrows out to its corners.
    p.rect_filled(button, Rounding::ZERO, c(th.panel));
    let g = button.shrink(3.0);
    let s = Stroke::new(1.0_f32, c(th.text_dim));
    p.rect_stroke(g, Rounding::ZERO, s);
    let m = g.center();
    p.line_segment([m - vec2(2.0, 0.0), m + vec2(2.0, 0.0)], s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zoom::TimeLimits;

    #[test]
    fn dragging_one_end_keeps_the_other() {
        let lim = TimeLimits::session(60.0, 0.001);
        let v = View {
            start: 10.0,
            len: 20.0,
        };
        let l = drag_end(&v, true, 5.0, &lim);
        assert!((l.start - 5.0).abs() < 1e-9 && (l.start + l.len - 30.0).abs() < 1e-9);
        let r = drag_end(&v, false, 50.0, &lim);
        assert!((r.start - 10.0).abs() < 1e-9 && (r.len - 40.0).abs() < 1e-9);
        // Past the other end: stops at the closest zoom.
        let l = drag_end(&v, true, 40.0, &lim);
        assert!((l.start + l.len - 30.0).abs() < 1e-9 && (l.len - 0.001).abs() < 1e-9);
        // Never before zero.
        let l = drag_end(&v, true, -5.0, &lim);
        assert_eq!(l.start, 0.0);
    }
}
