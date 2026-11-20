// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The Multitrack editor (tracks of clips on a timeline) and the Mixer.
//!
//! Move tool: drag clips in time and between tracks, drag a clip's ends
//! to trim it. Razor: click a clip to split it there. Time Selection:
//! drag to select a range. Drop files from the Files panel onto a track.
//!
//! Wheel (the full table is [`crate::zoom::wheel_action`]): over the
//! ruler and the overview strip it zooms in time at the pointer; over the
//! lanes it scrolls the tracks and Ctrl+wheel zooms in time exactly as the
//! ruler does; over the track headers it makes every track taller or
//! shorter evenly; Shift+wheel scrolls in time; Alt+wheel resizes the
//! track under the pointer; Alt+Shift+wheel zooms the waveforms'
//! amplitude (display only), or over the headers scales every track.
//! Height changes by wheel become one undo step once the wheel rests.
//! The bar on the right of the lanes scrolls the tracks; dragging its
//! ends or the wheel over it zooms the track heights (every track the
//! same height); double-click it to fit every track in view. The overview
//! strip above the ruler is in [`crate::overview`].

use egui::{pos2, vec2, Align2, Color32, FontId, Rect, RichText, Rounding, Sense, Stroke};

use crate::state::{State, Tool, View};
use crate::theme::{c, ca, lerp};
use crate::ui::Caches;
use crate::waveform::{draw_channel_gained, playhead, time_to_x, x_to_time};
use af27_core::{ClipId, Session};

const HEADER_W: f32 = 210.0;
/// Width of the vertical zoom bar on the right of the lanes.
const VBAR_W: f32 = 12.0;
/// Track height limits for the wheel and the zoom bar.
const MIN_TRACK_H: f32 = crate::zoom::TRACK_MIN_H;
const MAX_TRACK_H: f32 = crate::zoom::TRACK_MAX_H;
/// Room under the last track (the Add Track row and the Master row).
pub(crate) const TAIL_H: f32 = 40.0;

/// What a drag on the vertical zoom bar holds.
#[derive(Debug, Clone, Copy)]
pub enum VGrab {
    /// The thumb, grabbed this far below its top.
    Body(f32),
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy)]
pub enum DragKind {
    Move {
        clip: ClipId,
        grab: f64,
    },
    TrimLeft {
        clip: ClipId,
    },
    TrimRight {
        clip: ClipId,
    },
    Select {
        anchor: f64,
    },
    MoveMidi {
        clip: ClipId,
        grab: f64,
    },
    /// A fade handle: sideways sets the length, up and down the curve.
    Fade {
        clip: ClipId,
        fade_in: bool,
        y0: f32,
        skew0: f32,
    },
    /// The clip-gain handle: up or right is louder.
    Gain {
        clip: ClipId,
        p0: egui::Pos2,
        db0: f32,
    },
    /// A stretch handle on a clip corner.
    Stretch {
        clip: ClipId,
        left: bool,
    },
    /// A stretch handle on a MIDI clip corner.
    StretchMidi {
        clip: ClipId,
        left: bool,
    },
}

/// A handle on a clip, under the pointer.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Handle {
    Fade(bool),
    Gain,
    Stretch(bool),
}

#[derive(Default)]
pub struct Drag {
    pub kind: Option<DragKind>,
    pub before: Option<Session>,
    pub scroll_y: f32,
    pub vgrab: Option<VGrab>,
    /// Where the right-click menu was opened.
    pub menu: crate::lane_menu::MenuAt,
    /// A track being moved by its colour tab.
    pub track_move: Option<af27_core::TrackId>,
}

/// The rectangle a clip fills in its row.
fn clip_rect(row_top: f32, row_h: f32, x0: f32, x1: f32) -> Rect {
    Rect::from_min_max(
        pos2(x0, row_top + 2.0),
        pos2(x1.max(x0 + 2.0), row_top + row_h - 2.0),
    )
}

/// The fade handle's square, the gain box and the stretch triangles.
fn fade_handle(cr: Rect, x: f32) -> Rect {
    Rect::from_center_size(
        pos2(x.clamp(cr.left() + 5.0, cr.right() - 5.0), cr.top() + 20.0),
        vec2(9.0, 9.0),
    )
}
fn gain_handle(cr: Rect) -> Rect {
    Rect::from_min_size(pos2(cr.left() + 3.0, cr.bottom() - 15.0), vec2(16.0, 12.0))
}
/// A clip gain as its readout shows it: "+1.2", "-3.4", "0.0".
fn gain_text(db: f32) -> String {
    let r = (db * 10.0).round() / 10.0;
    if r == 0.0 {
        "0.0".into()
    } else {
        format!("{r:+.1}")
    }
}
fn stretch_handle(cr: Rect, left: bool) -> Rect {
    if left {
        Rect::from_min_size(cr.min, vec2(11.0, 11.0))
    } else {
        Rect::from_min_size(pos2(cr.right() - 11.0, cr.top()), vec2(11.0, 11.0))
    }
}

/// The uniform track height at which `share` (0..1] of the session's
/// height fits in a view `view_h` tall with `n` tracks.
pub fn height_for_share(share: f32, view_h: f32, n: usize) -> f32 {
    let content = view_h / share.clamp(0.01, 1.0);
    ((content - TAIL_H) / n.max(1) as f32).clamp(MIN_TRACK_H, MAX_TRACK_H)
}

#[cfg(test)]
mod vzoom_tests {
    use super::*;

    #[test]
    fn the_gain_readout_is_signed_with_one_decimal() {
        assert_eq!(gain_text(1.2), "+1.2");
        assert_eq!(gain_text(-3.44), "-3.4");
        assert_eq!(gain_text(0.0), "0.0");
        assert_eq!(gain_text(-0.04), "0.0");
        assert_eq!(gain_text(12.0), "+12.0");
    }

    #[test]
    fn the_thumb_share_sets_one_height_for_every_track() {
        // Half the session in a 400 px view with 4 tracks: 800 px in all.
        let h = height_for_share(0.5, 400.0, 4);
        assert!((h * 4.0 + TAIL_H - 800.0).abs() < 1e-3);
        // Limits.
        assert_eq!(height_for_share(1.0, 100.0, 50), MIN_TRACK_H);
        assert_eq!(height_for_share(0.01, 4000.0, 1), MAX_TRACK_H);
    }
}

pub(crate) fn track_at(st: &State, lanes_top: f32, scroll: f32, y: f32) -> Option<usize> {
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
    if st.no_session() {
        no_session(ui, st);
        return;
    }
    let th = st.theme.clone();
    let full = ui.available_rect_before_wrap();
    let (rect, resp) = ui.allocate_exact_size(full.size(), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, Rounding::ZERO, c(th.wave_background));
    // The overview strip, then the ruler, over the lanes.
    let overview = Rect::from_min_size(
        pos2(rect.left() + HEADER_W, rect.top()),
        vec2(rect.width() - HEADER_W - VBAR_W, crate::overview::STRIP_H),
    );
    let full_btn = Rect::from_min_max(
        pos2(overview.right(), overview.top()),
        pos2(rect.right(), overview.bottom()),
    );
    let ruler = Rect::from_min_size(
        pos2(rect.left() + HEADER_W, overview.bottom()),
        vec2(rect.width() - HEADER_W - VBAR_W, 22.0),
    );
    // The metronome row, while it is on, sits between the ruler and the
    // tracks (it is not a track).
    let metro_h = if st.session.tempo.metronome {
        crate::track_views::METRO_H
    } else {
        0.0
    };
    let metro = Rect::from_min_max(
        pos2(rect.left(), ruler.bottom()),
        pos2(ruler.right(), ruler.bottom() + metro_h),
    );
    // The Master row is pinned to the bottom of the editor.
    let master_band = Rect::from_min_max(
        pos2(rect.left(), rect.bottom() - crate::track_views::MASTER_H),
        pos2(ruler.right(), rect.bottom()),
    );
    let lanes = Rect::from_min_max(
        pos2(ruler.left(), metro.bottom()),
        pos2(ruler.right(), master_band.top()),
    );
    let vbar = Rect::from_min_max(
        pos2(lanes.right(), lanes.top()),
        pos2(rect.right(), lanes.bottom()),
    );
    st.session_px = lanes.width();
    st.session_lanes_h = lanes.height();
    let headers = Rect::from_min_max(
        pos2(rect.left(), metro.bottom()),
        pos2(ruler.left(), master_band.top()),
    );
    let vresp = ui.interact(vbar, ui.id().with("af27-vzoom"), Sense::click_and_drag());
    // The ruler is its own widget on top: it moves the playhead.
    let rresp = ui.interact(
        ruler,
        ui.id().with("af27-mt-ruler"),
        Sense::click_and_drag(),
    );
    let drag = &mut caches.drag;

    // Wheel. The pointer is read directly so the header widgets on top
    // do not hide it.
    let pointer = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| rect.contains(*p) && ui.rect_contains_pointer(rect));
    if let Some(hp) = pointer {
        let input = ui.input(|i| {
            crate::zoom::WheelInput::read(
                i.raw_scroll_delta.into(),
                i.zoom_delta(),
                i.modifiers.command || i.modifiers.ctrl,
                i.modifiers.shift,
                i.modifiers.alt,
            )
        });
        if let Some(input) = input {
            let areas = Areas {
                overview,
                ruler,
                lanes,
                headers,
                vbar,
            };
            wheel(st, drag, &areas, hp, input);
        }
    }
    if st.height_commit.is_pending() {
        // Wake up to keep the resting wheel gesture as an undo step.
        let quiet = crate::zoom::HEIGHT_COMMIT_QUIET as f32 + 0.02;
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f32(quiet));
    }
    crate::overview::input(ui, st, overview, full_btn);
    vzoom_bar(ui, st, drag, &vresp, lanes, vbar);
    let total_h: f32 = st.session.tracks.iter().map(|t| t.height).sum();
    // Tracks that all fit never scroll (the tail below them alone is no
    // reason to).
    let max_scroll = if total_h > lanes.height() {
        total_h - lanes.height() + TAIL_H
    } else {
        0.0
    };
    drag.scroll_y = drag.scroll_y.clamp(0.0, max_scroll);
    let lim = st.session_limits();
    st.session_view.limit(&lim);
    let v = st.session_view;

    // Ruler clicks and drags position the playhead.
    let marker_times: Vec<f64> = st.session.markers.iter().map(|m| m.time).collect();
    let ghost = crate::ruler::input(ui, st, &rresp, lanes, &v, f64::MAX, &marker_times, true);
    crate::tempo::markers_input(ui, st, ruler, &|t| time_to_x(&v, ruler, t), &|x| {
        x_to_time(&v, ruler, x)
    });
    // Middle-button drag anywhere but the track headers pans: sideways in
    // time, up and down through the tracks.
    let from_headers = ui
        .input(|i| i.pointer.press_origin())
        .is_some_and(|p| headers.contains(p));
    if !from_headers {
        if let Some(dv) = crate::ruler::pan_delta(ui, &resp).or(crate::ruler::pan_delta(ui, &rresp))
        {
            st.session_view
                .pan_px(-dv.x as f64, lanes.width() as f64, &lim);
            drag.scroll_y = (drag.scroll_y - dv.y).clamp(0.0, max_scroll);
            st.hand_moved_view();
        }
    }
    let scroll = drag.scroll_y;
    let playing = st.transport_playing();
    // Selection edges snap to ruler marks, markers, clip edges and the playhead.
    let mut snap_marks = marker_times.clone();
    // Not to the playhead while selecting: it sits on the selection start.
    if !matches!(drag.kind, Some(DragKind::Select { .. })) {
        snap_marks.push(st.session_cursor);
    }
    for tr in &st.session.tracks {
        for c in &tr.clips {
            snap_marks.push(c.start);
            snap_marks.push(c.end());
        }
    }
    let snapped = |st: &State, x: f32| {
        crate::ruler::snap(st, x_to_time(&v, lanes, x), &v, lanes, &snap_marks).max(0.0)
    };
    let on_edge = resp
        .interact_pointer_pos()
        .or(resp.hover_pos())
        .filter(|pp| lanes.contains(*pp) && st.tool != Tool::Razor)
        .and_then(|pp| crate::ruler::edge_at(st.session_sel, &v, lanes, pp.x));

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

    // A handle on the clip under a point (only on clips wide and tall
    // enough to show them).
    let stretch_on = st.prefs.stretch_handles;
    let handle_at = |st: &State, pp: egui::Pos2| -> Option<(usize, ClipId, Handle)> {
        let ti = track_at(st, lanes.top(), scroll, pp.y)?;
        let row_top = lanes.top() - scroll
            + st.session.tracks[..ti]
                .iter()
                .map(|t| t.height)
                .sum::<f32>();
        let tr = &st.session.tracks[ti];
        for c in tr.clips.iter().rev() {
            let x0 = time_to_x(&v, lanes, c.start);
            let x1 = time_to_x(&v, lanes, c.end());
            let cr = clip_rect(row_top, tr.height, x0, x1);
            if !cr.expand(4.0).contains(pp) || cr.width() < 24.0 || cr.height() < 34.0 {
                continue;
            }
            if stretch_on {
                for left in [true, false] {
                    if stretch_handle(cr, left).expand(2.0).contains(pp) {
                        return Some((ti, c.id, Handle::Stretch(left)));
                    }
                }
            }
            let fi = fade_handle(cr, time_to_x(&v, lanes, c.start + c.fade_in));
            if fi.expand(3.0).contains(pp) {
                return Some((ti, c.id, Handle::Fade(true)));
            }
            let fo = fade_handle(cr, time_to_x(&v, lanes, c.end() - c.fade_out));
            if fo.expand(3.0).contains(pp) {
                return Some((ti, c.id, Handle::Fade(false)));
            }
            if gain_handle(cr).expand(2.0).contains(pp) {
                return Some((ti, c.id, Handle::Gain));
            }
            if cr.contains(pp) {
                return None;
            }
        }
        None
    };

    // Right-click: remember where, select what is there; the menu opens
    // from the lanes' response below.
    if resp.secondary_clicked() {
        if let Some(pp) = resp
            .interact_pointer_pos()
            .filter(|pp| lanes.contains(*pp) || headers.contains(*pp))
        {
            let t = x_to_time(&v, lanes, pp.x).max(0.0);
            let track = track_at(st, lanes.top(), scroll, pp.y);
            // Which half of the track: new tracks go above or below it.
            let below = track.is_some_and(|ti| {
                let top_y = lanes.top() - scroll
                    + st.session.tracks[..ti]
                        .iter()
                        .map(|t| t.height)
                        .sum::<f32>();
                pp.y > top_y + st.session.tracks[ti].height / 2.0
            });
            let mut at = crate::lane_menu::MenuAt {
                below,
                t,
                track,
                clip: None,
                fade: None,
            };
            if let Some((_, id, Handle::Fade(fin))) = handle_at(st, pp) {
                at.fade = Some((id, fin));
            } else if let Some((_, id, _, _)) = hit(st, pp) {
                at.clip = Some(id);
                st.selected_clip = Some(id);
            }
            if let Some(ti) = track {
                st.selected_track = Some(st.session.tracks[ti].id);
            }
            drag.menu = at;
        }
    }

    // MIDI clips: (track index, clip id) under a point.
    let tempo = st.session.tempo.clone();
    let hit_midi = |st: &State, pp: egui::Pos2| -> Option<(usize, ClipId)> {
        let ti = track_at(st, lanes.top(), scroll, pp.y)?;
        let t = tempo.beats(x_to_time(&v, lanes, pp.x));
        st.session.tracks[ti]
            .midi
            .iter()
            .rev()
            .find(|c| t >= c.start && t <= c.end_beats(&tempo))
            .map(|c| (ti, c.id))
    };
    // A MIDI clip's stretch handle under a point (Stretch Handles on).
    let midi_stretch_at = |st: &State, pp: egui::Pos2| -> Option<(usize, ClipId, bool)> {
        if !stretch_on || st.tool != Tool::Move {
            return None;
        }
        let ti = track_at(st, lanes.top(), scroll, pp.y)?;
        let row_top = lanes.top() - scroll
            + st.session.tracks[..ti]
                .iter()
                .map(|t| t.height)
                .sum::<f32>();
        let tr = &st.session.tracks[ti];
        for c in tr.midi.iter().rev() {
            let x0 = time_to_x(&v, lanes, tempo.secs(c.start));
            let x1 = time_to_x(&v, lanes, c.end_secs(&tempo));
            let cr = clip_rect(row_top, tr.height, x0, x1);
            if cr.width() < 24.0 || cr.height() < 34.0 || !cr.expand(4.0).contains(pp) {
                continue;
            }
            for left in [true, false] {
                if stretch_handle(cr, left).expand(2.0).contains(pp) {
                    return Some((ti, c.id, left));
                }
            }
        }
        None
    };
    if resp.double_clicked() {
        if let Some(pp) = resp.interact_pointer_pos() {
            if let Some((_, id)) = hit_midi(st, pp) {
                st.piano_clip = Some(id);
                st.want_piano = true;
            } else if let Some(ti) = track_at(st, lanes.top(), scroll, pp.y) {
                if st.session.tracks[ti].kind == af27_core::TrackKind::Midi && lanes.contains(pp) {
                    let b = tempo.bar_floor(tempo.beats(x_to_time(&v, lanes, pp.x)).max(0.0));
                    let id = st.session.tracks[ti].id;
                    crate::pianoroll::new_clip(st, id, b);
                }
            }
        }
    }

    // Pointer.
    if let Some(pp) = resp.interact_pointer_pos() {
        if resp.drag_started_by(egui::PointerButton::Primary)
            && lanes.contains(pp)
            && st.tool == Tool::Move
            && on_edge.is_none()
        {
            if let Some((ti, id, left)) = midi_stretch_at(st, pp) {
                st.selected_clip = Some(id);
                st.selected_track = Some(st.session.tracks[ti].id);
                drag.before = Some(st.session.clone());
                drag.kind = Some(DragKind::StretchMidi { clip: id, left });
            } else if let Some((ti, id)) = hit_midi(st, pp) {
                let (mti, mci) = st.session.find_midi(id).unwrap();
                let grab = tempo.beats(x_to_time(&v, lanes, pp.x))
                    - st.session.tracks[mti].midi[mci].start;
                st.selected_clip = Some(id);
                st.selected_track = Some(st.session.tracks[ti].id);
                drag.before = Some(st.session.clone());
                drag.kind = Some(DragKind::MoveMidi { clip: id, grab });
            }
        }
        if resp.drag_started_by(egui::PointerButton::Primary)
            && lanes.contains(pp)
            && drag.kind.is_none()
        {
            let t = x_to_time(&v, lanes, pp.x).max(0.0);
            let handle = (st.tool == Tool::Move).then(|| handle_at(st, pp)).flatten();
            let locked = |st: &State, id: ClipId| {
                st.session
                    .find_clip(id)
                    .is_some_and(|(ti, ci)| st.session.tracks[ti].clips[ci].props.locked)
            };
            let kind = match (st.tool, hit(st, pp)) {
                _ if handle.is_some() => {
                    let (ti, id, h) = handle.unwrap();
                    st.selected_clip = Some(id);
                    st.selected_track = Some(st.session.tracks[ti].id);
                    let (cti, ci) = st.session.find_clip(id).unwrap();
                    let c = &st.session.tracks[cti].clips[ci];
                    match h {
                        Handle::Fade(fade_in) => Some(DragKind::Fade {
                            clip: id,
                            fade_in,
                            y0: pp.y,
                            skew0: if fade_in {
                                c.props.fade_in_skew
                            } else {
                                c.props.fade_out_skew
                            },
                        }),
                        Handle::Gain => Some(DragKind::Gain {
                            clip: id,
                            p0: pp,
                            db0: c.gain_db,
                        }),
                        Handle::Stretch(_) if c.props.locked => None,
                        Handle::Stretch(left) => Some(DragKind::Stretch { clip: id, left }),
                    }
                }
                // On a selection edge: drag that edge, the other stays.
                (Tool::Move | Tool::TimeSelect, _) if on_edge.is_some() => {
                    on_edge.map(|(_, other)| DragKind::Select { anchor: other })
                }
                (Tool::Move, Some((ti, id, _, _))) if locked(st, id) => {
                    st.selected_clip = Some(id);
                    st.selected_track = Some(st.session.tracks[ti].id);
                    st.toast("This clip's position is locked (right-click > Unlock Position).");
                    None
                }
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
                _ => Some(DragKind::Select {
                    anchor: snapped(st, pp.x),
                }),
            };
            if kind.is_some() {
                drag.before = Some(st.session.clone());
            }
            drag.kind = kind;
        }
        if resp.dragged_by(egui::PointerButton::Primary) {
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
                Some(DragKind::TrimLeft { clip }) | Some(DragKind::TrimRight { clip }) => {
                    let left = matches!(drag.kind, Some(DragKind::TrimLeft { .. }));
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
                        let ratio = c.props.stretch.applied_ratio();
                        let rev = c.props.reversed;
                        // Source room before the clip's audio and after it,
                        // in timeline seconds.
                        let before_room = c.offset * ratio;
                        let after_room = (media_len - c.offset - c.length / ratio).max(0.0) * ratio;
                        let (room_l, room_r) = if rev {
                            (after_room, before_room)
                        } else {
                            (before_room, after_room)
                        };
                        if left {
                            let end = c.end();
                            let new_start = t.clamp(c.start - room_l, end - 0.01).max(0.0);
                            let d = new_start - c.start;
                            c.start = new_start;
                            c.length -= d;
                            // The clip's lanes stay where they were in time.
                            c.shift_lanes(-d);
                            if !rev {
                                c.offset = (c.offset + d / ratio).max(0.0);
                            }
                        } else {
                            let new_len = (t - c.start).clamp(0.01, c.length + room_r);
                            let d = c.length - new_len;
                            c.length = new_len;
                            if rev {
                                c.offset = (c.offset + d / ratio).max(0.0);
                            }
                        }
                        c.fade_in = c.fade_in.min(c.length);
                        c.fade_out = c.fade_out.min(c.length - c.fade_in);
                        st.session_version += 1;
                    }
                }
                Some(DragKind::Fade {
                    clip,
                    fade_in,
                    y0,
                    skew0,
                }) => {
                    if let Some((ti, ci)) = st.session.find_clip(clip) {
                        let c = &mut st.session.tracks[ti].clips[ci];
                        let skew = (skew0 + (y0 - pp.y) * 1.0).clamp(-100.0, 100.0);
                        let skew = if skew.abs() < 4.0 { 0.0 } else { skew };
                        if fade_in {
                            c.fade_in = (t - c.start).clamp(0.0, c.length - c.fade_out);
                            c.props.fade_in_skew = skew;
                        } else {
                            c.fade_out = (c.end() - t).clamp(0.0, c.length - c.fade_in);
                            c.props.fade_out_skew = skew;
                        }
                        st.session_version += 1;
                    }
                }
                Some(DragKind::Gain { clip, p0, db0 }) => {
                    if let Some((ti, ci)) = st.session.find_clip(clip) {
                        let fine = ui.input(|i| i.modifiers.shift);
                        let px = (pp.x - p0.x) + (p0.y - pp.y);
                        let db = (db0 + px * if fine { 0.02 } else { 0.1 }).clamp(-96.0, 24.0);
                        st.session.tracks[ti].clips[ci].gain_db = (db * 10.0).round() / 10.0;
                        st.session_version += 1;
                    }
                }
                Some(DragKind::Stretch { clip, left }) => {
                    if let Some((ti, ci)) = st.session.find_clip(clip) {
                        let c = &mut st.session.tracks[ti].clips[ci];
                        let src = c.length / c.props.stretch.applied_ratio();
                        let end = c.end();
                        let new_len = if left { end - t } else { t - c.start };
                        let new_len = new_len.clamp(src * 0.25, src * 4.0);
                        if c.props.stretch.mode == af27_core::StretchMode::Off {
                            c.props.stretch.mode = af27_core::StretchMode::Live;
                        }
                        c.props.stretch.ratio = new_len / src;
                        if left {
                            c.start = (end - new_len).max(0.0);
                        }
                        // The clip's lanes stretch with it.
                        c.scale_lanes(new_len / c.length);
                        c.length = new_len;
                        c.fade_in = c.fade_in.min(c.length);
                        c.fade_out = c.fade_out.min(c.length - c.fade_in);
                        st.session_version += 1;
                    }
                }
                Some(DragKind::Select { anchor }) => {
                    let t = snapped(st, pp.x);
                    st.session_sel = Some((anchor.min(t), anchor.max(t)));
                    st.session_cursor = anchor.min(t);
                }
                Some(DragKind::StretchMidi { clip, left }) => {
                    if let Some((ti, ci)) = st.session.find_midi(clip) {
                        st.session.tracks[ti].midi[ci].stretch_edge(left, tempo.beats(t), &tempo);
                        st.session_version += 1;
                    }
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
                    DragKind::Fade { .. } => "Fade",
                    DragKind::Gain { .. } => "Clip gain",
                    DragKind::Stretch { .. } => "Stretch clip",
                    DragKind::StretchMidi { .. } => "Stretch MIDI clip",
                };
                st.commit_session(label, before);
            }
        }
    }
    if resp.clicked() {
        if let Some(pp) = resp.interact_pointer_pos() {
            // A click on a selection edge keeps the selection. While
            // playing, a click in the lanes leaves the playhead alone.
            if lanes.contains(pp) && on_edge.is_none() {
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
                        if !playing {
                            st.set_cursor(t);
                        }
                    }
                    _ => {
                        // A MIDI clip is selected by a click too (its
                        // Properties then show).
                        st.selected_clip = hit_midi(st, pp).map(|(_, id)| id);
                        st.session_sel = None;
                        if !playing {
                            st.set_cursor(t);
                        }
                        if let Some(ti) = track_at(st, lanes.top(), scroll, pp.y) {
                            st.selected_track = Some(st.session.tracks[ti].id);
                        }
                    }
                }
            }
        }
    }
    let hover_handle = resp
        .hover_pos()
        .filter(|_| st.tool == Tool::Move)
        .and_then(|hp| handle_at(st, hp));
    let hover_midi_stretch = resp.hover_pos().and_then(|hp| midi_stretch_at(st, hp));
    if let Some(hp) = resp.hover_pos() {
        let icon = match (st.tool, hit(st, hp)) {
            _ if hover_midi_stretch.is_some() => egui::CursorIcon::ResizeHorizontal,
            _ if hover_handle.is_some() => match hover_handle.unwrap().2 {
                Handle::Fade(_) => egui::CursorIcon::Move,
                Handle::Gain => egui::CursorIcon::ResizeVertical,
                Handle::Stretch(_) => egui::CursorIcon::ResizeHorizontal,
            },
            (Tool::Razor, _) => egui::CursorIcon::Crosshair,
            _ if on_edge.is_some() => egui::CursorIcon::ResizeHorizontal,
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
    // The clip-gain readout while it is dragged.
    if let Some(DragKind::Gain { clip, .. }) = drag.kind {
        if let (Some((ti, ci)), Some(hp)) =
            (st.session.find_clip(clip), resp.interact_pointer_pos())
        {
            let db = st.session.tracks[ti].clips[ci].gain_db;
            egui::show_tooltip_at(
                ui.ctx(),
                ui.layer_id(),
                ui.id().with("af2-gain-tip"),
                hp + vec2(14.0, -18.0),
                |ui| {
                    ui.label(format!("{db:+.1} dB"));
                },
            );
        }
    }
    let menu_at = drag.menu;
    resp.context_menu(|ui| crate::lane_menu::show(ui, st, menu_at));

    // Drawing.
    let tc = st.time_ctx();
    crate::ruler::draw_ruler(&p, ruler, &v, &th, &st.session.markers, &tc);
    crate::overview::draw(&p, st, overview, full_btn, &th);
    if st.session.tempo.metronome
        || st.session.tempo.has_changes()
        || st
            .session
            .tracks
            .iter()
            .any(|t| t.kind == af27_core::TrackKind::Midi)
    {
        crate::tempo::draw_grid(
            &ui.painter_at(ruler),
            Some(ruler),
            ruler,
            &tempo,
            &|t| time_to_x(&v, ruler, t),
            &|x| x_to_time(&v, ruler, x),
            &th,
        );
    }
    crate::tempo::draw_markers(
        &ui.painter_at(ruler),
        ruler,
        &tempo,
        &|t| time_to_x(&v, ruler, t),
        &th,
        st.tempo_ui.hot,
    );
    let corner = Rect::from_min_max(rect.min, pos2(ruler.left(), ruler.bottom()));
    p.rect_filled(corner, Rounding::ZERO, c(th.panel));
    p.line_segment(
        [corner.left_bottom(), corner.right_bottom()],
        Stroke::new(1.0_f32, c(th.border)),
    );
    crate::track_views::strip(ui, st, corner);
    if metro_h > 0.0 {
        let mlane = Rect::from_min_max(
            pos2(lanes.left(), metro.top()),
            pos2(lanes.right(), metro.bottom()),
        );
        crate::track_views::metronome_lane(
            &ui.painter_at(mlane),
            st,
            mlane,
            &|t| time_to_x(&v, lanes, t),
            &|x| x_to_time(&v, lanes, x),
        );
        let mhead = Rect::from_min_max(metro.min, pos2(lanes.left(), metro.bottom()));
        p.rect_filled(mhead, Rounding::ZERO, c(th.panel));
        p.rect_filled(
            Rect::from_min_size(mhead.min, vec2(4.0, mhead.height())),
            Rounding::ZERO,
            c(th.accent),
        );
        p.line_segment(
            [mhead.left_bottom(), mhead.right_bottom()],
            Stroke::new(1.0_f32, c(th.border)),
        );
        crate::track_views::metronome_header(ui, st, mhead.shrink2(vec2(8.0, 4.0)));
        let mr = ui.interact(mlane, ui.id().with("af2-metro-lane"), Sense::click());
        mr.context_menu(|ui| {
            let mut m = st.prefs.metronome;
            let mut ch = false;
            crate::track_views::metronome_menu(ui, st, &mut m, &mut ch);
            if ch {
                st.prefs.metronome = m;
                st.prefs.save();
                st.live_metronome();
            }
        });
    }
    let lane_p = ui.painter_at(lanes);
    let head_p = ui.painter_at(headers);
    // Bars and beats under the clips, from the tempo map, when there is
    // music to line up with.
    let show_grid = st.session.tempo.metronome
        || st.session.tempo.has_changes()
        || st
            .session
            .tracks
            .iter()
            .any(|t| t.kind == af27_core::TrackKind::Midi);
    let mut top = lanes.top() - scroll;
    let heard_all = st.session.heard();
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
        if show_grid {
            crate::tempo::draw_grid(
                &lane_p,
                None,
                row,
                &tempo,
                &|t| time_to_x(&v, lanes, t),
                &|x| x_to_time(&v, lanes, x),
                &th,
            );
        }
        let heard = heard_all.get(ti).copied().unwrap_or(true);
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
            let ccol = clip.props.color.unwrap_or(tr.color);
            lane_p.rect_filled(cr, rnd, ca(ccol, (alpha as f32 * 0.16) as u8));
            let title = Rect::from_min_size(cr.min, vec2(cr.width(), 15.0));
            lane_p.rect_filled(
                title,
                Rounding {
                    nw: rnd.nw,
                    ne: rnd.ne,
                    sw: 0.0,
                    se: 0.0,
                },
                ca(ccol, (alpha as f32 * 0.55) as u8),
            );
            let mut badges = String::new();
            if clip.props.locked {
                badges.push_str("LOCK ");
            }
            if clip.props.reversed {
                badges.push_str("REV ");
            }
            if clip.props.inverted {
                badges.push_str("INV ");
            }
            if !clip.props.chain.is_empty() {
                badges.push_str("FX ");
            }
            if clip.props.channel_map.is_some() {
                badges.push_str("CH ");
            }
            let s = &clip.props.stretch;
            if s.is_on() {
                if (s.applied_ratio() - 1.0).abs() > 1e-6 {
                    badges.push_str(&format!("{:.0}% ", s.applied_ratio() * 100.0));
                }
                if s.applied_pitch().abs() > 1e-3 {
                    badges.push_str(&format!("{:+.1}st ", s.applied_pitch()));
                }
            }
            lane_p.text(
                title.left_center() + vec2(if stretch_on { 13.0 } else { 5.0 }, 0.0),
                Align2::LEFT_CENTER,
                format!("{badges}{}", clip.name),
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
                    let ratio = clip.props.stretch.applied_ratio();
                    let src = View {
                        start: clip.offset + (t0 - clip.start) / ratio,
                        len: ((t1 - t0) / ratio).max(1e-6),
                    };
                    let wave =
                        Rect::from_min_max(pos2(vis0, title.bottom()), pos2(vis1, cr.bottom()));
                    let n = buf.num_channels().min(2);
                    let h = wave.height() / n as f32;
                    let tint = if alpha == 255 { ccol } else { [90, 90, 100] };
                    // Drawn at the clip's gain and along its volume line,
                    // as it plays (+6 dB twice as tall, -12 dB a quarter).
                    let level = af27_core::time::db_to_gain(clip.gain_db);
                    let vol = clip
                        .playing_lane(&af27_core::automation::LaneTarget::Volume)
                        .map(|l| l.points.as_slice());
                    let gain = |x: f32| -> f32 {
                        match vol {
                            Some(pts) => {
                                let t = x_to_time(&v, lanes, x) - clip.start;
                                let db = af27_core::automation::value_at(pts, t).unwrap_or(0.0);
                                level * af27_core::time::db_to_gain(db as f32)
                            }
                            None => level,
                        }
                    };
                    for ch in 0..n {
                        let lane = Rect::from_min_size(
                            pos2(wave.left(), wave.top() + h * ch as f32),
                            vec2(wave.width(), h),
                        );
                        draw_channel_gained(
                            &lane_p,
                            lane,
                            &buf.channels[ch],
                            &peaks,
                            ch,
                            buf.rate,
                            &src,
                            &th,
                            Some(tint),
                            st.session_amp,
                            Some(&gain),
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
            // Fades, drawn from the clip's own gain curve (so the shape is
            // what plays).
            let body_top = cr.top() + 15.0;
            let unity = af27_core::time::db_to_gain(clip.gain_db).max(1e-9);
            let mut quiet = clip.clone();
            quiet.muted = false;
            for (on, a, b) in [
                (clip.fade_in > 0.0, clip.start, clip.start + clip.fade_in),
                (clip.fade_out > 0.0, clip.end() - clip.fade_out, clip.end()),
            ] {
                if !on {
                    continue;
                }
                let n = 24;
                let pts: Vec<egui::Pos2> = (0..=n)
                    .map(|k| {
                        let t = a + (b - a) * k as f64 / n as f64;
                        let g = (quiet.gain_at(t.clamp(clip.start, clip.end() - 1e-9)) / unity)
                            .clamp(0.0, 1.0);
                        pos2(
                            time_to_x(&v, lanes, t),
                            cr.bottom() - (cr.bottom() - body_top) * g,
                        )
                    })
                    .collect();
                let mut shade = pts.clone();
                if a == clip.start {
                    shade.push(pos2(time_to_x(&v, lanes, b), body_top));
                    shade.push(pos2(time_to_x(&v, lanes, a), body_top));
                } else {
                    shade.push(pos2(time_to_x(&v, lanes, b), body_top));
                    shade.insert(0, pos2(time_to_x(&v, lanes, a), body_top));
                }
                lane_p.add(egui::Shape::Path(egui::epaint::PathShape {
                    points: shade,
                    closed: true,
                    fill: Color32::from_black_alpha(60),
                    stroke: egui::epaint::PathStroke::NONE,
                }));
                lane_p.add(egui::Shape::line(pts, Stroke::new(1.5_f32, c(th.text))));
            }
            let stroke = if sel {
                Stroke::new(2.0_f32, c(th.accent))
            } else {
                Stroke::new(1.0_f32, ca(ccol, 200))
            };
            lane_p.rect_stroke(cr, rnd, stroke);
            if sel {
                th.glow_rect(&lane_p, cr, th.accent, 0.8);
            }
            // Handles: shown on the selected clip and the one under the
            // pointer, on clips big enough for them.
            let hovered = resp
                .hover_pos()
                .is_some_and(|hp| cr.expand(4.0).contains(hp));
            if (sel || hovered) && cr.width() >= 24.0 && cr.height() >= 34.0 {
                let hot = hover_handle.filter(|h| h.1 == clip.id).map(|h| h.2);
                let hcol = |h: Handle| {
                    if hot == Some(h) {
                        c(th.accent)
                    } else {
                        c(th.text)
                    }
                };
                for (fin, x) in [
                    (true, time_to_x(&v, lanes, clip.start + clip.fade_in)),
                    (false, time_to_x(&v, lanes, clip.end() - clip.fade_out)),
                ] {
                    let r = fade_handle(cr, x);
                    lane_p.rect_filled(r, Rounding::same(1.0), ca(th.background, 200));
                    lane_p.rect_stroke(
                        r,
                        Rounding::same(1.0),
                        Stroke::new(1.2_f32, hcol(Handle::Fade(fin))),
                    );
                }
                let g = gain_handle(cr);
                lane_p.rect_filled(g, Rounding::same(2.0), ca(th.background, 190));
                lane_p.rect_stroke(
                    g,
                    Rounding::same(2.0),
                    Stroke::new(1.0_f32, hcol(Handle::Gain)),
                );
                for k in 0..4 {
                    let bx = g.left() + 3.0 + k as f32 * 3.0;
                    let bh = 2.0 + k as f32 * 2.0;
                    lane_p.line_segment(
                        [pos2(bx, g.bottom() - 2.0), pos2(bx, g.bottom() - 2.0 - bh)],
                        Stroke::new(1.5_f32, hcol(Handle::Gain)),
                    );
                }
                // Trim grips on the edges.
                for x in [cr.left() + 1.5, cr.right() - 1.5] {
                    lane_p.line_segment(
                        [pos2(x, cr.center().y - 8.0), pos2(x, cr.center().y + 8.0)],
                        Stroke::new(2.0_f32, ca(th.text, 150)),
                    );
                }
            }
            // The clip's gain beside its handle, always (clips big
            // enough for the handle and the text).
            if cr.width() >= 56.0 && cr.height() >= 34.0 {
                let g = gain_handle(cr);
                let hot = hover_handle.is_some_and(|h| h.1 == clip.id && h.2 == Handle::Gain)
                    || matches!(caches.drag.kind, Some(DragKind::Gain { clip: id, .. }) if id == clip.id);
                let galley = lane_p.layout_no_wrap(
                    gain_text(clip.gain_db),
                    FontId::monospace(9.5),
                    if hot { c(th.accent) } else { c(th.text) },
                );
                let at = pos2(g.right() + 3.0, g.center().y - galley.size().y / 2.0);
                let back = Rect::from_min_size(at, galley.size()).expand2(vec2(2.0, 0.5));
                if back.right() < cr.right() - 2.0 {
                    lane_p.rect_filled(back, Rounding::same(2.0), ca(th.background, 170));
                    lane_p.galley(at, galley, c(th.text));
                }
            }
            if stretch_on && cr.width() >= 24.0 && cr.height() >= 34.0 {
                let hot = hover_handle.filter(|h| h.1 == clip.id).map(|h| h.2);
                for left in [true, false] {
                    let r = stretch_handle(cr, left);
                    let pts = if left {
                        vec![r.left_top(), r.right_top(), r.left_bottom()]
                    } else {
                        vec![r.left_top(), r.right_top(), r.right_bottom()]
                    };
                    let col = if hot == Some(Handle::Stretch(left)) {
                        c(th.accent)
                    } else {
                        ca(th.background, 230)
                    };
                    lane_p.add(egui::Shape::convex_polygon(
                        pts,
                        col,
                        Stroke::new(1.0_f32, ca(th.text, 200)),
                    ));
                }
            }
        }
        for mc in &tr.midi {
            let x0 = time_to_x(&v, lanes, tempo.secs(mc.start));
            let x1 = time_to_x(&v, lanes, mc.end_secs(&tempo));
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
            // Kept speed, stretch and pitch, like an audio clip's badges.
            let badge = mc.badge();
            if !badge.is_empty() && cr.width() > 90.0 {
                lane_p.text(
                    title.right_center() - vec2(5.0, 0.0),
                    Align2::RIGHT_CENTER,
                    badge,
                    FontId::proportional(9.0),
                    c(th.background),
                );
            }
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
                // Through the clip's timing and stretch, so notes squeeze
                // and spread across tempo changes and glides as they play.
                for n in mc.notes.iter().filter(|n| n.start < mc.length) {
                    let nx0 = time_to_x(&v, lanes, mc.secs_at(n.start, &tempo));
                    let nx1 = time_to_x(
                        &v,
                        lanes,
                        mc.secs_at((n.start + n.length).min(mc.length), &tempo),
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
            if stretch_on && cr.width() >= 24.0 && cr.height() >= 34.0 {
                let hot = hover_midi_stretch.filter(|h| h.1 == mc.id).map(|h| h.2);
                for left in [true, false] {
                    let r = stretch_handle(cr, left);
                    let pts = if left {
                        vec![r.left_top(), r.right_top(), r.left_bottom()]
                    } else {
                        vec![r.left_top(), r.right_top(), r.right_bottom()]
                    };
                    let col = if hot == Some(left) {
                        c(th.accent)
                    } else {
                        ca(th.background, 230)
                    };
                    lane_p.add(egui::Shape::convex_polygon(
                        pts,
                        col,
                        Stroke::new(1.0_f32, ca(th.text, 200)),
                    ));
                }
            }
        }
        // Each clip's own automation over it (the track's lane, when
        // shown, covers the row instead).
        if st.shown_lane(ti).is_none() {
            // The lines leave the pointer to the clip's handles and edges,
            // to other tools and to drags already under way.
            let hp = ui.input(|i| i.pointer.hover_pos());
            for clip in &tr.clips {
                let free = st.tool == Tool::Move
                    && caches.drag.kind.is_none()
                    && on_edge.is_none()
                    && hp.is_some_and(|hp| {
                        handle_at(st, hp).is_none()
                            && !hit(st, hp).is_some_and(|h| h.1 == clip.id && (h.2 || h.3))
                    });
                crate::clip_lanes::overlay(ui, st, clip.id, row, &v, lanes, free);
            }
        }
        // The automation lane shown over the clips.
        crate::mixing::lane_overlay(ui, st, ti, row, &v, lanes);
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
        // The colour tab: bright when selected; drag it to move the track.
        let tab = Rect::from_min_size(head.min, vec2(8.0, head.height()));
        head_p.rect_filled(
            tab,
            Rounding::ZERO,
            if selected {
                c(tr.color)
            } else {
                lerp(th.panel, tr.color, 0.55)
            },
        );
        let tab_r = ui.interact(
            tab,
            ui.id().with(("track-tab", tr.id)),
            Sense::click_and_drag(),
        );
        if tab_r.hovered() || tab_r.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if tab_r.clicked() {
            st.selected_track = Some(tr.id);
        }
        if tab_r.drag_started() {
            caches.drag.track_move = Some(tr.id);
        }
        head_p.line_segment(
            [head.left_bottom(), head.right_bottom()],
            Stroke::new(1.0_f32, c(th.border)),
        );
        // The level meter down the header's right edge.
        let meter_r = Rect::from_min_max(
            pos2(head.right() - 11.0, head.top() + 4.0),
            pos2(head.right() - 4.0, head.bottom() - 4.0),
        );
        let lvl = st.track_levels.get(ti).map_or(0.0, |l| l.1)
            * af27_core::time::db_to_gain(tr.volume_db);
        crate::track_views::header_meter(&head_p, meter_r, lvl, &th);
        let inner = Rect::from_min_max(
            pos2(head.left() + 12.0, head.top() + 4.0),
            pos2(head.right() - 15.0, head.bottom() - 4.0),
        );
        crate::track_views::header(ui, st, ti, inner);
    }
    // A track moved by its colour tab: a line where it would land.
    if let Some(moving) = caches.drag.track_move {
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let down = ui.input(|i| i.pointer.primary_down());
        if let Some(pp) = pointer {
            // The gap nearest the pointer, 0 (above all) ..= n (below all).
            let mut y = lanes.top() - scroll;
            let mut gap = 0;
            let mut gap_y = y;
            for (k, t) in st.session.tracks.iter().enumerate() {
                if pp.y > y + t.height / 2.0 {
                    gap = k + 1;
                    gap_y = y + t.height;
                } else if k == gap {
                    gap_y = y;
                }
                y += t.height;
            }
            p.line_segment(
                [pos2(headers.left(), gap_y), pos2(lanes.right(), gap_y)],
                Stroke::new(2.0_f32, c(th.accent)),
            );
            if !down {
                if let Some(from) = st.session.track_index(moving) {
                    let to = if gap > from { gap - 1 } else { gap };
                    if to != from {
                        let before = st.session.clone();
                        st.session.move_track(moving, to);
                        st.commit_session("Move track", before);
                    }
                }
                caches.drag.track_move = None;
            }
        } else if !down {
            caches.drag.track_move = None;
        }
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
    // The Master row, pinned to the bottom of the frame.
    {
        let master_row =
            Rect::from_min_max(master_band.min, pos2(headers.right(), master_band.bottom()));
        let mlane = Rect::from_min_max(pos2(lanes.left(), master_band.top()), master_band.max);
        p.rect_filled(
            mlane,
            Rounding::ZERO,
            lerp(th.wave_background, th.panel, 0.4),
        );
        p.rect_filled(master_row, Rounding::ZERO, lerp(th.panel, th.accent, 0.06));
        p.rect_filled(
            Rect::from_min_size(master_row.min, vec2(4.0, master_row.height())),
            Rounding::ZERO,
            c(th.text_dim),
        );
        p.line_segment(
            [master_band.left_top(), master_band.right_top()],
            Stroke::new(1.0_f32, c(th.border)),
        );
        crate::track_views::master_header(ui, st, master_row.shrink2(vec2(8.0, 4.0)));
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
    if let Some(s) = st.start_marker {
        let x = time_to_x(&v, lanes, s);
        if x >= lanes.left() && x <= lanes.right() {
            crate::ruler::start_marker(&p, x, ruler, lanes.bottom(), &th);
        }
    }
    if let Some(g) = ghost {
        let x = time_to_x(&v, lanes, g);
        if x >= lanes.left() && x <= lanes.right() {
            crate::ruler::ghost(&p, x, ruler.top(), lanes.bottom(), &th);
        }
    }
    let x = time_to_x(&v, lanes, st.position());
    if x >= lanes.left() && x <= lanes.right() {
        playhead(&p, x, ruler.top(), lanes.bottom(), &th);
    }
    draw_vbar(&p, st, drag_scroll(caches), lanes, vbar, &th);
    if st
        .session
        .tracks
        .iter()
        .all(|t| t.clips.is_empty() && t.midi.is_empty())
    {
        p.text(
            lanes.center(),
            Align2::CENTER_CENTER,
            "Drag files here from the Files panel or the file manager, or open a .sesx / .audfrm / .af27 session",
            FontId::proportional(14.0),
            c(th.text_dim),
        );
    }
    file_drop(ui, st, &resp, &p, lanes, scroll, &v, &snap_marks);
}

/// No session open (all were closed, or none made yet): say so, offer New
/// and Open, and start a session for files dropped here.
fn no_session(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, Sense::hover());
    ui.painter()
        .rect_filled(rect, Rounding::ZERO, c(th.wave_background));
    // Dropped here: a new session takes them, one track each.
    let alt = ui.input(|i| i.modifiers.alt);
    if let Some(files) = resp.dnd_release_payload::<crate::files_panel::DragFiles>() {
        st.new_session(None);
        st.import_docs(
            files.0.clone(),
            crate::sessions::Layout::Drop {
                track: None,
                at: 0.0,
                alt,
            },
        );
        return;
    }
    let os = st
        .sessions
        .os_drop
        .as_ref()
        .and_then(|d| d.pos)
        .is_some_and(|pp| rect.contains(pp));
    if os {
        if let Some(d) = st.sessions.os_drop.take() {
            st.new_session(None);
            st.import_paths(
                d.paths,
                crate::sessions::Layout::Drop {
                    track: None,
                    at: 0.0,
                    alt: d.alt,
                },
            );
            return;
        }
    }
    let hovering =
        egui::DragAndDrop::has_payload_of_type::<crate::files_panel::DragFiles>(ui.ctx())
            && resp.contains_pointer();
    if hovering {
        ui.painter().rect_stroke(
            rect.shrink(6.0),
            Rounding::same(6.0),
            Stroke::new(2.0_f32, c(th.accent)),
        );
    }
    let col = Rect::from_center_size(rect.center(), vec2(360.0, 150.0));
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(col), |ui| {
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("No session open")
                    .size(18.0)
                    .color(c(th.text)),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Make a new session, open one, or drop audio here to start a session with it.",
                )
                .color(c(th.text_dim)),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(70.0);
                if ui.button("New Multitrack Session...").clicked() {
                    crate::new_dialogs::open_new_session(st, None);
                }
                if ui.button("Open...").clicked() {
                    crate::ui::open_dialog(st);
                }
            });
        });
    });
}

/// Files dragged onto the lanes, from the Files panel or the file manager.
/// Several go one per track from the drop time (the track under the
/// pointer, then the audio tracks below it, then new tracks); with Alt
/// held they go one after another on one track. While they hover, a box
/// shows where each would land, following Alt as it goes down and up.
#[allow(clippy::too_many_arguments)]
fn file_drop(
    ui: &egui::Ui,
    st: &mut State,
    resp: &egui::Response,
    p: &egui::Painter,
    lanes: Rect,
    scroll: f32,
    v: &View,
    snap_marks: &[f64],
) {
    use crate::files_panel::DragFiles;
    use crate::sessions::{layout_drop, Lane, Layout};
    let land = |st: &State, pos: egui::Pos2| {
        let track = track_at(st, lanes.top(), scroll, pos.y).map(|i| st.session.tracks[i].id);
        let t = x_to_time(v, lanes, pos.x).max(0.0);
        let t = crate::ruler::snap(st, t, v, lanes, snap_marks).max(0.0);
        let px_per_sec = lanes.width() as f64 / v.len.max(1e-9);
        let at = crate::sessions::snap_drop_start(t, px_per_sec, crate::ruler::EDGE_PX as f64);
        (track, at)
    };

    // Let go of Files panel rows.
    if let Some(files) = resp.dnd_release_payload::<DragFiles>() {
        if let Some(pos) = ui
            .input(|i| i.pointer.interact_pos())
            .filter(|pp| lanes.contains(*pp))
        {
            let alt = ui.input(|i| i.modifiers.alt);
            let (track, at) = land(st, pos);
            st.import_docs(files.0.clone(), Layout::Drop { track, at, alt });
        }
        return;
    }
    // Dropped from the file manager this frame.
    let on_lanes = st
        .sessions
        .os_drop
        .as_ref()
        .and_then(|d| d.pos)
        .is_some_and(|pp| lanes.contains(pp));
    if on_lanes {
        if let Some(d) = st.sessions.os_drop.take() {
            let (track, at) = land(st, d.pos.unwrap_or_default());
            st.import_paths(
                d.paths,
                Layout::Drop {
                    track,
                    at,
                    alt: d.alt,
                },
            );
        }
        return;
    }

    // Hovering: what is coming, where the pointer is, and Alt.
    let th = st.theme.clone();
    let ppp = ui.ctx().pixels_per_point();
    let modifiers_alt = ui.input(|i| i.modifiers.alt);
    // (name, length when known) for each file coming in.
    type Items = Vec<(String, Option<f64>)>;
    let (items, pos, alt): (Items, Option<egui::Pos2>, bool) = if let Some(f) =
        egui::DragAndDrop::payload::<DragFiles>(ui.ctx())
    {
        let items =
            f.0.iter()
                .filter_map(|d| st.doc_by_id(*d))
                .map(|d| (d.name.clone(), Some(d.duration())))
                .collect();
        (items, ui.input(|i| i.pointer.hover_pos()), modifiers_alt)
    } else {
        let hovered: Vec<std::path::PathBuf> = ui.input(|i| {
            i.raw
                .hovered_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if hovered.is_empty() {
            return;
        }
        // Loose files only: folders are searched once they land.
        let loose: Vec<std::path::PathBuf> = hovered.into_iter().filter(|p| !p.is_dir()).collect();
        let (audio, _) = crate::sessions::expand_paths(&loose);
        let items = audio
            .iter()
            .map(|p| {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let len = st
                    .doc_for_path(p)
                    .and_then(|d| st.doc_by_id(d))
                    .map(|d| d.duration());
                (name, len)
            })
            .collect();
        let pos = crate::hwcursor::system_pointer()
            .map(|(x, y)| egui::pos2(x / ppp, y / ppp))
            .or(ui.input(|i| i.pointer.hover_pos()));
        (items, pos, modifiers_alt || crate::hwcursor::alt_down())
    };
    let Some(pos) = pos.filter(|pp| lanes.contains(*pp)) else {
        return;
    };
    if items.is_empty() {
        return;
    }
    let (track, at) = land(st, pos);
    // Files not read yet show at a nominal width.
    let unknown = v.len * 120.0 / lanes.width().max(1.0) as f64;
    let lengths: Vec<f64> = items.iter().map(|(_, l)| l.unwrap_or(unknown)).collect();
    let audio: Vec<bool> = st
        .session
        .tracks
        .iter()
        .map(|t| t.kind == af27_core::TrackKind::Audio)
        .collect();
    let target = track.and_then(|id| st.session.track_index(id));
    let placements = layout_drop(&lengths, &audio, target, at, alt);
    let mut tops = Vec::with_capacity(st.session.tracks.len());
    let mut y = lanes.top() - scroll;
    for t in &st.session.tracks {
        tops.push(y);
        y += t.height;
    }
    let new_h = st.session.tracks.last().map_or(80.0, |t| t.height);
    let lp = p.with_clip_rect(lanes);
    for (((name, known), len), pl) in items.iter().zip(&lengths).zip(&placements) {
        let (y0, h) = match pl.lane {
            Lane::Track(i) => (tops[i], st.session.tracks[i].height),
            Lane::New(k) => (y + k as f32 * new_h, new_h),
        };
        let x0 = time_to_x(v, lanes, pl.start);
        let x1 = time_to_x(v, lanes, pl.start + len).max(x0 + 24.0);
        let r = Rect::from_min_max(pos2(x0, y0 + 3.0), pos2(x1, y0 + h - 3.0));
        lp.rect_filled(r, Rounding::same(4.0), ca(th.accent, 46));
        lp.rect_stroke(r, Rounding::same(4.0), Stroke::new(1.5_f32, c(th.accent)));
        let label = match known {
            Some(l) => format!("{name}  {}", af27_core::time::format_time(*l)),
            None => name.clone(),
        };
        lp.with_clip_rect(r.intersect(lanes)).text(
            r.left_top() + vec2(6.0, 4.0),
            Align2::LEFT_TOP,
            label,
            FontId::proportional(12.0),
            c(th.text),
        );
    }
    if items.len() > 1 {
        let hint = if alt {
            "One after another on one track (Alt)"
        } else {
            "One per track from here; hold Alt for one after another"
        };
        lp.text(
            pos + vec2(14.0, -18.0),
            Align2::LEFT_BOTTOM,
            hint,
            FontId::proportional(12.0),
            c(th.accent),
        );
    }
    ui.ctx().request_repaint();
}

fn drag_scroll(caches: &Caches) -> f32 {
    caches.drag.scroll_y
}

/// The Multitrack's parts, for the wheel.
struct Areas {
    overview: Rect,
    ruler: Rect,
    lanes: Rect,
    headers: Rect,
    vbar: Rect,
}

/// The mouse wheel over the Multitrack: the table in
/// [`crate::zoom::wheel_action`] picks what it does.
fn wheel(st: &mut State, drag: &mut Drag, a: &Areas, hp: egui::Pos2, w: crate::zoom::WheelInput) {
    use crate::zoom::{Anchor, Region, WheelAction};
    let region = if a.overview.contains(hp) {
        Region::Overview
    } else if a.ruler.contains(hp) {
        Region::Ruler
    } else if a.vbar.contains(hp) {
        Region::TrackBar
    } else if a.lanes.contains(hp) {
        Region::Lanes
    } else if a.headers.contains(hp) {
        Region::Headers
    } else {
        return;
    };
    let notches = w.notches();
    let lim = st.session_limits();
    // The spot the track height changes keep in place: the pointer's
    // place in the rows (read through the bar over the track bar).
    let rows_y = |st: &State, scroll: f32| -> f32 {
        if region == Region::TrackBar {
            let (_, _, content) = vbar_thumb(st, scroll, a.lanes, a.vbar);
            let share = ((hp.y - a.vbar.top()) / a.vbar.height().max(1.0)).clamp(0.0, 1.0);
            (share * content - scroll).clamp(0.0, a.lanes.height())
        } else {
            (hp.y - a.lanes.top()).clamp(0.0, a.lanes.height())
        }
    };
    match crate::zoom::wheel_action(region, w.mods, w.sideways, true) {
        WheelAction::ZoomTime(anchor) => {
            let v = st.session_view;
            let pointer_t = if region == Region::Overview {
                crate::overview::time_at(st, a.overview, hp.x).clamp(v.start, v.start + v.len)
            } else {
                x_to_time(&v, a.lanes, hp.x)
            };
            let t = match anchor {
                Anchor::Pointer => pointer_t,
                Anchor::Setting => {
                    crate::zoom::anchor_time(st.zoom.wheel_anchor, pointer_t, &v, st.position())
                }
                Anchor::Center => v.start + v.len / 2.0,
            };
            st.session_view
                .zoom_at(st.zoom.wheel_factor(w.points), t, &lim);
            st.hand_moved_view();
        }
        WheelAction::ScrollTime => {
            st.session_view
                .pan_px(-w.scroll[0] as f64, a.lanes.width() as f64, &lim);
            st.hand_moved_view();
        }
        WheelAction::ScrollTracks => drag.scroll_y -= w.scroll[1],
        WheelAction::ZoomTracksEven | WheelAction::ScaleAllTracks | WheelAction::ResizeOneTrack => {
            let old: Vec<f32> = st.session.tracks.iter().map(|t| t.height).collect();
            let Some(&first) = old.first() else { return };
            let new: Vec<f32> = match crate::zoom::wheel_action(region, w.mods, w.sideways, true) {
                WheelAction::ZoomTracksEven => {
                    // Every track takes the first one's height, stepped.
                    vec![crate::zoom::wheel_height(first, notches); old.len()]
                }
                WheelAction::ScaleAllTracks => old
                    .iter()
                    .map(|&h| crate::zoom::wheel_height(h, notches))
                    .collect(),
                _ => {
                    let Some(ti) = track_at(st, a.lanes.top(), drag.scroll_y, hp.y) else {
                        return;
                    };
                    let mut n = old.clone();
                    n[ti] = crate::zoom::wheel_height(old[ti], notches);
                    n
                }
            };
            if new == old {
                return;
            }
            let y = rows_y(st, drag.scroll_y);
            st.touch_track_heights();
            drag.scroll_y = crate::zoom::scroll_keeping_rows(&old, &new, drag.scroll_y, y);
            for (t, h) in st.session.tracks.iter_mut().zip(new) {
                t.height = h;
            }
        }
        WheelAction::ZoomAmp => {
            st.session_amp = crate::zoom::wheel_amp(st.session_amp, notches);
        }
        WheelAction::ZoomFreq | WheelAction::None => {}
    }
}

/// The thumb of the vertical zoom bar: (top, bottom) in screen y, and the
/// session's full height (tracks plus the tail row).
fn vbar_thumb(st: &State, scroll: f32, lanes: Rect, vbar: Rect) -> (f32, f32, f32) {
    let content = st.session.tracks.iter().map(|t| t.height).sum::<f32>() + TAIL_H;
    let content = content.max(lanes.height());
    let h = vbar.height();
    let top = vbar.top() + scroll / content * h;
    let bottom = (top + lanes.height() / content * h).min(vbar.bottom());
    (top, bottom, content)
}

/// Drags and double-clicks on the vertical zoom bar.
fn vzoom_bar(
    ui: &egui::Ui,
    st: &mut State,
    drag: &mut Drag,
    resp: &egui::Response,
    lanes: Rect,
    vbar: Rect,
) {
    let n = st.session.tracks.len();
    if n == 0 {
        return;
    }
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    if resp.double_clicked() {
        // Fit every track in view at one height.
        let before = st.session.clone();
        let h = crate::zoom::fit_height(lanes.height(), TAIL_H, n);
        for t in &mut st.session.tracks {
            t.height = h;
        }
        drag.scroll_y = 0.0;
        if before != st.session {
            st.commit_session("Fit tracks", before);
        }
        return;
    }
    let (top, bottom, content) = vbar_thumb(st, drag.scroll_y, lanes, vbar);
    if resp.drag_started() {
        // Where the press began: by the time a drag is seen the pointer has
        // already moved off a narrow end.
        let origin = ui.input(|i| i.pointer.press_origin());
        if let Some(p) = origin.or(resp.interact_pointer_pos()) {
            let grip = ((bottom - top) * 0.2).clamp(8.0, 16.0);
            drag.vgrab = Some(if (p.y - top).abs() < grip {
                VGrab::Top
            } else if (p.y - bottom).abs() < grip {
                VGrab::Bottom
            } else if p.y > top && p.y < bottom {
                VGrab::Body(p.y - top)
            } else {
                // Off the thumb: jump there, then drag it.
                let half = (bottom - top) / 2.0;
                drag.scroll_y = ((p.y - half - vbar.top()) / vbar.height() * content).max(0.0);
                VGrab::Body(half)
            });
            if !matches!(drag.vgrab, Some(VGrab::Body(_))) {
                drag.before = Some(st.session.clone());
            }
        }
    }
    if let (Some(g), Some(p)) = (drag.vgrab, resp.interact_pointer_pos()) {
        if resp.dragged() {
            let bh = vbar.height().max(1.0);
            match g {
                VGrab::Body(off) => {
                    drag.scroll_y = ((p.y - off - vbar.top()) / bh * content).max(0.0);
                }
                VGrab::Top | VGrab::Bottom => {
                    // Keep the other end in place; the thumb's share sets
                    // every track's height.
                    let (t, b) = match g {
                        VGrab::Top => (p.y.clamp(vbar.top(), bottom - 8.0), bottom),
                        _ => (top, p.y.clamp(top + 8.0, vbar.bottom())),
                    };
                    let share = (b - t) / bh;
                    let h = height_for_share(share, lanes.height(), n);
                    for tr in &mut st.session.tracks {
                        tr.height = h;
                    }
                    let new_content = (h * n as f32 + TAIL_H).max(lanes.height());
                    drag.scroll_y = match g {
                        VGrab::Top => (b - vbar.top()) / bh * new_content - lanes.height(),
                        _ => (t - vbar.top()) / bh * new_content,
                    }
                    .max(0.0);
                }
            }
        }
    }
    if resp.drag_stopped() {
        drag.vgrab = None;
        if let Some(before) = drag.before.take() {
            if before != st.session {
                st.commit_session("Track height", before);
            }
        }
    }
}

fn draw_vbar(
    p: &egui::Painter,
    st: &State,
    scroll: f32,
    lanes: Rect,
    vbar: Rect,
    th: &crate::theme::Theme,
) {
    p.rect_filled(vbar, Rounding::ZERO, c(th.panel));
    let (top, bottom, content) = vbar_thumb(st, scroll, lanes, vbar);
    // A coloured segment per track, sized by its share of the rows.
    let k = vbar.height() / content.max(1.0);
    let mut y = vbar.top();
    for t in &st.session.tracks {
        let h = t.height * k;
        if h >= 1.0 {
            p.rect_filled(
                Rect::from_min_max(
                    pos2(vbar.left() + 4.0, y + 0.5),
                    pos2(vbar.right() - 4.0, y + h - 0.5),
                ),
                Rounding::ZERO,
                ca(t.color, 110),
            );
        }
        y += h;
    }
    let thumb = Rect::from_min_max(
        pos2(vbar.left() + 2.0, top.max(vbar.top())),
        pos2(vbar.right() - 2.0, bottom.max(top + 8.0)),
    );
    p.rect_filled(thumb, Rounding::same(3.0), ca(th.accent, 70));
    p.rect_stroke(
        thumb,
        Rounding::same(3.0),
        Stroke::new(1.0_f32, c(th.accent)),
    );
    // Grips on both ends.
    for y in [thumb.top() + 3.0, thumb.bottom() - 3.0] {
        p.line_segment(
            [pos2(thumb.left() + 2.0, y), pos2(thumb.right() - 2.0, y)],
            Stroke::new(1.0_f32, c(th.text)),
        );
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
    // Mono or polarity: the mix changes shape.
    let mut reshaped = false;
    let mut clicked: Option<(char, usize)> = None;
    let mut moved: Vec<(usize, af27_core::automation::LaneTarget, f64)> = Vec::new();
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
                    let (mut m, mut s, mut r) = (tr.mute, tr.solo, tr.armed);
                    if key(
                        ui,
                        &mut m,
                        "M",
                        th.meter_mid,
                        "Mute (Ctrl+Shift: every track)",
                        &th,
                    ) {
                        clicked = Some(('M', ti));
                    }
                    if key(
                        ui,
                        &mut s,
                        "S",
                        th.accent,
                        "Solo (Ctrl: this one only; Ctrl+Shift: every track)",
                        &th,
                    ) {
                        clicked = Some(('S', ti));
                    }
                    if tr.kind != af27_core::TrackKind::Bus
                        && key(
                            ui,
                            &mut r,
                            "R",
                            th.accent_alt,
                            "Arm for recording (Ctrl+Shift: every track)",
                            &th,
                        )
                    {
                        clicked = Some(('R', ti));
                    }
                });
                cui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    ui.add_space(((W - 12.0) - 2.0 * 22.0 - 3.0) / 2.0);
                    if key(
                        ui,
                        &mut tr.mono,
                        "Mo",
                        th.accent,
                        "Mono: fold left and right together after the effects",
                        &th,
                    ) {
                        reshaped = true;
                    }
                    if key(ui, &mut tr.polarity, "Ø", th.accent, "Invert polarity", &th) {
                        reshaped = true;
                    }
                });
                if pan_knob(&mut cui, &mut tr.pan, &th) {
                    changed = true;
                    moved.push((ti, af27_core::automation::LaneTarget::Pan, tr.pan as f64));
                }
                let vol_before = tr.volume_db;
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
                if tr.volume_db != vol_before {
                    moved.push((
                        ti,
                        af27_core::automation::LaneTarget::Volume,
                        tr.volume_db as f64,
                    ));
                }
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
    if reshaped {
        st.commit_session("Mixer", before);
        return;
    }
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
        for (ti, target, value) in moved {
            st.auto_moved(ti, target, value);
        }
    }
    let mods = ui.input(|i| i.modifiers);
    match clicked {
        Some(('S', ti)) => st.click_solo(ti, mods),
        Some(('M', ti)) => st.click_mute(ti, mods),
        Some(('R', ti)) => st.click_arm(ti, mods),
        _ => {}
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
