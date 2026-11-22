// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The insides of four effect windows, rebuilt after the project lead's
//! Audiform windows (see docs/clean-room.md):
//!
//! * **Amp Model** (`AmpPanel.svelte`): a dark faceplate under a woven
//!   grille band; the capture with Load and Clear, then four sections:
//!   Amp (the Gate switch over its threshold, Input, Boost, Tight, Bass,
//!   Middle, Treble, Output), Cab (the Cabinet switch, the built-in
//!   cabinet, Level Match, and an optional loaded IR), Mic (two mics,
//!   each a kind, Position and Distance, with Blend between them) and
//!   Room (Room, Size and Double); a note.
//! * **Tube Preamp** and **Opto Leveler** (`TubePanel.svelte`,
//!   `NeedleMeter.svelte`): a charcoal faceplate with brass trim, big
//!   numbered dials either side of a cream needle meter with lever
//!   switches under it, and small dials for the rest.
//! * **Limiter** (`LimiterPanel.svelte`, `SliderRow.svelte`): Peak or
//!   True peak at the top, long numbered sliders for the maximum level,
//!   input boost, look-ahead and release, and Link channels.
//!
//! Each faceplate's own colours are blended into the theme (mostly its
//! own on a dark theme, a tint on a light one), so the dials and value
//! boxes read on it. The needle follows the level worked out from the
//! rack's meters, as the effect's own meter is not plumbed to the app.

use egui::{pos2, vec2, Align2, Color32, FontId, Rect, RichText, Rounding, Sense, Shape, Stroke};

use super::layout::{self, GAP, GAP_S};
use super::parts::{self, comp_gr};
use super::{Env, Fx};
use crate::theme::{c, ca, lerp, Rgb, Theme};
use crate::widgets;

// ------------------------------------------------------------ Looks

/// A faceplate's colours: face (top, bottom), ink, light and trim.
struct Look {
    face: Rgb,
    face2: Rgb,
    ink: Rgb,
    glow: Rgb,
    trim: Rgb,
}

/// The Amp Model's: dark green-black with a hot orange light.
const AMP: Look = Look {
    face: [0x2b, 0x2d, 0x2c],
    face2: [0x1d, 0x1f, 0x1e],
    ink: [0xe4, 0xe6, 0xe1],
    glow: [0xe2, 0x68, 0x3c],
    trim: [0x3c, 0x40, 0x3e],
};

/// The tube units': charcoal with amber ink and brass trim.
const TUBE: Look = Look {
    face: [0x2c, 0x2d, 0x30],
    face2: [0x1f, 0x20, 0x22],
    ink: [0xea, 0xdf, 0xc4],
    glow: [0xd9, 0xa4, 0x41],
    trim: [0x8a, 0x6a, 0x2d],
};

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = lerp(a, b, t);
    [m.r(), m.g(), m.b()]
}

/// The theme with a faceplate's colours blended in.
fn tinted(th: &Theme, look: &Look) -> Theme {
    let mut t = th.clone();
    if th.dark {
        t.panel_raised = mix(th.panel_raised, look.face, 0.85);
        t.background = mix(th.background, look.face2, 0.85);
        t.panel = mix(th.panel, look.face2, 0.7);
        t.accent = mix(th.accent, look.glow, 0.9);
        t.text = mix(th.text, look.ink, 0.75);
        t.text_dim = mix(th.text_dim, mix(look.ink, look.face, 0.45), 0.6);
        t.border = mix(th.border, look.face2, 0.4);
    } else {
        t.panel_raised = mix(th.panel_raised, look.glow, 0.10);
        t.background = mix(th.background, look.glow, 0.08);
        t.panel = mix(th.panel, look.glow, 0.06);
        t.accent = mix(mix(look.glow, [0, 0, 0], 0.4), th.accent, 0.2);
        t.accent_alt = mix(th.accent_alt, t.accent, 0.5);
    }
    t
}

/// Draws `add` with `th` as the theme the dials and boxes read.
fn with_theme(ui: &mut egui::Ui, th: &Theme, add: impl FnOnce(&mut egui::Ui)) {
    let was = widgets::theme(ui);
    widgets::set_theme(ui.ctx(), th);
    add(ui);
    widgets::set_theme(ui.ctx(), &was);
}

/// A faceplate `w` wide inside its margin (`top` more room at the top),
/// its face blended from top to bottom; `trim` draws an inset line of the
/// look's trim colour. Returns the plate's rect.
fn plate(
    ui: &mut egui::Ui,
    th: &Theme,
    look: &Look,
    w: f32,
    top: f32,
    add: impl FnOnce(&mut egui::Ui),
) -> Rect {
    let back = ui.painter().add(Shape::Noop);
    let rect = egui::Frame::none()
        .inner_margin(egui::Margin {
            left: GAP,
            right: GAP,
            top: GAP_S + top,
            bottom: GAP,
        })
        .show(ui, |ui| {
            ui.set_width(w);
            ui.vertical(add);
        })
        .response
        .rect;
    let square = th.bevel.is_some();
    let rad = if square { 0.0 } else { 6.0 };
    let mut shapes = parts::vgrad(rect, rad, c(th.panel_raised), c(th.background));
    let trim = if th.dark { c(look.trim) } else { ca(look.trim, 120) };
    shapes.push(Shape::rect_stroke(rect.shrink(1.5), rad, Stroke::new(2.0_f32, trim)));
    shapes.push(Shape::rect_stroke(rect, rad, Stroke::new(1.0_f32, Color32::from_black_alpha(200))));
    ui.painter().set(back, Shape::Vec(shapes));
    rect
}

fn note(ui: &mut egui::Ui, th: &Theme, w: f32, text: &str) {
    layout::note(ui, w, RichText::new(text).size(10.5).color(c(th.text_dim)));
}

/// A lit push switch: an LED and a name in capitals.
fn led_switch(ui: &mut egui::Ui, th: &Theme, on: bool, label: &str, tip: &str) -> egui::Response {
    let font = FontId::proportional(10.0);
    let text = label.to_uppercase();
    let tw = ui.fonts(|f| f.layout_no_wrap(text.clone(), font.clone(), Color32::WHITE).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(tw + 30.0, 22.0), Sense::click());
    let p = ui.painter_at(rect.expand(4.0));
    let hot = resp.hovered();
    let top = lerp(th.panel_raised, th.text, if hot { 0.16 } else { 0.10 });
    let bottom = lerp(th.panel_raised, th.background, 0.5);
    p.add(Shape::mesh(parts::blend(rect, top, bottom)));
    p.rect_stroke(rect, Rounding::same(3.0), Stroke::new(1.0_f32, Color32::from_black_alpha(200)));
    let led = pos2(rect.left() + 12.0, rect.center().y);
    if on {
        p.circle_filled(led, 6.0, ca(th.accent, 50));
        p.circle_filled(led, 3.5, c(th.accent));
    } else {
        p.circle_filled(led, 3.5, lerp(th.accent, th.background, 0.78));
        p.circle_stroke(led, 3.5, Stroke::new(1.0_f32, Color32::from_black_alpha(160)));
    }
    p.text(
        pos2(rect.left() + 21.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        font,
        c(th.text),
    );
    resp.on_hover_text(tip)
}

/// A switch control drawn as [`led_switch`].
fn led_toggle(ui: &mut egui::Ui, th: &Theme, fx: &mut Fx, key: &str, label: &str) {
    let on = fx.on(key);
    let name = fx.ctl(key).map(|c| c.name.clone()).unwrap_or_default();
    let tip = format!("{name}: {}", if on { "on" } else { "off" });
    if led_switch(ui, th, on, label, &tip).clicked() {
        fx.set(key, if on { 0.0 } else { 1.0 });
    }
}

// ------------------------------------------------------------ Amp Model

const AMP_W: f32 = 540.0;
const GRILLE_H: f32 = 14.0;

/// The woven band across the top of the Amp Model's plate.
fn grille(ui: &egui::Ui, plate: Rect) {
    let band = Rect::from_min_max(
        pos2(plate.left() + 1.0, plate.top() + 1.0),
        pos2(plate.right() - 1.0, plate.top() + 1.0 + GRILLE_H),
    );
    let p = ui.painter_at(band);
    p.rect_filled(band, Rounding::ZERO, Color32::from_rgb(0x3a, 0x35, 0x29));
    let n = ((band.width() + band.height()) / 5.0) as i32 + 1;
    for k in 0..n {
        let x = band.left() + k as f32 * 5.0;
        p.line_segment(
            [pos2(x, band.bottom()), pos2(x + band.height(), band.top())],
            Stroke::new(2.0_f32, Color32::from_white_alpha(14)),
        );
        p.line_segment(
            [pos2(x, band.top()), pos2(x + band.height(), band.bottom())],
            Stroke::new(2.0_f32, Color32::from_black_alpha(80)),
        );
    }
    p.line_segment(
        [pos2(band.left(), band.bottom() + 1.0), pos2(band.right(), band.bottom() + 1.0)],
        Stroke::new(2.0_f32, Color32::from_rgb(0x6b, 0x5f, 0x45)),
    );
}

/// One file in use: what it is, its name (or what happens without it) and
/// a line about it, with Load and Clear. Returns (load, clear) clicks.
fn file_row(
    ui: &mut egui::Ui,
    th: &Theme,
    w: f32,
    what: &str,
    name: Option<(&str, &str, &str)>,
    empty: &str,
) -> (bool, bool) {
    let mut out = (false, false);
    egui::Frame::none()
        .fill(lerp(th.background, [0, 0, 0], 0.45))
        .stroke(Stroke::new(1.0_f32, Color32::from_black_alpha(220)))
        .rounding(4.0)
        .inner_margin(egui::Margin::symmetric(8.0, 6.0))
        .show(ui, |ui| {
            let inner = w - 16.0;
            ui.set_width(inner);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (r, _) = ui.allocate_exact_size(vec2(64.0, 30.0), Sense::hover());
                ui.painter().text(
                    r.left_center(),
                    Align2::LEFT_CENTER,
                    what.to_uppercase(),
                    FontId::proportional(10.0),
                    c(th.accent),
                );
                let btn_w = 118.0;
                let name_w = (inner - 64.0 - btn_w - 16.0).max(60.0);
                ui.allocate_ui(vec2(name_w, 30.0), |ui| {
                    ui.set_width(name_w);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        match name {
                            Some((n, meta, tip)) => {
                                let l = ui.add(
                                    egui::Label::new(RichText::new(n).size(12.0).color(c(th.text)))
                                        .truncate(),
                                );
                                if !tip.is_empty() {
                                    l.on_hover_text(tip);
                                }
                                ui.label(RichText::new(meta).size(10.0).color(ca(th.text, 140)));
                            }
                            None => {
                                ui.add_space(6.0);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(empty).size(11.0).color(ca(th.text, 128)),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(empty);
                            }
                        }
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if name.is_some() && widgets::button(ui, "Clear", false).clicked() {
                        out.1 = true;
                    }
                    if widgets::button(ui, "Load...", false).clicked() {
                        out.0 = true;
                    }
                });
            });
        });
    out
}

fn khz(rate: f64) -> String {
    let k = rate / 1000.0;
    if (k - k.round()).abs() < 1e-6 {
        format!("{k:.0}")
    } else {
        format!("{k}")
    }
}

/// A section heading on the Amp Model's plate: the name in small
/// capitals and a rule across the rest of the width.
fn amp_section(ui: &mut egui::Ui, th: &Theme, w: f32, title: &str) {
    ui.add_space(2.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
    let p = ui.painter();
    let text = p.layout_no_wrap(
        title.to_uppercase(),
        FontId::proportional(10.5),
        c(th.accent),
    );
    let tw = text.size().x;
    p.galley(pos2(r.left(), r.center().y - text.size().y / 2.0), text, c(th.accent));
    p.line_segment(
        [pos2(r.left() + tw + 8.0, r.center().y), pos2(r.right(), r.center().y)],
        Stroke::new(1.0_f32, ca(th.text, 40)),
    );
}

/// One mic's column: its name, the kind and its Position and Distance.
fn mic_column(ui: &mut egui::Ui, th: &Theme, fx: &mut Fx, w: f32, which: &str) {
    ui.vertical(|ui| {
        ui.set_width(w);
        layout::centred(ui, w, |ui| {
            ui.label(
                RichText::new(format!("MIC {}", which.to_uppercase()))
                    .size(10.0)
                    .color(ca(th.text, 170)),
            );
        });
        layout::centred(ui, w, |ui| {
            fx.choice(
                ui,
                &format!("mic_{which}"),
                &[
                    "Dynamic: a forward presence peak, tight lows",
                    "Ribbon: dark and round, more proximity bass",
                    "Condenser: open and airy, most detail",
                ],
            );
        });
        let pos = format!("pos_{which}");
        let dist = format!("dist_{which}");
        let pan = format!("pan_{which}");
        layout::row(ui, w, &[1.0, 1.0, 1.0], |ui, i| match i {
            0 => fx.knob_fmt(ui, &pos, 40.0, "Position", &[], &|v| {
                if v < 1.0 {
                    "Centre".into()
                } else if v > 99.0 {
                    "Edge".into()
                } else {
                    format!("{v:.0}%")
                }
            }),
            1 => fx.knob(ui, &dist, 40.0, "Distance", &[]),
            _ => fx.knob_fmt(ui, &pan, 40.0, "Pan", &[0.0], &|v| {
                if v.abs() < 1.0 {
                    "C".into()
                } else if v < 0.0 {
                    format!("L{:.0}", -v)
                } else {
                    format!("R{v:.0}")
                }
            }),
        });
    });
}

/// The amp slot's summary, read again only when its data changes: the
/// data holds the whole capture (often hundreds of KB of JSON), too much
/// to parse every frame. The data is told apart by its length and ends.
fn amp_summary(data: Option<&str>) -> af2_fx::effects::amp::Summary {
    use std::hash::{Hash, Hasher};
    thread_local! {
        static LAST: std::cell::RefCell<Option<(u64, af2_fx::effects::amp::Summary)>> =
            const { std::cell::RefCell::new(None) };
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let Some(d) = data {
        let b = d.as_bytes();
        b.len().hash(&mut h);
        b[..b.len().min(4096)].hash(&mut h);
        b[b.len().saturating_sub(4096)..].hash(&mut h);
    }
    let key = h.finish();
    LAST.with(|l| {
        let mut l = l.borrow_mut();
        match &*l {
            Some((k, s)) if *k == key => s.clone(),
            _ => {
                let s = af2_fx::effects::amp::summary(data);
                *l = Some((key, s.clone()));
                s
            }
        }
    })
}

pub fn amp(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    use af2_fx::effects::{amp, amp_rig};
    let th = tinted(&env.th, &AMP);
    let sum = amp_summary(fx.data.as_deref());
    let failed_id = env.key.with("amp-failed");
    let failed: Option<String> = ui.ctx().data(|d| d.get_temp(failed_id));
    with_theme(ui, &th, |ui| {
        let rect = plate(ui, &th, &AMP, AMP_W, GRILLE_H, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            // The capture.
            let meta = sum.model.as_ref().map(|m| {
                let mut s = m.kind.clone();
                if let Some(r) = m.rate {
                    s += &format!(" · {} kHz", khz(r));
                }
                if !m.gear.is_empty() {
                    s += &format!(" · {}", m.gear);
                }
                s
            });
            let (load_m, clear_m) = file_row(
                ui,
                &th,
                AMP_W,
                "Capture",
                sum.model
                    .as_ref()
                    .map(|m| (m.name.as_str(), meta.as_deref().unwrap_or(""), m.detail.as_str())),
                "None loaded: the rig plays without an amp in it.",
            );
            let warn = |ui: &mut egui::Ui, t: &str| {
                ui.add(
                    egui::Label::new(
                        RichText::new(t)
                            .size(10.5)
                            .color(Color32::from_rgb(0xf0, 0xb4, 0x6a)),
                    )
                    .wrap(),
                );
            };
            if let Some(f) = &failed {
                warn(ui, f);
            }
            if let Some(p) = &sum.problem {
                warn(ui, p);
            }
            if let Some(r) = sum.model.as_ref().and_then(|m| m.rate) {
                let here = f64::from(env.rate);
                if (r - here).abs() > 1.0 {
                    warn(
                        ui,
                        &format!(
                            "This capture was made at {} kHz and plays here at {} kHz, so its tone shifts a little. A session at {} kHz plays it exactly.",
                            khz(r),
                            khz(here),
                            khz(r)
                        ),
                    );
                }
            }
            if sum.model.as_ref().is_some_and(|m| m.has_cab) && fx.on("cab") {
                warn(
                    ui,
                    "This capture already has a cabinet in it: turn Cabinet off below, or it plays through two.",
                );
            }
            let ctx = ui.ctx().clone();
            let set_failed = move |msg: Option<String>| {
                ctx.data_mut(|d| match msg {
                    Some(m) => d.insert_temp(failed_id, m),
                    None => d.remove::<String>(failed_id),
                });
            };
            if load_m {
                if let Some(path) = crate::file_dialog::FileDialog::new()
                    .add_filter("Amp capture", &["nam"])
                    .pick_file()
                {
                    match amp::with_model(fx.data.as_deref(), &path) {
                        Ok(d) => {
                            set_failed(None);
                            fx.new_data = Some(d);
                        }
                        Err(e) => set_failed(Some(format!("That capture did not load: {e}"))),
                    }
                }
            }
            if clear_m {
                fx.new_data = Some(amp::without(fx.data.as_deref(), true));
            }

            // Amp: the gate and boost before the capture, its tone after.
            amp_section(ui, &th, AMP_W, "Amp");
            layout::row(ui, AMP_W, &[1.15, 1.1, 1.0, 1.0, 1.0, 1.0, 1.0, 1.1], |ui, i| match i {
                0 => {
                    ui.vertical_centered(|ui| {
                        led_toggle(ui, &th, fx, "gate", "Gate");
                        ui.add_space(2.0);
                        fx.knob(ui, "gate_db", 44.0, "Threshold", &[]);
                    });
                }
                1 => fx.knob(ui, "input", 58.0, "", &[]),
                2 => fx.knob(ui, "boost", 50.0, "", &[]),
                3 => fx.knob(ui, "tight", 50.0, "", &[]),
                4 => fx.knob(ui, "bass", 50.0, "", &[]),
                5 => fx.knob(ui, "middle", 50.0, "", &[]),
                6 => fx.knob(ui, "treble", 50.0, "", &[]),
                _ => fx.knob(ui, "output", 58.0, "", &[]),
            });

            // Cab: a built-in cabinet, or the loaded response.
            amp_section(ui, &th, AMP_W, "Cab");
            let ir_used = fx.on("cab") && fx.on("use_ir") && sum.ir.is_some();
            layout::centred(ui, AMP_W, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    led_toggle(ui, &th, fx, "cab", "Cabinet");
                    let cur = (fx.get("cab_type").round().max(0.0) as usize)
                        .min(amp_rig::CABS.len() - 1);
                    ui.add_enabled_ui(fx.on("cab") && !ir_used, |ui| {
                        egui::ComboBox::from_id_salt(fx.uid.with("cab-type"))
                            .selected_text(amp_rig::CABS[cur])
                            .width(170.0)
                            .show_ui(ui, |ui| {
                                for (k, n) in amp_rig::CABS.iter().enumerate() {
                                    if ui.selectable_label(k == cur, *n).clicked() {
                                        fx.set("cab_type", k as f64);
                                    }
                                }
                            })
                            .response
                            .on_hover_text("The built-in cabinet the mics are on");
                    });
                    if sum.ir.is_some() {
                        led_toggle(ui, &th, fx, "use_ir", "Use IR");
                    }
                    led_toggle(ui, &th, fx, "level", "Level Match");
                });
            });
            let ir_meta = sum
                .ir
                .as_ref()
                .map(|(_, ms, rate)| format!("{ms} ms · {} kHz", khz(f64::from(*rate))));
            let (load_c, clear_c) = file_row(
                ui,
                &th,
                AMP_W,
                "Cab IR",
                sum.ir
                    .as_ref()
                    .map(|(n, _, _)| (n.as_str(), ir_meta.as_deref().unwrap_or(""), "")),
                "Optional: load your own cabinet impulse response (.wav).",
            );
            if load_c {
                if let Some(path) = crate::file_dialog::FileDialog::new()
                    .add_filter("Audio", &["wav", "flac", "aif", "aiff", "mp3", "ogg"])
                    .pick_file()
                {
                    match amp::with_ir(fx.data.as_deref(), &path) {
                        Ok(d) => {
                            set_failed(None);
                            fx.new_data = Some(d);
                        }
                        Err(e) => set_failed(Some(format!("That cabinet did not load: {e}"))),
                    }
                }
            }
            if clear_c {
                fx.new_data = Some(amp::without(fx.data.as_deref(), false));
            }

            // Mic: two mics on the built-in cabinet, blended.
            amp_section(ui, &th, AMP_W, "Mic");
            ui.add_enabled_ui(fx.on("cab") && !ir_used, |ui| {
                let side = 3.0;
                let mid = 1.1;
                let col_w = AMP_W * side / (2.0 * side + mid);
                layout::row(ui, AMP_W, &[side, mid, side], |ui, i| match i {
                    0 => mic_column(ui, &th, fx, col_w, "a"),
                    1 => fx.knob_fmt(ui, "mic_blend", 54.0, "Blend", &[], &|v| {
                        if v < 1.0 {
                            "A".into()
                        } else if v > 99.0 {
                            "B".into()
                        } else {
                            format!("{:.0}/{v:.0}", 100.0 - v)
                        }
                    }),
                    _ => mic_column(ui, &th, fx, col_w, "b"),
                });
            });

            // Room: ambience and the doubler.
            amp_section(ui, &th, AMP_W, "Room");
            layout::row(ui, AMP_W, &[1.0, 1.0, 1.0], |ui, i| match i {
                0 => fx.knob(ui, "room", 54.0, "Room", &[]),
                1 => fx.knob(ui, "room_size", 54.0, "Size", &[]),
                _ => fx.knob(ui, "double", 54.0, "Double", &[]),
            });
            note(
                ui,
                &th,
                AMP_W,
                if ir_used {
                    "Your loaded IR stands in for the built-in cabinet and mics (Use IR off to hear them). Boost and Tight push and tighten the amp; Room and Double make it stereo."
                } else {
                    "Boost and Tight push the amp harder and tighten its lows, like a pedal in front. Position runs from the cone's centre (bright) to its edge (dark); Blend mixes the two mics. Room and Double make it stereo."
                },
            );
        });
        grille(ui, rect);
    });
}

// ------------------------------------------------------------ Tube units

const TUBE_W: f32 = 600.0;
const BIG: f32 = 108.0;
const SMALL: f32 = 58.0;
/// Audiform's numbered dial scale.
const NUMS: [f64; 11] = [0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0];

/// The title strip: the name in amber capitals and what it is.
fn tube_title(ui: &mut egui::Ui, th: &Theme, w: f32, name: &str, sub: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(
            RichText::new(name.to_uppercase())
                .size(15.0)
                .strong()
                .extra_letter_spacing(1.8)
                .color(c(th.accent)),
        );
        ui.label(
            RichText::new(sub)
                .size(10.0)
                .italics()
                .extra_letter_spacing(0.8)
                .color(ca(th.text, 150)),
        );
    });
    let (x, y) = (ui.cursor().left(), ui.cursor().top() + 2.0);
    ui.painter().line_segment([pos2(x, y), pos2(x + w, y)], Stroke::new(1.0_f32, ca(th.accent, 90)));
    ui.add_space(GAP_S);
}

/// The rule above the small dials.
fn tube_rule(ui: &mut egui::Ui, th: &Theme, w: f32) {
    ui.add_space(2.0);
    let (x, y) = (ui.cursor().left(), ui.cursor().top());
    ui.painter().line_segment([pos2(x, y), pos2(x + w, y)], Stroke::new(1.0_f32, ca(th.accent, 52)));
    ui.add_space(6.0);
}

/// A lever switch: `key`'s choice 1 named above (`b`), 0 below (`a`); the
/// lit name is the one the bat points to.
fn lever(ui: &mut egui::Ui, th: &Theme, fx: &mut Fx, key: &str, a: &str, b: &str) {
    let on = fx.get(key).round() as i64 == 1;
    let name = fx.ctl(key).map(|c| c.name.clone()).unwrap_or_default();
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        let word = |ui: &mut egui::Ui, t: &str, lit: bool| {
            ui.label(
                RichText::new(t.to_uppercase())
                    .size(9.0)
                    .extra_letter_spacing(0.9)
                    .color(if lit { c(th.accent) } else { ca(th.text, 115) }),
            );
        };
        word(ui, b, on);
        let (r, resp) = ui.allocate_exact_size(vec2(22.0, 30.0), Sense::click());
        let p = ui.painter();
        p.rect_filled(r, Rounding::same(11.0), lerp(th.panel_raised, th.text, 0.18));
        p.rect_stroke(r, Rounding::same(11.0), Stroke::new(1.0_f32, Color32::BLACK));
        p.circle_filled(r.center(), 6.0, lerp(th.background, [0, 0, 0], 0.4));
        let tip = if on {
            pos2(r.center().x, r.top() + 3.0)
        } else {
            pos2(r.center().x, r.bottom() - 3.0)
        };
        p.line_segment([r.center(), tip], Stroke::new(6.0_f32, Color32::from_rgb(0x8d, 0x8f, 0x94)));
        p.line_segment([r.center(), tip], Stroke::new(2.5_f32, Color32::from_rgb(0xe8, 0xe9, 0xec)));
        word(ui, a, !on);
        if resp
            .on_hover_text(format!("{name}: {}. Click to switch.", if on { b } else { a }))
            .clicked()
        {
            fx.set(key, if on { 0.0 } else { 1.0 });
        }
    });
}

/// Where a reading falls on the needle scale (0..1), spread by amplitude.
fn needle_pos(db: f64) -> f64 {
    let g = |d: f64| 10f64.powf(d.clamp(-20.0, 3.0) / 20.0);
    (g(db) - g(-20.0)) / (g(3.0) - g(-20.0))
}

/// Audiform's needle meter: a cream face lit from below, the scale from
/// -20 to +3 with the red above 0, its name, and a smooth needle, in a
/// dark bezel. `reading` is dB against the 0 mark.
fn vu(ui: &mut egui::Ui, id: egui::Id, reading: f64, caption: &str, width: f32) {
    let h = width * 0.5;
    let (outer, _) = ui.allocate_exact_size(vec2(width + 12.0, h + 12.0), Sense::hover());
    let shown = parts::ballistic(ui, id, reading, 0.12);
    let p = ui.painter_at(outer);
    p.rect_filled(outer, Rounding::same(6.0), Color32::from_rgb(0x14, 0x13, 0x12));
    let rect = outer.shrink(6.0);
    // The face: warm cream, brighter low in the middle.
    let mut face = egui::Mesh::default();
    let (rim, mid) = (Color32::from_rgb(0xd9, 0xbf, 0x86), Color32::from_rgb(0xf6, 0xe7, 0xc1));
    face.colored_vertex(pos2(rect.center().x, rect.top() + h * 0.85), mid);
    let n = 24;
    for k in 0..=n {
        let t = k as f32 / n as f32 * std::f32::consts::TAU;
        let (s, co) = t.sin_cos();
        let q = pos2(
            (rect.center().x + co * width).clamp(rect.left(), rect.right()),
            (rect.top() + h * 0.85 + s * h * 1.3).clamp(rect.top(), rect.bottom()),
        );
        face.colored_vertex(q, rim);
        if k > 0 {
            face.add_triangle(0, k, k + 1);
        }
    }
    p.add(Shape::mesh(face));
    let ink = Color32::from_rgb(0x2a, 0x24, 0x20);
    let red = Color32::from_rgb(0xb3, 0x26, 0x1e);
    let pivot = pos2(rect.center().x, rect.top() + h * 1.55);
    let radius = h * 1.02;
    let ang = |u: f64| (-48.0 + u * 96.0 - 90.0f64).to_radians() as f32;
    let at = |u: f64, r: f32| pivot + vec2(ang(u).cos(), ang(u).sin()) * r;
    let arc = |from: f64, to: f64| -> Vec<egui::Pos2> {
        (0..=40).map(|k| at(from + (to - from) * k as f64 / 40.0, radius)).collect()
    };
    let zero = needle_pos(0.0);
    p.add(Shape::line(arc(0.0, 1.0), Stroke::new(1.4_f32, ink)));
    p.add(Shape::line(arc(zero, 1.0), Stroke::new(4.0_f32, red)));
    for m in [-20.0, -10.0, -7.0, -5.0, -3.0, -2.0, -1.0, 0.0, 1.0, 2.0, 3.0] {
        let u = needle_pos(m);
        let col = if m > 0.0 { red } else { ink };
        p.line_segment([at(u, radius), at(u, radius + 6.0)], Stroke::new(1.2_f32, col));
        p.text(
            at(u, radius + 14.0),
            Align2::CENTER_CENTER,
            if m > 0.0 { format!("+{m}") } else { format!("{m}") },
            FontId::proportional(9.0),
            col,
        );
    }
    p.text(
        pos2(rect.center().x, rect.bottom() - 24.0),
        Align2::CENTER_CENTER,
        caption,
        FontId::proportional(8.0),
        Color32::from_rgb(0x5a, 0x4c, 0x3c),
    );
    let tip = at(needle_pos(shown), radius + 4.0);
    p.line_segment([pivot, tip], Stroke::new(1.6_f32, Color32::from_rgb(0x1a, 0x15, 0x12)));
    let hood = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 14.0), rect.max);
    p.rect_filled(hood, Rounding::ZERO, Color32::from_rgb(0x1c, 0x1a, 0x18));
}

/// The rack's output in dB while playing.
fn output_db(env: &Env) -> f64 {
    if !env.playing {
        return -90.0;
    }
    env.levels.map_or(-90.0, |(_, o)| Env::db(o))
}

fn input_db(env: &Env) -> Option<f64> {
    env.playing.then_some(())?;
    env.levels.map(|(i, _)| Env::db(i))
}

pub fn preamp(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = tinted(&env.th, &TUBE);
    let reading = output_db(env) + 18.0;
    with_theme(ui, &th, |ui| {
        plate(ui, &th, &TUBE, TUBE_W, 0.0, |ui| {
            tube_title(ui, &th, TUBE_W, "Tube Preamp", "vintage tube preamplifier");
            layout::row(ui, TUBE_W, &[1.0, 1.0, 1.9, 1.0], |ui, i| match i {
                0 => fx.knob(ui, "input", BIG, "", &[-12.0, 0.0, 12.0, 24.0]),
                1 => fx.knob_fmt(ui, "drive", BIG, "", &NUMS, &|v| format!("{v:.0} %")),
                2 => {
                    ui.vertical_centered(|ui| {
                        vu(ui, env.key.with("needle"), reading, "OUTPUT", 200.0);
                        ui.add_space(GAP_S);
                        lever(ui, &th, fx, "transformer", "Clean", "Transformer");
                    });
                }
                _ => fx.knob(ui, "output", BIG, "", &[-24.0, -12.0, 0.0, 12.0]),
            });
            tube_rule(ui, &th, TUBE_W);
            small_row(ui, fx, &["low", "high", "mix"]);
            note(
                ui,
                &th,
                TUBE_W,
                "Drive adds warmth; past halfway it starts to sound overdriven. The needle shows the output against a 0 mark at -18 dBFS.",
            );
        });
    });
}

/// Small dials in a centred row, 22 apart.
fn small_row(ui: &mut egui::Ui, fx: &mut Fx, keys: &[&str]) {
    let cell = SMALL.max(66.0) + 22.0;
    let n = keys.len() as f32;
    let side = ((TUBE_W - cell * n) / 2.0).max(0.0);
    let mut weights = vec![side];
    weights.extend(keys.iter().map(|_| cell));
    weights.push(side);
    layout::row(ui, TUBE_W, &weights, |ui, i| {
        if i >= 1 && i <= keys.len() {
            fx.knob(ui, keys[i - 1], SMALL, "", &[]);
        }
    });
}

/// The Opto Leveler's static reduction for the needle (Peak Reduction
/// lowers the threshold to 60 dB down; Compress about 3:1 with a wide
/// knee, Limit about 20:1 with a narrow one).
fn opto_gr(fx: &Fx, input: f64) -> f64 {
    let thr = -fx.get("peak") * 0.6;
    if fx.get("mode").round() as i64 == 1 {
        comp_gr(input, thr, 20.0, 3.0)
    } else {
        comp_gr(input, thr, 3.0, 12.0)
    }
}

pub fn opto(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = tinted(&env.th, &TUBE);
    let target = input_db(env).map_or(0.0, |i| opto_gr(fx, i));
    let gr = parts::est_gr(ui, env.key.with("gr"), target, 10.0, 300.0);
    let out_mode = fx.get("meter").round() as i64 == 1;
    let reading = if out_mode { output_db(env) + 18.0 } else { -gr };
    with_theme(ui, &th, |ui| {
        plate(ui, &th, &TUBE, TUBE_W, 0.0, |ui| {
            tube_title(ui, &th, TUBE_W, "Opto Leveler", "optical leveling amplifier");
            layout::row(ui, TUBE_W, &[1.0, 1.9, 1.0], |ui, i| match i {
                0 => fx.knob_fmt(ui, "gain", BIG, "", &NUMS, &|v| format!("{v:.1}")),
                1 => {
                    ui.vertical_centered(|ui| {
                        vu(
                            ui,
                            env.key.with("needle"),
                            reading,
                            if out_mode { "OUTPUT" } else { "GAIN REDUCTION" },
                            230.0,
                        );
                        ui.add_space(GAP_S);
                        layout::cells(ui, 200.0, 2, |ui, k| match k {
                            0 => lever(ui, &th, fx, "mode", "Compress", "Limit"),
                            _ => lever(ui, &th, fx, "meter", "Reduction", "Output"),
                        });
                    });
                }
                _ => fx.knob_fmt(ui, "peak", BIG, "", &NUMS, &|v| format!("{v:.1}")),
            });
            tube_rule(ui, &th, TUBE_W);
            small_row(ui, fx, &["emphasis", "tube", "mix"]);
            note(
                ui,
                &th,
                TUBE_W,
                "Turn up Peak Reduction until the needle shows a few dB of leveling, then set Gain for the level you want. Emphasis makes it listen harder to \"s\" sounds.",
            );
        });
    });
}

// ------------------------------------------------------------ Limiter

const LIM_W: f32 = 520.0;
const NAME_W: f32 = 112.0;
const BOX_W: f32 = 74.0;

/// One setting as a long slider over a numbered scale, with its value in
/// a box to type into (Audiform's slider row). Log settings spread their
/// scale to match; double-click the slider for the default.
fn slider_row(ui: &mut egui::Ui, th: &Theme, fx: &mut Fx, key: &str, marks: &[f64], w: f32) {
    let Some(k) = fx.k(key) else { return };
    let ctl = fx.ctls[k].clone();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.text(
        pos2(rect.left(), rect.center().y + 4.0),
        Align2::LEFT_CENTER,
        &ctl.name,
        FontId::proportional(12.0),
        c(th.text),
    );
    let track = Rect::from_min_max(
        pos2(rect.left() + NAME_W + 8.0, rect.center().y + 2.0),
        pos2(rect.right() - BOX_W - 14.0, rect.center().y + 6.0),
    );
    let pos = |v: f64| widgets::to_pos(v, ctl.min, ctl.max, ctl.log) as f32;
    let x_of = |t: f32| track.left() + t.clamp(0.0, 1.0) * track.width();
    for &m in marks {
        let x = x_of(pos(m));
        let t = if m.abs() >= 1000.0 { format!("{}k", m / 1000.0) } else { format!("{m}") };
        p.text(pos2(x, track.top() - 4.0), Align2::CENTER_BOTTOM, t, FontId::proportional(9.0), c(th.text_dim));
        p.line_segment(
            [pos2(x, track.bottom() + 3.0), pos2(x, track.bottom() + 7.0)],
            Stroke::new(1.0_f32, ca(th.text_dim, 170)),
        );
    }
    let v = f64::from(fx.vals[k]);
    let at = pos(v);
    p.rect_filled(track, Rounding::same(2.0), ca(th.text_dim, 60));
    p.rect_filled(
        Rect::from_min_max(track.min, pos2(x_of(at), track.max.y)),
        Rounding::same(2.0),
        c(th.accent),
    );
    let hit = Rect::from_min_max(pos2(track.left() - 8.0, rect.top() + 8.0), pos2(track.right() + 8.0, rect.bottom()));
    let resp = ui
        .interact(hit, fx.uid.with(("slider", key)), Sense::click_and_drag())
        .on_hover_text(format!("{}. Double-click to reset.", ctl.name));
    let hot = resp.hovered() || resp.dragged();
    let knob = pos2(x_of(at), track.center().y);
    p.circle_filled(knob, if hot { 7.5 } else { 6.5 }, c(th.panel_raised));
    p.circle_stroke(knob, if hot { 7.5 } else { 6.5 }, Stroke::new(2.0_f32, if hot { c(th.accent) } else { c(th.text) }));
    if resp.double_clicked() {
        fx.set(key, ctl.default);
    } else if let Some(pp) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
        let fine = ui.input(|i| i.modifiers.ctrl || i.modifiers.shift);
        let t = if fine && resp.dragged() {
            at + resp.drag_delta().x / track.width().max(1.0) * 0.1
        } else {
            (pp.x - track.left()) / track.width().max(1.0)
        };
        let nv = widgets::from_pos(f64::from(t.clamp(0.0, 1.0)), ctl.min, ctl.max, ctl.log);
        if resp.clicked() {
            fx.set(key, nv);
        } else {
            fx.set_live(key, nv);
        }
    }
    if resp.drag_stopped() {
        fx.released = true;
    }
    let well = Rect::from_min_size(pos2(rect.right() - BOX_W, rect.center().y - 5.0), vec2(BOX_W, 18.0));
    let mut bv = f64::from(fx.vals[k]);
    let unit = ctl.unit.clone();
    let b = ui
        .allocate_new_ui(egui::UiBuilder::new().max_rect(well), |ui| {
            widgets::value_box(ui, fx.uid.with(("box", key)), &mut bv, ctl.min, ctl.max, ctl.log, BOX_W, &|v| {
                widgets::fmt_unit(v, &unit)
            })
        })
        .inner;
    if b.changed() {
        fx.vals[k] = bv.clamp(ctl.min, ctl.max) as f32;
    }
    fx.track(&b);
}

pub fn limiter(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = env.th.clone();
    ui.vertical(|ui| {
        ui.set_width(LIM_W);
        ui.add_space(GAP_S);
        let detect = fx.get("detect").round() as usize;
        layout::centred(ui, LIM_W, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 22.0;
                let tips = [
                    "Holds every sample under the maximum level",
                    "Also holds the level a player draws between samples, so the file never goes over once it is played or converted",
                ];
                for (i, name) in ["Peak", "True peak"].iter().enumerate() {
                    if ui.radio(detect == i, *name).on_hover_text(tips[i]).clicked() {
                        fx.set("detect", i as f64);
                    }
                }
            });
        });
        ui.add_space(6.0);
        let rows: [(&str, &[f64]); 4] = [
            ("ceiling", &[-60.0, -48.0, -36.0, -24.0, -12.0, -6.0, 0.0]),
            ("input_boost", &[-24.0, -12.0, 0.0, 12.0, 24.0]),
            ("lookahead", &[0.0, 5.0, 10.0, 15.0, 20.0]),
            ("release", &[10.0, 30.0, 100.0, 300.0, 1000.0]),
        ];
        for (key, marks) in rows {
            slider_row(ui, &th, fx, key, marks, LIM_W);
            ui.add_space(4.0);
        }
        ui.add_space(4.0);
        layout::centred(ui, LIM_W, |ui| {
            let mut on = fx.on("linked");
            if ui
                .checkbox(&mut on, "Link channels")
                .on_hover_text("Turn both sides down together, so the stereo picture never shifts")
                .changed()
            {
                fx.set("linked", if on { 1.0 } else { 0.0 });
            }
        });
        ui.add_space(GAP_S);
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_faces_name_real_controls() {
        let has = |id: &str, keys: &[&str]| {
            let info = af2_fx::info(id).unwrap();
            for k in keys {
                assert!(info.param(k).is_some(), "{id} {k}");
            }
        };
        has("af.amp_model", &["gate", "gate_db", "input", "bass", "middle", "treble", "output", "level", "cab"]);
        has("af.tube_preamp", &["input", "drive", "transformer", "output", "low", "high", "mix"]);
        has("af.opto_leveler", &["gain", "peak", "mode", "meter", "emphasis", "tube", "mix"]);
        has("af.limiter", &["detect", "ceiling", "input_boost", "lookahead", "release", "linked"]);
    }
}
