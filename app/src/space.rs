// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The 3D Space panel: the room seen from above, front at the top.
//!
//! Each placed source (a track or bus that reaches the master or a 3D
//! Bus) is a dot; a stereo source is a pair (L and R) about a diamond at
//! its spot. Drag the diamond (or a mono dot) to move it, drag L or R to
//! set the pair's width (Alt also turns it); the wheel over a dot raises
//! or lowers it, Shift+wheel turns a pair. Snapping pulls a spot onto a
//! speaker or the quarter grid (Alt while dragging skips it). The picked
//! source is drawn last, so one of several in one place can be grabbed,
//! and its settings sit beside the room. Automated spots follow the
//! playhead. Speaker meters run along the top.

use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Rounding, Sense, Stroke};

use crate::state::State;
use crate::theme::{c, ca};
use af27_core::space::{Monitor, SpaceLayout, SpacePos, SpatializeMode};
use af27_core::{Session, Track, TrackKind};

/// What a drag in the room holds.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Grab {
    Spot,
    Left,
    Right,
}

/// Whether a source is placed in the room: it reaches the master, or
/// outputs or sends to a 3D Bus. Tracks with nothing on them and 3D
/// Buses themselves are not.
pub fn is_placed(s: &Session, t: &Track) -> bool {
    if t.kind == TrackKind::Bus && t.spatial.room {
        return false;
    }
    if t.kind != TrackKind::Bus && t.clips.is_empty() && t.midi.is_empty() {
        return false;
    }
    let room = |id: af27_core::TrackId| {
        s.tracks
            .iter()
            .any(|b| b.id == id && b.kind == TrackKind::Bus && b.spatial.room)
    };
    let to_bus = t.output.filter(|&b| {
        s.tracks
            .iter()
            .any(|x| x.id == b && x.kind == TrackKind::Bus)
    });
    to_bus.is_none_or(room) || t.sends.iter().any(|x| room(x.bus))
}

/// Whether a source is placed as a stereo pair (else one point).
pub fn is_stereo(st: &State, t: &Track) -> bool {
    t.spatial.stereo.unwrap_or_else(|| {
        t.kind != TrackKind::Audio
            || t.clips.iter().any(|c| {
                !c.props.fold_channels
                    && st
                        .media_buffer(c.media)
                        .is_some_and(|b| b.num_channels() == 2)
            })
            || t.clips.iter().any(|c| {
                c.props.fold_channels
                    && st
                        .media_buffer(c.media)
                        .is_some_and(|b| b.num_channels() > 2)
            })
    })
}

fn monitor_label(m: Monitor) -> &'static str {
    match m {
        Monitor::StereoFold => "Stereo speakers",
        Monitor::Binaural => "Headphones",
        Monitor::Speakers => "Speakers, each on its own output",
    }
}

fn monitor_tip(m: Monitor) -> &'static str {
    match m {
        Monitor::StereoFold => "Each speaker folded left or right by where it stands",
        Monitor::Binaural => "Each speaker heard around your head (binaural), for headphones",
        Monitor::Speakers => "Each speaker to its own output of the audio device, in layout order (L on 1, R on 2, C on 3...). Needs a device with that many outputs; otherwise folded to stereo",
    }
}

/// The last layout used, so turning 3D Space back on restores it.
fn last_layout(ui: &egui::Ui) -> SpaceLayout {
    ui.data(|d| d.get_temp(egui::Id::new("space_last_layout")))
        .unwrap_or(SpaceLayout::Surround714)
}

fn set_last_layout(ui: &egui::Ui, l: SpaceLayout) {
    ui.data_mut(|d| d.insert_temp(egui::Id::new("space_last_layout"), l));
}

pub fn panel(ui: &mut egui::Ui, st: &mut State, dragging: &mut Option<usize>) {
    let th = st.theme.clone();
    let before = st.session.clone();
    let snap_id = egui::Id::new("space_snap");
    let mut snap: bool = ui.data_mut(|d| *d.get_persisted_mut_or(snap_id, true));
    ui.horizontal_wrapped(|ui| {
        let mut on = st.session.space.is_some();
        if ui
            .checkbox(&mut on, "3D Space")
            .on_hover_text("Place tracks in a room instead of panning them")
            .changed()
        {
            if let Some(l) = st.session.space {
                set_last_layout(ui, l);
            }
            st.session.space = on.then(|| last_layout(ui));
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
                            set_last_layout(ui, l);
                            st.commit_session("Speaker layout", before.clone());
                        }
                    }
                })
                .response
                .on_hover_text("Speaker layout");
            let m = st.session.monitor;
            egui::ComboBox::from_id_salt("monitor")
                .selected_text(monitor_label(m))
                .show_ui(ui, |ui| {
                    for v in [Monitor::StereoFold, Monitor::Binaural, Monitor::Speakers] {
                        if ui
                            .selectable_label(m == v, monitor_label(v))
                            .on_hover_text(monitor_tip(v))
                            .clicked()
                        {
                            st.session.monitor = v;
                            st.commit_session("Listen", before.clone());
                        }
                    }
                })
                .response
                .on_hover_text("How the room is heard while you work");
            if ui
                .checkbox(&mut snap, "Snap")
                .on_hover_text(
                    "Spots snap to speakers and the quarter grid (hold Alt while dragging to skip)",
                )
                .changed()
            {
                ui.data_mut(|d| d.insert_persisted(snap_id, snap));
            }
            if ui
                .button("Export Speaker Bed...")
                .on_hover_text("Write every speaker channel to a multichannel WAV")
                .clicked()
            {
                crate::export::open_bed(st);
            }
        }
    });
    let Some(layout) = st.session.space else {
        ui.add_space(6.0);
        ui.label(RichText::new("3D Space places each track in a room instead of panning it: from 5.1 up to 7.1.4, with height. Tracks keep their pans, so one session holds a stereo mix and a surround mix. Listen on stereo speakers, on headphones (binaural), or on a speaker for every channel; export the speaker bed as a multichannel WAV.").color(c(th.text_dim)));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let mut l = last_layout(ui);
            egui::ComboBox::from_id_salt("layout_off")
                .selected_text(l.label())
                .width(70.0)
                .show_ui(ui, |ui| {
                    for x in SpaceLayout::ALL {
                        ui.selectable_value(&mut l, x, x.label());
                    }
                });
            set_last_layout(ui, l);
            if ui.button("Turn On 3D Space").clicked() {
                st.session.space = Some(l);
                st.commit_session("3D Space", before.clone());
            }
        });
        return;
    };
    meters(ui, st, layout);

    // Which sources show, the picked one last (drawn on top).
    let placed: Vec<usize> = (0..st.session.tracks.len())
        .filter(|&i| is_placed(&st.session, &st.session.tracks[i]))
        .collect();
    let picked = st
        .selected_track
        .and_then(|id| st.session.tracks.iter().position(|t| t.id == id))
        .filter(|i| placed.contains(i));

    let avail = ui.available_rect_before_wrap();
    let side_w = if avail.width() > 420.0 { 190.0 } else { 0.0 };
    let side = (avail.width() - side_w - 8.0)
        .min(avail.height() - 18.0)
        .max(80.0);
    ui.horizontal_top(|ui| {
        let (rect, resp) = ui.allocate_exact_size(vec2(side, side), Sense::click_and_drag());
        room_view(
            ui, st, layout, rect, &resp, &placed, picked, dragging, snap, &before,
        );
        if side_w > 0.0 {
            ui.vertical(|ui| {
                ui.set_width(side_w);
                egui::ScrollArea::vertical()
                    .id_salt("space_side")
                    .max_height(side)
                    .show(ui, |ui| {
                        source_buttons(ui, st, &placed, picked);
                        if let Some(ti) = picked {
                            ui.separator();
                            spot_controls(ui, st, ti);
                        }
                    });
            });
        }
    });
    ui.label(
        RichText::new("Drag a dot to move it; drag L or R to set the width (Alt also turns the pair). Wheel: height; Shift+wheel: turn.")
            .size(10.0)
            .color(c(th.text_dim)),
    );
}

/// One button per placed source; a click picks it.
fn source_buttons(ui: &mut egui::Ui, st: &mut State, placed: &[usize], picked: Option<usize>) {
    if placed.is_empty() {
        ui.label(RichText::new("No tracks with clips reach the master yet.").size(10.0));
        return;
    }
    for &i in placed {
        let t = &st.session.tracks[i];
        let text = RichText::new(&t.name).color(if picked == Some(i) {
            Color32::BLACK
        } else {
            c(t.color)
        });
        let b = egui::Button::new(text)
            .fill(if picked == Some(i) {
                c(t.color)
            } else {
                ca(t.color, 30)
            })
            .min_size(vec2(ui.available_width(), 0.0));
        if ui.add(b).clicked() {
            st.selected_track = Some(st.session.tracks[i].id);
        }
    }
}

/// Speaker meters, one narrow bar per channel in layout order.
fn meters(ui: &mut egui::Ui, st: &mut State, layout: SpaceLayout) {
    let th = st.theme.clone();
    let names = af27_space::pan::layout_info(layout).names;
    let id = egui::Id::new("space_meters");
    let fresh = st
        .engine
        .as_ref()
        .map(|e| e.take_bed_peaks())
        .unwrap_or_default();
    let mut shown: Vec<f32> = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    shown.resize(names.len(), 0.0);
    let dt = ui.input(|i| i.stable_dt).min(0.1);
    for (k, v) in shown.iter_mut().enumerate() {
        let now = fresh.get(k).copied().unwrap_or(0.0);
        // Fall about 20 dB a second.
        *v = now.max(*v * 10f32.powf(-dt));
    }
    let h = 26.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h + 11.0), Sense::hover());
    let p = ui.painter_at(rect);
    let n = names.len().max(1) as f32;
    let w = (rect.width() / n).min(34.0);
    for (k, name) in names.iter().enumerate() {
        let x = rect.left() + k as f32 * w;
        let bar = Rect::from_min_size(pos2(x + 2.0, rect.top()), vec2(w - 4.0, h));
        p.rect_filled(bar, Rounding::same(2.0), c(th.wave_background));
        let db = 20.0 * shown[k].max(1e-6).log10();
        let f = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        let col = if db > -1.0 {
            th.meter_high
        } else if db > -12.0 {
            th.meter_mid
        } else {
            th.meter_low
        };
        let fill = Rect::from_min_max(pos2(bar.left(), bar.bottom() - bar.height() * f), bar.max);
        p.rect_filled(fill, Rounding::same(2.0), c(col));
        p.text(
            pos2(bar.center().x, bar.bottom() + 1.0),
            Align2::CENTER_TOP,
            name,
            FontId::proportional(8.5),
            c(th.text_dim),
        );
    }
    ui.data_mut(|d| d.insert_temp(id, shown));
}

/// A spot as drawn: its automation at the playhead, if any.
fn shown_pos(st: &State, t: &Track) -> SpacePos {
    if t.spatial.lanes.is_empty() {
        t.pos
    } else {
        t.pos.at_time(&t.spatial.lanes, st.position())
    }
}

/// Where the pair's sides sit (x, y), as the mix has them.
fn sides(p: &SpacePos) -> [[f64; 2]; 2] {
    let s = af27_space::pan::sides(&p.tidy());
    [
        [s[0][0] as f64, s[0][1] as f64],
        [s[1][0] as f64, s[1][1] as f64],
    ]
}

/// Snaps a spot onto a speaker (within 0.09) or each axis onto the
/// quarter grid (within 0.06).
fn snap_spot(layout: SpaceLayout, x: f64, y: f64) -> (f64, f64) {
    let near = af27_space::speakers(layout)
        .iter()
        .filter_map(|s| s.at)
        .map(|a| (a[0] as f64, a[1] as f64))
        .find(|(sx, sy)| ((sx - x).powi(2) + (sy - y).powi(2)).sqrt() < 0.09);
    if let Some(s) = near {
        return s;
    }
    let grid = |v: f64| {
        let q = (v * 4.0).round() / 4.0;
        if (q - v).abs() < 0.06 {
            q
        } else {
            v
        }
    };
    (grid(x), grid(y))
}

fn snap_to(v: f64, step: f64, within: f64) -> f64 {
    let q = (v / step).round() * step;
    if (q - v).abs() < within {
        q
    } else {
        v
    }
}

#[allow(clippy::too_many_arguments)]
fn room_view(
    ui: &mut egui::Ui,
    st: &mut State,
    layout: SpaceLayout,
    rect: Rect,
    resp: &egui::Response,
    placed: &[usize],
    picked: Option<usize>,
    dragging: &mut Option<usize>,
    snap: bool,
    before: &Session,
) {
    let th = st.theme.clone();
    let room = rect.shrink(rect.width() * 0.1);
    let p = ui.painter_at(rect);
    p.rect_filled(room, Rounding::same(th.rounding), c(th.wave_background));
    p.rect_stroke(
        room,
        Rounding::same(th.rounding),
        Stroke::new(1.0_f32, c(th.border)),
    );
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
    let unit = room.width() / 2.0;
    // Distance rings and centre lines.
    for f in [1.0 / 3.0, 2.0 / 3.0, 1.0] {
        p.circle_stroke(
            room.center(),
            unit * f,
            Stroke::new(1.0_f32, ca(th.grid, 90)),
        );
    }
    p.line_segment(
        [
            pos2(room.center().x, room.top()),
            pos2(room.center().x, room.bottom()),
        ],
        Stroke::new(1.0_f32, c(th.grid)),
    );
    p.line_segment(
        [
            pos2(room.left(), room.center().y),
            pos2(room.right(), room.center().y),
        ],
        Stroke::new(1.0_f32, c(th.grid)),
    );
    p.text(
        pos2(room.center().x, room.top() + 9.0),
        Align2::CENTER_TOP,
        "FRONT",
        FontId::proportional(9.0),
        c(th.text_dim),
    );
    // The listener, facing front.
    let head = room.center();
    p.circle_stroke(head, 7.0, Stroke::new(1.5_f32, c(th.text_dim)));
    p.line_segment(
        [head + vec2(-3.0, -6.0), head + vec2(0.0, -11.0)],
        Stroke::new(1.5_f32, c(th.text_dim)),
    );
    p.line_segment(
        [head + vec2(3.0, -6.0), head + vec2(0.0, -11.0)],
        Stroke::new(1.5_f32, c(th.text_dim)),
    );
    // Speakers: ear level as rounded squares, raised as diamonds set in
    // from their corners, named outside the room.
    for s in af27_space::speakers(layout) {
        let Some([x, y, z]) = s.at else { continue };
        let q = to_px(x as f64, y as f64);
        let out = vec2(x, -y).normalized() * 12.0;
        if z > 0.5 {
            let q = q - vec2(x, -y) * unit * 0.18;
            let r = 5.0;
            p.add(egui::Shape::convex_polygon(
                vec![
                    q + vec2(0.0, -r),
                    q + vec2(r, 0.0),
                    q + vec2(0.0, r),
                    q + vec2(-r, 0.0),
                ],
                Color32::TRANSPARENT,
                Stroke::new(1.5_f32, c(th.accent_alt)),
            ));
            p.text(
                q + vec2(0.0, 7.0),
                Align2::CENTER_TOP,
                s.name,
                FontId::proportional(8.5),
                c(th.text_dim),
            );
        } else {
            p.rect_filled(
                Rect::from_center_size(q, vec2(10.0, 10.0)),
                Rounding::same(3.0),
                c(th.accent_alt),
            );
            p.text(
                q + out,
                Align2::CENTER_CENTER,
                s.name,
                FontId::proportional(9.0),
                c(th.text_dim),
            );
        }
    }

    // Sources, the picked one last.
    let mut order: Vec<usize> = placed
        .iter()
        .copied()
        .filter(|i| Some(*i) != picked)
        .collect();
    order.extend(picked);
    let hover = resp.hover_pos();
    let mut hit: Option<(usize, Grab)> = None;
    for &i in &order {
        let t = &st.session.tracks[i];
        let pos = shown_pos(st, t);
        let stereo = is_stereo(st, t);
        let sel = picked == Some(i);
        let q = to_px(pos.x, pos.y);
        let r = (unit * 0.055).clamp(5.0, 11.0)
            * (1.0 + pos.z.max(0.0) as f32 * 0.5)
            * (1.0 + pos.size as f32 * 1.2);
        if pos.size > 0.0 {
            p.circle_filled(q, r + pos.size as f32 * unit * 0.5, ca(t.color, 25));
        }
        let mut grabs = Vec::new();
        if stereo {
            let [l, rr] = sides(&pos);
            let (mut lq, mut rq) = (to_px(l[0], l[1]), to_px(rr[0], rr[1]));
            // A narrow pair still draws its dots apart enough to grab.
            let min = 2.2 * r;
            if lq.distance(rq) < min {
                let a = (pos.rotation as f32).to_radians();
                let d = vec2(a.cos(), a.sin()) * min / 2.0;
                lq = q - d;
                rq = q + d;
            }
            p.line_segment([lq, rq], Stroke::new(1.5_f32, ca(t.color, 160)));
            for (dq, name) in [(lq, "L"), (rq, "R")] {
                p.circle_filled(dq, r * 0.8, c(t.color));
                p.text(
                    dq,
                    Align2::CENTER_CENTER,
                    name,
                    FontId::proportional(9.0),
                    Color32::BLACK,
                );
            }
            grabs.push((lq, r * 0.8, Grab::Left));
            grabs.push((rq, r * 0.8, Grab::Right));
            let d = r * 0.7;
            p.add(egui::Shape::convex_polygon(
                vec![
                    q + vec2(0.0, -d),
                    q + vec2(d, 0.0),
                    q + vec2(0.0, d),
                    q + vec2(-d, 0.0),
                ],
                c(t.color),
                Stroke::new(if sel { 2.0_f32 } else { 1.0 }, c(th.background)),
            ));
            grabs.push((q, d + 2.0, Grab::Spot));
        } else {
            p.circle_filled(q, r, c(t.color));
            p.circle_stroke(
                q,
                r,
                Stroke::new(if sel { 2.0_f32 } else { 1.0 }, c(th.background)),
            );
            grabs.push((q, r, Grab::Spot));
        }
        if pos.z.abs() > 0.01 {
            // A ring marks a raised (or lowered) source.
            p.circle_stroke(
                q,
                r + 3.0,
                Stroke::new(1.0_f32, ca(t.color, if pos.z > 0.0 { 200 } else { 90 })),
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
        let mut tag = t.name.clone();
        if pos.z.abs() > 0.005 {
            // Height as a signed percentage (up from ear level).
            tag += &format!(" {:+.0}%", pos.z * 100.0);
        }
        if pos.lfe > 0.0 {
            tag += " LFE";
        }
        p.text(
            q + vec2(0.0, r + 4.0),
            Align2::CENTER_TOP,
            tag,
            FontId::proportional(9.5),
            c(th.text),
        );
        if let Some(h) = hover {
            for (gq, gr, g) in grabs {
                if h.distance(gq) <= gr + 3.0 {
                    hit = Some((i, g));
                }
            }
        }
    }

    let grab_id = egui::Id::new("space_grab");
    if resp.drag_started() {
        *dragging = hit.map(|h| h.0);
        ui.data_mut(|d| d.insert_temp(grab_id, hit.map(|h| h.1)));
        if let Some((i, _)) = hit {
            st.selected_track = Some(st.session.tracks[i].id);
            st.commit_live("Move in 3D", before.clone());
        }
    }
    let grab: Option<Grab> = ui.data(|d| d.get_temp(grab_id)).flatten();
    if let (Some(i), Some(q), Some(g)) = (*dragging, resp.interact_pointer_pos(), grab) {
        if resp.dragged() && i < st.session.tracks.len() {
            let alt = ui.input(|i| i.modifiers.alt);
            let (x, y) = from_px(q);
            let pos = &mut st.session.tracks[i].pos;
            match g {
                Grab::Spot => {
                    let (x, y) = if snap && !alt {
                        snap_spot(layout, x, y)
                    } else {
                        (x, y)
                    };
                    pos.x = x;
                    pos.y = y;
                }
                Grab::Left | Grab::Right => {
                    let (dx, dy) = (x - pos.x, y - pos.y);
                    let w = (dx * dx + dy * dy).sqrt().min(1.0);
                    pos.width = if snap { snap_to(w, 0.25, 0.04) } else { w };
                    if alt {
                        // Turn the pair toward the pointer.
                        let (dx, dy) = if g == Grab::Left {
                            (-dx, -dy)
                        } else {
                            (dx, dy)
                        };
                        let deg = (-dy).atan2(dx).to_degrees();
                        pos.rotation = snap_to(deg, 15.0, 4.0).clamp(-180.0, 180.0);
                    }
                }
            }
            st.live_space(i);
        }
    }
    if resp.drag_stopped() {
        *dragging = None;
        ui.data_mut(|d| d.remove::<Option<Grab>>(grab_id));
    }
    if resp.clicked() {
        if let Some((i, _)) = hit {
            st.selected_track = Some(st.session.tracks[i].id);
        }
    }
    if let Some((i, _)) = hit {
        let (dy, dx, shift) = ui.input(|i| {
            (
                i.raw_scroll_delta.y,
                i.raw_scroll_delta.x,
                i.modifiers.shift,
            )
        });
        let notch = if dy != 0.0 { dy } else { dx };
        if notch != 0.0 {
            let b = st.session.clone();
            let pos = &mut st.session.tracks[i].pos;
            if shift {
                pos.rotation = (pos.rotation + 5.0 * notch.signum() as f64).clamp(-180.0, 180.0);
                st.live_space(i);
                st.commit_live("Turn in 3D", b);
            } else {
                pos.z = (pos.z + 0.05 * notch.signum() as f64).clamp(-1.0, 1.0);
                st.live_space(i);
                st.commit_live("Height", b);
            }
        }
    }
}

/// Ends a live edit: one undo step per gesture (as the Inspector does).
fn live_commit(st: &mut State, ui: &egui::Ui, label: &str, before: &Session, changed: bool) {
    let down = ui.input(|i| i.pointer.any_down());
    if changed && !st.gesture {
        st.commit_live(label, before.clone());
        st.gesture = down;
    }
    if !down {
        st.gesture = false;
    }
}

/// The picked source's spot settings (also in the Inspector).
pub fn spot_controls(ui: &mut egui::Ui, st: &mut State, ti: usize) {
    use crate::widgets::Slider;
    let Some(t) = st.session.tracks.get(ti) else {
        return;
    };
    let before = st.session.clone();
    let is_bus = t.kind == TrackKind::Bus;
    if is_bus {
        let mut room = t.spatial.room;
        if ui
            .checkbox(&mut room, "3D Bus")
            .on_hover_text("What feeds this bus is placed in the room; its effects run on every speaker, and its speaker bed goes on to the master's. A plain stereo bus outside 3D Space.")
            .changed()
        {
            st.session.tracks[ti].spatial.room = room;
            st.commit_session("3D Bus", before.clone());
            return;
        }
        if room {
            ui.label(RichText::new("A 3D Bus is not placed itself: its inputs are.").size(10.0));
            return;
        }
    }
    let stereo = is_stereo(st, &st.session.tracks[ti]);
    let automated = !st.session.tracks[ti].spatial.lanes.is_empty();
    let mut ch = false;
    let mut rebuild = false;
    {
        let p = &mut st.session.tracks[ti].pos;
        let mut height = p.z * 100.0;
        if ui
            .add(
                Slider::new(&mut height, -100.0..=100.0)
                    .suffix("%")
                    .text("Height"),
            )
            .on_hover_text("Ear level 0, ceiling 100%, below ear level down to -100%")
            .changed()
        {
            p.z = height / 100.0;
            ch = true;
        }
        ch |= ui
            .add(Slider::new(&mut p.size, 0.0..=1.0).text("Size"))
            .on_hover_text("0 is a point; 1 fills the room")
            .changed();
        ch |= ui
            .add(Slider::new(&mut p.spread, 0.0..=1.0).text("Spread"))
            .on_hover_text("How much is spread through the room as a diffuse field")
            .changed();
        if stereo {
            ch |= ui
                .add(Slider::new(&mut p.width, 0.0..=1.0).text("Width"))
                .on_hover_text("How far each side sits from the spot")
                .changed();
            ch |= ui
                .add(
                    Slider::new(&mut p.rotation, -180.0..=180.0)
                        .suffix("°")
                        .text("Rotation"),
                )
                .on_hover_text(
                    "Turns the pair, clockwise seen from above (90 puts the left side in front)",
                )
                .changed();
            ch |= ui
                .add(Slider::new(&mut p.spatialize, 0.0..=1.0).text("Spatialize"))
                .on_hover_text("How much of the pair opens out into the room")
                .changed();
            let mode = p.spatialize_mode;
            egui::ComboBox::from_label("Method")
                .selected_text(mode.label())
                .show_ui(ui, |ui| {
                    for m in SpatializeMode::ALL {
                        if ui
                            .selectable_label(mode == m, m.label())
                            .on_hover_text(m.tip())
                            .clicked()
                            && m != mode
                        {
                            p.spatialize_mode = m;
                            // Place Elements delays the whole room by a
                            // frame: the mix is rebuilt to switch it.
                            rebuild |= m == SpatializeMode::Steer || mode == SpatializeMode::Steer;
                            ch = true;
                        }
                    }
                });
        }
        // The LFE send in dB; the far left is off.
        let mut db = if p.lfe > 0.0 {
            (20.0 * p.lfe.log10()).max(-60.0)
        } else {
            -60.0
        };
        if ui
            .add(
                Slider::new(&mut db, -60.0..=0.0)
                    .suffix(" dB")
                    .text("LFE send"),
            )
            .on_hover_text(
                "Also feeds the low-frequency channel (low-passed at 120 Hz). Far left is off",
            )
            .changed()
        {
            p.lfe = if db <= -59.95 {
                0.0
            } else {
                10f64.powf(db / 20.0)
            };
            ch = true;
        }
        ch |= ui
            .add(Slider::new(&mut p.x, -1.0..=1.0).text("Left / right"))
            .changed();
        ch |= ui
            .add(Slider::new(&mut p.y, -1.0..=1.0).text("Back / front"))
            .changed();
        if ui
            .button("Back to Front Center")
            .on_hover_text("Front centre at ear level")
            .clicked()
        {
            let d = SpacePos::default();
            p.x = d.x;
            p.y = d.y;
            p.z = d.z;
            ch = true;
        }
    }
    if automated {
        ui.label(
            RichText::new("Some settings are automated: those follow their lanes in playback.")
                .size(10.0),
        );
    }
    if rebuild {
        st.commit_session("Spatialize method", before);
        return;
    }
    if ch {
        st.live_space(ti);
    }
    live_commit(st, ui, "3D position", &before, ch);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapping_pulls_to_speakers_then_the_grid() {
        let l = SpaceLayout::Surround51;
        assert_eq!(snap_spot(l, -0.95, 0.96), (-1.0, 1.0));
        assert_eq!(snap_spot(l, 0.27, 0.53), (0.25, 0.5));
        assert_eq!(snap_spot(l, 0.4, 0.1), (0.4, 0.1));
        assert_eq!(snap_to(47.0, 15.0, 4.0), 45.0);
        assert_eq!(snap_to(0.58, 0.25, 0.04), 0.58);
    }

    #[test]
    fn placed_sources_reach_the_master_or_a_3d_bus() {
        let mut s = Session::new("t", 48_000, 2);
        let a = s.tracks[0].id;
        let bus = s.add_track_of(TrackKind::Bus);
        // Tracks with nothing on them are not shown.
        assert!(!is_placed(&s, &s.tracks[0]));
        s.tracks[0].midi.push(af27_core::midi::MidiClip {
            id: 99,
            name: "m".into(),
            start: 0.0,
            length: 4.0,
            notes: Vec::new(),
            muted: false,
            gain_db: 0.0,
            controls: Vec::new(),
            ext_id: None,
        });
        assert!(is_placed(&s, &s.tracks[0]));
        s.track_mut(a).unwrap().output = Some(bus);
        assert!(!is_placed(&s, &s.tracks[0]), "to a stereo bus");
        assert!(
            is_placed(&s, s.tracks.last().unwrap()),
            "the bus reaches the master"
        );
        s.track_mut(bus).unwrap().spatial.room = true;
        assert!(is_placed(&s, &s.tracks[0]), "to a 3D Bus");
        assert!(
            !is_placed(&s, s.tracks.last().unwrap()),
            "3D Buses are not placed"
        );
    }
}
