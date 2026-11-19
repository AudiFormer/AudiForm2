// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The Multitrack editor (tracks of clips on a timeline) and the Mixer.
//!
//! Move tool: drag clips in time and between tracks, drag a clip's ends
//! to trim it. Razor: click a clip to split it there. Time Selection:
//! drag to select a range. Drop files from the Files panel onto a track.
//! Wheel over the lanes zooms at the pointer; over the headers it
//! scrolls the tracks; Shift+wheel scrolls in time.

use egui::{pos2, vec2, Align2, Color32, FontId, Rect, RichText, Rounding, Sense, Stroke};

use crate::state::{DocId, State, Tool, View};
use crate::theme::{c, ca, lerp};
use crate::ui::Caches;
use crate::waveform::{draw_channel, draw_ruler, playhead, time_to_x, x_to_time};
use af27_core::{ClipId, Session};

const HEADER_W: f32 = 210.0;

#[derive(Debug, Clone, Copy)]
pub enum DragKind {
    Move { clip: ClipId, grab: f64 },
    TrimLeft { clip: ClipId },
    TrimRight { clip: ClipId },
    Select { anchor: f64 },
    MoveMidi { clip: ClipId, grab: f64 },
}

#[derive(Default)]
pub struct Drag {
    pub kind: Option<DragKind>,
    pub before: Option<Session>,
    pub scroll_y: f32,
}

fn track_at(st: &State, lanes_top: f32, scroll: f32, y: f32) -> Option<usize> {
    let mut top = lanes_top - scroll;
    for (i, t) in st.session.tracks.iter().enumerate() {
        if y >= top && y < top + t.height {
            return Some(i);
        }
        top += t.height;
    }
    None
}

pub fn editor(ui: &mut egui::Ui, st: &mut State, caches: &mut Caches) {
    let th = st.theme.clone();
    let full = ui.available_rect_before_wrap();
    let (rect, resp) = ui.allocate_exact_size(full.size(), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, Rounding::ZERO, c(th.wave_background));
    let ruler = Rect::from_min_size(
        pos2(rect.left() + HEADER_W, rect.top()),
        vec2(rect.width() - HEADER_W, 22.0),
    );
    let lanes = Rect::from_min_max(pos2(ruler.left(), ruler.bottom()), rect.max);
    let headers = Rect::from_min_max(
        pos2(rect.left(), ruler.bottom()),
        pos2(ruler.left(), rect.bottom()),
    );
    let total_h: f32 = st.session.tracks.iter().map(|t| t.height).sum();
    let drag = &mut caches.drag;
    drag.scroll_y = drag
        .scroll_y
        .clamp(0.0, (total_h - lanes.height() + 40.0).max(0.0));

    // Wheel.
    if let Some(hp) = resp.hover_pos() {
        let (scroll, shift, zoomd) =
            ui.input(|i| (i.smooth_scroll_delta, i.modifiers.shift, i.zoom_delta()));
        let dur = st.session.duration().max(30.0);
        if headers.contains(hp) {
            drag.scroll_y -= scroll.y;
        } else if zoomd != 1.0 {
            let t = x_to_time(&st.session_view, lanes, hp.x);
            st.session_view.zoom(1.0 / zoomd as f64, t, dur);
        } else if shift && scroll.y != 0.0 {
            let v = &mut st.session_view;
            v.start = (v.start - scroll.y as f64 / lanes.width() as f64 * v.len).max(0.0);
        } else if scroll.y != 0.0 {
            let t = x_to_time(&st.session_view, lanes, hp.x);
            st.session_view
                .zoom((-scroll.y as f64 * 0.004).exp(), t, dur);
        } else if scroll.x != 0.0 {
            let v = &mut st.session_view;
            v.start = (v.start - scroll.x as f64 / lanes.width() as f64 * v.len).max(0.0);
        }
    }
    let v = st.session_view;
    let scroll = drag.scroll_y;

    // Hit-test clips under a point: (track index, clip id, near left edge, near right edge).
    let hit = |st: &State, pp: egui::Pos2| -> Option<(usize, ClipId, bool, bool)> {
        let ti = track_at(st, lanes.top(), scroll, pp.y)?;
        let t = x_to_time(&v, lanes, pp.x);
        let tr = &st.session.tracks[ti];
        tr.clips
            .iter()
            .rev()
            .find(|c| t >= c.start && t <= c.end())
            .map(|c| {
                let x0 = time_to_x(&v, lanes, c.start);
                let x1 = time_to_x(&v, lanes, c.end());
                (ti, c.id, (pp.x - x0).abs() < 6.0, (pp.x - x1).abs() < 6.0)
            })
    };

    // Drop from the Files panel.
    if let Some(doc) = resp.dnd_release_payload::<DocId>() {
        if let Some(pp) = ui.input(|i| i.pointer.interact_pos()) {
            let t = x_to_time(&v, lanes, pp.x).max(0.0);
            let ti = track_at(st, lanes.top(), scroll, pp.y).unwrap_or_else(|| {
                st.add_track();
                st.session.tracks.len() - 1
            });
            let track = st.session.tracks[ti].id;
            st.place_doc(*doc, track, t);
        }
    }

    // MIDI clips: (track index, clip id) under a point.
    let tempo = st.session.tempo;
    let hit_midi = |st: &State, pp: egui::Pos2| -> Option<(usize, ClipId)> {
        let ti = track_at(st, lanes.top(), scroll, pp.y)?;
        let t = tempo.beats(x_to_time(&v, lanes, pp.x));
        st.session.tracks[ti]
            .midi
            .iter()
            .rev()
            .find(|c| t >= c.start && t <= c.end())
            .map(|c| (ti, c.id))
    };
    if resp.double_clicked() {
        if let Some(pp) = resp.interact_pointer_pos() {
            if let Some((_, id)) = hit_midi(st, pp) {
                st.piano_clip = Some(id);
                st.want_piano = true;
            } else if let Some(ti) = track_at(st, lanes.top(), scroll, pp.y) {
                if st.session.tracks[ti].kind == af27_core::TrackKind::Midi && lanes.contains(pp) {
                    let bpb = st.session.tempo.beats_per_bar.max(1) as f64;
                    let b = (tempo.beats(x_to_time(&v, lanes, pp.x)) / bpb).floor() * bpb;
                    let id = st.session.tracks[ti].id;
                    crate::pianoroll::new_clip(st, id, b);
                }
            }
        }
    }

    // Pointer.
    if let Some(pp) = resp.interact_pointer_pos() {
        if resp.drag_started() && lanes.contains(pp) && st.tool == Tool::Move {
            if let Some((ti, id)) = hit_midi(st, pp) {
                let (mti, mci) = st.session.find_midi(id).unwrap();
                let grab = tempo.beats(x_to_time(&v, lanes, pp.x))
                    - st.session.tracks[mti].midi[mci].start;
                st.selected_clip = Some(id);
                st.selected_track = Some(st.session.tracks[ti].id);
                drag.before = Some(st.session.clone());
                drag.kind = Some(DragKind::MoveMidi { clip: id, grab });
            }
        }
        if resp.drag_started() && lanes.contains(pp) && drag.kind.is_none() {
            let t = x_to_time(&v, lanes, pp.x).max(0.0);
            let kind = match (st.tool, hit(st, pp)) {
                (Tool::Move, Some((ti, id, l, r))) => {
                    st.selected_clip = Some(id);
                    st.selected_track = Some(st.session.tracks[ti].id);
                    let (cti, ci) = st.session.find_clip(id).unwrap();
                    let clip = &st.session.tracks[cti].clips[ci];
                    Some(if l {
                        DragKind::TrimLeft { clip: id }
                    } else if r {
                        DragKind::TrimRight { clip: id }
                    } else {
                        DragKind::Move {
                            clip: id,
                            grab: t - clip.start,
                        }
                    })
                }
                (Tool::Razor, _) => None,
                _ => Some(DragKind::Select { anchor: t }),
            };
            if kind.is_some() {
                drag.before = Some(st.session.clone());
            }
            drag.kind = kind;
        }
        if resp.dragged() {
            let t = x_to_time(&v, lanes, pp.x).max(0.0);
            match drag.kind {
                Some(DragKind::Move { clip, grab }) => {
                    let target = track_at(st, lanes.top(), scroll, pp.y);
                    if let Some((ti, ci)) = st.session.find_clip(clip) {
                        let mut new_start = (t - grab).max(0.0);
                        // Snap to other clip edges and the playhead within 8 px.
                        let snap_px = 8.0 * v.len / lanes.width() as f64;
                        let len = st.session.tracks[ti].clips[ci].length;
                        let mut edges: Vec<f64> = vec![st.session_cursor, 0.0];
                        for tr in &st.session.tracks {
                            for c in tr.clips.iter().filter(|c| c.id != clip) {
                                edges.push(c.start);
                                edges.push(c.end());
                            }
                        }
                        for e in edges {
                            if (new_start - e).abs() < snap_px {
                                new_start = e;
                            } else if (new_start + len - e).abs() < snap_px {
                                new_start = e - len;
                            }
                        }
                        st.session.tracks[ti].clips[ci].start = new_start.max(0.0);
                        if let Some(to) = target.filter(|&to| to != ti) {
                            let c = st.session.tracks[ti].clips.remove(ci);
                            st.session.tracks[to].clips.push(c);
                            st.selected_track = Some(st.session.tracks[to].id);
                        }
                        st.session_version += 1;
                    }
                }
                Some(DragKind::TrimLeft { clip }) => {
                    if let Some((ti, ci)) = st.session.find_clip(clip) {
                        let c = &mut st.session.tracks[ti].clips[ci];
                        let end = c.end();
                        let new_start = t.clamp(c.start - c.offset, end - 0.01);
                        let d = new_start - c.start;
                        c.start = new_start;
                        c.offset += d;
                        c.length -= d;
                        st.session_version += 1;
                    }
                }
                Some(DragKind::TrimRight { clip }) => {
                    if let Some((ti, ci)) = st.session.find_clip(clip) {
                        let media_len = {
                            let m = st.session.tracks[ti].clips[ci].media;
                            st.session.media(m).map_or(f64::MAX, |m| {
                                if m.duration > 0.0 {
                                    m.duration
                                } else {
                                    f64::MAX
                                }
                            })
                        };
                        let c = &mut st.session.tracks[ti].clips[ci];
                        c.length = (t - c.start).clamp(0.01, media_len - c.offset);
                        st.session_version += 1;
                    }
                }
                Some(DragKind::Select { anchor }) => {
                    st.session_sel = Some((anchor.min(t), anchor.max(t)));
                    st.session_cursor = anchor.min(t);
                }
                Some(DragKind::MoveMidi { clip, grab }) => {
                    if let Some((ti, ci)) = st.session.find_midi(clip) {
                        // Whole beats.
                        let b = (tempo.beats(t) - grab).round();
                        st.session.tracks[ti].midi[ci].start = b.max(0.0);
                        st.session_version += 1;
                    }
                }
                None => {}
            }
        }
    }
    if resp.drag_stopped() {
        if let (Some(kind), Some(before)) = (drag.kind.take(), drag.before.take()) {
            if before != st.session && !matches!(kind, DragKind::Select { .. }) {
                let label = match kind {
                    DragKind::Move { .. } => "Move clip",
                    DragKind::TrimLeft { .. } | DragKind::TrimRight { .. } => "Trim clip",
                    DragKind::Select { .. } => "Select",
                    DragKind::MoveMidi { .. } => "Move MIDI clip",
                };
                st.commit_session(label, before);
            }
        }
    }
    if resp.clicked() {
        if let Some(pp) = resp.interact_pointer_pos() {
            if lanes.contains(pp) || ruler.contains(pp) {
                let t = x_to_time(&v, lanes, pp.x).max(0.0);
                match (st.tool, hit(st, pp)) {
                    (Tool::Razor, Some((_, id, _, _))) => {
                        let before = st.session.clone();
                        if st.session.split_clip(id, t).is_some() {
                            st.commit_session("Split", before);
                        }
                    }
                    (_, Some((ti, id, _, _))) if lanes.contains(pp) => {
                        st.selected_clip = Some(id);
                        st.selected_track = Some(st.session.tracks[ti].id);
                        st.session_cursor = t;
                    }
                    _ => {
                        st.selected_clip = None;
                        st.session_sel = None;
                        st.session_cursor = t;
                        if let Some(ti) = track_at(st, lanes.top(), scroll, pp.y) {
                            st.selected_track = Some(st.session.tracks[ti].id);
                        }
                        if st.is_playing() {
                            st.play();
                        }
                    }
                }
            }
        }
    }
    if let Some(hp) = resp.hover_pos() {
        let icon = match (st.tool, hit(st, hp)) {
            (Tool::Razor, _) => egui::CursorIcon::Crosshair,
            (Tool::Move, Some((_, _, true, _))) | (Tool::Move, Some((_, _, _, true))) => {
                egui::CursorIcon::ResizeHorizontal
            }
            (Tool::Move, Some(_)) => egui::CursorIcon::Grab,
            _ => egui::CursorIcon::Text,
        };
        if lanes.contains(hp) {
            ui.ctx().set_cursor_icon(icon);
        }
    }

    // Drawing.
    draw_ruler(&p, ruler, &v, &th, &st.session.markers);
    p.rect_filled(
        Rect::from_min_max(rect.min, pos2(ruler.left(), ruler.bottom())),
        Rounding::ZERO,
        c(th.panel),
    );
    let lane_p = ui.painter_at(lanes);
    let head_p = ui.painter_at(headers);
    let mut top = lanes.top() - scroll;
    let solo = st.session.any_solo();
    let tracks = st.session.tracks.clone();
    for (ti, tr) in tracks.iter().enumerate() {
        let row = Rect::from_min_size(pos2(lanes.left(), top), vec2(lanes.width(), tr.height));
        let head = Rect::from_min_size(pos2(headers.left(), top), vec2(HEADER_W, tr.height));
        top += tr.height;
        if row.bottom() < lanes.top() || row.top() > lanes.bottom() {
            continue;
        }
        let selected = st.selected_track == Some(tr.id);
        lane_p.rect_filled(
            row,
            Rounding::ZERO,
            if selected {
                ca(tr.color, 10)
            } else {
                c(th.wave_background)
            },
        );
        lane_p.line_segment(
            [row.left_bottom(), row.right_bottom()],
            Stroke::new(1.0_f32, c(th.grid)),
        );
        let heard = !tr.mute && (!solo || tr.solo);
        for clip in &tr.clips {
            let x0 = time_to_x(&v, lanes, clip.start);
            let x1 = time_to_x(&v, lanes, clip.end());
            if x1 < lanes.left() || x0 > lanes.right() {
                continue;
            }
            let cr = Rect::from_min_max(
                pos2(x0, row.top() + 2.0),
                pos2(x1.max(x0 + 2.0), row.bottom() - 2.0),
            );
            let sel = st.selected_clip == Some(clip.id);
            let alpha = if heard && !clip.muted { 255 } else { 90 };
            let rnd = Rounding::same(th.rounding.min(cr.height() / 3.0));
            lane_p.rect_filled(cr, rnd, ca(tr.color, (alpha as f32 * 0.16) as u8));
            let title = Rect::from_min_size(cr.min, vec2(cr.width(), 15.0));
            lane_p.rect_filled(
                title,
                Rounding {
                    nw: rnd.nw,
                    ne: rnd.ne,
                    sw: 0.0,
                    se: 0.0,
                },
                ca(tr.color, (alpha as f32 * 0.55) as u8),
            );
            lane_p.text(
                title.left_center() + vec2(5.0, 0.0),
                Align2::LEFT_CENTER,
                &clip.name,
                FontId::proportional(10.0),
                c(th.background),
            );
            // Waveform of the visible part.
            if let Some(buf) = st.media_buffer(clip.media) {
                let doc_peaks = st
                    .media_docs
                    .get(&clip.media)
                    .and_then(|d| st.doc_by_id(*d))
                    .map(|d| d.peaks.clone());
                if let Some(peaks) = doc_peaks {
                    let vis0 = x0.max(lanes.left());
                    let vis1 = x1.min(lanes.right());
                    let t0 = x_to_time(&v, lanes, vis0);
                    let t1 = x_to_time(&v, lanes, vis1);
                    let src = View {
                        start: clip.offset + (t0 - clip.start),
                        len: (t1 - t0).max(1e-6),
                    };
                    let wave =
                        Rect::from_min_max(pos2(vis0, title.bottom()), pos2(vis1, cr.bottom()));
                    let n = buf.num_channels().min(2);
                    let h = wave.height() / n as f32;
                    let tint = if alpha == 255 {
                        tr.color
                    } else {
                        [90, 90, 100]
                    };
                    for ch in 0..n {
                        let lane = Rect::from_min_size(
                            pos2(wave.left(), wave.top() + h * ch as f32),
                            vec2(wave.width(), h),
                        );
                        draw_channel(
                            &lane_p,
                            lane,
                            &buf.channels[ch],
                            &peaks,
                            ch,
                            buf.rate,
                            &src,
                            &th,
                            Some(tint),
                        );
                    }
                }
            } else {
                let path = st.session.media(clip.media).and_then(|m| m.path.clone());
                let loading = path.as_ref().is_some_and(|p| p.exists());
                let msg = match (loading, &path) {
                    (true, _) => "Loading…".to_string(),
                    (false, Some(p)) => format!("Missing: {}", p.display()),
                    (false, None) => "Missing media".to_string(),
                };
                lane_p.text(
                    cr.center(),
                    Align2::CENTER_CENTER,
                    msg,
                    FontId::proportional(10.0),
                    if loading {
                        c(th.text_dim)
                    } else {
                        c(th.accent_alt)
                    },
                );
            }
            // Fades.
            if clip.fade_in > 0.0 {
                let fx = time_to_x(&v, lanes, clip.start + clip.fade_in);
                lane_p.line_segment(
                    [cr.left_bottom(), pos2(fx, cr.top() + 15.0)],
                    Stroke::new(1.5_f32, c(th.text)),
                );
            }
            if clip.fade_out > 0.0 {
                let fx = time_to_x(&v, lanes, clip.end() - clip.fade_out);
                lane_p.line_segment(
                    [pos2(fx, cr.top() + 15.0), cr.right_bottom()],
                    Stroke::new(1.5_f32, c(th.text)),
                );
            }
            let stroke = if sel {
                Stroke::new(2.0_f32, c(th.accent))
            } else {
                Stroke::new(1.0_f32, ca(tr.color, 200))
            };
            lane_p.rect_stroke(cr, rnd, stroke);
            if sel {
                th.glow_rect(&lane_p, cr, th.accent, 0.8);
            }
        }
        for mc in &tr.midi {
            let x0 = time_to_x(&v, lanes, tempo.secs(mc.start));
            let x1 = time_to_x(&v, lanes, tempo.secs(mc.end()));
            if x1 < lanes.left() || x0 > lanes.right() {
                continue;
            }
            let cr = Rect::from_min_max(
                pos2(x0, row.top() + 2.0),
                pos2(x1.max(x0 + 2.0), row.bottom() - 2.0),
            );
            let sel = st.selected_clip == Some(mc.id);
            let rnd = Rounding::same(th.rounding.min(cr.height() / 3.0));
            lane_p.rect_filled(
                cr,
                rnd,
                ca(tr.color, if heard && !mc.muted { 40 } else { 15 }),
            );
            let title = Rect::from_min_size(cr.min, vec2(cr.width(), 15.0));
            lane_p.rect_filled(
                title,
                Rounding {
                    nw: rnd.nw,
                    ne: rnd.ne,
                    sw: 0.0,
                    se: 0.0,
                },
                ca(tr.color, 150),
            );
            lane_p.text(
                title.left_center() + vec2(5.0, 0.0),
                Align2::LEFT_CENTER,
                format!("♪ {}", mc.name),
                FontId::proportional(10.0),
                c(th.background),
            );
            if !mc.notes.is_empty() {
                let (lo, hi) = mc
                    .notes
                    .iter()
                    .fold((127u8, 0u8), |(a, b), n| (a.min(n.pitch), b.max(n.pitch)));
                let span = (hi.saturating_sub(lo) as f32).max(12.0);
                let body = Rect::from_min_max(
                    pos2(cr.left(), title.bottom() + 2.0),
                    pos2(cr.right(), cr.bottom() - 2.0),
                );
                for n in mc.notes.iter().filter(|n| n.start < mc.length) {
                    let nx0 = time_to_x(&v, lanes, tempo.secs(mc.start + n.start));
                    let nx1 = time_to_x(
                        &v,
                        lanes,
                        tempo.secs(mc.start + (n.start + n.length).min(mc.length)),
                    );
                    let y =
                        body.bottom() - (n.pitch - lo) as f32 / span * (body.height() - 3.0) - 3.0;
                    lane_p.rect_filled(
                        Rect::from_min_max(pos2(nx0, y), pos2(nx1.max(nx0 + 1.5), y + 3.0)),
                        Rounding::same(1.0),
                        crate::pianoroll::channel_color(n.channel),
                    );
                }
            }
            lane_p.rect_stroke(
                cr,
                rnd,
                if sel {
                    Stroke::new(2.0_f32, c(th.accent))
                } else {
                    Stroke::new(1.0_f32, ca(tr.color, 200))
                },
            );
        }
        // Header.
        head_p.rect_filled(
            head,
            Rounding::ZERO,
            if selected {
                lerp(th.panel, tr.color, 0.12)
            } else {
                c(th.panel)
            },
        );
        head_p.rect_filled(
            Rect::from_min_size(head.min, vec2(4.0, head.height())),
            Rounding::ZERO,
            c(tr.color),
        );
        head_p.line_segment(
            [head.left_bottom(), head.right_bottom()],
            Stroke::new(1.0_f32, c(th.border)),
        );
        track_header(ui, st, ti, head.shrink2(vec2(8.0, 4.0)));
    }
    // Add-track hint row.
    let add_row = Rect::from_min_size(pos2(headers.left(), top), vec2(HEADER_W, 26.0));
    if add_row.top() < headers.bottom() {
        let r = ui.put(
            add_row.shrink(3.0),
            egui::Button::new(RichText::new("+ Add Track").color(c(th.text_dim))),
        );
        if r.clicked() {
            st.add_track();
        }
    }
    // Selection, markers, playhead.
    if let Some((a, b)) = st.session_sel {
        let x0 = time_to_x(&v, lanes, a).max(lanes.left());
        let x1 = time_to_x(&v, lanes, b).min(lanes.right());
        if x1 > x0 {
            lane_p.rect_filled(
                Rect::from_min_max(pos2(x0, lanes.top()), pos2(x1, lanes.bottom())),
                Rounding::ZERO,
                ca(th.selection, th.selection_alpha),
            );
        }
    }
    for m in &st.session.markers {
        let x = time_to_x(&v, lanes, m.time);
        if x >= lanes.left() && x <= lanes.right() {
            crate::waveform::dashed_v(&lane_p, x, lanes.top(), lanes.bottom(), c(th.marker));
        }
    }
    let x = time_to_x(&v, lanes, st.position());
    if x >= lanes.left() && x <= lanes.right() {
        playhead(&p, x, ruler.top(), lanes.bottom(), &th);
    }
    if st
        .session
        .tracks
        .iter()
        .all(|t| t.clips.is_empty() && t.midi.is_empty())
    {
        p.text(
            lanes.center(),
            Align2::CENTER_CENTER,
            "Drag files here from the Files panel, or open a .sesx / .audfrm / .af27 session",
            FontId::proportional(14.0),
            c(th.text_dim),
        );
    }
}

fn track_header(ui: &mut egui::Ui, st: &mut State, ti: usize, rect: Rect) {
    let th = st.theme.clone();
    let mut changed = false;
    let before = st.session.clone();
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
        let tr = &mut st.session.tracks[ti];
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut tr.name)
                    .desired_width(86.0)
                    .frame(false)
                    .font(egui::TextStyle::Body),
            );
            let toggle =
                |ui: &mut egui::Ui, on: &mut bool, label: &str, col: [u8; 3], tip: &str| {
                    let b =
                        egui::Button::new(RichText::new(label).strong().size(11.0).color(if *on {
                            c(th.background)
                        } else {
                            c(th.text_dim)
                        }))
                        .fill(if *on { c(col) } else { c(th.panel_raised) })
                        .min_size(vec2(20.0, 18.0));
                    if ui.add(b).on_hover_text(tip).clicked() {
                        *on = !*on;
                        return true;
                    }
                    false
                };
            changed |= toggle(ui, &mut tr.mute, "M", th.meter_mid, "Mute");
            changed |= toggle(ui, &mut tr.solo, "S", th.accent, "Solo");
            changed |= toggle(ui, &mut tr.armed, "R", th.accent_alt, "Arm for recording");
        });
        if tr.height >= 60.0 {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Vol").size(10.0).color(c(th.text_dim)));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut tr.volume_db)
                            .speed(0.1)
                            .range(-90.0..=12.0)
                            .suffix(" dB")
                            .max_decimals(1),
                    )
                    .changed();
                ui.label(RichText::new("Pan").size(10.0).color(c(th.text_dim)));
                let mut pan = tr.pan * 100.0;
                if ui
                    .add(
                        egui::DragValue::new(&mut pan)
                            .speed(1.0)
                            .range(-100.0..=100.0)
                            .max_decimals(0),
                    )
                    .changed()
                {
                    tr.pan = pan / 100.0;
                    changed = true;
                }
            });
        }
    });
    if changed {
        let down = ui.input(|i| i.pointer.any_down());
        if !st.gesture {
            st.commit_live("Track settings", before);
            st.gesture = down;
        }
        st.live_track(ti);
    }
}

/// Fader travel for a level: 0 at -inf (the bottom), 1 at +12 dB, with
/// more room around unity where mixing happens, as on a desk.
fn fader_pos(db: f32) -> f32 {
    const MAP: [(f32, f32); 8] = [
        (-90.0, 0.0),
        (-60.0, 0.04),
        (-48.0, 0.12),
        (-24.0, 0.34),
        (-12.0, 0.52),
        (-6.0, 0.64),
        (0.0, 0.78),
        (12.0, 1.0),
    ];
    let db = db.clamp(-90.0, 12.0);
    for w in MAP.windows(2) {
        let ((d0, p0), (d1, p1)) = (w[0], w[1]);
        if db <= d1 {
            return p0 + (p1 - p0) * (db - d0) / (d1 - d0);
        }
    }
    1.0
}

fn fader_db(pos: f32) -> f32 {
    // Invert by bisection: the map is monotonic.
    let (mut lo, mut hi) = (-90.0f32, 12.0f32);
    for _ in 0..30 {
        let mid = 0.5 * (lo + hi);
        if fader_pos(mid) < pos {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// A tall fader with a dB scale and, if `meter` is given, a stereo meter
/// beside it. Drag to move (Shift for fine), double-click for 0 dB.
fn fader(
    ui: &mut egui::Ui,
    id: egui::Id,
    db: &mut f32,
    rect: Rect,
    th: &crate::theme::Theme,
    meter: Option<(f32, f32)>,
) -> bool {
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let p = ui.painter_at(rect.expand(24.0));
    let track = Rect::from_min_max(
        pos2(rect.center().x - 2.0, rect.top() + 8.0),
        pos2(rect.center().x + 2.0, rect.bottom() - 8.0),
    );
    let y_of = |d: f32| track.bottom() - fader_pos(d) * track.height();
    let mut changed = false;
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let dy = resp.drag_delta().y * if fine { 0.2 } else { 1.0 };
        let pos = (fader_pos(*db) - dy / track.height()).clamp(0.0, 1.0);
        *db = (fader_db(pos) * 10.0).round() / 10.0;
        changed = true;
    }
    if resp.double_clicked() {
        *db = 0.0;
        changed = true;
    }
    // Scale.
    for (d, label) in [
        (12.0, "+12"),
        (6.0, "+6"),
        (0.0, "0"),
        (-6.0, "-6"),
        (-12.0, "-12"),
        (-24.0, "-24"),
        (-48.0, "-48"),
    ] {
        let y = y_of(d);
        let strong = d == 0.0;
        p.line_segment(
            [pos2(track.left() - 9.0, y), pos2(track.left() - 3.0, y)],
            Stroke::new(1.0_f32, ca(th.text_dim, if strong { 220 } else { 120 })),
        );
        p.text(
            pos2(track.left() - 11.0, y),
            Align2::RIGHT_CENTER,
            label,
            FontId::proportional(8.5),
            ca(th.text_dim, if strong { 230 } else { 150 }),
        );
    }
    p.rect_filled(track, Rounding::same(2.0), c(th.wave_background));
    p.rect_stroke(
        track,
        Rounding::same(2.0),
        Stroke::new(1.0_f32, ca(th.border, 200)),
    );
    // Meter to the right of the groove.
    if let Some((l, r)) = meter {
        for (k, v) in [l, r].into_iter().enumerate() {
            let mdb = crate::ui::meter_db(v);
            let col = Rect::from_min_max(
                pos2(track.right() + 8.0 + k as f32 * 6.0, track.top()),
                pos2(track.right() + 12.0 + k as f32 * 6.0, track.bottom()),
            );
            p.rect_filled(col, Rounding::same(1.0), c(th.wave_background));
            let top = y_of(mdb);
            let mut y = col.bottom();
            while y > top {
                let seg_db = fader_db(((track.bottom() - y) / track.height()).clamp(0.0, 1.0));
                let y0 = (y - 2.0).max(top);
                p.rect_filled(
                    Rect::from_min_max(pos2(col.left(), y0), pos2(col.right(), y)),
                    Rounding::ZERO,
                    th.meter_color(seg_db),
                );
                y -= 3.0;
            }
        }
    }
    // The cap.
    let cy = y_of(*db);
    let cap = Rect::from_center_size(pos2(track.center().x, cy), vec2(28.0, 14.0));
    let hot = resp.hovered() || resp.dragged();
    let square = th.bevel.is_some();
    p.rect_filled(
        cap.translate(vec2(1.0, 2.0)),
        Rounding::same(2.0),
        Color32::from_black_alpha(90),
    );
    p.rect_filled(
        cap,
        Rounding::same(if square { 0.0 } else { 2.0 }),
        lerp(th.panel_raised, th.text, if hot { 0.28 } else { 0.18 }),
    );
    if square {
        crate::skin::bevel(&p, cap, th, false);
    } else {
        p.rect_stroke(
            cap,
            Rounding::same(2.0),
            Stroke::new(
                1.0_f32,
                if hot {
                    c(th.accent)
                } else {
                    ca(th.border, 230)
                },
            ),
        );
    }
    p.line_segment(
        [pos2(cap.left() + 4.0, cy), pos2(cap.right() - 4.0, cy)],
        Stroke::new(2.0_f32, c(th.accent)),
    );
    resp.on_hover_text("Drag; Shift for fine steps; double-click for 0 dB");
    changed
}

/// A pan knob: drag up/down or left/right, double-click to centre.
fn pan_knob(ui: &mut egui::Ui, pan: &mut f32, th: &crate::theme::Theme) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::click_and_drag());
    let mut changed = false;
    if resp.dragged() {
        let d = resp.drag_delta();
        *pan = (*pan + (d.x - d.y) * 0.01).clamp(-1.0, 1.0);
        changed = true;
    }
    if resp.double_clicked() {
        *pan = 0.0;
        changed = true;
    }
    let p = ui.painter();
    let cen = rect.center();
    let r = 13.0;
    p.circle_filled(cen, r, lerp(th.panel_raised, th.text, 0.12));
    p.circle_stroke(cen, r, Stroke::new(1.0_f32, ca(th.border, 230)));
    // Arc from the top (centre) to the pan position.
    let to = *pan * 135.0f32.to_radians();
    let n = 16;
    let pts: Vec<_> = (0..=n)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + to * i as f32 / n as f32;
            cen + vec2(a.cos(), a.sin()) * (r + 3.0)
        })
        .collect();
    p.add(egui::Shape::line(pts, Stroke::new(2.0_f32, c(th.accent))));
    let a = -std::f32::consts::FRAC_PI_2 + to;
    p.line_segment(
        [
            cen + vec2(a.cos(), a.sin()) * 4.0,
            cen + vec2(a.cos(), a.sin()) * (r - 2.0),
        ],
        Stroke::new(2.0_f32, c(th.text)),
    );
    let label = if pan.abs() < 0.005 {
        "C".to_string()
    } else if *pan < 0.0 {
        format!("L{:.0}", -*pan * 100.0)
    } else {
        format!("R{:.0}", *pan * 100.0)
    };
    resp.on_hover_text(format!("Pan {label} (drag; double-click to centre)"));
    changed
}

/// A square mixer key (Mute, Solo, Arm).
fn key(
    ui: &mut egui::Ui,
    on: &mut bool,
    label: &str,
    col: [u8; 3],
    tip: &str,
    th: &crate::theme::Theme,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 20.0), Sense::click());
    let p = ui.painter();
    let square = th.bevel.is_some();
    let face = if *on {
        c(col)
    } else {
        lerp(
            th.panel_raised,
            th.text,
            if resp.hovered() { 0.12 } else { 0.06 },
        )
    };
    p.rect_filled(rect, Rounding::same(if square { 0.0 } else { 3.0 }), face);
    if square {
        crate::skin::bevel(p, rect, th, *on);
    } else {
        p.rect_stroke(
            rect,
            Rounding::same(3.0),
            Stroke::new(1.0_f32, ca(th.border, 220)),
        );
    }
    let tc = if *on { c(th.background) } else { c(th.text) };
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        tc,
    );
    if resp.on_hover_text(tip).clicked() {
        *on = !*on;
        return true;
    }
    false
}

/// The card behind a strip: solid enough to read over any backdrop.
fn strip_card(ui: &egui::Ui, rect: Rect, th: &crate::theme::Theme, master: bool) {
    let p = ui.painter();
    let fill = if master {
        lerp(th.panel_raised, th.accent, 0.07)
    } else {
        c(th.panel_raised)
    };
    let rounding = Rounding::same(if th.bevel.is_some() {
        0.0
    } else {
        th.rounding.min(6.0)
    });
    p.rect_filled(
        rect,
        rounding,
        Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), 242),
    );
    if th.bevel.is_some() {
        crate::skin::panel(p, rect, th);
    } else {
        p.rect_stroke(rect, rounding, Stroke::new(1.0_f32, ca(th.border, 200)));
    }
}

/// The Mixer: one full-height strip per track (colour, name, Mute, Solo,
/// Arm, pan knob, fader with scale, level box) and the master with its
/// meter, in the leading editor's arrangement.
pub fn mixer(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    let before = st.session.clone();
    let mut changed = false;
    let avail = ui.available_rect_before_wrap();
    let h = avail.height().max(260.0);
    const W: f32 = 92.0;
    egui::ScrollArea::horizontal().show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let n = st.session.tracks.len();
            for ti in 0..n {
                let (rect, _) = ui.allocate_exact_size(vec2(W, h - 4.0), Sense::hover());
                strip_card(ui, rect, &th, false);
                let tr = &mut st.session.tracks[ti];
                let id = egui::Id::new(("mixer-strip", tr.id));
                let mut cui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect.shrink(6.0))
                        .layout(egui::Layout::top_down(egui::Align::Center)),
                );
                let (cap, _) = cui.allocate_exact_size(vec2(W - 12.0, 4.0), Sense::hover());
                cui.painter()
                    .rect_filled(cap, Rounding::same(2.0), c(tr.color));
                cui.add(egui::Label::new(RichText::new(&tr.name).strong().size(11.0)).truncate());
                cui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    ui.add_space(((W - 12.0) - 3.0 * 22.0 - 6.0) / 2.0);
                    changed |= key(ui, &mut tr.mute, "M", th.meter_mid, "Mute", &th);
                    changed |= key(ui, &mut tr.solo, "S", th.accent, "Solo", &th);
                    changed |= key(
                        ui,
                        &mut tr.armed,
                        "R",
                        th.accent_alt,
                        "Arm for recording",
                        &th,
                    );
                });
                changed |= pan_knob(&mut cui, &mut tr.pan, &th);
                // The fader takes the rest, above the level box.
                let top = cui.cursor().top() + 4.0;
                let fr = Rect::from_center_size(
                    pos2(rect.center().x + 6.0, (top + rect.bottom() - 34.0) / 2.0),
                    vec2(36.0, (rect.bottom() - 34.0 - top).max(40.0)),
                );
                changed |= fader(ui, id.with("fader"), &mut tr.volume_db, fr, &th, None);
                let boxr = Rect::from_min_size(
                    pos2(rect.center().x - 32.0, rect.bottom() - 28.0),
                    vec2(64.0, 20.0),
                );
                changed |= ui
                    .put(
                        boxr,
                        egui::DragValue::new(&mut tr.volume_db)
                            .range(-90.0..=12.0)
                            .speed(0.1)
                            .suffix(" dB")
                            .max_decimals(1),
                    )
                    .changed();
            }
            // Master.
            let (rect, _) = ui.allocate_exact_size(vec2(W + 10.0, h - 4.0), Sense::hover());
            strip_card(ui, rect, &th, true);
            ui.painter().text(
                pos2(rect.center().x, rect.top() + 14.0),
                Align2::CENTER_CENTER,
                "MASTER",
                FontId::proportional(12.0),
                c(th.accent),
            );
            let fr = Rect::from_center_size(
                pos2(
                    rect.center().x + 2.0,
                    (rect.top() + 36.0 + rect.bottom() - 34.0) / 2.0,
                ),
                vec2(36.0, (rect.height() - 70.0).max(40.0)),
            );
            changed |= fader(
                ui,
                egui::Id::new("mixer-master"),
                &mut st.session.master_db,
                fr,
                &th,
                Some(st.meter),
            );
            let boxr = Rect::from_min_size(
                pos2(rect.center().x - 32.0, rect.bottom() - 28.0),
                vec2(64.0, 20.0),
            );
            changed |= ui
                .put(
                    boxr,
                    egui::DragValue::new(&mut st.session.master_db)
                        .range(-90.0..=12.0)
                        .speed(0.1)
                        .suffix(" dB")
                        .max_decimals(1),
                )
                .changed();
        });
    });
    if changed {
        let down = ui.input(|i| i.pointer.any_down());
        if !st.gesture {
            st.commit_live("Mixer", before);
            st.gesture = down;
        }
        for ti in 0..st.session.tracks.len() {
            st.live_track(ti);
        }
        st.live_master();
    }
}

#[cfg(test)]
mod mixer_tests {
    #[test]
    fn fader_law_round_trips_and_is_monotonic() {
        let mut last = -1.0;
        for d in [-90.0f32, -60.0, -30.0, -12.0, -3.0, 0.0, 6.0, 12.0] {
            let p = super::fader_pos(d);
            assert!(p > last);
            last = p;
            assert!((super::fader_db(p) - d).abs() < 0.05, "{d}");
        }
        assert_eq!(super::fader_pos(0.0), 0.78);
    }
}
