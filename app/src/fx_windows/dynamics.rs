// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The dynamics units: AudiForm 2's own Compressor, Limiter, Noise Gate
//! and De-Esser faces (kept for the built-in effects that older sessions
//! use), and the Vox Strip face after the lead's Audiform panel. The Vox
//! Strip follows its Audiform window closely: a heading with the Low Cut
//! switch, three numbered modules in signal order (Dynamics, Tone,
//! Effects), each with a power light, a style stepper with the style's
//! icon, its main controls and a trim, and the Input and Output dials
//! either side of an output ladder. (The Opto Leveler, Tube Preamp, Amp
//! Model and Audiform Limiter faces are in `units.rs`.)
//!
//! The needles show the reduction worked out from the rack's input level
//! and the unit's own curve and timing (or the rack's output level); the
//! bars show the rack's input and output.

use std::sync::Arc;

use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke};

use super::layout::{self, GAP, GAP_S, HERO, KNOB, PAD, SIDE};
use super::parts::{self, comp_gr};
use super::{Env, Fx};
use crate::theme::{c, ca, lerp, Theme};

/// The rack's input level in dB while playing.
fn input_db(env: &Env) -> Option<f64> {
    env.playing.then_some(())?;
    env.levels.map(|(i, _)| Env::db(i))
}

fn output_db(env: &Env) -> f64 {
    if !env.playing {
        return -90.0;
    }
    env.levels.map_or(-90.0, |(_, o)| Env::db(o))
}

/// A dynamics unit's width: a hero dial either side of the meters.
const UNIT_W: f32 = 2.0 * SIDE + 256.0;
/// The meters' width (the needle's).
const METER_W: f32 = 232.0;

/// The middle of a unit: the needle over the transfer graph, the input
/// bar on its left and the output bar on its right.
fn centre(
    ui: &mut egui::Ui,
    env: &Env,
    reading: f64,
    caption: &str,
    curve: &dyn Fn(f64) -> f64,
    threshold: Option<f64>,
) {
    ui.vertical(|ui| {
        parts::needle(
            ui,
            &env.th,
            env.key.with("needle"),
            reading,
            caption,
            METER_W,
        );
        ui.add_space(GAP_S);
        layout::row(ui, METER_W, &[40.0, 152.0, 40.0], |ui, i| match i {
            0 => parts::level_bar(ui, env, false, 82.0),
            1 => parts::transfer(ui, &env.th, 112.0, curve, threshold, input_db(env)),
            _ => parts::level_bar(ui, env, true, 82.0),
        });
    });
}

pub fn compressor(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let (thr, ratio, knee) = (fx.get("Threshold"), fx.get("Ratio"), fx.get("Knee"));
    let mk = fx.get("Makeup");
    let target = input_db(env).map_or(0.0, |i| comp_gr(i, thr, ratio, knee));
    let gr = parts::est_gr(
        ui,
        env.key.with("gr"),
        target,
        fx.get("Attack"),
        fx.get("Release"),
    );
    parts::faceplate(
        ui,
        &env.th,
        "Compressor",
        "levels the loud parts",
        UNIT_W,
        |ui| {
            layout::hero(ui, UNIT_W, SIDE, |ui, i| match i {
                0 => fx.knob(ui, "Threshold", HERO, "", &[-60.0, -40.0, -20.0, 0.0]),
                1 => centre(
                    ui,
                    env,
                    -gr,
                    "GAIN REDUCTION",
                    &|i| i - comp_gr(i, thr, ratio, knee) + mk,
                    Some(thr),
                ),
                _ => fx.knob_fmt(ui, "Ratio", HERO, "", &[1.0, 5.0, 10.0, 20.0], &|v| {
                    format!("{v:.1}:1")
                }),
            });
            ui.add_space(GAP);
            let keys = ["Attack", "Release", "Knee", "Makeup"];
            layout::cells(ui, UNIT_W, keys.len(), |ui, i| {
                fx.knob(ui, keys[i], KNOB, "", &[]);
            });
        },
    );
}

/// A hero dial with a small one centred under it (a side column).
fn hero_and_small(ui: &mut egui::Ui, fx: &mut Fx, big: (&str, &str, &[f64]), small: &str) {
    layout::centred(ui, SIDE, |ui| fx.knob(ui, big.0, HERO, big.1, big.2));
    ui.add_space(GAP_S);
    layout::centred(ui, SIDE, |ui| fx.knob(ui, small, KNOB, "", &[]));
}

pub fn limiter(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let (ceil, boost) = (fx.get("Ceiling"), fx.get("Input boost"));
    let target = input_db(env).map_or(0.0, |i| (i + boost - ceil).max(0.0));
    let gr = parts::est_gr(
        ui,
        env.key.with("gr"),
        target,
        fx.get("Look-ahead").max(0.5),
        fx.get("Release"),
    );
    parts::faceplate(
        ui,
        &env.th,
        "Limiter",
        "a brick wall at the ceiling",
        UNIT_W,
        |ui| {
            // Each big dial with its timing under it: in (boost, how far
            // ahead it looks) on the left, out (ceiling, release) on the
            // right.
            layout::hero(ui, UNIT_W, SIDE, |ui, i| match i {
                0 => hero_and_small(
                    ui,
                    fx,
                    ("Input boost", "Boost", &[0.0, 6.0, 12.0, 18.0, 24.0]),
                    "Look-ahead",
                ),
                1 => centre(
                    ui,
                    env,
                    -gr,
                    "GAIN REDUCTION",
                    &|i| (i + boost).min(ceil),
                    Some(ceil - boost),
                ),
                _ => hero_and_small(
                    ui,
                    fx,
                    ("Ceiling", "", &[-24.0, -12.0, -6.0, 0.0]),
                    "Release",
                ),
            });
        },
    );
}

pub fn gate(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let (thr, floor) = (fx.get("Threshold"), fx.get("Floor"));
    let target = input_db(env).map_or(0.0, |i| if i < thr { -floor } else { 0.0 });
    let gr = parts::est_gr(
        ui,
        env.key.with("gr"),
        target,
        fx.get("Attack"),
        fx.get("Release"),
    );
    parts::faceplate(
        ui,
        &env.th,
        "Noise Gate",
        "closes in the quiet",
        UNIT_W,
        |ui| {
            layout::hero(ui, UNIT_W, SIDE, |ui, i| match i {
                0 => fx.knob(
                    ui,
                    "Threshold",
                    HERO,
                    "",
                    &[-80.0, -60.0, -40.0, -20.0, 0.0],
                ),
                1 => centre(
                    ui,
                    env,
                    -gr,
                    "ATTENUATION",
                    &|i| if i < thr { i + floor } else { i },
                    Some(thr),
                ),
                _ => fx.knob(ui, "Floor", HERO, "", &[-80.0, -40.0, 0.0]),
            });
            ui.add_space(GAP);
            let keys = ["Attack", "Hold", "Release"];
            layout::cells(ui, UNIT_W, keys.len(), |ui, i| {
                fx.knob(ui, keys[i], KNOB, "", &[]);
            });
        },
    );
}

pub fn deesser(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let (thr, red) = (fx.get("Threshold"), fx.get("Reduction"));
    let target = input_db(env).map_or(0.0, |i| (i - thr).clamp(0.0, red));
    let gr = parts::est_gr(ui, env.key.with("gr"), target, 2.0, 60.0);
    parts::faceplate(
        ui,
        &env.th,
        "De-Esser",
        "tames the s sounds",
        UNIT_W,
        |ui| {
            layout::hero(ui, UNIT_W, SIDE, |ui, i| match i {
                0 => fx.knob(ui, "Threshold", HERO, "", &[-60.0, -40.0, -20.0, 0.0]),
                1 => centre(
                    ui,
                    env,
                    -gr,
                    "SIBILANCE CUT",
                    &|i| i - (i - thr).clamp(0.0, red),
                    Some(thr),
                ),
                _ => fx.knob(ui, "Reduction", HERO, "", &[0.0, 6.0, 12.0, 18.0, 24.0]),
            });
        },
    );
}

// ------------------------------------------------------------ Vox Strip

/// A Vox Strip module: its name and its power, style and trim controls.
struct Module {
    name: &'static str,
    power: &'static str,
    style: &'static str,
    trim: &'static str,
}

/// The modules in signal order.
const MODULES: [Module; 3] = [
    Module {
        name: "Dynamics",
        power: "dyn_on",
        style: "dyn_style",
        trim: "dyn_trim",
    },
    Module {
        name: "Tone",
        power: "tone_on",
        style: "tone_style",
        trim: "tone_trim",
    },
    Module {
        name: "Effects",
        power: "fx_on",
        style: "fx_style",
        trim: "fx_trim",
    },
];

/// Each style's line icon, as path data on a 30 by 28 box.
pub(super) fn style_icon(module: usize, style: &str) -> &'static str {
    match (module, style) {
        (0, "Smooth + Fast") => "M4 20c6 0 6-12 11-12M15 8l3 6 3-10 3 16",
        (0, "Clean") => "M4 20L15 9h11",
        (0, "Warm") => "M15 4c-4 6-7 9-7 13a7 7 0 0 0 14 0c0-4-3-7-7-13z",
        (1, "Clean") => "M3 16c6 0 6-6 12-6s6 6 12 6",
        (1, "Vintage") => "M15 5a9 9 0 1 0 0.01 0M15 9v5l3 2",
        (1, "Modern") => "M3 18l6-8 6 6 6-10 6 4",
        (2, "Chorus") => "M3 14c3-6 5-6 8 0s5 6 8 0 5-6 8 0",
        (2, "Doubler") => "M6 8h8v12H6zM16 8h8v12h-8z",
        (2, "Saturate") => "M3 20c4 0 5-12 12-12s8 12 12 12",
        (2, "Radio") => "M5 11h20v11H5zM8 7l9 4M19 16a2.5 2.5 0 1 0 0.01 0",
        (2, "Widen") => "M15 6v16M4 14h7M8 11l-4 3 4 3M26 14h-7M22 11l4 3-4 3",
        _ => "",
    }
}

/// The strip's mark: a voice's wave.
const MARK: &str = "M2 12h4l3-8 4 16 4-12 3 6 2-2h4";
/// The Low Cut switch's curve.
const LOW_CUT: &str = "M2 11c4 0 5-8 9-8h11";

/// A module's width, and the whole strip's (three modules side by side).
const MOD_W: f32 = 228.0;
const VOX_W: f32 = 3.0 * MOD_W + 2.0 * GAP_S;
/// The inside of a module.
const MOD_IN: f32 = MOD_W - 2.0 * PAD;
/// The height of a module's main controls (the same in all three, so the
/// modules line up).
const BODY_H: f32 = 226.0;
/// The Input and Output dials' columns.
const IO_SIDE: f32 = 84.0;
/// About how far a voice's "s" band sits under its overall peak, for the
/// de-essing estimate.
const SIBILANCE_DB: f64 = 15.0;

/// The Dynamics module's reduction and de-essing (dB) while playing,
/// estimated from the rack's input level and the style's own curve and
/// timing (the same settings `af2_fx`'s Vox Strip uses). The effect
/// reports its real reduction only to a meter the app does not read yet.
fn vox_meters(ui: &egui::Ui, fx: &Fx, env: &Env) -> (f64, f64) {
    let input = input_db(env).map(|i| i + fx.get("input"));
    let on = fx.on("dyn_on");
    let c = fx.get("comp") / 100.0;
    let (thr, ratio, knee, att, rel) = match fx.get("dyn_style").round() as i64 {
        0 => (-8.0 - 30.0 * c, 3.0, 12.0, 10.0, 350.0),
        1 => (-6.0 - 30.0 * c, 4.0, 6.0, 4.0, 120.0),
        _ => (-8.0 - 28.0 * c, 2.5, 14.0, 15.0, 600.0),
    };
    let gr = match input {
        Some(i) if on => comp_gr(i, thr, ratio, knee),
        _ => 0.0,
    };
    let d = fx.get("deess") / 100.0;
    let ds = match input {
        Some(i) if on && d > 0.0 => {
            ((i - SIBILANCE_DB) - (-14.0 - 30.0 * d)).max(0.0) * (0.55 + 0.4 * d)
        }
        _ => 0.0,
    };
    (
        parts::est_gr(ui, env.key.with("vox-gr"), gr, att, rel),
        parts::est_gr(ui, env.key.with("vox-ds"), ds, 2.0, 60.0),
    )
}

/// Text in capitals, spaced out.
fn caps(ui: &egui::Ui, text: &str, size: f32, spacing: f32, col: Color32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &text.to_uppercase(),
        0.0,
        egui::TextFormat {
            font_id: FontId::proportional(size),
            color: col,
            extra_letter_spacing: spacing,
            ..Default::default()
        },
    );
    ui.painter().layout_job(job)
}

pub fn vox(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = env.th.clone();
    let (gr, ds) = vox_meters(ui, fx, env);
    parts::faceplate(ui, &th, "", "", VOX_W, |ui| {
        brand(ui, fx, &th);
        ui.add_space(GAP_S);
        layout::columns(ui, &[MOD_W; 3], GAP_S, |ui, m| {
            module(ui, fx, env, m, (gr, ds));
        });
        ui.add_space(GAP_S);
        io_row(ui, fx, env);
    });
}

/// The strip's heading: its mark and name, and the Low Cut switch at the
/// right (it runs before every module).
fn brand(ui: &mut egui::Ui, fx: &mut Fx, th: &Theme) {
    let (row, _) = ui.allocate_exact_size(vec2(VOX_W, 28.0), Sense::hover());
    let p = ui.painter();
    let mark = Rect::from_min_size(
        pos2(row.left() + 2.0, row.center().y - 11.0),
        vec2(36.0, 22.0),
    );
    parts::icon(
        p,
        MARK,
        vec2(40.0, 24.0),
        mark,
        Stroke::new(2.2_f32, c(th.accent)),
    );
    let word = caps(ui, "Vox Strip", 14.0, 3.0, c(th.text));
    let wx = mark.right() + 8.0;
    p.galley(
        pos2(wx, row.center().y - word.size().y / 2.0),
        word.clone(),
        c(th.text),
    );
    let tag = caps(ui, "vocal chain", 9.0, 0.8, c(th.text_dim));
    p.galley(
        pos2(
            wx + word.size().x + 10.0,
            row.center().y - tag.size().y / 2.0 + 1.0,
        ),
        tag,
        c(th.text_dim),
    );
    // Low Cut.
    let lit = fx.on("low_cut");
    let label = caps(ui, "Low Cut", 10.0, 0.5, c(th.text));
    let bw = 20.0 + 4.0 + label.size().x + 16.0;
    let br = Rect::from_min_size(
        pos2(row.right() - bw, row.center().y - 11.0),
        vec2(bw, 22.0),
    );
    let resp = ui
        .interact(br, fx.uid.with("low-cut"), Sense::click())
        .on_hover_text("Low Cut: removes rumble below 80 Hz before everything else");
    if resp.clicked() {
        fx.set("low_cut", if lit { 0.0 } else { 1.0 });
    }
    let p = ui.painter();
    let square = th.bevel.is_some();
    let rad = if square { 0.0 } else { 4.0 };
    p.rect_filled(
        br,
        rad,
        lerp(
            th.panel_raised,
            th.text,
            if resp.hovered() { 0.10 } else { 0.04 },
        ),
    );
    if square {
        crate::skin::bevel(p, br, th, lit);
    } else {
        p.rect_stroke(
            br,
            rad,
            Stroke::new(
                1.0_f32,
                if lit {
                    c(th.accent)
                } else {
                    ca(th.border, 220)
                },
            ),
        );
    }
    let ink = if lit { c(th.text) } else { c(th.text_dim) };
    parts::icon(
        p,
        LOW_CUT,
        vec2(24.0, 12.0),
        Rect::from_min_size(pos2(br.left() + 8.0, br.center().y - 5.0), vec2(20.0, 10.0)),
        Stroke::new(1.6_f32, if lit { c(th.accent) } else { ink }),
    );
    p.galley(
        pos2(br.left() + 32.0, br.center().y - label.size().y / 2.0),
        label,
        ink,
    );
    // The line under the heading.
    p.line_segment(
        [
            pos2(row.left(), row.bottom() + 3.0),
            pos2(row.right(), row.bottom() + 3.0),
        ],
        Stroke::new(1.0_f32, ca(th.text, 16)),
    );
    ui.add_space(4.0);
}

/// One module: its power light, number and name; the style stepper; its
/// main controls; its trim.
fn module(ui: &mut egui::Ui, fx: &mut Fx, env: &Env, m: usize, meters: (f64, f64)) {
    let th = env.th.clone();
    let md = &MODULES[m];
    let live = fx.on(md.power);
    let back = ui.painter().add(Shape::Noop);
    let rect = egui::Frame::none()
        .inner_margin(egui::Margin::same(PAD))
        .show(ui, |ui| {
            ui.set_width(MOD_IN);
            ui.spacing_mut().item_spacing.y = 6.0;
            header(ui, fx, &th, m, live);
            ui.scope(|ui| {
                if !live {
                    ui.multiply_opacity(0.45);
                }
                stepper(ui, fx, &th, m);
                body(ui, fx, env, m, meters);
            });
            layout::centred(ui, MOD_IN, |ui| fx.knob(ui, md.trim, 40.0, "Trim", &[]));
        })
        .response
        .rect;
    let square = th.bevel.is_some();
    let rad = if square { 0.0 } else { 6.0 };
    let mut shapes = parts::vgrad(
        rect,
        rad,
        lerp(th.panel_raised, th.text, 0.03),
        lerp(th.panel_raised, th.background, 0.55),
    );
    if !square {
        shapes.push(Shape::rect_stroke(
            rect,
            rad,
            Stroke::new(1.0_f32, ca(th.border, 170)),
        ));
    }
    ui.painter().set(back, Shape::Vec(shapes));
    if square {
        crate::skin::bevel(ui.painter(), rect, &th, false);
    }
}

/// A module's heading: the power light, its number and its name.
fn header(ui: &mut egui::Ui, fx: &mut Fx, th: &Theme, m: usize, live: bool) {
    let md = &MODULES[m];
    let (row, _) = ui.allocate_exact_size(vec2(MOD_IN, 18.0), Sense::hover());
    let led = Rect::from_center_size(pos2(row.left() + 8.0, row.center().y), vec2(16.0, 16.0));
    let resp = ui
        .interact(led, fx.uid.with(("power", m)), Sense::click())
        .on_hover_text(if live {
            format!("{}: on (click to bypass)", md.name)
        } else {
            format!("{}: bypassed (click to turn on)", md.name)
        });
    if resp.clicked() {
        fx.set(md.power, if live { 0.0 } else { 1.0 });
    }
    let p = ui.painter();
    p.circle_filled(led.center(), 8.0, c(th.wave_background));
    p.circle_stroke(
        led.center(),
        8.0,
        Stroke::new(
            1.0_f32,
            if resp.hovered() {
                c(th.accent)
            } else {
                ca(th.border, 230)
            },
        ),
    );
    if live {
        if th.glow > 0.0 {
            p.circle_filled(
                led.center(),
                6.5,
                ca(th.accent, (60.0 * th.glow.min(1.0)) as u8),
            );
        }
        p.circle_filled(led.center(), 4.0, c(th.accent));
    } else {
        p.circle_filled(led.center(), 4.0, lerp(th.wave_background, th.text, 0.15));
    }
    // The number in its box.
    let nb = Rect::from_center_size(
        pos2(led.right() + 6.0 + 7.5, row.center().y),
        vec2(15.0, 15.0),
    );
    p.rect_stroke(
        nb,
        if th.bevel.is_some() { 0.0 } else { 3.0 },
        Stroke::new(1.0_f32, ca(th.text_dim, 120)),
    );
    p.text(
        nb.center(),
        Align2::CENTER_CENTER,
        format!("{}", m + 1),
        FontId::proportional(9.0),
        c(th.text_dim),
    );
    let name = caps(ui, md.name, 10.0, 1.4, c(th.text));
    ui.painter().galley(
        pos2(nb.right() + 6.0, row.center().y - name.size().y / 2.0),
        name,
        c(th.text),
    );
}

/// The style stepper: back and on through the module's styles, the
/// style's icon, name and place among them in a lit window.
fn stepper(ui: &mut egui::Ui, fx: &mut Fx, th: &Theme, m: usize) {
    let md = &MODULES[m];
    let Some(ctl) = fx.ctl(md.style).cloned() else {
        return;
    };
    let n = ctl.choices.len().max(1);
    let cur = ((fx.get(md.style) - ctl.min).round().max(0.0) as usize).min(n - 1);
    let style = ctl.choices.get(cur).cloned().unwrap_or_default();
    let (row, _) = ui.allocate_exact_size(vec2(MOD_IN, 54.0), Sense::hover());
    let square = th.bevel.is_some();
    let rad = if square { 0.0 } else { 4.0 };
    let bw = 18.0;
    let mut step = 0i64;
    for (k, d) in [(0, -1i64), (1, 1)] {
        let r = if k == 0 {
            Rect::from_min_size(row.min, vec2(bw, row.height()))
        } else {
            Rect::from_min_size(pos2(row.right() - bw, row.top()), vec2(bw, row.height()))
        };
        let resp = ui
            .interact(r, fx.uid.with(("style-step", m, k)), Sense::click())
            .on_hover_text(if d < 0 {
                format!("Previous {} style", md.name)
            } else {
                format!("Next {} style", md.name)
            });
        if resp.clicked() {
            step = d;
        }
        let p = ui.painter();
        p.rect_filled(r, rad, lerp(th.wave_background, th.panel_raised, 0.3));
        if square {
            crate::skin::bevel(p, r, th, false);
        } else {
            p.rect_stroke(r, rad, Stroke::new(1.0_f32, ca(th.border, 200)));
        }
        let col = if resp.hovered() {
            c(th.accent)
        } else {
            c(th.text_dim)
        };
        let (cx, cy) = (r.center().x, r.center().y);
        let s = -(d as f32) * 3.0;
        p.add(Shape::line(
            vec![
                pos2(cx + s, cy - 6.0),
                pos2(cx - s, cy),
                pos2(cx + s, cy + 6.0),
            ],
            Stroke::new(1.6_f32, col),
        ));
    }
    if step != 0 {
        let next = (cur as i64 + step).rem_euclid(n as i64) as f64;
        fx.set(md.style, ctl.min + next);
    }
    // The window.
    let win = Rect::from_min_max(
        pos2(row.left() + bw + 3.0, row.top()),
        pos2(row.right() - bw - 3.0, row.bottom()),
    );
    let p = ui.painter();
    p.rect_filled(win, rad, c(th.wave_background));
    if square {
        crate::skin::bevel(p, win, th, true);
    } else {
        p.line_segment(
            [
                pos2(win.left() + rad, win.top() + 1.0),
                pos2(win.right() - rad, win.top() + 1.0),
            ],
            Stroke::new(1.0_f32, Color32::from_black_alpha(110)),
        );
        p.rect_stroke(win, rad, Stroke::new(1.0_f32, ca(th.border, 160)));
    }
    let icon_r = Rect::from_center_size(pos2(win.center().x, win.top() + 17.0), vec2(30.0, 26.0));
    let d = style_icon(m, &style);
    if th.glow > 0.0 {
        parts::icon(
            p,
            d,
            vec2(30.0, 28.0),
            icon_r,
            Stroke::new(4.0_f32, ca(th.accent, (45.0 * th.glow.min(1.0)) as u8)),
        );
    }
    parts::icon(
        p,
        d,
        vec2(30.0, 28.0),
        icon_r,
        Stroke::new(1.6_f32, c(th.accent)),
    );
    p.text(
        pos2(win.center().x, win.top() + 38.0),
        Align2::CENTER_CENTER,
        &style,
        FontId::proportional(11.0),
        c(th.text),
    );
    let dots_w = n as f32 * 4.0 + (n - 1) as f32 * 3.0;
    for k in 0..n {
        let x = win.center().x - dots_w / 2.0 + 2.0 + k as f32 * 7.0;
        p.circle_filled(
            pos2(x, win.bottom() - 6.0),
            2.0,
            if k == cur {
                c(th.accent)
            } else {
                lerp(th.wave_background, th.text, 0.22)
            },
        );
    }
}

/// A module's main controls, in a block of one height.
fn body(ui: &mut egui::Ui, fx: &mut Fx, env: &Env, m: usize, (gr, ds): (f64, f64)) {
    let th = &env.th;
    let rect = Rect::from_min_size(ui.cursor().min, vec2(MOD_IN, BODY_H));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("vox-body", m))
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let ui2 = &mut child;
    ui2.spacing_mut().item_spacing.y = 6.0;
    let live = fx.on(MODULES[m].power);
    match m {
        0 => {
            layout::centred(ui2, MOD_IN, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    let g = if live { gr } else { 0.0 };
                    let d = if live { ds } else { 0.0 };
                    reduction_bar(ui, th, g, "GR", c(th.accent)).on_hover_text(format!(
                        "Gain reduction: {g:.1} dB (estimated from the input level)"
                    ));
                    reduction_bar(ui, th, d, "DS", c(th.meter_mid)).on_hover_text(format!(
                        "De-essing: {d:.1} dB (estimated from the input level)"
                    ));
                    ui.add_space(3.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        fx.knob(ui, "comp", 74.0, "", &[]);
                        fx.knob(ui, "deess", 74.0, "", &[]);
                    });
                });
            });
        }
        1 => {
            tone_graph(ui2, fx, th);
            layout::cells(ui2, MOD_IN, 4, |ui, k| {
                let key = ["low", "mid", "high", "air"][k];
                tone_slider(ui, fx, th, key, k == 3);
            });
        }
        _ => {
            ui2.add_space((BODY_H - 112.0 - 36.0) / 2.0);
            layout::centred(ui2, MOD_IN, |ui| fx.knob(ui, "fx", 112.0, "", &[]));
        }
    }
    ui.advance_cursor_after_rect(rect);
}

/// A reduction bar: 0 to 24 dB, hanging from the top.
fn reduction_bar(
    ui: &mut egui::Ui,
    th: &Theme,
    db: f64,
    label: &str,
    col: Color32,
) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(12.0, 190.0), Sense::hover());
    let p = ui.painter();
    let rad = if th.bevel.is_some() { 0.0 } else { 3.0 };
    p.rect_filled(r, rad, c(th.wave_background));
    let f = (db / 24.0).clamp(0.0, 1.0) as f32;
    if f > 0.0 {
        let lit = Rect::from_min_size(r.min, vec2(r.width(), r.height() * f)).shrink(1.0);
        p.add(Shape::mesh(parts::blend(
            lit,
            col,
            lerp([col.r(), col.g(), col.b()], [0, 0, 0], 0.5),
        )));
    }
    p.rect_stroke(r, rad, Stroke::new(1.0_f32, ca(th.border, 200)));
    p.text(
        pos2(r.center().x, r.bottom() - 2.0),
        Align2::CENTER_BOTTOM,
        label,
        FontId::proportional(8.0),
        c(th.text_dim),
    );
    resp
}

/// The Tone module's curve (Low, Mid, High and Air as set, from 20 Hz to
/// 20 kHz, -15 to +15 dB), worked out by the effect's own filters.
fn tone_graph(ui: &mut egui::Ui, fx: &Fx, th: &Theme) {
    let (r, resp) = ui.allocate_exact_size(vec2(MOD_IN, 64.0), Sense::hover());
    resp.on_hover_text(
        "The Tone curve: Low, Mid, High and Air as set (Air's lift on loud parts not shown)",
    );
    let pairs: Vec<(String, f64)> = fx
        .ctls
        .iter()
        .zip(&fx.vals)
        .map(|(c, v)| (c.key.clone(), f64::from(*v)))
        .collect();
    let curve = af2_fx::params_from(&fx.id, &pairs)
        .map(|p| af2_fx::effects::vocal::tone_curve(&p, 96))
        .unwrap_or_default();
    let p = ui.painter_at(r.expand(1.0));
    let rad = if th.bevel.is_some() { 0.0 } else { 4.0 };
    p.rect_filled(r, rad, c(th.wave_background));
    let x = |f: f64| r.left() + r.width() * ((f / 20.0).log10() / 3.0) as f32;
    let y =
        |db: f64| r.center().y - (db.clamp(-15.0, 15.0) / 15.0) as f32 * (r.height() / 2.0 - 4.0);
    let grid = Stroke::new(1.0_f32, ca(th.text, 18));
    p.line_segment(
        [pos2(r.left(), r.center().y), pos2(r.right(), r.center().y)],
        grid,
    );
    for f in [100.0, 1000.0, 10_000.0] {
        p.line_segment([pos2(x(f), r.top()), pos2(x(f), r.bottom())], grid);
    }
    if curve.len() > 1 {
        let pts: Vec<Pos2> = curve.iter().map(|q| pos2(x(q[0]), y(q[1]))).collect();
        if th.glow > 0.0 {
            p.add(Shape::line(
                pts.clone(),
                Stroke::new(5.0_f32, ca(th.accent, (40.0 * th.glow.min(1.0)) as u8)),
            ));
        }
        p.add(Shape::line(pts, Stroke::new(2.0_f32, c(th.accent))));
    }
    p.rect_stroke(r, rad, Stroke::new(1.0_f32, ca(th.border, 160)));
}

/// One of the Tone module's upright faders, its name and a value box
/// (double-click it to type a value).
fn tone_slider(ui: &mut egui::Ui, fx: &mut Fx, th: &Theme, key: &str, air: bool) {
    let Some(ctl) = fx.ctl(key).cloned() else {
        return;
    };
    let w = 50.0;
    ui.allocate_ui_with_layout(
        vec2(w, 150.0),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            ui.set_width(w);
            ui.spacing_mut().item_spacing.y = 2.0;
            let mut v = fx.get(key);
            let r = parts::vfader(
                ui,
                th,
                &mut v,
                ctl.min,
                ctl.max,
                ctl.default,
                vec2(22.0, 104.0),
                c(th.accent),
            )
            .on_hover_text(format!(
                "{}: {}. Double-click for the default.",
                ctl.name,
                crate::widgets::fmt_unit(v, &ctl.unit)
            ));
            if r.changed() {
                fx.set_live(key, v);
            }
            fx.track(&r);
            let name = caps(
                ui,
                &ctl.name,
                9.0,
                0.6,
                if air { c(th.accent) } else { c(th.text) },
            );
            let (nr, _) = ui.allocate_exact_size(vec2(w, 12.0), Sense::hover());
            ui.painter()
                .galley(nr.center() - name.size() / 2.0, name, c(th.text));
            let mut bv = fx.get(key);
            let unit = ctl.unit.clone();
            let b = crate::widgets::value_box(
                ui,
                fx.uid.with(("tone-box", key)),
                &mut bv,
                ctl.min,
                ctl.max,
                false,
                w,
                &|v| crate::widgets::fmt_unit(v, &unit),
            );
            if b.changed() {
                fx.set_live(key, bv);
            }
            fx.track(&b);
        },
    );
}

/// The Input and Output dials either side of the output level.
fn io_row(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = &env.th;
    let out = output_db(env);
    egui::Frame::none()
        .fill(lerp(th.wave_background, th.background, 0.4))
        .rounding(if th.bevel.is_some() { 0.0 } else { 6.0 })
        .inner_margin(egui::Margin::symmetric(PAD, 6.0))
        .show(ui, |ui| {
            let w = VOX_W - 2.0 * PAD;
            let lw = w - 2.0 * IO_SIDE - 2.0 * GAP_S;
            layout::row(
                ui,
                w,
                &[IO_SIDE, lw + 2.0 * GAP_S, IO_SIDE],
                |ui, i| match i {
                    0 => fx.knob(ui, "input", 44.0, "", &[]),
                    1 => ladder(ui, th, out, lw),
                    _ => fx.knob(ui, "output", 44.0, "", &[]),
                },
            );
        });
}

/// The output level as a row of 30 lamps (-60 to 0 dBFS, the last three
/// hot).
fn ladder(ui: &mut egui::Ui, th: &Theme, db: f64, w: f32) {
    let (r, resp) = ui.allocate_exact_size(vec2(w, 10.0), Sense::hover());
    resp.on_hover_text(if db <= -89.0 {
        "Output level (the rack's output)".to_string()
    } else {
        format!("Output level: {db:.1} dBFS (the rack's output)")
    });
    let p = ui.painter();
    let n = 30;
    let gap = 2.0;
    let seg = (w - gap * (n - 1) as f32) / n as f32;
    let lit = (((db + 60.0) / 60.0).clamp(0.0, 1.0) * n as f64) as f32;
    for k in 0..n {
        let x = r.left() + k as f32 * (seg + gap);
        let cell = Rect::from_min_size(pos2(x, r.top()), vec2(seg, r.height()));
        let col = if (k as f32) < lit {
            if k >= 27 {
                c(th.meter_high)
            } else {
                c(th.accent)
            }
        } else {
            lerp(th.panel_raised, th.background, 0.2)
        };
        p.rect_filled(cell, 1.0, col);
    }
}
