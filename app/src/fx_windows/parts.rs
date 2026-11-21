// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Pieces the effect faces share: the faceplate and its modules, the
//! needle meter, level bars, the dynamics transfer graph and the
//! gain-reduction estimate. All from the theme.

use egui::{
    pos2, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Rounding, Sense, Shape, Stroke, Vec2,
};

use super::{layout, Env};
use crate::theme::{c, ca, lerp, Theme};

/// The unit's faceplate: a raised plate with a title strip (none when
/// `title` is empty), `w` wide inside its margin (the face lays its rows
/// out across `w`).
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
            if title.is_empty() {
                // A face that draws its own heading.
                ui.vertical(add);
                return;
            }
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

// ------------------------------------------------------------ blends

/// A vertical blend from `top` to `bottom` over `rect`, its corners
/// rounded by `rad`, as shapes.
pub fn vgrad(rect: Rect, rad: f32, top: Color32, bottom: Color32) -> Vec<Shape> {
    let rad = rad.min(rect.height() / 2.0 - 0.5).max(0.0);
    let cap = (2.0 * rad + 1.0).min(rect.height());
    let mut out = vec![
        Shape::rect_filled(
            Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + cap)),
            Rounding {
                nw: rad,
                ne: rad,
                sw: 0.0,
                se: 0.0,
            },
            top,
        ),
        Shape::rect_filled(
            Rect::from_min_max(pos2(rect.left(), rect.bottom() - cap), rect.max),
            Rounding {
                nw: 0.0,
                ne: 0.0,
                sw: rad,
                se: rad,
            },
            bottom,
        ),
    ];
    let (y0, y1) = (rect.top() + rad, rect.bottom() - rad);
    if y1 > y0 {
        out.push(Shape::mesh(blend(
            Rect::from_x_y_ranges(rect.x_range(), y0..=y1),
            top,
            bottom,
        )));
    }
    out
}

/// A rectangle blended from `top` to `bottom`.
pub fn blend(r: Rect, top: Color32, bottom: Color32) -> egui::Mesh {
    let mut m = egui::Mesh::default();
    m.colored_vertex(r.left_top(), top);
    m.colored_vertex(r.right_top(), top);
    m.colored_vertex(r.right_bottom(), bottom);
    m.colored_vertex(r.left_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    m
}

// ------------------------------------------------------------ line icons

/// An icon drawn from SVG path data (`M L H V C S A Z`, absolute and
/// relative): its lines, the box `vb` of the path's own units mapped onto
/// `rect`. Curves and arcs become short straight steps.
pub fn icon(p: &egui::Painter, d: &str, vb: Vec2, rect: Rect, stroke: Stroke) {
    let s = (rect.width() / vb.x).min(rect.height() / vb.y);
    let off = rect.center() - vb * s / 2.0;
    for (pts, closed) in path_lines(d) {
        let pts: Vec<Pos2> = pts.iter().map(|q| off + q.to_vec2() * s).collect();
        if pts.len() < 2 {
            continue;
        }
        if closed {
            p.add(Shape::closed_line(pts, stroke));
        } else {
            p.add(Shape::line(pts, stroke));
        }
    }
}

/// One token of path data.
#[derive(Clone, Copy)]
enum Tok {
    Cmd(char),
    Num(f32),
}

fn tokens(d: &str) -> Vec<Tok> {
    // "3-8" is two numbers, so is ".5.5".
    let mut toks = Vec::new();
    let mut num = String::new();
    let flush = |num: &mut String, toks: &mut Vec<Tok>| {
        if !num.is_empty() {
            if let Ok(v) = num.parse() {
                toks.push(Tok::Num(v));
            }
            num.clear();
        }
    };
    for ch in d.chars() {
        if ch.is_ascii_alphabetic() && ch != 'e' {
            flush(&mut num, &mut toks);
            toks.push(Tok::Cmd(ch));
        } else if (ch == '-' && !num.ends_with('e')) || (ch == '.' && num.contains('.')) {
            // A sign or a second point starts the next number.
            flush(&mut num, &mut toks);
            num.push(ch);
        } else if ch.is_ascii_digit() || ch == '.' || ch == 'e' {
            num.push(ch);
        } else {
            flush(&mut num, &mut toks);
        }
    }
    flush(&mut num, &mut toks);
    toks
}

/// A cubic curve from `a` to `e` as short steps (after `a`).
fn cubic(out: &mut Vec<Pos2>, a: Pos2, b: Pos2, c2: Pos2, e: Pos2) {
    for k in 1..=12 {
        let t = k as f32 / 12.0;
        let u = 1.0 - t;
        let q = a.to_vec2() * u * u * u
            + b.to_vec2() * 3.0 * u * u * t
            + c2.to_vec2() * 3.0 * u * t * t
            + e.to_vec2() * t * t * t;
        out.push(q.to_pos2());
    }
}

/// SVG path data as polylines (each with whether it closes).
pub fn path_lines(d: &str) -> Vec<(Vec<Pos2>, bool)> {
    let toks = tokens(d);
    let mut out: Vec<(Vec<Pos2>, bool)> = Vec::new();
    let mut cur: Vec<Pos2> = Vec::new();
    let mut at = Pos2::ZERO;
    let mut start = Pos2::ZERO;
    let mut last_ctl: Option<Pos2> = None;
    let mut cmd = 'M';
    let mut i = 0;
    let nums = |i: &mut usize, n: usize| -> Option<Vec<f32>> {
        let v: Vec<f32> = toks
            .get(*i..*i + n)?
            .iter()
            .map_while(|t| match t {
                Tok::Num(v) => Some(*v),
                Tok::Cmd(_) => None,
            })
            .collect();
        (v.len() == n).then(|| {
            *i += n;
            v
        })
    };
    while i < toks.len() {
        if let Tok::Cmd(ch) = toks[i] {
            cmd = ch;
            i += 1;
            if ch == 'Z' || ch == 'z' {
                if !cur.is_empty() {
                    out.push((std::mem::take(&mut cur), true));
                }
                at = start;
                last_ctl = None;
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { at.to_vec2() } else { Vec2::ZERO };
        let pt = |x: f32, y: f32| pos2(x, y) + base;
        let want = match cmd.to_ascii_uppercase() {
            'M' | 'L' => 2,
            'H' | 'V' => 1,
            'C' => 6,
            'S' => 4,
            'A' => 7,
            _ => 0,
        };
        let Some(v) = (want > 0).then(|| nums(&mut i, want)).flatten() else {
            // Unknown or short: skip a token.
            i += 1;
            continue;
        };
        match cmd.to_ascii_uppercase() {
            'M' => {
                if cur.len() > 1 {
                    out.push((std::mem::take(&mut cur), false));
                }
                cur.clear();
                at = pt(v[0], v[1]);
                start = at;
                cur.push(at);
                // Further pairs are lines.
                cmd = if rel { 'l' } else { 'L' };
                last_ctl = None;
            }
            'L' => {
                at = pt(v[0], v[1]);
                cur.push(at);
                last_ctl = None;
            }
            'H' => {
                at = pos2(if rel { at.x + v[0] } else { v[0] }, at.y);
                cur.push(at);
                last_ctl = None;
            }
            'V' => {
                at = pos2(at.x, if rel { at.y + v[0] } else { v[0] });
                cur.push(at);
                last_ctl = None;
            }
            'C' => {
                let (b, c2, e) = (pt(v[0], v[1]), pt(v[2], v[3]), pt(v[4], v[5]));
                cubic(&mut cur, at, b, c2, e);
                last_ctl = Some(c2);
                at = e;
            }
            'S' => {
                let b = last_ctl.map_or(at, |c0| at + (at - c0));
                let (c2, e) = (pt(v[0], v[1]), pt(v[2], v[3]));
                cubic(&mut cur, at, b, c2, e);
                last_ctl = Some(c2);
                at = e;
            }
            _ => {
                let e = pt(v[5], v[6]);
                arc(
                    &mut cur,
                    at,
                    e,
                    (v[0], v[1]),
                    v[2],
                    v[3] != 0.0,
                    v[4] != 0.0,
                );
                at = e;
                last_ctl = None;
            }
        }
    }
    if cur.len() > 1 {
        out.push((cur, false));
    }
    out
}

/// An SVG arc from `a` to `b` as points (the endpoint form turned into a
/// centre and sweep, as the SVG spec gives it).
fn arc(out: &mut Vec<Pos2>, a: Pos2, b: Pos2, r: (f32, f32), rot: f32, large: bool, sweep: bool) {
    let (mut rx, mut ry) = (r.0.abs(), r.1.abs());
    if rx < 1e-6 || ry < 1e-6 || a == b {
        out.push(b);
        return;
    }
    let phi = rot.to_radians();
    let (cs, sn) = (phi.cos(), phi.sin());
    let dx = (a.x - b.x) / 2.0;
    let dy = (a.y - b.y) / 2.0;
    let x1 = cs * dx + sn * dy;
    let y1 = -sn * dx + cs * dy;
    let l = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if l > 1.0 {
        rx *= l.sqrt();
        ry *= l.sqrt();
    }
    let num = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.0);
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut k = (num / den.max(1e-12)).sqrt();
    if large == sweep {
        k = -k;
    }
    let cx1 = k * rx * y1 / ry;
    let cy1 = -k * ry * x1 / rx;
    let cx = cs * cx1 - sn * cy1 + (a.x + b.x) / 2.0;
    let cy = sn * cx1 + cs * cy1 + (a.y + b.y) / 2.0;
    let t1 = ((y1 - cy1) / ry).atan2((x1 - cx1) / rx);
    let t2 = ((-y1 - cy1) / ry).atan2((-x1 - cx1) / rx);
    let tau = std::f32::consts::TAU;
    let mut dt = t2 - t1;
    if sweep && dt < 0.0 {
        dt += tau;
    } else if !sweep && dt > 0.0 {
        dt -= tau;
    }
    let n = ((dt.abs() / tau) * 32.0).ceil().max(2.0) as usize;
    for s in 1..=n {
        let t = t1 + dt * s as f32 / n as f32;
        let (x, y) = (rx * t.cos(), ry * t.sin());
        out.push(pos2(cs * x - sn * y + cx, sn * x + cs * y + cy));
    }
}

// ------------------------------------------------------------ fader

/// A short upright fader: drag up or down (Shift: fine), wheel, double-
/// click for the default. Lit from the default to the value.
#[allow(clippy::too_many_arguments)]
pub fn vfader(
    ui: &mut egui::Ui,
    th: &Theme,
    value: &mut f64,
    lo: f64,
    hi: f64,
    default: f64,
    size: Vec2,
    lit: Color32,
) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let span = (hi - lo).max(1e-9);
    let mut v = *value;
    if resp.double_clicked() {
        v = default;
    } else if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let dy = f64::from(resp.drag_delta().y) * if fine { 0.1 } else { 1.0 };
        v -= dy / f64::from(rect.height() - 10.0) * span;
    }
    if resp.hovered() {
        let wheel = ui.input(|i| i.smooth_scroll_delta.y);
        if wheel != 0.0 {
            v += f64::from(wheel.signum()) * span / 100.0;
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let v = v.clamp(lo, hi);
    if v != *value {
        *value = v;
        resp.mark_changed();
    }
    let p = ui.painter();
    let travel = rect.shrink2(vec2(0.0, 5.0));
    let y = |v: f64| travel.bottom() - travel.height() * ((v - lo) / span) as f32;
    let groove = Rect::from_center_size(rect.center(), vec2(4.0, travel.height() + 4.0));
    p.rect_filled(groove, Rounding::same(2.0), c(th.wave_background));
    p.rect_stroke(
        groove,
        Rounding::same(2.0),
        Stroke::new(1.0_f32, ca(th.border, 200)),
    );
    let (ya, yb) = (y(default), y(*value));
    if (ya - yb).abs() > 0.5 {
        p.rect_filled(
            Rect::from_min_max(
                pos2(groove.left() + 1.0, ya.min(yb)),
                pos2(groove.right() - 1.0, ya.max(yb)),
            ),
            Rounding::same(1.0),
            lit,
        );
    }
    let hot = resp.hovered() || resp.dragged();
    let cap = Rect::from_center_size(pos2(rect.center().x, yb), vec2(size.x - 2.0, 9.0));
    p.rect_filled(
        cap.translate(vec2(0.0, 1.5)),
        Rounding::same(2.0),
        Color32::from_black_alpha(80),
    );
    p.rect_filled(
        cap,
        Rounding::same(2.0),
        lerp(th.panel_raised, th.text, if hot { 0.30 } else { 0.18 }),
    );
    p.rect_stroke(
        cap,
        Rounding::same(2.0),
        Stroke::new(1.0_f32, ca(th.border, 230)),
    );
    p.line_segment(
        [
            pos2(cap.left() + 3.0, cap.center().y),
            pos2(cap.right() - 3.0, cap.center().y),
        ],
        Stroke::new(1.5_f32, lit),
    );
    resp
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

    #[test]
    fn path_data_becomes_lines() {
        // Relative steps, a run of numbers without spaces, and H.
        let l = path_lines("M2 12h4l3-8 4 16");
        assert_eq!(l.len(), 1);
        assert_eq!(
            l[0].0,
            vec![
                pos2(2.0, 12.0),
                pos2(6.0, 12.0),
                pos2(9.0, 4.0),
                pos2(13.0, 20.0)
            ]
        );
        // Two shapes, both closed.
        let l = path_lines("M6 8h8v12H6zM16 8h8v12h-8z");
        assert_eq!(l.len(), 2);
        assert!(l.iter().all(|(p, closed)| *closed && p.len() == 4));
        // A near-full circle by arc stays on its radius.
        let l = path_lines("M15 5a9 9 0 1 0 0.01 0");
        let ctr = pos2(15.005, 14.0);
        assert!(l[0].0.len() > 10);
        for q in &l[0].0 {
            assert!(((*q - ctr).length() - 9.0).abs() < 0.05, "{q:?}");
        }
        // A curve ends where it says.
        let l = path_lines("M3 16c6 0 6-6 12-6s6 6 12 6");
        assert_eq!(*l[0].0.last().unwrap(), pos2(27.0, 16.0));
    }
}
