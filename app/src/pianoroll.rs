// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The Piano Roll for one MIDI clip.
//!
//! Tools: Draw (click empty space for a note one grid step long; keep the
//! button down to move it), Select (click a note to pick it, drag on empty
//! space to pick every note in a box) and Erase (click or sweep). In every
//! tool, drag a note's middle to move it and either end to stretch it that
//! way; Shift-click picks several and Shift-drag boxes, and moving or
//! stretching one picked note moves or stretches them all. Right-click a
//! note to delete it; hold the right button and drag to erase every note
//! it passes. Hold Alt while dragging to leave the grid. Notes sound
//! through the track's instrument as they are drawn, picked up and moved
//! to other pitches, and the keys play when clicked.
//!
//! Cut, Copy, Paste, Delete and Select All (their usual keys while the
//! pointer is over the roll, or the buttons) act on the picked notes;
//! Paste puts them at the playhead (or the last spot clicked when the
//! playhead is outside the clip). Tools sets, scales or randomizes
//! velocity and pan, sets the channel and humanizes timing; Quantize pulls
//! notes to a grid with strength and swing. Each acts on the picked notes,
//! or all of them when none are picked. Every gesture is one undo step.
//!
//! Getting around: wheel scrolls the keys, Shift+wheel scrolls time,
//! Ctrl+wheel zooms time, Alt+wheel makes the keys taller or shorter, the
//! middle button pans.

use egui::{
    pos2, vec2, Align2, Color32, FontId, PointerButton, Rect, RichText, Rounding, Sense, Stroke,
};

use crate::state::State;
use crate::theme::{c, ca};
use af27_core::midi::{note_name, MidiNote};
use af27_core::noteops as ops;
use af27_core::quantize::{Ends, Quantize};

const KEYS_W: f32 = 52.0;
const LANE_H: f32 = 70.0;
const EDGE_PX: f32 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Draw,
    Select,
    Erase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Velocity,
    Pan,
    /// A controller's moves (0..127, 128 aftertouch, 129 pitch bend).
    Control(u8),
}

/// A drawn controller value, kept to the controller's own steps: pedals
/// up or down, pitch bend 16384 steps, others 128.
fn shape_control(controller: u8, frac: f32) -> f32 {
    let f = frac.clamp(0.0, 1.0);
    if af27_core::midi::ControlEvent::is_switch(controller) {
        if f >= 0.5 {
            1.0
        } else {
            0.0
        }
    } else if controller == af27_core::midi::PITCH_BEND {
        (f * 16383.0).round() / 16383.0
    } else {
        (f * 127.0).round() / 127.0
    }
}

/// Draws a controller value at beat `b` over the span dragged since
/// `from`: moves in that span give way to the new one. A pedal move is
/// kept only where the value changes.
fn paint_control(
    controls: &mut Vec<af27_core::midi::ControlEvent>,
    controller: u8,
    channel: u8,
    from: f64,
    b: f64,
    value: f32,
) {
    // The span since the last move, not counting that move itself.
    let inside = |at: f64| {
        if b > from {
            at > from && at <= b
        } else if b < from {
            at >= b && at < from
        } else {
            at == b
        }
    };
    controls.retain(|e| !(e.controller == controller && e.channel == channel && inside(e.at)));
    let before = controls
        .iter()
        .filter(|e| e.controller == controller && e.channel == channel && e.at < b)
        .max_by(|x, y| x.at.total_cmp(&y.at))
        .map(|e| e.value);
    if af27_core::midi::ControlEvent::is_switch(controller) && before == Some(value) {
        return;
    }
    controls.push(af27_core::midi::ControlEvent {
        at: b.max(0.0),
        controller,
        value,
        channel,
    });
    controls.sort_by(|x, y| x.at.total_cmp(&y.at));
}

/// Edit commands routed from the keyboard while the pointer is on the roll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PianoCmd {
    Cut,
    Copy,
    Paste,
    Delete,
    SelectAll,
}

/// Grid steps, as in Audiform's piano roll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grid {
    Bar,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
    EighthTriplet,
    SixteenthTriplet,
    Off,
}

impl Grid {
    const ALL: [Grid; 8] = [
        Grid::Bar,
        Grid::Quarter,
        Grid::Eighth,
        Grid::Sixteenth,
        Grid::ThirtySecond,
        Grid::EighthTriplet,
        Grid::SixteenthTriplet,
        Grid::Off,
    ];
    fn label(self) -> &'static str {
        match self {
            Grid::Bar => "1 bar",
            Grid::Quarter => "1/4",
            Grid::Eighth => "1/8",
            Grid::Sixteenth => "1/16",
            Grid::ThirtySecond => "1/32",
            Grid::EighthTriplet => "1/8 triplet",
            Grid::SixteenthTriplet => "1/16 triplet",
            Grid::Off => "Off",
        }
    }
    /// The step in quarter notes (0 = off). "1 bar" is the bar where the
    /// clip starts.
    fn beats(self, bar_beats: f64) -> f64 {
        match self {
            Grid::Bar => bar_beats,
            Grid::Quarter => 1.0,
            Grid::Eighth => 0.5,
            Grid::Sixteenth => 0.25,
            Grid::ThirtySecond => 0.125,
            Grid::EighthTriplet => 1.0 / 3.0,
            Grid::SixteenthTriplet => 1.0 / 6.0,
            Grid::Off => 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum NoteDrag {
    /// The picked notes, by the grabbed one.
    Move {
        grabbed: usize,
        grab_beats: f64,
        grab_pitch: i32,
    },
    ResizeEnd {
        grabbed: usize,
        grab_beats: f64,
    },
    ResizeStart {
        grabbed: usize,
        grab_beats: f64,
    },
    /// A box from (beats, pitch); notes picked before it stay picked.
    Box {
        b0: f64,
        p0: i32,
    },
    Erase,
    Lane,
    Pan,
    /// Drawing a controller; where the last move went.
    Control {
        controller: u8,
        last: Option<f64>,
    },
}

pub struct PianoState {
    /// A note drawn on the press itself, held until the button comes up.
    press_new: bool,
    tool: Tool,
    lane: Lane,
    grid: Grid,
    channel: u8,
    /// Pixels per beat.
    zoom: f32,
    /// Key height in pixels.
    row: f32,
    /// First beat shown.
    scroll_x: f64,
    /// Highest pitch shown at the top.
    top: f32,
    drag: Option<NoteDrag>,
    before: Option<af27_core::Session>,
    /// The notes when the gesture began.
    orig: Vec<MidiNote>,
    /// Picked notes, by index.
    sel: Vec<usize>,
    /// Picked before a Shift-box began.
    sel_base: Vec<usize>,
    box_now: Option<egui::Pos2>,
    clip: Option<af27_core::ClipId>,
    last_len: f64,
    clipboard: Vec<MidiNote>,
    /// The last spot clicked, in beats from the clip start.
    click_at: f64,
    /// The pitch sounding from a press (note or key).
    held: Option<u8>,
    /// A note added by a click sounds at least until this time (ui seconds).
    held_until: f64,
    show_tools: bool,
    show_quantize: bool,
    vel_set: u8,
    vel_scale: f64,
    vel_rand: f64,
    pan_set: f32,
    pan_rand: f32,
    stray_ms: f64,
    quant: Quantize,
    quant_grid: Grid,
    rng: ops::Rng,
}

impl Default for PianoState {
    fn default() -> Self {
        PianoState {
            press_new: false,
            tool: Tool::Draw,
            lane: Lane::Velocity,
            grid: Grid::Sixteenth,
            channel: 0,
            zoom: 80.0,
            row: 12.0,
            scroll_x: 0.0,
            top: 84.0,
            drag: None,
            before: None,
            orig: Vec::new(),
            sel: Vec::new(),
            sel_base: Vec::new(),
            box_now: None,
            clip: None,
            last_len: 0.25,
            clipboard: Vec::new(),
            click_at: 0.0,
            held: None,
            held_until: 0.0,
            show_tools: false,
            show_quantize: false,
            vel_set: 100,
            vel_scale: 110.0,
            vel_rand: 10.0,
            pan_set: 0.0,
            pan_rand: 30.0,
            stray_ms: 10.0,
            quant: Quantize::default(),
            quant_grid: Grid::Sixteenth,
            rng: ops::Rng::from_time(),
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

fn grid_combo(ui: &mut egui::Ui, id: &str, g: &mut Grid) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(g.label())
        .width(84.0)
        .show_ui(ui, |ui| {
            for x in Grid::ALL {
                ui.selectable_value(g, x, x.label());
            }
        });
}

/// Applies an edit to the picked notes (or all) as one undo step.
fn apply(
    st: &mut State,
    ti: usize,
    ci: usize,
    sel: &[usize],
    label: &str,
    f: impl FnOnce(&mut Vec<MidiNote>, &[usize]),
) {
    let before = st.session.clone();
    let notes = &mut st.session.tracks[ti].midi[ci].notes;
    let idx = ops::targets(notes.len(), sel);
    f(notes, &idx);
    if before != st.session {
        st.commit_session(label, before);
    }
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
    if ps.clip != st.piano_clip {
        ps.clip = st.piano_clip;
        st.audition_all_off();
        ps.held = None;
        ps.sel.clear();
        ps.drag = None;
        ps.click_at = 0.0;
    }
    let n_notes = st.session.tracks[ti].midi[ci].notes.len();
    ps.sel.retain(|&i| i < n_notes);
    let clip_start = st.session.tracks[ti].midi[ci].start;
    let clip_len = st.session.tracks[ti].midi[ci].length;
    let tempo = st.session.tempo.clone();
    let bar_beats = tempo.seg_at_beat(clip_start).bar_beats();
    let step = ps.grid.beats(bar_beats);
    // The playhead, in beats from the clip start.
    let play_at = tempo.beats(st.position()) - clip_start;

    // Keyboard commands routed here.
    if let Some(cmd) = st.piano_cmd.take() {
        run_cmd(st, ps, ti, ci, cmd, play_at);
    }

    // Toolbar.
    ui.horizontal_wrapped(|ui| {
        let name = st.session.tracks[ti].midi[ci].name.clone();
        ui.label(
            RichText::new(format!("{} • {}", st.session.tracks[ti].name, name))
                .strong()
                .color(c(th.accent)),
        );
        ui.separator();
        ui.selectable_value(&mut ps.tool, Tool::Draw, "✏ Draw")
            .on_hover_text("Click empty space to add a note; keep the button down to move it");
        ui.selectable_value(&mut ps.tool, Tool::Select, "⬚ Select")
            .on_hover_text(
                "Click a note to pick it; drag on empty space to pick every note in a box",
            );
        ui.selectable_value(&mut ps.tool, Tool::Erase, "⌫ Erase")
            .on_hover_text("Click or drag over notes to remove them");
        ui.separator();
        ui.label("Grid");
        grid_combo(ui, "piano-grid", &mut ps.grid);
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
        {
            use af27_core::midi::{ControlEvent, LANE_CONTROLLERS};
            let used: Vec<u8> = st.session.tracks[ti].midi[ci]
                .controls
                .iter()
                .map(|e| e.controller)
                .collect();
            let shown = match ps.lane {
                Lane::Control(c) => ControlEvent::name(c),
                _ => "Controller".to_string(),
            };
            let mut list: Vec<u8> = LANE_CONTROLLERS.to_vec();
            for c in &used {
                if !list.contains(c) {
                    list.push(*c);
                }
            }
            egui::ComboBox::from_id_salt("piano-ctl-lane")
                .selected_text(shown)
                .width(110.0)
                .show_ui(ui, |ui| {
                    for c in list {
                        // A dot marks a controller the clip already uses.
                        let mark = if used.contains(&c) { " •" } else { "" };
                        ui.selectable_value(
                            &mut ps.lane,
                            Lane::Control(c),
                            format!("{}{mark}", ControlEvent::name(c)),
                        );
                    }
                });
        }
        ui.separator();
        let picked = !ps.sel.is_empty();
        if ui.add_enabled(picked, egui::Button::new("Cut")).clicked() {
            run_cmd(st, ps, ti, ci, PianoCmd::Cut, play_at);
        }
        if ui.add_enabled(picked, egui::Button::new("Copy")).clicked() {
            run_cmd(st, ps, ti, ci, PianoCmd::Copy, play_at);
        }
        if ui
            .add_enabled(!ps.clipboard.is_empty(), egui::Button::new("Paste"))
            .on_hover_text(
                "At the playhead (or the last spot clicked when the playhead is outside the clip)",
            )
            .clicked()
        {
            run_cmd(st, ps, ti, ci, PianoCmd::Paste, play_at);
        }
        ui.separator();
        ui.toggle_value(&mut ps.show_tools, "Tools ▾")
            .on_hover_text("Velocity, pan, channel and Humanize, on the picked notes or all");
        ui.toggle_value(&mut ps.show_quantize, "Quantize ▾")
            .on_hover_text("Pull notes to a grid, on the picked notes or all");
        ui.separator();
        ui.add(
            crate::widgets::Slider::new(&mut ps.zoom, 20.0..=400.0)
                .logarithmic(true)
                .show_value(false)
                .text("Zoom"),
        );
        if ui.small_button("−").on_hover_text("Shorter keys").clicked() {
            ps.row = (ps.row - 2.0).max(6.0);
        }
        if ui.small_button("+").on_hover_text("Taller keys").clicked() {
            ps.row = (ps.row + 2.0).min(32.0);
        }
    });
    if ps.show_tools {
        tools_row(ui, st, ps, ti, ci, clip_start);
    }
    if ps.show_quantize {
        quantize_row(ui, st, ps, ti, ci, bar_beats);
    }

    let full = ui.available_rect_before_wrap();
    let (rect, resp) = ui.allocate_exact_size(full.size(), Sense::click_and_drag());
    if resp.hovered() || resp.dragged() {
        st.piano_hover = true;
    }
    let row_h = ps.row;
    let grid_r = Rect::from_min_max(
        pos2(rect.left() + KEYS_W, rect.top()),
        pos2(rect.right(), rect.bottom() - LANE_H - 4.0),
    );
    let keys_r = Rect::from_min_max(rect.min, pos2(grid_r.left(), grid_r.bottom()));
    let lane_r = Rect::from_min_max(pos2(grid_r.left(), grid_r.bottom() + 4.0), rect.max);

    // Scrolling and zoom.
    if let Some(h) = resp.hover_pos() {
        let (sc, zoomd, shift, alt) = ui.input(|i| {
            (
                i.smooth_scroll_delta,
                i.zoom_delta(),
                i.modifiers.shift,
                i.modifiers.alt,
            )
        });
        if zoomd != 1.0 {
            // Ctrl+wheel (egui hands it over as a zoom factor) or a pinch:
            // time zoom, 1.2 times per notch, at the pointer.
            let notches = crate::zoom::ctrl_points(zoomd) / crate::zoom::NOTCH;
            let px = (h.x - grid_r.left()).max(0.0);
            let anchor = ps.scroll_x + (px / ps.zoom) as f64;
            ps.zoom = (ps.zoom * 1.2f32.powf(notches)).clamp(20.0, 400.0);
            ps.scroll_x = (anchor - (px / ps.zoom) as f64).max(0.0);
        } else if alt && (sc.y != 0.0 || sc.x != 0.0) {
            // Keys taller or shorter, 2 px per notch, keeping the key
            // under the pointer in place.
            let d = if sc.y != 0.0 { sc.y } else { sc.x };
            let py = h.y - grid_r.top();
            let key = ps.top - py / ps.row;
            ps.row = (ps.row + 2.0 * d / crate::zoom::NOTCH).clamp(6.0, 32.0);
            ps.top = (key + py / ps.row).clamp(12.0, 127.0);
        } else if shift || sc.x != 0.0 {
            let d = if shift { sc.y } else { sc.x };
            ps.scroll_x = (ps.scroll_x - d as f64 / ps.zoom as f64).clamp(0.0, clip_len.max(1.0));
        } else if sc.y != 0.0 && grid_r.union(keys_r).contains(h) {
            ps.top = (ps.top + sc.y / row_h).clamp(12.0, 127.0);
        }
    }
    if resp.dragged_by(PointerButton::Middle) {
        let d = resp.drag_delta();
        ps.scroll_x = (ps.scroll_x - d.x as f64 / ps.zoom as f64).clamp(0.0, clip_len.max(1.0));
        ps.top = (ps.top + d.y / row_h).clamp(12.0, 127.0);
    }
    let (scroll_x, zoom, top) = (ps.scroll_x, ps.zoom, ps.top);
    let x_of = |b: f64| grid_r.left() + ((b - scroll_x) as f32) * zoom;
    let b_of = |x: f32| scroll_x + ((x - grid_r.left()) / zoom) as f64;
    let y_of = |p: f32| grid_r.top() + (top - p) * row_h;
    let p_of = |y: f32| (top - ((y - grid_r.top()) / row_h).floor()).round() as i32;
    let free = ui.input(|i| i.modifiers.alt) || step <= 0.0;
    let snap_down = |b: f64| if free { b } else { (b / step).floor() * step };
    let snap_near = |b: f64| if free { b } else { (b / step).round() * step };

    let p = ui.painter_at(rect);
    p.rect_filled(grid_r, Rounding::ZERO, c(th.wave_background));
    // Rows and keys.
    let lo = (top - grid_r.height() / row_h).floor().max(0.0) as i32;
    for pitch in lo..=(top as i32).min(127) {
        let y = y_of(pitch as f32);
        let row = Rect::from_min_size(pos2(grid_r.left(), y), vec2(grid_r.width(), row_h));
        if is_black(pitch as u8) {
            p.rect_filled(row, Rounding::ZERO, ca(th.panel, 160));
        }
        if pitch % 12 == 0 {
            p.line_segment(
                [row.left_bottom(), row.right_bottom()],
                Stroke::new(1.0_f32, c(th.border)),
            );
        }
        let key = Rect::from_min_size(pos2(keys_r.left(), y), vec2(KEYS_W - 2.0, row_h - 1.0));
        let black = is_black(pitch as u8);
        let down = ps.held == Some(pitch as u8);
        p.rect_filled(
            key,
            Rounding::same(2.0),
            if down {
                c(th.accent)
            } else if black {
                Color32::from_gray(30)
            } else {
                Color32::from_gray(215)
            },
        );
        if pitch % 12 == 0 && row_h >= 8.0 {
            p.text(
                key.right_center() - vec2(3.0, 0.0),
                Align2::RIGHT_CENTER,
                note_name(pitch as u8),
                FontId::proportional(9.0_f32.min(row_h - 1.0)),
                Color32::from_gray(40),
            );
        }
    }
    // Grid steps, beats and bars from the tempo map; beyond the clip's end shaded.
    let first_b = ps.scroll_x;
    let last_b = b_of(grid_r.right());
    if step > 0.0 && step * ps.zoom as f64 >= 6.0 {
        let mut k = (first_b / step).floor() as i64;
        while (k as f64) * step <= last_b {
            let x = x_of(k as f64 * step);
            p.line_segment(
                [pos2(x, grid_r.top()), pos2(x, grid_r.bottom())],
                Stroke::new(1.0_f32, ca(th.grid, 70)),
            );
            k += 1;
        }
    }
    {
        let first_bar = tempo.bar_at((clip_start + first_b).max(0.0)).0;
        for bar in first_bar..first_bar + 400 {
            let bs = tempo.bar_start(bar);
            let rel = bs - clip_start;
            if rel > last_b {
                break;
            }
            let seg = tempo.seg_at_beat(bs);
            for k in 0..seg.beats_per_bar.max(1) {
                let b = rel + k as f64 * seg.unit_beats();
                let x = x_of(b);
                let is_bar = k == 0;
                p.line_segment(
                    [pos2(x, grid_r.top()), pos2(x, lane_r.bottom())],
                    Stroke::new(
                        if is_bar { 1.5_f32 } else { 1.0 },
                        if is_bar { c(th.border) } else { c(th.grid) },
                    ),
                );
                if is_bar && x >= grid_r.left() - 1.0 {
                    let label = if seg.bar == bar && seg.change.is_some() {
                        format!("{}  {} {}", bar + 1, seg.meter_text(), seg.bpm)
                    } else {
                        format!("{}", bar + 1)
                    };
                    p.text(
                        pos2(x + 3.0, grid_r.top() + 1.0),
                        Align2::LEFT_TOP,
                        label,
                        FontId::proportional(9.0),
                        c(th.text_dim),
                    );
                }
            }
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

    // Hit test: index, and whether on the left or right edge.
    let notes = st.session.tracks[ti].midi[ci].notes.clone();
    let note_rect = |n: &MidiNote| {
        Rect::from_min_max(
            pos2(x_of(n.start), y_of(n.pitch as f32) + 1.0),
            pos2(
                x_of(n.start + n.length).max(x_of(n.start) + 3.0),
                y_of(n.pitch as f32) + row_h - 1.0,
            ),
        )
    };
    let hit = |q: egui::Pos2| -> Option<(usize, bool, bool)> {
        notes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, n)| note_rect(n).contains(q))
            .map(|(i, n)| {
                let r = note_rect(n);
                let wide = r.width() > EDGE_PX * 3.0;
                let left = wide && q.x - r.left() < EDGE_PX;
                let right = !left && (r.right() - q.x < EDGE_PX.max(r.width() * 0.2).min(8.0));
                (i, left, right)
            })
    };
    let shift = ui.input(|i| i.modifiers.shift);
    let press = ui.input(|i| i.pointer.primary_pressed());
    let released = ui.input(|i| !i.pointer.primary_down());

    // Keys play while held.
    if press {
        if let Some(q) = ui.input(|i| i.pointer.interact_pos()) {
            if keys_r.contains(q) {
                let pitch = p_of(q.y).clamp(0, 127) as u8;
                st.audition_on(ti, pitch, 100, 0.0);
                ps.held = Some(pitch);
            } else if grid_r.contains(q) && ps.tool != Tool::Erase {
                if let Some((i, _, _)) = hit(q) {
                    let n = notes[i];
                    st.audition_on(ti, n.pitch, n.velocity, n.pan);
                    ps.held = Some(n.pitch);
                } else if ps.tool == Tool::Draw && !shift {
                    // Draw: the note appears the moment the button goes
                    // down and sounds while held; moving the pointer moves it.
                    let b = b_of(q.x);
                    ps.click_at = b.max(0.0);
                    ps.before = Some(st.session.clone());
                    let i = add_note(st, ps, ti, ci, snap_down(b).max(0.0), p_of(q.y), step);
                    ps.sel = vec![i];
                    ps.orig = st.session.tracks[ti].midi[ci].notes.clone();
                    let n = ps.orig[i];
                    if let Some(h) = ps.held.take() {
                        st.audition_off(h);
                    }
                    st.audition_on(ti, n.pitch, n.velocity, n.pan);
                    ps.held = Some(n.pitch);
                    ps.held_until = 0.0;
                    ps.drag = Some(NoteDrag::Move {
                        grabbed: i,
                        grab_beats: b,
                        grab_pitch: p_of(q.y),
                    });
                    ps.press_new = true;
                }
            }
        }
    }

    // Starting a drag.
    if resp.drag_started_by(PointerButton::Secondary) {
        ps.before = Some(st.session.clone());
        ps.drag = Some(NoteDrag::Erase);
    } else if resp.drag_started_by(PointerButton::Primary) && !ps.press_new {
        if let Some(q) = resp.interact_pointer_pos() {
            ps.before = Some(st.session.clone());
            ps.drag = if lane_r.contains(q) {
                Some(match ps.lane {
                    Lane::Velocity => NoteDrag::Lane,
                    Lane::Pan => NoteDrag::Pan,
                    Lane::Control(controller) => NoteDrag::Control {
                        controller,
                        last: None,
                    },
                })
            } else if grid_r.contains(q) {
                let b = b_of(q.x);
                ps.click_at = b.max(0.0);
                match (ps.tool, hit(q)) {
                    (Tool::Erase, _) => Some(NoteDrag::Erase),
                    (_, Some((i, left, right))) => {
                        if !ps.sel.contains(&i) {
                            if !shift {
                                ps.sel.clear();
                            }
                            ps.sel.push(i);
                        }
                        ps.orig = notes.clone();
                        Some(if left {
                            NoteDrag::ResizeStart {
                                grabbed: i,
                                grab_beats: b,
                            }
                        } else if right {
                            NoteDrag::ResizeEnd {
                                grabbed: i,
                                grab_beats: b,
                            }
                        } else {
                            NoteDrag::Move {
                                grabbed: i,
                                grab_beats: b,
                                grab_pitch: p_of(q.y),
                            }
                        })
                    }
                    (Tool::Select, None) | (Tool::Draw, None)
                        if shift || ps.tool == Tool::Select =>
                    {
                        ps.sel_base = if shift { ps.sel.clone() } else { Vec::new() };
                        if !shift {
                            ps.sel.clear();
                        }
                        ps.box_now = Some(q);
                        Some(NoteDrag::Box {
                            b0: b,
                            p0: p_of(q.y),
                        })
                    }
                    (_, None) => {
                        // Draw: a new note, then held to move it.
                        let i = add_note(st, ps, ti, ci, snap_down(b).max(0.0), p_of(q.y), step);
                        ps.sel = vec![i];
                        ps.orig = st.session.tracks[ti].midi[ci].notes.clone();
                        let n = ps.orig[i];
                        st.audition_on(ti, n.pitch, n.velocity, n.pan);
                        ps.held = Some(n.pitch);
                        Some(NoteDrag::Move {
                            grabbed: i,
                            grab_beats: n.start,
                            grab_pitch: n.pitch as i32,
                        })
                    }
                }
            } else {
                None
            };
        }
    }

    // Dragging.
    if let (Some(NoteDrag::Control { controller, last }), Some(q)) =
        (ps.drag, resp.interact_pointer_pos())
    {
        if resp.dragged() {
            let b = b_of(q.x).clamp(0.0, clip_len);
            let b = snap_down(b).max(0.0);
            let frac = (lane_r.bottom() - q.y) / lane_r.height();
            let value = shape_control(controller, frac);
            let channel = ps.channel;
            let ctl = &mut st.session.tracks[ti].midi[ci].controls;
            paint_control(ctl, controller, channel, last.unwrap_or(b), b, value);
            ps.drag = Some(NoteDrag::Control {
                controller,
                last: Some(b),
            });
        }
    } else if let (Some(d), Some(q)) = (ps.drag, resp.interact_pointer_pos()) {
        if resp.dragged() {
            let ns = &mut st.session.tracks[ti].midi[ci].notes;
            match d {
                NoteDrag::Move {
                    grabbed,
                    grab_beats,
                    grab_pitch,
                } => {
                    if let Some(o) = ps.orig.get(grabbed).copied() {
                        let target = snap_near(o.start + b_of(q.x) - grab_beats);
                        let ds = p_of(q.y) - grab_pitch;
                        ops::move_from(ns, &ps.orig, &ps.sel, target - o.start, ds);
                        let now = ns.get(grabbed).map(|n| n.pitch);
                        if now.is_some() && now != ps.held {
                            let n = ns[grabbed];
                            if let Some(h) = ps.held {
                                st.audition_off(h);
                            }
                            st.audition_on(ti, n.pitch, n.velocity, n.pan);
                            ps.held = Some(n.pitch);
                        }
                    }
                }
                NoteDrag::ResizeEnd {
                    grabbed,
                    grab_beats,
                } => {
                    if let Some(o) = ps.orig.get(grabbed).copied() {
                        let end = o.start + o.length;
                        let target = snap_near(end + b_of(q.x) - grab_beats);
                        let target = if free {
                            target
                        } else {
                            target.max(o.start + step)
                        };
                        ops::resize_end_from(ns, &ps.orig, &ps.sel, target - end);
                        ps.last_len = ns.get(grabbed).map_or(ps.last_len, |n| n.length);
                    }
                }
                NoteDrag::ResizeStart {
                    grabbed,
                    grab_beats,
                } => {
                    if let Some(o) = ps.orig.get(grabbed).copied() {
                        let target = snap_near(o.start + b_of(q.x) - grab_beats);
                        ops::resize_start_from(ns, &ps.orig, &ps.sel, target - o.start);
                    }
                }
                NoteDrag::Box { b0, p0 } => {
                    ps.box_now = Some(q);
                    let mut s = ps.sel_base.clone();
                    for i in ops::in_box(ns, b0, b_of(q.x), p0, p_of(q.y)) {
                        if !s.contains(&i) {
                            s.push(i);
                        }
                    }
                    ps.sel = s;
                }
                NoteDrag::Erase => {
                    if let Some(i) = ns.iter().rposition(|n| note_rect(n).contains(q)) {
                        ns.remove(i);
                        ps.sel.clear();
                    }
                }
                NoteDrag::Lane | NoteDrag::Pan => {
                    let b = b_of(q.x);
                    let frac = ((lane_r.bottom() - q.y) / lane_r.height()).clamp(0.0, 1.0);
                    if let Some(n) = ns
                        .iter_mut()
                        .filter(|n| (n.start - b).abs() < 6.0 / zoom as f64 + 0.02)
                        .min_by(|a, b2| (a.start - b).abs().total_cmp(&(b2.start - b).abs()))
                    {
                        match d {
                            NoteDrag::Lane => n.velocity = (1.0 + frac * 126.0).round() as u8,
                            _ => n.pan = frac * 2.0 - 1.0,
                        }
                    }
                }
                NoteDrag::Control { .. } => {}
            }
        }
    }
    if resp.drag_stopped() {
        ps.drag = None;
        ps.box_now = None;
        if let Some(b) = ps.before.take() {
            if b != st.session {
                st.commit_session("Edit notes", b);
            }
        }
    }

    // Clicks (no drag).
    if resp.secondary_clicked() {
        if let (Some(q), Lane::Control(controller)) = (resp.interact_pointer_pos(), ps.lane) {
            if lane_r.contains(q) {
                // Right-click in a controller lane removes the nearest move.
                let b = b_of(q.x);
                let before = st.session.clone();
                let ctl = &mut st.session.tracks[ti].midi[ci].controls;
                if let Some(k) = ctl
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.controller == controller)
                    .filter(|(_, e)| ((e.at - b).abs() as f32) * zoom < 8.0)
                    .min_by(|a, c2| (a.1.at - b).abs().total_cmp(&(c2.1.at - b).abs()))
                    .map(|(k, _)| k)
                {
                    ctl.remove(k);
                    st.commit_session("Remove controller move", before);
                }
            }
        }
        if let Some(q) = resp.interact_pointer_pos().filter(|q| !lane_r.contains(*q)) {
            if let Some((i, _, _)) = hit(q) {
                apply(st, ti, ci, &[i], "Delete note", |ns, idx| {
                    ops::remove(ns, idx)
                });
                ps.sel.clear();
            }
        }
    } else if resp.clicked() {
        if let Some(q) = resp.interact_pointer_pos() {
            if grid_r.contains(q) {
                ps.click_at = b_of(q.x).max(0.0);
                match (ps.tool, hit(q)) {
                    (Tool::Erase, Some((i, _, _))) => {
                        apply(st, ti, ci, &[i], "Delete note", |ns, idx| {
                            ops::remove(ns, idx)
                        });
                        ps.sel.clear();
                    }
                    (Tool::Erase, None) => {}
                    (_, Some((i, _, _))) => {
                        if shift {
                            match ps.sel.iter().position(|&x| x == i) {
                                Some(k) => {
                                    ps.sel.remove(k);
                                }
                                None => ps.sel.push(i),
                            }
                        } else {
                            ps.sel = vec![i];
                        }
                    }
                    (Tool::Select, None) => {
                        if !shift {
                            ps.sel.clear();
                        }
                    }
                    (Tool::Draw, None) => {
                        if shift || ps.press_new {
                            // Drawn already, on the press.
                            // Shift-click on empty space never draws.
                        } else {
                            let before = st.session.clone();
                            let i = add_note(
                                st,
                                ps,
                                ti,
                                ci,
                                snap_down(b_of(q.x)).max(0.0),
                                p_of(q.y),
                                step,
                            );
                            ps.sel = vec![i];
                            let n = st.session.tracks[ti].midi[ci].notes[i];
                            if ps.held.is_none() {
                                st.audition_on(ti, n.pitch, n.velocity, n.pan);
                                ps.held = Some(n.pitch);
                                ps.held_until = ui.input(|i| i.time) + 0.3;
                                ui.ctx()
                                    .request_repaint_after(std::time::Duration::from_millis(320));
                            }
                            st.commit_session("Add note", before);
                        }
                    }
                }
            }
        }
    }
    // A note drawn on the press: done when the button comes up (a drag
    // that moved it was finished above).
    if ps.press_new && released {
        ps.press_new = false;
        if ps.drag.is_some() {
            ps.drag = None;
            ps.box_now = None;
        }
        if let Some(b) = ps.before.take() {
            if b != st.session {
                st.commit_session("Add note", b);
            }
        }
    }
    // Whatever was sounding from a press stops when the button comes up.
    let now = ui.input(|i| i.time);
    if released && ps.held.is_some() && ps.drag.is_none() && now >= ps.held_until {
        if let Some(h) = ps.held.take() {
            st.audition_off(h);
        }
    }

    // Notes.
    let gp = ui.painter_at(grid_r);
    for (i, n) in st.session.tracks[ti].midi[ci].notes.iter().enumerate() {
        let r = note_rect(n);
        if r.right() < grid_r.left() || r.left() > grid_r.right() {
            continue;
        }
        let col = channel_color(n.channel);
        let a = (90.0 + n.velocity as f32 * 1.3).min(255.0) as u8;
        let picked = ps.sel.contains(&i);
        gp.rect_filled(
            r,
            Rounding::same(2.0),
            Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), a),
        );
        gp.rect_stroke(
            r,
            Rounding::same(2.0),
            if picked {
                Stroke::new(2.0_f32, Color32::WHITE)
            } else {
                Stroke::new(1.0_f32, Color32::from_black_alpha(120))
            },
        );
        if r.width() > 26.0 && row_h >= 9.0 {
            gp.text(
                r.left_center() + vec2(3.0, 0.0),
                Align2::LEFT_CENTER,
                note_name(n.pitch),
                FontId::proportional(8.0),
                Color32::from_black_alpha(200),
            );
        }
    }
    if let (Some(NoteDrag::Box { b0, p0 }), Some(q)) = (ps.drag, ps.box_now) {
        let a = pos2(x_of(b0), y_of(p0 as f32) + row_h * 0.5);
        let r = Rect::from_two_pos(a, q);
        gp.rect_filled(r, Rounding::ZERO, ca(th.accent, 30));
        gp.rect_stroke(r, Rounding::ZERO, Stroke::new(1.0_f32, c(th.accent)));
    }
    // Hover cursor.
    if let Some(h) = resp.hover_pos() {
        if grid_r.contains(h) && ps.drag.is_none() {
            let icon = match (ps.tool, hit(h)) {
                (Tool::Erase, _) => egui::CursorIcon::NotAllowed,
                (_, Some((_, true, _))) | (_, Some((_, _, true))) => {
                    egui::CursorIcon::ResizeHorizontal
                }
                (_, Some(_)) => egui::CursorIcon::Grab,
                (Tool::Select, None) => egui::CursorIcon::Crosshair,
                _ => egui::CursorIcon::Default,
            };
            ui.ctx().set_cursor_icon(icon);
        }
    }
    // Lane.
    p.rect_filled(lane_r, Rounding::same(th.rounding), c(th.panel));
    p.text(
        lane_r.left_top() - vec2(KEYS_W - 4.0, -2.0),
        Align2::LEFT_TOP,
        match ps.lane {
            Lane::Velocity => "Velocity".to_string(),
            Lane::Pan => "Pan".to_string(),
            Lane::Control(c) => af27_core::midi::ControlEvent::name(c),
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
    if let Lane::Control(controller) = ps.lane {
        // Stepped: each move holds until the next.
        let mut moves: Vec<af27_core::midi::ControlEvent> = st.session.tracks[ti].midi[ci]
            .controls
            .iter()
            .filter(|e| e.controller == controller)
            .copied()
            .collect();
        moves.sort_by(|a, b| a.at.total_cmp(&b.at));
        let y_of_v = |v: f32| lane_r.bottom() - v.clamp(0.0, 1.0) * lane_r.height();
        for (k, e) in moves.iter().enumerate() {
            let x0 = x_of(e.at);
            let x1 = moves.get(k + 1).map_or(x_of(clip_len), |n| x_of(n.at));
            let col = channel_color(e.channel);
            let y = y_of_v(e.value);
            lp.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(1.5_f32, col));
            if let Some(n) = moves.get(k + 1) {
                lp.line_segment(
                    [pos2(x1, y), pos2(x1, y_of_v(n.value))],
                    Stroke::new(1.0_f32, col),
                );
            }
            lp.circle_filled(pos2(x0, y), 2.5, col);
        }
        if let Some(e) = resp
            .hover_pos()
            .filter(|q| lane_r.contains(*q))
            .and_then(|q| {
                let b = b_of(q.x);
                moves.iter().rev().find(|e| e.at <= b).copied()
            })
        {
            lp.text(
                lane_r.right_top() + vec2(-4.0, 2.0),
                Align2::RIGHT_TOP,
                af27_core::midi::ControlEvent::value_text(controller, e.value),
                FontId::proportional(9.0),
                c(th.text_dim),
            );
        }
    }
    for (i, n) in st.session.tracks[ti].midi[ci].notes.iter().enumerate() {
        if matches!(ps.lane, Lane::Control(_)) {
            break;
        }
        let x = x_of(n.start);
        let frac = match ps.lane {
            Lane::Velocity => n.velocity as f32 / 127.0,
            Lane::Pan => (n.pan + 1.0) / 2.0,
            Lane::Control(_) => 0.0,
        };
        let y = lane_r.bottom() - frac * lane_r.height();
        let base = if ps.lane == Lane::Pan {
            lane_r.center().y
        } else {
            lane_r.bottom()
        };
        let col = if ps.sel.contains(&i) {
            Color32::WHITE
        } else {
            channel_color(n.channel)
        };
        lp.line_segment([pos2(x, base), pos2(x, y)], Stroke::new(2.0_f32, col));
        lp.circle_filled(pos2(x, y), 3.0, col);
    }
    // Playhead within the clip.
    let x = x_of(play_at);
    if x >= grid_r.left() && x <= grid_r.right() {
        crate::waveform::playhead(&p, x, grid_r.top(), lane_r.bottom(), &th);
    }
    // Where Paste goes when the playhead is outside the clip.
    if !(0.0..clip_len).contains(&play_at) {
        let x = x_of(ps.click_at);
        if x >= grid_r.left() && x <= grid_r.right() {
            p.line_segment(
                [pos2(x, grid_r.top()), pos2(x, grid_r.top() + 10.0)],
                Stroke::new(2.0_f32, c(th.accent_alt)),
            );
        }
    }
    let count = st.session.tracks[ti].midi[ci].notes.len();
    p.text(
        rect.left_bottom() + vec2(4.0, -4.0),
        Align2::LEFT_BOTTOM,
        format!("{count} notes\n{} picked", ps.sel.len()),
        FontId::proportional(9.0),
        c(th.text_dim),
    );
}

/// Adds a note at `start` (beats in the clip) and `pitch`; returns its index.
fn add_note(
    st: &mut State,
    ps: &mut PianoState,
    ti: usize,
    ci: usize,
    start: f64,
    pitch: i32,
    step: f64,
) -> usize {
    let len = if step > 0.0 {
        step.max(ps.last_len)
    } else {
        ps.last_len
    };
    let ns = &mut st.session.tracks[ti].midi[ci].notes;
    ns.push(MidiNote {
        pitch: pitch.clamp(0, 127) as u8,
        start,
        length: len.max(ops::MIN_NOTE),
        velocity: 100,
        channel: ps.channel,
        pan: 0.0,
    });
    ns.len() - 1
}

fn run_cmd(st: &mut State, ps: &mut PianoState, ti: usize, ci: usize, cmd: PianoCmd, play_at: f64) {
    let len = st.session.tracks[ti].midi[ci].notes.len();
    match cmd {
        PianoCmd::SelectAll => ps.sel = (0..len).collect(),
        PianoCmd::Copy | PianoCmd::Cut => {
            if ps.sel.is_empty() {
                return;
            }
            ps.clipboard = ops::copy(&st.session.tracks[ti].midi[ci].notes, &ps.sel);
            if cmd == PianoCmd::Cut {
                let sel = std::mem::take(&mut ps.sel);
                apply(st, ti, ci, &sel, "Cut notes", |ns, idx| {
                    ops::remove(ns, idx)
                });
            }
        }
        PianoCmd::Delete => {
            if ps.sel.is_empty() {
                return;
            }
            let sel = std::mem::take(&mut ps.sel);
            apply(st, ti, ci, &sel, "Delete notes", |ns, idx| {
                ops::remove(ns, idx)
            });
        }
        PianoCmd::Paste => {
            if ps.clipboard.is_empty() {
                return;
            }
            let clip_len = st.session.tracks[ti].midi[ci].length;
            let at = if (0.0..clip_len).contains(&play_at) {
                play_at
            } else {
                ps.click_at
            };
            let before = st.session.clone();
            let copied = ps.clipboard.clone();
            let clip_start = st.session.tracks[ti].midi[ci].start;
            let new = ops::paste(&mut st.session.tracks[ti].midi[ci].notes, &copied, at);
            // Pasted past the end: the clip grows to the next bar line.
            let end = st.session.tracks[ti].midi[ci]
                .notes
                .iter()
                .map(|n| n.start + n.length)
                .fold(0.0, f64::max);
            if end > clip_len {
                let tempo = &st.session.tempo;
                let abs_end = clip_start + end;
                let (bar, into, _) = tempo.bar_at(abs_end);
                let to = if into < 1e-9 {
                    tempo.bar_start(bar)
                } else {
                    tempo.bar_start(bar + 1)
                };
                st.session.tracks[ti].midi[ci].length = to - clip_start;
            }
            ps.sel = new;
            ps.click_at = at
                + copied
                    .iter()
                    .map(|n| n.start + n.length)
                    .fold(0.0, f64::max);
            st.commit_session("Paste notes", before);
        }
    }
}

fn tools_row(
    ui: &mut egui::Ui,
    st: &mut State,
    ps: &mut PianoState,
    ti: usize,
    ci: usize,
    clip_start: f64,
) {
    let sel = ps.sel.clone();
    let on = if sel.is_empty() {
        "all notes"
    } else {
        "picked notes"
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Velocity").strong());
        ui.add(egui::DragValue::new(&mut ps.vel_set).range(1..=127));
        if ui
            .button("Set")
            .on_hover_text(format!("On the {on}"))
            .clicked()
        {
            let v = ps.vel_set;
            apply(st, ti, ci, &sel, "Set velocity", |ns, idx| {
                ops::set_velocity(ns, idx, v)
            });
        }
        ui.add(
            egui::DragValue::new(&mut ps.vel_scale)
                .range(1.0..=400.0)
                .suffix(" %"),
        );
        if ui
            .button("Scale")
            .on_hover_text("Harder (over 100%) or softer (under), keeping the differences")
            .clicked()
        {
            let k = ps.vel_scale;
            apply(st, ti, ci, &sel, "Scale velocity", |ns, idx| {
                ops::scale_velocity(ns, idx, k)
            });
        }
        ui.label("±");
        ui.add(egui::DragValue::new(&mut ps.vel_rand).range(1.0..=127.0));
        if ui.button("Randomize").clicked() {
            let r = ps.vel_rand;
            let rng = &mut ps.rng;
            apply(st, ti, ci, &sel, "Randomize velocity", |ns, idx| {
                ops::randomize_velocity(ns, idx, r, rng)
            });
        }
        ui.separator();
        ui.label(RichText::new("Pan").strong());
        let mut pan = ps.pan_set * 100.0;
        if ui
            .add(egui::DragValue::new(&mut pan).range(-100.0..=100.0))
            .on_hover_text("-100 is left, 0 the centre, 100 right")
            .changed()
        {
            ps.pan_set = pan / 100.0;
        }
        if ui.button("Set").clicked() {
            let v = ps.pan_set;
            apply(st, ti, ci, &sel, "Set pan", |ns, idx| {
                ops::set_pan(ns, idx, v)
            });
        }
        ui.label("±");
        ui.add(
            egui::DragValue::new(&mut ps.pan_rand)
                .range(1.0..=100.0)
                .suffix(" %"),
        );
        if ui.button("Randomize").clicked() {
            let r = ps.pan_rand / 100.0;
            let rng = &mut ps.rng;
            apply(st, ti, ci, &sel, "Randomize pan", |ns, idx| {
                ops::randomize_pan(ns, idx, r, rng)
            });
        }
        ui.separator();
        if ui
            .button(format!("Channel {}", ps.channel + 1))
            .on_hover_text(format!("Puts the {on} on the channel picked above"))
            .clicked()
        {
            let ch = ps.channel;
            apply(st, ti, ci, &sel, "Set channel", |ns, idx| {
                ops::set_channel(ns, idx, ch)
            });
        }
        ui.separator();
        ui.label("±");
        ui.add(
            egui::DragValue::new(&mut ps.stray_ms)
                .range(1.0..=200.0)
                .suffix(" ms"),
        );
        if ui
            .button("Humanize")
            .on_hover_text(
                "Moves each start a little earlier or later at random, up to the amount set",
            )
            .clicked()
        {
            let bpm = st.session.tempo.seg_at_beat(clip_start).bpm;
            let beats = ps.stray_ms / 1000.0 * bpm / 60.0;
            let rng = &mut ps.rng;
            apply(st, ti, ci, &sel, "Humanize", |ns, idx| {
                ops::humanize(ns, idx, beats, rng)
            });
        }
    });
}

fn quantize_row(
    ui: &mut egui::Ui,
    st: &mut State,
    ps: &mut PianoState,
    ti: usize,
    ci: usize,
    bar_beats: f64,
) {
    let sel = ps.sel.clone();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Quantize").strong());
        grid_combo(ui, "quantize-grid", &mut ps.quant_grid);
        let mut strength = ps.quant.strength * 100.0;
        ui.add(
            egui::DragValue::new(&mut strength)
                .range(0.0..=100.0)
                .suffix(" %")
                .prefix("Strength "),
        );
        ps.quant.strength = strength / 100.0;
        let mut swing = ps.quant.swing * 100.0;
        ui.add(
            egui::DragValue::new(&mut swing)
                .range(0.0..=100.0)
                .suffix(" %")
                .prefix("Swing "),
        )
        .on_hover_text("Delays every second grid line; 100% is the triplet feel");
        ps.quant.swing = swing / 100.0;
        ui.checkbox(&mut ps.quant.starts, "Starts");
        egui::ComboBox::from_id_salt("quantize-ends")
            .selected_text(match ps.quant.ends {
                Ends::Keep => "Ends: keep length",
                Ends::End => "Ends: to the grid",
                Ends::Length => "Lengths: whole steps",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut ps.quant.ends, Ends::Keep, "Ends: keep length");
                ui.selectable_value(&mut ps.quant.ends, Ends::End, "Ends: to the grid");
                ui.selectable_value(&mut ps.quant.ends, Ends::Length, "Lengths: whole steps");
            });
        let what = if sel.is_empty() {
            "Quantize Clip"
        } else {
            "Quantize Picked"
        };
        if ui.button(what).clicked() {
            let q = Quantize {
                grid: ps.quant_grid.beats(bar_beats),
                ..ps.quant
            };
            let clip_start = st.session.tracks[ti].midi[ci].start;
            apply(st, ti, ci, &sel, "Quantize", |ns, idx| {
                q.notes(ns, clip_start, |i| idx.contains(&i))
            });
        }
    });
}

/// Puts a new 4-bar MIDI clip on `track` at `beats` and opens it.
pub fn new_clip(st: &mut State, track: af27_core::TrackId, beats: f64) {
    let before = st.session.clone();
    let id = st.session.alloc_id();
    let start = beats.max(0.0);
    let tempo = &st.session.tempo;
    let bar = tempo.bar_at(start).0;
    let length = tempo.bar_start(bar + 4) - start;
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
            start,
            length,
            notes: Vec::new(),
            muted: false,
            gain_db: 0.0,
            controls: Vec::new(),
            ext_id: None,
        });
        st.piano_clip = Some(id);
        st.selected_clip = Some(id);
        st.want_piano = true;
        st.commit_session("New MIDI clip", before);
    }
}

#[cfg(test)]
mod control_lane_tests {
    use super::{paint_control, shape_control};

    #[test]
    fn controller_values_keep_their_steps() {
        assert_eq!(shape_control(64, 0.7), 1.0);
        assert_eq!(shape_control(64, 0.2), 0.0);
        assert_eq!(shape_control(1, 0.5), 64.0 / 127.0);
        assert_eq!(
            shape_control(129, 0.5),
            (0.5f32 * 16383.0).round() / 16383.0
        );
    }

    #[test]
    fn painting_replaces_the_dragged_span_and_pedals_only_change() {
        let mut c = Vec::new();
        paint_control(&mut c, 1, 0, 0.0, 0.0, 0.2);
        paint_control(&mut c, 1, 0, 0.0, 0.5, 0.4);
        paint_control(&mut c, 1, 0, 0.5, 1.0, 0.6);
        assert_eq!(c.len(), 3);
        // Dragging back over the span gives way to one move at the end.
        paint_control(&mut c, 1, 0, 1.0, 0.25, 0.9);
        assert_eq!(c.len(), 3);
        assert_eq!((c[1].at, c[1].value), (0.25, 0.9));
        // A pedal move that does not change the pedal is not kept.
        paint_control(&mut c, 64, 0, 0.0, 0.0, 1.0);
        paint_control(&mut c, 64, 0, 2.0, 2.0, 1.0);
        assert_eq!(c.iter().filter(|e| e.controller == 64).count(), 1);
    }
}
