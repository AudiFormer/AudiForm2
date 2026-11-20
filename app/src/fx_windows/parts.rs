// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Pieces the effect faces share: the faceplate and its modules, the
//! needle meter, level bars, the dynamics transfer graph and the
//! gain-reduction estimate. All from the theme.

use egui::{
    pos2, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Rounding, Sense, Shape, Stroke,
};

use super::{layout, Env};
use crate::theme::{c, ca, lerp, Theme};

/// The unit's faceplate: a raised plate with a title strip, `w` wide
/// inside its margin (the face lays its rows out across `w`).
pub fn faceplate(
    ui: &mut egui::Ui,
    th: &Theme,
    title: &str,
    sub: &str,
    w: f32,
    add: impl FnOnce(&mut egui::Ui),
) {
    let square = th.bevel.is_some();
    egui::Frame::none()
        .fill(lerp(th.panel_raised, th.background, 0.35))
        .stroke(Stroke::new(1.0_f32, ca(th.border, 220)))
        .rounding(if square { 0.0 } else { 6.0 })
        .inner_margin(egui::Margin::same(layout::GAP))
        .show(ui, |ui| {
            ui.set_width(w);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(title.to_uppercase())
                        .size(13.0)
                        .strong()
                        .extra_letter_spacing(3.0)
                        .color(c(th.accent)),
                );
                if !sub.is_empty() {
                    ui.label(
                        RichText::new(sub)
                            .size(10.5)
                            .italics()
                            .color(c(th.text_dim)),
                    );
                }
            });
            let x = ui.cursor().left();
            let y = ui.cursor().top();
            ui.painter().line_segment(
                [pos2(x, y), pos2(x + w, y)],
                Stroke::new(1.0_f32, ca(th.accent, 70)),
            );
            ui.add_space(layout::GAP_S);
            ui.vertical(add);
        });
}

/// A value eased toward `target` like a needle with some weight (about
/// 300 ms to settle), kept between frames under `id`. Asks for frames
/// while it moves.
pub fn ballistic(ui: &egui::Ui, id: egui::Id, target: f64, tau: f64) -> f64 {
    let now = ui.input(|i| i.time);
    let (mut v, last): (f64, f64) = ui.ctx().data(|d| d.get_temp(id)).unwrap_or((target, now));
    let dt = (now - last).clamp(0.0, 0.1);
    v += (target - v) * (1.0 - (-dt / tau.max(1e-3)).exp());
    ui.ctx().data_mut(|d| d.insert_temp(id, (v, now)));
    if (target - v).abs() > 0.05 {
        ui.ctx().request_repaint();
    }
    v
}

/// Where on a needle scale (0..1) a reading in dB falls: spread by
/// amplitude, so the top of the scale gets more room.
fn needle_pos(db: f64, lo: f64, hi: f64) -> f64 {
    let g = |d: f64| 10f64.powf(d.clamp(lo, hi) / 20.0);
    (g(db) - g(lo)) / (g(hi) - g(lo))
}

/// A needle meter: a lit face, the scale from -20 to +3 (red above 0) and
/// a smooth needle. `reading` is dB against the meter's 0 mark.
pub fn needle(
    ui: &mut egui::Ui,
    th: &Theme,
    id: egui::Id,
    reading: f64,
    caption: &str,
    width: f32,
) {
    let h = width * 0.52;
    let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    let shown = ballistic(ui, id, reading, 0.12);
    let p = ui.painter_at(rect);
    let square = th.bevel.is_some();
    // The face: the theme's light behind glass.
    let face = lerp(
        th.panel_raised,
        th.accent,
        if th.dark { 0.10 } else { 0.18 },
    );
    p.rect_filled(rect, Rounding::same(if square { 0.0 } else { 5.0 }), face);
    let mut glow = egui::Mesh::default();
    let (top, bottom) = (
        ca(th.accent, 6),
        ca(th.accent, if th.dark { 40 } else { 25 }),
    );
    glow.colored_vertex(rect.left_top(), top);
    glow.colored_vertex(rect.right_top(), top);
    glow.colored_vertex(rect.right_bottom(), bottom);
    glow.colored_vertex(rect.left_bottom(), bottom);
    glow.add_triangle(0, 1, 2);
    glow.add_triangle(0, 2, 3);
    p.add(Shape::mesh(glow));
    let pivot = pos2(rect.center().x, rect.top() + h * 1.45);
    let radius = h * 1.08;
    let ang = |u: f64| (-48.0 + u * 96.0 - 90.0f64).to_radians() as f32;
    let at = |u: f64, r: f32| pivot + vec2(ang(u).cos(), ang(u).sin()) * r;
    let (lo, hi) = (-20.0, 3.0);
    let arc = |from: f64, to: f64, r: f32| -> Vec<Pos2> {
        (0..=40)
            .map(|k| at(from + (to - from) * k as f64 / 40.0, r))
            .collect()
    };
    let zero = needle_pos(0.0, lo, hi);
    p.add(Shape::line(
        arc(0.0, zero, radius),
        Stroke::new(1.5_f32, c(th.text_dim)),
    ));
    p.add(Shape::line(
        arc(zero, 1.0, radius),
        Stroke::new(3.0_f32, c(th.meter_high)),
    ));
    for m in [
        -20.0, -10.0, -7.0, -5.0, -3.0, -2.0, -1.0, 0.0, 1.0, 2.0, 3.0,
    ] {
        let u = needle_pos(m, lo, hi);
        let red = m > 0.0;
        let col = if red { c(th.meter_high) } else { c(th.text) };
        p.line_segment(
            [at(u, radius), at(u, radius + 6.0)],
            Stroke::new(1.0_f32, col),
        );
        if m != -1.0 && m != -2.0 || width > 220.0 {
            p.text(
                at(u, radius + 13.0),
                Align2::CENTER_CENTER,
                if m > 0.0 {
                    format!("+{m}")
                } else {
                    format!("{m}")
                },
                FontId::proportional(9.0),
                col,
            );
        }
    }
    let u = needle_pos(shown, lo, hi);
    let tip = at(u, radius + 3.0);
    let start = pivot + (tip - pivot) * 0.55;
    p.line_segment(
        [start + vec2(1.0, 1.5), tip + vec2(1.0, 1.5)],
        Stroke::new(2.0_f32, Color32::from_black_alpha(70)),
    );
    p.line_segment([start, tip], Stroke::new(1.6_f32, c(th.text)));
    // The hood over the pivot.
    let hood = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 14.0), rect.max);
    p.rect_filled(
        hood,
        Rounding::ZERO,
        lerp(th.panel_raised, th.background, 0.5),
    );
    p.text(
        hood.center(),
        Align2::CENTER_CENTER,
        caption,
        FontId::proportional(9.0),
        c(th.text_dim),
    );
    if square {
        crate::skin::bevel(&p, rect, th, true);
    } else {
        p.rect_stroke(
            rect,
            Rounding::same(5.0),
            Stroke::new(1.0_f32, ca(th.border, 230)),
        );
    }
}

/// Input and output bars (dBFS, -60 to 0) from the rack's levels, with a
/// peak readout under each.
pub fn io_bars(ui: &mut egui::Ui, env: &Env, height: f32) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        level_bar(ui, env, false, height);
        level_bar(ui, env, true, height);
    });
}

/// The rack's input (or, `out`, output) bar with its name and peak under
/// it, for a face that puts the input on one side and the output on the
/// other.
pub fn level_bar(ui: &mut egui::Ui, env: &Env, out: bool, height: f32) {
    let th = &env.th;
    let (lin, lout) = env.levels.unwrap_or((0.0, 0.0));
    let (name, v) = if out { ("OUT", lout) } else { ("IN", lin) };
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        let (rect, _) = ui.allocate_exact_size(vec2(16.0, height), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, Rounding::same(2.0), c(th.wave_background));
        let db = Env::db(v);
        let f = ((db + 60.0) / 60.0).clamp(0.0, 1.0) as f32;
        if f > 0.0 {
            let top = rect.bottom() - rect.height() * f;
            let mut m = egui::Mesh::default();
            let lit = Rect::from_min_max(
                pos2(rect.left() + 2.0, top),
                rect.right_bottom() - vec2(2.0, 1.0),
            );
            let tc = if db > -6.0 {
                c(th.meter_high)
            } else if db > -18.0 {
                c(th.meter_mid)
            } else {
                c(th.meter_low)
            };
            m.colored_vertex(lit.left_top(), tc);
            m.colored_vertex(lit.right_top(), tc);
            m.colored_vertex(lit.right_bottom(), c(th.meter_low));
            m.colored_vertex(lit.left_bottom(), c(th.meter_low));
            m.add_triangle(0, 1, 2);
            m.add_triangle(0, 2, 3);
            p.add(Shape::mesh(m));
        }
        for mark in [0.0, -6.0, -12.0, -24.0, -48.0] {
            let y = rect.bottom() - rect.height() * ((mark + 60.0) / 60.0) as f32;
            p.line_segment(
                [pos2(rect.right(), y), pos2(rect.right() + 3.0, y)],
                Stroke::new(1.0_f32, ca(th.text_dim, 140)),
            );
        }
        p.rect_stroke(
            rect,
            Rounding::same(2.0),
            Stroke::new(1.0_f32, ca(th.border, 220)),
        );
        ui.label(RichText::new(name).size(9.0).color(c(th.text_dim)));
        ui.label(
            RichText::new(if db <= -89.0 {
                "-∞".to_string()
            } else {
                format!("{db:.0}")
            })
            .size(9.0)
            .monospace()
            .color(c(th.accent)),
        );
    })
    .response
    .on_hover_text(if out {
        "The rack's output (dBFS)"
    } else {
        "The rack's input (dBFS)"
    });
}

/// A transfer graph: input level across, output level up (both -60 to
/// 0 dBFS), the unit's curve lit, the threshold marked, and the input's
/// level as a dot on the curve while sound plays.
pub fn transfer(
    ui: &mut egui::Ui,
    th: &Theme,
    size: f32,
    curve: &dyn Fn(f64) -> f64,
    threshold: Option<f64>,
    input_db: Option<f64>,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let p = ui.painter_at(rect.expand(1.0));
    crate::graph::backdrop(&p, rect, th);
    let (lo, hi) = (-60.0f64, 0.0f64);
    let x = |db: f64| rect.left() + rect.width() * ((db - lo) / (hi - lo)).clamp(0.0, 1.0) as f32;
    let y =
        |db: f64| rect.bottom() - rect.height() * ((db - lo) / (hi - lo)).clamp(0.0, 1.0) as f32;
    for g in [-48.0, -36.0, -24.0, -12.0] {
        let s = Stroke::new(1.0_f32, ca(th.grid, 70));
        p.line_segment([pos2(x(g), rect.top()), pos2(x(g), rect.bottom())], s);
        p.line_segment([pos2(rect.left(), y(g)), pos2(rect.right(), y(g))], s);
    }
    // Unity, faint.
    p.line_segment(
        [pos2(x(lo), y(lo)), pos2(x(hi), y(hi))],
        Stroke::new(1.0_f32, ca(th.text_dim, 70)),
    );
    if let Some(t) = threshold {
        p.line_segment(
            [pos2(x(t), rect.top()), pos2(x(t), rect.bottom())],
            Stroke::new(1.0_f32, ca(th.accent_alt, 150)),
        );
    }
    let pts: Vec<Pos2> = (0..=60)
        .map(|k| {
            let i = lo + k as f64;
            pos2(x(i), y(curve(i)))
        })
        .collect();
    if th.dark && th.glow > 0.0 {
        p.add(Shape::line(
            pts.clone(),
            Stroke::new(5.0_f32, ca(th.accent, (35.0 * th.glow) as u8)),
        ));
    }
    p.add(Shape::line(pts, Stroke::new(2.0_f32, c(th.accent))));
    if let Some(i) = input_db.filter(|i| *i > lo) {
        p.circle_filled(pos2(x(i), y(curve(i))), 4.0, c(th.text));
    }
    p.text(
        rect.left_top() + vec2(4.0, 3.0),
        Align2::LEFT_TOP,
        "out",
        FontId::proportional(9.0),
        c(th.text_dim),
    );
    p.text(
        rect.right_bottom() - vec2(4.0, 3.0),
        Align2::RIGHT_BOTTOM,
        "in",
        FontId::proportional(9.0),
        c(th.text_dim),
    );
}

/// A compressor's static gain reduction (dB, 0 or more) for an input in
/// dB: threshold, ratio, soft knee width.
pub fn comp_gr(input: f64, thr: f64, ratio: f64, knee: f64) -> f64 {
    let over = input - thr;
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    if knee > 0.0 && over.abs() <= knee / 2.0 {
        slope * (over + knee / 2.0).powi(2) / (2.0 * knee)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

/// An estimate of a unit's gain reduction while playing: the static
/// reduction for the rack's input level, eased by the unit's attack and
/// release. (The windows read the rack's levels; the effects do not
/// report their own reduction to the app yet.)
pub fn est_gr(ui: &egui::Ui, id: egui::Id, target: f64, attack_ms: f64, release_ms: f64) -> f64 {
    let now = ui.input(|i| i.time);
    let (mut v, last): (f64, f64) = ui.ctx().data(|d| d.get_temp(id)).unwrap_or((0.0, now));
    let dt = (now - last).clamp(0.0, 0.1);
    let tau = if target > v { attack_ms } else { release_ms }.max(1.0) / 1000.0;
    v += (target - v) * (1.0 - (-dt / tau).exp());
    ui.ctx().data_mut(|d| d.insert_temp(id, (v, now)));
    if (target - v).abs() > 0.05 {
        ui.ctx().request_repaint();
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressor_curve_and_needle_scale() {
        assert_eq!(comp_gr(-30.0, -18.0, 4.0, 0.0), 0.0);
        assert!((comp_gr(-6.0, -18.0, 4.0, 0.0) - 9.0).abs() < 1e-9);
        // The knee meets both sides.
        let k = 6.0;
        assert!((comp_gr(-21.0, -18.0, 4.0, k)).abs() < 1e-9);
        assert!((comp_gr(-15.0, -18.0, 4.0, k) - 0.75 * 3.0).abs() < 1e-9);
        assert_eq!(needle_pos(-20.0, -20.0, 3.0), 0.0);
        assert!((needle_pos(3.0, -20.0, 3.0) - 1.0).abs() < 1e-12);
        assert!(needle_pos(0.0, -20.0, 3.0) > 0.6);
    }
}
