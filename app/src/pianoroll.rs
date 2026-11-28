// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

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
//! Ctrl+wheel zooms time, Alt+wheel (or the wheel over the keyboard)
//! makes the keys taller or shorter around the pointer, the middle button
//! pans. The bar at the right edge shows the keys in view: drag its middle
//! to scroll, either end to zoom, double-click to fit the clip's notes.

use egui::{
    pos2, vec2, Align2, Color32, FontId, PointerButton, Rect, RichText, Rounding, Sense, Stroke,
};

use crate::state::State;
use crate::theme::{c, ca};
use af2_core::midi::{note_name, MidiNote};
use af2_core::noteops as ops;
use af2_core::quantize::{Ends, Quantize};

const KEYS_W: f32 = 52.0;
/// The lane under the roll: its height to start with, and its limits.
pub const LANE_H: f32 = 110.0;
const LANE_MIN: f32 = 40.0;
const LANE_MAX: f32 = 600.0;
/// The bar between the roll and the lane, dragged to resize the lane.
const SPLIT_H: f32 = 7.0;
/// How far into a note its ends grab for stretching (at most a third of
/// the note, so its middle still moves it).
const EDGE_IN: f32 = 10.0;
/// How far past a note's ends they still grab for stretching.
const EDGE_OUT: f32 = 6.0;
/// The time ruler across the top of the roll: click or drag on it to move
/// the playhead.
const RULER_H: f32 = 18.0;
/// The transport bar along the bottom.
const TRANSPORT_H: f32 = 34.0;
/// The scroll and zoom bar over the ruler.
const TBAR_H: f32 = 12.0;

/// The bar over the ruler: the whole clip across it, its thumb the part in
/// view. Drag the thumb to scroll, its ends to zoom; a click elsewhere
/// jumps there; double-click shows the whole clip.
fn time_bar(ui: &mut egui::Ui, ps: &mut PianoState, bar: Rect, grid_w: f32, clip_len: f64, th: &crate::theme::Theme) {
    let resp = ui.interact(bar, ui.id().with("piano-time-bar"), Sense::click_and_drag());
    let visible = (grid_w / ps.zoom) as f64;
    let total = clip_len.max(ps.scroll_x + visible).max(1e-6);
    let w = bar.width().max(1.0) as f64;
    let x_of = |b: f64| bar.left() + (b / total * w) as f32;
    let b_of = |x: f32| ((x - bar.left()) as f64 / w * total).clamp(0.0, total);
    let (t0, t1) = (x_of(ps.scroll_x), x_of(ps.scroll_x + visible));
    let thumb = Rect::from_min_max(pos2(t0, bar.top() + 2.0), pos2(t1.max(t0 + 6.0), bar.bottom() - 2.0));
    let set_span = |ps: &mut PianoState, a: f64, b: f64| {
        let len = (b - a).max(1e-3);
        ps.zoom = (grid_w / len as f32).clamp(20.0, 400.0);
        ps.scroll_x = a.max(0.0);
    };
    if resp.double_clicked() {
        set_span(ps, 0.0, clip_len.max(1e-3));
    } else if resp.drag_started() {
        if let Some(q) = ui.input(|i| i.pointer.press_origin()) {
            // 0 move, 1 the start, 2 the end.
            let grab = if (q.x - thumb.left()).abs() <= 5.0 {
                1
            } else if (q.x - thumb.right()).abs() <= 5.0 {
                2
            } else if thumb.contains(q) {
                0
            } else {
                // Outside it: the thumb jumps there first.
                ps.scroll_x = (b_of(q.x) - visible / 2.0).max(0.0);
                0
            };
            ps.tbar_grab = Some((grab, ps.scroll_x, ps.scroll_x + visible));
        }
    } else if resp.clicked() {
        if let Some(q) = resp.interact_pointer_pos() {
            if !thumb.contains(q) {
                ps.scroll_x = (b_of(q.x) - visible / 2.0).max(0.0);
            }
        }
    }
    if let (Some((grab, a0, b0)), true) = (ps.tbar_grab, resp.dragged()) {
        let dx = ui.input(|i| match (i.pointer.press_origin(), i.pointer.latest_pos()) {
            (Some(a), Some(b)) => b.x - a.x,
            _ => 0.0,
        });
        let moved = dx as f64 / w * total;
        match grab {
            1 => set_span(ps, (a0 + moved).clamp(0.0, b0 - 0.25), b0),
            2 => set_span(ps, a0, (b0 + moved).max(a0 + 0.25)),
            _ => ps.scroll_x = (a0 + moved).max(0.0),
        }
    }
    if resp.drag_stopped() {
        ps.tbar_grab = None;
    }
    let hover = resp.hover_pos();
    let near_end = hover.is_some_and(|h| (h.x - thumb.left()).abs() <= 5.0 || (h.x - thumb.right()).abs() <= 5.0);
    if near_end || matches!(ps.tbar_grab, Some((1 | 2, _, _))) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    } else if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    let p = ui.painter_at(bar);
    p.rect_filled(bar, Rounding::ZERO, c(th.panel));
    // Where the clip ends, when the view runs past it.
    let end = x_of(clip_len);
    if end < bar.right() {
        p.rect_filled(Rect::from_min_max(pos2(end, bar.top()), bar.max), Rounding::ZERO, Color32::from_black_alpha(90));
    }
    let hot = resp.hovered() || resp.dragged();
    p.rect_filled(thumb, Rounding::same(3.0), ca(th.accent, if hot { 150 } else { 95 }));
    for x in [thumb.left() + 2.0, thumb.right() - 2.0] {
        p.line_segment([pos2(x, thumb.top() + 2.0), pos2(x, thumb.bottom() - 2.0)], Stroke::new(1.0_f32, c(th.text)));
    }
}

/// What a key on the computer keyboard does in the Piano Roll.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Typed {
    /// A note, in semitones above C of the keyboard's octave.
    Note(i32),
    OctaveDown,
    OctaveUp,
}

/// The computer keyboard as a piano, laid out as in the well-known
/// pattern sequencers: the bottom letter row (Z to /, then ' for F) is one
/// octave and a bit, its sharps on the row above (S D, G H J, L ;); the top
/// row (Q to P, then [ and ]) the next, its sharps on the number row (2 3,
/// 5 6 7, 9 0, =). - sits over E to F, where there is no sharp. Page Down
/// and Page Up move both an octave.
pub fn typed_key(k: egui::Key) -> Option<Typed> {
    use egui::Key::*;
    let n = match k {
        Z => 0,
        S => 1,
        X => 2,
        D => 3,
        C => 4,
        V => 5,
        G => 6,
        B => 7,
        H => 8,
        N => 9,
        J => 10,
        M => 11,
        Comma => 12,
        L => 13,
        Period => 14,
        Semicolon => 15,
        Slash => 16,
        Q => 12,
        Num2 => 13,
        W => 14,
        Num3 => 15,
        E => 16,
        R => 17,
        Num5 => 18,
        T => 19,
        Num6 => 20,
        Y => 21,
        Num7 => 22,
        U => 23,
        I => 24,
        Num9 => 25,
        O => 26,
        Num0 => 27,
        P => 28,
        Quote => 17,
        OpenBracket => 29,
        Equals => 30,
        CloseBracket => 31,
        PageDown => return Some(Typed::OctaveDown),
        PageUp => return Some(Typed::OctaveUp),
        _ => return None,
    };
    Some(Typed::Note(n))
}

/// The MIDI note a typed key plays, with Z at C of `octave` (4: middle C).
fn typed_pitch(octave: i32, n: i32) -> u8 {
    (12 * (octave + 1) + n).clamp(0, 127) as u8
}

/// Plays (and, while recording into this clip, keeps) the notes typed on
/// the computer keyboard; lets them all go when the roll loses the keys.
fn play_typed(st: &mut State, ps: &mut PianoState, ti: usize, ci: usize) {
    use af2_core::midi_live::KeyEvent;
    let track = st.session.tracks[ti].id;
    let into = st.piano_rec_clip == Some(st.session.tracks[ti].midi[ci].id) && st.is_recording();
    let keys = std::mem::take(&mut st.piano_keys);
    if !(ps.focused && ps.typing_on) {
        for (_, pitch, heard) in std::mem::take(&mut ps.typed) {
            st.audition_off(heard, ps.channel);
            if into {
                st.midi_capture(track, KeyEvent::Off { pitch, channel: ps.channel });
            }
        }
        return;
    }
    for (key, down) in keys {
        match typed_key(key) {
            Some(Typed::OctaveDown) if down => ps.kb_octave = (ps.kb_octave - 1).max(-1),
            Some(Typed::OctaveUp) if down => ps.kb_octave = (ps.kb_octave + 1).min(8),
            Some(Typed::Note(n)) => {
                if down {
                    if ps.typed.iter().any(|t| t.0 == key) {
                        continue;
                    }
                    let pitch = typed_pitch(ps.kb_octave, n);
                    // Heard at the clip's pitch, kept as played (playback
                    // transposes it the same way).
                    let shift = st.session.tracks[ti].midi[ci].pitch.round() as i32;
                    let heard = (pitch as i32 + shift).clamp(0, 127) as u8;
                    st.audition_on(ti, heard, ps.kb_velocity, 0.0, ps.channel);
                    ps.typed.push((key, pitch, heard));
                    if into {
                        st.midi_capture(
                            track,
                            KeyEvent::On {
                                pitch,
                                velocity: ps.kb_velocity as f32 / 127.0,
                                channel: ps.channel,
                            },
                        );
                    }
                } else if let Some(k) = ps.typed.iter().position(|t| t.0 == key) {
                    let (_, pitch, heard) = ps.typed.remove(k);
                    // Another key may still hold the same note.
                    if !ps.typed.iter().any(|t| t.2 == heard) {
                        st.audition_off(heard, ps.channel);
                    }
                    if into {
                        st.midi_capture(track, KeyEvent::Off { pitch, channel: ps.channel });
                    }
                }
            }
            _ => {}
        }
    }
}

/// The Piano Roll's transport: the time, Stop, Play, Pause, back to the
/// clip's start, Record into the clip and Loop the clip, the metronome,
/// and the computer keyboard's octave and velocity.
fn transport_row(ui: &mut egui::Ui, st: &mut State, ps: &mut PianoState, ti: usize, ci: usize) {
    use crate::hud::{transport_button as tb, Icon};
    use crate::transport::PlayMode;
    let th = st.theme.clone();
    let clip = &st.session.tracks[ti].midi[ci];
    let (clip_id, clip_start, clip_len) = (clip.id, clip.start, clip.length);
    let tempo = st.session.tempo.clone();
    let span = (tempo.secs(clip_start), tempo.secs(clip_start + clip_len));
    // Scrolls sideways when the panel is too narrow for all of it.
    egui::ScrollArea::horizontal()
        .id_salt("piano-transport")
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let rec = st.is_recording();
        let playing = st.transport_playing();
        let caption = if rec {
            "REC // TIME"
        } else if playing {
            "PLAY // TIME"
        } else if st.paused {
            "PAUSED // TIME"
        } else {
            "TIME"
        };
        let fx = st.fx.clone();
        let t = st.time_ctx().format(st.position());
        crate::hud::readout(ui, &t, caption, &th, &fx, st.meter);
        ui.add_sized(
            vec2(64.0, 26.0),
            egui::Label::new(RichText::new(tempo.format(st.position())).monospace().color(c(th.text_dim))),
        );
        ui.add_space(6.0);
        if tb(ui, Icon::Stop, "Stop (Space)", false, th.text, &th).clicked() {
            if rec {
                st.stop_recording();
            } else {
                st.stop();
            }
        }
        if tb(ui, Icon::Play, "Play (Space)", playing, th.accent, &th).clicked() && !playing {
            st.mode = crate::state::Mode::Multitrack;
            st.play();
        }
        if tb(ui, Icon::Pause, "Pause", st.paused, th.accent, &th).clicked() {
            st.toggle_pause();
        }
        if tb(ui, Icon::ToStart, "To the clip's start", false, th.text, &th).clicked() {
            st.mode = crate::state::Mode::Multitrack;
            st.jump_to(span.0);
        }
        let mine = rec && st.piano_rec_clip == Some(clip_id);
        if tb(
            ui,
            Icon::Record,
            "Record into this clip from the playhead: play the computer keyboard (or an armed MIDI keyboard); Stop puts the notes in",
            mine,
            th.accent_alt,
            &th,
        )
        .clicked()
        {
            if rec {
                st.stop_recording();
            } else {
                st.piano_record_start(clip_id);
            }
        }
        // What a take does to the notes already there.
        let mode = if st.piano_rec_replace { "Replace" } else { "Overdub" };
        if ui
            .button(mode)
            .on_hover_text(
                "Overdub: a take adds to the notes already in the clip. Replace: it clears the notes in the stretch recorded over. Click to switch.",
            )
            .clicked()
        {
            st.piano_rec_replace = !st.piano_rec_replace;
        }
        let looping = st.play_mode() == PlayMode::Loop
            && st
                .session_sel
                .is_some_and(|s| (s.0 - span.0).abs() < 1e-6 && (s.1 - span.1).abs() < 1e-6);
        if tb(ui, Icon::Loop, "Loop this clip", looping, th.accent, &th).clicked() {
            if looping {
                st.set_play_mode(PlayMode::Normal);
            } else {
                st.mode = crate::state::Mode::Multitrack;
                st.session_sel = Some(span);
                st.set_play_mode(PlayMode::Loop);
            }
        }
        let mut m = st.session.tempo.metronome;
        if ui
            .toggle_value(&mut m, "Click")
            .on_hover_text("The click, while playing and recording")
            .changed()
        {
            st.session.tempo.metronome = m;
            st.live_metronome();
        }
        ui.separator();
        ui.toggle_value(&mut ps.typing_on, "Keys").on_hover_text(
            "Play notes on the computer keyboard while the Piano Roll has the keys (click in it first): \
             Z to / and ' is one octave (sharps on S D G H J L ;), Q to ] the next (sharps on the number row and =); \
             Page Down and Page Up change the octave",
        );
        ui.add_enabled_ui(ps.typing_on, |ui| {
            ui.label(RichText::new(format!("C{}", ps.kb_octave)).monospace())
                .on_hover_text("The octave the letter keys start at: Z plays this C (Page Down / Page Up)");
            ui.add(egui::DragValue::new(&mut ps.kb_velocity).range(1..=127).prefix("Vel "))
                .on_hover_text("How hard typed notes play");
        });
        ui.separator();
        // The clip's pitch as three knobs: octaves, semitones and cents. It
        // is heard on playback, and keys played are heard at it too.
        clip_pitch_knobs(ui, st, ps, ti, ci, &th);
    }));
}
/// The Oct, Trans and Fine knobs: the clip's pitch (`MidiClip::pitch`) in
/// octaves, semitones and cents. The knobs keep their own positions (a
/// drag moves them a fraction at a time), taking the clip's pitch again
/// when something else changed it. Each gesture is one undo step.
fn clip_pitch_knobs(ui: &mut egui::Ui, st: &mut State, ps: &mut PianoState, ti: usize, ci: usize, th: &crate::theme::Theme) {
    use af2_core::midi::MIDI_PITCH_MAX;
    use crate::track_views::dial;
    let pitch = st.session.tracks[ti].midi[ci].pitch;
    let composed = |k: &[f32; 3]| (k[0].round() * 12.0 + k[1].round() + k[2].round() / 100.0).clamp(-MIDI_PITCH_MAX, MIDI_PITCH_MAX);
    if (composed(&ps.pitch_knobs) - pitch).abs() > 1e-4 {
        let whole = pitch.round();
        let oct = (whole / 12.0).trunc();
        ps.pitch_knobs = [oct, whole - oct * 12.0, (pitch - whole) * 100.0];
    }
    let span = (MIDI_PITCH_MAX / 12.0).floor();
    let mut k = ps.pitch_knobs;
    // The octave: − and +, one undo step a click.
    let before_oct = k[0];
    if ui.small_button("\u{2212}").on_hover_text("This clip an octave down").clicked() {
        k[0] = (k[0].round() - 1.0).max(-span);
    }
    let oct = ui.label(RichText::new(format!("Oct {:+}", k[0].round() as i32)).monospace());
    let plus = ui.small_button("+").on_hover_text("This clip an octave up");
    if plus.clicked() {
        k[0] = (k[0].round() + 1.0).min(span);
    }
    ps.oct_plus = plus.rect;
    let _ = oct;
    if k[0] != before_oct {
        ps.pitch_knobs = k;
        let before = st.session.clone();
        st.session.tracks[ti].midi[ci].pitch = composed(&k);
        if before != st.session {
            st.commit_session("Clip octave", before);
        }
    }
    let rs = [
        dial(ui, &mut k[1], -12.0..=12.0, 0.0, |v| format!("Trans {:+}", v.round() as i32), th)
            .on_hover_text("Transposes this clip by semitones (double-click: none)"),
        dial(ui, &mut k[2], -50.0..=50.0, 0.0, |v| format!("Fine {:+.0}c", v.round()), th)
            .on_hover_text("Fine-tunes this clip, in cents (double-click: none)"),
    ];
    ps.knob_rects = [rs[0].rect, rs[1].rect];
    if rs.iter().any(|r| r.drag_started()) {
        ps.pitch_before = Some(st.session.clone());
    }
    if rs.iter().any(|r| r.changed()) {
        ps.pitch_knobs = k;
        let before = st.session.clone();
        st.session.tracks[ti].midi[ci].pitch = composed(&k);
        // A double-click: its own step at once.
        if ps.pitch_before.is_none() && before != st.session {
            st.commit_session("Clip pitch", before);
        }
    }
    if rs.iter().any(|r| r.drag_stopped()) || (ps.pitch_before.is_some() && !ui.input(|i| i.pointer.any_down())) {
        if let Some(b) = ps.pitch_before.take() {
            if b != st.session {
                st.commit_session("Clip pitch", b);
            }
        }
    }
}

/// Key height limits, in pixels.
const ROW_MIN: f32 = 6.0;
const ROW_MAX: f32 = 32.0;
/// The keys' zoom bar at the roll's right edge.
const KBAR_W: f32 = 12.0;

/// The keys after `notches` of wheel over them (up: taller), keeping the
/// key `py` points below the roll's top in place: (top, row).
fn zoom_keys(top: f32, row: f32, py: f32, notches: f32) -> (f32, f32) {
    let key = top - py / row;
    let row2 = (row * 1.15f32.powf(notches)).clamp(ROW_MIN, ROW_MAX);
    ((key + py / row2).clamp(12.0, 127.0), row2)
}

/// The keys in view on a roll `h` points tall, as the zoom bar's thumb
/// shows them: (lowest, highest), on a scale of 0..128 keys.
fn keys_in_view(top: f32, row: f32, h: f32) -> (f32, f32) {
    (top + 1.0 - h / row, top + 1.0)
}

/// The keys showing `lo..hi` (or as near as the key height limits
/// allow), centred, on a roll `h` points tall: (top, row).
fn fit_keys(lo: f32, hi: f32, h: f32) -> (f32, f32) {
    let span = (hi - lo).clamp(h / ROW_MAX, h / ROW_MIN);
    let mid = (lo + hi) / 2.0;
    ((mid + span / 2.0 - 1.0).clamp(12.0, 127.0), (h / span).clamp(ROW_MIN, ROW_MAX))
}

/// Which part of the keys' zoom bar is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyGrab {
    /// The top end: the highest key follows, the lowest stays.
    Top,
    /// The bottom end: the lowest key follows, the highest stays.
    Bottom,
    /// The middle: scrolls.
    Body,
}

/// The keys after dragging the zoom bar's `grab` by `dk` keys (up is
/// positive), from a thumb of `lo0..hi0` keys on a roll `h` points tall:
/// (top, row).
fn drag_key_bar(grab: KeyGrab, lo0: f32, hi0: f32, dk: f32, h: f32) -> (f32, f32) {
    let span0 = (hi0 - lo0).max(1e-3);
    let fit = |span: f32| span.clamp(h / ROW_MAX, h / ROW_MIN);
    let (hi, span) = match grab {
        KeyGrab::Body => (hi0 + dk, span0),
        KeyGrab::Top => {
            let span = fit(hi0.min(128.0) + dk - lo0);
            (lo0 + span, span)
        }
        KeyGrab::Bottom => (hi0, fit(hi0 - (lo0 + dk))),
    };
    ((hi - 1.0).clamp(12.0, 127.0), (h / span).clamp(ROW_MIN, ROW_MAX))
}

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
pub(crate) fn shape_control(controller: u8, frac: f32) -> f32 {
    let f = frac.clamp(0.0, 1.0);
    if af2_core::midi::ControlEvent::is_switch(controller) {
        if f >= 0.5 {
            1.0
        } else {
            0.0
        }
    } else if controller == af2_core::midi::PITCH_BEND {
        (f * 16383.0).round() / 16383.0
    } else {
        (f * 127.0).round() / 127.0
    }
}

/// Draws a controller value at beat `b` over the span dragged since
/// `from`: moves in that span give way to the new one. A pedal move is
/// kept only where the value changes.
fn paint_control(
    controls: &mut Vec<af2_core::midi::ControlEvent>,
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
    if af2_core::midi::ControlEvent::is_switch(controller) && before == Some(value) {
        return;
    }
    controls.push(af2_core::midi::ControlEvent {
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
    /// A copy of the picked notes right after them (Ctrl+D).
    Duplicate,
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
    /// Where the time ruler was last drawn.
    ruler_r: Rect,
    /// The time bar's drag: what was grabbed (0 the thumb, 1 its start, 2
    /// its end) and the span in view when it began.
    tbar_grab: Option<(u8, f64, f64)>,
    tool: Tool,
    lane: Lane,
    grid: Grid,
    channel: u8,
    /// The lane's height in pixels (drag the bar above it); saved in
    /// Preferences as piano_lane_h.
    lane_h: f32,
    /// lane_h has been read from Preferences.
    lane_loaded: bool,
    /// The last click landed in the Piano Roll: its keys (Ctrl+C, X, V,
    /// D, A, Delete) act on the notes even with the pointer elsewhere.
    focused: bool,
    /// Pixels per beat.
    zoom: f32,
    /// Key height in pixels.
    row: f32,
    /// The keys' zoom bar being dragged: the part held, the thumb's keys
    /// (lowest, highest) and the pointer's height when it was pressed.
    key_grab: Option<(KeyGrab, f32, f32, f32)>,
    /// First beat shown.
    scroll_x: f64,
    /// Highest pitch shown at the top.
    top: f32,
    drag: Option<NoteDrag>,
    before: Option<af2_core::Session>,
    /// The notes when the gesture began.
    orig: Vec<MidiNote>,
    /// Picked notes, by index.
    sel: Vec<usize>,
    /// Picked before a Shift-box began.
    sel_base: Vec<usize>,
    box_now: Option<egui::Pos2>,
    clip: Option<af2_core::ClipId>,
    last_len: f64,
    clipboard: Vec<MidiNote>,
    /// The last spot clicked, in beats from the clip start.
    click_at: f64,
    /// The pitch sounding from a press (note or key).
    held: Option<u8>,
    /// The computer keyboard plays notes while the roll has the keys.
    typing_on: bool,
    /// The octave of the bottom letter row (Z is C of it; 4: middle C).
    kb_octave: i32,
    /// The clip pitch knobs' own positions (octaves, semitones, cents).
    pitch_knobs: [f32; 3],
    /// The session as a knob drag began (its undo step).
    pitch_before: Option<af2_core::Session>,
    /// Where the Trans and Fine knobs and the octave's + were last drawn
    /// (for tests).
    knob_rects: [Rect; 2],
    oct_plus: Rect,
    /// The velocity typed notes play and record at.
    kb_velocity: u8,
    /// Keys down now: the note each plays (kept as played) and the pitch it
    /// is heard at (the clip's pitch on top).
    typed: Vec<(egui::Key, u8, u8)>,
    /// Its MIDI channel (multi-part instruments sound it with that part).
    held_ch: u8,
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
            ruler_r: Rect::NOTHING,
            tbar_grab: None,
            tool: Tool::Draw,
            lane: Lane::Velocity,
            grid: Grid::Sixteenth,
            channel: 0,
            lane_h: LANE_H,
            lane_loaded: false,
            focused: false,
            zoom: 80.0,
            row: 12.0,
            key_grab: None,
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
            typing_on: true,
            kb_octave: 4,
            pitch_knobs: [0.0; 3],
            pitch_before: None,
            knob_rects: [Rect::NOTHING; 2],
            oct_plus: Rect::NOTHING,
            kb_velocity: 100,
            typed: Vec::new(),
            held_ch: 0,
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

/// A note's colour in the roll: channel 1 has the track's own colour,
/// the others their channel's.
pub fn note_color(track: Color32, ch: u8) -> Color32 {
    if ch == 0 {
        track
    } else {
        channel_color(ch)
    }
}

/// Grabbing note `i` to drag it: a picked note drags every picked note
/// with it; an unpicked one is picked first (alone, or with Shift added
/// to the others).
fn pick_for_drag(sel: &mut Vec<usize>, i: usize, shift: bool) {
    if !sel.contains(&i) {
        if !shift {
            sel.clear();
        }
        sel.push(i);
    }
}

/// Where Duplicate puts the copy: after the picked notes (earliest start
/// `first`, latest end `last_end`), a whole number of grid steps on.
fn duplicate_at(first: f64, last_end: f64, step: f64) -> f64 {
    let span = (last_end - first).max(1e-9);
    if step > 1e-9 {
        first + (span / step - 1e-6).ceil().max(1.0) * step
    } else {
        first + span
    }
}

/// The lane's height: what was asked for, between LANE_MIN and LANE_MAX,
/// leaving the roll above at least 80 points of a `total`-point panel.
pub fn lane_height(want: f32, total: f32) -> f32 {
    let want = if want.is_finite() { want } else { LANE_H };
    let room = (total - SPLIT_H - 80.0).min(LANE_MAX).max(LANE_MIN);
    want.clamp(LANE_MIN, room)
}

/// Whether a point inside note `r` grabs its start or its end: up to
/// EDGE_IN in from each, at most a third of the note so its middle still
/// moves it; a short note's start is grabbed from just outside it instead.
fn grab_ends(r: Rect, q: egui::Pos2) -> (bool, bool) {
    let zone = (r.width() / 3.0).min(EDGE_IN);
    let left = r.width() > 12.0 && q.x - r.left() < zone;
    let right = !left && r.right() - q.x < zone.max(4.0);
    (left, right)
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

/// A channel swatch: channel `ch`'s colour with its number on it.
fn channel_swatch(p: &egui::Painter, r: Rect, track: Color32, ch: u8, strong: bool) {
    p.rect_filled(r, Rounding::same(3.0), note_color(track, ch));
    p.rect_stroke(r, Rounding::same(3.0), Stroke::new(1.0_f32, Color32::from_black_alpha(110)));
    p.text(
        r.center(),
        Align2::CENTER_CENTER,
        format!("{}", ch + 1),
        FontId::proportional(9.5),
        Color32::from_black_alpha(if strong { 240 } else { 190 }),
    );
}

/// The id of the channel picker's list.
fn channel_popup_id(ui: &egui::Ui) -> egui::Id {
    ui.id().with("piano-channel-list")
}

/// The channel picker: closed, it shows the channel's colour and number;
/// open, it lists all 16 with their colours. Returns a channel picked.
fn channel_picker(
    ui: &mut egui::Ui,
    track: Color32,
    current: u8,
    th: &crate::theme::Theme,
) -> Option<u8> {
    let popup = channel_popup_id(ui);
    let open = ui.memory(|m| m.is_popup_open(popup));
    let (r, resp) = ui.allocate_exact_size(vec2(74.0, 20.0), Sense::click());
    let vis = if open {
        ui.visuals().widgets.open
    } else {
        *ui.style().interact(&resp)
    };
    let p = ui.painter();
    p.rect(r, vis.rounding, vis.bg_fill, vis.bg_stroke);
    let sw = Rect::from_min_size(r.min + vec2(3.0, 2.0), vec2(22.0, r.height() - 4.0));
    channel_swatch(p, sw, track, current, true);
    p.text(
        pos2(sw.right() + 5.0, r.center().y),
        Align2::LEFT_CENTER,
        format!("Ch {}", current + 1),
        FontId::proportional(11.0),
        vis.text_color(),
    );
    p.text(
        r.right_center() - vec2(5.0, 0.0),
        Align2::RIGHT_CENTER,
        "\u{25BE}",
        FontId::proportional(10.0),
        vis.text_color(),
    );
    let resp = resp.on_hover_text(format!(
        "Channel {}{}: new notes go here; with notes picked, picking another moves them there. \
         Multi-part instruments play each channel with its own sound.",
        current + 1,
        if current == 0 { " (the track colour)" } else { "" }
    ));
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup));
    }
    let mut picked = None;
    egui::popup_below_widget(
        ui,
        popup,
        &resp,
        egui::PopupCloseBehavior::CloseOnClick,
        |ui| {
            ui.set_min_width(170.0);
            ui.spacing_mut().item_spacing.y = 1.0;
            for ch in 0..16u8 {
                let (row, rr) = ui.allocate_exact_size(vec2(170.0, 20.0), Sense::click());
                let on = ch == current;
                let p = ui.painter();
                if rr.hovered() || on {
                    p.rect_filled(
                        row,
                        Rounding::same(3.0),
                        if rr.hovered() { ca(th.accent, 70) } else { ca(th.accent, 35) },
                    );
                }
                let sw = Rect::from_min_size(row.min + vec2(3.0, 2.0), vec2(22.0, 16.0));
                channel_swatch(p, sw, track, ch, on);
                p.text(
                    pos2(sw.right() + 6.0, row.center().y),
                    Align2::LEFT_CENTER,
                    if ch == 0 {
                        "Channel 1 (track colour)".to_string()
                    } else {
                        format!("Channel {}", ch + 1)
                    },
                    FontId::proportional(11.0),
                    c(th.text),
                );
                if rr.clicked() {
                    picked = Some(ch);
                }
            }
        },
    );
    picked
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
            let t = st.add_track_kind(af2_core::TrackKind::Midi);
            new_clip(st, t, 0.0);
        }
        return;
    };
    if ps.clip != st.piano_clip {
        ps.clip = st.piano_clip;
        st.audition_all_off();
        ps.held = None;
        ps.typed.clear();
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

    // Toolbar: two rows of labelled groups. The first edits (tool, grid,
    // clipboard, the Tools and Quantize rows, zoom); the second says what
    // new notes get and what the lane under the roll shows.
    let tc = st.session.tracks[ti].color;
    let track_col = Color32::from_rgb(tc[0], tc[1], tc[2]);
    let caption = |ui: &mut egui::Ui, text: &str| {
        ui.label(RichText::new(text).small().color(c(th.text_dim)));
    };
    ui.spacing_mut().item_spacing.x = 6.0;
    ui.horizontal(|ui| {
        let name = st.session.tracks[ti].midi[ci].name.clone();
        ui.label(
            RichText::new(format!("{} • {}", st.session.tracks[ti].name, name))
                .strong()
                .color(c(th.accent)),
        );
        ui.separator();
        caption(ui, "TOOL");
        ui.selectable_value(&mut ps.tool, Tool::Draw, "✏ Draw")
            .on_hover_text("Click empty space to add a note; keep the button down to move it");
        ui.selectable_value(&mut ps.tool, Tool::Select, "⬚ Select")
            .on_hover_text(
                "Click a note to pick it; drag on empty space to pick every note in a box",
            );
        ui.selectable_value(&mut ps.tool, Tool::Erase, "⌫ Erase")
            .on_hover_text("Click or drag over notes to remove them");
        ui.separator();
        caption(ui, "GRID");
        grid_combo(ui, "piano-grid", &mut ps.grid);
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
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("+").on_hover_text("Taller keys").clicked() {
                ps.row = (ps.row + 2.0).min(32.0);
            }
            if ui.small_button("−").on_hover_text("Shorter keys").clicked() {
                ps.row = (ps.row - 2.0).max(6.0);
            }
            caption(ui, "KEYS");
            ui.add_space(6.0);
            ui.add(
                crate::widgets::Slider::new(&mut ps.zoom, 20.0..=400.0)
                    .logarithmic(true)
                    .show_value(false),
            )
            .on_hover_text("Time zoom (Ctrl+wheel)");
            caption(ui, "ZOOM");
        });
    });
    ui.horizontal(|ui| {
        caption(ui, "CHANNEL");
        // A dropdown of the 16 MIDI channels, each in its notes' colour.
        // Picking one sets where new notes go, and moves the picked notes
        // there: instruments with several parts play each channel with
        // its own.
        if let Some(ch) = channel_picker(ui, track_col, ps.channel, &th) {
            ps.channel = ch;
            if !ps.sel.is_empty() {
                let sel = ps.sel.clone();
                apply(st, ti, ci, &sel, "Set channel", |ns, idx| {
                    ops::set_channel(ns, idx, ch)
                });
            }
        }
        ui.separator();
        caption(ui, "LANE");
        ui.selectable_value(&mut ps.lane, Lane::Velocity, "Velocity");
        ui.selectable_value(&mut ps.lane, Lane::Pan, "Pan");
        {
            use af2_core::midi::{ControlEvent, LANE_CONTROLLERS};
            let used: Vec<u8> = st.session.tracks[ti].midi[ci]
                .controls
                .iter()
                .map(|e| e.controller)
                .collect();
            let shown = match ps.lane {
                Lane::Control(c) => ControlEvent::name(c),
                _ => "Controller…".to_string(),
            };
            let mut list: Vec<u8> = LANE_CONTROLLERS.to_vec();
            for c in &used {
                if !list.contains(c) {
                    list.push(*c);
                }
            }
            egui::ComboBox::from_id_salt("piano-ctl-lane")
                .selected_text(shown)
                .width(120.0)
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
    });
    if ps.show_tools {
        tools_row(ui, st, ps, ti, ci, clip_start);
    }
    if ps.show_quantize {
        quantize_row(ui, st, ps, ti, ci, bar_beats);
    }

    // A click anywhere in the Piano Roll gives it the keys; one elsewhere
    // takes them away.
    if let Some(q) = ui.input(|i| {
        if i.pointer.any_pressed() {
            i.pointer.press_origin()
        } else {
            None
        }
    }) {
        let here = ui.max_rect().contains(q) && ui.clip_rect().contains(q);
        // A click in a floating window (the instrument's settings, an
        // effect) leaves the keys with the roll, so they still play it.
        let on_window = ui
            .ctx()
            .layer_id_at(q)
            .is_some_and(|l| l.order == egui::Order::Middle && l != ui.layer_id());
        if here {
            ps.focused = true;
        } else if !on_window {
            ps.focused = false;
        }
    }
    if ps.focused {
        st.piano_hover = true;
    }
    if ps.focused && ps.typing_on {
        st.piano_typing = true;
    }
    play_typed(st, ps, ti, ci);
    // The transport along the bottom, as under the Editor.
    let avail = ui.available_rect_before_wrap();
    let bar_r = Rect::from_min_max(pos2(avail.left(), avail.bottom() - TRANSPORT_H), avail.max);
    let full = Rect::from_min_max(avail.min, pos2(avail.right(), bar_r.top()));
    let (rect, resp) = ui.allocate_exact_size(full.size(), Sense::click_and_drag());
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(bar_r.shrink2(vec2(2.0, 3.0))), |ui| {
        transport_row(ui, st, ps, ti, ci);
    });
    if resp.hovered() || resp.dragged() {
        st.piano_hover = true;
    }
    let row_h = ps.row;
    if !ps.lane_loaded {
        ps.lane_loaded = true;
        ps.lane_h = st.prefs.piano_lane_h;
    }
    // The lane keeps at least LANE_MIN, and the roll at least 80 points.
    ps.lane_h = lane_height(ps.lane_h, rect.height());
    let grid_r = Rect::from_min_max(
        pos2(rect.left() + KEYS_W, rect.top() + TBAR_H + RULER_H),
        pos2(rect.right() - KBAR_W, rect.bottom() - ps.lane_h - SPLIT_H),
    );
    let keys_r = Rect::from_min_max(pos2(rect.left(), grid_r.top()), pos2(grid_r.left(), grid_r.bottom()));
    let tbar_r = Rect::from_min_max(pos2(grid_r.left(), rect.top()), pos2(grid_r.right(), rect.top() + TBAR_H));
    let ruler_r = Rect::from_min_max(pos2(grid_r.left(), tbar_r.bottom()), pos2(grid_r.right(), grid_r.top()));
    time_bar(ui, ps, tbar_r, grid_r.width(), clip_len, &th);
    let ruler = ui
        .interact(ruler_r, ui.id().with("piano-ruler"), Sense::click_and_drag())
        .on_hover_text("Click or drag to move the playhead");
    if ruler.hovered() || ruler.dragged() {
        st.piano_hover = true;
    }
    ps.ruler_r = ruler_r;
    let lane_r = Rect::from_min_max(
        pos2(grid_r.left(), grid_r.bottom() + SPLIT_H),
        pos2(grid_r.right(), rect.bottom()),
    );
    // The keys' zoom bar beside the roll.
    let kbar_r = Rect::from_min_max(pos2(grid_r.right(), grid_r.top()), pos2(rect.right(), grid_r.bottom()));
    let kbar = ui.interact(kbar_r, ui.id().with("piano-key-bar"), Sense::click_and_drag());
    // The bar between them: drag it to make the lane taller or shorter
    // (double-click: back to the usual height).
    let split_r = Rect::from_min_max(
        pos2(rect.left(), grid_r.bottom()),
        pos2(rect.right(), lane_r.top()),
    );
    let split = ui
        .interact(split_r, ui.id().with("piano-lane-split"), Sense::click_and_drag())
        .on_hover_cursor(egui::CursorIcon::ResizeVertical);
    if split.dragged() {
        ps.lane_h -= split.drag_delta().y;
    }
    if split.double_clicked() {
        ps.lane_h = LANE_H;
    }
    // Remembered for next time (the drawn height stays clamped to this
    // panel; the saved one is what was asked for).
    if split.drag_stopped() || split.double_clicked() {
        let h = lane_height(ps.lane_h, f32::INFINITY);
        if (st.prefs.piano_lane_h - h).abs() > 0.5 {
            st.prefs.piano_lane_h = h;
            st.prefs.save();
        }
    }
    {
        let p = ui.painter();
        let hot = split.hovered() || split.dragged();
        p.rect_filled(
            split_r.shrink2(vec2(0.0, 1.0)),
            Rounding::ZERO,
            c(if hot { th.border } else { th.panel }),
        );
        let m = split_r.center();
        for k in -2..=2 {
            p.circle_filled(pos2(m.x + k as f32 * 5.0, m.y), 1.2, c(th.text_dim));
        }
    }

    // The ruler, as in sessions: the wheel zooms in time at the pointer,
    // Shift+wheel (or a sideways wheel) scrolls in time, a middle-button
    // drag pans.
    if let Some(h) = ruler.hover_pos() {
        let (sc, shift) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers.shift));
        if shift || sc.x != 0.0 {
            let d = if shift { sc.y + sc.x } else { sc.x };
            ps.scroll_x = (ps.scroll_x - d as f64 / ps.zoom as f64).clamp(0.0, clip_len.max(1.0));
        } else if sc.y != 0.0 {
            let notches = sc.y / crate::zoom::NOTCH;
            let px = (h.x - grid_r.left()).max(0.0);
            let anchor = ps.scroll_x + (px / ps.zoom) as f64;
            ps.zoom = (ps.zoom * 1.2f32.powf(notches)).clamp(20.0, 400.0);
            ps.scroll_x = (anchor - (px / ps.zoom) as f64).max(0.0);
        }
    }
    if ruler.dragged_by(PointerButton::Middle) {
        let d = ruler.drag_delta();
        ps.scroll_x = (ps.scroll_x - d.x as f64 / ps.zoom as f64).clamp(0.0, clip_len.max(1.0));
    }
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
        } else if (alt && (sc.y != 0.0 || sc.x != 0.0))
            || (keys_r.contains(h) && !shift && sc.y != 0.0)
        {
            // Keys taller or shorter (Alt+wheel, or the wheel over the
            // keyboard, as over the Multitrack's track headers), keeping
            // the key under the pointer in place.
            let d = if sc.y != 0.0 { sc.y } else { sc.x };
            let py = h.y - grid_r.top();
            (ps.top, ps.row) = zoom_keys(ps.top, ps.row, py, d / crate::zoom::NOTCH);
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
    key_bar(ui, st, ps, &kbar, kbar_r, grid_r.height(), ti, ci);
    let row_h = ps.row;
    let (scroll_x, zoom, top) = (ps.scroll_x, ps.zoom, ps.top);
    let x_of = |b: f64| grid_r.left() + ((b - scroll_x) as f32) * zoom;
    let b_of = |x: f32| scroll_x + ((x - grid_r.left()) / zoom) as f64;
    let y_of = |p: f32| grid_r.top() + (top - p) * row_h;
    let p_of = |y: f32| (top - ((y - grid_r.top()) / row_h).floor()).round() as i32;
    let free = ui.input(|i| i.modifiers.alt) || step <= 0.0;
    let snap_down = |b: f64| if free { b } else { (b / step).floor() * step };
    let snap_near = |b: f64| if free { b } else { (b / step).round() * step };

    // The ruler moves the playhead, within the clip.
    if (ruler.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down())) || ruler.clicked() {
        if let Some(q) = ruler.interact_pointer_pos() {
            let b = b_of(q.x.clamp(grid_r.left(), grid_r.right())).clamp(0.0, clip_len);
            let t = tempo.secs(clip_start + b);
            // The clip's playhead is the session's.
            if st.mode != crate::state::Mode::Multitrack {
                st.mode = crate::state::Mode::Multitrack;
            }
            if (t - st.position()).abs() > 1e-4 {
                st.jump_to(t);
            }
        }
    }

    let p = ui.painter_at(rect);
    p.rect_filled(grid_r, Rounding::ZERO, c(th.wave_background));
    // The ruler and the corner over the keys, edged below (the time bar
    // over the ruler paints itself).
    p.rect_filled(
        Rect::from_min_max(pos2(rect.left(), ruler_r.top()), pos2(rect.right(), grid_r.top())),
        Rounding::ZERO,
        c(th.panel),
    );
    p.rect_filled(
        Rect::from_min_max(rect.min, pos2(ruler_r.left(), ruler_r.top())),
        Rounding::ZERO,
        c(th.panel),
    );
    p.line_segment(
        [pos2(rect.left(), grid_r.top() - 0.5), pos2(rect.right(), grid_r.top() - 0.5)],
        Stroke::new(1.0_f32, c(th.border)),
    );
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
        let down = ps.held == Some(pitch as u8) || ps.typed.iter().any(|t| t.1 == pitch as u8);
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
                // Its tick on the ruler: bars tall, beats short.
                if x >= ruler_r.left() && x <= ruler_r.right() {
                    let tick = if is_bar { RULER_H - 4.0 } else { 5.0 };
                    p.line_segment(
                        [pos2(x, grid_r.top() - tick), pos2(x, grid_r.top())],
                        Stroke::new(1.0_f32, c(if is_bar { th.text } else { th.text_dim })),
                    );
                }
                if is_bar && x >= grid_r.left() - 1.0 {
                    let label = if seg.bar == bar && seg.is_ramp() {
                        let word = if seg.end_bpm < seg.bpm {
                            "rit."
                        } else {
                            "accel."
                        };
                        format!("{}  {word} {}→{}", bar + 1, seg.bpm, seg.end_bpm)
                    } else if seg.bar == bar && seg.change.is_some() {
                        format!("{}  {} {}", bar + 1, seg.meter_text(), seg.bpm)
                    } else {
                        format!("{}", bar + 1)
                    };
                    p.text(
                        pos2(x + 3.0, ruler_r.top() + 2.0),
                        Align2::LEFT_TOP,
                        label,
                        FontId::proportional(10.0),
                        c(th.text),
                    );
                }
            }
        }
    }
    let end_x = x_of(clip_len);
    if end_x < grid_r.right() {
        p.rect_filled(
            Rect::from_min_max(pos2(end_x.max(grid_r.left()), ruler_r.top()), grid_r.max),
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
    // A note's ends also grab a little past it, except for Erase (a click
    // beside a note should not remove it).
    let reach = ps.tool != Tool::Erase;
    let hit = |q: egui::Pos2| -> Option<(usize, bool, bool)> {
        let inside = notes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, n)| note_rect(n).contains(q))
            .map(|(i, n)| {
                let (left, right) = grab_ends(note_rect(n), q);
                (i, left, right)
            });
        if inside.is_some() || !reach {
            return inside;
        }
        notes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, n)| {
                let r = note_rect(n);
                if q.y < r.top() || q.y > r.bottom() {
                    None
                } else if q.x < r.left() && r.left() - q.x <= EDGE_OUT {
                    Some((i, true, false))
                } else if q.x > r.right() && q.x - r.right() <= EDGE_OUT {
                    Some((i, false, true))
                } else {
                    None
                }
            })
    };
    let shift = ui.input(|i| i.modifiers.shift);
    // Not a press on a popup or window over the roll.
    let press = ui.input(|i| i.pointer.primary_pressed())
        && ui
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|q| crate::widgets::press_is_here(ui, q));
    let released = ui.input(|i| !i.pointer.primary_down());

    // Keys play while held.
    if press {
        if let Some(q) = ui.input(|i| i.pointer.interact_pos()) {
            if keys_r.contains(q) {
                let pitch = p_of(q.y).clamp(0, 127) as u8;
                hold(st, ps, ti, pitch, 100, 0.0, ps.channel);
            } else if grid_r.contains(q) && ps.tool != Tool::Erase {
                if let Some((i, _, _)) = hit(q) {
                    let n = notes[i];
                    hold(st, ps, ti, n.pitch, n.velocity, n.pan, n.channel);
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
                        st.audition_off(h, ps.held_ch);
                    }
                    hold(st, ps, ti, n.pitch, n.velocity, n.pan, n.channel);
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
        // Where the button went down decides what the drag does: by the
        // time a drag is reported the pointer has moved a few pixels, off
        // a short note's edge or the note itself.
        if let Some(q) = ui
            .input(|i| i.pointer.press_origin())
            .filter(|q| resp.rect.contains(*q))
            .or(resp.interact_pointer_pos())
        {
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
                        pick_for_drag(&mut ps.sel, i, shift);
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
                        hold(st, ps, ti, n.pitch, n.velocity, n.pan, n.channel);
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
                                st.audition_off(h, ps.held_ch);
                            }
                            hold(st, ps, ti, n.pitch, n.velocity, n.pan, n.channel);
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
                                hold(st, ps, ti, n.pitch, n.velocity, n.pan, n.channel);
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
    if released && ps.held.is_some() && now >= ps.held_until {
        if let Some(h) = ps.held.take() {
            st.audition_off(h, ps.held_ch);
        }
    }

    // Notes.
    let gp = ui.painter_at(grid_r);
    for (i, n) in st.session.tracks[ti].midi[ci].notes.iter().enumerate() {
        let r = note_rect(n);
        if r.right() < grid_r.left() || r.left() > grid_r.right() {
            continue;
        }
        let col = note_color(track_col, n.channel);
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
        // The velocity, as a bar along the note's foot.
        if r.height() >= 8.0 && r.width() > 6.0 {
            let w = (r.width() - 4.0) * n.velocity as f32 / 127.0;
            let foot = Rect::from_min_size(
                pos2(r.left() + 2.0, r.bottom() - 3.5),
                vec2(w.max(1.0), 2.0),
            );
            gp.rect_filled(foot, Rounding::same(1.0), Color32::from_black_alpha(110));
        }
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
            Lane::Control(c) => af2_core::midi::ControlEvent::name(c),
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
        let mut moves: Vec<af2_core::midi::ControlEvent> = st.session.tracks[ti].midi[ci]
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
            let col = note_color(track_col, e.channel);
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
                af2_core::midi::ControlEvent::value_text(controller, e.value),
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
            note_color(track_col, n.channel)
        };
        lp.line_segment([pos2(x, base), pos2(x, y)], Stroke::new(2.0_f32, col));
        lp.circle_filled(pos2(x, y), 3.0, col);
    }
    // Playhead within the clip.
    let x = x_of(play_at);
    if x >= grid_r.left() && x <= grid_r.right() {
        crate::waveform::playhead(&p, x, ruler_r.top(), lane_r.bottom(), &th);
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

/// The keys' zoom bar at the roll's right edge (`bar`, beside a roll
/// `h` points tall): its thumb is the keys in view. Drag the middle to
/// scroll, an end to zoom (the other end stays), click off the thumb to
/// jump there, double-click to fit the clip's notes; the wheel scrolls.
#[allow(clippy::too_many_arguments)]
fn key_bar(
    ui: &egui::Ui,
    st: &State,
    ps: &mut PianoState,
    resp: &egui::Response,
    bar: Rect,
    h: f32,
    ti: usize,
    ci: usize,
) {
    let th = &st.theme;
    let bh = bar.height().max(1.0);
    let y_of = |k: f32| bar.top() + (128.0 - k) / 128.0 * bh;
    let k_of = |y: f32| 128.0 - (y - bar.top()) / bh * 128.0;
    if resp.hovered() {
        let sc = ui.input(|i| i.smooth_scroll_delta.y);
        if sc != 0.0 {
            ps.top = (ps.top + sc / ps.row).clamp(12.0, 127.0);
        }
    }
    if resp.double_clicked() {
        let notes = &st.session.tracks[ti].midi[ci].notes;
        if notes.is_empty() {
            ps.row = 12.0;
        } else {
            let lo = notes.iter().map(|n| n.pitch).min().unwrap_or(60) as f32 - 2.0;
            let hi = notes.iter().map(|n| n.pitch).max().unwrap_or(60) as f32 + 3.0;
            (ps.top, ps.row) = fit_keys(lo, hi, h);
        }
        ps.key_grab = None;
        return;
    }
    let (lo, hi) = keys_in_view(ps.top, ps.row, h);
    let (t, b) = (y_of(hi.min(128.0)), y_of(lo.max(0.0)));
    if resp.drag_started() {
        // Where the press began: a drag is only seen once the pointer has
        // moved, by then maybe off a narrow end.
        if let Some(p) = ui
            .input(|i| i.pointer.press_origin())
            .or(resp.interact_pointer_pos())
        {
            let grip = ((b - t) * 0.2).clamp(4.0, 10.0);
            let grab = if (p.y - t).abs() < grip {
                KeyGrab::Top
            } else if (p.y - b).abs() < grip {
                KeyGrab::Bottom
            } else {
                if p.y < t || p.y > b {
                    // Off the thumb: centre it there, then drag it.
                    let half = (hi - lo) / 2.0;
                    ps.top = (k_of(p.y) + half - 1.0).clamp(12.0, 127.0);
                }
                KeyGrab::Body
            };
            let (lo, hi) = keys_in_view(ps.top, ps.row, h);
            ps.key_grab = Some((grab, lo, hi, p.y));
        }
    } else if resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos().filter(|p| p.y < t || p.y > b) {
            let half = (hi - lo) / 2.0;
            ps.top = (k_of(p.y) + half - 1.0).clamp(12.0, 127.0);
        }
    }
    if let (Some((grab, lo0, hi0, y0)), Some(p)) = (ps.key_grab, resp.interact_pointer_pos()) {
        if resp.dragged() {
            let dk = (y0 - p.y) / bh * 128.0;
            (ps.top, ps.row) = drag_key_bar(grab, lo0, hi0, dk, h);
        }
    }
    if resp.drag_stopped() {
        ps.key_grab = None;
    }
    // Drawn with the keys as they now are.
    let (lo, hi) = keys_in_view(ps.top, ps.row, h);
    let thumb = Rect::from_min_max(
        pos2(bar.left() + 2.0, y_of(hi.min(128.0))),
        pos2(bar.right() - 2.0, y_of(lo.max(0.0)).max(y_of(hi.min(128.0)) + 8.0)),
    );
    let hot = resp.hovered() || resp.dragged();
    let p = ui.painter_at(bar);
    p.rect_filled(bar, Rounding::ZERO, c(th.panel));
    // Each C as a tick, so the bar reads as the keyboard.
    for oct in 0..=10 {
        let y = y_of(oct as f32 * 12.0);
        p.line_segment([pos2(bar.left() + 3.0, y), pos2(bar.right() - 3.0, y)], Stroke::new(1.0_f32, ca(th.border, 160)));
    }
    p.rect_filled(thumb, Rounding::same(3.0), ca(th.accent, if hot { 150 } else { 95 }));
    // Grips at the ends: drag them to zoom.
    for y in [thumb.top() + 3.0, thumb.bottom() - 3.0] {
        p.line_segment(
            [pos2(thumb.left() + 2.0, y), pos2(thumb.right() - 2.0, y)],
            Stroke::new(1.5_f32, ca(th.text, if hot { 230 } else { 150 })),
        );
    }
    if hot {
        let near_end = resp.hover_pos().is_some_and(|q| {
            let grip = (thumb.height() * 0.2).clamp(4.0, 10.0);
            (q.y - thumb.top()).abs() < grip || (q.y - thumb.bottom()).abs() < grip
        });
        let held_end = matches!(ps.key_grab, Some((KeyGrab::Top | KeyGrab::Bottom, ..)));
        if near_end || held_end {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
    }
    if !resp.dragged() {
        let _ = resp.clone().on_hover_text(
            "The keys in view: drag the middle to scroll, an end to zoom; double-click to fit the notes",
        );
    }
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
        PianoCmd::Duplicate => {
            if ps.sel.is_empty() {
                return;
            }
            let notes = &st.session.tracks[ti].midi[ci].notes;
            let picked = || ps.sel.iter().filter_map(|&i| notes.get(i));
            let first = picked().map(|n| n.start).fold(f64::MAX, f64::min);
            let last_end = picked().map(|n| n.start + n.length).fold(f64::MIN, f64::max);
            let copied = ops::copy(notes, &ps.sel);
            let clip_start = st.session.tracks[ti].midi[ci].start;
            let bar_beats = st.session.tempo.seg_at_beat(clip_start).bar_beats();
            let at = duplicate_at(first, last_end, ps.grid.beats(bar_beats));
            paste_notes(st, ps, ti, ci, &copied, at, "Duplicate notes");
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
            let copied = ps.clipboard.clone();
            paste_notes(st, ps, ti, ci, &copied, at, "Paste notes");
        }
    }
}

/// Adds `copied` at `at` (beats into the clip) as one undo step, picks
/// them, and grows the clip to the next bar line when they run past it.
fn paste_notes(
    st: &mut State,
    ps: &mut PianoState,
    ti: usize,
    ci: usize,
    copied: &[MidiNote],
    at: f64,
    what: &str,
) {
    {
            let clip_len = st.session.tracks[ti].midi[ci].length;
            let before = st.session.clone();
            let clip_start = st.session.tracks[ti].midi[ci].start;
            let new = ops::paste(&mut st.session.tracks[ti].midi[ci].notes, copied, at);
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
            st.commit_session(what, before);
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
            let bpm = st.session.tempo.bpm_at(clip_start);
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
pub fn new_clip(st: &mut State, track: af2_core::TrackId, beats: f64) {
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
        t.midi.push(af2_core::midi::MidiClip {
            id,
            name: format!("MIDI {n}"),
            start,
            length,
            notes: Vec::new(),
            timing: Default::default(),
            stretch: 1.0,
            pitch: 0.0,
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
    use super::{lane_height, paint_control, shape_control, LANE_H, LANE_MAX, LANE_MIN, SPLIT_H};

    #[test]
    fn dragging_a_picked_note_drags_every_picked_note() {
        use super::pick_for_drag;
        use af2_core::noteops::move_from;
        let note = |start, pitch| af2_core::midi::MidiNote {
            start,
            length: 1.0,
            pitch,
            velocity: 100,
            channel: 3,
            pan: 0.0,
        };
        let orig = vec![note(0.0, 60), note(1.0, 64), note(2.0, 67)];
        // Grabbing a picked note keeps the whole pick.
        let mut sel = vec![0, 2];
        pick_for_drag(&mut sel, 2, false);
        assert_eq!(sel, vec![0, 2]);
        let mut ns = orig.clone();
        move_from(&mut ns, &orig, &sel, 1.5, -2);
        assert_eq!(
            ns.iter().map(|n| (n.start, n.pitch, n.channel)).collect::<Vec<_>>(),
            vec![(1.5, 58, 3), (1.0, 64, 3), (3.5, 65, 3)]
        );
        // An unpicked one: alone, or added with Shift.
        pick_for_drag(&mut sel, 1, false);
        assert_eq!(sel, vec![1]);
        pick_for_drag(&mut sel, 0, true);
        assert_eq!(sel, vec![1, 0]);
    }

    #[test]
    fn duplicate_lands_right_after_the_pick_on_the_grid() {
        use super::duplicate_at;
        // Notes 0..1.5 beats on a quarter grid: the copy starts at 2.
        assert_eq!(duplicate_at(0.0, 1.5, 1.0), 2.0);
        // Exactly a bar long: the next bar.
        assert_eq!(duplicate_at(4.0, 8.0, 1.0), 8.0);
        // Shorter than a step: one step on.
        assert_eq!(duplicate_at(2.0, 2.25, 1.0), 3.0);
        // No grid: straight after.
        assert_eq!(duplicate_at(1.0, 1.75, 0.0), 1.75);
    }

    #[test]
    fn the_lane_height_stays_within_its_limits() {
        // A roomy panel: what was dragged to, within LANE_MIN..LANE_MAX.
        assert_eq!(lane_height(200.0, 900.0), 200.0);
        assert_eq!(lane_height(5.0, 900.0), LANE_MIN);
        assert_eq!(lane_height(5000.0, 2000.0), LANE_MAX);
        // The roll above keeps 80 points.
        assert_eq!(lane_height(400.0, 300.0), 300.0 - SPLIT_H - 80.0);
        // A tiny panel still shows the lane's minimum; junk falls back.
        assert_eq!(lane_height(200.0, 50.0), LANE_MIN);
        assert_eq!(lane_height(f32::NAN, 900.0), LANE_H);
        // What gets saved: the clamp without a panel.
        assert_eq!(lane_height(250.0, f32::INFINITY), 250.0);
    }

    #[test]
    fn the_lane_height_and_velocity_curve_are_saved_with_the_prefs() {
        let mut p = crate::settings::Prefs::default();
        assert_eq!(p.piano_lane_h, LANE_H);
        use af2_core::midi_live::VelocityMap;
        p.piano_lane_h = 260.0;
        p.midi_velocity_map = VelocityMap::preset("S-curve").unwrap();
        let back: crate::settings::Prefs =
            serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.piano_lane_h, 260.0);
        assert_eq!(back.midi_velocity_map, p.midi_velocity_map);
        // Older prefs files without them get the defaults.
        let old: crate::settings::Prefs = serde_json::from_str("{}").unwrap();
        assert_eq!(old.piano_lane_h, LANE_H);
        assert_eq!(old.midi_velocity_map, VelocityMap::linear());
        // A file from before the map: its single curve becomes the map.
        let mut older: crate::settings::Prefs =
            serde_json::from_str(r#"{"midi_velocity_curve":0.6}"#).unwrap();
        older.carry_old_velocity_curve();
        assert_eq!(older.midi_velocity_map, VelocityMap::preset("Soft").unwrap());
        assert_eq!(older.midi_velocity_curve, 0.0);
    }

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

/// Sounds `pitch` as the held note, stopping whatever was held before (a
/// note left sounding by a missed release would otherwise stay on).
fn hold(
    st: &mut State,
    ps: &mut PianoState,
    ti: usize,
    pitch: u8,
    velocity: u8,
    pan: f32,
    channel: u8,
) {
    if let Some(h) = ps.held.take() {
        st.audition_off(h, ps.held_ch);
    }
    st.audition_on(ti, pitch, velocity, pan, channel);
    ps.held = Some(pitch);
    ps.held_ch = channel;
}

#[cfg(test)]
mod key_zoom_tests {
    use super::*;
    use egui::{Event, PointerButton};

    #[test]
    fn wheel_zoom_keeps_the_key_under_the_pointer() {
        let (top, row) = zoom_keys(84.0, 12.0, 120.0, 1.0);
        assert!(row > 12.0 && row <= ROW_MAX);
        // The key 120 points down stays 120 points down.
        assert!(((84.0 - 120.0 / 12.0) - (top - 120.0 / row)).abs() < 1e-4);
        // Limits hold.
        assert_eq!(zoom_keys(84.0, 30.0, 0.0, 10.0).1, ROW_MAX);
        assert_eq!(zoom_keys(84.0, 7.0, 0.0, -10.0).1, ROW_MIN);
    }

    #[test]
    fn the_key_bar_scrolls_from_its_middle_and_zooms_from_its_ends() {
        let h = 480.0;
        let (lo, hi) = keys_in_view(84.0, 12.0, h);
        assert_eq!((lo, hi), (45.0, 85.0));
        // Middle up 10 keys: scrolls, same height.
        assert_eq!(drag_key_bar(KeyGrab::Body, lo, hi, 10.0, h), (94.0, 12.0));
        // Top end up 20 keys: 60 keys in view, the bottom stays.
        let (top, row) = drag_key_bar(KeyGrab::Top, lo, hi, 20.0, h);
        assert_eq!(row, 8.0);
        assert_eq!(keys_in_view(top, row, h), (45.0, 105.0));
        // Bottom end up 20 keys: 20 keys in view, the top stays.
        let (top, row) = drag_key_bar(KeyGrab::Bottom, lo, hi, 20.0, h);
        assert_eq!((top, row), (84.0, 24.0));
        // Never past the key height limits.
        assert_eq!(drag_key_bar(KeyGrab::Bottom, lo, hi, 39.0, h).1, ROW_MAX);
        assert_eq!(drag_key_bar(KeyGrab::Top, lo, hi, 500.0, h).1, ROW_MIN);
        // Fitting: the notes' span, centred.
        let (top, row) = fit_keys(48.0, 72.0, h);
        assert_eq!(row, 20.0);
        assert_eq!(keys_in_view(top, row, h), (48.0, 72.0));
    }

    fn piano_state() -> (State, PianoState) {
        let mut st = State::new();
        let t = st.add_track_kind(af2_core::TrackKind::Midi);
        new_clip(&mut st, t, 0.0);
        let ps = PianoState { rng: ops::Rng::new(1), ..PianoState::default() };
        (st, ps)
    }

    fn run(ctx: &egui::Context, events: Vec<Event>, f: &mut dyn FnMut(&mut egui::Ui)) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| f(ui));
        });
    }

    #[test]
    fn the_wheel_over_the_keys_makes_them_taller() {
        let (mut st, mut ps) = piano_state();
        let ctx = egui::Context::default();
        run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        let row0 = ps.row;
        let top0 = ps.top;
        let at = pos2(20.0, 300.0);
        run(&ctx, vec![Event::PointerMoved(at)], &mut |ui| panel(ui, &mut st, &mut ps));
        run(
            &ctx,
            vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: vec2(0.0, 2.0),
                modifiers: Default::default(),
            }],
            &mut |ui| panel(ui, &mut st, &mut ps),
        );
        for _ in 0..30 {
            run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        }
        assert!(ps.row > row0, "taller keys: {} -> {}", row0, ps.row);
        assert_ne!(ps.top, top0, "around the pointer, not the top");
        // Over the roll itself the wheel still scrolls.
        let (row1, top1) = (ps.row, ps.top);
        run(&ctx, vec![Event::PointerMoved(pos2(600.0, 300.0))], &mut |ui| panel(ui, &mut st, &mut ps));
        run(
            &ctx,
            vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: vec2(0.0, -2.0),
                modifiers: Default::default(),
            }],
            &mut |ui| panel(ui, &mut st, &mut ps),
        );
        for _ in 0..30 {
            run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        }
        assert_eq!(ps.row, row1);
        assert!(ps.top < top1);
    }


    #[test]
    fn the_computer_keyboard_plays_two_octaves_from_z_and_q() {
        use egui::Key;
        assert_eq!(typed_key(Key::Z), Some(Typed::Note(0)));
        assert_eq!(typed_key(Key::S), Some(Typed::Note(1)));
        assert_eq!(typed_key(Key::M), Some(Typed::Note(11)));
        assert_eq!(typed_key(Key::Q), Some(Typed::Note(12)));
        assert_eq!(typed_key(Key::Num2), Some(Typed::Note(13)));
        assert_eq!(typed_key(Key::P), Some(Typed::Note(28)));
        assert_eq!(typed_key(Key::PageDown), Some(Typed::OctaveDown));
        assert_eq!(typed_key(Key::Quote), Some(Typed::Note(17)));
        assert_eq!(typed_key(Key::OpenBracket), Some(Typed::Note(29)));
        assert_eq!(typed_key(Key::Equals), Some(Typed::Note(30)));
        assert_eq!(typed_key(Key::CloseBracket), Some(Typed::Note(31)));
        assert_eq!(typed_key(Key::Minus), None);
        assert_eq!(typed_key(Key::Space), None);
        assert_eq!(typed_key(Key::A), None);
        // Z is middle C in octave 4.
        assert_eq!(typed_pitch(4, 0), 60);
        assert_eq!(typed_pitch(4, 12), 72);
    }

    #[test]
    fn typed_keys_hold_notes_and_let_go_when_the_roll_loses_the_keys() {
        let (mut st, mut ps) = piano_state();
        let (ti, ci) = st.session.find_midi(st.piano_clip.unwrap()).unwrap();
        ps.focused = true;
        st.piano_keys = vec![(egui::Key::Z, true), (egui::Key::E, true)];
        play_typed(&mut st, &mut ps, ti, ci);
        assert_eq!(ps.typed.iter().map(|t| t.1).collect::<Vec<_>>(), vec![60, 76]);
        st.piano_keys = vec![(egui::Key::Z, false), (egui::Key::PageUp, true)];
        play_typed(&mut st, &mut ps, ti, ci);
        assert_eq!(ps.typed.iter().map(|t| t.1).collect::<Vec<_>>(), vec![76]);
        assert_eq!(ps.kb_octave, 5);
        ps.focused = false;
        play_typed(&mut st, &mut ps, ti, ci);
        assert!(ps.typed.is_empty());
    }

    #[test]
    fn replace_clears_the_recorded_span_and_overdub_keeps_it() {
        use af2_core::midi_live::KeyEvent;
        for replace in [false, true] {
            let (mut st, _ps) = piano_state();
            st.mode = crate::state::Mode::Multitrack;
            let clip = st.piano_clip.unwrap();
            let (ti, ci) = st.session.find_midi(clip).unwrap();
            // A note already there at beat 0.
            st.session.tracks[ti].midi[ci].notes.push(MidiNote { pitch: 60, start: 0.0, length: 1.0, velocity: 90, channel: 0, pan: 0.0 });
            let track = st.session.tracks[ti].id;
            st.piano_rec_clip = Some(clip);
            st.piano_rec_replace = replace;
            st.midi_record_start();
            // The recording's clock, as each frame samples it (from 0 s).
            st.midi_tick();
            st.midi_capture(track, KeyEvent::On { pitch: 64, velocity: 0.8, channel: 0 });
            st.midi_capture(track, KeyEvent::Off { pitch: 64, channel: 0 });
            st.midi_record_stop(1.0);
            let (ti, ci) = st.session.find_midi(clip).unwrap();
            let pitches: Vec<u8> = st.session.tracks[ti].midi[ci].notes.iter().map(|n| n.pitch).collect();
            if replace {
                assert_eq!(pitches, vec![64], "replace clears the old note");
            } else {
                assert_eq!(pitches.len(), 2, "overdub keeps it: {pitches:?}");
            }
        }
    }

    #[test]
    fn a_take_recorded_from_the_piano_roll_goes_into_its_clip() {
        use af2_core::midi_live::KeyEvent;
        let (mut st, _ps) = piano_state();
        st.mode = crate::state::Mode::Multitrack;
        let clip = st.piano_clip.unwrap();
        let (ti, _) = st.session.find_midi(clip).unwrap();
        let track = st.session.tracks[ti].id;
        let clips_before = st.session.tracks[ti].midi.len();
        st.piano_rec_clip = Some(clip);
        st.midi_record_start();
        st.midi_capture(track, KeyEvent::On { pitch: 64, velocity: 0.8, channel: 0 });
        st.midi_capture(track, KeyEvent::Off { pitch: 64, channel: 0 });
        st.midi_record_stop(1.0);
        let (ti, ci) = st.session.find_midi(clip).unwrap();
        assert_eq!(st.session.tracks[ti].midi.len(), clips_before, "no new clip");
        let notes = &st.session.tracks[ti].midi[ci].notes;
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].pitch, 64);
        assert!(st.piano_rec_clip.is_none());
    }

    #[test]
    fn a_note_end_grabs_from_further_in_and_just_past_it() {
        let r = Rect::from_min_max(pos2(100.0, 10.0), pos2(200.0, 20.0));
        // Up to 10 points in from either end stretches; the middle moves.
        assert_eq!(grab_ends(r, pos2(108.0, 15.0)), (true, false));
        assert_eq!(grab_ends(r, pos2(192.0, 15.0)), (false, true));
        assert_eq!(grab_ends(r, pos2(150.0, 15.0)), (false, false));
        // A short note keeps a middle to move it by.
        let short = Rect::from_min_max(pos2(100.0, 10.0), pos2(118.0, 20.0));
        assert_eq!(grab_ends(short, pos2(109.0, 15.0)), (false, false));
        assert_eq!(grab_ends(short, pos2(116.0, 15.0)), (false, true));
    }

    #[test]
    fn plus_raises_the_clip_an_octave_and_trans_adds_semitones() {
        use egui::PointerButton;
        let (mut st, mut ps) = piano_state();
        let ctx = egui::Context::default();
        run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        let pitch = |st: &State| {
            let (ti, ci) = st.session.find_midi(st.piano_clip.unwrap()).unwrap();
            st.session.tracks[ti].midi[ci].pitch
        };
        let b = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        let at = ps.oct_plus.center();
        run(&ctx, vec![Event::PointerMoved(at)], &mut |ui| panel(ui, &mut st, &mut ps));
        run(&ctx, vec![b(at, true), b(at, false)], &mut |ui| panel(ui, &mut st, &mut ps));
        run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        assert_eq!(pitch(&st), 12.0, "+ raises it an octave");
        let at = ps.knob_rects[0].left_center() + vec2(9.0, 0.0);
        run(&ctx, vec![Event::PointerMoved(at)], &mut |ui| panel(ui, &mut st, &mut ps));
        run(&ctx, vec![b(at, true)], &mut |ui| panel(ui, &mut st, &mut ps));
        for i in 1..=10 {
            let p = at - vec2(0.0, 3.0 * i as f32);
            run(&ctx, vec![Event::PointerMoved(p)], &mut |ui| panel(ui, &mut st, &mut ps));
        }
        run(&ctx, vec![b(at - vec2(0.0, 30.0), false)], &mut |ui| panel(ui, &mut st, &mut ps));
        run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        let p = pitch(&st);
        assert!(p > 12.0 && p.fract() == 0.0, "Trans adds whole semitones: {p}");
        st.mode = crate::state::Mode::Multitrack;
        st.undo();
        assert_eq!(pitch(&st), 12.0, "one undo step for the knob");
    }

    #[test]
    fn double_clicking_the_time_bar_shows_the_whole_clip() {
        use egui::PointerButton;
        let (mut st, mut ps) = piano_state();
        let ctx = egui::Context::default();
        ps.zoom = 400.0;
        ps.scroll_x = 3.0;
        run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        let at = pos2(ps.ruler_r.center().x, ps.ruler_r.top() - TBAR_H / 2.0);
        let b = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        run(&ctx, vec![Event::PointerMoved(at)], &mut |ui| panel(ui, &mut st, &mut ps));
        run(&ctx, vec![b(true), b(false)], &mut |ui| panel(ui, &mut st, &mut ps));
        run(&ctx, vec![b(true), b(false)], &mut |ui| panel(ui, &mut st, &mut ps));
        run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        assert_eq!(ps.scroll_x, 0.0, "back to the start");
        assert!(ps.zoom < 400.0, "zoomed out to the clip: {}", ps.zoom);
    }

    #[test]
    fn the_wheel_over_the_ruler_zooms_and_shift_scrolls_in_time() {
        let (mut st, mut ps) = piano_state();
        let ctx = egui::Context::default();
        run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        let at = ps.ruler_r.center();
        let z0 = ps.zoom;
        run(&ctx, vec![Event::PointerMoved(at)], &mut |ui| panel(ui, &mut st, &mut ps));
        let wheel = |dy: f32, shift: bool| Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: vec2(0.0, dy),
            modifiers: egui::Modifiers { shift, ..Default::default() },
        };
        run(&ctx, vec![wheel(2.0, false)], &mut |ui| panel(ui, &mut st, &mut ps));
        for _ in 0..30 {
            run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        }
        assert!(ps.zoom > z0, "zoomed in: {z0} -> {}", ps.zoom);
        let x0 = ps.scroll_x;
        run(&ctx, vec![wheel(-3.0, true)], &mut |ui| panel(ui, &mut st, &mut ps));
        for _ in 0..30 {
            run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        }
        assert!(ps.scroll_x > x0, "scrolled later: {x0} -> {}", ps.scroll_x);
    }

    #[test]
    fn dragging_on_the_ruler_moves_the_playhead_back_and_forth() {
        use egui::PointerButton;
        let (mut st, mut ps) = piano_state();
        let ctx = egui::Context::default();
        run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        let r = ps.ruler_r;
        assert!(r.height() > 10.0, "a ruler is drawn: {r:?}");
        let y = r.center().y;
        let at = |frac: f32| pos2(r.left() + r.width() * frac, y);
        let press = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        run(&ctx, vec![Event::PointerMoved(at(0.5))], &mut |ui| panel(ui, &mut st, &mut ps));
        run(&ctx, vec![press(at(0.5), true)], &mut |ui| panel(ui, &mut st, &mut ps));
        run(&ctx, vec![], &mut |ui| panel(ui, &mut st, &mut ps));
        let mid = st.position();
        assert!(mid > 0.0, "a press on the ruler moves the playhead");
        for k in 1..=5 {
            let p = at(0.5 + 0.06 * k as f32);
            run(&ctx, vec![Event::PointerMoved(p)], &mut |ui| panel(ui, &mut st, &mut ps));
        }
        let later = st.position();
        assert!(later > mid, "dragging right moves it later: {mid} -> {later}");
        for k in 1..=10 {
            let p = at(0.8 - 0.07 * k as f32);
            run(&ctx, vec![Event::PointerMoved(p)], &mut |ui| panel(ui, &mut st, &mut ps));
        }
        run(&ctx, vec![press(at(0.1), false)], &mut |ui| panel(ui, &mut st, &mut ps));
        let back = st.position();
        assert!(back < mid, "and dragging left moves it back: {back}");
        // No notes were drawn on the way.
        let (ti, ci) = st.session.find_midi(st.piano_clip.unwrap()).unwrap();
        assert!(st.session.tracks[ti].midi[ci].notes.is_empty());
    }

    #[test]
    fn the_channel_dropdown_lists_every_channel_and_picks_one() {
        let ctx = egui::Context::default();
        let th = crate::theme::Theme::default();
        let mut picked = None;
        let mut popup = egui::Id::NULL;
        let show = |events, current: u8, picked: &mut Option<u8>, popup: &mut egui::Id| {
            run(&ctx, events, &mut |ui| {
                *popup = channel_popup_id(ui);
                if let Some(ch) = channel_picker(ui, Color32::RED, current, &th) {
                    *picked = Some(ch);
                }
            });
        };
        let press = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        show(vec![], 2, &mut picked, &mut popup);
        let button = pos2(30.0, 10.0);
        show(vec![Event::PointerMoved(button), press(button, true)], 2, &mut picked, &mut popup);
        show(vec![press(button, false)], 2, &mut picked, &mut popup);
        show(vec![], 2, &mut picked, &mut popup);
        assert!(ctx.memory(|m| m.is_popup_open(popup)), "the list opens");
        // Below the button: 16 rows of 20 points, a point apart.
        let area = ctx.memory(|m| m.area_rect(popup)).expect("the list is shown");
        assert!(area.top() >= 19.0);
        assert!(area.height() >= 16.0 * 20.0);
        let margin = egui::Frame::popup(&ctx.style()).total_margin();
        let row9 = pos2(area.left() + margin.left + 60.0, area.top() + margin.top + 9.0 * 21.0 + 10.0);
        show(vec![Event::PointerMoved(row9), press(row9, true)], 2, &mut picked, &mut popup);
        show(vec![press(row9, false)], 2, &mut picked, &mut popup);
        assert_eq!(picked, Some(9), "channel 10 picked");
        show(vec![], 9, &mut picked, &mut popup);
        assert!(!ctx.memory(|m| m.is_popup_open(popup)), "and the list closes");
    }
}
