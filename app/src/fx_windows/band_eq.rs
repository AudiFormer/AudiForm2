// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The Band EQ window, after the lead's Audiform Band EQ window: tabs
//! for 10, 20 or 30 bands (a new count keeps the curve's shape, as one
//! undo step); the response graph across the top (20 Hz to 20 kHz, 100,
//! 1k and 10k marked); then, in a dark recess, the dB scale on the left,
//! one upright slider per band with its level above and its centre
//! below, lines every 5 dB and a 0 dB line across them, and the Output
//! slider on the right with its value box. At the bottom, how to use it
//! and Flat (every band back to 0 dB).
//!
//! Drag a slider (Shift for fine steps) or turn the wheel over it;
//! double-click it for 0 dB. Alt+drag across the sliders, or drag across
//! the graph, to draw a shape: the band under the pointer takes the
//! pointer's level. Every gesture is one undo step.
//!
//! The curve is worked out with the effect's own sections
//! (`band_eq::response_db`), so what is drawn is what is heard, and only
//! again when a value changes.

use std::sync::Arc;

use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke};

use af2_fx::effects::band_eq::{self, carry, centres, count_of, COUNTS, GAIN_KEYS, RANGE};

use super::{layout, parts, Env, Fx};
use crate::graph::{self, Plot};
use crate::theme::{c, ca, lerp, Theme};

/// The graph's height and the sliders' height.
const GRAPH_H: f32 = 110.0;
const FADER_H: f32 = 180.0;
/// The Output column's width, the dB scale's, and the recess's margin.
const OUT_W: f32 = 60.0;
const SCALE_W: f32 = 26.0;
const PAD: f32 = 8.0;
/// The graph's dB range.
const GRAPH_DB: f64 = 24.0;
/// A level or frequency line's height above and below the sliders.
const LABEL_H: f32 = 16.0;

/// A column per band, narrower as the count rises.
fn col_w(n: usize) -> f32 {
    match n {
        10 => 46.0,
        20 => 30.0,
        _ => 24.0,
    }
}

/// A centre as a short label: "31.5", "125", "1k", "1.25k", "12.5k".
pub fn short_hz(f: f64, n: usize) -> String {
    let trim = |s: String| {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    };
    if f >= 1000.0 {
        let k = f / 1000.0;
        // Half-octave centres are not round numbers: one decimal is
        // enough to tell them apart.
        let digits = if n == 20 || k >= 10.0 { 1 } else { 2 };
        format!("{}k", trim(format!("{k:.digits$}")))
    } else if f < 100.0 && (f - f.round()).abs() > 0.05 {
        trim(format!("{f:.1}"))
    } else {
        format!("{f:.0}")
    }
}

fn hz_text(f: f64) -> String {
    if f >= 1000.0 {
        format!("{:.2} kHz", f / 1000.0)
    } else {
        format!("{f:.0} Hz")
    }
}

fn signed(v: f64) -> String {
    if v.abs() < 0.05 {
        "0.0".to_string()
    } else {
        format!("{v:+.1}")
    }
}

/// The band count shown and each band's gain.
fn state(fx: &Fx) -> (usize, Vec<f64>) {
    let n = count_of(fx.get("bands").round().max(0.0) as usize);
    (n, GAIN_KEYS.iter().map(|k| fx.get(k)).collect())
}

/// The curve sampled across the plot, kept until a value changes.
fn sampled(ui: &egui::Ui, fx: &Fx, plot: &Plot, rate: u32) -> Arc<Vec<(f64, f64)>> {
    let mut h: u64 = u64::from(rate) ^ (u64::from(plot.rect.width().to_bits()) << 32);
    for v in &fx.vals {
        h = h
            .wrapping_mul(0x100_0000_01b3)
            .wrapping_add(u64::from(v.to_bits()));
    }
    let id = fx.uid.with("band-eq-curve");
    if let Some((k, v)) = ui
        .ctx()
        .data(|d| d.get_temp::<(u64, Arc<Vec<(f64, f64)>>)>(id))
    {
        if k == h {
            return v;
        }
    }
    let (n, gains) = state(fx);
    let out = fx.get("output");
    let steps = (plot.rect.width() / 2.0).max(60.0) as usize;
    let v: Vec<(f64, f64)> = (0..=steps)
        .map(|i| {
            let hz = plot.hz(plot.rect.left() + plot.rect.width() * i as f32 / steps as f32);
            (hz, band_eq::response_db(n, &gains, out, rate, hz))
        })
        .collect();
    let v = Arc::new(v);
    ui.ctx().data_mut(|d| d.insert_temp(id, (h, v.clone())));
    v
}

/// The level at height `y` on a slider whose thumb travels `travel`.
fn level_at(travel: Rect, y: f32) -> f64 {
    let t = ((travel.bottom() - y) / travel.height()).clamp(0.0, 1.0);
    ((f64::from(t) * 2.0 - 1.0) * RANGE * 10.0).round() / 10.0
}

fn text_in(ui: &mut egui::Ui, w: f32, text: String, size: f32, col: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(w, LABEL_H), Sense::hover());
    ui.painter().text(
        r.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(size),
        col,
    );
}

pub fn panel(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = &env.th;
    let (n, gains) = state(fx);
    let cw = col_w(n);
    let fc = centres(n);
    let w = PAD + SCALE_W + cw * n as f32 + 9.0 + OUT_W + PAD;

    tabs(ui, fx, n, &gains);
    ui.add_space(layout::GAP_S);
    graph_view(ui, fx, env, w, n, &fc);
    ui.add_space(layout::GAP_S);
    sliders(ui, fx, th, w, n, &fc);
    ui.add_space(layout::GAP_S);

    // How to use it, and Flat.
    ui.allocate_ui_with_layout(
        vec2(w, 20.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_width(w);
            ui.label(
                RichText::new(
                    "Alt+drag across the sliders to draw a shape. Double-click a slider for 0 dB.",
                )
                .size(10.5)
                .color(c(th.text_dim)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (_, g) = state(fx);
                let flat = g.iter().take(n).all(|v| v.abs() < 1e-6);
                let r = ui.add_enabled_ui(!flat, |ui| crate::widgets::button(ui, "Flat", false));
                if r.inner.on_hover_text("Every band back to 0 dB").clicked() {
                    for k in GAIN_KEYS {
                        fx.set(k, 0.0);
                    }
                }
            });
        },
    );
}

/// The band count tabs: a new count keeps the curve's shape.
fn tabs(ui: &mut egui::Ui, fx: &mut Fx, n: usize, gains: &[f64]) {
    let mut pick = COUNTS.iter().position(|&k| k == n).unwrap_or(0);
    let tips = [
        "Ten bands an octave apart",
        "Twenty bands a half-octave apart",
        "Thirty bands a third-octave apart",
    ];
    let names = ["10 Bands", "20 Bands", "30 Bands"];
    if crate::widgets::segmented(ui, &mut pick, &names, &tips).changed() && COUNTS[pick] != n {
        let next = carry(n, gains, COUNTS[pick]);
        fx.set("bands", pick as f64);
        for (k, v) in GAIN_KEYS.iter().zip(next) {
            fx.set(k, v);
        }
    }
}

/// The response graph: 20 Hz to 20 kHz evenly in pitch, ±24 dB, a line
/// at each frequency of each decade (100, 1k and 10k stronger and
/// labelled). Dragging across it draws a shape.
fn graph_view(ui: &mut egui::Ui, fx: &mut Fx, env: &Env, w: f32, n: usize, fc: &[f64]) {
    let th = &env.th;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, GRAPH_H + 13.0), Sense::click_and_drag());
    let plot = Plot {
        rect: Rect::from_min_size(rect.min, vec2(w, GRAPH_H)),
        f_lo: 20.0,
        f_hi: 20_000.0,
        linear: false,
        y_lo: -GRAPH_DB,
        y_hi: GRAPH_DB,
    };
    let nearest = |x: f32| -> usize {
        let hz = plot.hz(x);
        (0..n)
            .min_by(|&a, &b| (fc[a] / hz).ln().abs().total_cmp(&(fc[b] / hz).ln().abs()))
            .unwrap_or(0)
    };
    if resp.dragged() || resp.drag_started() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let v = (plot.value(pos.y).clamp(-RANGE, RANGE) * 10.0).round() / 10.0;
            fx.set_live(GAIN_KEYS[nearest(pos.x)], v);
        }
    }
    if resp.drag_stopped() {
        fx.released = true;
    }
    let hot = resp.hover_pos().map(|p| nearest(p.x));
    let p = ui.painter_at(rect.expand(2.0));
    let r = plot.rect;
    graph::backdrop(&p, r, th);
    let faint = Stroke::new(1.0_f32, ca(th.grid, 55));
    let strong = Stroke::new(1.0_f32, ca(th.grid, 150));
    for decade in [10.0, 100.0, 1000.0, 10_000.0] {
        for k in 1..=9 {
            let f = decade * f64::from(k);
            if !(20.0..=20_000.0).contains(&f) {
                continue;
            }
            let x = plot.x(f);
            let major = k == 1 && decade >= 100.0;
            p.line_segment(
                [pos2(x, r.top()), pos2(x, r.bottom())],
                if major { strong } else { faint },
            );
            if major {
                let text = if f >= 1000.0 {
                    format!("{}k", f / 1000.0)
                } else {
                    format!("{f}")
                };
                p.text(
                    pos2(x, r.bottom() + 1.0),
                    Align2::CENTER_TOP,
                    text,
                    FontId::proportional(9.0),
                    c(th.text_dim),
                );
            }
        }
    }
    for db in [-12.0, 12.0] {
        let y = plot.y(db);
        p.line_segment([pos2(r.left(), y), pos2(r.right(), y)], faint);
    }
    let zero = plot.y(0.0);
    p.line_segment([pos2(r.left(), zero), pos2(r.right(), zero)], strong);
    let curve = sampled(ui, fx, &plot, env.rate);
    let line: Vec<Pos2> = curve
        .iter()
        .map(|(hz, db)| pos2(plot.x(*hz), plot.y(*db)))
        .collect();
    p.add(Shape::line(line, Stroke::new(1.5_f32, c(th.accent))));
    if let Some(i) = hot {
        let (_, g) = state(fx);
        let at = pos2(plot.x(fc[i]), plot.y(g[i]));
        p.circle_filled(at, 3.5, c(th.accent));
        let text = format!("{} · {} dB", hz_text(fc[i]), signed(g[i]));
        graph::tag(&p, at, &text, th, r);
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
}

/// The recess with the dB scale, a slider per band and Output.
fn sliders(ui: &mut egui::Ui, fx: &mut Fx, th: &Theme, w: f32, n: usize, fc: &[f64]) {
    let cw = col_w(n);
    let frame = Rect::from_min_size(
        ui.cursor().min,
        vec2(w, PAD + LABEL_H + FADER_H + LABEL_H + PAD),
    );
    // Painted behind the sliders once their place is known.
    let behind = ui.painter().add(Shape::Noop);
    let mut track: Option<Rect> = None;
    let lit = ca(th.accent, 160);
    let size = if n >= 30 { 8.5 } else { 9.5 };
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(frame.shrink(PAD)), |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.add_space(SCALE_W);
            for (i, key) in GAIN_KEYS.iter().enumerate().take(n) {
                ui.vertical(|ui| {
                    ui.set_width(cw);
                    let mut v = fx.get(key);
                    let tone = if v.abs() < 0.05 { th.text_dim } else { th.text };
                    text_in(ui, cw, signed(v), size, c(tone));
                    let fw = (cw - 6.0).min(18.0);
                    let r = ui
                        .horizontal(|ui| {
                            ui.add_space((cw - fw) / 2.0);
                            parts::vfader(ui, th, &mut v, -RANGE, RANGE, 0.0, vec2(fw, FADER_H), lit)
                        })
                        .inner;
                    track.get_or_insert(r.rect);
                    if r.changed() {
                        fx.set_live(key, (v * 10.0).round() / 10.0);
                    }
                    fx.track(&r);
                    r.on_hover_text(format!(
                        "{}: {} dB. Drag (Shift: fine) or wheel; double-click for 0 dB.",
                        hz_text(fc[i]),
                        signed(v)
                    ));
                    text_in(ui, cw, short_hz(fc[i], n), size, c(th.text_dim));
                });
            }
            ui.add_space(9.0);
            ui.vertical(|ui| {
                ui.set_width(OUT_W);
                text_in(ui, OUT_W, "Output".into(), 9.5, c(th.text_dim));
                let mut v = fx.get("output");
                let r = ui
                    .horizontal(|ui| {
                        ui.add_space((OUT_W - 18.0) / 2.0);
                        out_fader(ui, th, &mut v, vec2(18.0, FADER_H - 4.0))
                    })
                    .inner;
                if r.changed() {
                    fx.set_live("output", (v * 10.0).round() / 10.0);
                }
                fx.track(&r);
                r.on_hover_text("Output level. Double-click for 0 dB.");
                ui.add_space(2.0);
                let mut o = fx.get("output");
                let r = crate::widgets::value_box(
                    ui,
                    fx.uid.with("band-eq-out"),
                    &mut o,
                    -RANGE,
                    RANGE,
                    false,
                    OUT_W,
                    &|v| format!("{v:.1} dB"),
                );
                if r.changed() {
                    fx.set_live("output", (o * 10.0).round() / 10.0);
                }
                fx.track(&r);
            });
        });
    });
    ui.allocate_rect(frame, Sense::hover());
    let Some(t) = track else { return };
    let travel = t.shrink2(vec2(0.0, 5.0));
    let left = frame.left() + PAD + SCALE_W;
    let right = left + cw * n as f32;

    // Behind: the recess, the lines every 5 dB, the scale, the divider.
    let painter = ui.painter();
    let mut shapes = vec![
        Shape::rect_filled(frame, 3.0, Color32::from_black_alpha(if th.dark { 77 } else { 22 })),
        Shape::rect_stroke(frame, 3.0, Stroke::new(1.0_f32, ca(th.border, 160))),
    ];
    for d in [15, 10, 5, 0, -5, -10, -15] {
        let y = travel.bottom() - travel.height() * ((f64::from(d) + RANGE) / (2.0 * RANGE)) as f32;
        let stroke = if d == 0 {
            Stroke::new(1.0_f32, ca(th.text, 52))
        } else {
            Stroke::new(1.0_f32, ca(th.text, 15))
        };
        shapes.push(Shape::line_segment([pos2(left, y), pos2(right, y)], stroke));
        let text = if d > 0 { format!("+{d}") } else { format!("{d}") };
        let g = painter.layout_no_wrap(text, FontId::proportional(9.0), c(th.text_dim));
        let at = pos2(left - 4.0 - g.size().x, y - g.size().y / 2.0);
        shapes.push(Shape::galley(at, g, c(th.text_dim)));
    }
    shapes.push(Shape::line_segment(
        [
            pos2(right + 4.5, frame.top() + PAD),
            pos2(right + 4.5, frame.bottom() - PAD),
        ],
        Stroke::new(1.0_f32, ca(th.border, 160)),
    ));
    painter.set(behind, Shape::Vec(shapes));

    // Alt+drag across the sliders draws a shape.
    let area = Rect::from_x_y_ranges(left..=right, t.y_range());
    let (alt, down, pos) = ui.input(|i| {
        (
            i.modifiers.alt,
            i.pointer.primary_down(),
            i.pointer.interact_pos(),
        )
    });
    let drawing_id = fx.uid.with("band-eq-drawing");
    let mut drawing: bool = ui.ctx().data(|d| d.get_temp(drawing_id)).unwrap_or(false);
    if let Some(pos) = pos {
        if alt && down && (drawing || area.contains(pos)) {
            drawing = true;
            let i = (((pos.x - left) / cw).floor().max(0.0) as usize).min(n - 1);
            fx.set_live(GAIN_KEYS[i], level_at(travel, pos.y));
        }
    }
    if drawing && !down {
        drawing = false;
        fx.released = true;
    }
    ui.ctx().data_mut(|d| d.insert_temp(drawing_id, drawing));
}

/// The Output slider: lit from the bottom up to the thumb. Drag (Shift:
/// fine), wheel, double-click for 0 dB.
fn out_fader(ui: &mut egui::Ui, th: &Theme, value: &mut f64, size: egui::Vec2) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let span = 2.0 * RANGE;
    let mut v = *value;
    if resp.double_clicked() {
        v = 0.0;
    } else if resp.dragged() {
        let fine = if ui.input(|i| i.modifiers.shift) { 0.1 } else { 1.0 };
        v -= f64::from(resp.drag_delta().y) * fine / f64::from(rect.height() - 10.0) * span;
    }
    if resp.hovered() {
        let wheel = ui.input(|i| i.smooth_scroll_delta.y);
        if wheel != 0.0 {
            v += f64::from(wheel.signum()) * 0.5;
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let v = v.clamp(-RANGE, RANGE);
    if v != *value {
        *value = v;
        resp.mark_changed();
    }
    let p = ui.painter();
    let travel = rect.shrink2(vec2(0.0, 5.0));
    let y = travel.bottom() - travel.height() * ((*value + RANGE) / span) as f32;
    let groove = Rect::from_center_size(rect.center(), vec2(6.0, travel.height() + 4.0));
    p.rect_filled(groove, 3.0, c(th.wave_background));
    p.rect_filled(
        Rect::from_min_max(
            pos2(groove.left() + 1.0, y),
            pos2(groove.right() - 1.0, groove.bottom() - 1.0),
        ),
        2.0,
        ca(th.accent, 190),
    );
    p.rect_stroke(groove, 3.0, Stroke::new(1.0_f32, ca(th.border, 200)));
    let hot = resp.hovered() || resp.dragged();
    let cap = Rect::from_center_size(pos2(rect.center().x, y), vec2(size.x, 10.0));
    p.rect_filled(cap.translate(vec2(0.0, 1.5)), 2.0, Color32::from_black_alpha(80));
    p.rect_filled(
        cap,
        2.0,
        lerp(th.panel_raised, th.text, if hot { 0.85 } else { 0.7 }),
    );
    p.line_segment(
        [
            pos2(cap.left() + 3.0, cap.center().y),
            pos2(cap.right() - 3.0, cap.center().y),
        ],
        Stroke::new(1.0_f32, Color32::from_black_alpha(140)),
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_labels_are_short() {
        assert_eq!(short_hz(31.5, 10), "31.5");
        assert_eq!(short_hz(125.0, 10), "125");
        assert_eq!(short_hz(1000.0, 10), "1k");
        assert_eq!(short_hz(1250.0, 30), "1.25k");
        assert_eq!(short_hz(12_500.0, 30), "12.5k");
        assert_eq!(short_hz(16_000.0, 10), "16k");
        // Half-octave centres: one decimal.
        let h = centres(20);
        assert_eq!(short_hz(h[10], 20), "1k");
        assert_eq!(short_hz(h[11], 20), "1.4k");
        assert_eq!(short_hz(h[1], 20), "44.5");
    }

    #[test]
    fn a_slider_height_reads_as_a_level() {
        let travel = Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 100.0));
        assert_eq!(level_at(travel, 0.0), RANGE);
        assert_eq!(level_at(travel, 100.0), -RANGE);
        assert_eq!(level_at(travel, 50.0), 0.0);
        assert_eq!(level_at(travel, -40.0), RANGE);
    }
}
