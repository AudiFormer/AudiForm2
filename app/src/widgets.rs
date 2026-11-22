// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! AudiForm 2's own slider, in place of egui's generic track-and-circle.
//! Same builder calls as `egui::Slider` for what the app uses (`text`,
//! `suffix`, `logarithmic`, `show_value`, `step_by`, `integer`), so it
//! drops in. Drawn from the theme: a slim recessed groove lit with the
//! theme's light up to the value, and a fader-cap handle with a lit line
//! (square and bevelled on themes with bevels). Drag anywhere on it; Shift
//! for fine steps; the value box beside it takes typing.
//!
//! Also here: the vertical fader ([`VFader`]), the big dial ([`knob`]),
//! the value box, stepped switches and lit switches, all from the theme.

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

pub fn theme(ui: &Ui) -> Theme {
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

// ------------------------------------------------------------ value box

/// A number in a small recessed box, in the theme's light: drag up or
/// down on it to change it (Shift for fine steps), turn the wheel over
/// it, double-click it to type a value (Enter sets it, Escape leaves).
/// `lo..=hi` along `log` or straight; `fmt` shows it.
#[allow(clippy::too_many_arguments)]
pub fn value_box(
    ui: &mut Ui,
    id: egui::Id,
    value: &mut f64,
    lo: f64,
    hi: f64,
    log: bool,
    width: f32,
    fmt: &dyn Fn(f64) -> String,
) -> Response {
    let th = theme(ui);
    let editing: Option<String> = ui.ctx().data(|d| d.get_temp(id));
    let size = vec2(width, 18.0);
    if let Some(mut text) = editing {
        let r = ui.add_sized(
            size,
            egui::TextEdit::singleline(&mut text)
                .horizontal_align(egui::Align::Center)
                .font(egui::FontId::proportional(11.0)),
        );
        let (enter, esc) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        let mut resp = r.clone();
        if esc {
            ui.ctx().data_mut(|d| d.remove::<String>(id));
        } else if enter || r.lost_focus() {
            ui.ctx().data_mut(|d| d.remove::<String>(id));
            if let Some(v) = parse_value(&text) {
                let v = v.clamp(lo.min(hi), hi.max(lo));
                if v != *value {
                    *value = v;
                    resp.mark_changed();
                }
            }
        } else {
            r.request_focus();
            ui.ctx().data_mut(|d| d.insert_temp(id, text));
        }
        return resp;
    }
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let mut v = *value;
    if resp.double_clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(id, fmt(v)));
    } else if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let dy = -resp.drag_delta().y as f64 * if fine { 0.1 } else { 1.0 };
        if dy != 0.0 {
            v = from_pos(to_pos(v, lo, hi, log) + dy / 250.0, lo, hi, log);
        }
    }
    if resp.hovered() {
        v = wheel(ui, v, lo, hi, log);
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    if v != *value {
        *value = v;
        resp.mark_changed();
    }
    let p = ui.painter();
    let square = th.bevel.is_some();
    let r = Rounding::same(if square { 0.0 } else { 2.0 });
    p.rect_filled(rect, r, c(th.wave_background));
    if square {
        crate::skin::bevel(p, rect, &th, true);
    } else {
        let edge = if resp.hovered() || resp.dragged() {
            c(th.accent)
        } else {
            ca(th.border, 230)
        };
        p.rect_stroke(rect, r, Stroke::new(1.0_f32, edge));
    }
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        fmt(*value),
        egui::FontId::monospace(10.5),
        c(th.accent),
    );
    resp.on_hover_text("Drag up or down, or turn the wheel (Shift: fine). Double-click to type.")
}

/// One wheel notch over a control moves it 1% of the way (Shift: 0.2%).
pub fn wheel(ui: &Ui, v: f64, lo: f64, hi: f64, log: bool) -> f64 {
    let (dy, fine) = ui.input(|i| (i.raw_scroll_delta.y, i.modifiers.shift));
    if dy == 0.0 {
        return v;
    }
    let step = if fine { 0.002 } else { 0.01 } * dy.signum() as f64;
    from_pos(to_pos(v, lo, hi, log) + step, lo, hi, log)
}

/// A typed value: "1.5k" is 1500, "-6 dB" is -6, "2,5" is 2.5.
pub fn parse_value(text: &str) -> Option<f64> {
    let t = text.trim().replace(',', ".");
    let end = t
        .char_indices()
        .find(|(i, ch)| {
            !(ch.is_ascii_digit() || *ch == '.' || ((*ch == '-' || *ch == '+') && *i == 0))
        })
        .map_or(t.len(), |(i, _)| i);
    let mut v: f64 = t[..end].parse().ok()?;
    if t[end..].trim_start().to_ascii_lowercase().starts_with('k') {
        v *= 1000.0;
    }
    v.is_finite().then_some(v)
}

/// A value as a readout: kHz from 1000 Hz, seconds from 1000 ms.
pub fn fmt_unit(v: f64, unit: &str) -> String {
    match unit {
        "Hz" if v.abs() >= 1000.0 => format!("{:.2} kHz", v / 1000.0),
        "Hz" => format!("{v:.0} Hz"),
        "ms" if v.abs() >= 1000.0 => format!("{:.2} s", v / 1000.0),
        "ms" => format!("{v:.0} ms"),
        "s" => format!("{v:.2} s"),
        "dB" | "dBFS" | "LUFS" => format!("{v:.1} {unit}"),
        "%" => format!("{v:.0}%"),
        "°" => format!("{v:.1}°"),
        "" => format!("{v:.2}"),
        u => format!("{v:.1} {u}"),
    }
}

/// A tick label: "2k", "-12", "0.5".
fn tick_label(t: f64) -> String {
    if t.abs() >= 1000.0 {
        format!("{}k", t / 1000.0)
    } else if t.fract() == 0.0 {
        format!("{t:.0}")
    } else {
        format!("{t:.1}")
    }
}

// ------------------------------------------------------------ the fader

/// A vertical fader: a recessed groove lit from the bottom up to the
/// value, a fader cap, tick labels on its left and a readout under it.
/// Drag it (Shift for fine), click the groove to jump, double-click for
/// the default, wheel to step; the readout takes typing.
pub struct VFader<'a> {
    value: &'a mut f64,
    lo: f64,
    hi: f64,
    default: f64,
    height: f32,
    log: bool,
    label: String,
    unit: String,
    ticks: Vec<f64>,
}

impl<'a> VFader<'a> {
    pub fn new(value: &'a mut f64, range: RangeInclusive<f64>) -> Self {
        let (lo, hi) = (*range.start(), *range.end());
        VFader {
            value,
            lo,
            hi,
            default: lo,
            height: 140.0,
            log: false,
            label: String::new(),
            unit: String::new(),
            ticks: vec![lo, (lo + hi) / 2.0, hi],
        }
    }
    pub fn default_value(mut self, d: f64) -> Self {
        self.default = d;
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
    #[allow(dead_code)]
    pub fn logarithmic(mut self, on: bool) -> Self {
        self.log = on;
        self
    }
    pub fn label(mut self, t: impl ToString) -> Self {
        self.label = t.to_string();
        self
    }
    pub fn unit(mut self, u: impl ToString) -> Self {
        self.unit = u.to_string();
        self
    }
    pub fn ticks(mut self, t: &[f64]) -> Self {
        self.ticks = t.to_vec();
        self
    }
}

impl Widget for VFader<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let th = theme(ui);
        let (lo, hi, log) = (self.lo, self.hi, self.log);
        let unit = self.unit.clone();
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let w = 60.0;
            if !self.label.is_empty() {
                let (r, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
                ui.painter().text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    &self.label,
                    egui::FontId::proportional(11.0),
                    c(th.text_dim),
                );
            }
            let (rect, mut resp) =
                ui.allocate_exact_size(vec2(w, self.height), Sense::click_and_drag());
            let gx = rect.right() - 17.0;
            let groove = Rect::from_min_max(
                pos2(gx - 2.5, rect.top() + 7.0),
                pos2(gx + 2.5, rect.bottom() - 7.0),
            );
            let y_of = |v: f64| groove.bottom() - groove.height() * to_pos(v, lo, hi, log) as f32;
            let mut v = *self.value;
            if resp.double_clicked() {
                v = self.default;
            } else if resp.drag_started() || resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    // A press on the cap grabs it; elsewhere it jumps.
                    if (p.y - y_of(v)).abs() > 8.0 {
                        let pos = ((groove.bottom() - p.y) / groove.height()) as f64;
                        v = from_pos(pos, lo, hi, log);
                    }
                }
            } else if resp.dragged() {
                let fine = ui.input(|i| i.modifiers.shift);
                let dy = -resp.drag_delta().y as f64 * if fine { 0.15 } else { 1.0 };
                if dy != 0.0 {
                    v = from_pos(
                        to_pos(v, lo, hi, log) + dy / groove.height() as f64,
                        lo,
                        hi,
                        log,
                    );
                }
            }
            if resp.hovered() {
                v = wheel(ui, v, lo, hi, log);
            }
            if v != *self.value {
                *self.value = v;
                resp.mark_changed();
            }
            let hot = resp.hovered() || resp.dragged();
            let p = ui.painter();
            let square = th.bevel.is_some();
            for t in &self.ticks {
                let y = y_of(*t);
                p.line_segment(
                    [pos2(groove.left() - 8.0, y), pos2(groove.left() - 3.0, y)],
                    Stroke::new(1.0_f32, ca(th.text_dim, 150)),
                );
                p.text(
                    pos2(groove.left() - 10.0, y),
                    egui::Align2::RIGHT_CENTER,
                    tick_label(*t),
                    egui::FontId::proportional(9.0),
                    c(th.text_dim),
                );
            }
            let r = Rounding::same(if square { 0.0 } else { 2.0 });
            p.rect_filled(groove, r, c(th.wave_background));
            if square {
                crate::skin::bevel(p, groove.expand(1.0), &th, true);
            } else {
                p.rect_stroke(groove, r, Stroke::new(1.0_f32, ca(th.border, 220)));
            }
            let y = y_of(*self.value);
            if y < groove.bottom() - 0.5 {
                let lit = Rect::from_min_max(pos2(groove.left() + 0.5, y), groove.right_bottom());
                let mut m = egui::Mesh::default();
                let (a, b) = (c(th.accent), lerp(th.accent, th.accent_alt, 0.55));
                m.colored_vertex(lit.left_top(), a);
                m.colored_vertex(lit.right_top(), a);
                m.colored_vertex(lit.right_bottom(), b);
                m.colored_vertex(lit.left_bottom(), b);
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
            // The cap: wide and short, with a lit line across.
            let cap = Rect::from_center_size(pos2(gx, y), vec2(24.0, 11.0));
            p.rect_filled(
                cap.translate(vec2(1.0, 1.5)),
                Rounding::same(2.0),
                Color32::from_black_alpha(80),
            );
            let face = lerp(th.panel_raised, th.text, if hot { 0.30 } else { 0.18 });
            p.rect_filled(cap, r, face);
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
                [pos2(cap.left() + 3.0, y), pos2(cap.right() - 3.0, y)],
                Stroke::new(2.0_f32, c(th.accent)),
            );
            let mut bv = *self.value;
            let b = value_box(ui, resp.id.with("box"), &mut bv, lo, hi, log, w, &|x| {
                fmt_unit(x, &unit)
            });
            if b.changed() {
                *self.value = bv;
                resp.mark_changed();
            }
            resp
        })
        .inner
    }
}

// ------------------------------------------------------------ the knob

/// A large dial for the hardware-style windows: a ring of ticks with
/// numbers, a knurled skirt, a cap with a lit pointer, and the arc lit
/// from the default to the value. Its name is under it and a value box
/// under that. Drag up or down (Shift: fine), wheel, double-click for the
/// default.
#[allow(clippy::too_many_arguments)]
pub fn knob(
    ui: &mut Ui,
    id: egui::Id,
    value: &mut f64,
    lo: f64,
    hi: f64,
    default: f64,
    log: bool,
    size: f32,
    label: &str,
    marks: &[f64],
    fmt: &dyn Fn(f64) -> String,
) -> Response {
    let th = theme(ui);
    let w = size.max(66.0);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        let (rect, mut resp) = ui.allocate_exact_size(vec2(w, size), Sense::click_and_drag());
        let mut v = *value;
        if resp.double_clicked() {
            v = default;
        } else if resp.dragged() {
            let fine = ui.input(|i| i.modifiers.shift);
            let d = resp.drag_delta();
            let px = (d.x - d.y) as f64 * if fine { 0.1 } else { 1.0 };
            if px != 0.0 {
                v = from_pos(to_pos(v, lo, hi, log) + px / 220.0, lo, hi, log);
            }
        }
        if resp.hovered() {
            v = wheel(ui, v, lo, hi, log);
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if v != *value {
            *value = v;
            resp.mark_changed();
        }
        let hot = resp.hovered() || resp.dragged();
        let p = ui.painter();
        let ctr = rect.center();
        let big = size >= 80.0;
        let rad = size * if marks.is_empty() { 0.38 } else { 0.31 };
        let a0 = std::f32::consts::PI * 0.75;
        let sweep = std::f32::consts::PI * 1.5;
        let at = |f: f32, r: f32| {
            let a = a0 + sweep * f;
            ctr + vec2(a.cos(), a.sin()) * r
        };
        let n = if big { 20 } else { 10 };
        for k in 0..=n {
            let f = k as f32 / n as f32;
            let long = k % 2 == 0;
            p.line_segment(
                [at(f, rad + 3.0), at(f, rad + if long { 7.0 } else { 5.0 })],
                Stroke::new(1.0_f32, ca(th.text_dim, if long { 200 } else { 120 })),
            );
        }
        for m in marks {
            let f = to_pos(*m, lo, hi, log) as f32;
            p.text(
                at(f, rad + 14.0),
                egui::Align2::CENTER_CENTER,
                tick_label(*m),
                egui::FontId::proportional(if big { 9.5 } else { 8.5 }),
                c(th.text_dim),
            );
        }
        let f = to_pos(*value, lo, hi, log) as f32;
        let fd = to_pos(default, lo, hi, log) as f32;
        let arc = |from: f32, to: f32, r: f32| -> Vec<egui::Pos2> {
            let steps = 32;
            (0..=steps)
                .map(|k| at(from + (to - from) * k as f32 / steps as f32, r))
                .collect()
        };
        p.add(Shape::line(
            arc(0.0, 1.0, rad + 1.0),
            Stroke::new(2.0_f32, ca(th.border, 200)),
        ));
        // Lit from the default to the value.
        let (a, b) = if f >= fd { (fd, f) } else { (f, fd) };
        if b - a > 1e-3 {
            if th.dark && th.glow > 0.0 {
                p.add(Shape::line(
                    arc(a, b, rad + 1.0),
                    Stroke::new(6.0_f32, ca(th.accent, (30.0 * th.glow) as u8)),
                ));
            }
            p.add(Shape::line(
                arc(a, b, rad + 1.0),
                Stroke::new(2.5_f32, c(th.accent)),
            ));
        }
        // Skirt, knurls and cap.
        p.circle_filled(
            ctr + vec2(1.0, 2.0),
            rad - 2.0,
            Color32::from_black_alpha(90),
        );
        p.circle_filled(ctr, rad - 2.0, lerp(th.panel, th.background, 0.5));
        p.circle_stroke(ctr, rad - 2.0, Stroke::new(1.0_f32, ca(th.border, 230)));
        let cap_r = rad * 0.68;
        let grips = if big { 24 } else { 16 };
        for k in 0..grips {
            let a = std::f32::consts::TAU * k as f32 / grips as f32;
            let d = vec2(a.cos(), a.sin());
            p.line_segment(
                [ctr + d * (cap_r + 1.0), ctr + d * (rad - 4.0)],
                Stroke::new(1.0_f32, ca(th.text_dim, 60)),
            );
        }
        p.circle_filled(
            ctr,
            cap_r,
            lerp(th.panel_raised, th.text, if hot { 0.16 } else { 0.08 }),
        );
        if th.bevel.is_some() {
            p.circle_stroke(ctr, cap_r, Stroke::new(1.0_f32, ca(th.text, 90)));
        }
        p.line_segment(
            [at(f, cap_r * 0.25), at(f, rad - 3.0)],
            Stroke::new(if big { 3.0_f32 } else { 2.0 }, c(th.accent)),
        );
        if hot && th.glow > 0.0 {
            p.circle_stroke(
                ctr,
                rad + 1.0,
                Stroke::new(2.0_f32, ca(th.accent, (40.0 * th.glow) as u8)),
            );
        }
        let (lr, _) = ui.allocate_exact_size(vec2(w, 13.0), Sense::hover());
        ui.painter().text(
            lr.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(if big { 11.5 } else { 10.5 }),
            c(th.text),
        );
        let mut bv = *value;
        let bw = (w - 6.0).clamp(56.0, 86.0);
        let b = ui
            .horizontal(|ui| {
                ui.add_space((w - bw) / 2.0);
                value_box(ui, id.with("box"), &mut bv, lo, hi, log, bw, fmt)
            })
            .inner;
        if b.changed() {
            *value = bv;
            resp.mark_changed();
        }
        resp.union(b)
    })
    .inner
}

/// A row of lit buttons, one of them chosen (a stepped switch).
pub fn segmented(ui: &mut Ui, current: &mut usize, names: &[&str], tips: &[&str]) -> Response {
    let th = theme(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let mut out: Option<Response> = None;
        for (k, n) in names.iter().enumerate() {
            let on = *current == k;
            let tw = ui
                .painter()
                .layout_no_wrap(n.to_string(), egui::FontId::proportional(11.5), c(th.text))
                .size()
                .x;
            let (r, mut resp) = ui.allocate_exact_size(vec2(tw + 18.0, 22.0), Sense::click());
            let p = ui.painter();
            let fill = if on {
                ca(th.accent, 55)
            } else if resp.hovered() {
                lerp(th.panel_raised, th.text, 0.10)
            } else {
                c(th.wave_background)
            };
            p.rect_filled(r, 0.0, fill);
            if th.bevel.is_some() {
                crate::skin::bevel(p, r, &th, on);
            } else {
                p.rect_stroke(
                    r,
                    0.0,
                    Stroke::new(1.0_f32, if on { c(th.accent) } else { ca(th.border, 220) }),
                );
            }
            if on {
                p.line_segment(
                    [
                        pos2(r.left() + 4.0, r.bottom() - 2.0),
                        pos2(r.right() - 4.0, r.bottom() - 2.0),
                    ],
                    Stroke::new(2.0_f32, c(th.accent)),
                );
            }
            p.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                *n,
                egui::FontId::proportional(11.5),
                if on { c(th.accent) } else { c(th.text) },
            );
            if resp.clicked() && !on {
                *current = k;
                resp.mark_changed();
            }
            if let Some(t) = tips.get(k) {
                resp = resp.on_hover_text(*t);
            }
            out = Some(match out {
                Some(o) => o.union(resp),
                None => resp,
            });
        }
        out.unwrap_or_else(|| ui.allocate_response(vec2(0.0, 0.0), Sense::hover()))
    })
    .inner
}

/// A lit on/off switch with its name.
pub fn switch(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let th = theme(ui);
    let tw = ui
        .painter()
        .layout_no_wrap(
            label.to_string(),
            egui::FontId::proportional(11.5),
            c(th.text),
        )
        .size()
        .x;
    let (r, mut resp) = ui.allocate_exact_size(vec2(tw + 26.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let p = ui.painter();
    let lamp = Rect::from_center_size(pos2(r.left() + 8.0, r.center().y), vec2(11.0, 11.0));
    let square = th.bevel.is_some();
    let rr = Rounding::same(if square { 0.0 } else { 6.0 });
    p.rect_filled(
        lamp,
        rr,
        if *on {
            c(th.accent)
        } else {
            c(th.wave_background)
        },
    );
    if square {
        crate::skin::bevel(p, lamp, &th, true);
    } else {
        p.rect_stroke(
            lamp,
            rr,
            Stroke::new(
                1.0_f32,
                if *on {
                    c(th.accent)
                } else {
                    ca(th.border, 230)
                },
            ),
        );
        if *on && th.glow > 0.0 {
            p.rect_stroke(
                lamp.expand(2.0),
                Rounding::same(8.0),
                Stroke::new(2.0_f32, ca(th.accent, (50.0 * th.glow) as u8)),
            );
        }
    }
    p.text(
        pos2(lamp.right() + 6.0, r.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(11.5),
        if resp.hovered() {
            c(th.accent)
        } else {
            c(th.text)
        },
    );
    resp
}

/// A themed button (lit when `on`), for window footers and toolbars;
/// check `.clicked()`.
pub fn button(ui: &mut Ui, label: &str, on: bool) -> Response {
    let mut current = if on { 0 } else { 1 };
    segmented(ui, &mut current, &[label], &[])
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

    #[test]
    fn typed_values_read_units() {
        assert_eq!(parse_value("1.5k"), Some(1500.0));
        assert_eq!(parse_value(" -6 dB"), Some(-6.0));
        assert_eq!(parse_value("2,5"), Some(2.5));
        assert_eq!(parse_value("+3"), Some(3.0));
        assert_eq!(parse_value("dB"), None);
        assert_eq!(fmt_unit(1500.0, "Hz"), "1.50 kHz");
        assert_eq!(fmt_unit(350.0, "ms"), "350 ms");
    }
}

/// Whether a press at `p` belongs to this `ui`'s layer, not to a popup
/// or window drawn over it. For widgets that read the pointer directly.
pub fn press_is_here(ui: &Ui, p: egui::Pos2) -> bool {
    ui.ctx().layer_id_at(p).map_or(true, |l| l == ui.layer_id())
}
