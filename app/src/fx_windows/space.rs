// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The space units: the Reverb and Convolution Reverb faceplates and the
//! Delay faceplate (after the lead's Audiform windows), here driving the
//! `af.*` Reverb, the Convolution Reverb, and AudiForm 2's own Echo and
//! built-in Reverb. Each has its two big dials either side of a lit
//! display that draws what it does (the tail dying away, the impulse
//! response, the repeats fading), and small dials in groups below.
//!
//! The Reverb and Convolution Reverb share one row of kind tabs (Room,
//! Hall, Plate, Spring, Tape, Impulse Response) and each kind its own
//! faceplate colours, blended into the theme. Impulse Response turns a
//! Reverb slot into the Convolution Reverb and a made kind turns it back,
//! carrying the settings both have (`to_impulse`, `from_impulse`).
//!
//! On a 3D Space session or a surround file these effects run across the
//! speaker bed (see `af2_dsp::wide`): the reverbs fill the room, the
//! echo repeats on each speaker pair; the displays say so.

use std::sync::Arc;

use egui::{
    pos2, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Rounding, Sense, Shape, Stroke,
};

use super::layout::{self, GAP, GAP_S, HERO, KNOB, SIDE};
use super::parts;
use super::{Env, Fx};
use crate::theme::{c, ca, lerp, Rgb, Theme};

/// The lit display between the big dials: a big readout, a small line
/// under it, and a picture drawn by `draw` (in the display's light).
pub(super) fn screen(
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

pub(super) fn bar(p: &egui::Painter, x: f32, base: f32, h: f32, w: f32, col: Color32) {
    if h > 0.3 {
        p.rect_filled(
            Rect::from_min_max(pos2(x - w / 2.0, base - h), pos2(x + w / 2.0, base)),
            Rounding::ZERO,
            col,
        );
    }
}

pub(super) fn secs(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else {
        format!("{ms:.0} ms")
    }
}

fn spatial_note(ui: &mut egui::Ui, env: &Env, w: f32, text: &str) {
    if env.spatial {
        ui.add_space(GAP_S);
        layout::note(
            ui,
            w,
            RichText::new(text).size(10.5).color(c(env.th.accent_alt)),
        );
    }
}

/// A face with a hero dial either side of a display `display` wide.
pub(super) const fn hero_w(display: f32) -> f32 {
    2.0 * SIDE + display + 2.0 * GAP
}

/// A group of small dials: its heading and its (key, name) dials.
type Group<'a> = (&'a str, &'a [(&'a str, &'a str)]);

// ------------------------------------------------------------ Reverb

/// The Reverb window's tabs: the made spaces, then the recorded one
/// (Impulse Response turns the slot into the Convolution Reverb).
pub(super) const TABS: [&str; 7] = [
    "Room",
    "Hall",
    "Plate",
    "Spring",
    "Tape",
    "Impulse Response",
    "Studio",
];
/// The order the tabs are shown in: Studio first, the recorded space last.
const TAB_ORDER: [usize; 7] = [STUDIO_TAB, 0, 1, 2, 3, 4, IMPULSE];
/// The Studio tab (its kind is `af2_fx::effects::verb::STUDIO`).
const STUDIO_TAB: usize = 6;

/// The tab a Reverb kind is shown on.
pub(super) fn tab_of(kind: usize) -> usize {
    if kind == af2_fx::effects::verb::STUDIO {
        STUDIO_TAB
    } else {
        kind.min(4)
    }
}

/// The Reverb kind a made-space tab stands for.
fn kind_of(tab: usize) -> usize {
    if tab == STUDIO_TAB {
        af2_fx::effects::verb::STUDIO
    } else {
        tab
    }
}
const TAB_TIPS: [&str; 7] = [
    "A small room: strong early reflections and a short, natural tail",
    "A large hall: slow, soft and wide, with a long build",
    "A plate: no reflections, a dense, bright wash",
    "A spring: short and boingy, dark at both ends",
    "A ring of tape echoes that blur into a tail, with wow and saturation",
    "A recorded space: load any audio file of a room's, hall's or plate's echo",
    "A studio: a dense, even room that grows and softens with its size, with early reflections only when asked",
];
/// The Impulse Response tab.
pub(super) const IMPULSE: usize = 5;
pub const REVERB_ID: &str = "af.reverb";
pub const IMPULSE_ID: &str = "af.convolution_reverb";

/// A kind's faceplate: its face (top and bottom), its ink and its glow.
pub(super) struct Look {
    pub(super) face: Rgb,
    pub(super) face2: Rgb,
    pub(super) ink: Rgb,
    pub(super) glow: Rgb,
}

/// Room, Hall, Plate, Spring, Tape, Impulse Response, then Studio.
const LOOKS: [Look; 7] = [
    Look {
        face: [0x32, 0x26, 0x1c],
        face2: [0x1c, 0x14, 0x0e],
        ink: [0xf2, 0xe2, 0xd0],
        glow: [0xe0, 0xa4, 0x6a],
    },
    Look {
        face: [0x1c, 0x22, 0x36],
        face2: [0x10, 0x14, 0x1f],
        ink: [0xdb, 0xe4, 0xfb],
        glow: [0x7f, 0xa8, 0xff],
    },
    Look {
        face: [0x2a, 0x2d, 0x33],
        face2: [0x16, 0x18, 0x1c],
        ink: [0xee, 0xf0, 0xf4],
        glow: [0xd8, 0xdd, 0xe6],
    },
    Look {
        face: [0x1d, 0x2b, 0x1c],
        face2: [0x10, 0x19, 0x10],
        ink: [0xdc, 0xef, 0xd8],
        glow: [0x8f, 0xd1, 0x8a],
    },
    Look {
        face: [0x33, 0x2a, 0x14],
        face2: [0x1d, 0x17, 0x0a],
        ink: [0xf4, 0xe8, 0xc8],
        glow: [0xf0, 0xc0, 0x50],
    },
    Look {
        face: [0x24, 0x1d, 0x33],
        face2: [0x12, 0x0f, 0x1b],
        ink: [0xec, 0xe4, 0xff],
        glow: [0xb4, 0x8c, 0xff],
    },
    // Studio: graphite with a cool teal light.
    Look {
        face: [0x1e, 0x26, 0x2a],
        face2: [0x10, 0x15, 0x18],
        ink: [0xe0, 0xf0, 0xf2],
        glow: [0x5c, 0xd0, 0xc8],
    },
];

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = lerp(a, b, t);
    [m.r(), m.g(), m.b()]
}

/// The theme with a kind's faceplate blended in. On a dark theme the
/// face, ink and light are mostly the kind's; on a light theme the face
/// takes a tint of the glow and the light is the glow darkened enough to
/// read on it. Borders, fonts and the theme's shapes stay its own.
pub(super) fn tinted(th: &Theme, look: &Look) -> Theme {
    let mut t = th.clone();
    if th.dark {
        t.panel_raised = mix(th.panel_raised, look.face, 0.8);
        t.background = mix(th.background, look.face2, 0.8);
        t.panel = mix(th.panel, look.face2, 0.6);
        t.wave_background = mix(th.wave_background, mix(look.face2, [0, 0, 0], 0.55), 0.7);
        t.accent = mix(th.accent, look.glow, 0.85);
        t.accent_alt = mix(th.accent_alt, look.glow, 0.5);
        t.text = mix(th.text, look.ink, 0.6);
        t.text_dim = mix(th.text_dim, mix(look.ink, look.face, 0.45), 0.5);
        t.border = mix(th.border, look.face2, 0.3);
    } else {
        t.panel_raised = mix(th.panel_raised, look.glow, 0.12);
        t.background = mix(th.background, look.glow, 0.10);
        t.panel = mix(th.panel, look.glow, 0.08);
        t.wave_background = mix(th.wave_background, look.glow, 0.10);
        t.accent = mix(mix(look.glow, [0, 0, 0], 0.45), th.accent, 0.2);
        t.accent_alt = mix(th.accent_alt, t.accent, 0.5);
    }
    t
}

/// Draws `add` with `th` as the theme the widgets read (the knobs and
/// value boxes take the kind's light), then puts the theme back.
pub(super) fn with_theme(ui: &mut egui::Ui, th: &Theme, add: impl FnOnce(&mut egui::Ui)) {
    let was = crate::widgets::theme(ui);
    crate::widgets::set_theme(ui.ctx(), th);
    add(ui);
    crate::widgets::set_theme(ui.ctx(), &was);
}

/// The kind's faceplate: its face blended from top to bottom, lit from
/// the top in its glow, `w` wide inside its margin.
pub(super) fn plate(ui: &mut egui::Ui, th: &Theme, w: f32, add: impl FnOnce(&mut egui::Ui)) {
    // A slot taken before the contents, so the plate lands under them.
    let back = ui.painter().add(Shape::Noop);
    let rect = egui::Frame::none()
        .inner_margin(egui::Margin::same(GAP + 2.0))
        .show(ui, |ui| {
            ui.set_width(w);
            ui.vertical(add);
        })
        .response
        .rect;
    let square = th.bevel.is_some();
    let rad = if square { 0.0 } else { 8.0 };
    let mut shapes = parts::vgrad(rect, rad, c(th.panel_raised), c(th.background));
    // The glow from the top, gone by about halfway down.
    shapes.push(Shape::mesh(parts::blend(
        Rect::from_min_max(
            pos2(rect.left() + rad, rect.top() + 1.0),
            pos2(rect.right() - rad, rect.top() + rect.height() * 0.55),
        ),
        ca(th.accent, if th.dark { 26 } else { 18 }),
        Color32::TRANSPARENT,
    )));
    if !square {
        shapes.push(Shape::line_segment(
            [
                pos2(rect.left() + rad, rect.top() + 1.0),
                pos2(rect.right() - rad, rect.top() + 1.0),
            ],
            Stroke::new(1.0_f32, Color32::from_white_alpha(18)),
        ));
        shapes.push(Shape::rect_stroke(
            rect,
            rad,
            Stroke::new(
                1.0_f32,
                if th.dark {
                    Color32::from_black_alpha(230)
                } else {
                    ca(th.border, 230)
                },
            ),
        ));
    }
    ui.painter().set(back, Shape::Vec(shapes));
    if square {
        crate::skin::bevel(ui.painter(), rect, th, false);
    }
}

/// A faceplate key's label: capitals, spaced out.
fn key_text(ui: &egui::Ui, label: &str, col: Color32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &label.to_uppercase(),
        0.0,
        egui::TextFormat {
            font_id: FontId::proportional(11.0),
            color: col,
            extra_letter_spacing: 0.9,
            ..Default::default()
        },
    );
    ui.painter().layout_job(job)
}

/// A key's width.
fn key_w(ui: &egui::Ui, label: &str) -> f32 {
    key_text(ui, label, Color32::WHITE).size().x + 20.0
}

/// A faceplate key: a dark switch, lit in the kind's glow when on.
pub(super) fn key(ui: &mut egui::Ui, th: &Theme, label: &str, lit: bool, enabled: bool) -> egui::Response {
    let w = key_w(ui, label);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (r, resp) = ui.allocate_exact_size(vec2(w, 24.0), sense);
    let p = ui.painter();
    let square = th.bevel.is_some();
    let rad = if square { 0.0 } else { 4.0 };
    let hot = enabled && resp.hovered();
    let glow = c(th.accent);
    let (top, bottom, ink) = if lit {
        let ink = if th.dark {
            Color32::from_gray(17)
        } else {
            Color32::WHITE
        };
        (lerp(th.accent, [255, 255, 255], 0.12), glow, ink)
    } else if th.dark {
        (
            lerp(
                th.panel_raised,
                [255, 255, 255],
                if hot { 0.16 } else { 0.10 },
            ),
            lerp(th.background, [0, 0, 0], 0.25),
            if hot { glow } else { ca(th.text, 185) },
        )
    } else {
        (
            lerp(
                th.panel_raised,
                [255, 255, 255],
                if hot { 0.55 } else { 0.4 },
            ),
            lerp(th.panel_raised, [0, 0, 0], 0.12),
            if hot { glow } else { c(th.text) },
        )
    };
    let fade = |col: Color32| {
        if enabled {
            col
        } else {
            col.gamma_multiply(0.4)
        }
    };
    if lit && !square {
        p.rect_stroke(
            r.expand(1.5),
            rad + 1.5,
            Stroke::new(2.0_f32, ca(th.accent, 80)),
        );
    }
    p.extend(parts::vgrad(r, rad, fade(top), fade(bottom)));
    if square {
        crate::skin::bevel(p, r, th, lit);
    } else {
        p.line_segment(
            [
                pos2(r.left() + rad, r.top() + 1.0),
                pos2(r.right() - rad, r.top() + 1.0),
            ],
            Stroke::new(
                1.0_f32,
                Color32::from_white_alpha(if lit { 90 } else { 28 }),
            ),
        );
        p.rect_stroke(
            r,
            rad,
            Stroke::new(
                1.0_f32,
                if th.dark {
                    Color32::from_black_alpha(240)
                } else {
                    ca(th.border, 230)
                },
            ),
        );
    }
    let g = key_text(ui, label, fade(ink));
    let at = r.center() - g.size() / 2.0;
    ui.painter().galley(at, g, fade(ink));
    resp
}

/// The kind tabs across the top, `lit` the one shown. Returns a tab
/// clicked (never the lit one).
fn kind_tabs(ui: &mut egui::Ui, th: &Theme, lit: usize) -> Option<usize> {
    let mut picked = None;
    for i in TAB_ORDER {
        let name = TABS[i];
        if key(ui, th, name, i == lit, true)
            .on_hover_text(TAB_TIPS[i])
            .clicked()
            && i != lit
        {
            picked = Some(i);
        }
    }
    picked
}

/// The width of the tab row.
fn tabs_w(ui: &egui::Ui) -> f32 {
    TABS.iter().map(|t| key_w(ui, t)).sum::<f32>() + TAB_GAP * (TABS.len() - 1) as f32
}

const TAB_GAP: f32 = 4.0;

/// A Reverb level (%) as the Convolution Reverb's (dB).
fn to_db(pct: f64) -> f64 {
    if pct <= 0.0 {
        -48.0
    } else {
        (20.0 * (pct / 100.0).log10()).clamp(-48.0, 6.0)
    }
}

/// And back.
fn to_pct(db: f64) -> f64 {
    if db <= -48.0 {
        0.0
    } else {
        (100.0 * 10f64.powf(db / 20.0)).clamp(0.0, 100.0)
    }
}

/// The settings that carry over when a Reverb slot becomes the
/// Convolution Reverb (the levels from % to dB).
pub fn to_impulse(get: impl Fn(&str) -> f64) -> Vec<(String, f64)> {
    let mut out = vec![
        ("dry".to_string(), to_db(get("dry"))),
        ("wet".to_string(), to_db(get("wet"))),
    ];
    for k in ["predelay", "low_cut", "high_cut", "width"] {
        out.push((k.to_string(), get(k)));
    }
    out
}

/// And back: a Convolution Reverb slot becoming a Reverb of kind `kind`.
pub fn from_impulse(get: impl Fn(&str) -> f64, kind: usize) -> Vec<(String, f64)> {
    let mut out = vec![
        ("kind".to_string(), kind as f64),
        ("dry".to_string(), to_pct(get("dry"))),
        ("wet".to_string(), to_pct(get("wet"))),
    ];
    for k in ["predelay", "low_cut", "high_cut"] {
        out.push((k.to_string(), get(k)));
    }
    out.push(("width".to_string(), get("width").min(100.0)));
    out
}

/// The Reverb's face width: the big dials either side of the display.
const VERB_W: f32 = hero_w(256.0);

pub fn verb(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let kind = (fx.get("kind").round().max(0.0) as usize).min(af2_fx::effects::verb::STUDIO);
    let tab = tab_of(kind);
    let th = tinted(&env.th, &LOOKS[tab]);
    // Plates and springs have no walls to bounce off.
    let no_early = kind == 2 || kind == 3;
    let w = VERB_W.max(tabs_w(ui));
    with_theme(ui, &th, |ui| {
        plate(ui, &th, w, |ui| {
            let picked = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = TAB_GAP;
                    kind_tabs(ui, &th, tab)
                })
                .inner;
            match picked {
                Some(IMPULSE) => {
                    fx.new_effect = Some((IMPULSE_ID.into(), to_impulse(|k| fx.get(k))));
                }
                Some(t) => {
                    for (k, v) in af2_fx::effects::verb::switch_kind(kind, kind_of(t), |k| fx.get(k)) {
                        fx.set(k, v);
                    }
                }
                None => {}
            }
            ui.add_space(GAP);
            if kind == af2_fx::effects::verb::STUDIO {
                // Studio: its ten long sliders, in the kind's light.
                let studio_env = Env {
                    th: th.clone(),
                    levels: env.levels,
                    playing: env.playing,
                    spatial: env.spatial,
                    rate: env.rate,
                    tempo: env.tempo,
                    key: env.key,
                };
                super::studio::panel(ui, fx, &studio_env);
                spatial_note(
                    ui,
                    env,
                    w,
                    "On the speaker bed: the tail spreads round the whole room (the LFE stays dry).",
                );
                return;
            }
            let disp = w - 2.0 * SIDE - 2.0 * GAP;
            layout::hero(ui, w, SIDE, |ui, i| match i {
                0 => fx.knob(ui, "size", HERO, "Size", &[0.0, 50.0, 100.0]),
                1 => {
                    let (pre, decay) = (fx.get("predelay"), fx.get("decay"));
                    let early = if no_early {
                        0.0
                    } else {
                        fx.get("early") / 100.0
                    };
                    let size = fx.get("size") / 100.0;
                    let rough = 1.0 - fx.get("diffusion") / 100.0;
                    let ink = c(th.text).gamma_multiply(0.6);
                    screen(
                        ui,
                        &th,
                        vec2(disp, 128.0),
                        c(th.accent),
                        &secs(decay),
                        &format!("{} · pre-delay {}", TABS[tab].to_lowercase(), secs(pre)),
                        |p, r, light| {
                            let span = (pre + decay * 1.15).max(300.0);
                            let x = |ms: f64| r.left() + r.width() * (ms / span) as f32;
                            bar(p, r.left() + 1.5, r.bottom(), r.height(), 3.0, ink);
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
                }
                _ => fx.knob(ui, "decay", HERO, "Decay", &[100.0, 1000.0, 20_000.0]),
            });
            ui.add_space(GAP);
            // The groups two to a row, every dial in a cell of one width.
            let rows: [[Group; 2]; 2] = [
                [
                    (
                        "Space",
                        &[
                            ("predelay", "Pre-delay"),
                            ("early", "Early"),
                            ("width", "Width"),
                        ],
                    ),
                    (
                        "Character",
                        &[
                            ("damping", "Damping"),
                            ("diffusion", "Diffusion"),
                            ("modulation", "Motion"),
                        ],
                    ),
                ],
                [
                    ("Filters", &[("low_cut", "Lo cut"), ("high_cut", "Hi cut")]),
                    ("Output", &[("dry", "Dry"), ("wet", "Wet")]),
                ],
            ];
            for (r, groups) in rows.iter().enumerate() {
                if r > 0 {
                    ui.add_space(GAP_S);
                }
                let items = [(groups[0].0, 1.0), (groups[1].0, 1.0)];
                layout::groups(ui, &th, w, &items, |ui, g| {
                    let knobs = groups[g].1;
                    let iw = ui.available_width();
                    layout::grid(ui, iw, knobs.len(), 3, |ui, k| {
                        let (key, name) = knobs[k];
                        if key == "early" {
                            ui.add_enabled_ui(!no_early, |ui| fx.knob(ui, key, KNOB, name, &[]))
                                .response
                                .on_disabled_hover_text(
                                    "Plates and springs have no walls to bounce off",
                                );
                        } else {
                            fx.knob(ui, key, KNOB, name, &[]);
                        }
                    });
                });
            }
            spatial_note(
                ui,
                env,
                w,
                "On the speaker bed: the tail spreads round the whole room (the LFE stays dry).",
            );
        });
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
    let d: af2_fx::effects::reverb::ReverbData =
        serde_json::from_str(data.filter(|t| !t.is_empty())?).ok()?;
    let id = egui::Id::new(("ir-shape", d.path.to_string_lossy().to_string(), rate));
    if let Some(s) = ui
        .ctx()
        .data(|m| m.get_temp::<Arc<Result<Arc<IrShape>, String>>>(id))
    {
        return Some((*s).clone());
    }
    let res = af2_fx::effects::reverb::read(&d.path, rate).map(|r| {
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

/// The Convolution Reverb's face width: its three groups of dials.
const CONV_W: f32 = 7.0 * layout::CELL + 2.0 * GAP + 6.0 * layout::PAD;

pub fn convolution(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = tinted(&env.th, &LOOKS[IMPULSE]);
    let ir = ir_shape(ui, fx.data.as_deref(), env.rate);
    let has = fx.data.as_deref().is_some_and(|t| !t.is_empty());
    let load = if has { "Load another" } else { "Load" };
    let switches_w = key_w(ui, load) + TAB_GAP + key_w(ui, "Clear");
    // The face is as wide with a file as without one.
    let widest = key_w(ui, "Load another") + TAB_GAP + key_w(ui, "Clear");
    let tabs = tabs_w(ui);
    // The tabs and the Load and Clear switches on one line when they fit
    // a face a little wider than the dials need, else the switches go
    // on a line of their own at the right.
    let one_line = tabs + GAP + widest <= CONV_W + 80.0;
    let w = if one_line {
        CONV_W.max(tabs + GAP + widest)
    } else {
        CONV_W.max(tabs)
    };
    with_theme(ui, &th, |ui| {
        plate(ui, &th, w, |ui| {
            let mut picked = None;
            let mut pick_file = false;
            let mut clear = false;
            let mut switches = |ui: &mut egui::Ui| {
                pick_file = key(ui, &th, load, !has, true)
                    .on_hover_text("Pick an audio file of a space's echo")
                    .clicked();
                clear = key(ui, &th, "Clear", false, has)
                    .on_hover_text("Remove the impulse response")
                    .clicked();
            };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = TAB_GAP;
                picked = kind_tabs(ui, &th, IMPULSE);
                if one_line {
                    ui.add_space((w - tabs - switches_w - 2.0 * TAB_GAP).max(0.0));
                    switches(ui);
                }
            });
            if !one_line {
                ui.add_space(TAB_GAP);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = TAB_GAP;
                    ui.add_space((w - switches_w - TAB_GAP).max(0.0));
                    switches(ui);
                });
            }
            if let Some(k) = picked {
                fx.new_effect = Some((REVERB_ID.into(), from_impulse(|k| fx.get(k), kind_of(k))));
            }
            if pick_file {
                if let Some(path) = crate::file_dialog::FileDialog::new()
                    .add_filter(
                        "Audio",
                        &["wav", "flac", "aif", "aiff", "mp3", "ogg", "m4a"],
                    )
                    .pick_file()
                {
                    af2_fx::effects::reverb::forget(&path);
                    let name = path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    let d = af2_fx::effects::reverb::ReverbData { path, name };
                    if let Ok(t) = serde_json::to_string(&d) {
                        fx.new_data = Some(Some(t));
                    }
                }
            }
            if clear {
                fx.new_data = Some(None);
            }
            ui.add_space(GAP);
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
                Some(Err(e)) => (
                    "--".into(),
                    format!("Could not be read: {e}. Only the dry sound plays until it is loaded again."),
                ),
                None => ("--".into(), "no impulse response loaded".into()),
            };
            let ink = c(th.text);
            screen(
                ui,
                &th,
                vec2(w, 150.0),
                c(th.accent),
                &big,
                &small,
                |p, r, light| {
                    for g in [0.25, 0.5, 0.75] {
                        let y = r.top() + r.height() * g;
                        p.line_segment(
                            [pos2(r.left(), y), pos2(r.right(), y)],
                            Stroke::new(1.0_f32, light.gamma_multiply(0.18)),
                        );
                    }
                    let Some(Ok(s)) = &ir else {
                        p.text(
                            r.center(),
                            Align2::CENTER_CENTER,
                            "Load any audio file of a space's echo: a room, hall, plate, spring, or something stranger.",
                            FontId::proportional(11.0),
                            ink.gamma_multiply(0.6),
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
                                r.bottom() - (r.height() - 4.0) * v,
                            )
                        })
                        .collect();
                    let mut m = egui::Mesh::default();
                    for (k, q) in pts.iter().enumerate() {
                        let lit = q.x <= keep_x;
                        let top = if lit {
                            light.gamma_multiply(0.85)
                        } else {
                            light.gamma_multiply(0.12)
                        };
                        let foot = if lit {
                            light.gamma_multiply(0.12)
                        } else {
                            light.gamma_multiply(0.04)
                        };
                        m.colored_vertex(*q, top);
                        m.colored_vertex(pos2(q.x, r.bottom()), foot);
                        if k > 0 {
                            let i = (k * 2) as u32;
                            m.add_triangle(i - 2, i - 1, i);
                            m.add_triangle(i - 1, i + 1, i);
                        }
                    }
                    p.add(Shape::mesh(m));
                    let dashed = |x: f32, col: Color32| {
                        p.extend(Shape::dashed_line(
                            &[pos2(x, r.top()), pos2(x, r.bottom())],
                            Stroke::new(1.0_f32, col),
                            3.0,
                            3.0,
                        ));
                    };
                    if keep < 0.999 {
                        dashed(keep_x, ink.gamma_multiply(0.7));
                    }
                    if pre > 0.0 {
                        dashed(x0, light.gamma_multiply(0.6));
                    }
                },
            );
            ui.add_space(GAP);
            // Each group as wide as its dials need, every dial in a cell
            // of one width.
            let groups: [Group; 3] = [
                (
                    "Response",
                    &[
                        ("predelay", "Pre-delay"),
                        ("length", "Length"),
                        ("width", "Width"),
                    ],
                ),
                ("Filters", &[("low_cut", "Lo cut"), ("high_cut", "Hi cut")]),
                ("Output", &[("dry", "Dry"), ("wet", "Wet")]),
            ];
            let items: Vec<(&str, f32)> = groups
                .iter()
                .map(|g| (g.0, g.1.len() as f32 * layout::CELL + 2.0 * layout::PAD))
                .collect();
            layout::groups(ui, &th, w, &items, |ui, g| {
                let knobs = groups[g].1;
                let iw = ui.available_width();
                layout::cells(ui, iw, knobs.len(), |ui, k| {
                    fx.knob(ui, knobs[k].0, KNOB, knobs[k].1, &[]);
                });
            });
            spatial_note(
                ui,
                env,
                w,
                "On the speaker bed: the response is spread round the whole room (the LFE stays dry).",
            );
        });
    });
}

// ------------------------------------------------------------ built-in Reverb

pub fn room(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = env.th.clone();
    let light = c(th.accent);
    const W: f32 = hero_w(236.0);
    let pct = |v: f64| format!("{:.0}%", v * 100.0);
    parts::faceplate(ui, &th, "Reverb", "a quick room", W, |ui| {
        layout::hero(ui, W, SIDE, |ui, i| match i {
            0 => fx.knob_fmt(ui, "Room size", HERO, "Size", &[0.0, 0.5, 1.0], &pct),
            1 => {
                let size = fx.get("Room size");
                let damp = fx.get("Damping");
                let pre = fx.get("Pre-delay");
                // About how long the tail rings: a bigger room longer,
                // damping shortens the top of it.
                let decay = 300.0 + size * size * 4200.0;
                screen(
                    ui,
                    &th,
                    vec2(236.0, 124.0),
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
            }
            _ => fx.knob_fmt(ui, "Mix", HERO, "Mix", &[0.0, 0.5, 1.0], &pct),
        });
        ui.add_space(GAP);
        layout::cells(ui, W, 3, |ui, i| match i {
            0 => fx.knob_fmt(ui, "Damping", KNOB, "", &[], &pct),
            1 => fx.knob_fmt(ui, "Width", KNOB, "", &[], &pct),
            _ => fx.knob(ui, "Pre-delay", KNOB, "", &[]),
        });
        spatial_note(
            ui,
            env,
            W,
            "On the speaker bed: a copy per speaker pair, each its own room (the LFE stays dry).",
        );
    });
}

// ------------------------------------------------------------ Echo

pub fn echo(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = env.th.clone();
    let light = c(th.accent);
    let pct = |v: f64| format!("{:.0}%", v * 100.0);
    const W: f32 = hero_w(256.0);
    parts::faceplate(ui, &th, "Echo", "repeats that fade away", W, |ui| {
        layout::hero(ui, W, SIDE, |ui, i| match i {
            0 => fx.knob(ui, "Delay", HERO, "Time", &[1.0, 500.0, 1000.0, 2000.0]),
            1 => {
                let ms = fx.get("Delay").max(1.0);
                let fb = fx.get("Feedback");
                screen(
                    ui,
                    &th,
                    vec2(256.0, 128.0),
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
            }
            _ => fx.knob_fmt(ui, "Feedback", HERO, "", &[0.0, 0.5, 0.95], &pct),
        });
        ui.add_space(GAP);
        // The blend sits on the centre line, under the display.
        layout::centred(ui, W, |ui| fx.knob_fmt(ui, "Mix", KNOB, "", &[], &pct));
        spatial_note(
            ui,
            env,
            W,
            "On the speaker bed: each speaker pair repeats on its own, the LFE left dry and in time.",
        );
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn readouts() {
        assert_eq!(super::secs(350.0), "350 ms");
        assert_eq!(super::secs(1800.0), "1.80 s");
    }

    #[test]
    fn studio_is_the_first_tab_and_every_kind_has_its_own() {
        use af2_fx::effects::verb::{KINDS, STUDIO};
        assert_eq!(super::TABS[super::TAB_ORDER[0]], "Studio");
        assert_eq!(*super::TAB_ORDER.last().unwrap(), super::IMPULSE);
        for (kind, name) in KINDS.iter().enumerate() {
            let tab = super::tab_of(kind);
            assert_eq!(super::TABS[tab], *name);
            assert_eq!(super::kind_of(tab), kind);
        }
        assert_eq!(super::kind_of(super::tab_of(STUDIO)), STUDIO);
    }

    #[test]
    fn settings_carry_between_the_reverb_and_impulse_response() {
        use super::{from_impulse, to_impulse};
        let verb = |k: &str| match k {
            "dry" => 100.0,
            "wet" => 0.0,
            "predelay" => 25.0,
            "low_cut" => 120.0,
            "high_cut" => 8000.0,
            "width" => 80.0,
            _ => f64::NAN,
        };
        let ir: std::collections::HashMap<String, f64> = to_impulse(verb).into_iter().collect();
        assert!(ir["dry"].abs() < 1e-9);
        assert_eq!(ir["wet"], -48.0);
        assert_eq!(ir["predelay"], 25.0);
        assert_eq!(ir["width"], 80.0);
        // Back to a Hall: the levels in %, the width kept to the Reverb's 100.
        let get = |k: &str| match k {
            "width" => 150.0,
            k => ir[k],
        };
        let back: std::collections::HashMap<String, f64> =
            from_impulse(get, 1).into_iter().collect();
        assert_eq!(back["kind"], 1.0);
        assert!((back["dry"] - 100.0).abs() < 1e-9);
        assert_eq!(back["wet"], 0.0);
        assert_eq!(back["high_cut"], 8000.0);
        assert_eq!(back["width"], 100.0);
        // -6 dB is about half.
        assert!((super::to_pct(-6.0) - 50.1).abs() < 0.1);
        // The carried values land on the other effect's keys, in range.
        let v = crate::fx_windows::values_for(super::IMPULSE_ID, &to_impulse(verb));
        assert_eq!(v.len(), crate::fx::controls(super::IMPULSE_ID).len());
    }
}
