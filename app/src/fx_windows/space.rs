// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The space units: the Reverb and Convolution Reverb faceplates and the
//! Delay faceplate (after the lead's Audiform windows), here driving the
//! `af.*` Reverb, the Convolution Reverb, and AudiForm 2's own Echo and
//! built-in Reverb. Each has its two big dials either side of a lit
//! display that draws what it does (the tail dying away, the impulse
//! response, the repeats fading), and small dials in groups below.
//!
//! On a 3D Space session or a surround file these effects run across the
//! speaker bed (see `af27_dsp::wide`): the reverbs fill the room, the
//! echo repeats on each speaker pair; the displays say so.

use std::sync::Arc;

use egui::{
    pos2, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Rounding, Sense, Shape, Stroke,
};

use super::parts;
use super::{Env, Fx};
use crate::theme::{c, ca, lerp, Theme};

/// The lit display between the big dials: a big readout, a small line
/// under it, and a picture drawn by `draw` (in the display's light).
fn screen(
    ui: &mut egui::Ui,
    th: &Theme,
    size: egui::Vec2,
    light: Color32,
    big: &str,
    small: &str,
    draw: impl FnOnce(&egui::Painter, Rect, Color32),
) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect);
    let square = th.bevel.is_some();
    let r = Rounding::same(if square { 0.0 } else { 5.0 });
    p.rect_filled(rect, r, lerp(th.wave_background, th.background, 0.4));
    p.rect_filled(rect, r, light.gamma_multiply(0.05));
    p.text(
        rect.left_top() + vec2(10.0, 7.0),
        Align2::LEFT_TOP,
        big,
        FontId::monospace(20.0),
        light,
    );
    p.text(
        rect.left_top() + vec2(10.0, 32.0),
        Align2::LEFT_TOP,
        small,
        FontId::proportional(10.5),
        light.gamma_multiply(0.75),
    );
    let pic = Rect::from_min_max(
        pos2(rect.left() + 8.0, rect.top() + 50.0),
        rect.max - vec2(8.0, 8.0),
    );
    p.line_segment(
        [pic.left_bottom(), pic.right_bottom()],
        Stroke::new(1.0_f32, light.gamma_multiply(0.3)),
    );
    draw(&p, pic, light);
    if square {
        crate::skin::bevel(&p, rect, th, true);
    } else {
        p.rect_stroke(r_rect(rect), r, Stroke::new(1.0_f32, ca(th.border, 230)));
    }
}

fn r_rect(r: Rect) -> Rect {
    r.shrink(0.5)
}

fn bar(p: &egui::Painter, x: f32, base: f32, h: f32, w: f32, col: Color32) {
    if h > 0.3 {
        p.rect_filled(
            Rect::from_min_max(pos2(x - w / 2.0, base - h), pos2(x + w / 2.0, base)),
            Rounding::ZERO,
            col,
        );
    }
}

fn secs(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else {
        format!("{ms:.0} ms")
    }
}

/// Each reverb kind's display light: from the theme's light toward its
/// second light, a step per kind.
fn kind_light(th: &Theme, kind: usize, of: usize) -> Color32 {
    lerp(
        th.accent,
        th.accent_alt,
        kind as f32 / of.max(2).saturating_sub(1) as f32,
    )
}

fn spatial_note(ui: &mut egui::Ui, env: &Env, text: &str) {
    if env.spatial {
        ui.label(RichText::new(text).size(10.5).color(c(env.th.accent_alt)));
    }
}

// ------------------------------------------------------------ Reverb

const KIND_TIPS: [&str; 5] = [
    "A small room: strong early reflections and a short, natural tail",
    "A large hall: slow, soft and wide, with a long build",
    "A plate: no reflections, a dense, bright wash",
    "A spring: short and boingy, dark at both ends",
    "A ring of tape echoes that blur into a tail, with wow and saturation",
];

pub fn verb(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = env.th.clone();
    let kind = fx.get("kind").round().max(0.0) as usize;
    let light = kind_light(&th, kind, 5);
    // Plates and springs have no walls to bounce off.
    let no_early = kind == 2 || kind == 3;
    parts::faceplate(ui, &th, "Reverb", "", |ui| {
        fx.choice(ui, "kind", &KIND_TIPS);
        ui.add_space(6.0);
        ui.horizontal_top(|ui| {
            fx.knob(ui, "size", 104.0, "Size", &[0.0, 50.0, 100.0]);
            let (pre, decay) = (fx.get("predelay"), fx.get("decay"));
            let early = if no_early {
                0.0
            } else {
                fx.get("early") / 100.0
            };
            let size = fx.get("size") / 100.0;
            let rough = 1.0 - fx.get("diffusion") / 100.0;
            let kname = fx
                .ctl("kind")
                .and_then(|c| c.choices.get(kind).cloned())
                .unwrap_or_default()
                .to_lowercase();
            screen(
                ui,
                &th,
                vec2(250.0, 128.0),
                light,
                &secs(decay),
                &format!("{kname} · pre-delay {}", secs(pre)),
                |p, r, light| {
                    let span = (pre + decay * 1.15).max(300.0);
                    let x = |ms: f64| r.left() + r.width() * (ms / span) as f32;
                    bar(p, r.left() + 1.5, r.bottom(), r.height(), 3.0, light);
                    let n = 70;
                    for i in 0..n {
                        let t = i as f64 / n as f64 * decay;
                        let level = (1.0 - t / decay).max(0.0);
                        let noise = ((i as f64 * 12.9898).sin() * 43758.5453).fract().abs();
                        let h = level * (1.0 - rough * 0.5 * noise) * 0.85;
                        bar(
                            p,
                            x(pre + t),
                            r.bottom(),
                            r.height() * h as f32,
                            2.0,
                            light.gamma_multiply(0.55),
                        );
                    }
                    if early > 0.0 {
                        let len = 15.0 + size * 70.0;
                        for i in 0..9 {
                            let at = pre + len * ((i as f64 * 0.618) % 1.0);
                            let h = early * (0.55 + 0.45 * ((i as f64 * 0.37) % 1.0));
                            bar(p, x(at), r.bottom(), r.height() * h as f32, 2.5, light);
                        }
                    }
                },
            );
            fx.knob(ui, "decay", 104.0, "Decay", &[100.0, 1000.0, 20_000.0]);
        });
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            parts::module(ui, &th, "Space", |ui| {
                ui.horizontal_top(|ui| {
                    fx.knob(ui, "predelay", 56.0, "Pre-delay", &[]);
                    ui.add_enabled_ui(!no_early, |ui| fx.knob(ui, "early", 56.0, "Early", &[]))
                        .response
                        .on_disabled_hover_text("Plates and springs have no walls to bounce off");
                    fx.knob(ui, "width", 56.0, "Width", &[]);
                });
            });
            parts::module(ui, &th, "Character", |ui| {
                ui.horizontal_top(|ui| {
                    fx.knob(ui, "damping", 56.0, "Damping", &[]);
                    fx.knob(ui, "diffusion", 56.0, "Diffusion", &[]);
                    fx.knob(ui, "modulation", 56.0, "Motion", &[]);
                });
            });
        });
        ui.horizontal_top(|ui| {
            parts::module(ui, &th, "Filters", |ui| {
                ui.horizontal_top(|ui| {
                    fx.knob(ui, "low_cut", 56.0, "Lo cut", &[]);
                    fx.knob(ui, "high_cut", 56.0, "Hi cut", &[]);
                });
            });
            parts::module(ui, &th, "Output", |ui| {
                ui.horizontal_top(|ui| {
                    fx.knob(ui, "dry", 56.0, "Dry", &[]);
                    fx.knob(ui, "wet", 56.0, "Wet", &[]);
                });
            });
        });
        spatial_note(
            ui,
            env,
            "On the speaker bed: the tail spreads round the whole room (the LFE stays dry).",
        );
    });
}

// ------------------------------------------------------------ Convolution

/// A response file as the display draws it.
struct IrShape {
    name: String,
    secs: f64,
    channels: usize,
    file_rate: u32,
    /// Level over time on a dB scale (0..1 from -60 dB to the peak).
    shape: Vec<f32>,
}

/// The slot's response file, read once per file (and kept for the
/// window), or why it could not be read.
fn ir_shape(ui: &egui::Ui, data: Option<&str>, rate: u32) -> Option<Result<Arc<IrShape>, String>> {
    let d: af27_fx::effects::reverb::ReverbData =
        serde_json::from_str(data.filter(|t| !t.is_empty())?).ok()?;
    let id = egui::Id::new(("ir-shape", d.path.to_string_lossy().to_string(), rate));
    if let Some(s) = ui
        .ctx()
        .data(|m| m.get_temp::<Arc<Result<Arc<IrShape>, String>>>(id))
    {
        return Some((*s).clone());
    }
    let res = af27_fx::effects::reverb::read(&d.path, rate).map(|r| {
        let frames = r.channels.first().map_or(0, Vec::len);
        let n = 240;
        let per = (frames / n).max(1);
        let peak = r
            .channels
            .iter()
            .flatten()
            .fold(1e-9f32, |m, v| m.max(v.abs()));
        let shape = (0..n)
            .map(|k| {
                let a = k * per;
                let b = ((k + 1) * per).min(frames);
                let m = r
                    .channels
                    .iter()
                    .flat_map(|ch| ch.get(a..b).unwrap_or(&[]))
                    .fold(0.0f32, |m, v| m.max(v.abs()));
                let db = 20.0 * (m / peak).max(1e-6).log10();
                ((db + 60.0) / 60.0).clamp(0.0, 1.0)
            })
            .collect();
        Arc::new(IrShape {
            name: d.name.clone(),
            secs: frames as f64 / f64::from(rate.max(1)),
            channels: r.file_channels,
            file_rate: r.file_rate,
            shape,
        })
    });
    ui.ctx()
        .data_mut(|m| m.insert_temp(id, Arc::new(res.clone())));
    Some(res)
}

pub fn convolution(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = env.th.clone();
    let ir = ir_shape(ui, fx.data.as_deref(), env.rate);
    let light = c(th.accent);
    parts::faceplate(ui, &th, "Convolution Reverb", "a recorded space", |ui| {
        ui.horizontal(|ui| {
            let has = fx.data.as_deref().is_some_and(|t| !t.is_empty());
            if crate::widgets::button(ui, if has { "Load another..." } else { "Load..." }, !has)
                .on_hover_text("Pick an audio file of a space's echo")
                .clicked()
            {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter(
                        "Audio",
                        &["wav", "flac", "aif", "aiff", "mp3", "ogg", "m4a"],
                    )
                    .pick_file()
                {
                    af27_fx::effects::reverb::forget(&path);
                    let name = path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    let d = af27_fx::effects::reverb::ReverbData { path, name };
                    if let Ok(t) = serde_json::to_string(&d) {
                        fx.new_data = Some(Some(t));
                    }
                }
            }
            if has
                && crate::widgets::button(ui, "Clear", false)
                    .on_hover_text("Remove the impulse response")
                    .clicked()
            {
                fx.new_data = Some(None);
            }
        });
        ui.add_space(4.0);
        let keep = fx.get("length") / 100.0;
        let pre = fx.get("predelay") / 1000.0;
        let (big, small) = match &ir {
            Some(Ok(s)) => (
                format!("{:.2} s", s.secs * keep),
                format!(
                    "{} · {} · {:.1} kHz",
                    s.name,
                    match s.channels {
                        1 => "mono".to_string(),
                        2 => "stereo".to_string(),
                        4 => "true stereo (4 channels)".to_string(),
                        n => format!("{n} channels, used as stereo"),
                    },
                    f64::from(s.file_rate) / 1000.0
                ),
            ),
            Some(Err(e)) => ("--".into(), format!("Could not be read: {e}")),
            None => ("--".into(), "No impulse response loaded".into()),
        };
        screen(
            ui,
            &th,
            vec2(520.0, 140.0),
            light,
            &big,
            &small,
            |p, r, light| {
                let Some(Ok(s)) = &ir else {
                    p.text(
                        r.center(),
                        Align2::CENTER_CENTER,
                        "Load any audio file of a space's echo: a room, hall, plate, spring...",
                        FontId::proportional(11.0),
                        light.gamma_multiply(0.6),
                    );
                    return;
                };
                let total = s.secs + pre;
                let x0 = r.left() + r.width() * (pre / total) as f32;
                let span = r.width() * (s.secs / total) as f32;
                let keep_x = x0 + span * keep as f32;
                let n = s.shape.len().max(2);
                let pts: Vec<Pos2> = s
                    .shape
                    .iter()
                    .enumerate()
                    .map(|(k, v)| {
                        pos2(
                            x0 + span * k as f32 / (n - 1) as f32,
                            r.bottom() - r.height() * v,
                        )
                    })
                    .collect();
                let mut m = egui::Mesh::default();
                for (k, q) in pts.iter().enumerate() {
                    let lit = q.x <= keep_x;
                    let top = if lit {
                        light.gamma_multiply(0.8)
                    } else {
                        light.gamma_multiply(0.18)
                    };
                    m.colored_vertex(*q, top);
                    m.colored_vertex(pos2(q.x, r.bottom()), light.gamma_multiply(0.06));
                    if k > 0 {
                        let i = (k * 2) as u32;
                        m.add_triangle(i - 2, i - 1, i);
                        m.add_triangle(i - 1, i + 1, i);
                    }
                }
                p.add(Shape::mesh(m));
                if keep < 0.999 {
                    p.line_segment(
                        [pos2(keep_x, r.top()), pos2(keep_x, r.bottom())],
                        Stroke::new(1.0_f32, light),
                    );
                }
                if pre > 0.0 {
                    p.line_segment(
                        [pos2(x0, r.top()), pos2(x0, r.bottom())],
                        Stroke::new(1.0_f32, light.gamma_multiply(0.5)),
                    );
                }
            },
        );
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            parts::module(ui, &th, "Response", |ui| {
                ui.horizontal_top(|ui| {
                    fx.knob(ui, "predelay", 56.0, "Pre-delay", &[]);
                    fx.knob(ui, "length", 56.0, "Length", &[]);
                    fx.knob(ui, "width", 56.0, "Width", &[]);
                });
            });
            parts::module(ui, &th, "Filters", |ui| {
                ui.horizontal_top(|ui| {
                    fx.knob(ui, "low_cut", 56.0, "Lo cut", &[]);
                    fx.knob(ui, "high_cut", 56.0, "Hi cut", &[]);
                });
            });
            parts::module(ui, &th, "Output", |ui| {
                ui.horizontal_top(|ui| {
                    fx.knob(ui, "dry", 56.0, "Dry", &[]);
                    fx.knob(ui, "wet", 56.0, "Wet", &[]);
                });
            });
        });
        spatial_note(
            ui,
            env,
            "On the speaker bed: the response is spread round the whole room (the LFE stays dry).",
        );
    });
}

// ------------------------------------------------------------ built-in Reverb

pub fn room(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = env.th.clone();
    let light = c(th.accent);
    parts::faceplate(ui, &th, "Reverb", "a quick room", |ui| {
        ui.horizontal_top(|ui| {
            fx.knob_fmt(ui, "Room size", 100.0, "Size", &[0.0, 0.5, 1.0], &|v| {
                format!("{:.0}%", v * 100.0)
            });
            let size = fx.get("Room size");
            let damp = fx.get("Damping");
            let pre = fx.get("Pre-delay");
            // About how long the tail rings: a bigger room longer, damping
            // shortens the top of it.
            let decay = 300.0 + size * size * 4200.0;
            screen(
                ui,
                &th,
                vec2(230.0, 118.0),
                light,
                &secs(decay),
                &format!("pre-delay {}", secs(pre)),
                |p, r, light| {
                    let span = (pre + decay * 1.15).max(300.0);
                    let x = |ms: f64| r.left() + r.width() * (ms / span) as f32;
                    bar(p, r.left() + 1.5, r.bottom(), r.height(), 3.0, light);
                    for i in 0..60 {
                        let t = i as f64 / 60.0 * decay;
                        let level = (-(t / decay) * (3.0 + 2.0 * damp)).exp();
                        bar(
                            p,
                            x(pre + t),
                            r.bottom(),
                            r.height() * 0.85 * level as f32,
                            2.0,
                            light.gamma_multiply(0.55),
                        );
                    }
                },
            );
            fx.knob_fmt(ui, "Mix", 100.0, "Mix", &[0.0, 0.5, 1.0], &|v| {
                format!("{:.0}%", v * 100.0)
            });
        });
        ui.horizontal_top(|ui| {
            fx.knob_fmt(ui, "Damping", 60.0, "", &[], &|v| {
                format!("{:.0}%", v * 100.0)
            });
            fx.knob_fmt(ui, "Width", 60.0, "", &[], &|v| {
                format!("{:.0}%", v * 100.0)
            });
            fx.knob(ui, "Pre-delay", 60.0, "", &[]);
        });
        spatial_note(
            ui,
            env,
            "On the speaker bed: a copy per speaker pair, each its own room (the LFE stays dry).",
        );
    });
}

// ------------------------------------------------------------ Echo

pub fn echo(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = env.th.clone();
    let light = c(th.accent);
    let pct = |v: f64| format!("{:.0}%", v * 100.0);
    parts::faceplate(ui, &th, "Echo", "repeats that fade away", |ui| {
        ui.horizontal_top(|ui| {
            fx.knob(ui, "Delay", 104.0, "Time", &[1.0, 500.0, 1000.0, 2000.0]);
            let ms = fx.get("Delay").max(1.0);
            let fb = fx.get("Feedback");
            screen(
                ui,
                &th,
                vec2(250.0, 128.0),
                light,
                &secs(ms),
                &format!("feedback {} · mix {}", pct(fb), pct(fx.get("Mix"))),
                |p, r, light| {
                    let span = (ms * 8.0).clamp(1500.0, 6000.0);
                    let x = |t: f64| r.left() + r.width() * (t / span) as f32;
                    bar(p, r.left() + 1.5, r.bottom(), r.height(), 3.0, light);
                    let mut level = 1.0;
                    let mut k = 1;
                    while k as f64 * ms <= span && k < 48 {
                        bar(
                            p,
                            x(k as f64 * ms),
                            r.bottom(),
                            r.height() * level as f32,
                            3.0,
                            light.gamma_multiply(0.8),
                        );
                        level *= fb;
                        if level < 0.01 {
                            break;
                        }
                        k += 1;
                    }
                },
            );
            fx.knob_fmt(ui, "Feedback", 104.0, "", &[0.0, 0.5, 0.95], &pct);
        });
        ui.horizontal_top(|ui| {
            fx.knob_fmt(ui, "Mix", 62.0, "", &[], &pct);
        });
        spatial_note(ui, env, "On the speaker bed: each speaker pair repeats on its own, the LFE left dry and in time.");
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn readouts() {
        assert_eq!(super::secs(350.0), "350 ms");
        assert_eq!(super::secs(1800.0), "1.80 s");
    }
}
