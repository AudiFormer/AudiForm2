// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.
// Ported from Audiform (Audiform/src/panels/voice-chain/MixerPanel.svelte), written by its author (AudiFormer)
// for Audiform and relicensed here by them. See docs/clean-room.md.

//! The Channel Mixer's window: a small tab for each output side (L, R);
//! on each, under an "Input Channel / Invert" header, one row per input
//! channel: a long slider over a 0, 50, 100 scale for how much of it goes
//! in, its value in a box to type into, and a box to tick to flip that
//! part's polarity (Invert). Drag or click a slider (Ctrl or Shift for
//! fine moves); double-click it for its default.

use egui::{pos2, vec2, Align2, FontId, Rect, Rounding, Sense, Stroke};

use super::layout::{self, GAP, GAP_S};
use super::{Env, Fx};
use crate::theme::{c, ca, lerp, Theme};
use crate::widgets;

/// The output sides: key letter, tab name, and the side's name.
const SIDES: [(&str, &str, &str); 2] = [("l", "L", "left"), ("r", "R", "right")];
/// The numbers along each slider's scale.
const MARKS: [f64; 3] = [0.0, 50.0, 100.0];

/// The face's width inside the frame.
const FACE_W: f32 = 380.0;
/// The name column, the value box and the Invert column.
const NAME_W: f32 = 14.0;
const VALUE_W: f32 = 64.0;
const INVERT_W: f32 = 44.0;
const ROW_H: f32 = 40.0;

/// The amount key for output `out` from input `inp` ("lr": left from right).
fn amount_key(out: &str, inp: &str) -> String {
    format!("{out}{inp}")
}

fn invert_key(out: &str, inp: &str) -> String {
    format!("{out}{inp}_inv")
}

/// Where an amount sits along its slider (0..1), and back (whole steps).
fn pos_of(v: f64, lo: f64, hi: f64) -> f32 {
    ((v - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0) as f32
}

fn val_of(t: f32, lo: f64, hi: f64) -> f64 {
    (lo + f64::from(t.clamp(0.0, 1.0)) * (hi - lo)).round()
}

pub fn panel(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = &env.th;
    let w = FACE_W.max(layout::MIN_W);
    let side_id = fx.uid.with("mixer-side");
    let mut side: usize = ui.ctx().data(|d| d.get_temp(side_id)).unwrap_or(0);
    let square = th.bevel.is_some();
    egui::Frame::none()
        .stroke(Stroke::new(1.0_f32, ca(th.border, 230)))
        .rounding(if square { 0.0 } else { 6.0 })
        .inner_margin(egui::Margin { left: 14.0, right: 14.0, top: GAP_S, bottom: GAP })
        .show(ui, |ui| {
            ui.set_width(w);
            // The output tabs.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                for (k, (_, name, long)) in SIDES.iter().enumerate() {
                    if tab(ui, th, name, side == k)
                        .on_hover_text(format!("{} output: what goes into it", capital(long)))
                        .clicked()
                    {
                        side = k;
                    }
                }
            });
            ui.ctx().data_mut(|d| d.insert_temp(side_id, side));
            ui.add_space(4.0);
            // The header over the rows.
            let (head, _) = ui.allocate_exact_size(vec2(w, 16.0), Sense::hover());
            let p = ui.painter();
            let font = FontId::proportional(12.0);
            p.text(pos2(head.left() + 24.0, head.center().y), Align2::LEFT_CENTER, "Input Channel", font.clone(), c(th.text_dim));
            p.text(pos2(head.right() - INVERT_W / 2.0, head.center().y), Align2::CENTER_CENTER, "Invert", font, c(th.text_dim));
            let (out, _, out_long) = SIDES[side];
            for (inp, name, long) in SIDES {
                let (row, _) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::hover());
                let slider_r = Rect::from_min_max(row.min, pos2(row.right() - INVERT_W, row.bottom()));
                slider_row(ui, fx, th, slider_r, &amount_key(out, inp), name);
                let inv_r = Rect::from_min_max(pos2(row.right() - INVERT_W, row.top()), row.max);
                let key = invert_key(out, inp);
                let on = fx.on(&key);
                let resp = check(ui, th, fx.uid.with(("mixer-inv", out, inp)), inv_r, on)
                    .on_hover_text(format!("Flip the polarity of this part (invert {long} into {out_long})"));
                if resp.clicked() {
                    fx.set(&key, if on { 0.0 } else { 1.0 });
                }
            }
        });
}

fn capital(s: &str) -> String {
    let mut ch = s.chars();
    ch.next().map_or_else(String::new, |f| f.to_uppercase().chain(ch).collect())
}

/// An output tab: its name, underlined in the accent when shown.
fn tab(ui: &mut egui::Ui, th: &Theme, name: &str, on: bool) -> egui::Response {
    let font = FontId::proportional(13.0);
    let g = ui.painter().layout_no_wrap(name.to_string(), font, c(th.text));
    let (r, resp) = ui.allocate_exact_size(g.size() + vec2(4.0, 4.0), Sense::click());
    let col = if on || resp.hovered() { c(th.text) } else { c(th.text_dim) };
    let p = ui.painter();
    p.text(pos2(r.center().x, r.top()), Align2::CENTER_TOP, name, FontId::proportional(13.0), col);
    // Twice, a hair apart: a heavier face.
    p.text(pos2(r.center().x + 0.5, r.top()), Align2::CENTER_TOP, name, FontId::proportional(13.0), col);
    if on {
        p.line_segment([pos2(r.left(), r.bottom() - 1.0), pos2(r.right(), r.bottom() - 1.0)], Stroke::new(2.0_f32, c(th.accent)));
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// One input's amount: its name, a long slider over a numbered scale,
/// and its value in a box.
fn slider_row(ui: &mut egui::Ui, fx: &mut Fx, th: &Theme, r: Rect, key: &str, name: &str) {
    let Some(ctl) = fx.ctl(key).cloned() else { return };
    let v = fx.get(key);
    let p = ui.painter_at(r.expand(2.0));
    let cy = r.center().y + 3.0;
    p.text(pos2(r.left() + NAME_W, cy), Align2::RIGHT_CENTER, name, FontId::proportional(12.0), c(th.text_dim));
    let track = Rect::from_min_max(
        pos2(r.left() + NAME_W + 10.0 + 7.0, cy - 2.0),
        pos2(r.right() - VALUE_W - 10.0 - 7.0, cy + 2.0),
    );
    let x_of = |t: f32| track.left() + t * track.width();
    // The numbers over the track and the ticks under it.
    let faint = ca(th.text_dim, 170);
    for m in MARKS {
        let x = x_of(pos_of(m, ctl.min, ctl.max));
        p.text(pos2(x, track.top() - 8.0), Align2::CENTER_BOTTOM, format!("{m}"), FontId::proportional(10.0), faint);
        p.line_segment([pos2(x, track.bottom() + 6.0), pos2(x, track.bottom() + 10.0)], Stroke::new(1.0_f32, faint));
    }
    let at = pos_of(v, ctl.min, ctl.max);
    p.rect_filled(track, Rounding::same(2.0), ca(th.text_dim, 55));
    p.rect_filled(Rect::from_min_max(track.min, pos2(x_of(at), track.max.y)), Rounding::same(2.0), ca(th.accent, 210));
    let hit = Rect::from_min_max(pos2(track.left() - 8.0, r.top() + 6.0), pos2(track.right() + 8.0, r.bottom()));
    let resp = ui
        .interact(hit, fx.uid.with(("mixer-slider", key)), Sense::click_and_drag())
        .on_hover_text(format!("{}: drag or click; Ctrl or Shift for fine steps; double-click to reset", ctl.name));
    let hot = resp.hovered() || resp.dragged();
    let knob = pos2(x_of(at), track.center().y);
    let rad = if hot { 7.5 } else { 6.5 };
    p.circle_filled(knob, rad, c(th.panel_raised));
    p.circle_stroke(knob, rad, Stroke::new(2.0_f32, if hot { c(th.accent) } else { c(th.text) }));
    if resp.double_clicked() {
        fx.set(key, ctl.default);
    } else if let Some(pp) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
        let fine = ui.input(|i| i.modifiers.ctrl || i.modifiers.shift);
        let t = if fine && resp.dragged() {
            at + resp.drag_delta().x / track.width().max(1.0) * 0.1
        } else {
            (pp.x - track.left()) / track.width().max(1.0)
        };
        let nv = val_of(t, ctl.min, ctl.max);
        if resp.clicked() {
            fx.set(key, nv);
        } else {
            fx.set_live(key, nv);
        }
    }
    if resp.drag_stopped() {
        fx.released = true;
    }
    // The value box at the end.
    let well = Rect::from_min_size(pos2(r.right() - VALUE_W, cy - 9.0), vec2(VALUE_W, 18.0));
    let mut bv = fx.get(key);
    let unit = ctl.unit.clone();
    // A child placed on the row (the row already took the space).
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(well));
    let b = widgets::value_box(&mut child, fx.uid.with(("mixer-box", key)), &mut bv, ctl.min, ctl.max, false, VALUE_W, &|v| {
        widgets::fmt_unit(v, &unit)
    });
    if b.changed() {
        fx.set_live(key, bv);
    }
    fx.track(&b);
}

/// A tick box centred in `r`.
fn check(ui: &mut egui::Ui, th: &Theme, id: egui::Id, r: Rect, on: bool) -> egui::Response {
    let bx = Rect::from_center_size(pos2(r.center().x, r.center().y + 3.0), vec2(15.0, 15.0));
    let resp = ui.interact(bx.expand(3.0), id, Sense::click());
    let p = ui.painter();
    let square = th.bevel.is_some();
    let rad = Rounding::same(if square { 0.0 } else { 3.0 });
    let hot = resp.hovered();
    p.rect_filled(bx, rad, if on { c(th.accent) } else { c(th.wave_background) });
    if square {
        crate::skin::bevel(p, bx, th, !on);
    } else {
        let edge = if on || hot { c(th.accent) } else { ca(th.border, 230) };
        p.rect_stroke(bx, rad, Stroke::new(1.0_f32, edge));
    }
    if on {
        let ink = if th.dark { lerp(th.background, [0, 0, 0], 0.3) } else { egui::Color32::WHITE };
        let s = Stroke::new(2.0_f32, ink);
        let a = pos2(bx.left() + 3.5, bx.center().y);
        let b = pos2(bx.left() + 6.5, bx.bottom() - 4.0);
        let e = pos2(bx.right() - 3.0, bx.top() + 3.5);
        p.line_segment([a, b], s);
        p.line_segment([b, e], s);
    }
    if hot {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_used_is_the_effects() {
        let info = af2_fx::effects::mixer::mixer_info();
        let has = |k: &str| info.params.iter().any(|p| p.key == k);
        for (out, _, _) in SIDES {
            for (inp, _, _) in SIDES {
                assert!(has(&amount_key(out, inp)), "{out}{inp}");
                assert!(has(&invert_key(out, inp)), "{out}{inp}_inv");
            }
        }
        let p = info.params.iter().find(|p| p.key == "ll").unwrap();
        assert_eq!((p.min, p.max), (MARKS[0], MARKS[2]));
    }

    #[test]
    fn slider_positions_round_trip() {
        for v in [0.0, 1.0, 37.0, 50.0, 99.0, 100.0] {
            assert_eq!(val_of(pos_of(v, 0.0, 100.0), 0.0, 100.0), v);
        }
        assert_eq!(pos_of(150.0, 0.0, 100.0), 1.0);
        assert_eq!(val_of(-0.2, 0.0, 100.0), 0.0);
        assert_eq!(capital("left"), "Left");
    }
}
