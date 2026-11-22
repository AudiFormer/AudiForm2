// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.
// Ported from Audiform (Audiform/src/panels/voice-chain/ModPanel.svelte), written by its author (AudiFormer)
// for Audiform and relicensed here by them. See docs/clean-room.md.

//! The Modulation window, as a piece of hardware, after the lead's
//! Audiform Modulation window (ModPanel.svelte), driving `af.modulation`.
//!
//! Across the top, the kind tabs (Chorus, Flanger, Phaser, Double Track),
//! each kind with its own faceplate colours blended into the theme. In
//! the middle, the big Rate and Depth dials either side of a lit display
//! that draws the movement over two turns: each chorus voice's swing, the
//! flanger's and phaser's sweep (the right side running behind by the
//! width). Under them, framed groups of small dials: the kind's own
//! settings, the Movement (feedback, width, room spread) and the Output.
//!
//! Double Track has its own layout: the Input switches (left, both,
//! right) and a Mono check across the top; a Source and a Copy strip
//! (pan, drive, level, Mute and polarity) either side of a display of
//! where the copy sits against the source (-20 to +20 ms, its sway
//! shaded), the big Offset dial and the LFO group.
//!
//! Picking a kind starts it from settings that suit it ([`START`]), set
//! in the same frame as the kind so it is one undoable step.

use egui::{pos2, vec2, Align2, Color32, FontId, Layout, Pos2, Rect, Rounding, Shape, Stroke};

use af2_fx::effects::modulation::{ADT_MS, KINDS};

use super::layout::{self, CELL, GAP, GAP_S, HERO, KNOB, PAD, SIDE};
use super::space::{self, Look};
use super::{Env, Fx};
use crate::theme::{c, Theme};
use crate::widgets;

/// Double Track's place in [`KINDS`].
const ADT: usize = 3;

/// The tabs' tips.
const TIPS: [&str; 4] = [
    "A few copies drifting in time and pitch: thicker, wider, shimmering",
    "A short swept delay fed back on itself: the jet sweep",
    "Moving notches from a chain of all-pass filters: the swirling sweep",
    "A second take on a second tape machine: the source and its copy each panned, driven and leveled, the copy up to 20 ms early or late and swaying",
];

/// The shared settings each kind starts from when its tab is picked.
const START: [&[(&str, f64)]; 4] = [
    &[("rate", 0.6), ("depth", 50.0), ("feedback", 0.0), ("wet", 50.0)],
    &[("rate", 0.25), ("depth", 70.0), ("feedback", 50.0), ("wet", 50.0)],
    &[("rate", 0.4), ("depth", 60.0), ("feedback", 30.0), ("wet", 100.0)],
    &[("adt_offset", 12.0), ("lfo_range", 1.0), ("lfo_rate", 0.4)],
];

/// The dials shown under each kind beyond the shared ones, with their
/// short names. (Double Track lays its own out.)
const OWN: [&[(&str, &str)]; 4] = [
    &[("voices", "Voices"), ("chorus_delay", "Delay")],
    &[("flange_delay", "Delay")],
    &[("stages", "Stages"), ("center", "Center")],
    &[("adt_offset", "Offset"), ("lfo_range", "Range")],
];

const MOVEMENT: [(&str, &str); 3] = [
    ("feedback", "Feedback"),
    ("width", "Width"),
    ("spread", "Spread"),
];
const OUTPUT: [(&str, &str); 2] = [("dry", "Dry"), ("wet", "Wet")];

/// Double Track's strips: their heading and key prefix.
const STRIPS: [(&str, &str); 2] = [("Source", "src"), ("Copy", "adt")];
const INPUTS: [&str; 3] = ["Left", "L + R", "Right"];
const LFO: [(&str, &str); 3] = [
    ("lfo_range", "Range"),
    ("lfo_rate", "Rate"),
    ("lfo_shape", "Shape"),
];

/// Controls that take whole steps, put on one when let go.
const STEPPED: [&str; 3] = ["voices", "stages", "lfo_shape"];

/// Each kind's faceplate (Audiform's colours): face, lower face, ink, glow.
const LOOKS: [Look; 4] = [
    Look {
        face: [0x1b, 0x2a, 0x2c],
        face2: [0x0f, 0x19, 0x1a],
        ink: [0xd3, 0xee, 0xee],
        glow: [0x4f, 0xe0, 0xd2],
    },
    Look {
        face: [0x2a, 0x20, 0x33],
        face2: [0x17, 0x11, 0x1d],
        ink: [0xe8, 0xdc, 0xf3],
        glow: [0xc3, 0x8b, 0xff],
    },
    Look {
        face: [0x33, 0x26, 0x1a],
        face2: [0x1c, 0x14, 0x0d],
        ink: [0xf3, 0xe3, 0xcf],
        glow: [0xff, 0x9d, 0x4a],
    },
    Look {
        face: [0x20, 0x2b, 0x1d],
        face2: [0x12, 0x19, 0x10],
        ink: [0xdc, 0xef, 0xd6],
        glow: [0x9b, 0xe0, 0x6a],
    },
];

/// The face's width: the widest row is Chorus's or Phaser's seven small
/// dials in three groups; the display between the big dials takes the rest.
const W: f32 = {
    let row = 7.0 * CELL + 2.0 * GAP + 6.0 * PAD;
    let hero = space::hero_w(256.0);
    if row > hero {
        row
    } else {
        hero
    }
};
const DISPLAY: f32 = W - 2.0 * SIDE - 2.0 * GAP;
/// Double Track's strips, and the column between them.
const STRIP_W: f32 = CELL + 2.0 * PAD + 8.0;
const CENTRE_W: f32 = W - 2.0 * STRIP_W - 2.0 * GAP;
const TAB_GAP: f32 = 4.0;

/// A control's value as its dial shows it.
fn show(key: &str, v: f64, unit: &str, choices: &[String]) -> String {
    if !choices.is_empty() {
        let i = (v.round().max(0.0) as usize).min(choices.len() - 1);
        return choices[i].clone();
    }
    match key {
        "rate" | "lfo_rate" if v < 10.0 => format!("{v:.2} Hz"),
        "rate" | "lfo_rate" => format!("{v:.1} Hz"),
        "chorus_delay" => format!("{v:.1} ms"),
        "flange_delay" | "lfo_range" => format!("{v:.2} ms"),
        "adt_offset" => offset_text(v),
        "voices" => format!("{v:.0}"),
        "src_pan" | "adt_pan" => pan_text(v),
        _ => widgets::fmt_unit(v, unit),
    }
}

/// A pan: C, or the side and how far.
fn pan_text(v: f64) -> String {
    let n = v.round();
    if n < 0.0 {
        format!("L{:.0}", -n)
    } else if n > 0.0 {
        format!("R{n:.0}")
    } else {
        "C".into()
    }
}

/// Double Track's offset, signed.
fn offset_text(v: f64) -> String {
    format!("{}{v:.2} ms", if v > 0.0 { "+" } else { "" })
}

/// Where the copy sits, in words.
fn offset_words(v: f64) -> &'static str {
    if v < 0.0 {
        "copy early"
    } else if v > 0.0 {
        "copy late"
    } else {
        "together"
    }
}

/// A dial for `key`, its value shown as [`show`] has it.
fn dial(ui: &mut egui::Ui, fx: &mut Fx, key: &str, size: f32, label: &str, marks: &[f64]) {
    let Some(ctl) = fx.ctl(key) else { return };
    let (unit, choices) = (ctl.unit.clone(), ctl.choices.clone());
    fx.knob_fmt(ui, key, size, label, marks, &|v| {
        show(key, v, &unit, &choices)
    });
}

/// The movement over two turns, one line per voice, each point 0..1 of
/// the way across and -1..1 up (before the depth).
fn movement(kind: usize, voices: usize, width: f64) -> Vec<Vec<(f64, f64)>> {
    const STEPS: usize = 96;
    let n = match kind {
        0 => voices.max(1),
        ADT => 1,
        _ => 2,
    };
    (0..n)
        .map(|v| {
            // Chorus voices spread round the turn; the right side runs
            // behind by the width.
            let off = if kind == 0 {
                v as f64 / n as f64
            } else {
                v as f64 * width * 0.25
            };
            (0..=STEPS)
                .map(|i| {
                    let x = i as f64 / STEPS as f64;
                    let t = x * 2.0;
                    let y = if kind == ADT {
                        (t * 5.1).sin() * 0.5
                            + (t * 2.3 + 1.0).sin() * 0.35
                            + (t * 9.7 + 2.0).sin() * 0.15
                    } else {
                        ((t + off) * std::f64::consts::TAU).sin()
                    };
                    (x, y)
                })
                .collect()
        })
        .collect()
}

/// A lit line with a soft glow under it.
fn glow_line(p: &egui::Painter, pts: Vec<Pos2>, light: Color32, width: f32) {
    p.add(Shape::line(
        pts.clone(),
        Stroke::new(width + 3.0, light.gamma_multiply(0.18)),
    ));
    p.add(Shape::line(pts, Stroke::new(width, light)));
}

pub fn panel(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let kind = (fx.get("kind").round().max(0.0) as usize).min(KINDS.len() - 1);
    let th = space::tinted(&env.th, &LOOKS[kind]);
    space::with_theme(ui, &th, |ui| {
        space::plate(ui, &th, W, |ui| {
            let picked = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = TAB_GAP;
                    let mut picked = None;
                    for (i, name) in KINDS.iter().enumerate() {
                        if space::key(ui, &th, name, i == kind, true)
                            .on_hover_text(TIPS[i])
                            .clicked()
                            && i != kind
                        {
                            picked = Some(i);
                        }
                    }
                    picked
                })
                .inner;
            if let Some(i) = picked {
                // The kind and its start, in one frame: one undo step.
                fx.set("kind", i as f64);
                for (k, v) in START[i] {
                    fx.set(k, *v);
                }
            }
            ui.add_space(GAP);
            if kind == ADT {
                double_track(ui, fx, &th);
            } else {
                sweeps(ui, fx, &th, kind);
                if env.spatial {
                    ui.add_space(GAP_S);
                    layout::note(
                        ui,
                        W,
                        egui::RichText::new(
                            "On the speaker bed: every speaker has its own moving copy, each a step further round the turn, so the movement travels round the room; Spread feeds each copy its neighbours' sound.",
                        )
                        .size(10.5)
                        .color(c(th.accent_alt)),
                    );
                }
            }
        });
    });
    // Whole-step controls land on a step when let go.
    if fx.released {
        for key in STEPPED {
            let v = fx.get(key);
            if v != v.round() {
                fx.set(key, v.round());
            }
        }
    }
}

/// Chorus, Flanger and Phaser: the big dials round the display, then the
/// kind's own dials, the movement and the levels.
fn sweeps(ui: &mut egui::Ui, fx: &mut Fx, th: &Theme, kind: usize) {
    let fmt = |fx: &Fx, k: &str| {
        let v = fx.get(k);
        fx.ctl(k)
            .map_or_else(String::new, |c| show(k, v, &c.unit, &c.choices))
    };
    layout::hero(ui, W, SIDE, |ui, i| match i {
        0 => dial(ui, fx, "rate", HERO, "Rate", &[0.1, 1.0, 10.0]),
        1 => {
            let detail = match kind {
                0 => format!(
                    "{:.0} voices · {}",
                    fx.get("voices").round(),
                    fmt(fx, "chorus_delay")
                ),
                1 => format!(
                    "{} · feedback {:.0}%",
                    fmt(fx, "flange_delay"),
                    fx.get("feedback")
                ),
                _ => format!("{} stages · {}", fmt(fx, "stages"), fmt(fx, "center")),
            };
            let depth = (fx.get("depth") / 100.0) as f32;
            let lines = movement(
                kind,
                fx.get("voices").round().max(1.0) as usize,
                fx.get("width") / 100.0,
            );
            space::screen(
                ui,
                th,
                vec2(DISPLAY, 128.0),
                c(th.accent),
                &fmt(fx, "rate"),
                &detail,
                |p, r, light| {
                    let mid = r.center().y;
                    let half = r.height() / 2.0 - 3.0;
                    p.line_segment(
                        [pos2(r.left(), mid), pos2(r.right(), mid)],
                        Stroke::new(1.0_f32, light.gamma_multiply(0.3)),
                    );
                    for (n, line) in lines.iter().enumerate() {
                        let pts = line
                            .iter()
                            .map(|(x, y)| {
                                pos2(
                                    r.left() + r.width() * *x as f32,
                                    mid - *y as f32 * depth * half,
                                )
                            })
                            .collect();
                        let fade = (1.0 - n as f32 * 0.12).max(0.2);
                        glow_line(p, pts, light.gamma_multiply(fade), 1.6);
                    }
                },
            );
        }
        _ => dial(ui, fx, "depth", HERO, "Depth", &[0.0, 50.0, 100.0]),
    });
    ui.add_space(GAP);
    let row: [(&str, &[(&str, &str)]); 3] = [
        (KINDS[kind], OWN[kind]),
        ("Movement", &MOVEMENT),
        ("Output", &OUTPUT),
    ];
    let items: Vec<(&str, f32)> = row.iter().map(|g| (g.0, g.1.len() as f32)).collect();
    layout::groups(ui, th, W, &items, |ui, g| {
        let knobs = row[g].1;
        let iw = ui.available_width();
        layout::cells(ui, iw, knobs.len(), |ui, k| {
            let (key, name) = knobs[k];
            if key == "spread" {
                ui.scope(|ui| dial(ui, fx, key, KNOB, name, &[]))
                    .response
                    .on_hover_text(
                        "On a surround bed or a 3D Space master: how much of each speaker's neighbours its moving copy takes in, so the movement spreads round the room. Stereo ignores it.",
                    );
            } else {
                dial(ui, fx, key, KNOB, name, &[]);
            }
        });
    });
}

/// Double Track: the input and mono switches, then the Source and Copy
/// strips either side of the copy's place, the Offset dial and the LFO.
fn double_track(ui: &mut egui::Ui, fx: &mut Fx, th: &Theme) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = TAB_GAP;
        let input = fx.get("adt_input").round() as usize;
        for (i, name) in INPUTS.iter().enumerate() {
            if space::key(ui, th, name, input == i, true)
                .on_hover_text("What the source is: the left side, both, or the right side")
                .clicked()
            {
                fx.set("adt_input", i as f64);
            }
        }
        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
            let mono = fx.on("adt_mono");
            if space::key(ui, th, "Mono", mono, true)
                .on_hover_text(
                    "Hear the source and copy folded to mono, to check how they sit together",
                )
                .clicked()
            {
                fx.set("adt_mono", if mono { 0.0 } else { 1.0 });
            }
        });
    });
    ui.add_space(GAP);
    // The strips stretch to the centre column's height (last frame's).
    let centre_h = layout::last_height(ui, "adt-centre", None);
    let hs = layout::columns(ui, &[STRIP_W, CENTRE_W, STRIP_W], GAP, |ui, col| {
        if col == 1 {
            centre(ui, fx, th);
            return;
        }
        let (name, k) = STRIPS[col / 2];
        layout::groups_min(ui, th, STRIP_W, &[(name, 1.0)], centre_h, |ui, _| {
            let iw = ui.available_width();
            for (part, label) in [("pan", "Pan"), ("drive", "Drive"), ("level", "Level")] {
                let key = format!("{k}_{part}");
                let marks: &[f64] = if part == "pan" { &[-100.0, 0.0, 100.0] } else { &[] };
                layout::centred(ui, iw, |ui| dial(ui, fx, &key, KNOB, label, marks));
                ui.add_space(GAP_S / 2.0);
            }
            let lower = name.to_lowercase();
            layout::centred(ui, iw, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = TAB_GAP;
                    for (part, label, tip) in [
                        ("mute", "Mute", format!("Mute the {lower}")),
                        ("invert", "Ø", format!("Flip the {lower}'s polarity")),
                    ] {
                        let key = format!("{k}_{part}");
                        let on = fx.on(&key);
                        if space::key(ui, th, label, on, true)
                            .on_hover_text(tip)
                            .clicked()
                        {
                            fx.set(&key, if on { 0.0 } else { 1.0 });
                        }
                    }
                });
            });
        });
    });
    layout::last_height(ui, "adt-centre", hs.get(1).copied());
}

/// Double Track's centre: the copy's place, the Offset dial, the LFO.
fn centre(ui: &mut egui::Ui, fx: &mut Fx, th: &Theme) {
    let off = fx.get("adt_offset");
    let range = fx.get("lfo_range");
    let ink = c(th.text).gamma_multiply(0.7);
    space::screen(
        ui,
        th,
        vec2(CENTRE_W, 124.0),
        c(th.accent),
        &offset_text(off),
        offset_words(off),
        |p, r, light| {
            let x = |ms: f64| r.center().x + (ms / ADT_MS) as f32 * (r.width() / 2.0 - 8.0);
            let axis = r.bottom() - 17.0;
            p.line_segment(
                [pos2(r.left(), axis), pos2(r.right(), axis)],
                Stroke::new(1.0_f32, light.gamma_multiply(0.3)),
            );
            for t in (-4..=4).map(|i| f64::from(i) * 5.0) {
                let tx = x(t);
                p.line_segment(
                    [pos2(tx, axis - 4.0), pos2(tx, axis + 4.0)],
                    Stroke::new(1.0_f32, light.gamma_multiply(0.45)),
                );
                p.text(
                    pos2(tx, r.bottom() - 2.0),
                    Align2::CENTER_BOTTOM,
                    format!("{t:.0}"),
                    FontId::proportional(8.5),
                    light.gamma_multiply(0.6),
                );
            }
            let (top, bottom) = (r.top() + 2.0, axis - 5.0);
            let (a, b) = (x(off - range), x(off + range));
            p.rect_filled(
                Rect::from_min_max(pos2(a, top + 2.0), pos2(b.max(a + 1.0), bottom)),
                Rounding::ZERO,
                light.gamma_multiply(0.18),
            );
            let mark = |at: f32, col: Color32| {
                p.rect_filled(
                    Rect::from_min_max(pos2(at - 1.5, top), pos2(at + 1.5, bottom)),
                    Rounding::ZERO,
                    col,
                );
            };
            mark(x(0.0), ink);
            mark(x(off), light);
        },
    );
    ui.add_space(GAP_S);
    layout::centred(ui, CENTRE_W, |ui| {
        dial(ui, fx, "adt_offset", 96.0, "Offset", &[-ADT_MS, 0.0, ADT_MS]);
    });
    ui.add_space(GAP_S);
    layout::groups(ui, th, CENTRE_W, &[("LFO", 1.0)], |ui, _| {
        let iw = ui.available_width();
        layout::cells(ui, iw, LFO.len(), |ui, k| {
            let (key, name) = LFO[k];
            dial(ui, fx, key, KNOB, name, &[]);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_the_window_uses_is_a_control() {
        let info = af2_fx::effects::modulation::modulation_info();
        let has = |k: &str| info.params.iter().any(|p| p.key == k);
        let range = |k: &str| {
            info.params
                .iter()
                .find(|p| p.key == k)
                .map(|p| (f64::from(p.min), f64::from(p.max)))
                .unwrap()
        };
        assert_eq!(KINDS.len(), TIPS.len());
        assert_eq!(KINDS.len(), LOOKS.len());
        for own in OWN {
            for (k, _) in own {
                assert!(has(k), "no {k}");
            }
        }
        for (k, _) in MOVEMENT.iter().chain(&OUTPUT).chain(&LFO) {
            assert!(has(k), "no {k}");
        }
        for (_, s) in STRIPS {
            for part in ["pan", "drive", "level", "mute", "invert"] {
                assert!(has(&format!("{s}_{part}")), "no {s}_{part}");
            }
        }
        for k in ["kind", "rate", "depth", "adt_input", "adt_mono"].iter().chain(&STEPPED) {
            assert!(has(k), "no {k}");
        }
        // Each kind's start is within its controls' ranges.
        for start in START {
            for (k, v) in start {
                let (lo, hi) = range(k);
                assert!(has(k) && (lo..=hi).contains(v), "{k} = {v}");
            }
        }
    }

    #[test]
    fn readouts_and_movement() {
        assert_eq!(pan_text(0.0), "C");
        assert_eq!(pan_text(-40.0), "L40");
        assert_eq!(pan_text(25.0), "R25");
        assert_eq!(offset_text(12.0), "+12.00 ms");
        assert_eq!(offset_text(-3.5), "-3.50 ms");
        assert_eq!(offset_words(0.0), "together");
        assert_eq!(show("rate", 0.6, "Hz", &[]), "0.60 Hz");
        // One line per chorus voice, two for the sweeps, one for the wander.
        assert_eq!(movement(0, 4, 1.0).len(), 4);
        assert_eq!(movement(1, 4, 1.0).len(), 2);
        assert_eq!(movement(ADT, 4, 1.0).len(), 1);
        assert!(movement(2, 3, 0.5)
            .iter()
            .flatten()
            .all(|(x, y)| (0.0..=1.0).contains(x) && y.abs() <= 1.0 + 1e-9));
    }
}
