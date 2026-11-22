// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The Studio Reverb's window: its ten controls as long sliders in two
//! groups, Characteristics and Output Level, each with a scale of its own
//! printed over it (log scales for Decay and the two cuts) and its value
//! at the end. Drag, or click anywhere on a slider to jump there;
//! Ctrl or Shift for fine moves; double-click for the control's default.

use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Rounding, Sense, Stroke};

use super::{Env, Fx};
use crate::theme::{c, ca};

/// A control's scale: its ends, log or not, and the marks printed over it.
struct Row {
    key: &'static str,
    name: &'static str,
    log: bool,
    lo: f64,
    hi: f64,
    marks: &'static [f64],
    unit: &'static str,
}

const CHARACTER: [Row; 8] = [
    Row { key: "size", name: "Room Size", log: false, lo: 0.0, hi: 100.0, marks: &[0.0, 20.0, 40.0, 60.0, 80.0, 100.0], unit: "" },
    Row { key: "decay", name: "Decay", log: true, lo: 200.0, hi: 10_000.0, marks: &[200.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0], unit: "ms" },
    Row { key: "early", name: "Early Reflections", log: false, lo: 0.0, hi: 100.0, marks: &[0.0, 20.0, 40.0, 60.0, 80.0, 100.0], unit: "%" },
    Row { key: "width", name: "Width", log: false, lo: 0.0, hi: 100.0, marks: &[0.0, 20.0, 40.0, 60.0, 80.0, 100.0], unit: "" },
    Row { key: "high_cut", name: "High Frequency Cut", log: true, lo: 20.0, hi: 20_000.0, marks: &[20.0, 80.0, 200.0, 800.0, 2000.0, 6000.0, 16_000.0], unit: "Hz" },
    Row { key: "low_cut", name: "Low Frequency Cut", log: true, lo: 20.0, hi: 4_000.0, marks: &[20.0, 60.0, 200.0, 600.0, 2000.0, 4000.0], unit: "Hz" },
    Row { key: "damping", name: "Damping", log: false, lo: 0.0, hi: 100.0, marks: &[0.0, 20.0, 40.0, 60.0, 80.0, 100.0], unit: "%" },
    Row { key: "diffusion", name: "Diffusion", log: false, lo: 0.0, hi: 100.0, marks: &[0.0, 20.0, 40.0, 60.0, 80.0, 100.0], unit: "%" },
];

const OUTPUT: [Row; 2] = [
    Row { key: "dry", name: "Dry", log: false, lo: 0.0, hi: 100.0, marks: &[0.0, 20.0, 40.0, 60.0, 80.0, 100.0], unit: "%" },
    Row { key: "wet", name: "Wet", log: false, lo: 0.0, hi: 100.0, marks: &[0.0, 20.0, 40.0, 60.0, 80.0, 100.0], unit: "%" },
];

impl Row {
    fn pos(&self, v: f64) -> f32 {
        let t = if self.log {
            (v.max(self.lo).ln() - self.lo.ln()) / (self.hi.ln() - self.lo.ln())
        } else {
            (v - self.lo) / (self.hi - self.lo)
        };
        t.clamp(0.0, 1.0) as f32
    }

    fn val(&self, p: f32) -> f64 {
        let p = f64::from(p.clamp(0.0, 1.0));
        let v = if self.log {
            (self.lo.ln() + p * (self.hi.ln() - self.lo.ln())).exp()
        } else {
            self.lo + p * (self.hi - self.lo)
        };
        // Whole numbers, as the controls step.
        v.round()
    }
}

/// A scale mark's label: 8000 as "8000", 16000 as "16k" when tight.
fn mark_text(v: f64) -> String {
    if v >= 10_000.0 {
        format!("{}k", (v / 1000.0).round())
    } else {
        format!("{}", v.round())
    }
}

/// The name column's width and the value column's.
const NAME_W: f32 = 132.0;
const VALUE_W: f32 = 74.0;
const ROW_H: f32 = 34.0;

pub fn panel(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let w = ui.available_width().max(420.0);
    group(ui, fx, env, w, "Characteristics", &CHARACTER);
    ui.add_space(8.0);
    group(ui, fx, env, w, "Output Level", &OUTPUT);
}

fn group(ui: &mut egui::Ui, fx: &mut Fx, env: &Env, w: f32, title: &str, rows: &[Row]) {
    let th = &env.th;
    let h = 26.0 + ROW_H * rows.len() as f32 + 8.0;
    let (frame, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let p = ui.painter_at(frame);
    // A recessed card with a thin lit edge along its top.
    p.rect_filled(frame, Rounding::same(6.0), ca(th.background, 150));
    p.rect_stroke(frame, Rounding::same(6.0), Stroke::new(1.0_f32, ca(th.border, 200)));
    p.line_segment(
        [pos2(frame.left() + 8.0, frame.top() + 0.5), pos2(frame.right() - 8.0, frame.top() + 0.5)],
        Stroke::new(1.0_f32, ca(th.accent, 90)),
    );
    p.text(
        pos2(frame.left() + 12.0, frame.top() + 7.0),
        Align2::LEFT_TOP,
        title.to_uppercase(),
        FontId::proportional(10.5),
        ca(th.accent, 220),
    );
    let mut y = frame.top() + 26.0;
    for row in rows {
        let r = Rect::from_min_size(pos2(frame.left(), y), vec2(w, ROW_H));
        slider_row(ui, fx, env, r, row);
        y += ROW_H;
    }
}

fn slider_row(ui: &mut egui::Ui, fx: &mut Fx, env: &Env, r: Rect, row: &Row) {
    let th = &env.th;
    let p = ui.painter_at(r);
    let v = fx.get(row.key);
    // The name, right-aligned to the slider.
    p.text(
        pos2(r.left() + NAME_W, r.center().y + 5.0),
        Align2::RIGHT_CENTER,
        row.name,
        FontId::proportional(12.0),
        c(th.text),
    );
    let track = Rect::from_min_max(
        pos2(r.left() + NAME_W + 16.0, r.center().y + 3.0),
        pos2(r.right() - VALUE_W - 14.0, r.center().y + 7.0),
    );
    let x_of = |t: f32| track.left() + t * track.width();
    // The scale over the track: ticks and their numbers.
    let small = FontId::proportional(9.0);
    for &m in row.marks {
        let x = x_of(row.pos(m));
        p.line_segment([pos2(x, track.top() - 5.0), pos2(x, track.top() - 1.0)], Stroke::new(1.0_f32, ca(th.text_dim, 170)));
        p.text(pos2(x, track.top() - 6.0), Align2::CENTER_BOTTOM, mark_text(m), small.clone(), ca(th.text_dim, 200));
    }
    // Finer ticks between, on the linear scales.
    if !row.log {
        for k in 0..=20 {
            let x = x_of(k as f32 / 20.0);
            p.line_segment([pos2(x, track.top() - 3.0), pos2(x, track.top() - 1.0)], Stroke::new(1.0_f32, ca(th.text_dim, 90)));
        }
    }
    let at = row.pos(v);
    p.rect_filled(track, Rounding::same(2.0), ca(th.text_dim, 55));
    p.rect_filled(
        Rect::from_min_max(track.min, pos2(x_of(at), track.max.y)),
        Rounding::same(2.0),
        ca(th.accent, 210),
    );
    // The hit area: the track's row, generous in height.
    let hit = Rect::from_min_max(pos2(track.left() - 8.0, r.top() + 4.0), pos2(track.right() + 8.0, r.bottom()));
    let resp = ui
        .interact(hit, fx.uid.with(("studio", row.key)), Sense::click_and_drag())
        .on_hover_text(format!("{}: drag or click; Ctrl or Shift for fine steps; double-click for the default", row.name));
    let hot = resp.hovered() || resp.dragged();
    let knob = pos2(x_of(at), track.center().y);
    p.circle_filled(knob, if hot { 7.5 } else { 6.5 }, c(th.panel_raised));
    p.circle_stroke(knob, if hot { 7.5 } else { 6.5 }, Stroke::new(2.0_f32, if hot { c(th.accent) } else { c(th.text) }));
    p.circle_filled(knob, 2.0, if hot { c(th.accent) } else { Color32::TRANSPARENT });
    // The value, in a small well at the end.
    let well = Rect::from_min_size(pos2(r.right() - VALUE_W - 6.0, r.center().y - 4.0), vec2(VALUE_W, 18.0));
    p.rect_filled(well, Rounding::same(3.0), ca(th.background, 200));
    let shown = if row.unit.is_empty() {
        format!("{}", v.round())
    } else {
        format!("{} {}", v.round(), row.unit)
    };
    p.text(pos2(well.right() - 6.0, well.center().y), Align2::RIGHT_CENTER, shown, FontId::monospace(11.5), c(th.accent));

    if resp.double_clicked() {
        if let Some(d) = fx.ctl(row.key).map(|c| c.default) {
            fx.set(row.key, d);
        }
        return;
    }
    if let Some(pp) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
        let fine = ui.input(|i| i.modifiers.ctrl || i.modifiers.shift);
        let t = if fine && resp.dragged() {
            at + resp.drag_delta().x / track.width().max(1.0) * 0.1
        } else {
            (pp.x - track.left()) / track.width().max(1.0)
        };
        let nv = row.val(t);
        if resp.clicked() {
            fx.set(row.key, nv);
        } else {
            fx.set_live(row.key, nv);
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
    fn scales_round_trip_and_follow_the_controls() {
        for row in CHARACTER.iter().chain(OUTPUT.iter()) {
            let info = af2_fx::effects::studio::studio_info();
            let p = info.params.iter().find(|p| p.key == row.key).expect(row.key);
            assert_eq!((p.min, p.max), (row.lo, row.hi), "{}", row.key);
            for &m in row.marks {
                let back = row.val(row.pos(m));
                assert!((back - m).abs() <= (m * 0.01).max(1.0), "{} {m} -> {back}", row.key);
            }
        }
        assert_eq!(mark_text(16_000.0), "16k");
    }
}
