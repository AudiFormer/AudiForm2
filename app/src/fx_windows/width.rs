// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Stereo Width: two sliders across the window, each with a notch dead in
//! the middle where it leaves the sound alone. Center Pan runs left to
//! right with the center at the notch; Width runs from mono (0%) on the
//! left through 100% at the notch to 300% on the right. Near the notch the
//! thumb settles onto it; double-click a slider to go back to the notch.

use egui::{pos2, vec2, Align2, FontId, Rect, Sense, Stroke};

use super::{Env, Fx};
use crate::theme::{c, ca};

/// How near the notch (as a share of the slider) the thumb settles on it.
const DETENT: f32 = 0.025;

/// Where a value sits on its slider (0 left, 1 right), and back.
struct Scale {
    to_pos: fn(f64) -> f32,
    to_val: fn(f32) -> f64,
    notch: f64,
}

/// Center Pan: -100 (left) .. 100 (right), the center in the middle.
const PAN: Scale = Scale {
    to_pos: |v| ((v + 100.0) / 200.0) as f32,
    to_val: |p| f64::from(p) * 200.0 - 100.0,
    notch: 0.0,
};

/// Width: 0% .. 100% across the left half, 100% .. 300% across the right.
const WIDTH: Scale = Scale {
    to_pos: |v| {
        if v <= 100.0 {
            (v / 200.0) as f32
        } else {
            (0.5 + (v - 100.0) / 400.0) as f32
        }
    },
    to_val: |p| {
        if p <= 0.5 {
            f64::from(p) * 200.0
        } else {
            100.0 + (f64::from(p) - 0.5) * 400.0
        }
    },
    notch: 100.0,
};

fn pan_text(v: f64) -> String {
    let n = v.round();
    if n.abs() < 0.5 {
        "C".into()
    } else if n < 0.0 {
        format!("L{}", -n)
    } else {
        format!("R{n}")
    }
}

fn width_text(v: f64) -> String {
    match v.round() {
        w if w <= 0.0 => "Mono".into(),
        w => format!("{w}%"),
    }
}

pub fn panel(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    ui.add_space(4.0);
    slider(ui, fx, env, "pan", "Center Pan", ("L", "R"), &PAN, pan_text,
        "Moves what both sides share left or right. The notch is the center.");
    ui.add_space(10.0);
    slider(ui, fx, env, "width", "Width", ("Mono", "300%"), &WIDTH, width_text,
        "Narrower to the left (down to mono), wider to the right. The notch is 100%: unchanged.");
    ui.add_space(4.0);
}

#[allow(clippy::too_many_arguments)]
fn slider(
    ui: &mut egui::Ui,
    fx: &mut Fx,
    env: &Env,
    key: &str,
    name: &str,
    ends: (&str, &str),
    sc: &Scale,
    text: fn(f64) -> String,
    tip: &str,
) {
    let th = &env.th;
    let v = fx.get(key);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(name).color(c(th.text)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // A value box: drag it, or double-click it to type a value.
            if let Some(ctl) = fx.ctl(key).cloned() {
                let mut bv = v;
                let b = crate::widgets::value_box(ui, fx.uid.with(("width-box", key)), &mut bv, ctl.min, ctl.max, false, 76.0, &|x| text(x));
                if b.changed() {
                    fx.set_live(key, bv);
                }
                fx.track(&b);
            }
        });
    });
    let w = ui.available_width().max(120.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click_and_drag());
    let resp = resp.on_hover_text(tip);
    let track = Rect::from_min_max(pos2(rect.left() + 10.0, rect.center().y - 2.0), pos2(rect.right() - 10.0, rect.center().y + 2.0));
    let x_of = |p: f32| track.left() + p.clamp(0.0, 1.0) * track.width();
    let p = ui.painter_at(rect.expand(2.0));
    // The track, filled from the notch to the thumb.
    p.rect_filled(track, 2.0, ca(th.text_dim, 60));
    let mid = x_of(0.5);
    let at = x_of((sc.to_pos)(v));
    p.rect_filled(
        Rect::from_min_max(pos2(mid.min(at), track.top()), pos2(mid.max(at), track.bottom())),
        2.0,
        ca(th.accent, 200),
    );
    // The notch: a tick above and below, dead in the middle.
    let notch = Stroke::new(2.0_f32, c(th.text));
    p.line_segment([pos2(mid, rect.top() + 2.0), pos2(mid, track.top() - 3.0)], notch);
    p.line_segment([pos2(mid, track.bottom() + 3.0), pos2(mid, rect.bottom() - 2.0)], notch);
    // Quarter ticks, fainter.
    for q in [0.0, 0.25, 0.75, 1.0] {
        let x = x_of(q);
        p.line_segment([pos2(x, track.bottom() + 4.0), pos2(x, rect.bottom() - 6.0)], Stroke::new(1.0_f32, ca(th.text_dim, 140)));
    }
    // The thumb.
    let hot = resp.hovered() || resp.dragged();
    let thumb = Rect::from_center_size(pos2(at, rect.center().y), vec2(10.0, 20.0));
    p.rect_filled(thumb, 2.0, if hot { c(th.accent) } else { c(th.text) });
    p.rect_stroke(thumb, 2.0, Stroke::new(1.0_f32, c(th.background)));
    // The ends' names under the track.
    let font = FontId::proportional(9.5);
    p.text(pos2(track.left(), rect.bottom()), Align2::LEFT_BOTTOM, ends.0, font.clone(), c(th.text_dim));
    p.text(pos2(track.right(), rect.bottom()), Align2::RIGHT_BOTTOM, ends.1, font, c(th.text_dim));

    if resp.double_clicked() {
        fx.set(key, sc.notch);
        return;
    }
    if let Some(pp) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
        let mut pos = (pp.x - track.left()) / track.width().max(1.0);
        // Fine control with Ctrl or Shift held: a tenth of the movement.
        let fine = ui.input(|i| i.modifiers.ctrl || i.modifiers.shift);
        if fine && resp.dragged() {
            pos = (sc.to_pos)(v) + resp.drag_delta().x / track.width().max(1.0) * 0.1;
        }
        let pos = pos.clamp(0.0, 1.0);
        let pos = if !fine && (pos - 0.5).abs() < DETENT { 0.5 } else { pos };
        let nv = (sc.to_val)(pos);
        if resp.clicked() {
            fx.set(key, nv);
        } else {
            fx.set_live(key, nv);
        }
    }
    if resp.drag_stopped() {
        fx.released = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notch_is_dead_middle_and_the_scales_round_trip() {
        assert_eq!((PAN.to_pos)(0.0), 0.5);
        assert_eq!((WIDTH.to_pos)(100.0), 0.5);
        assert_eq!((WIDTH.to_pos)(0.0), 0.0);
        assert_eq!((WIDTH.to_pos)(300.0), 1.0);
        for v in [0.0, 25.0, 100.0, 180.0, 300.0] {
            assert!(((WIDTH.to_val)((WIDTH.to_pos)(v)) - v).abs() < 1e-3, "{v}");
        }
        for v in [-100.0, -40.0, 0.0, 40.0, 100.0] {
            assert!(((PAN.to_val)((PAN.to_pos)(v)) - v).abs() < 1e-3, "{v}");
        }
        assert_eq!(pan_text(-40.0), "L40");
        assert_eq!(pan_text(0.2), "C");
        assert_eq!(width_text(0.0), "Mono");
    }
}
