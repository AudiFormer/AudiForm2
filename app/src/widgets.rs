// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! AudiForm 2's own slider, in place of egui's generic track-and-circle.
//! Same builder calls as `egui::Slider` for what the app uses (`text`,
//! `suffix`, `logarithmic`, `show_value`, `step_by`, `integer`), so it
//! drops in. Drawn from the theme: a slim recessed groove lit with the
//! theme's light up to the value, and a fader-cap handle with a lit line
//! (square and bevelled on themes with bevels). Drag anywhere on it; Shift
//! for fine steps; the value box beside it takes typing.

use egui::emath::Numeric;
use egui::{pos2, vec2, Color32, Rect, Response, Rounding, Sense, Shape, Stroke, Ui, Widget};
use std::ops::RangeInclusive;

use crate::theme::{c, ca, lerp, Theme};

pub struct Slider<'a, T: Numeric> {
    value: &'a mut T,
    range: RangeInclusive<T>,
    text: String,
    suffix: String,
    log: bool,
    show_value: bool,
    step: Option<f64>,
    integer: bool,
}

impl<'a, T: Numeric> Slider<'a, T> {
    pub fn new(value: &'a mut T, range: RangeInclusive<T>) -> Self {
        Slider {
            value,
            range,
            text: String::new(),
            suffix: String::new(),
            log: false,
            show_value: true,
            step: None,
            integer: T::INTEGRAL,
        }
    }
    pub fn text(mut self, t: impl ToString) -> Self {
        self.text = t.to_string();
        self
    }
    pub fn suffix(mut self, s: impl ToString) -> Self {
        self.suffix = s.to_string();
        self
    }
    pub fn logarithmic(mut self, on: bool) -> Self {
        self.log = on;
        self
    }
    pub fn show_value(mut self, on: bool) -> Self {
        self.show_value = on;
        self
    }
    pub fn step_by(mut self, step: f64) -> Self {
        self.step = (step > 0.0).then_some(step);
        self
    }
    pub fn integer(mut self) -> Self {
        self.integer = true;
        self
    }
}

/// Position 0..1 along the slider for value `v` in `lo..=hi`.
pub fn to_pos(v: f64, lo: f64, hi: f64, log: bool) -> f64 {
    if hi <= lo {
        return 0.0;
    }
    let v = v.clamp(lo, hi);
    if log && lo >= 0.0 {
        let floor = (hi * 1e-4).max(f64::MIN_POSITIVE);
        let l = lo.max(floor);
        if v <= l {
            return 0.0;
        }
        ((v / l).ln() / (hi / l).ln()).clamp(0.0, 1.0)
    } else {
        (v - lo) / (hi - lo)
    }
}

/// The value at position `p` (0..1); inverse of [`to_pos`].
pub fn from_pos(p: f64, lo: f64, hi: f64, log: bool) -> f64 {
    let p = p.clamp(0.0, 1.0);
    if hi <= lo {
        return lo;
    }
    if log && lo >= 0.0 {
        let floor = (hi * 1e-4).max(f64::MIN_POSITIVE);
        let l = lo.max(floor);
        if p <= 0.0 {
            return lo;
        }
        l * (hi / l).powf(p)
    } else {
        lo + p * (hi - lo)
    }
}

fn theme(ui: &Ui) -> Theme {
    ui.ctx()
        .data(|d| d.get_temp::<Theme>(egui::Id::new("af2-theme")))
        .unwrap_or_default()
}

/// Call once per frame so widgets can read the theme.
pub fn set_theme(ctx: &egui::Context, th: &Theme) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("af2-theme"), th.clone()));
}

impl<T: Numeric> Widget for Slider<'_, T> {
    fn ui(self, ui: &mut Ui) -> Response {
        let th = theme(ui);
        let (lo, hi) = (self.range.start().to_f64(), self.range.end().to_f64());
        let mut v = self.value.to_f64();
        let snap = |x: f64| {
            let mut x = x.clamp(lo.min(hi), hi.max(lo));
            if let Some(s) = self.step {
                x = lo + ((x - lo) / s).round() * s;
            }
            if self.integer {
                x = x.round();
            }
            x
        };
        ui.horizontal(|ui| {
            let width = ui.spacing().slider_width.max(110.0);
            let h = ui.spacing().interact_size.y.max(18.0);
            let (rect, mut resp) = ui.allocate_exact_size(vec2(width, h), Sense::click_and_drag());
            let groove = Rect::from_min_max(
                pos2(rect.left() + 6.0, rect.center().y - 2.0),
                pos2(rect.right() - 6.0, rect.center().y + 2.0),
            );
            // Interaction: absolute on click, relative (fine with Shift)
            // while dragging so the value never jumps under the hand.
            if resp.drag_started() || resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let pos = ((p.x - groove.left()) / groove.width()) as f64;
                    let nv = snap(from_pos(pos, lo, hi, self.log));
                    if nv != v {
                        v = nv;
                        resp.mark_changed();
                    }
                }
            } else if resp.dragged() {
                let fine = ui.input(|i| i.modifiers.shift);
                let dx = resp.drag_delta().x as f64 * if fine { 0.15 } else { 1.0 };
                if dx != 0.0 {
                    let pos = to_pos(v, lo, hi, self.log) + dx / groove.width() as f64;
                    let nv = snap(from_pos(pos, lo, hi, self.log));
                    if nv != v {
                        v = nv;
                        resp.mark_changed();
                    }
                }
            }
            let hot = resp.hovered() || resp.dragged();
            let p = ui.painter();
            let square = th.bevel.is_some();
            // The groove, recessed.
            let r = Rounding::same(if square { 0.0 } else { 2.0 });
            p.rect_filled(groove, r, c(th.wave_background));
            if square {
                crate::skin::bevel(p, groove.expand(1.0), &th, true);
            } else {
                p.rect_stroke(groove, r, Stroke::new(1.0_f32, ca(th.border, 220)));
            }
            // Lit up to the value: a gradient from the theme's second light
            // to its main one, glowing a little on dark themes.
            let f = to_pos(v, lo, hi, self.log) as f32;
            let x = groove.left() + groove.width() * f;
            if f > 0.0 {
                let lit =
                    Rect::from_min_max(groove.min, pos2(x, groove.max.y)).shrink2(vec2(0.0, 0.5));
                let mut m = egui::Mesh::default();
                let (a, b) = (lerp(th.accent, th.accent_alt, 0.55), c(th.accent));
                m.colored_vertex(lit.left_top(), a);
                m.colored_vertex(lit.right_top(), b);
                m.colored_vertex(lit.right_bottom(), b);
                m.colored_vertex(lit.left_bottom(), a);
                m.add_triangle(0, 1, 2);
                m.add_triangle(0, 2, 3);
                p.add(Shape::mesh(m));
                if th.dark && th.glow > 0.0 {
                    p.rect_filled(
                        lit.expand(2.0),
                        Rounding::same(3.0),
                        ca(th.accent, (18.0 * th.glow) as u8),
                    );
                }
            }
            // Ticks at the quarters, quiet.
            for q in [0.25f32, 0.5, 0.75] {
                let tx = groove.left() + groove.width() * q;
                p.line_segment(
                    [
                        pos2(tx, groove.bottom() + 3.0),
                        pos2(tx, groove.bottom() + 5.0),
                    ],
                    Stroke::new(1.0_f32, ca(th.text_dim, 90)),
                );
            }
            // The cap.
            let cap =
                Rect::from_center_size(pos2(x, rect.center().y), vec2(10.0, (h - 2.0).min(18.0)));
            p.rect_filled(
                cap.translate(vec2(1.0, 1.5)),
                Rounding::same(2.0),
                Color32::from_black_alpha(80),
            );
            let face = lerp(th.panel_raised, th.text, if hot { 0.30 } else { 0.18 });
            p.rect_filled(cap, Rounding::same(if square { 0.0 } else { 2.0 }), face);
            if square {
                crate::skin::bevel(p, cap, &th, resp.dragged());
            } else {
                p.rect_stroke(
                    cap,
                    Rounding::same(2.0),
                    Stroke::new(
                        1.0_f32,
                        if hot {
                            c(th.accent)
                        } else {
                            ca(th.border, 240)
                        },
                    ),
                );
                if hot && th.glow > 0.0 {
                    p.rect_stroke(
                        cap.expand(2.0),
                        Rounding::same(3.0),
                        Stroke::new(2.0_f32, ca(th.accent, (50.0 * th.glow) as u8)),
                    );
                }
            }
            p.line_segment(
                [
                    pos2(cap.center().x, cap.top() + 3.0),
                    pos2(cap.center().x, cap.bottom() - 3.0),
                ],
                Stroke::new(2.0_f32, c(th.accent)),
            );
            if self.show_value {
                let mut dv = egui::DragValue::new(&mut v)
                    .range(lo.min(hi)..=hi.max(lo))
                    .suffix(self.suffix.clone())
                    .speed(((hi - lo).abs() / 300.0).max(if self.integer { 1.0 } else { 1e-4 }));
                if self.integer {
                    dv = dv.max_decimals(0);
                } else {
                    dv = dv.max_decimals(2);
                }
                let r2 = ui.add(dv);
                if r2.changed() {
                    v = snap(v);
                    resp.mark_changed();
                }
                resp = resp.union(r2);
            }
            if !self.text.is_empty() {
                ui.label(&self.text);
            }
            if resp.changed() {
                *self.value = T::from_f64(v);
            }
            resp
        })
        .inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_round_trip_linear_and_log() {
        for &(lo, hi, log) in &[
            (-60.0, 12.0, false),
            (20.0, 20000.0, true),
            (0.0, 1.0, true),
            (0.0, 8.0, false),
        ] {
            for k in 0..=10 {
                let p = k as f64 / 10.0;
                let v = from_pos(p, lo, hi, log);
                let back = to_pos(v, lo, hi, log);
                assert!(
                    (back - p).abs() < 1e-9 || (p == 0.0 && back == 0.0),
                    "{lo} {hi} {log} {p} {back}"
                );
            }
        }
        assert!((to_pos(632.45, 20.0, 20000.0, true) - 0.5).abs() < 1e-3);
    }
}
