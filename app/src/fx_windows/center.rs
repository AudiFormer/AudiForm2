// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.
// Ported from Audiform (Audiform/src/panels/voice-chain/CenterPanel.svelte), written by its author (AudiFormer)
// for Audiform and relicensed here by them. See docs/clean-room.md.

//! The Center Channel Extractor's window, on a cyan faceplate. At the
//! top, the stereo stage seen from above: the listener at the bottom,
//! left to right across, and the place being looked for lit up (its
//! spread follows Amplitude discrimination; out-of-phase sound sits
//! behind the listener). Drag the light to pick a pan of your own (the
//! extractor turns to Custom, keeping the phase it had). Beside it, the
//! Center and Side level faders: Center all the way down takes the vocal
//! out, Side all the way down keeps only the vocal. Below, two tabs.
//! Extraction: what to look for (Center, Left, Right, Surround or Custom,
//! with its phase angle, pan and delay, live only for Custom) and the
//! frequency range (its centre and width live only for Custom).
//! Discrimination: how strict the match is. Advanced holds the FFT size.

use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Rounding, Sense, Shape, Stroke};

use super::layout::{self, GAP, GAP_S, KNOB};
use super::parts;
use super::space::{self, Look};
use super::{Env, Fx};
use crate::theme::{c, ca, lerp, Theme};
use crate::widgets;

/// Graphite with the extractor's cyan light (Audiform's colours).
const LOOK: Look = Look {
    face: [0x17, 0x20, 0x28],
    face2: [0x0e, 0x13, 0x18],
    ink: [0xd7, 0xf4, 0xf1],
    glow: [0x3e, 0xd6, 0xd0],
};

const EXTRACT: [&str; 5] = ["Center", "Left", "Right", "Surround", "Custom"];
const CUSTOM: usize = 4;
/// The named frequency ranges (Hz), as the effect has them.
const RANGE_HZ: [(f64, f64); 4] = [(20.0, 20_000.0), (20.0, 250.0), (90.0, 10_000.0), (160.0, 12_000.0)];

/// The face's width inside the plate's margin.
const FACE_W: f32 = 520.0;
/// One fader's column (fader and its readout).
const FADER_W: f32 = 84.0;
/// A value field's box.
const FIELD_W: f32 = 86.0;
/// The tab page's least height, so the window keeps its size across tabs.
const PAGE_H: f32 = 150.0;

// The stage, in its own units (scaled to fit, centred): an arc from left
// to right in front of the listener.
const SW: f32 = 300.0;
const SH: f32 = 150.0;
const CX: f32 = SW / 2.0;
const CY: f32 = SH - 18.0;
const RAD: f32 = 110.0;
/// How far round the arc hard left and right are (degrees from ahead).
const REACH: f32 = 70.0;

/// Pan -100..100 to an angle on the arc (degrees from straight ahead).
fn ang_of(pan: f32) -> f32 {
    pan / 100.0 * REACH
}

/// A point `r` from the listener at `deg` from straight ahead.
fn pt(deg: f32, r: f32) -> Pos2 {
    let a = (deg - 90.0).to_radians();
    pos2(CX + r * a.cos(), CY + r * a.sin())
}

/// The pan a point on the stage points to (whole steps, -100..100).
fn pan_at(p: Pos2) -> f64 {
    let deg = (p.x - CX).atan2(-(p.y - CY)).to_degrees();
    f64::from((deg / REACH * 100.0).clamp(-100.0, 100.0)).round()
}

/// The pan and phase being looked for.
fn aim(extract: usize, pan: f64, phase: f64) -> (f64, f64) {
    match extract {
        0 => (0.0, 0.0),
        1 => (-100.0, 0.0),
        2 => (100.0, 0.0),
        3 => (0.0, 180.0),
        _ => (pan, phase),
    }
}

/// The light's place: on the arc, or behind the listener for sound out
/// of phase.
fn spot_of(pan: f64, phase: f64) -> Pos2 {
    if phase.abs() > 90.0 {
        pos2(CX + pan as f32 / 100.0 * 60.0, CY + 6.0)
    } else {
        pt(ang_of(pan as f32), RAD)
    }
}

/// The frequency range taking part (Hz).
fn band(range: usize, center: f64, width: f64) -> (f64, f64) {
    RANGE_HZ.get(range).copied().unwrap_or_else(|| {
        let half = 2f64.powf(width / 2.0);
        (center / half, center * half)
    })
}

fn hz(f: f64) -> String {
    if f >= 1000.0 {
        format!("{:.*} kHz", if f >= 10_000.0 { 0 } else { 1 }, f / 1000.0)
    } else {
        format!("{} Hz", f.round())
    }
}

/// A level fader's readout: "off" at the floor.
fn level_text(v: f64, floor: f64) -> String {
    if v <= floor + 1e-9 {
        "off".into()
    } else {
        format!("{}{v:.1} dB", if v > 0.0 { "+" } else { "" })
    }
}

/// The line under the title: what it looks for and where.
fn sub_text(extract: usize, pan: f64, phase: f64, lo: f64, hi: f64) -> String {
    let what = match extract {
        3 => "out-of-phase sound".to_string(),
        CUSTOM => format!(
            "pan {}{}, {}°",
            if pan > 0.0 { "+" } else { "" },
            pan.round(),
            phase.round()
        ),
        k => format!("{} of the mix", EXTRACT.get(k).copied().unwrap_or("center").to_lowercase()),
    };
    format!("{what} · {} to {}", hz(lo), hz(hi))
}

fn index(fx: &Fx, key: &str, n: usize) -> usize {
    (fx.get(key).round().max(0.0) as usize).min(n - 1)
}

pub fn panel(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = space::tinted(&env.th, &LOOK);
    let w = FACE_W.max(layout::MIN_W);
    space::with_theme(ui, &th, |ui| {
        space::plate(ui, &th, w, |ui| {
            let extract = index(fx, "extract", EXTRACT.len());
            let range = index(fx, "range", RANGE_HZ.len() + 1);
            let (pan, phase) = aim(extract, fx.get("pan"), fx.get("phase"));
            let (lo, hi) = band(range, fx.get("center_freq"), fx.get("width"));

            // The title plate.
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("CENTER CHANNEL EXTRACTOR")
                        .size(14.0)
                        .strong()
                        .extra_letter_spacing(1.4)
                        .color(c(th.accent)),
                );
                ui.label(RichText::new(sub_text(extract, pan, phase, lo, hi)).size(11.0).color(ca(th.text, 150)));
            });
            rule(ui, &th, w, 77);
            ui.add_space(GAP_S);

            // The stage and the two faders beside it.
            let stage_w = w - 2.0 * FADER_W - GAP_S;
            let stage_h = (stage_w * SH / SW).round();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                stage(ui, fx, &th, vec2(stage_w, stage_h), extract, pan, phase);
                ui.add_space(GAP_S);
                for (key, name) in [("center", "Center"), ("side", "Side")] {
                    ui.allocate_ui_with_layout(
                        vec2(FADER_W, stage_h),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| fader(ui, fx, &th, key, name, stage_h),
                    );
                }
            });
            ui.add_space(GAP);

            // The tabs.
            let tab_id = fx.uid.with("cce-tab");
            let mut tab: usize = ui.ctx().data(|d| d.get_temp(tab_id)).unwrap_or(0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (k, name) in ["Extraction", "Discrimination"].into_iter().enumerate() {
                    if space::key(ui, &th, name, tab == k, true).clicked() {
                        tab = k;
                    }
                }
            });
            ui.ctx().data_mut(|d| d.insert_temp(tab_id, tab));
            ui.add_space(2.0);
            rule(ui, &th, w, 64);
            ui.add_space(GAP_S);

            let top = ui.cursor().top();
            if tab == 0 {
                extraction(ui, fx, &th, extract, range);
            } else {
                discrimination(ui, fx, &th, w);
            }
            let used = ui.cursor().top() - top;
            if used < PAGE_H {
                ui.add_space(PAGE_H - used);
            }

            // Advanced: the FFT size.
            egui::CollapsingHeader::new(RichText::new("Advanced").size(11.0).color(ca(th.text, 180)))
                .id_salt(fx.uid.with("cce-advanced"))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("FFT size").size(11.0).color(ca(th.text, 190)));
                        if let Some(ctl) = fx.ctl("fft").cloned() {
                            let cur = index(fx, "fft", ctl.choices.len().max(1));
                            let mut pick = cur;
                            egui::ComboBox::from_id_salt(fx.uid.with("cce-fft"))
                                .selected_text(ctl.choices.get(cur).cloned().unwrap_or_default())
                                .show_ui(ui, |ui| {
                                    for (k, n) in ctl.choices.iter().enumerate() {
                                        ui.selectable_value(&mut pick, k, n);
                                    }
                                });
                            if pick != cur {
                                fx.set("fft", pick as f64);
                            }
                        }
                    });
                    hint(ui, &th, w, "Larger sizes separate low frequencies better but smear quick sounds and add delay.");
                });
        });
    });
}

/// A thin lit line across the face.
fn rule(ui: &mut egui::Ui, th: &Theme, w: f32, alpha: u8) {
    let (r, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter()
        .line_segment([r.left_center(), r.right_center()], Stroke::new(1.0_f32, ca(th.accent, alpha)));
}

fn hint(ui: &mut egui::Ui, th: &Theme, w: f32, text: &str) {
    ui.set_max_width(w);
    ui.add(egui::Label::new(RichText::new(text).size(11.0).color(ca(th.text, 140))).wrap());
}

/// The stage seen from above, with the light to drag.
#[allow(clippy::too_many_arguments)]
fn stage(ui: &mut egui::Ui, fx: &mut Fx, th: &Theme, size: egui::Vec2, extract: usize, pan: f64, phase: f64) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let s = (rect.width() / SW).min(rect.height() / SH);
    let origin = rect.center() - vec2(SW, SH) * s / 2.0;
    let at = |p: Pos2| origin + p.to_vec2() * s;
    let p = ui.painter_at(rect);
    let square = th.bevel.is_some();
    let round = Rounding::same(if square { 0.0 } else { 4.0 });

    // The floor: dark, lit from where the listener sits.
    p.rect_filled(rect, round, lerp(th.wave_background, [0, 0, 0], 0.35));
    p.add(fan(
        pos2(rect.center().x, rect.bottom()),
        vec2(rect.width() * 0.9, rect.height() * 1.2),
        -180.0,
        0.0,
        ca(th.accent, 34),
        Color32::TRANSPARENT,
    ));
    // Rings and rays.
    let thin = Stroke::new(s, ca(th.accent, 31));
    for k in [0.35, 0.6, 0.85] {
        p.add(Shape::line(arc_pts(RAD * k, -REACH, REACH).into_iter().map(at).collect(), thin));
    }
    for a in [-70.0, -35.0, 0.0, 35.0, 70.0] {
        p.line_segment([at(pos2(CX, CY)), at(pt(a, RAD))], Stroke::new(s, ca(th.accent, 26)));
    }
    // The lit wedge toward the spot (not for sound behind the listener).
    let behind = phase.abs() > 90.0;
    if !behind {
        let spread = 6.0 + (fx.get("amp_disc") as f32 / 20.0) * 40.0;
        let a = ang_of(pan as f32);
        let mut pts = vec![at(pos2(CX, CY))];
        pts.extend(arc_pts(RAD + 16.0, a - spread / 2.0, a + spread / 2.0).into_iter().map(at));
        let (top, bottom) = pts.iter().fold((f32::MAX, f32::MIN), |(t, b), q| (t.min(q.y), b.max(q.y)));
        let shade = |y: f32| {
            let t = if bottom > top { (bottom - y) / (bottom - top) } else { 1.0 };
            ca(th.accent, (5.0 + t * 77.0) as u8)
        };
        let mut mesh = Mesh::default();
        for q in &pts {
            mesh.vertices.push(Vertex { pos: *q, uv: WHITE_UV, color: shade(q.y) });
        }
        for i in 1..pts.len() as u32 - 1 {
            mesh.add_triangle(0, i, i + 1);
        }
        p.add(Shape::mesh(mesh));
    }
    // The arc, its ends' names and the listener.
    p.add(Shape::line(
        arc_pts(RAD, -REACH, REACH).into_iter().map(at).collect(),
        Stroke::new(1.5 * s, ca(th.accent, 140)),
    ));
    let font = FontId::proportional(10.0 * s);
    let lab = ca(th.text, 180);
    p.text(at(pt(-REACH, RAD + 10.0) - vec2(4.0, 0.0)), Align2::RIGHT_CENTER, "L", font.clone(), lab);
    p.text(at(pt(REACH, RAD + 10.0) + vec2(4.0, 0.0)), Align2::LEFT_CENTER, "R", font.clone(), lab);
    p.text(at(pos2(CX + 30.0, CY - RAD + 4.0)), Align2::CENTER_BOTTOM, "C", font, lab);
    let head = at(pos2(CX, CY));
    p.circle_filled(head, 7.0 * s, lerp(th.background, th.accent, 0.15));
    p.circle_stroke(head, 7.0 * s, Stroke::new(s, ca(th.text, 153)));

    // The light: drag it to another pan.
    let spot = at(spot_of(pan, phase));
    let hit = Rect::from_center_size(spot, vec2(44.0, 44.0) * s);
    let resp = ui
        .interact(hit, fx.uid.with("cce-spot"), Sense::click_and_drag())
        .on_hover_text("Drag to look at another pan");
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(if resp.dragged() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        });
    }
    let core = lerp(th.accent, [255, 255, 255], 0.45);
    p.add(glow(spot, 22.0 * s, core, c(th.accent)));
    p.circle_filled(spot, 5.0 * s, lerp(th.text, [255, 255, 255], 0.6));
    p.circle_stroke(spot, 5.0 * s, Stroke::new(s, lerp(th.background, [0, 0, 0], 0.4)));
    if square {
        crate::skin::bevel(&p, rect, th, true);
    } else {
        p.rect_stroke(rect.shrink(0.5), round, Stroke::new(1.0_f32, ca(th.accent, 46)));
    }

    if resp.is_pointer_button_down_on() {
        if let Some(pp) = ui.input(|i| i.pointer.interact_pos()) {
            let here = ((pp - origin) / s).to_pos2();
            fx.set_live("pan", pan_at(here));
            if extract != CUSTOM {
                // Dragging away from a named place makes it Custom, at
                // the phase it had: one step with the pan.
                fx.set_live("extract", CUSTOM as f64);
                fx.set_live("phase", phase);
            }
        }
    }
    if resp.drag_stopped() || resp.clicked() {
        fx.released = true;
    }
}

/// Points along an arc `r` from the listener, from `a0` to `a1` degrees.
fn arc_pts(r: f32, a0: f32, a1: f32) -> Vec<Pos2> {
    let n = (((a1 - a0).abs() / 3.0).ceil() as usize).max(2);
    (0..=n).map(|i| pt(a0 + (a1 - a0) * i as f32 / n as f32, r)).collect()
}

/// A filled fan (part of an ellipse) shaded from `inner` at its centre to
/// `outer` at its rim.
fn fan(centre: Pos2, radii: egui::Vec2, a0: f32, a1: f32, inner: Color32, outer: Color32) -> Shape {
    let mut mesh = Mesh::default();
    mesh.vertices.push(Vertex { pos: centre, uv: WHITE_UV, color: inner });
    let n = 48u32;
    for i in 0..=n {
        let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
        let pos = centre + vec2(radii.x * a.cos(), radii.y * a.sin());
        mesh.vertices.push(Vertex { pos, uv: WHITE_UV, color: outer });
    }
    for i in 1..=n {
        mesh.add_triangle(0, i, i + 1);
    }
    Shape::mesh(mesh)
}

/// The light's soft glow: bright in the middle, gone at `r`.
fn glow(centre: Pos2, r: f32, core: Color32, light: Color32) -> Shape {
    let mut mesh = Mesh::default();
    let n = 40u32;
    mesh.vertices.push(Vertex { pos: centre, uv: WHITE_UV, color: core.gamma_multiply(0.95) });
    for (ring, (k, col)) in [(0.6, light.gamma_multiply(0.35)), (1.0, Color32::TRANSPARENT)]
        .into_iter()
        .enumerate()
    {
        for i in 0..n {
            let a = std::f32::consts::TAU * i as f32 / n as f32;
            mesh.vertices.push(Vertex {
                pos: centre + vec2(a.cos(), a.sin()) * r * k,
                uv: WHITE_UV,
                color: col,
            });
        }
        let base = 1 + ring as u32 * n;
        for i in 0..n {
            let j = (i + 1) % n;
            if ring == 0 {
                mesh.add_triangle(0, base + i, base + j);
            } else {
                let prev = base - n;
                mesh.add_triangle(prev + i, base + i, base + j);
                mesh.add_triangle(prev + i, base + j, prev + j);
            }
        }
    }
    Shape::mesh(mesh)
}

/// A level fader with its readout under it.
fn fader(ui: &mut egui::Ui, fx: &mut Fx, th: &Theme, key: &str, name: &str, h: f32) {
    let Some(ctl) = fx.ctl(key).cloned() else { return };
    let mut v = fx.get(key);
    let r = parts::vfader(ui, th, &mut v, ctl.min, ctl.max, ctl.default, vec2(24.0, h - 18.0), c(th.accent))
        .on_hover_text(format!("{}: drag (Shift: fine), wheel, double-click for 0 dB", ctl.name));
    if r.changed() {
        fx.set_live(key, v);
    }
    fx.track(&r);
    let text = level_text(fx.get(key), ctl.min);
    let col = if text == "off" { c(th.meter_high) } else { c(th.accent) };
    ui.label(RichText::new(format!("{name}: {text}")).size(10.0).color(col));
}

/// A setting's name and its value box (dimmed and still when `enabled`
/// is false).
fn field(ui: &mut egui::Ui, fx: &mut Fx, th: &Theme, key: &str, label: &str, enabled: bool) {
    let Some(ctl) = fx.ctl(key).cloned() else { return };
    ui.add_enabled_ui(enabled, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(label).size(11.0).color(ca(th.text, 190)));
            let mut v = fx.get(key);
            let unit = ctl.unit.clone();
            let fmt = move |v: f64| {
                if unit == "ms" {
                    format!("{v:.2} ms")
                } else {
                    widgets::fmt_unit(v, &unit)
                }
            };
            let r = widgets::value_box(ui, fx.uid.with(("cce-field", key)), &mut v, ctl.min, ctl.max, ctl.log, FIELD_W, &fmt);
            if r.changed() {
                fx.set_live(key, v);
            }
            fx.track(&r);
        });
    });
}

fn line_name(ui: &mut egui::Ui, th: &Theme, name: &str) {
    ui.label(RichText::new(name).size(11.0).color(ca(th.text, 180)));
}

fn extraction(ui: &mut egui::Ui, fx: &mut Fx, th: &Theme, extract: usize, range: usize) {
    let custom = extract == CUSTOM;
    line_name(ui, th, "Extract");
    fx.choice(
        ui,
        "extract",
        &[
            "The middle of the mix: usually the lead vocal",
            "Hard left",
            "Hard right",
            "Out-of-phase sound, as from a surround mix",
            "A pan, phase angle and delay of your own",
        ],
    );
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        field(ui, fx, th, "phase", "Phase angle", custom);
        field(ui, fx, th, "pan", "Pan", custom);
        field(ui, fx, th, "delay", "Delay", custom);
    });
    ui.add_space(GAP_S);
    line_name(ui, th, "Frequency range");
    fx.choice(ui, "range", &[]);
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        field(ui, fx, th, "center_freq", "Center frequency", range == RANGE_HZ.len());
        field(ui, fx, th, "width", "Width", range == RANGE_HZ.len());
    });
}

fn discrimination(ui: &mut egui::Ui, fx: &mut Fx, th: &Theme, w: f32) {
    const KNOBS: [(&str, &str); 5] = [
        ("bleed", "Crossover bleed"),
        ("phase_disc", "Phase"),
        ("amp_disc", "Amplitude"),
        ("amp_band", "Amp. bandwidth"),
        ("decay", "Spectral decay"),
    ];
    layout::cells(ui, w, KNOBS.len(), |ui, i| {
        let (key, name) = KNOBS[i];
        fx.knob(ui, key, KNOB + 8.0, name, &[]);
    });
    ui.add_space(GAP_S);
    hint(
        ui,
        th,
        w,
        "Lower Phase and Amplitude find only sound right at the spot; higher catch more around it. \
         Crossover bleed softens the edge. Spectral decay smooths a fluttery, watery sound.",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_light_and_the_pan_round_trip() {
        for pan in [-100.0, -63.0, -20.0, 0.0, 1.0, 45.0, 100.0] {
            assert_eq!(pan_at(spot_of(pan, 0.0)), pan, "{pan}");
        }
        assert_eq!(spot_of(0.0, 0.0), pos2(CX, CY - RAD));
        // Out of phase: behind the listener.
        assert!(spot_of(0.0, 180.0).y > CY);
        // Past the ends of the arc stays at the ends.
        assert_eq!(pan_at(pt(85.0, 50.0)), 100.0);
        assert_eq!(pan_at(pt(-85.0, 50.0)), -100.0);
    }

    #[test]
    fn named_places_and_ranges_follow_the_effect() {
        assert_eq!(aim(1, 30.0, 40.0), (-100.0, 0.0));
        assert_eq!(aim(3, 30.0, 40.0), (0.0, 180.0));
        assert_eq!(aim(CUSTOM, 30.0, 40.0), (30.0, 40.0));
        let (lo, hi) = band(4, 1000.0, 2.0);
        assert!((lo - 500.0).abs() < 1e-9 && (hi - 2000.0).abs() < 1e-9);
        assert_eq!(hz(12_000.0), "12 kHz");
        assert_eq!(hz(1_500.0), "1.5 kHz");
        assert_eq!(level_text(-48.0, -48.0), "off");
        assert_eq!(level_text(3.0, -48.0), "+3.0 dB");
        assert_eq!(sub_text(0, 0.0, 0.0, 20.0, 20_000.0), "center of the mix · 20 Hz to 20 kHz");
    }

    #[test]
    fn every_key_used_is_the_effects() {
        let info = af2_fx::effects::center::center_extractor_info();
        for k in [
            "extract", "phase", "pan", "delay", "range", "center_freq", "width", "bleed", "phase_disc",
            "amp_disc", "amp_band", "decay", "center", "side", "fft",
        ] {
            assert!(info.params.iter().any(|p| p.key == k), "{k}");
        }
        let names = |k: &str| info.params.iter().find(|p| p.key == k).map(|p| p.choices.len());
        assert_eq!(names("extract"), Some(EXTRACT.len()));
        assert_eq!(names("range"), Some(RANGE_HZ.len() + 1));
    }
}
