// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The Multitrack's track headers and the strip above them.
//!
//! The strip switches what every header shows (Mix: level, pan, input and
//! output; Routing: input, output, sends and sidechain keys; Effects: each
//! track's rack) and turns on the metronome row, snapping and the stretch
//! handles. The metronome row sits under the ruler while it is on: it is
//! not a track, so track numbers and drops never shift. The Master row
//! closes the list.

use egui::{pos2, vec2, Align2, FontId, Rect, Response, RichText, Rounding, Sense, Stroke};
use std::ops::RangeInclusive;

use af27_core::midi::{ClickSound, Subdivide};
use af27_core::{Send, TrackId, TrackKind};
use af27_engine::MASTER;

use crate::state::State;
use crate::theme::{c, ca, lerp, Theme};

/// What the track headers show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum HeaderView {
    #[default]
    Mix,
    Routing,
    Effects,
}

/// Height of the metronome row.
pub const METRO_H: f32 = 62.0;
/// Height of the Master row at the end of the list.
pub const MASTER_H: f32 = 58.0;

// ------------------------------------------------------------ the dial

/// A small lit-arc dial with its value beside it. Drag up/down (or
/// sideways) to turn, Shift for fine steps, double-click for `default`.
pub fn dial(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: RangeInclusive<f32>,
    default: f32,
    fmt: impl Fn(f32) -> String,
    th: &Theme,
) -> Response {
    let (lo, hi) = (*range.start(), *range.end());
    let text = fmt(*value);
    let galley =
        ui.painter()
            .layout_no_wrap(text.clone(), FontId::proportional(11.0), c(th.accent));
    let d = 18.0;
    let w = d + 4.0 + galley.size().x.max(28.0);
    let (rect, mut resp) = ui.allocate_exact_size(vec2(w, 20.0), Sense::click_and_drag());
    let knob = Rect::from_center_size(pos2(rect.left() + d / 2.0, rect.center().y), vec2(d, d));
    if resp.double_clicked() {
        *value = default;
        resp.mark_changed();
    } else if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let dv = resp.drag_delta();
        let px = dv.x - dv.y;
        if px != 0.0 {
            let span = hi - lo;
            let step = span / if fine { 900.0 } else { 180.0 };
            let nv = (*value + px * step).clamp(lo, hi);
            if nv != *value {
                *value = nv;
                resp.mark_changed();
            }
        }
    }
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let p = ui.painter();
    let hot = resp.hovered() || resp.dragged();
    let ctr = knob.center();
    let rad = d * 0.42;
    let a0 = std::f32::consts::PI * 0.75;
    let sweep = std::f32::consts::PI * 1.5;
    let f = ((*value - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0);
    let fd = ((default - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0);
    let arc = |from: f32, to: f32| -> Vec<egui::Pos2> {
        let n = 24;
        (0..=n)
            .map(|k| {
                let a = a0 + sweep * (from + (to - from) * k as f32 / n as f32);
                ctr + vec2(a.cos(), a.sin()) * rad
            })
            .collect()
    };
    p.circle_filled(ctr, rad - 1.5, c(th.wave_background));
    p.add(egui::Shape::line(
        arc(0.0, 1.0),
        Stroke::new(2.0_f32, ca(th.border, 230)),
    ));
    // Lit from the default (unity, centre) to the value.
    let (a, b) = if f >= fd { (fd, f) } else { (f, fd) };
    if b - a > 1e-3 {
        p.add(egui::Shape::line(
            arc(a, b),
            Stroke::new(2.0_f32, c(th.accent)),
        ));
    }
    let ang = a0 + sweep * f;
    p.line_segment(
        [ctr, ctr + vec2(ang.cos(), ang.sin()) * (rad - 1.0)],
        Stroke::new(
            1.6_f32,
            if hot {
                c(th.text)
            } else {
                lerp(th.text, th.text_dim, 0.4)
            },
        ),
    );
    if hot && th.glow > 0.0 {
        p.circle_stroke(
            ctr,
            rad + 2.0,
            Stroke::new(2.0_f32, ca(th.accent, (40.0 * th.glow) as u8)),
        );
    }
    p.galley(
        pos2(knob.right() + 4.0, rect.center().y - galley.size().y / 2.0),
        galley,
        c(th.accent),
    );
    resp
}

/// A little square toggle with a letter (M, S, R).
fn letter(ui: &mut egui::Ui, on: bool, label: &str, col: [u8; 3], tip: &str, th: &Theme) -> bool {
    let b = egui::Button::new(RichText::new(label).strong().size(10.5).color(if on {
        c(th.background)
    } else {
        c(th.text_dim)
    }))
    .fill(if on { c(col) } else { c(th.panel_raised) })
    .min_size(vec2(18.0, 17.0));
    ui.add(b).on_hover_text(tip).clicked()
}

// ------------------------------------------------------------ the strip

#[derive(Clone, Copy, PartialEq)]
enum Glyph {
    Mix,
    Routing,
    Effects,
    Metronome,
    Snap,
    Stretch,
}

fn glyph(p: &egui::Painter, g: Glyph, r: Rect, col: egui::Color32) {
    let s = Stroke::new(1.5_f32, col);
    let ctr = r.center();
    match g {
        Glyph::Mix => {
            // Three faders.
            for (k, h) in [(-5.0f32, 0.3f32), (0.0, -0.25), (5.0, 0.1)] {
                let x = ctr.x + k;
                p.line_segment([pos2(x, ctr.y - 6.0), pos2(x, ctr.y + 6.0)], s);
                p.rect_filled(
                    Rect::from_center_size(pos2(x, ctr.y + h * 10.0), vec2(4.0, 2.5)),
                    Rounding::ZERO,
                    col,
                );
            }
        }
        Glyph::Routing => {
            // Two arrows, one each way.
            p.line_segment(
                [
                    pos2(ctr.x - 7.0, ctr.y - 3.0),
                    pos2(ctr.x + 7.0, ctr.y - 3.0),
                ],
                s,
            );
            p.line_segment(
                [
                    pos2(ctr.x + 7.0, ctr.y - 3.0),
                    pos2(ctr.x + 4.0, ctr.y - 6.0),
                ],
                s,
            );
            p.line_segment(
                [
                    pos2(ctr.x - 7.0, ctr.y + 3.0),
                    pos2(ctr.x + 7.0, ctr.y + 3.0),
                ],
                s,
            );
            p.line_segment(
                [
                    pos2(ctr.x - 7.0, ctr.y + 3.0),
                    pos2(ctr.x - 4.0, ctr.y + 6.0),
                ],
                s,
            );
        }
        Glyph::Effects => {
            p.text(
                ctr,
                Align2::CENTER_CENTER,
                "fx",
                FontId::new(13.0, egui::FontFamily::Proportional),
                col,
            );
        }
        Glyph::Metronome => {
            let pts = vec![
                pos2(ctr.x - 5.5, ctr.y + 6.5),
                pos2(ctr.x - 2.0, ctr.y - 6.5),
                pos2(ctr.x + 2.0, ctr.y - 6.5),
                pos2(ctr.x + 5.5, ctr.y + 6.5),
            ];
            p.add(egui::Shape::closed_line(pts, s));
            p.line_segment(
                [pos2(ctr.x, ctr.y + 4.0), pos2(ctr.x + 5.0, ctr.y - 5.0)],
                s,
            );
        }
        Glyph::Snap => {
            // A magnet.
            let n = 12;
            let pts: Vec<egui::Pos2> = (0..=n)
                .map(|k| {
                    let a = std::f32::consts::PI * k as f32 / n as f32;
                    pos2(ctr.x + 5.0 * a.cos(), ctr.y + 1.0 + 5.0 * a.sin())
                })
                .collect();
            p.add(egui::Shape::line(pts, Stroke::new(3.0_f32, col)));
            p.line_segment(
                [
                    pos2(ctr.x - 5.0, ctr.y + 1.0),
                    pos2(ctr.x - 5.0, ctr.y - 6.0),
                ],
                Stroke::new(3.0_f32, col),
            );
            p.line_segment(
                [
                    pos2(ctr.x + 5.0, ctr.y + 1.0),
                    pos2(ctr.x + 5.0, ctr.y - 6.0),
                ],
                Stroke::new(3.0_f32, col),
            );
        }
        Glyph::Stretch => {
            // A clip corner with the stretch triangle.
            p.rect_stroke(
                Rect::from_center_size(ctr, vec2(14.0, 10.0)),
                Rounding::same(1.0),
                s,
            );
            p.add(egui::Shape::convex_polygon(
                vec![
                    pos2(ctr.x + 7.0, ctr.y - 5.0),
                    pos2(ctr.x + 1.0, ctr.y - 5.0),
                    pos2(ctr.x + 7.0, ctr.y + 1.0),
                ],
                col,
                Stroke::NONE,
            ));
        }
    }
}

fn strip_button(ui: &mut egui::Ui, g: Glyph, on: bool, tip: &str, th: &Theme) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 20.0), Sense::click());
    let resp = resp.on_hover_text(tip);
    let p = ui.painter_at(rect.expand(1.0));
    let hover = resp.hovered();
    if th.bevel.is_some() {
        p.rect_filled(
            rect,
            0.0,
            if on {
                lerp(th.panel_raised, th.accent, 0.25)
            } else {
                c(th.panel_raised)
            },
        );
        crate::skin::bevel(&p, rect, th, on || resp.is_pointer_button_down_on());
    } else if on || hover {
        p.rect_filled(
            rect,
            Rounding::same(3.0),
            if on {
                ca(th.accent, 46)
            } else {
                ca(th.text, 14)
            },
        );
        if on {
            p.rect_stroke(
                rect,
                Rounding::same(3.0),
                Stroke::new(1.0_f32, ca(th.accent, 160)),
            );
        }
    }
    glyph(
        &p,
        g,
        rect,
        if on {
            c(th.accent)
        } else if hover {
            c(th.text)
        } else {
            c(th.text_dim)
        },
    );
    resp
}

/// The strip above the track headers.
pub fn strip(ui: &mut egui::Ui, st: &mut State, rect: Rect) {
    let th = st.theme.clone();
    ui.allocate_new_ui(
        egui::UiBuilder::new().max_rect(rect.shrink2(vec2(4.0, 1.0))),
        |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (v, g, tip) in [
                    (
                        HeaderView::Mix,
                        Glyph::Mix,
                        "Mix: level, pan, input and output",
                    ),
                    (
                        HeaderView::Routing,
                        Glyph::Routing,
                        "Routing: inputs, outputs, sends and sidechains",
                    ),
                    (
                        HeaderView::Effects,
                        Glyph::Effects,
                        "Effects: every track's rack",
                    ),
                ] {
                    if strip_button(ui, g, st.prefs.header_view == v, tip, &th).clicked() {
                        st.prefs.header_view = v;
                        st.prefs.save();
                    }
                }
                ui.add_space(8.0);
                let m = st.session.tempo.metronome;
                if strip_button(
                    ui,
                    Glyph::Metronome,
                    m,
                    "Metronome (shows its row above the tracks)",
                    &th,
                )
                .clicked()
                {
                    st.session.tempo.metronome = !m;
                    st.live_metronome();
                }
                if strip_button(ui, Glyph::Snap, st.prefs.snap_ruler, "Snapping", &th).clicked() {
                    st.prefs.snap_ruler = !st.prefs.snap_ruler;
                    st.prefs.save();
                }
                if strip_button(
                    ui,
                    Glyph::Stretch,
                    st.prefs.stretch_handles,
                    "Stretch handles on clips",
                    &th,
                )
                .clicked()
                {
                    st.prefs.stretch_handles = !st.prefs.stretch_handles;
                    st.prefs.save();
                }
            });
        },
    );
}

// ------------------------------------------------------------ headers

/// One track's header, as the strip's view says.
pub fn header(ui: &mut egui::Ui, st: &mut State, ti: usize, rect: Rect) {
    let th = st.theme.clone();
    let before = st.session.clone();
    let mut live = false;
    let mut structural = false;
    let kind = st.session.tracks[ti].kind;
    let id = st.session.tracks[ti].id;
    let h = st.session.tracks[ti].height;
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 3.0);
        // Name and the letters, in every view.
        ui.horizontal(|ui| {
            let tr = &mut st.session.tracks[ti];
            if kind == TrackKind::Bus {
                ui.label(RichText::new("BUS").size(9.0).strong().color(c(tr.color)));
            }
            let r = ui.add(
                egui::TextEdit::singleline(&mut tr.name)
                    .desired_width(if kind == TrackKind::Bus { 70.0 } else { 92.0 })
                    .frame(false)
                    .font(egui::TextStyle::Body),
            );
            structural |= r.lost_focus() && before.tracks[ti].name != tr.name;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if kind != TrackKind::Bus
                    && letter(ui, tr.armed, "R", th.accent_alt, "Arm for recording", &th)
                {
                    tr.armed = !tr.armed;
                    live = true;
                }
                if letter(ui, tr.solo, "S", th.accent, "Solo", &th) {
                    tr.solo = !tr.solo;
                    live = true;
                }
                if letter(ui, tr.mute, "M", th.meter_mid, "Mute", &th) {
                    tr.mute = !tr.mute;
                    live = true;
                }
            });
        });
        match st.prefs.header_view {
            HeaderView::Mix => {
                ui.horizontal(|ui| {
                    let tr = &mut st.session.tracks[ti];
                    ui.label(RichText::new("Vol").size(9.5).color(c(th.text_dim)));
                    live |= dial(
                        ui,
                        &mut tr.volume_db,
                        -60.0..=12.0,
                        0.0,
                        |v| format!("{v:+.1}"),
                        &th,
                    )
                    .on_hover_text("Volume (dB). Double-click for 0 dB.")
                    .changed();
                    if st.session.space.is_none() {
                        ui.label(RichText::new("Pan").size(9.5).color(c(th.text_dim)));
                        let mut pan = tr.pan * 100.0;
                        if dial(
                            ui,
                            &mut pan,
                            -100.0..=100.0,
                            0.0,
                            |v| {
                                if v.abs() < 0.5 {
                                    "C".into()
                                } else if v < 0.0 {
                                    format!("L{:.0}", -v)
                                } else {
                                    format!("R{v:.0}")
                                }
                            },
                            &th,
                        )
                        .on_hover_text("Pan. Double-click for centre.")
                        .changed()
                        {
                            tr.pan = pan / 100.0;
                            live = true;
                        }
                    }
                });
                if h >= 60.0 && kind != TrackKind::Bus {
                    input_row(ui, st, id, &th);
                }
                if h >= 80.0 || (kind == TrackKind::Bus && h >= 60.0) {
                    structural |= output_row(ui, st, ti, &th);
                }
                if h >= 124.0 {
                    structural |= sends_rows(ui, st, ti, &th, 3);
                }
            }
            HeaderView::Routing => {
                if kind != TrackKind::Bus {
                    input_row(ui, st, id, &th);
                }
                structural |= output_row(ui, st, ti, &th);
                let room = ((h - 70.0) / 21.0).max(0.0) as usize;
                structural |= sends_rows(ui, st, ti, &th, room.max(1));
                structural |= key_rows(ui, st, ti, &th);
            }
            HeaderView::Effects => {
                rack_rows(ui, st, ti, h, &th);
            }
        }
    });
    if live {
        let down = ui.input(|i| i.pointer.any_down());
        if !st.gesture {
            st.commit_live("Track settings", before.clone());
            st.gesture = down;
        }
        st.live_track(ti);
    }
    if structural {
        st.commit_session("Routing", before);
    }
}

fn input_row(ui: &mut egui::Ui, st: &mut State, id: TrackId, th: &Theme) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("→").color(c(th.text_dim)))
            .on_hover_text("Input");
        crate::record_input::track_input(ui, &mut st.rec_inputs, id);
    });
}

/// Master or a bus that does not feed back. True when changed.
fn output_row(ui: &mut egui::Ui, st: &mut State, ti: usize, th: &Theme) -> bool {
    let me = st.session.tracks[ti].id;
    let cur = st.session.tracks[ti].output;
    let buses: Vec<(TrackId, String, bool)> = st
        .session
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Bus && t.id != me)
        .map(|t| (t.id, t.name.clone(), st.session.can_route(me, t.id)))
        .collect();
    let shown = cur
        .and_then(|o| buses.iter().find(|b| b.0 == o))
        .map_or("Master".to_string(), |b| b.1.clone());
    let mut pick = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("←").color(c(th.text_dim)))
            .on_hover_text("Output");
        egui::ComboBox::from_id_salt(("track-out", me))
            .selected_text(RichText::new(shown).size(11.0))
            .width(150.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(cur.is_none(), "Master").clicked() {
                    pick = Some(None);
                }
                for (id, name, ok) in &buses {
                    let r = ui
                        .add_enabled(*ok, egui::SelectableLabel::new(cur == Some(*id), name))
                        .on_disabled_hover_text("It would feed back into this track");
                    if r.clicked() {
                        pick = Some(Some(*id));
                    }
                }
            });
    });
    if let Some(p) = pick {
        st.session.tracks[ti].output = p;
        return p != cur;
    }
    false
}

/// Sends to buses, up to `room` rows, then "Add Send". True when changed.
fn sends_rows(ui: &mut egui::Ui, st: &mut State, ti: usize, th: &Theme, room: usize) -> bool {
    let me = st.session.tracks[ti].id;
    let mut changed = false;
    let mut remove = None;
    let names: Vec<(TrackId, String)> = st
        .session
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Bus)
        .map(|t| (t.id, t.name.clone()))
        .collect();
    let n = st.session.tracks[ti].sends.len();
    for (k, s) in st.session.tracks[ti].sends.iter_mut().enumerate() {
        if k >= room {
            ui.label(
                RichText::new(format!("+{} more sends", n - room))
                    .size(10.0)
                    .color(c(th.text_dim)),
            );
            break;
        }
        let name = names
            .iter()
            .find(|b| b.0 == s.bus)
            .map_or("(gone)".to_string(), |b| b.1.clone());
        ui.horizontal(|ui| {
            ui.label(RichText::new("Send").size(9.5).color(c(th.text_dim)));
            ui.add_sized(
                vec2(44.0, 18.0),
                egui::Label::new(RichText::new(name).size(10.5)).truncate(),
            );
            changed |= dial(
                ui,
                &mut s.level_db,
                -60.0..=12.0,
                0.0,
                |v| format!("{v:+.0}"),
                th,
            )
            .on_hover_text("Send level (dB)")
            .changed();
            let pre = egui::Button::new(RichText::new("P").size(10.0).color(if s.pre_fader {
                c(th.background)
            } else {
                c(th.text_dim)
            }))
            .fill(if s.pre_fader {
                c(th.accent)
            } else {
                c(th.panel_raised)
            })
            .min_size(vec2(16.0, 16.0));
            if ui
                .add(pre)
                .on_hover_text("Pre-fader: before the volume")
                .clicked()
            {
                s.pre_fader = !s.pre_fader;
                changed = true;
            }
            if ui.small_button("✕").on_hover_text("Remove send").clicked() {
                remove = Some(k);
            }
        });
    }
    if let Some(k) = remove {
        st.session.tracks[ti].sends.remove(k);
        changed = true;
    }
    let free: Vec<(TrackId, String)> = names
        .iter()
        .filter(|(id, _)| {
            *id != me
                && st.session.can_route(me, *id)
                && !st.session.tracks[ti].sends.iter().any(|s| s.bus == *id)
        })
        .cloned()
        .collect();
    if n < room + 1 {
        ui.horizontal(|ui| {
            ui.menu_button(
                RichText::new("+ Send").size(10.5).color(c(th.text_dim)),
                |ui| {
                    if free.is_empty() {
                        ui.label("Add a bus track first (right-click the tracks).");
                    }
                    for (id, name) in &free {
                        if ui.button(name).clicked() {
                            st.session.tracks[ti].sends.push(Send {
                                bus: *id,
                                level_db: 0.0,
                                pre_fader: false,
                                pan: 0.0,
                                enabled: true,
                                ext_id: None,
                            });
                            changed = true;
                            ui.close_menu();
                        }
                    }
                },
            );
        });
    }
    changed
}

/// Sidechain keys: this track's effects that listen to another, and the
/// effects elsewhere this track keys.
fn key_rows(ui: &mut egui::Ui, st: &mut State, ti: usize, th: &Theme) -> bool {
    let me = st.session.tracks[ti].id;
    let mut changed = false;
    let name_of = |st: &State, id: TrackId| {
        st.session
            .tracks
            .iter()
            .find(|t| t.id == id)
            .map_or("(gone)".into(), |t| t.name.clone())
    };
    // Listener side.
    let slots: Vec<(usize, String, Option<TrackId>)> = st.session.tracks[ti]
        .chain
        .iter()
        .enumerate()
        .filter(|(_, s)| crate::fx::takes_key(&s.effect))
        .map(|(i, s)| (i, crate::fx::name(&s.effect), s.sidechain))
        .collect();
    for (i, name, cur) in slots {
        let shown = cur.map_or("own input".into(), |id| name_of(st, id));
        let mut pick = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Key").size(9.5).color(c(th.text_dim)))
                .on_hover_text("Sidechain key");
            egui::ComboBox::from_id_salt(("key-in", me, i))
                .selected_text(RichText::new(format!("{name}: {shown}")).size(10.5))
                .width(160.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(cur.is_none(), "Own input").clicked() {
                        pick = Some(None);
                    }
                    let ids: Vec<TrackId> = st
                        .session
                        .tracks
                        .iter()
                        .map(|t| t.id)
                        .filter(|id| *id != me)
                        .collect();
                    for id in ids {
                        let ok = st.session.can_route(id, me);
                        if ui
                            .add_enabled(
                                ok,
                                egui::SelectableLabel::new(cur == Some(id), name_of(st, id)),
                            )
                            .clicked()
                        {
                            pick = Some(Some(id));
                        }
                    }
                });
        });
        if let Some(p) = pick {
            st.session.tracks[ti].chain[i].sidechain = p;
            changed = true;
        }
    }
    // Source side: what this track keys, and keying something new.
    let mut keyed: Vec<(usize, usize, String)> = Vec::new();
    let mut can_key: Vec<(usize, usize, String)> = Vec::new();
    for (oi, t) in st.session.tracks.iter().enumerate() {
        if t.id == me {
            continue;
        }
        for (si, s) in t.chain.iter().enumerate() {
            if !crate::fx::takes_key(&s.effect) {
                continue;
            }
            let label = format!("{} › {}", t.name, crate::fx::name(&s.effect));
            if s.sidechain == Some(me) {
                keyed.push((oi, si, label));
            } else if st.session.can_route(me, t.id) {
                can_key.push((oi, si, label));
            }
        }
    }
    let mut unkey = None;
    for (oi, si, label) in &keyed {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Sidechain: {label}"))
                    .size(10.5)
                    .color(c(th.accent_alt)),
            );
            if ui.small_button("✕").clicked() {
                unkey = Some((*oi, *si));
            }
        });
    }
    if let Some((oi, si)) = unkey {
        st.session.tracks[oi].chain[si].sidechain = None;
        changed = true;
    }
    if !can_key.is_empty() {
        ui.menu_button(
            RichText::new("Sidechain to…")
                .size(10.5)
                .color(c(th.text_dim)),
            |ui| {
                for (oi, si, label) in &can_key {
                    if ui.button(label).clicked() {
                        st.session.tracks[*oi].chain[*si].sidechain = Some(me);
                        changed = true;
                        ui.close_menu();
                    }
                }
            },
        );
    }
    changed
}

/// The track's rack in the header: the rack power, then the slots.
fn rack_rows(ui: &mut egui::Ui, st: &mut State, ti: usize, h: f32, th: &Theme) {
    let target = ti as u16;
    let before = st.session.clone();
    let n = st.session.tracks[ti].chain.len();
    let room = (((h - 30.0) / 19.0).floor() as usize).max(1);
    let mut toggle = None;
    let mut open = None;
    let mut add = None;
    ui.spacing_mut().item_spacing.y = 1.0;
    for i in 0..n.min(room) {
        let s = st.session.tracks[ti].chain[i].clone();
        ui.horizontal(|ui| {
            let (pr, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
            crate::rack::power_icon(ui.painter(), pr, !s.bypass, resp.hovered(), th);
            if resp.clicked() {
                toggle = Some(i);
            }
            let r = ui.add(
                egui::Label::new(
                    RichText::new(format!("{} {}", i + 1, crate::fx::name(&s.effect)))
                        .size(11.0)
                        .color(if s.bypass { c(th.text_dim) } else { c(th.text) }),
                )
                .sense(Sense::click())
                .truncate(),
            );
            if r.on_hover_text("Open").clicked() {
                open = Some(i);
            }
        });
    }
    if n > room {
        ui.label(
            RichText::new(format!("+{} more", n - room))
                .size(10.0)
                .color(c(th.text_dim)),
        );
    } else if n < af27_core::session::CHAIN_SLOTS {
        ui.menu_button(
            RichText::new("+ Add effect")
                .size(10.5)
                .color(c(th.text_dim)),
            |ui| {
                if let Some(id) = crate::rack::effect_picker(ui) {
                    add = Some(id);
                }
            },
        );
    }
    if let Some(i) = toggle {
        let s = &mut st.session.tracks[ti].chain[i];
        s.bypass = !s.bypass;
        st.live_slot(target, i);
        st.commit_live("Bypass", before.clone());
    }
    if let Some(i) = open {
        crate::rack::open_window(st, target, i);
    }
    if let Some(id) = add {
        st.session.tracks[ti].chain.push(crate::rack::new_slot(&id));
        let at = st.session.tracks[ti].chain.len() - 1;
        crate::rack::open_window(st, target, at);
        st.commit_session("Add effect", before);
    }
}

// ------------------------------------------------------------ master row

/// The Master row closing the track list.
pub fn master_header(ui: &mut egui::Ui, st: &mut State, rect: Rect) {
    let th = st.theme.clone();
    let before = st.session.clone();
    let mut changed = false;
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 3.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Master").strong().color(c(th.text)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let n = st.session.master_chain.len();
                if ui
                    .add(
                        egui::Button::new(RichText::new(format!("fx {n}")).size(10.5))
                            .min_size(vec2(30.0, 17.0)),
                    )
                    .on_hover_text("The master effects rack")
                    .clicked()
                {
                    st.rack_tab = crate::rack::RackTab::Master;
                    st.want_effects = true;
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Vol").size(9.5).color(c(th.text_dim)));
            changed |= dial(
                ui,
                &mut st.session.master_db,
                -60.0..=12.0,
                0.0,
                |v| format!("{v:+.1}"),
                &th,
            )
            .changed();
            ui.label(RichText::new("←").color(c(th.text_dim)));
            let out = st
                .engine
                .as_ref()
                .map_or("No output".to_string(), |e| e.device.clone());
            ui.add(
                egui::Label::new(RichText::new(out).size(10.5).color(c(th.text_dim))).truncate(),
            );
        });
    });
    if changed {
        st.live_master();
        let down = ui.input(|i| i.pointer.any_down());
        if !st.gesture {
            st.commit_live("Master level", before);
            st.gesture = down;
        }
    }
}

// ------------------------------------------------------------ metronome

/// The metronome row's header.
pub fn metronome_header(ui: &mut egui::Ui, st: &mut State, rect: Rect) {
    let th = st.theme.clone();
    let mut m = st.prefs.metronome;
    let mut changed = false;
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 2.0);
        ui.horizontal(|ui| {
            let (gr, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            glyph(ui.painter(), Glyph::Metronome, gr, c(th.accent));
            ui.label(RichText::new("Metronome").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.menu_button("…", |ui| metronome_menu(ui, st, &mut m, &mut changed));
                if letter(ui, m.muted, "M", th.meter_mid, "Mute the click", &th) {
                    m.muted = !m.muted;
                    changed = true;
                }
            });
        });
        ui.horizontal(|ui| {
            changed |= dial(
                ui,
                &mut m.level_db,
                -40.0..=6.0,
                -6.0,
                |v| format!("{v:+.0} dB"),
                &th,
            )
            .on_hover_text("Click level")
            .changed();
            egui::ComboBox::from_id_salt("metro-sound")
                .selected_text(RichText::new(sound_name(m.sound)).size(11.0))
                .width(70.0)
                .show_ui(ui, |ui| {
                    for s in [
                        ClickSound::Tick,
                        ClickSound::Wood,
                        ClickSound::Beep,
                        ClickSound::Shaker,
                    ] {
                        changed |= ui
                            .selectable_value(&mut m.sound, s, sound_name(s))
                            .changed();
                    }
                });
            // The tempo and time signature under the playhead.
            let seg = st.session.tempo.seg_at_secs(st.position().max(0.0));
            let mut bpm = seg.bpm;
            let r = ui.add(
                egui::DragValue::new(&mut bpm)
                    .range(af27_core::midi::BPM_MIN..=af27_core::midi::BPM_MAX)
                    .speed(0.5)
                    .suffix(" BPM")
                    .max_decimals(1),
            );
            if r.drag_started() {
                st.gesture_before = Some(st.session.clone());
            }
            if r.changed() {
                let before = st.session.clone();
                st.session
                    .tempo
                    .set_values(seg.change, bpm, seg.beats_per_bar, seg.beat_unit);
                st.session_version += 1;
                st.live_metronome();
                if !r.dragged() && st.gesture_before.is_none() {
                    st.commit_session("Tempo", before);
                }
            }
            if r.drag_stopped() {
                if let Some(b) = st.gesture_before.take() {
                    if b != st.session {
                        st.commit_session("Tempo", b);
                    }
                }
            }
            if ui
                .add(egui::Button::new(RichText::new(seg.meter_text()).size(11.0)).frame(false))
                .on_hover_text("Time signature (opens Tempo and Time Signature)")
                .clicked()
            {
                st.tempo_ui.open = true;
            }
        });
    });
    if changed {
        st.prefs.metronome = m;
        st.prefs.save();
        st.live_metronome();
    }
}

fn sound_name(s: ClickSound) -> &'static str {
    match s {
        ClickSound::Tick => "Tick",
        ClickSound::Wood => "Wood",
        ClickSound::Beep => "Beep",
        ClickSound::Shaker => "Shaker",
    }
}

/// The metronome's … menu (also its right-click menu).
pub fn metronome_menu(
    ui: &mut egui::Ui,
    st: &mut State,
    m: &mut af27_core::midi::MetronomeSettings,
    changed: &mut bool,
) {
    ui.label(RichText::new("Click").small());
    *changed |= ui
        .radio_value(&mut m.recording_only, false, "Playback and recording")
        .changed();
    *changed |= ui
        .radio_value(&mut m.recording_only, true, "Recording only")
        .changed();
    ui.separator();
    ui.label(RichText::new("Between beats").small());
    for (s, n) in [
        (Subdivide::None, "Beats only"),
        (Subdivide::Halves, "Halves"),
        (Subdivide::Triplets, "Triplets"),
        (Subdivide::Quarters, "Quarters"),
    ] {
        *changed |= ui.radio_value(&mut m.subdivide, s, n).changed();
    }
    ui.separator();
    *changed |= ui
        .checkbox(&mut m.accent, "Higher click on the first beat of each bar")
        .changed();
    ui.separator();
    if ui.button("Add Tempo Change at Nearest Bar").clicked() {
        let before = st.session.clone();
        let t = &st.session.tempo;
        let bar = t.nearest_bar(t.beats(st.position().max(0.0)));
        st.session.tempo.add_change(bar);
        st.session_version += 1;
        st.commit_session("Add tempo change", before);
        st.tempo_ui.open = true;
        ui.close_menu();
    }
    if ui.button("Turn Off Metronome").clicked() {
        st.session.tempo.metronome = false;
        st.live_metronome();
        ui.close_menu();
    }
}

/// The metronome row's lane: bar and beat lines (bars brighter).
pub fn metronome_lane(
    p: &egui::Painter,
    st: &State,
    row: Rect,
    to_x: &dyn Fn(f64) -> f32,
    to_t: &dyn Fn(f32) -> f64,
) {
    let th = &st.theme;
    let muted = st.prefs.metronome.muted;
    p.rect_filled(row, Rounding::ZERO, lerp(th.wave_background, th.panel, 0.5));
    let tempo = &st.session.tempo;
    let t0 = to_t(row.left()).max(0.0);
    let t1 = to_t(row.right());
    let mut b = tempo.bar_floor(tempo.beats(t0));
    let alpha = if muted { 70 } else { 200 };
    let mut guard = 0;
    loop {
        let t = tempo.secs(b);
        if t > t1 || guard > 4000 {
            break;
        }
        guard += 1;
        let x = to_x(t);
        let (_, into, seg) = tempo.bar_at(b);
        let bar = into < 1e-6;
        let (top, col, w) = if bar {
            (row.top() + 6.0, ca(th.accent, alpha), 2.0_f32)
        } else {
            (
                row.top() + row.height() * 0.45,
                ca(th.text_dim, alpha / 2 + 30),
                1.0,
            )
        };
        if x >= row.left() && x <= row.right() {
            p.line_segment(
                [pos2(x, top), pos2(x, row.bottom() - 6.0)],
                Stroke::new(w, col),
            );
        }
        b += seg.unit_beats();
    }
    p.line_segment(
        [row.left_bottom(), row.right_bottom()],
        Stroke::new(1.0_f32, c(th.border)),
    );
}

#[allow(dead_code)]
fn unused(_: u16) -> u16 {
    MASTER
}
