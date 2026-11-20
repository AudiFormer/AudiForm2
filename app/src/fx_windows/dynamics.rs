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

/// The middle of a unit: the needle over the transfer graph and bars.
fn centre(
    ui: &mut egui::Ui,
    env: &Env,
    reading: f64,
    caption: &str,
    curve: &dyn Fn(f64) -> f64,
    threshold: Option<f64>,
) {
    ui.vertical(|ui| {
        parts::needle(ui, &env.th, env.key.with("needle"), reading, caption, 232.0);
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            parts::transfer(ui, &env.th, 112.0, curve, threshold, input_db(env));
            ui.add_space(8.0);
            parts::io_bars(ui, env, 82.0);
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
    parts::faceplate(ui, &env.th, "Compressor", "levels the loud parts", |ui| {
        ui.horizontal_top(|ui| {
            fx.knob(ui, "Threshold", 108.0, "", &[-60.0, -40.0, -20.0, 0.0]);
            centre(
                ui,
                env,
                -gr,
                "GAIN REDUCTION",
                &|i| i - comp_gr(i, thr, ratio, knee) + mk,
                Some(thr),
            );
            fx.knob_fmt(ui, "Ratio", 108.0, "", &[1.0, 5.0, 10.0, 20.0], &|v| {
                format!("{v:.1}:1")
            });
        });
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            for k in ["Attack", "Release", "Knee", "Makeup"] {
                fx.knob(ui, k, 62.0, "", &[]);
                ui.add_space(6.0);
            }
        });
    });
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
        |ui| {
            ui.horizontal_top(|ui| {
                fx.knob(
                    ui,
                    "Input boost",
                    108.0,
                    "Boost",
                    &[0.0, 6.0, 12.0, 18.0, 24.0],
                );
                centre(
                    ui,
                    env,
                    -gr,
                    "GAIN REDUCTION",
                    &|i| (i + boost).min(ceil),
                    Some(ceil - boost),
                );
                fx.knob(ui, "Ceiling", 108.0, "", &[-24.0, -12.0, -6.0, 0.0]);
            });
            ui.add_space(4.0);
            ui.horizontal_top(|ui| {
                fx.knob(ui, "Look-ahead", 62.0, "", &[]);
                ui.add_space(6.0);
                fx.knob(ui, "Release", 62.0, "", &[]);
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
    parts::faceplate(ui, &env.th, "Noise Gate", "closes in the quiet", |ui| {
        ui.horizontal_top(|ui| {
            fx.knob(
                ui,
                "Threshold",
                108.0,
                "",
                &[-80.0, -60.0, -40.0, -20.0, 0.0],
            );
            centre(
                ui,
                env,
                -gr,
                "ATTENUATION",
                &|i| if i < thr { i + floor } else { i },
                Some(thr),
            );
            fx.knob(ui, "Floor", 108.0, "", &[-80.0, -40.0, 0.0]);
        });
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            for k in ["Attack", "Hold", "Release"] {
                fx.knob(ui, k, 62.0, "", &[]);
                ui.add_space(6.0);
            }
        });
    });
}

pub fn deesser(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let (thr, red) = (fx.get("Threshold"), fx.get("Reduction"));
    let target = input_db(env).map_or(0.0, |i| (i - thr).clamp(0.0, red));
    let gr = parts::est_gr(ui, env.key.with("gr"), target, 2.0, 60.0);
    parts::faceplate(ui, &env.th, "De-Esser", "tames the s sounds", |ui| {
        ui.horizontal_top(|ui| {
            fx.knob(ui, "Threshold", 108.0, "", &[-60.0, -40.0, -20.0, 0.0]);
            centre(
                ui,
                env,
                -gr,
                "SIBILANCE CUT",
                &|i| i - (i - thr).clamp(0.0, red),
                Some(thr),
            );
            fx.knob(ui, "Reduction", 108.0, "", &[0.0, 6.0, 12.0, 18.0, 24.0]);
        });
    });
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
    parts::faceplate(
        ui,
        &env.th,
        "Opto Leveler",
        "optical leveling amplifier",
        |ui| {
            ui.horizontal_top(|ui| {
                fx.knob_fmt(ui, "gain", 112.0, "Gain", &NUMS, &|v| format!("{v:.1}"));
                ui.vertical(|ui| {
                    let f1 = opto_gr;
                    let snap: Vec<f64> = (0..=60).map(|k| -60.0 + k as f64).collect();
                    let table: Vec<f64> = snap.iter().map(|i| i - f1(fx, *i) + makeup).collect();
                    centre(
                        ui,
                        env,
                        reading,
                        if out_mode { "OUTPUT" } else { "GAIN REDUCTION" },
                        &|i| table[((i + 60.0).round() as usize).min(60)],
                        Some(-fx.get("peak") * 0.6),
                    );
                    ui.add_space(2.0);
                    lever(ui, fx, "mode", "Compress", "Limit");
                    lever(ui, fx, "meter", "Reduction", "Output");
                });
                fx.knob_fmt(ui, "peak", 112.0, "Peak Reduction", &NUMS, &|v| {
                    format!("{v:.1}")
                });
            });
            ui.horizontal_top(|ui| {
                for k in ["emphasis", "tube", "mix"] {
                    fx.knob(ui, k, 62.0, "", &[]);
                    ui.add_space(6.0);
                }
            });
            ui.label(
            RichText::new(
                "Turn up Peak Reduction until the needle shows a few dB of leveling, then set Gain for the level you want. Emphasis makes it listen harder to s sounds.",
            )
            .size(10.5)
            .color(c(env.th.text_dim)),
        );
        },
    );
}

pub fn preamp(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let reading = output_db(env) + 18.0;
    parts::faceplate(
        ui,
        &env.th,
        "Tube Preamp",
        "vintage tube preamplifier",
        |ui| {
            ui.horizontal_top(|ui| {
                fx.knob(ui, "input", 100.0, "Input", &[-12.0, 0.0, 12.0, 24.0]);
                fx.knob_fmt(ui, "drive", 100.0, "Drive", &NUMS, &|v| format!("{v:.0}"));
                ui.vertical(|ui| {
                    parts::needle(
                        ui,
                        &env.th,
                        env.key.with("needle"),
                        reading,
                        "OUTPUT (0 = -18 dBFS)",
                        220.0,
                    );
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        parts::io_bars(ui, env, 70.0);
                        ui.add_space(10.0);
                        fx.toggle(ui, "transformer", "Transformer");
                    });
                });
                fx.knob(ui, "output", 100.0, "Output", &[-24.0, -12.0, 0.0, 12.0]);
            });
            ui.horizontal_top(|ui| {
                for k in ["low", "high", "mix"] {
                    fx.knob(ui, k, 62.0, "", &[]);
                    ui.add_space(6.0);
                }
            });
            ui.label(
                RichText::new("Drive adds warmth; past halfway it starts to sound overdriven.")
                    .size(10.5)
                    .color(c(env.th.text_dim)),
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

pub fn vox(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = env.th.clone();
    parts::faceplate(
        ui,
        &th,
        "Vox Strip",
        "dynamics, tone and effects in signal order",
        |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    fx.knob(ui, "input", 70.0, "Input", &[]);
                    fx.toggle(ui, "low_cut", "Low Cut");
                });
                for (n, (title, power, style, knobs)) in [
                    ("Dynamics", "dyn_on", "dyn_style", &["comp", "deess"][..]),
                    (
                        "Tone",
                        "tone_on",
                        "tone_style",
                        &["low", "mid", "high", "air"][..],
                    ),
                    ("Effects", "fx_on", "fx_style", &["fx"][..]),
                ]
                .into_iter()
                .enumerate()
                {
                    parts::module(ui, &th, &format!("{} {title}", n + 1), |ui| {
                        fx.toggle(ui, power, "On");
                        let on = fx.on(power);
                        ui.add_enabled_ui(on, |ui| {
                            stepper(ui, fx, style);
                            ui.horizontal_top(|ui| {
                                for k in knobs {
                                    fx.knob(ui, k, 58.0, "", &[]);
                                }
                            });
                        });
                    });
                }
                ui.vertical(|ui| {
                    fx.knob(ui, "output", 70.0, "Output", &[]);
                    parts::io_bars(ui, env, 60.0);
                });
            });
        },
    );
}
