// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The Delay and Echo faceplate, after the lead's Audiform Delay window
//! (DelayPanel.svelte), driving `af.echo`. Across the top, the kind tabs
//! (Digital, Tape, Analog, Simple) and the Tempo and Ping-pong switches
//! (for Simple, whether times show in ms or samples). In the middle, the
//! big Time dial (the note value when following the tempo) and Feedback
//! dial either side of a lit display: the time (or the note, the tempo
//! and the ms) over the repeats fading away, alternating up and down when
//! they ping-pong, and a "Use session BPM" button when the slot's tempo
//! is not the session's. Below, framed groups of small dials:
//! Modulation (Depth, Wow on Tape; Rate), Filters, Machine (the Analog
//! dial, Tape and Analog only), Room (the spread used on a speaker bed)
//! and Output.
//!
//! Simple shows the two sides' times (ms, or samples at the session
//! rate) and a Left and a Right group instead, each with its Time and
//! Mix dials and an Invert switch.
//!
//! Each kind has its own faceplate, as in Audiform: Tape a warm brown
//! with an amber light, Analog a dark red with a red one (the colours of
//! Audiform's faceplate.css); Digital and Simple keep the theme's.

use egui::{vec2, Align2, FontId, Rect, Rounding, Sense, Stroke};

use af2_fx::effects::echo::{ANALOG, BBD, BEATS, NOTES, SIMPLE, TAPE};

use super::layout::{self, GAP, GAP_S, HERO, KNOB, SIDE};
use super::{parts, space, Env, Fx};
use crate::theme::{c, ca, lerp, Rgb, Theme};
use crate::widgets;

const KIND_TIPS: [&str; 4] = [
    "Clean repeats, each a copy of the last",
    "A tape loop: wow, flutter and saturation, each pass a little darker",
    "A bucket-brigade delay: darker repeats that clip softly",
    "Each side moved by its own time, early or late, with its own mix and polarity: for lining up or widening",
];

const SUBS: [&str; 4] = [
    "clean repeats",
    "wow, flutter and saturation",
    "dark repeats that clip softly",
    "each side early or late",
];

/// The display between the big dials.
const DISPLAY: f32 = 256.0;
const W: f32 = space::hero_w(DISPLAY);
/// Simple's dials.
const SIMPLE_KNOB: f32 = 72.0;

/// The theme as this kind's faceplate has it: Tape and Analog in their
/// own colours (face, lower face, lettering, light), the others as they
/// are.
fn tint(th: &Theme, kind: usize) -> Theme {
    let (face, face2, ink, glow): (Rgb, Rgb, Rgb, Rgb) = match kind {
        TAPE => (
            [0x3a, 0x2c, 0x1e],
            [0x22, 0x19, 0x11],
            [0xf1, 0xe2, 0xc6],
            [0xf0, 0xb3, 0x5a],
        ),
        BBD => (
            [0x33, 0x1d, 0x1f],
            [0x1c, 0x10, 0x11],
            [0xf0, 0xd6, 0xd2],
            [0xff, 0x6b, 0x5e],
        ),
        _ => return th.clone(),
    };
    let mix = |a: Rgb, b: Rgb, t: f32| -> Rgb {
        [0, 1, 2].map(|i| (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * t).round() as u8)
    };
    let mut t = th.clone();
    t.dark = true;
    t.panel_raised = face;
    t.background = face2;
    t.panel = mix(face, face2, 0.5);
    t.wave_background = mix(face2, [0, 0, 0], 0.6);
    t.border = mix(face2, [0, 0, 0], 0.55);
    t.text = ink;
    t.text_dim = mix(ink, face, 0.4);
    t.accent = glow;
    t.accent_alt = mix(glow, [255, 255, 255], 0.3);
    t
}

/// The width of a lit switch with this name.
fn switch_w(ui: &egui::Ui, t: &str) -> f32 {
    text_w(ui, t) + 26.0
}

/// The width of a stepped switch with these names.
fn steps_w(ui: &egui::Ui, names: &[&str]) -> f32 {
    names.iter().map(|n| text_w(ui, n) + 18.0).sum()
}

fn text_w(ui: &egui::Ui, t: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(
            t.to_string(),
            FontId::proportional(11.5),
            egui::Color32::WHITE,
        )
        .size()
        .x
}

/// A tempo as a short number ("120", "97.5").
fn bpm_text(t: f64) -> String {
    if (t - t.round()).abs() < 0.05 {
        format!("{t:.0}")
    } else {
        format!("{t:.1}")
    }
}

pub fn panel(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let kind = (fx.get("kind").round().max(0.0) as usize).min(SIMPLE);
    let th = tint(&env.th, kind);
    // The dials and switches read the theme too: light them in the kind's
    // colours, and put the window's own back after.
    widgets::set_theme(ui.ctx(), &th);
    parts::faceplate(ui, &th, "Delay and Echo", SUBS[kind], W, |ui| {
        top(ui, fx, env, kind);
        ui.add_space(GAP_S);
        if kind == SIMPLE {
            simple(ui, fx, env, &th);
        } else {
            echoes(ui, fx, env, &th, kind);
        }
        if env.spatial {
            ui.add_space(GAP_S);
            layout::note(
                ui,
                W,
                egui::RichText::new(if kind == SIMPLE {
                    "On the speaker bed: the left-hand speakers move by the left time, the right-hand ones by the right; the rest (and the LFE) are held back to match."
                } else {
                    "On the speaker bed: every speaker repeats on its own line, Ping-pong passes each repeat on round the room, Spread lets it leak into the speakers either side; the LFE stays dry."
                })
                .size(10.5)
                .color(c(th.accent_alt)),
            );
        }
    });
    widgets::set_theme(ui.ctx(), &env.th);
    // A note value or the Analog step lands on a whole step when let go.
    if fx.released {
        for key in ["note", "analog"] {
            let v = fx.get(key);
            if v != v.round() {
                fx.set(key, v.round());
            }
        }
    }
}

/// The kind tabs on the left, the switches on the right.
fn top(ui: &mut egui::Ui, fx: &mut Fx, env: &Env, kind: usize) {
    ui.horizontal(|ui| {
        let sp = ui.spacing().item_spacing.x;
        fx.choice(ui, "kind", &KIND_TIPS);
        let right = if kind == SIMPLE {
            steps_w(ui, &["ms", "Samples"])
        } else {
            switch_w(ui, "Tempo") + sp + switch_w(ui, "Ping-pong")
        };
        let fill = ui.available_width() - right - 2.0 * sp - 1.0;
        if fill > 0.0 {
            ui.add_space(fill);
        }
        if kind == SIMPLE {
            fx.choice(
                ui,
                "units",
                &["Show times in milliseconds", "Show times in samples"],
            );
            return;
        }
        let mut sync = fx.on("sync");
        if widgets::switch(ui, &mut sync, "Tempo")
            .on_hover_text("The time follows the tempo as a note value")
            .changed()
        {
            fx.set("sync", f64::from(u8::from(sync)));
            if let (true, Some(t)) = (sync, env.tempo) {
                fx.set("tempo", t.clamp(30.0, 300.0));
            }
        }
        let mut pp = fx.on("pingpong");
        if widgets::switch(ui, &mut pp, "Ping-pong")
            .on_hover_text("Each repeat crosses to the other side. On surround, each repeat moves on to the next speaker round the room.")
            .changed()
        {
            fx.set("pingpong", f64::from(u8::from(pp)));
        }
    });
}

/// Digital, Tape and Analog: the big dials round the display, then the
/// groups of small dials.
fn echoes(ui: &mut egui::Ui, fx: &mut Fx, env: &Env, th: &Theme, kind: usize) {
    let sync = fx.on("sync");
    let pingpong = fx.on("pingpong");
    let bpm = fx.get("tempo").clamp(30.0, 300.0);
    let note = (fx.get("note").round().max(0.0) as usize).min(NOTES.len() - 1);
    let ms = if sync {
        BEATS[note] * 60_000.0 / bpm
    } else {
        fx.get("time")
    }
    .clamp(1.0, 4000.0);
    let fb = fx.get("feedback") / 100.0;
    let note_fmt = |v: f64| NOTES[(v.round().max(0.0) as usize).min(NOTES.len() - 1)].to_string();
    layout::hero(ui, W, SIDE, |ui, i| match i {
        0 if sync => fx.knob_fmt(ui, "note", HERO, "Time", &[], &note_fmt),
        0 => fx.knob(ui, "time", HERO, "Time", &[1.0, 100.0, 1000.0, 4000.0]),
        1 => {
            let (big, small) = if sync {
                (
                    NOTES[note].to_string(),
                    format!("{} BPM · {}", bpm_text(bpm), space::secs(ms)),
                )
            } else {
                (
                    space::secs(ms),
                    ["digital", "tape", "analog"][kind.min(2)].to_string(),
                )
            };
            space::screen(
                ui,
                th,
                vec2(DISPLAY, 128.0),
                c(th.accent),
                &big,
                &small,
                |p, r, light| {
                    let mid = r.center().y;
                    let half = r.height() / 2.0 - 2.0;
                    p.line_segment(
                        [egui::pos2(r.left(), mid), egui::pos2(r.right(), mid)],
                        Stroke::new(1.0_f32, light.gamma_multiply(0.25)),
                    );
                    // The sound itself, then each repeat: when it comes,
                    // how loud, which side.
                    p.rect_filled(
                        Rect::from_min_max(
                            egui::pos2(r.left(), mid - half),
                            egui::pos2(r.left() + 3.0, mid + half),
                        ),
                        Rounding::ZERO,
                        ca(th.text, 150),
                    );
                    let span = (ms * 8.0).clamp(1500.0, 6000.0);
                    let mut level = 1.0f64;
                    let mut k = 1;
                    while k as f64 * ms <= span && k <= 48 {
                        let x = r.left() + r.width() * (k as f64 * ms / span) as f32;
                        let len = (level.min(1.0) as f32 * half).max(1.0);
                        let up = !pingpong || k % 2 == 1;
                        let (y0, y1) = if up {
                            (mid - len, mid)
                        } else {
                            (mid, mid + len)
                        };
                        p.rect_filled(
                            Rect::from_min_max(egui::pos2(x - 1.5, y0), egui::pos2(x + 1.5, y1)),
                            Rounding::ZERO,
                            light,
                        );
                        level *= fb;
                        if level < 0.01 {
                            break;
                        }
                        k += 1;
                    }
                },
            );
            if let Some(t) = env.tempo.filter(|t| sync && (t - bpm).abs() > 0.05) {
                ui.add_space(GAP_S / 2.0);
                if widgets::button(ui, &format!("Use session {} BPM", bpm_text(t)), false)
                    .on_hover_text("Use the session tempo")
                    .clicked()
                {
                    fx.set("tempo", t.clamp(30.0, 300.0));
                }
            }
        }
        _ => fx.knob(ui, "feedback", HERO, "Feedback", &[0.0, 60.0, 120.0]),
    });
    ui.add_space(GAP);
    let depth = if kind == TAPE { "Wow" } else { "Depth" };
    let modulation = [("depth", depth), ("rate", "Rate")];
    let filters = [("low_cut", "Lo cut"), ("tone", "Hi cut")];
    let machine = [("analog", "Analog")];
    let room = [("spread", "Spread")];
    let output = [("drive", "Drive"), ("dry", "Dry"), ("wet", "Echo")];
    let mut first: Vec<(&str, &[(&str, &str)])> =
        vec![("Modulation", &modulation), ("Filters", &filters)];
    if kind == TAPE || kind == BBD {
        first.push(("Machine", &machine));
    }
    let second: [(&str, &[(&str, &str)]); 2] = [("Room", &room), ("Output", &output)];
    let analog_fmt =
        |v: f64| ANALOG[(v.round().max(0.0) as usize).min(ANALOG.len() - 1)].to_string();
    let rows: [&[(&str, &[(&str, &str)])]; 2] = [&first, &second];
    for (n, row) in rows.iter().enumerate() {
        if n > 0 {
            ui.add_space(GAP_S);
        }
        let items: Vec<(&str, f32)> = row.iter().map(|g| (g.0, g.1.len() as f32)).collect();
        layout::groups(ui, th, W, &items, |ui, g| {
            let knobs = row[g].1;
            let iw = ui.available_width();
            layout::cells(ui, iw, knobs.len(), |ui, k| {
                let (key, name) = knobs[k];
                if key == "analog" {
                    fx.knob_fmt(ui, key, KNOB, name, &[], &analog_fmt);
                } else {
                    fx.knob(ui, key, KNOB, name, &[]);
                }
            });
        });
    }
}

/// Simple: the two sides' times, then a group for each side.
fn simple(ui: &mut egui::Ui, fx: &mut Fx, env: &Env, th: &Theme) {
    let samples = fx.get("units") >= 0.5;
    let rate = f64::from(env.rate.max(1));
    let tt = |ms: f64| {
        if samples {
            format!("{:.0} smp", ms / 1000.0 * rate)
        } else {
            format!("{}{ms:.2} ms", if ms > 0.0 { "+" } else { "" })
        }
    };
    readout(
        ui,
        th,
        &format!("L {}", tt(fx.get("l_time"))),
        &format!("R {}", tt(fx.get("r_time"))),
    );
    ui.add_space(GAP);
    let sides = [
        ("l_time", "l_mix", "l_invert"),
        ("r_time", "r_mix", "r_invert"),
    ];
    layout::groups(ui, th, W, &[("Left", 1.0), ("Right", 1.0)], |ui, g| {
        let (time, mix, invert) = sides[g];
        let iw = ui.available_width();
        layout::cells(ui, iw, 2, |ui, i| {
            if i == 0 {
                fx.knob_fmt(ui, time, SIMPLE_KNOB, "Time", &[-500.0, 0.0, 500.0], &tt);
            } else {
                fx.knob(ui, mix, SIMPLE_KNOB, "Mix", &[0.0, 50.0, 100.0]);
            }
        });
        ui.add_space(GAP_S / 2.0);
        layout::centred(ui, iw, |ui| {
            let mut on = fx.on(invert);
            if widgets::switch(ui, &mut on, "Invert")
                .on_hover_text("Flip this side's polarity")
                .changed()
            {
                fx.set(invert, f64::from(u8::from(on)));
            }
        });
    });
}

/// Simple's lit display: the left time on the left, the right on the
/// right.
fn readout(ui: &mut egui::Ui, th: &Theme, left: &str, right: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(W, 46.0), Sense::hover());
    let p = ui.painter_at(rect);
    let square = th.bevel.is_some();
    let r = Rounding::same(if square { 0.0 } else { 5.0 });
    let light = c(th.accent);
    p.rect_filled(rect, r, lerp(th.wave_background, th.background, 0.4));
    p.rect_filled(rect, r, light.gamma_multiply(0.05));
    let font = FontId::monospace(20.0);
    p.text(
        rect.left_center() + vec2(14.0, 0.0),
        Align2::LEFT_CENTER,
        left,
        font.clone(),
        light,
    );
    p.text(
        rect.right_center() - vec2(14.0, 0.0),
        Align2::RIGHT_CENTER,
        right,
        font,
        light,
    );
    if square {
        crate::skin::bevel(&p, rect, th, true);
    } else {
        p.rect_stroke(
            rect.shrink(0.5),
            r,
            Stroke::new(1.0_f32, ca(th.border, 230)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tempo_text_and_kind_colours() {
        assert_eq!(bpm_text(120.0), "120");
        assert_eq!(bpm_text(97.5), "97.5");
        let th = Theme::default();
        assert_eq!(tint(&th, 0).accent, th.accent);
        assert_eq!(tint(&th, SIMPLE).panel_raised, th.panel_raised);
        assert_eq!(tint(&th, TAPE).accent, [0xf0, 0xb3, 0x5a]);
        assert_eq!(tint(&th, BBD).accent, [0xff, 0x6b, 0x5e]);
    }
}
