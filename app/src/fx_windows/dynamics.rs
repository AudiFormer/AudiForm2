// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The dynamics and mastering units: AudiForm 2's own Compressor,
//! Limiter, Noise Gate and De-Esser faces, and the Opto Leveler, Tube
//! Preamp and Vox Strip faces after the lead's Audiform panels (big
//! numbered dials, a needle meter, lever switches, small dials for the
//! rest; the Vox Strip's three modules in signal order).
//!
//! The needles show the reduction worked out from the rack's input level
//! and the unit's own curve and timing (or the rack's output level); the
//! bars show the rack's input and output.

use egui::RichText;

use super::layout::{self, CELL, GAP, GAP_S, HERO, KNOB, PAD, SIDE};
use super::parts::{self, comp_gr};
use super::{Env, Fx};
use crate::theme::c;

const NUMS: [f64; 6] = [0.0, 20.0, 40.0, 60.0, 80.0, 100.0];

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

/// The Opto Leveler's static reduction (its curve: Peak Reduction lowers
/// the threshold to 60 dB down; Compress 3:1 with a 12 dB knee, Limit
/// 20:1 with 3 dB).
fn opto_gr(fx: &Fx, input: f64) -> f64 {
    let limit = fx.get("mode").round() as i64 == 1;
    let thr = -fx.get("peak") * 0.6;
    if limit {
        comp_gr(input, thr, 20.0, 3.0)
    } else {
        comp_gr(input, thr, 3.0, 12.0)
    }
}

/// A two-way lever: `key`'s choice 0 or 1, named on either side.
fn lever(ui: &mut egui::Ui, fx: &mut Fx, key: &str, a: &str, b: &str) {
    ui.horizontal(|ui| {
        let mut on = fx.get(key).round() as i64 == 1;
        ui.label(RichText::new(a).size(10.5));
        if crate::widgets::switch(ui, &mut on, b).changed() {
            fx.set(key, if on { 1.0 } else { 0.0 });
        }
    });
}

pub fn opto(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let target = input_db(env).map_or(0.0, |i| opto_gr(fx, i));
    let gr = parts::est_gr(ui, env.key.with("gr"), target, 10.0, 300.0);
    let out_mode = fx.get("meter").round() as i64 == 1;
    let reading = if out_mode { output_db(env) + 18.0 } else { -gr };
    let makeup = (fx.get("gain") - 40.0) * 0.5;
    let th = env.th.clone();
    parts::faceplate(
        ui,
        &th,
        "Opto Leveler",
        "optical leveling amplifier",
        UNIT_W,
        |ui| {
            layout::hero(ui, UNIT_W, SIDE, |ui, i| match i {
                0 => fx.knob_fmt(ui, "gain", HERO, "Gain", &NUMS, &|v| format!("{v:.1}")),
                1 => {
                    let snap: Vec<f64> = (0..=60).map(|k| -60.0 + k as f64).collect();
                    let table: Vec<f64> =
                        snap.iter().map(|i| i - opto_gr(fx, *i) + makeup).collect();
                    centre(
                        ui,
                        env,
                        reading,
                        if out_mode { "OUTPUT" } else { "GAIN REDUCTION" },
                        &|i| table[((i + 60.0).round() as usize).min(60)],
                        Some(-fx.get("peak") * 0.6),
                    );
                }
                _ => fx.knob_fmt(ui, "peak", HERO, "Peak Reduction", &NUMS, &|v| {
                    format!("{v:.1}")
                }),
            });
            ui.add_space(GAP_S);
            layout::cells(ui, UNIT_W, 2, |ui, i| match i {
                0 => lever(ui, fx, "mode", "Compress", "Limit"),
                _ => lever(ui, fx, "meter", "Reduction", "Output"),
            });
            ui.add_space(GAP);
            let keys = ["emphasis", "tube", "mix"];
            layout::cells(ui, UNIT_W, keys.len(), |ui, i| {
                fx.knob(ui, keys[i], KNOB, "", &[]);
            });
            ui.add_space(GAP_S);
            layout::note(
                ui,
                UNIT_W,
                RichText::new(
                    "Turn up Peak Reduction until the needle shows a few dB of leveling, then set Gain for the level you want. Emphasis makes it listen harder to s sounds.",
                )
                .size(10.5)
                .color(c(th.text_dim)),
            );
        },
    );
}

/// The Tube Preamp's side columns and width: two big dials either side
/// of the needle.
const PRE_SIDE: f32 = 112.0;
const PRE_W: f32 = 4.0 * PRE_SIDE + 256.0;

pub fn preamp(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let reading = output_db(env) + 18.0;
    let th = env.th.clone();
    parts::faceplate(
        ui,
        &th,
        "Tube Preamp",
        "vintage tube preamplifier",
        PRE_W,
        |ui| {
            // In signal order: Input and Drive, the needle with the tone
            // either side of the level bars, Output and Mix.
            let s = PRE_SIDE;
            let cw = PRE_W - 4.0 * s;
            layout::row(ui, PRE_W, &[s, s, cw, s, s], |ui, i| match i {
                0 => fx.knob(ui, "input", HERO, "Input", &[-12.0, 0.0, 12.0, 24.0]),
                1 => fx.knob_fmt(ui, "drive", HERO, "Drive", &NUMS, &|v| format!("{v:.0}")),
                2 => {
                    layout::centred(ui, cw, |ui| {
                        parts::needle(
                            ui,
                            &th,
                            env.key.with("needle"),
                            reading,
                            "OUTPUT (0 = -18 dBFS)",
                            METER_W,
                        );
                    });
                    ui.add_space(GAP_S);
                    layout::row(ui, cw, &[1.0, 0.55, 0.55, 1.0], |ui, k| match k {
                        0 => fx.knob(ui, "low", KNOB, "", &[]),
                        1 => parts::level_bar(ui, env, false, 56.0),
                        2 => parts::level_bar(ui, env, true, 56.0),
                        _ => fx.knob(ui, "high", KNOB, "", &[]),
                    });
                    ui.add_space(GAP_S);
                    layout::centred(ui, cw, |ui| fx.toggle(ui, "transformer", "Transformer"));
                }
                3 => fx.knob(ui, "output", HERO, "Output", &[-24.0, -12.0, 0.0, 12.0]),
                _ => {
                    // Numbered like its neighbours, so it is drawn the same size.
                    let (lo, hi) = fx.ctl("mix").map_or((0.0, 100.0), |c| (c.min, c.max));
                    fx.knob(ui, "mix", HERO, "", &[lo, (lo + hi) / 2.0, hi]);
                }
            });
            ui.add_space(GAP_S);
            layout::note(
                ui,
                PRE_W,
                RichText::new("Drive adds warmth; past halfway it starts to sound overdriven.")
                    .size(10.5)
                    .color(c(th.text_dim)),
            );
        },
    );
}

/// A style stepper: ◀ name ▶ through a choice control.
fn stepper(ui: &mut egui::Ui, fx: &mut Fx, key: &str) {
    let Some(ctl) = fx.ctl(key).cloned() else {
        return;
    };
    let n = ctl.choices.len().max(1);
    let cur = ((fx.get(key) - ctl.min).round().max(0.0) as usize).min(n - 1);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if crate::widgets::button(ui, "◀", false).clicked() {
            fx.set(key, ctl.min + ((cur + n - 1) % n) as f64);
        }
        let (r, _) = ui.allocate_exact_size(egui::vec2(96.0, 22.0), egui::Sense::hover());
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            ctl.choices.get(cur).map_or("", String::as_str),
            egui::FontId::proportional(11.5),
            c(crate::widgets::theme(ui).accent),
        );
        if crate::widgets::button(ui, "▶", false).clicked() {
            fx.set(key, ctl.min + ((cur + 1) % n) as f64);
        }
    });
}

/// The Vox Strip's input and output columns.
const VOX_SIDE: f32 = 96.0;
/// Its three modules' widths (by what each holds: a stepper and its
/// dials, every dial in a cell of the same width).
const VOX_MODS: [f32; 3] = [
    2.0 * CELL + 2.0 * PAD,
    4.0 * CELL + 2.0 * PAD,
    2.0 * CELL + 2.0 * PAD,
];
const VOX_MID: f32 = VOX_MODS[0] + VOX_MODS[1] + VOX_MODS[2] + 2.0 * GAP;
const VOX_W: f32 = 2.0 * (VOX_SIDE + GAP_S) + VOX_MID;

pub fn vox(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = env.th.clone();
    let mods: [(&str, &str, &str, &[&str]); 3] = [
        ("1 Dynamics", "dyn_on", "dyn_style", &["comp", "deess"]),
        (
            "2 Tone",
            "tone_on",
            "tone_style",
            &["low", "mid", "high", "air"],
        ),
        ("3 Effects", "fx_on", "fx_style", &["fx"]),
    ];
    parts::faceplate(
        ui,
        &th,
        "Vox Strip",
        "dynamics, tone and effects in signal order",
        VOX_W,
        |ui| {
            let side = VOX_SIDE + GAP_S;
            layout::row(ui, VOX_W, &[side, VOX_MID, side], |ui, i| match i {
                0 => {
                    layout::centred(ui, side, |ui| fx.knob(ui, "input", 72.0, "Input", &[]));
                    ui.add_space(GAP_S);
                    layout::centred(ui, side, |ui| fx.toggle(ui, "low_cut", "Low Cut"));
                }
                1 => {
                    let items: Vec<(&str, f32)> =
                        mods.iter().zip(VOX_MODS).map(|(m, w)| (m.0, w)).collect();
                    layout::groups(ui, &th, VOX_MID, &items, |ui, g| {
                        let (_, power, style, knobs) = mods[g];
                        let iw = ui.available_width();
                        layout::centred(ui, iw, |ui| fx.toggle(ui, power, "On"));
                        let on = fx.on(power);
                        ui.add_enabled_ui(on, |ui| {
                            layout::centred(ui, iw, |ui| stepper(ui, fx, style));
                            ui.add_space(GAP_S);
                            layout::cells(ui, iw, knobs.len(), |ui, k| {
                                fx.knob(ui, knobs[k], KNOB, "", &[]);
                            });
                        });
                    });
                }
                _ => {
                    layout::centred(ui, side, |ui| {
                        fx.knob(ui, "output", 72.0, "Output", &[]);
                    });
                    ui.add_space(GAP_S);
                    layout::centred(ui, side, |ui| parts::io_bars(ui, env, 56.0));
                }
            });
        },
    );
}
