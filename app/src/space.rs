// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The 3D Space panel: the room seen from above, front at the top. Each
//! track is a dot (a stereo track shows its two sides); drag a dot to
//! move the track, scroll over it to raise or lower it. Speakers of the
//! chosen layout are drawn on the walls, raised ones hollow.

use egui::{pos2, vec2, Align2, FontId, Pos2, Rect, RichText, Rounding, Sense, Stroke};

use crate::state::State;
use crate::theme::{c, ca};
use af27_core::space::{Monitor, SpaceLayout};

pub fn panel(ui: &mut egui::Ui, st: &mut State, dragging: &mut Option<usize>) {
    let th = st.theme.clone();
    let before = st.session.clone();
    ui.horizontal_wrapped(|ui| {
        let mut on = st.session.space.is_some();
        if ui
            .checkbox(&mut on, "3D Space session")
            .on_hover_text("Place tracks in a room instead of panning them")
            .changed()
        {
            st.session.space = on.then_some(SpaceLayout::Surround51);
            st.commit_session("3D Space", before.clone());
        }
        if let Some(layout) = st.session.space {
            egui::ComboBox::from_id_salt("layout")
                .selected_text(layout.label())
                .width(70.0)
                .show_ui(ui, |ui| {
                    for l in SpaceLayout::ALL {
                        if ui.selectable_label(layout == l, l.label()).clicked() {
                            st.session.space = Some(l);
                            st.commit_session("Speaker layout", before.clone());
                        }
                    }
                });
            let m = st.session.monitor;
            egui::ComboBox::from_id_salt("monitor")
                .selected_text(match m {
                    Monitor::Binaural => "Headphones (binaural)",
                    Monitor::StereoFold => "Stereo speakers",
                })
                .show_ui(ui, |ui| {
                    for (v, n) in [
                        (Monitor::Binaural, "Headphones (binaural)"),
                        (Monitor::StereoFold, "Stereo speakers"),
                    ] {
                        if ui.selectable_label(m == v, n).clicked() {
                            st.session.monitor = v;
                            st.commit_session("Monitoring", before.clone());
                        }
                    }
                });
        }
    });
    let Some(layout) = st.session.space else {
        ui.label(RichText::new("Turn on 3D Space to place each track in a room: 5.1 up to 7.1.4, heard binaurally on headphones or folded to stereo, exported as a multichannel WAV.").color(c(th.text_dim)));
        return;
    };
    let avail = ui.available_rect_before_wrap();
    let side = avail.width().min(avail.height()).max(80.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(avail.width(), side), Sense::click_and_drag());
    let room = Rect::from_center_size(rect.center(), vec2(side - 24.0, side - 24.0));
    let p = ui.painter_at(rect);
    p.rect_filled(room, Rounding::same(th.rounding), c(th.wave_background));
    p.rect_stroke(
        room,
        Rounding::same(th.rounding),
        Stroke::new(1.0_f32, c(th.border)),
    );
    for f in [0.25f32, 0.5, 0.75] {
        let x = room.left() + room.width() * f;
        let y = room.top() + room.height() * f;
        p.line_segment(
            [pos2(x, room.top()), pos2(x, room.bottom())],
            Stroke::new(1.0_f32, c(th.grid)),
        );
        p.line_segment(
            [pos2(room.left(), y), pos2(room.right(), y)],
            Stroke::new(1.0_f32, c(th.grid)),
        );
    }
    let to_px = |x: f64, y: f64| -> Pos2 {
        pos2(
            room.center().x + x as f32 * room.width() / 2.0,
            room.center().y - y as f32 * room.height() / 2.0,
        )
    };
    let from_px = |q: Pos2| -> (f64, f64) {
        (
            (((q.x - room.center().x) / (room.width() / 2.0)) as f64).clamp(-1.0, 1.0),
            (((room.center().y - q.y) / (room.height() / 2.0)) as f64).clamp(-1.0, 1.0),
        )
    };
    // Listener and speakers.
    p.circle_stroke(room.center(), 8.0, Stroke::new(1.5_f32, c(th.text_dim)));
    p.text(
        room.center() + vec2(0.0, -12.0),
        Align2::CENTER_BOTTOM,
        "front ▲",
        FontId::proportional(9.0),
        c(th.text_dim),
    );
    for s in af27_space::speakers(layout) {
        if let Some([x, y, z]) = s.at {
            let q = to_px(x as f64 * 0.98, y as f64 * 0.98)
                + if z > 0.5 {
                    vec2(x * -14.0, y * 14.0)
                } else {
                    vec2(0.0, 0.0)
                };
            if z > 0.5 {
                p.circle_stroke(q, 6.0, Stroke::new(1.5_f32, c(th.accent_alt)));
            } else {
                p.circle_filled(q, 6.0, c(th.accent_alt));
            }
            p.text(
                q + vec2(0.0, 9.0),
                Align2::CENTER_TOP,
                s.name,
                FontId::proportional(9.0),
                c(th.text_dim),
            );
        }
    }
    // Track dots.
    let mut hit = None;
    let hover = resp.hover_pos();
    for (i, t) in st.session.tracks.iter().enumerate() {
        let q = to_px(t.pos.x, t.pos.y);
        let r = 7.0 + t.pos.z as f32 * 6.0;
        let sel = st.selected_track == Some(t.id);
        if t.pos.size > 0.0 {
            p.circle_filled(
                q,
                r + t.pos.size as f32 * room.width() * 0.25,
                ca(t.color, 25),
            );
        }
        let stereo = t.kind != af27_core::TrackKind::Audio
            || t.clips.iter().any(|c| {
                st.media_buffer(c.media)
                    .is_some_and(|b| b.num_channels() >= 2)
            });
        if stereo {
            let a = (t.pos.rotation as f32).to_radians();
            let w = t.pos.width as f32;
            let (dx, dy) = (w * a.cos(), -w * a.sin());
            let lq = to_px(
                (t.pos.x - dx as f64).clamp(-1.0, 1.0),
                (t.pos.y - dy as f64).clamp(-1.0, 1.0),
            );
            let rq = to_px(
                (t.pos.x + dx as f64).clamp(-1.0, 1.0),
                (t.pos.y + dy as f64).clamp(-1.0, 1.0),
            );
            p.line_segment([lq, rq], Stroke::new(1.0_f32, ca(t.color, 140)));
            p.text(
                lq,
                Align2::CENTER_CENTER,
                "L",
                FontId::proportional(9.0),
                c(t.color),
            );
            p.text(
                rq,
                Align2::CENTER_CENTER,
                "R",
                FontId::proportional(9.0),
                c(t.color),
            );
        }
        if sel {
            th.glow_rect(
                &p,
                Rect::from_center_size(q, vec2(r * 2.0, r * 2.0)),
                t.color,
                1.0,
            );
        }
        p.circle_filled(q, r, c(t.color));
        p.circle_stroke(
            q,
            r,
            Stroke::new(if sel { 2.0_f32 } else { 1.0 }, c(th.background)),
        );
        p.text(
            q + vec2(r + 3.0, 0.0),
            Align2::LEFT_CENTER,
            &t.name,
            FontId::proportional(10.0),
            c(th.text),
        );
        if let Some(h) = hover {
            if h.distance(q) <= r + 3.0 {
                hit = Some(i);
            }
        }
    }
    if resp.drag_started() {
        *dragging = hit;
        if let Some(i) = hit {
            st.selected_track = Some(st.session.tracks[i].id);
            st.commit_live("Move in 3D", before.clone());
        }
    }
    if let (Some(i), Some(q)) = (*dragging, resp.interact_pointer_pos()) {
        if resp.dragged() {
            let (x, y) = from_px(q);
            st.session.tracks[i].pos.x = x;
            st.session.tracks[i].pos.y = y;
            st.live_space(i);
        }
    }
    if resp.drag_stopped() {
        *dragging = None;
    }
    if resp.clicked() {
        if let Some(i) = hit {
            st.selected_track = Some(st.session.tracks[i].id);
        }
    }
    if let Some(i) = hit {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let b = st.session.clone();
            let z = &mut st.session.tracks[i].pos.z;
            *z = (*z + scroll as f64 * 0.002).clamp(0.0, 1.0);
            st.live_space(i);
            st.commit_live("Height", b);
        }
    }
    ui.label(RichText::new("Drag a dot to move a track; scroll over it to raise or lower it. More settings in the Inspector.").size(10.0).color(c(th.text_dim)));
}
