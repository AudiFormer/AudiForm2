// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! A MIDI clip's velocity and controller lanes, drawn over the clip in the
//! Multitrack the way an audio clip's volume and pan lines are. Each MIDI
//! clip shows the lanes picked in the small menu at the right of its title
//! bar: its notes' velocities (on unless switched off, with Volume and Pan
//! Lines on All Clips) and any controller (those the clip already moves
//! show by default). Which lanes show is not saved.
//!
//! Velocity: each note is a stem from the clip's foot with a dot at its
//! velocity; drag a dot up or down (notes starting together at the same
//! velocity move as one). Controllers: a stepped line, each move holding
//! until the next; a click on the line adds a move, a move drags (between
//! its neighbours, inside the clip, its value shown beside it),
//! right-click removes it. Every gesture is one undo step.

use af2_core::midi::{ControlEvent, MidiClip, LANE_CONTROLLERS, PITCH_BEND};
use af2_core::{ClipId, Session};
use egui::{pos2, vec2, Color32, CursorIcon, Rect, Sense, Stroke};

use crate::mixing::draw_readout;
use crate::state::{State, View};
use crate::waveform::{time_to_x, x_to_time};

/// A lane drawn over a MIDI clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiLine {
    Velocity,
    /// A controller (0..127, 128 aftertouch, 129 pitch bend).
    Control(u8),
}

/// Where the lanes picked for clip `id` are kept (UI only, not saved).
fn shown_key(id: ClipId) -> egui::Id {
    egui::Id::new(("af2-midi-lines", id))
}

/// The lanes clip `c` shows by default: velocity (while Volume and Pan
/// Lines on All Clips is on) and every controller it moves.
pub fn default_lines(c: &MidiClip, clip_lines: bool) -> Vec<MidiLine> {
    let mut v = Vec::new();
    if clip_lines {
        v.push(MidiLine::Velocity);
    }
    for e in &c.controls {
        let l = MidiLine::Control(e.controller);
        if !v.contains(&l) {
            v.push(l);
        }
    }
    v
}

/// The lanes clip `c` shows: as picked, or [`default_lines`].
pub fn lines_shown(ctx: &egui::Context, st: &State, c: &MidiClip) -> Vec<MidiLine> {
    ctx.data(|d| d.get_temp::<Vec<MidiLine>>(shown_key(c.id)))
        .unwrap_or_else(|| default_lines(c, st.prefs.clip_lines))
}

/// `shown` with `line` switched on or off.
pub fn toggled(mut shown: Vec<MidiLine>, line: MidiLine) -> Vec<MidiLine> {
    match shown.iter().position(|l| *l == line) {
        Some(k) => {
            shown.remove(k);
        }
        None => shown.push(line),
    }
    shown
}

/// A controller's value with no move before: pitch bend and pan in the
/// middle, volume at 100, expression full, the rest at 0.
pub fn resting(controller: u8) -> f32 {
    match controller {
        PITCH_BEND | 10 => 0.5,
        7 => 100.0 / 127.0,
        11 => 1.0,
        _ => 0.0,
    }
}

/// A controller's value at beat `b` of the clip: the last move at or
/// before it, else its resting value.
pub fn control_at(controls: &[ControlEvent], controller: u8, b: f64) -> f32 {
    controls
        .iter()
        .filter(|e| e.controller == controller && e.at <= b)
        .max_by(|x, y| x.at.total_cmp(&y.at))
        .map_or(resting(controller), |e| e.value)
}

/// A velocity from a height in the lane, 0 (foot) to 1 (top).
pub fn velocity_of(frac: f32) -> u8 {
    (1.0 + frac.clamp(0.0, 1.0) * 126.0).round() as u8
}

/// The notes drawn as one velocity dot: those starting together at the
/// same velocity, by index (in order of the first).
pub fn velocity_dots(c: &MidiClip) -> Vec<Vec<usize>> {
    let mut dots: Vec<Vec<usize>> = Vec::new();
    for (i, n) in c.notes.iter().enumerate() {
        if n.start < 0.0 || n.start >= c.length {
            continue;
        }
        match dots.iter_mut().find(|d| {
            let m = &c.notes[d[0]];
            m.start == n.start && m.velocity == n.velocity
        }) {
            Some(d) => d.push(i),
            None => dots.push(vec![i]),
        }
    }
    dots
}

fn short_name(l: MidiLine) -> String {
    match l {
        MidiLine::Velocity => "Vel".into(),
        MidiLine::Control(c) => {
            let name = ControlEvent::name(c);
            if name.chars().count() > 8 {
                format!("{}.", name.chars().take(7).collect::<String>())
            } else {
                name
            }
        }
    }
}

/// The menu button's label: the lanes shown ("Vel", "Vel/Mod Wheel", "Vel/Sustain+1").
pub fn button_label(shown: &[MidiLine]) -> String {
    let names: Vec<String> = shown.iter().map(|l| short_name(*l)).collect();
    match names.len() {
        0 => "Lanes".into(),
        1 | 2 => names.join("/"),
        n => format!("{}/{}+{}", names[0], names[1], n - 2),
    }
}

/// Draws MIDI clip `id`'s lanes over it (in track row `row`) and edits
/// them. `free` is false while the pointer is on one of the clip's handles
/// or edges, or something else is being dragged: the lanes then leave the
/// pointer alone (a drag already under way carries on).
pub fn overlay(
    ui: &mut egui::Ui,
    st: &mut State,
    id: ClipId,
    row: Rect,
    v: &View,
    lanes: Rect,
    free: bool,
) {
    let Some((ti, ci)) = st.session.find_midi(id) else {
        return;
    };
    let mc = st.session.tracks[ti].midi[ci].clone();
    let shown = lines_shown(ui.ctx(), st, &mc);
    if shown.is_empty() {
        return;
    }
    let th = st.theme.clone();
    let tempo = st.session.tempo.clone();
    let tc = st.session.tracks[ti].color;
    let track_col = Color32::from_rgb(tc[0], tc[1], tc[2]);
    let x0 = time_to_x(v, lanes, tempo.secs(mc.start));
    let x1 = time_to_x(v, lanes, mc.end_secs(&tempo));
    // Under the clip's title bar, clear of its bottom edge.
    let body = Rect::from_min_max(pos2(x0, row.top() + 21.0), pos2(x1, row.bottom() - 7.0));
    if body.width() < 10.0 || body.height() < 12.0 {
        return;
    }
    let len = mc.length;
    let x_of = |b: f64| time_to_x(v, lanes, mc.secs_at(b, &tempo));
    let b_of = |x: f32| mc.local_at(tempo.beats(x_to_time(v, lanes, x)), &tempo).clamp(0.0, len);
    let frac_of = |y: f32| (body.bottom() - y) / body.height().max(1.0);
    let y_of = |f: f32| body.bottom() - f.clamp(0.0, 1.0) * body.height();
    let painter = ui.painter_at(lanes.intersect(body.expand(5.0)));
    let readout_area = lanes.intersect(row.expand2(vec2(80.0, 0.0)));
    let readout_painter = ui.painter_at(readout_area);
    let hover = ui.input(|i| i.pointer.hover_pos());
    let drag_key = egui::Id::new(("af2-midi-lane-drag", id));
    let mut edited: Option<(&str, Session)> = None;
    let mut on_point = false;
    let mut readout: Option<(egui::Pos2, String)> = None;
    // The controller line nearest the pointer: (distance, controller, value there).
    let mut nearest: Option<(f32, u8, f32)> = None;
    let mut named = 0;
    let others = [
        Color32::from_rgb(120, 205, 160),
        Color32::from_rgb(215, 140, 230),
        Color32::from_rgb(235, 175, 80),
        Color32::from_rgb(110, 180, 240),
    ];
    let mut n_ctl = 0;
    for line in &shown {
        match *line {
            MidiLine::Velocity => {
                for dot in &velocity_dots(&mc) {
                    let n = mc.notes[dot[0]];
                    let x = x_of(n.start);
                    if x < lanes.left() - 6.0 || x > lanes.right() + 6.0 {
                        continue;
                    }
                    let col = crate::pianoroll::note_color(track_col, n.channel);
                    let at = pos2(x, y_of(n.velocity as f32 / 127.0));
                    painter.line_segment(
                        [pos2(x, body.bottom()), at],
                        Stroke::new(1.5_f32, col.gamma_multiply(0.75)),
                    );
                    painter.circle_filled(at, 2.5, col);
                    if edited.is_some() {
                        continue;
                    }
                    let pid = egui::Id::new(("af2-midi-vel", id, dot[0]));
                    if !free && !ui.ctx().is_being_dragged(pid) {
                        continue;
                    }
                    let r = ui.interact(
                        Rect::from_center_size(at, vec2(10.0, 10.0)).intersect(lanes),
                        pid,
                        Sense::drag(),
                    );
                    if r.hovered() || r.dragged() {
                        on_point = true;
                        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
                        painter.circle_stroke(at, 5.0, Stroke::new(1.5_f32, col));
                    }
                    if r.drag_started() {
                        let before = st.session.clone();
                        ui.data_mut(|d| d.insert_temp(drag_key, before));
                        st.selected_clip = Some(id);
                    }
                    let mut vel = n.velocity;
                    if r.dragged() {
                        if let Some(p) = r.interact_pointer_pos() {
                            vel = velocity_of(frac_of(p.y));
                            let ns = &mut st.session.tracks[ti].midi[ci].notes;
                            for &i in dot {
                                if let Some(m) = ns.get_mut(i) {
                                    m.velocity = vel;
                                }
                            }
                            st.session_version += 1;
                        }
                        readout = Some((
                            pos2(x, y_of(vel as f32 / 127.0)),
                            format!("Velocity {vel}"),
                        ));
                    }
                    if r.drag_stopped() {
                        if let Some(b) = ui.data_mut(|d| d.remove_temp::<Session>(drag_key)) {
                            edited = Some(("Set Velocity", b));
                        }
                    }
                    if !r.dragged() {
                        r.on_hover_text(format!(
                            "{} velocity {}. Drag up or down to change it.",
                            af2_core::midi::note_name(n.pitch),
                            n.velocity
                        ));
                    }
                }
            }
            MidiLine::Control(controller) => {
                n_ctl += 1;
                let col = others[(n_ctl - 1) % others.len()];
                let mut moves: Vec<(usize, ControlEvent)> = mc
                    .controls
                    .iter()
                    .copied()
                    .enumerate()
                    .filter(|(_, e)| e.controller == controller)
                    .collect();
                moves.sort_by(|a, b| a.1.at.total_cmp(&b.1.at));
                let quiet = moves.is_empty();
                let lc = if quiet { col.gamma_multiply(0.6) } else { col };
                // Stepped: each move holds until the next.
                let mut path = vec![pos2(x0, y_of(control_at(&mc.controls, controller, 0.0)))];
                for (_, e) in moves.iter().filter(|(_, e)| e.at >= 0.0 && e.at <= len) {
                    let x = x_of(e.at);
                    path.push(pos2(x, path.last().map_or(body.bottom(), |p| p.y)));
                    path.push(pos2(x, y_of(e.value)));
                }
                path.push(pos2(x1, path.last().map_or(body.bottom(), |p| p.y)));
                painter.add(egui::Shape::line(
                    path,
                    Stroke::new(if quiet { 1.0_f32 } else { 1.5 }, lc),
                ));
                if body.width() > 60.0 {
                    painter.text(
                        body.left_top() + vec2(4.0, 1.0 + 11.0 * named as f32),
                        egui::Align2::LEFT_TOP,
                        ControlEvent::name(controller),
                        egui::FontId::proportional(9.5),
                        lc,
                    );
                    named += 1;
                }
                if let Some(hp) = hover {
                    let val = control_at(&mc.controls, controller, b_of(hp.x));
                    let d = (hp.y - y_of(val)).abs();
                    if nearest.is_none_or(|n| d < n.0) {
                        nearest = Some((d, controller, val));
                    }
                }
                for (pos, &(i, e)) in moves.iter().enumerate() {
                    if e.at < 0.0 || e.at > len {
                        continue;
                    }
                    let at = pos2(x_of(e.at), y_of(e.value));
                    painter.circle_filled(at, 3.0, col);
                    if edited.is_some() {
                        continue;
                    }
                    let pid = egui::Id::new(("af2-midi-cc", id, controller, pos));
                    if !free && !ui.ctx().is_being_dragged(pid) {
                        continue;
                    }
                    let r = ui.interact(
                        Rect::from_center_size(at, vec2(11.0, 11.0)).intersect(lanes),
                        pid,
                        Sense::click_and_drag(),
                    );
                    if r.hovered() || r.dragged() {
                        on_point = true;
                        ui.ctx().set_cursor_icon(CursorIcon::Grab);
                        painter.circle_stroke(at, 5.0, Stroke::new(1.5_f32, col));
                    }
                    if r.drag_started() {
                        let before = st.session.clone();
                        ui.data_mut(|d| d.insert_temp(drag_key, before));
                        st.selected_clip = Some(id);
                    }
                    if r.dragged() {
                        let mut shown_at = (at, e.value);
                        if let Some(p) = r.interact_pointer_pos() {
                            let lo_t = pos.checked_sub(1).map_or(0.0, |j| moves[j].1.at.max(0.0));
                            let hi_t = moves.get(pos + 1).map_or(len, |m| m.1.at.min(len));
                            let value =
                                crate::pianoroll::shape_control(controller, frac_of(p.y));
                            let ev = &mut st.session.tracks[ti].midi[ci].controls[i];
                            ev.at = b_of(p.x).clamp(lo_t, hi_t.max(lo_t));
                            ev.value = value;
                            shown_at = (pos2(x_of(ev.at), y_of(value)), value);
                            st.session_version += 1;
                        }
                        readout = Some((
                            shown_at.0,
                            format!(
                                "{} {}",
                                ControlEvent::name(controller),
                                ControlEvent::value_text(controller, shown_at.1)
                            ),
                        ));
                    }
                    if r.drag_stopped() {
                        if let Some(b) = ui.data_mut(|d| d.remove_temp::<Session>(drag_key)) {
                            edited = Some(("Move Controller Move", b));
                        }
                    }
                    if r.secondary_clicked() {
                        let before = st.session.clone();
                        st.session.tracks[ti].midi[ci].controls.remove(i);
                        edited = Some(("Remove Controller Move", before));
                        break;
                    }
                    if !r.dragged() {
                        r.on_hover_text(format!(
                            "{} {}. Drag to move, right-click to remove.",
                            ControlEvent::name(controller),
                            ControlEvent::value_text(controller, e.value)
                        ));
                    }
                }
            }
        }
    }
    // Near a controller line (not on a point): a click adds a move there.
    if let (true, false, None, Some(hp), Some((d, controller, value))) =
        (free, on_point, edited.as_ref(), hover, nearest)
    {
        if d < 4.5 && body.expand2(vec2(0.0, 4.0)).contains(hp) && lanes.contains(hp) {
            let r = ui.interact(
                Rect::from_center_size(hp, vec2(12.0, 12.0)),
                egui::Id::new(("af2-midi-cc-line", id)),
                Sense::click(),
            );
            if r.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                painter.circle_stroke(hp, 3.5, Stroke::new(1.0_f32, Color32::WHITE));
            }
            if r.clicked() {
                let before = st.session.clone();
                let b = b_of(hp.x);
                let ctl = &mut st.session.tracks[ti].midi[ci].controls;
                // On the channel of the move before, else the first.
                let channel = ctl
                    .iter()
                    .filter(|e| e.controller == controller && e.at <= b)
                    .max_by(|x, y| x.at.total_cmp(&y.at))
                    .map_or(0, |e| e.channel);
                ctl.push(ControlEvent {
                    at: b,
                    controller,
                    value,
                    channel,
                });
                ctl.sort_by(|x, y| x.at.total_cmp(&y.at));
                st.selected_clip = Some(id);
                edited = Some(("Add Controller Move", before));
            }
            r.on_hover_text(format!(
                "{}: {}. Click to add a move.",
                ControlEvent::name(controller),
                ControlEvent::value_text(controller, value)
            ));
        }
    }
    if let Some((at, text)) = readout {
        draw_readout(&readout_painter, &th, at, text, readout_area);
    }
    if let Some((label, before)) = edited {
        st.commit_session(label, before);
    }
}

/// The menu button at the right of MIDI clip `id`'s title bar (`title`),
/// when the clip is wide enough: it names the lanes shown and its menu
/// picks them. `right_pad` keeps it clear of the corner handle and badge.
pub fn title_button(
    ui: &mut egui::Ui,
    st: &mut State,
    id: ClipId,
    title: Rect,
    right_pad: f32,
    lanes: Rect,
) -> bool {
    let Some((ti, ci)) = st.session.find_midi(id) else {
        return false;
    };
    let mc = st.session.tracks[ti].midi[ci].clone();
    let shown = lines_shown(ui.ctx(), st, &mc);
    let label = format!("{} \u{25BE}", button_label(&shown));
    let font = egui::FontId::proportional(9.5);
    let w = ui
        .painter()
        .layout_no_wrap(label.clone(), font.clone(), Color32::WHITE)
        .size()
        .x
        + 8.0;
    let r = Rect::from_min_size(
        pos2(title.right() - right_pad - w, title.top() + 1.0),
        vec2(w, title.height() - 2.0),
    );
    if title.width() < w + right_pad + 64.0 || !lanes.contains_rect(r) {
        return false;
    }
    let th = st.theme.clone();
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("af2-midi-lanes-button", id))
            .max_rect(r)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().button_padding = vec2(4.0, 0.0);
    child.spacing_mut().interact_size.y = r.height();
    let button = egui::Button::new(
        egui::RichText::new(label)
            .font(font)
            .color(Color32::from_rgb(th.text[0], th.text[1], th.text[2])),
    )
    .fill(crate::theme::ca(th.background, 190))
    .stroke(Stroke::NONE)
    .rounding(3.0)
    .min_size(r.size());
    let inner = egui::menu::menu_custom_button(&mut child, button, |ui| menu(ui, st, &mc));
    // A press on the button never starts a clip drag underneath.
    let _ = ui.interact(
        r,
        egui::Id::new(("af2-midi-lanes-block", id)),
        Sense::drag(),
    );
    inner
        .response
        .on_hover_text("The lanes shown on this clip: its notes' velocities and its controllers");
    true
}

/// The clip's lanes menu: which lanes show, each a tick.
fn menu(ui: &mut egui::Ui, st: &mut State, mc: &MidiClip) {
    let shown = lines_shown(ui.ctx(), st, mc);
    let mut toggle: Option<MidiLine> = None;
    let mut tick = |ui: &mut egui::Ui, line: MidiLine, label: String| {
        let mut on = shown.contains(&line);
        if ui.checkbox(&mut on, label).clicked() {
            toggle = Some(line);
        }
    };
    tick(ui, MidiLine::Velocity, "Velocity".into());
    ui.separator();
    let mut list: Vec<u8> = LANE_CONTROLLERS.to_vec();
    for e in &mc.controls {
        if !list.contains(&e.controller) {
            list.push(e.controller);
        }
    }
    for c in list {
        // A dot marks a controller the clip already moves.
        let used = mc.controls.iter().any(|e| e.controller == c);
        let mark = if used { " \u{2022}" } else { "" };
        tick(ui, MidiLine::Control(c), format!("{}{mark}", ControlEvent::name(c)));
    }
    if let Some(l) = toggle {
        let v = toggled(shown.clone(), l);
        ui.ctx().data_mut(|d| d.insert_temp(shown_key(mc.id), v));
    }
    ui.separator();
    if ui
        .selectable_label(st.prefs.clip_lines, "Volume and Pan Lines on All Clips")
        .on_hover_text("Whether clips show their volume and pan lines (MIDI clips: velocity) unless picked otherwise")
        .clicked()
    {
        st.prefs.clip_lines = !st.prefs.clip_lines;
        st.prefs.save();
        ui.close_menu();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use af2_core::midi::MidiNote;

    fn note(start: f64, pitch: u8, velocity: u8) -> MidiNote {
        MidiNote { pitch, start, length: 1.0, velocity, channel: 0, pan: 0.0 }
    }

    #[test]
    fn velocity_shows_by_default_and_used_controllers_too() {
        let mut c = MidiClip::new(5, "m", 0.0, 8.0);
        assert_eq!(default_lines(&c, true), vec![MidiLine::Velocity]);
        assert!(default_lines(&c, false).is_empty());
        c.controls.push(ControlEvent { at: 1.0, controller: 64, value: 1.0, channel: 0 });
        c.controls.push(ControlEvent { at: 2.0, controller: 64, value: 0.0, channel: 0 });
        assert_eq!(default_lines(&c, true), vec![MidiLine::Velocity, MidiLine::Control(64)]);
        let v = toggled(default_lines(&c, true), MidiLine::Velocity);
        assert_eq!(v, vec![MidiLine::Control(64)]);
        assert_eq!(toggled(v, MidiLine::Control(1)), vec![MidiLine::Control(64), MidiLine::Control(1)]);
        assert_eq!(button_label(&[MidiLine::Velocity, MidiLine::Control(64)]), "Vel/Sustain.");
    }

    #[test]
    fn chords_at_one_velocity_are_one_dot() {
        let mut c = MidiClip::new(5, "m", 0.0, 4.0);
        c.notes = vec![note(0.0, 60, 100), note(0.0, 64, 100), note(0.0, 67, 80), note(1.0, 60, 100), note(5.0, 60, 100)];
        // The note past the clip's end is not drawn.
        assert_eq!(velocity_dots(&c), vec![vec![0, 1], vec![2], vec![3]]);
        assert_eq!(velocity_of(0.0), 1);
        assert_eq!(velocity_of(1.0), 127);
        assert_eq!(velocity_of(2.0), 127);
    }

    #[test]
    fn a_controller_holds_its_last_move() {
        let ev = |at, value| ControlEvent { at, controller: 1, value, channel: 0 };
        let c = vec![ev(1.0, 0.25), ev(3.0, 0.75)];
        assert_eq!(control_at(&c, 1, 0.5), 0.0);
        assert_eq!(control_at(&c, 1, 1.0), 0.25);
        assert_eq!(control_at(&c, 1, 2.9), 0.25);
        assert_eq!(control_at(&c, 1, 9.0), 0.75);
        // Pitch bend rests in the middle.
        assert_eq!(control_at(&c, PITCH_BEND, 2.0), 0.5);
    }

    /// Runs the Multitrack for one frame, 1600 x 900.
    fn frame(ctx: &egui::Context, st: &mut State, caches: &mut crate::ui::Caches, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 900.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| crate::multitrack::editor(ui, st, caches));
        });
    }

    #[test]
    fn dragging_a_velocity_dot_on_a_midi_clip_sets_the_velocity() {
        use egui::{Event, PointerButton};
        let mut st = State::new();
        st.mode = crate::state::Mode::Multitrack;
        st.prefs.clip_lines = true;
        let t = st.add_track_kind(af2_core::TrackKind::Midi);
        // Only the MIDI track, so its row is in view.
        st.session.tracks.retain(|x| x.id == t);
        let ti = 0;
        st.session.tracks[ti].height = 120.0;
        let mut clip = MidiClip::new(77, "m", 4.0, 8.0);
        clip.notes = vec![note(2.0, 60, 64), note(2.0, 64, 64), note(5.0, 67, 100)];
        st.session.tracks[ti].midi.push(clip);
        st.session_view = View { start: 0.0, len: 20.0 };
        let ctx = egui::Context::default();
        let mut caches = crate::ui::Caches::default();
        frame(&ctx, &mut st, &mut caches, vec![]);
        frame(&ctx, &mut st, &mut caches, vec![]);
        let tempo = st.session.tempo.clone();
        // The lanes' geometry, as the editor lays them out.
        let metro = if st.session.tempo.metronome { crate::track_views::METRO_H } else { 0.0 };
        let (left, top) = (st.prefs.header_w, crate::overview::STRIP_H + 22.0 + metro);
        let width = 1600.0 - left - 12.0;
        let x_of = |secs: f64| left + width * (secs / 20.0) as f32;
        let row_top = top + st.session.tracks[..ti].iter().map(|t| t.height).sum::<f32>();
        let (body_top, body_bottom) = (row_top + 21.0, row_top + 120.0 - 7.0);
        let y_of = |vel: f32| body_bottom - vel / 127.0 * (body_bottom - body_top);
        let from = pos2(x_of(tempo.secs(6.0)), y_of(64.0));
        let to = pos2(from.x, y_of(120.0));
        let press = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&ctx, &mut st, &mut caches, vec![Event::PointerMoved(from)]);
        frame(&ctx, &mut st, &mut caches, vec![press(from, true)]);
        for k in 1..=10 {
            let p = from + (to - from) * (k as f32 / 10.0);
            frame(&ctx, &mut st, &mut caches, vec![Event::PointerMoved(p)]);
        }
        frame(&ctx, &mut st, &mut caches, vec![press(to, false)]);
        frame(&ctx, &mut st, &mut caches, vec![]);
        let c = &st.session.tracks[ti].midi[0];
        // The chord moves as one; the clip stays put; the other note is untouched.
        assert!((118..=122).contains(&c.notes[0].velocity), "velocity {}", c.notes[0].velocity);
        assert_eq!(c.notes[0].velocity, c.notes[1].velocity);
        assert_eq!(c.notes[2].velocity, 100);
        assert_eq!(c.start, 4.0);
        // One undo step brings it back.
        st.undo();
        assert_eq!(st.session.tracks[ti].midi[0].notes[0].velocity, 64);
    }
}
