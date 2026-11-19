// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The Piano Roll for one MIDI clip. Draw tool: click empty space to add a
//! note one grid step long and drag to stretch it; drag a note to move
//! it, drag its right edge to resize it; right-click a note to delete it.
//! Erase tool: click or sweep over notes. The lane underneath sets each
//! note's velocity or pan. Notes take the 16 MIDI channels' colours.

use egui::{pos2, vec2, Align2, Color32, FontId, Rect, RichText, Rounding, Sense, Stroke};

use crate::state::State;
use crate::theme::{c, ca};
use af27_core::midi::{note_name, MidiNote};

const KEYS_W: f32 = 52.0;
const ROW: f32 = 12.0;
const LANE_H: f32 = 70.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Draw,
    Erase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Velocity,
    Pan,
}

#[derive(Debug, Clone, Copy)]
enum NoteDrag {
    Move {
        idx: usize,
        grab_beats: f64,
        grab_pitch: i32,
    },
    Resize {
        idx: usize,
    },
    Erase,
    Lane,
}

pub struct PianoState {
    tool: Tool,
    lane: Lane,
    /// Grid step in beats (0 = off).
    grid: f64,
    channel: u8,
    /// Pixels per beat.
    zoom: f32,
    /// First beat shown.
    scroll_x: f64,
    /// Highest pitch shown at the top.
    top: f32,
    drag: Option<NoteDrag>,
    before: Option<af27_core::Session>,
    last_len: f64,
}

impl Default for PianoState {
    fn default() -> Self {
        PianoState {
            tool: Tool::Draw,
            lane: Lane::Velocity,
            grid: 0.25,
            channel: 0,
            zoom: 80.0,
            scroll_x: 0.0,
            top: 84.0,
            drag: None,
            before: None,
            last_len: 0.25,
        }
    }
}

/// Distinct colours for the 16 MIDI channels.
pub fn channel_color(ch: u8) -> Color32 {
    let h = (ch as f32 * 0.0625 + 0.52).fract();
    let rgb = egui::ecolor::Hsva::new(h, 0.65, 0.95, 1.0);
    Color32::from(rgb)
}

fn is_black(p: u8) -> bool {
    matches!(p % 12, 1 | 3 | 6 | 8 | 10)
}

pub fn panel(ui: &mut egui::Ui, st: &mut State, ps: &mut PianoState) {
    let th = st.theme.clone();
    let Some((ti, ci)) = st.piano_clip.and_then(|id| st.session.find_midi(id)) else {
        ui.add_space(8.0);
        ui.label(RichText::new("No MIDI clip open.").strong());
        ui.label(RichText::new("Double-click a MIDI clip in the Multitrack editor, or double-click an empty spot on a MIDI track to make one.").color(c(th.text_dim)));
        if ui.button("➕ New MIDI Track with a 4-bar Clip").clicked() {
            let t = st.add_track_kind(af27_core::TrackKind::Midi);
            new_clip(st, t, 0.0);
        }
        return;
    };
    // Toolbar.
    ui.horizontal_wrapped(|ui| {
        let name = st.session.tracks[ti].midi[ci].name.clone();
        ui.label(
            RichText::new(format!("{} • {}", st.session.tracks[ti].name, name))
                .strong()
                .color(c(th.accent)),
        );
        ui.separator();
        ui.selectable_value(&mut ps.tool, Tool::Draw, "✏ Draw");
        ui.selectable_value(&mut ps.tool, Tool::Erase, "⌫ Erase");
        ui.separator();
        egui::ComboBox::from_label("Grid")
            .selected_text(match ps.grid {
                g if g == 1.0 => "1/4",
                g if g == 0.5 => "1/8",
                g if g == 0.25 => "1/16",
                g if g == 0.125 => "1/32",
                _ => "Off",
            })
            .width(56.0)
            .show_ui(ui, |ui| {
                for (g, n) in [
                    (1.0, "1/4"),
                    (0.5, "1/8"),
                    (0.25, "1/16"),
                    (0.125, "1/32"),
                    (0.0, "Off"),
                ] {
                    ui.selectable_value(&mut ps.grid, g, n);
                }
            });
        ui.label("Channel");
        ui.add(
            egui::DragValue::new(&mut ps.channel)
                .range(0..=15)
                .custom_formatter(|v, _| format!("{}", v as u8 + 1)),
        );
        let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
        ui.painter()
            .rect_filled(r, Rounding::same(3.0), channel_color(ps.channel));
        ui.separator();
        ui.selectable_value(&mut ps.lane, Lane::Velocity, "Velocity");
        ui.selectable_value(&mut ps.lane, Lane::Pan, "Pan");
        if ui
            .button("Humanize")
            .on_hover_text("Nudge every note's start a little at random")
            .clicked()
        {
            let before = st.session.clone();
            let mut seed = 0x1234_5678u32;
            let step = if ps.grid > 0.0 { ps.grid } else { 0.25 };
            for n in &mut st.session.tracks[ti].midi[ci].notes {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let r = seed as f64 / u32::MAX as f64 * 2.0 - 1.0;
                n.start = (n.start + r * step * 0.12).max(0.0);
                n.velocity = (n.velocity as f64 * (1.0 + r * 0.08))
                    .round()
                    .clamp(1.0, 127.0) as u8;
            }
            st.commit_session("Humanize", before);
        }
        ui.add(
            egui::Slider::new(&mut ps.zoom, 20.0..=400.0)
                .logarithmic(true)
                .show_value(false)
                .text("Zoom"),
        );
    });
    let full = ui.available_rect_before_wrap();
    let (rect, resp) = ui.allocate_exact_size(full.size(), Sense::click_and_drag());
    let grid_r = Rect::from_min_max(
        pos2(rect.left() + KEYS_W, rect.top()),
        pos2(rect.right(), rect.bottom() - LANE_H - 4.0),
    );
    let keys_r = Rect::from_min_max(rect.min, pos2(grid_r.left(), grid_r.bottom()));
    let lane_r = Rect::from_min_max(pos2(grid_r.left(), grid_r.bottom() + 4.0), rect.max);
    let clip_len = st.session.tracks[ti].midi[ci].length;
    let bpb = st.session.tempo.beats_per_bar.max(1) as f64;

    // Scrolling and zoom.
    if let Some(h) = resp.hover_pos() {
        let (sc, ctrl, shift) = ui.input(|i| {
            (
                i.smooth_scroll_delta,
                i.modifiers.command,
                i.modifiers.shift,
            )
        });
        if ctrl && sc.y != 0.0 {
            ps.zoom = (ps.zoom * (sc.y * 0.004).exp()).clamp(20.0, 400.0);
        } else if shift || sc.x != 0.0 {
            let d = if shift { sc.y } else { sc.x };
            ps.scroll_x = (ps.scroll_x - d as f64 / ps.zoom as f64).clamp(0.0, clip_len.max(1.0));
        } else if sc.y != 0.0 && grid_r.union(keys_r).contains(h) {
            ps.top = (ps.top + sc.y / ROW).clamp(12.0, 127.0);
        }
    }
    let x_of = |b: f64| grid_r.left() + ((b - ps.scroll_x) as f32) * ps.zoom;
    let b_of = |x: f32| ps.scroll_x + ((x - grid_r.left()) / ps.zoom) as f64;
    let y_of = |p: f32| grid_r.top() + (ps.top - p) * ROW;
    let p_of = |y: f32| (ps.top - ((y - grid_r.top()) / ROW).floor()).round() as i32;
    let snap = |b: f64| {
        if ps.grid > 0.0 {
            (b / ps.grid).floor() * ps.grid
        } else {
            b
        }
    };

    let p = ui.painter_at(rect);
    p.rect_filled(grid_r, Rounding::ZERO, c(th.wave_background));
    // Rows and keys.
    let lo = (ps.top - grid_r.height() / ROW).floor().max(0.0) as i32;
    for pitch in lo..=(ps.top as i32).min(127) {
        let y = y_of(pitch as f32);
        let row = Rect::from_min_size(pos2(grid_r.left(), y), vec2(grid_r.width(), ROW));
        if is_black(pitch as u8) {
            p.rect_filled(row, Rounding::ZERO, ca(th.panel, 160));
        }
        if pitch % 12 == 0 {
            p.line_segment(
                [row.left_bottom(), row.right_bottom()],
                Stroke::new(1.0_f32, c(th.border)),
            );
        }
        let key = Rect::from_min_size(pos2(keys_r.left(), y), vec2(KEYS_W - 2.0, ROW - 1.0));
        let black = is_black(pitch as u8);
        p.rect_filled(
            key,
            Rounding::same(2.0),
            if black {
                Color32::from_gray(30)
            } else {
                Color32::from_gray(215)
            },
        );
        if pitch % 12 == 0 {
            p.text(
                key.right_center() - vec2(3.0, 0.0),
                Align2::RIGHT_CENTER,
                note_name(pitch as u8),
                FontId::proportional(9.0),
                Color32::from_gray(40),
            );
        }
    }
    // Beat and bar lines; beyond the clip's end shaded.
    let first = ps.scroll_x.floor() as i64;
    let last = b_of(grid_r.right()).ceil() as i64;
    for b in first..=last {
        let x = x_of(b as f64);
        let bar = (b as f64 % bpb) == 0.0;
        p.line_segment(
            [pos2(x, grid_r.top()), pos2(x, lane_r.bottom())],
            Stroke::new(
                if bar { 1.5_f32 } else { 1.0 },
                if bar { c(th.border) } else { c(th.grid) },
            ),
        );
        if bar {
            p.text(
                pos2(x + 3.0, grid_r.top() + 1.0),
                Align2::LEFT_TOP,
                format!("{}", (b as f64 / bpb) as i64 + 1),
                FontId::proportional(9.0),
                c(th.text_dim),
            );
        }
    }
    let end_x = x_of(clip_len);
    if end_x < grid_r.right() {
        p.rect_filled(
            Rect::from_min_max(pos2(end_x.max(grid_r.left()), grid_r.top()), grid_r.max),
            Rounding::ZERO,
            Color32::from_black_alpha(110),
        );
    }

    // Hit test.
    let notes = st.session.tracks[ti].midi[ci].notes.clone();
    let note_rect = |n: &MidiNote| {
        Rect::from_min_max(
            pos2(x_of(n.start), y_of(n.pitch as f32) + 1.0),
            pos2(
                x_of(n.start + n.length).max(x_of(n.start) + 3.0),
                y_of(n.pitch as f32) + ROW - 1.0,
            ),
        )
    };
    let hit = |q: egui::Pos2| {
        notes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, n)| note_rect(n).contains(q))
            .map(|(i, n)| (i, (note_rect(n).right() - q.x).abs() < 6.0))
    };

    if resp.drag_started() {
        if let Some(q) = resp.interact_pointer_pos() {
            ps.before = Some(st.session.clone());
            ps.drag = if lane_r.contains(q) {
                Some(NoteDrag::Lane)
            } else if grid_r.contains(q) {
                match (ps.tool, hit(q)) {
                    (Tool::Erase, _) => Some(NoteDrag::Erase),
                    (Tool::Draw, Some((i, true))) => Some(NoteDrag::Resize { idx: i }),
                    (Tool::Draw, Some((i, false))) => Some(NoteDrag::Move {
                        idx: i,
                        grab_beats: b_of(q.x) - notes[i].start,
                        grab_pitch: p_of(q.y) - notes[i].pitch as i32,
                    }),
                    (Tool::Draw, None) => {
                        let start = snap(b_of(q.x)).max(0.0);
                        let pitch = p_of(q.y).clamp(0, 127) as u8;
                        let len = if ps.grid > 0.0 {
                            ps.grid.max(ps.last_len)
                        } else {
                            ps.last_len
                        };
                        let ns = &mut st.session.tracks[ti].midi[ci].notes;
                        ns.push(MidiNote {
                            pitch,
                            start,
                            length: len,
                            velocity: 100,
                            channel: ps.channel,
                            pan: 0.0,
                        });
                        Some(NoteDrag::Resize { idx: ns.len() - 1 })
                    }
                }
            } else {
                None
            };
        }
    }
    if let (Some(d), Some(q)) = (ps.drag, resp.interact_pointer_pos()) {
        if resp.dragged() {
            let ns = &mut st.session.tracks[ti].midi[ci].notes;
            match d {
                NoteDrag::Move {
                    idx,
                    grab_beats,
                    grab_pitch,
                } => {
                    if let Some(n) = ns.get_mut(idx) {
                        n.start = snap(
                            b_of(q.x) - grab_beats
                                + if ps.grid > 0.0 { ps.grid / 2.0 } else { 0.0 },
                        )
                        .max(0.0);
                        n.pitch = (p_of(q.y) - grab_pitch).clamp(0, 127) as u8;
                    }
                }
                NoteDrag::Resize { idx } => {
                    if let Some(n) = ns.get_mut(idx) {
                        let end = if ps.grid > 0.0 {
                            (b_of(q.x) / ps.grid).ceil() * ps.grid
                        } else {
                            b_of(q.x)
                        };
                        n.length = (end - n.start).max(if ps.grid > 0.0 { ps.grid } else { 0.05 });
                        ps.last_len = n.length;
                    }
                }
                NoteDrag::Erase => {
                    if let Some(i) = ns.iter().position(|n| note_rect(n).contains(q)) {
                        ns.remove(i);
                    }
                }
                NoteDrag::Lane => {
                    let b = b_of(q.x);
                    let frac = ((lane_r.bottom() - q.y) / lane_r.height()).clamp(0.0, 1.0);
                    if let Some(n) = ns
                        .iter_mut()
                        .filter(|n| (n.start - b).abs() < 6.0 / ps.zoom as f64 + 0.02)
                        .min_by(|a, b2| (a.start - b).abs().total_cmp(&(b2.start - b).abs()))
                    {
                        match ps.lane {
                            Lane::Velocity => n.velocity = (1.0 + frac * 126.0).round() as u8,
                            Lane::Pan => n.pan = frac * 2.0 - 1.0,
                        }
                    }
                }
            }
        }
    }
    if resp.drag_stopped() {
        ps.drag = None;
        if let Some(b) = ps.before.take() {
            if b != st.session {
                st.commit_session("Edit notes", b);
            }
        }
    }
    if resp.secondary_clicked() || (resp.clicked() && ps.tool == Tool::Erase) {
        if let Some(q) = resp.interact_pointer_pos() {
            if let Some((i, _)) = hit(q) {
                let before = st.session.clone();
                st.session.tracks[ti].midi[ci].notes.remove(i);
                st.commit_session("Delete note", before);
            }
        }
    } else if resp.clicked() && ps.tool == Tool::Draw {
        if let Some(q) = resp.interact_pointer_pos() {
            if grid_r.contains(q) && hit(q).is_none() {
                let before = st.session.clone();
                let start = snap(b_of(q.x)).max(0.0);
                let pitch = p_of(q.y).clamp(0, 127) as u8;
                st.session.tracks[ti].midi[ci].notes.push(MidiNote {
                    pitch,
                    start,
                    length: ps.last_len,
                    velocity: 100,
                    channel: ps.channel,
                    pan: 0.0,
                });
                st.commit_session("Add note", before);
            }
        }
    }

    // Notes.
    let gp = ui.painter_at(grid_r);
    for n in &st.session.tracks[ti].midi[ci].notes {
        let r = note_rect(n);
        let col = channel_color(n.channel);
        let a = (90.0 + n.velocity as f32 * 1.3).min(255.0) as u8;
        gp.rect_filled(
            r,
            Rounding::same(2.0),
            Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), a),
        );
        gp.rect_stroke(
            r,
            Rounding::same(2.0),
            Stroke::new(1.0_f32, Color32::from_black_alpha(120)),
        );
        if r.width() > 26.0 {
            gp.text(
                r.left_center() + vec2(3.0, 0.0),
                Align2::LEFT_CENTER,
                note_name(n.pitch),
                FontId::proportional(8.0),
                Color32::from_black_alpha(200),
            );
        }
    }
    // Lane.
    p.rect_filled(lane_r, Rounding::same(th.rounding), c(th.panel));
    p.text(
        lane_r.left_top() - vec2(KEYS_W - 4.0, -2.0),
        Align2::LEFT_TOP,
        match ps.lane {
            Lane::Velocity => "Velocity",
            Lane::Pan => "Pan",
        },
        FontId::proportional(9.0),
        c(th.text_dim),
    );
    let lp = ui.painter_at(lane_r);
    if ps.lane == Lane::Pan {
        let y = lane_r.center().y;
        lp.line_segment(
            [pos2(lane_r.left(), y), pos2(lane_r.right(), y)],
            Stroke::new(1.0_f32, c(th.border)),
        );
    }
    for n in &st.session.tracks[ti].midi[ci].notes {
        let x = x_of(n.start);
        let frac = match ps.lane {
            Lane::Velocity => n.velocity as f32 / 127.0,
            Lane::Pan => (n.pan + 1.0) / 2.0,
        };
        let y = lane_r.bottom() - frac * lane_r.height();
        let base = if ps.lane == Lane::Pan {
            lane_r.center().y
        } else {
            lane_r.bottom()
        };
        lp.line_segment(
            [pos2(x, base), pos2(x, y)],
            Stroke::new(2.0_f32, channel_color(n.channel)),
        );
        lp.circle_filled(pos2(x, y), 3.0, channel_color(n.channel));
    }
    // Playhead within the clip.
    let clip_start = st.session.tracks[ti].midi[ci].start;
    let beat = st.session.tempo.beats(st.position()) - clip_start;
    let x = x_of(beat);
    if x >= grid_r.left() && x <= grid_r.right() {
        crate::waveform::playhead(&p, x, grid_r.top(), lane_r.bottom(), &th);
    }
}

/// Puts a new 4-bar MIDI clip on `track` at `beats` and opens it.
pub fn new_clip(st: &mut State, track: af27_core::TrackId, beats: f64) {
    let before = st.session.clone();
    let id = st.session.alloc_id();
    let bars = 4.0 * st.session.tempo.beats_per_bar.max(1) as f64;
    let n = st
        .session
        .tracks
        .iter()
        .map(|t| t.midi.len())
        .sum::<usize>()
        + 1;
    if let Some(t) = st.session.track_mut(track) {
        t.midi.push(af27_core::midi::MidiClip {
            id,
            name: format!("MIDI {n}"),
            start: beats.max(0.0),
            length: bars,
            notes: Vec::new(),
            muted: false,
            ext_id: None,
        });
        st.piano_clip = Some(id);
        st.selected_clip = Some(id);
        st.want_piano = true;
        st.commit_session("New MIDI clip", before);
    }
}
