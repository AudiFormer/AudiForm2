// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.
// The Notch Filter window follows the project lead's Audiform window
// (src/panels/voice-chain/NotchGraph.svelte, created clean in ba16c79),
// rebuilt here in Rust/egui. The response graph over the Curve EQ,
// Precision Filter and Hum Cut controls recreates the behaviour of
// Audiform's ResponseGraph.svelte and graph.ts, which carry the GeekCast
// Radio Network / Altered Audio Editor copyright line: nothing was copied
// from those two files, only what the graph shows and does was written
// fresh. See docs/clean-room.md.

//! The filters' faces.
//!
//! The Curve EQ, Precision Filter and Hum Cut: their response curve, 20 Hz
//! to 20 kHz evenly in pitch, over the dial columns. The level range is
//! ±24 dB, opened out to fit the effect's gain controls and deep cuts.
//! The Curve EQ's eight points sit on the curve: drag one left and right
//! for its frequency, up and down for its gain.
//!
//! The Notch Filter: its response with a numbered marker per notch. Drag
//! a marker left and right for its frequency, up and down for its depth;
//! double-click it to switch that notch on or off. Below, a column per
//! notch (frequency, depth and its switch), then the width all the
//! notches share. Every drag is one undo step.
//!
//! The curves are worked out from the effects' own sections
//! (`af2_fx::effects::filters::response_db`), so what is drawn is what is
//! heard, and only again when a value changes.

use std::sync::Arc;

use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Rounding, Sense, Shape, Stroke};

use af2_fx::effects::curve_eq::{FREQ_KEYS, GAIN_KEYS, GROUPS};
use af2_fx::effects::filters::{notch_keys, response_db};

use super::{columns, layout, Env, Fx};
use crate::theme::{c, ca, lerp};
use crate::widgets;

const LOW: f64 = 20.0;
const HIGH: f64 = 20_000.0;

/// 0 to 1 across a graph.
fn x_of(hz: f64) -> f32 {
    ((hz.clamp(LOW, HIGH) / LOW).ln() / (HIGH / LOW).ln()) as f32
}

fn hz_at(x: f32) -> f64 {
    LOW * (HIGH / LOW).powf(f64::from(x.clamp(0.0, 1.0)))
}

/// "940 Hz", "1.25 kHz", "12.5 kHz".
pub fn hz_text(hz: f64) -> String {
    if hz >= 1000.0 {
        let k = hz / 1000.0;
        let s = if hz >= 10_000.0 { format!("{k:.1}") } else { format!("{k:.2}") };
        let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        format!("{s} kHz")
    } else {
        format!("{} Hz", hz.round())
    }
}

/// The effect's response at `n` frequencies evenly in pitch, kept until a
/// value changes.
fn sampled(ui: &egui::Ui, fx: &Fx, env: &Env, n: usize) -> Arc<Vec<(f64, f64)>> {
    let mut h: u64 = n as u64 ^ u64::from(env.rate);
    for v in &fx.vals {
        h = h.wrapping_mul(0x100_0000_01b3).wrapping_add(u64::from(v.to_bits()));
    }
    let id = fx.uid.with("filter-response");
    if let Some((k, v)) = ui.ctx().data(|d| d.get_temp::<(u64, Arc<Vec<(f64, f64)>>)>(id)) {
        if k == h {
            return v;
        }
    }
    let saved: Vec<(String, f64)> = fx
        .ctls
        .iter()
        .zip(&fx.vals)
        .map(|(c, v)| (c.key.clone(), f64::from(*v)))
        .collect();
    let rate = env.rate.max(8_000);
    // Evenly in pitch, and on each notch's own frequency so a narrow one
    // shows its full depth.
    let mut freqs: Vec<f64> = (0..n)
        .map(|i| LOW * (HIGH / LOW).powf(i as f64 / (n - 1) as f64))
        .collect();
    freqs.extend(centres(fx));
    freqs.sort_by(f64::total_cmp);
    let v: Vec<(f64, f64)> = match af2_fx::params_from(&fx.id, &saved) {
        Some(p) => freqs
            .into_iter()
            .map(|hz| (hz, response_db(&fx.id, &p, rate, hz).unwrap_or(0.0).clamp(-120.0, 40.0)))
            .collect(),
        None => Vec::new(),
    };
    let v = Arc::new(v);
    ui.ctx().data_mut(|d| d.insert_temp(id, (h, v.clone())));
    v
}

/// The frequencies of the notches the effect cuts, if any.
fn centres(fx: &Fx) -> Vec<f64> {
    if fx.id == af2_fx::effects::filters::HUM {
        let base = if fx.get("mains").round() as i64 == 1 { 50.0 } else { 60.0 };
        let n = fx.get("harmonics").round().max(1.0) as usize;
        (1..=n).map(|k| base * k as f64).filter(|f| *f <= HIGH).collect()
    } else if fx.id == af2_fx::effects::filters::NOTCH {
        notch_keys().filter(|(_, k)| fx.on(k[0])).map(|(_, k)| fx.get(k[1])).collect()
    } else {
        Vec::new()
    }
}

// ------------------------------------------------------------ response graph

/// The level range shown: ±24 dB, opened out to the effect's dB controls
/// and to the curve's deep cuts (to -60), the bottom on a 6 dB line.
fn range(fx: &Fx, curve: &[(f64, f64)]) -> (f64, f64) {
    let (mut top, mut bottom) = (24.0f64, -24.0f64);
    for c in fx.ctls.iter().filter(|c| c.unit == "dB" && !c.hidden) {
        top = top.max(c.max.min(40.0));
        bottom = bottom.min(c.min.max(-60.0));
    }
    for (_, db) in curve {
        bottom = bottom.min(db.max(-60.0));
    }
    (top, (bottom / 6.0).floor() * 6.0)
}

/// The response graph, `w` wide, with the Curve EQ's points to drag.
fn response(ui: &mut egui::Ui, fx: &mut Fx, env: &Env, w: f32) {
    let th = &env.th;
    let h = 110.0;
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let curve = sampled(ui, fx, env, 160);
    let (top, bottom) = range(fx, &curve);
    let x = |hz: f64| rect.left() + x_of(hz) * rect.width();
    let y = |db: f64| rect.top() + (((top - db) / (top - bottom)).clamp(0.0, 1.0) as f32) * rect.height();
    // The Curve EQ's points: (index, Hz, dB).
    let curve_eq = fx.id == af2_fx::effects::curve_eq::ID;
    let dots: Vec<(usize, f64, f64)> = if curve_eq {
        (0..FREQ_KEYS.len())
            .map(|i| (i, fx.get(FREQ_KEYS[i]), fx.get(GAIN_KEYS[i])))
            .collect()
    } else {
        Vec::new()
    };
    let active_id = fx.uid.with("curve-active");
    let mut active: Option<usize> = ui.ctx().data(|d| d.get_temp(active_id));
    let mut hover = None;
    for (i, f, g) in &dots {
        let at = pos2(x(*f), y(*g));
        let r = ui.interact(
            Rect::from_center_size(at, vec2(12.0, 12.0)),
            fx.uid.with(("curve-dot", *i)),
            Sense::drag(),
        );
        if r.hovered() {
            hover = Some(*i);
        }
        if r.drag_started() {
            active = Some(*i);
        }
        if r.dragged() {
            if let Some(p) = r.interact_pointer_pos() {
                let hz = hz_at((p.x - rect.left()) / rect.width()).round();
                let db = top - f64::from(((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0)) * (top - bottom);
                fx.set_live(FREQ_KEYS[*i], hz);
                fx.set_live(GAIN_KEYS[*i], (db * 10.0).round() / 10.0);
            }
        }
        if r.drag_stopped() {
            fx.released = true;
            active = None;
        }
    }
    ui.ctx().data_mut(|d| match active {
        Some(a) => d.insert_temp(active_id, a),
        None => d.remove::<usize>(active_id),
    });
    // Drawing, with the values as they are now.
    let p = ui.painter_at(rect.expand(1.0));
    crate::graph::backdrop(&p, rect, th);
    let line = Stroke::new(1.0_f32, ca(th.grid, 90));
    for f in [100.0, 1000.0, 10_000.0] {
        p.line_segment([pos2(x(f), rect.top()), pos2(x(f), rect.bottom())], line);
    }
    let mut levels = vec![top, 12.0, 0.0, -12.0, bottom];
    levels.dedup();
    for d in levels {
        if d <= top && d >= bottom {
            let s = if d == 0.0 { Stroke::new(1.0_f32, ca(th.grid, 190)) } else { line };
            p.line_segment([pos2(rect.left(), y(d)), pos2(rect.right(), y(d))], s);
        }
    }
    let curve = sampled(ui, fx, env, 160);
    let pts: Vec<Pos2> = curve.iter().map(|(f, d)| pos2(x(*f), y(*d))).collect();
    if pts.len() > 1 {
        p.add(Shape::line(pts, Stroke::new(1.5_f32, c(th.accent))));
    }
    let hot = active.or(hover);
    for (i, _, _) in &dots {
        let at = pos2(x(fx.get(FREQ_KEYS[*i])), y(fx.get(GAIN_KEYS[*i])));
        let lit = active == Some(*i);
        p.circle(
            at,
            4.0,
            if lit { c(th.accent) } else { c(th.panel_raised) },
            Stroke::new(1.5_f32, c(th.accent)),
        );
    }
    if let Some(i) = hot {
        let (f, g) = (fx.get(FREQ_KEYS[i]), fx.get(GAIN_KEYS[i]));
        let text = format!("{} · {} · {g:+.1} dB", GROUPS[i], hz_text(f));
        crate::graph::tag(&p, pos2(x(f), y(g)), &text, th, rect);
        ui.ctx().set_cursor_icon(if active.is_some() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        });
    }
    // The scale under it.
    let (srect, _) = ui.allocate_exact_size(vec2(w, 12.0), Sense::hover());
    let font = FontId::proportional(10.0);
    let dim = lerp(th.panel, th.text_dim, 0.8);
    for (f, t, a) in [
        (100.0, "100", Align2::CENTER_TOP),
        (1000.0, "1k", Align2::CENTER_TOP),
        (10_000.0, "10k", Align2::CENTER_TOP),
    ] {
        ui.painter()
            .text(pos2(srect.left() + x_of(f) * w, srect.top()), a, t, font.clone(), dim);
    }
}

/// The response graph over the dial columns, as wide as they are.
fn graphed(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let wid = fx.uid.with("graph-w");
    let w: f32 = ui.ctx().data(|d| d.get_temp(wid)).unwrap_or(layout::MIN_W);
    ui.vertical(|ui| {
        response(ui, fx, env, w);
        ui.add_space(layout::GAP_S);
        let r = ui.vertical(|ui| columns(ui, fx, env)).response.rect;
        if (r.width() - w).abs() > 0.5 {
            ui.ctx().data_mut(|d| d.insert_temp(wid, r.width()));
            ui.ctx().request_repaint();
        }
    });
}

pub fn curve_eq(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    graphed(ui, fx, env);
}

pub fn precision(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    graphed(ui, fx, env);
}

pub fn hum(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    graphed(ui, fx, env);
}

// ------------------------------------------------------------ Notch Filter

/// The frequency ruler's lines: every 1 to 9 step of each decade, with a
/// label where there is room; 100 Hz, 1 kHz and 10 kHz are major.
fn freq_ticks(width: f32) -> Vec<(f64, Option<String>, bool)> {
    let mut out = Vec::new();
    let mut last = f32::NEG_INFINITY;
    for decade in [10.0, 100.0, 1000.0, 10_000.0] {
        for k in 1..=9 {
            let f = decade * f64::from(k);
            if !(LOW..=HIGH).contains(&f) {
                continue;
            }
            let x = x_of(f) * width;
            let major = k == 1 && decade >= 100.0;
            let text = if f == LOW {
                "Hz".to_string()
            } else if f >= 1000.0 {
                format!("{}k", f / 1000.0)
            } else {
                format!("{f}")
            };
            let ends = f == LOW || f == HIGH;
            let room = if major || ends { 0.0 } else { 20.0 + text.len() as f32 * 2.0 };
            let fits = x <= width - 14.0 + if f == HIGH { 14.0 } else { 0.0 };
            let label = (x - last >= room && fits).then_some(text);
            if label.is_some() {
                last = x;
            }
            out.push((f, label, major));
        }
    }
    out
}

/// A value box for control `key`.
fn vbox(ui: &mut egui::Ui, fx: &mut Fx, key: &str, width: f32) {
    let Some(k) = fx.k(key) else { return };
    let ctl = fx.ctls[k].clone();
    let mut v = f64::from(fx.vals[k]);
    let unit = ctl.unit.clone();
    let r = widgets::value_box(
        ui,
        fx.uid.with(("vbox", k)),
        &mut v,
        ctl.min,
        ctl.max,
        ctl.log,
        width,
        &|v| {
            if unit == "Hz" {
                hz_text(v)
            } else {
                widgets::fmt_unit(v, &unit)
            }
        },
    );
    if r.changed() {
        fx.vals[k] = v as f32;
    }
    fx.track(&r);
}

pub fn notch(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = &env.th;
    let w = 520.0;
    let h = 220.0;
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let (top, bottom) = (6.0f64, -66.0f64);
    let plot = Rect::from_min_max(rect.min + vec2(6.0, 10.0), rect.max - vec2(34.0, 18.0));
    let x = |hz: f64| plot.left() + x_of(hz) * plot.width();
    let y = |db: f64| plot.top() + (((top - db.clamp(bottom, top)) / (top - bottom)) as f32) * plot.height();
    let notches: Vec<(usize, &str, [&str; 3])> =
        notch_keys().enumerate().map(|(i, (g, k))| (i, g, k)).collect();
    let at = |fx: &Fx, k: &[&str; 3]| {
        let on = fx.on(k[0]);
        pos2(x(fx.get(k[1])), if on { y(fx.get(k[2])) } else { y(0.0) })
    };
    let active_id = fx.uid.with("notch-active");
    let mut active: Option<usize> = ui.ctx().data(|d| d.get_temp(active_id));
    let mut hover = None;
    for (i, _, k) in &notches {
        let r = ui.interact(
            Rect::from_center_size(at(fx, k), vec2(20.0, 20.0)),
            fx.uid.with(("notch", *i)),
            Sense::click_and_drag(),
        );
        if r.hovered() {
            hover = Some(*i);
        }
        if r.double_clicked() {
            let on = fx.on(k[0]);
            fx.set(k[0], if on { 0.0 } else { 1.0 });
        }
        if r.drag_started() {
            active = Some(*i);
        }
        if r.dragged() {
            if let Some(p) = r.interact_pointer_pos() {
                let hz = hz_at((p.x - plot.left()) / plot.width()).round();
                let db = top - f64::from((p.y - plot.top()) / plot.height()) * (top - bottom);
                fx.set_live(k[1], hz);
                fx.set_live(k[2], (db * 2.0).round() / 2.0);
            }
        }
        if r.drag_stopped() {
            fx.released = true;
            active = None;
        }
    }
    ui.ctx().data_mut(|d| match active {
        Some(a) => d.insert_temp(active_id, a),
        None => d.remove::<usize>(active_id),
    });
    // Drawing.
    let p = ui.painter_at(rect);
    crate::graph::backdrop(&p, plot, th);
    let faint = Stroke::new(1.0_f32, ca(th.grid, 40));
    let major = Stroke::new(1.0_f32, ca(th.grid, 90));
    let zero = Stroke::new(1.0_f32, ca(th.grid, 190));
    let font = FontId::proportional(9.0);
    let dim = c(th.text_dim);
    let faint_text = lerp(th.panel, th.text_dim, 0.75);
    for (f, label, strong) in freq_ticks(plot.width()) {
        p.line_segment([pos2(x(f), plot.top()), pos2(x(f), plot.bottom())], if strong { major } else { faint });
        if let Some(t) = label {
            let align = if f == LOW {
                Align2::LEFT_BOTTOM
            } else if f == HIGH {
                Align2::RIGHT_BOTTOM
            } else {
                Align2::CENTER_BOTTOM
            };
            p.text(pos2(x(f), rect.bottom() - 3.0), align, t, font.clone(), if strong { dim } else { faint_text });
        }
    }
    for d in [0.0, -12.0, -24.0, -36.0, -48.0, -60.0] {
        p.line_segment([pos2(plot.left(), y(d)), pos2(plot.right(), y(d))], if d == 0.0 { zero } else { faint });
        p.text(pos2(plot.right() + 4.0, y(d)), Align2::LEFT_CENTER, format!("{d}"), font.clone(), faint_text);
    }
    p.text(pos2(plot.right() + 4.0, rect.bottom() - 3.0), Align2::LEFT_BOTTOM, "dB", font.clone(), faint_text);
    let curve = sampled(ui, fx, env, 400);
    let line: Vec<Pos2> = curve.iter().map(|(f, d)| pos2(x(*f), y(*d))).collect();
    if line.len() > 1 {
        p.add(Shape::line(line, Stroke::new(2.0_f32, c(th.accent))));
    }
    let hot = active.or(hover);
    for (i, _, k) in &notches {
        let on = fx.on(k[0]);
        let q = at(fx, k);
        if on {
            p.add(Shape::dashed_line(
                &[pos2(q.x, y(0.0)), q],
                Stroke::new(1.0_f32, ca(th.text, 50)),
                2.0,
                2.0,
            ));
        }
        let sq = Rect::from_center_size(q, vec2(10.0, 10.0));
        if on {
            let fill = if active == Some(*i) { c(th.accent) } else { lerp(th.panel_raised, th.text, 0.95) };
            p.rect_filled(sq, Rounding::ZERO, fill);
            p.rect_stroke(sq, Rounding::ZERO, Stroke::new(1.0_f32, Color32::from_black_alpha(200)));
        } else {
            p.rect_stroke(sq, Rounding::ZERO, Stroke::new(1.0_f32, ca(th.text, 150)));
        }
        p.text(q + vec2(0.0, 7.0), Align2::CENTER_TOP, format!("{}", i + 1), FontId::proportional(10.0), c(th.text));
    }
    if let Some((i, g, k)) = hot.and_then(|h| notches.iter().find(|n| n.0 == h)) {
        let mut parts = vec![g.to_string(), hz_text(fx.get(k[1])), format!("{:.1} dB", fx.get(k[2]))];
        if !fx.on(k[0]) {
            parts.push("off".into());
        }
        crate::graph::tag(&p, at(fx, k), &parts.join(" · "), th, rect);
        let _ = i;
        ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
    }
    // The table: a column per notch.
    ui.add_space(layout::GAP_S);
    let head = 74.0;
    let gap = 2.0;
    let col = (w - head - gap * notches.len() as f32) / notches.len() as f32;
    let rowhead = |ui: &mut egui::Ui, t: &str| {
        ui.allocate_ui_with_layout(vec2(head, 22.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(6.0);
            ui.label(RichText::new(t).size(11.0).color(c(th.text_dim)));
        });
    };
    for row in 0..3 {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(gap, 4.0);
            rowhead(ui, ["Frequency", "Depth", "Notch"][row]);
            for (i, g, k) in &notches {
                let on = fx.on(k[0]);
                ui.allocate_ui_with_layout(vec2(col, 22.0), egui::Layout::centered_and_justified(egui::Direction::LeftToRight), |ui| {
                    match row {
                        0 | 1 => {
                            if !on {
                                ui.multiply_opacity(0.45);
                            }
                            vbox(ui, fx, k[row + 1], col);
                        }
                        _ => {
                            ui.vertical_centered(|ui| {
                                let r = widgets::button(ui, &format!("{}", i + 1), on)
                                    .on_hover_text(format!("{g}: {}", if on { "on" } else { "off" }));
                                if r.clicked() {
                                    fx.set(k[0], if on { 0.0 } else { 1.0 });
                                }
                            });
                        }
                    }
                });
            }
        });
    }
    // The width all of them share.
    ui.add_space(layout::GAP_S / 2.0);
    let (sep, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter().line_segment([sep.left_center(), sep.right_center()], Stroke::new(1.0_f32, ca(th.border, 230)));
    ui.add_space(6.0);
    layout::centred(ui, w, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(RichText::new("Width (Q), all notches:").size(11.0).color(c(th.text_dim)));
            vbox(ui, fx, "width", 80.0);
            ui.label(RichText::new("Higher is narrower.").size(11.0).color(c(th.text_dim)));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hz_reads_as_audiform_wrote_it() {
        assert_eq!(hz_text(940.4), "940 Hz");
        assert_eq!(hz_text(1000.0), "1 kHz");
        assert_eq!(hz_text(1250.0), "1.25 kHz");
        assert_eq!(hz_text(12_500.0), "12.5 kHz");
    }

    #[test]
    fn the_ruler_labels_the_round_frequencies() {
        let t = freq_ticks(480.0);
        assert_eq!(t.len(), 28);
        let labelled: Vec<&str> = t.iter().filter_map(|(_, l, _)| l.as_deref()).collect();
        for want in ["Hz", "100", "1k", "10k", "20k"] {
            assert!(labelled.contains(&want), "{want} in {labelled:?}");
        }
        assert_eq!(t.iter().filter(|x| x.2).count(), 3);
    }
}
