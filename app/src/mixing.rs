// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Mixing rules the track headers and the Mixer share: solo, mute and arm
//! clicks with their modifiers, automation writing while playing, and
//! the automation lane drawn over a track's clips.
//!
//! Solo: a click solos or unsolos; with Exclusive Solo on (Settings), or
//! Ctrl+click whatever the setting ("Solo Track Only"), every other track
//! and bus is unsoloed in the same step. Soloing a bus solos everything
//! whose output reaches it. Ctrl+Shift+click on M, S or R sets it on
//! every track.
//!
//! Automation: while playing, moving a track's volume or pan on a track
//! in Write, Latch or Touch writes into its lane (see
//! `af2_core::automation::AutoWriter`). The lanes being written play
//! from the live control, so nothing is rebuilt while writing; the
//! written points go into the session when play stops, as one undo step
//! with the moves that made them.

use std::collections::HashMap;

use af2_core::automation::{
    AutoWriter, AutomationMode, Curve, Lane, LanePoint, LaneTarget, Owner, Written,
};
use af2_core::{Session, TrackId, TrackKind};
use egui::{pos2, vec2, Color32, Modifiers, Rect, Sense, Stroke};

use crate::state::State;
use crate::state::View;
use crate::waveform::{time_to_x, x_to_time};

/// The app's mixing state that is not saved with the session.
#[derive(Default)]
pub struct Mixing {
    pub writer: AutoWriter,
    /// The session before the current writing pass (its undo step).
    pass_before: Option<Session>,
    /// Stretches finished during the pass (Touch let go), applied at stop.
    done: Vec<Written>,
    /// Lanes the engine plays from their live control.
    live: Vec<(Owner, LaneTarget)>,
    /// Which lane each track shows over its clips (index into its lanes).
    pub shown: HashMap<TrackId, usize>,
}

impl Mixing {
    /// A writing pass is open: control moves belong to its undo step.
    pub fn pass_open(&self) -> bool {
        self.pass_before.is_some()
    }
}

fn now(st: &State) -> f64 {
    st.clock()
}

fn kind_of(target: &LaneTarget) -> Option<af2_engine::mix::LaneKind> {
    match target {
        LaneTarget::Volume => Some(af2_engine::mix::LaneKind::Volume),
        LaneTarget::Pan => Some(af2_engine::mix::LaneKind::Pan),
        _ => None,
    }
}

impl State {
    /// A click on a track's S, with the keys held.
    pub fn click_solo(&mut self, ti: usize, mods: Modifiers) {
        let Some(t) = self.session.tracks.get(ti) else {
            return;
        };
        let (id, on) = (t.id, !t.solo);
        let before = self.session.clone();
        if mods.command && mods.shift {
            for t in &mut self.session.tracks {
                t.solo = on;
            }
        } else {
            let exclusive = mods.command || self.prefs.solo_exclusive;
            self.session.set_solo(id, on, exclusive);
        }
        self.commit_live("Solo", before);
        self.live_all_tracks();
    }

    /// A click on a track's M (Ctrl+Shift: every track).
    pub fn click_mute(&mut self, ti: usize, mods: Modifiers) {
        let Some(t) = self.session.tracks.get(ti) else {
            return;
        };
        let on = !t.mute;
        let before = self.session.clone();
        if mods.command && mods.shift {
            for t in &mut self.session.tracks {
                t.mute = on;
            }
        } else {
            self.session.tracks[ti].mute = on;
        }
        self.commit_live("Mute", before);
        self.live_all_tracks();
    }

    /// A click on a track's R (Ctrl+Shift: every audio and MIDI track).
    pub fn click_arm(&mut self, ti: usize, mods: Modifiers) {
        let Some(t) = self.session.tracks.get(ti) else {
            return;
        };
        let on = !t.armed;
        let before = self.session.clone();
        if mods.command && mods.shift {
            for t in self
                .session
                .tracks
                .iter_mut()
                .filter(|t| t.kind != TrackKind::Bus)
            {
                t.armed = on;
            }
        } else {
            self.session.tracks[ti].armed = on;
        }
        self.commit_live("Arm", before);
    }

    /// Sends every track's levels and whether it is heard to the mix.
    pub fn live_all_tracks(&mut self) {
        for ti in 0..self.session.tracks.len() {
            self.live_track(ti);
        }
    }

    /// A track's volume or pan moved to `value` (dB, or -1..1). While
    /// playing on a track in a writing mode, it is written.
    pub fn auto_moved(&mut self, ti: usize, target: LaneTarget, value: f64) {
        if !self.transport_playing() {
            return;
        }
        let Some(t) = self.session.tracks.get(ti) else {
            return;
        };
        let mode = t.automation;
        if !mode.writes() {
            return;
        }
        let owner = Owner::Track(t.id);
        if self.mixing.pass_before.is_none() {
            self.mixing.pass_before = Some(self.session.clone());
        }
        let (pos, clock) = (self.position(), now(self));
        self.mixing
            .writer
            .moved(owner, target, mode, value, pos, clock);
        self.auto_live();
    }

    /// Play started at `from`: tracks in Write start writing every lane
    /// they write (volume and pan).
    pub fn auto_start(&mut self, from: f64) {
        let mut write = Vec::new();
        for t in &self.session.tracks {
            if t.automation == AutomationMode::Write {
                let o = Owner::Track(t.id);
                write.push((o, LaneTarget::Volume, t.volume_db as f64));
                if self.session.space.is_none() {
                    write.push((o, LaneTarget::Pan, t.pan as f64));
                }
            }
        }
        self.mixing.done.clear();
        self.mixing.pass_before = (!write.is_empty()).then(|| self.session.clone());
        let clock = now(self);
        self.mixing.writer.start(from, clock, &write);
        self.auto_live();
    }

    /// Each frame while playing: held values move on with the playhead,
    /// Touch lanes let go.
    pub fn auto_tick(&mut self) {
        if !self.mixing.writer.is_active() || !self.transport_playing() {
            return;
        }
        let (pos, clock) = (self.position(), now(self));
        let done = self.mixing.writer.tick(pos, clock);
        if !done.is_empty() {
            self.mixing.done.extend(done);
            self.auto_live();
        }
    }

    /// Play stopped at `t`: what was written goes into the lanes as one
    /// undo step, and the lanes play from their points again.
    pub fn auto_stop(&mut self, t: f64) {
        let mut done = std::mem::take(&mut self.mixing.done);
        done.extend(self.mixing.writer.stop(t));
        self.auto_live();
        let Some(before) = self.mixing.pass_before.take() else {
            return;
        };
        if done.is_empty() {
            return;
        }
        // Lanes on slots that had none get the effect's slot id.
        af2_core::automation::apply(&mut self.session, &done);
        self.commit_session("Write Automation", before);
    }

    /// Tells the mix which lanes play from their live control now.
    fn auto_live(&mut self) {
        let now_live = self.mixing.writer.live();
        let old = std::mem::take(&mut self.mixing.live);
        let index = |s: &Session, o: &Owner| match o {
            Owner::Track(id) => s.track_index(*id).map(Some),
            Owner::Master => Some(None),
        };
        if let Some(e) = &mut self.engine {
            for (o, tg) in old.iter().filter(|x| !now_live.contains(x)) {
                if let (Some(ti), Some(k)) = (index(&self.session, o), kind_of(tg)) {
                    e.set_lane_live(ti, k, false);
                }
            }
            for (o, tg) in now_live.iter().filter(|x| !old.contains(x)) {
                if let (Some(ti), Some(k)) = (index(&self.session, o), kind_of(tg)) {
                    e.set_lane_live(ti, k, true);
                }
            }
        }
        self.mixing.live = now_live;
        // A lane stepping aside hands its control the current value.
        for ti in 0..self.session.tracks.len() {
            self.live_track(ti);
        }
    }

    /// The lane track `ti` shows over its clips, if any.
    pub fn shown_lane(&self, ti: usize) -> Option<usize> {
        let t = self.session.tracks.get(ti)?;
        if !t.show_lanes || t.lanes.is_empty() {
            return None;
        }
        let k = self.mixing.shown.get(&t.id).copied().unwrap_or(0);
        Some(k.min(t.lanes.len() - 1))
    }

    /// Adds (or shows) a lane for `target` on track `ti`.
    pub fn add_lane(&mut self, ti: usize, target: LaneTarget) {
        let before = self.session.clone();
        let Some(t) = self.session.tracks.get_mut(ti) else {
            return;
        };
        let k = match t.lanes.iter().position(|l| l.target == target) {
            Some(k) => k,
            None => {
                t.lanes.push(Lane::new(target));
                t.lanes.len() - 1
            }
        };
        t.show_lanes = true;
        let id = t.id;
        self.mixing.shown.insert(id, k);
        self.commit_session("Add Automation Lane", before);
    }
}

/// The value a lane of `target` holds for track `t` now (its control).
pub fn control_value(t: &af2_core::Track, target: &LaneTarget) -> f64 {
    match target {
        LaneTarget::Volume => t.volume_db as f64,
        LaneTarget::Pan => t.pan as f64,
        LaneTarget::Effect { slot, param, .. } => {
            if let Some(s) = plugin_slot_of(t, slot) {
                return plugin_value(s, *param as u32);
            }
            t.chain
                .iter()
                .find(|s| s.ext_id.as_deref() == Some(slot.as_str()))
                .and_then(|s| s.values.get(*param))
                .map_or(0.0, |v| *v as f64)
        }
        LaneTarget::Space(_) => 0.0,
        LaneTarget::Mute => t.mute as u8 as f64,
        LaneTarget::SendLevel(id) => t
            .sends
            .iter()
            .find(|s| s.ext_id.as_deref() == Some(id.as_str()))
            .map_or(0.0, |s| s.level_db as f64),
        LaneTarget::SendPan(id) => t
            .sends
            .iter()
            .find(|s| s.ext_id.as_deref() == Some(id.as_str()))
            .map_or(0.0, |s| s.pan as f64),
        LaneTarget::Rack(p) => p.value_in(&t.rack),
    }
}

/// The range a lane is drawn over: its target's, widened for effect
/// values to the effect's own range when known.
fn lane_range(t: &af2_core::Track, target: &LaneTarget) -> (f64, f64) {
    lane_range_in(&t.chain, target)
}

/// Lanes drawn upside down: pan (and send pan) puts left at the top and
/// right at the bottom, as multitrack editors draw it.
pub fn drawn_flipped(target: &LaneTarget) -> bool {
    matches!(target, LaneTarget::Pan | LaneTarget::SendPan(_))
}

/// [`lane_range`] for a lane on the owner of `chain` (a track or a clip).
pub fn lane_range_in(chain: &[af2_core::EffectSlot], target: &LaneTarget) -> (f64, f64) {
    if let LaneTarget::Effect { slot, param, .. } = target {
        if let Some(s) = chain
            .iter()
            .find(|s| s.ext_id.as_deref() == Some(slot.as_str()))
        {
            if let Some(p) = af2_dsp::Effect::from_id(&s.effect)
                .map(|e| e.params())
                .and_then(|ps| ps.get(*param).map(|p| (p.min as f64, p.max as f64)))
            {
                return p;
            }
            if let Some(p) =
                af2_fx::info(&s.effect).and_then(|i| i.params.get(*param).map(|p| (p.min, p.max)))
            {
                return p;
            }
        }
    }
    target.range()
}

/// An effect lane's value as its effect has it: (name, unit, default),
/// when the effect is known.
fn effect_param(
    chain: &[af2_core::EffectSlot],
    target: &LaneTarget,
) -> Option<(String, &'static str, f64)> {
    let LaneTarget::Effect { slot, param, .. } = target else {
        return None;
    };
    let s = chain
        .iter()
        .find(|s| s.ext_id.as_deref() == Some(slot.as_str()))
        .or_else(|| {
            // A slot not yet given an id (see `pending-` targets).
            slot.strip_prefix("pending-")
                .and_then(|n| n.parse::<usize>().ok())
                .and_then(|i| chain.get(i))
        })?;
    if af2_core::plugin::is_plugin(&s.effect) {
        let p = plugin_param(s, *param as u32);
        let name = p.as_ref().map_or_else(
            || format!("Control {param}"),
            |p| p.title.clone(),
        );
        return Some((name, "", p.map_or(0.0, |p| p.default)));
    }
    if let Some(p) =
        af2_dsp::Effect::from_id(&s.effect).and_then(|e| e.params().get(*param).cloned())
    {
        return Some((p.name.to_string(), p.unit, p.default as f64));
    }
    af2_fx::info(&s.effect).and_then(|i| {
        i.params
            .get(*param)
            .map(|p| (p.name.to_string(), p.unit, p.default))
    })
}

/// What a lane's value rests at by default (where a dragged point snaps
/// to): 0 dB, centre, the rack's 0 dB, full mix and on, or the effect
/// value's default.
pub fn lane_default_in(chain: &[af2_core::EffectSlot], target: &LaneTarget) -> f64 {
    match target {
        LaneTarget::Rack(p) => p.default_value(),
        LaneTarget::Effect { .. } => effect_param(chain, target).map_or(0.0, |p| p.2),
        _ => 0.0,
    }
}

/// A lane's name for its label: the effect value's own name when known.
pub fn lane_label_in(chain: &[af2_core::EffectSlot], target: &LaneTarget) -> String {
    effect_param(chain, target).map_or_else(|| target.label(), |p| p.0)
}

/// A lane value as the readout shows it: "-3.4 dB", "L 20", "Centre",
/// "80 % wet", "On", or the effect value in its unit.
pub fn lane_value_text_in(chain: &[af2_core::EffectSlot], target: &LaneTarget, v: f64) -> String {
    use af2_core::automation::RackParam;
    let db = |v: f64| {
        let r = (v * 10.0).round() / 10.0;
        if r == 0.0 {
            "0.0 dB".to_string()
        } else {
            format!("{r:+.1} dB")
        }
    };
    match target {
        LaneTarget::Volume | LaneTarget::SendLevel(_) => {
            if v <= af2_core::automation::SILENT_DB {
                "-inf dB".into()
            } else {
                db(v)
            }
        }
        LaneTarget::Rack(RackParam::Input | RackParam::Output) => db(v),
        LaneTarget::Pan | LaneTarget::SendPan(_) => {
            let p = (v * 100.0).round();
            if p == 0.0 {
                "Centre".into()
            } else if p < 0.0 {
                format!("L {:.0}", -p)
            } else {
                format!("R {p:.0}")
            }
        }
        LaneTarget::Rack(RackParam::Mix) => format!("{:.0} % wet", v.clamp(0.0, 1.0) * 100.0),
        LaneTarget::Rack(RackParam::Power) => if v >= 0.5 { "On" } else { "Off" }.into(),
        LaneTarget::Mute => if v >= 0.5 { "Muted" } else { "Heard" }.into(),
        LaneTarget::Effect { slot, param, .. } if plugin_in(chain, slot).is_some() => {
            let s = plugin_in(chain, slot).unwrap();
            plugin_value_text(plugin_param(s, *param as u32).as_ref(), v)
        }
        _ => match effect_param(chain, target) {
            Some((_, unit, _)) => {
                let digits = if v.abs() >= 100.0 {
                    0
                } else if v.abs() >= 10.0 {
                    1
                } else {
                    2
                };
                if unit.is_empty() {
                    format!("{v:.digits$}")
                } else {
                    format!("{v:.digits$} {unit}")
                }
            }
            None => format!("{v:.2}"),
        },
    }
}

/// A dragged point's value: snapped onto the lane's default when within
/// [`af2_core::automation::SNAP_PX`] of it (not while Alt is held), and
/// on or off for a power lane. `px_per_unit` is the lane's drawn scale.
pub fn drag_value(
    ui: &egui::Ui,
    chain: &[af2_core::EffectSlot],
    target: &LaneTarget,
    raw: f64,
    px_per_unit: f64,
) -> f64 {
    if matches!(
        target,
        LaneTarget::Rack(af2_core::automation::RackParam::Power) | LaneTarget::Mute
    ) {
        return if raw >= 0.5 { 1.0 } else { 0.0 };
    }
    if let Some(v) = plugin_snap(chain, target, raw) {
        return v;
    }
    if ui.input(|i| i.modifiers.alt) {
        return raw;
    }
    af2_core::automation::snap_to(
        raw,
        lane_default_in(chain, target),
        px_per_unit,
        af2_core::automation::SNAP_PX,
    )
}

/// The value label drawn beside a point being dragged, kept inside
/// `inside`.
pub fn draw_readout(
    painter: &egui::Painter,
    th: &crate::theme::Theme,
    at: egui::Pos2,
    text: String,
    inside: Rect,
) {
    let galley = painter.layout_no_wrap(
        text,
        egui::FontId::monospace(10.0),
        Color32::from_rgb(th.text[0], th.text[1], th.text[2]),
    );
    let size = galley.size() + vec2(6.0, 2.0);
    // Up and to the right of the point, or to its left near the edge.
    let mut min = at + vec2(8.0, -size.y - 4.0);
    if min.x + size.x > inside.right() {
        min.x = at.x - 8.0 - size.x;
    }
    if min.y < inside.top() {
        min.y = at.y + 6.0;
    }
    let back = Rect::from_min_size(min, size);
    painter.rect_filled(back, 3.0, crate::theme::ca(th.background, 225));
    painter.rect_stroke(
        back,
        3.0,
        Stroke::new(1.0_f32, crate::theme::ca(th.accent, 200)),
    );
    painter.galley(back.min + vec2(3.0, 1.0), galley, Color32::WHITE);
}

/// Draws track `ti`'s shown lane over its row, and edits it: click adds a
/// point, drag moves the nearest, right-click removes it. Points being
/// written show as they are written.
pub fn lane_overlay(
    ui: &mut egui::Ui,
    st: &mut State,
    ti: usize,
    row: Rect,
    v: &View,
    lanes: Rect,
) {
    let Some(k) = st.shown_lane(ti) else {
        return;
    };
    let th = st.theme.clone();
    let t = st.session.tracks[ti].clone();
    let lane = &t.lanes[k];
    let (lo, hi) = lane_range(&t, &lane.target);
    let area = row.shrink2(vec2(0.0, 6.0));
    // Pan runs left at the top, right at the bottom.
    let flip = drawn_flipped(&lane.target);
    let y_of = |val: f64| {
        let mut x = ((val - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0);
        if flip {
            x = 1.0 - x;
        }
        area.bottom() - x as f32 * area.height()
    };
    let val_of = |y: f32| {
        let mut x = ((area.bottom() - y) / area.height().max(1.0)).clamp(0.0, 1.0) as f64;
        if flip {
            x = 1.0 - x;
        }
        lo + (hi - lo) * x
    };
    let painter = ui.painter_at(lanes.intersect(row));
    let col = if lane.enabled {
        Color32::from_rgb(th.accent[0], th.accent[1], th.accent[2])
    } else {
        Color32::from_gray(120)
    };
    painter.rect_filled(area, 0.0, Color32::from_black_alpha(40));
    let pts = &lane.points;
    let shown_value = control_value(&t, &lane.target);
    let mut line: Vec<egui::Pos2> = Vec::new();
    if pts.is_empty() {
        let y = y_of(shown_value);
        line.push(pos2(lanes.left(), y));
        line.push(pos2(lanes.right(), y));
    } else {
        line.push(pos2(lanes.left(), y_of(pts[0].value)));
        for (i, p) in pts.iter().enumerate() {
            let x = time_to_x(v, lanes, p.time);
            if i > 0 && pts[i - 1].curve == Curve::Hold {
                line.push(pos2(x, y_of(pts[i - 1].value)));
            }
            line.push(pos2(x, y_of(p.value)));
        }
        line.push(pos2(lanes.right(), y_of(pts[pts.len() - 1].value)));
    }
    painter.add(egui::Shape::line(line, Stroke::new(1.5_f32, col)));
    for p in pts {
        let at = pos2(time_to_x(v, lanes, p.time), y_of(p.value));
        painter.circle_filled(at, 3.0, col);
    }
    // What is being written now.
    let owner = Owner::Track(t.id);
    for (o, tg, draft) in st.mixing.writer.drafts() {
        if o == owner && *tg == lane.target && draft.len() >= 2 {
            let line = draft
                .iter()
                .map(|p| pos2(time_to_x(v, lanes, p.time), y_of(p.value)))
                .collect();
            painter.add(egui::Shape::line(line, Stroke::new(2.0_f32, Color32::RED)));
        }
    }
    painter.text(
        area.left_top() + vec2(4.0, 2.0),
        egui::Align2::LEFT_TOP,
        format!(
            "{} ({})",
            lane_label_in(&lane_slots(&t), &lane.target),
            t.automation.label()
        ),
        egui::FontId::proportional(10.0),
        col,
    );

    // Editing.
    let id = egui::Id::new(("af2-lane", t.id, k));
    let resp = ui.interact(area.intersect(lanes), id, Sense::click_and_drag());
    let near = |pos: egui::Pos2| {
        pts.iter()
            .enumerate()
            .map(|(i, p)| {
                let at = pos2(time_to_x(v, lanes, p.time), y_of(p.value));
                (i, at.distance(pos))
            })
            .filter(|(_, d)| *d < 7.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    let drag_key = id.with("drag");
    let before = st.session.clone();
    let mut edited = false;
    if resp.drag_started() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let i = near(pos);
            ui.data_mut(|d| d.insert_temp(drag_key, i.map(|i| (i, before.clone()))));
        }
    }
    let dragging: Option<(usize, Session)> = ui.data(|d| d.get_temp(drag_key)).flatten();
    if let (Some((i, _)), true, Some(pos)) = (
        dragging.as_ref(),
        resp.dragged(),
        resp.interact_pointer_pos(),
    ) {
        let value = drag_value(
            ui,
            &lane_slots(&t),
            &lane.target,
            val_of(pos.y),
            area.height() as f64 / (hi - lo).max(1e-9),
        );
        let lane = &mut st.session.tracks[ti].lanes[k];
        if let Some(p) = lane.points.get_mut(*i) {
            p.time = x_to_time(v, lanes, pos.x).max(0.0);
            p.value = value;
        }
        // Keep it between its neighbours.
        let (prev, next) = (
            i.checked_sub(1)
                .and_then(|j| lane.points.get(j))
                .map(|p| p.time),
            lane.points.get(i + 1).map(|p| p.time),
        );
        if let Some(p) = lane.points.get_mut(*i) {
            if let Some(a) = prev {
                p.time = p.time.max(a);
            }
            if let Some(b) = next {
                p.time = p.time.min(b);
            }
        }
    }
    // The value of the point being dragged, beside it.
    if let (Some((i, _)), true) = (dragging.as_ref(), resp.dragged()) {
        if let Some(p) = st.session.tracks[ti].lanes[k].points.get(*i) {
            let at = pos2(time_to_x(v, lanes, p.time), y_of(p.value));
            draw_readout(
                &painter,
                &th,
                at,
                lane_value_text_in(&lane_slots(&t), &lane.target, p.value),
                lanes.intersect(row),
            );
        }
    }
    if resp.drag_stopped() {
        if let Some((_, b)) = dragging {
            st.commit_session("Move Automation Point", b);
        }
        ui.data_mut(|d| d.remove::<Option<(usize, Session)>>(drag_key));
    }
    if resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            if near(pos).is_none() {
                let lane = &mut st.session.tracks[ti].lanes[k];
                lane.points.push(LanePoint {
                    time: x_to_time(v, lanes, pos.x).max(0.0),
                    value: val_of(pos.y),
                    curve: Curve::Linear,
                });
                lane.sort();
                edited = true;
            }
        }
    }
    if resp.secondary_clicked() {
        if let Some(i) = resp.interact_pointer_pos().and_then(near) {
            st.session.tracks[ti].lanes[k].points.remove(i);
            edited = true;
        }
    }
    if resp.double_clicked() {
        if let Some(i) = resp.interact_pointer_pos().and_then(near) {
            let p = &mut st.session.tracks[ti].lanes[k].points[i];
            p.curve = match p.curve {
                Curve::Linear => Curve::Hold,
                _ => Curve::Linear,
            };
            edited = true;
        }
    }
    if edited {
        st.commit_session("Edit Automation", before);
    }
    resp.on_hover_text(
        "Click: add a point. Drag: move one (it snaps to the default; hold Alt to move freely). Right-click: remove. Double-click: hold or ramp.",
    );
}

/// Menu entries to add and show lanes for track `ti`: volume, pan, and
/// every value of the effects in its chain.
pub fn lanes_menu(ui: &mut egui::Ui, st: &mut State, ti: usize) {
    let Some(t) = st.session.tracks.get(ti) else {
        return;
    };
    let mut pick: Option<LaneTarget> = None;
    let has = |tg: &LaneTarget| t.lanes.iter().any(|l| &l.target == tg);
    let mut targets = vec![LaneTarget::Volume];
    if st.session.space.is_none() {
        targets.push(LaneTarget::Pan);
    }
    for tg in targets {
        let label = format!("{}{}", tg.label(), if has(&tg) { " (shown)" } else { "" });
        if ui.button(label).clicked() {
            pick = Some(tg);
            ui.close_menu();
        }
    }
    for p in af2_core::automation::RackParam::ALL {
        let tg = LaneTarget::Rack(p);
        let label = format!("{}{}", tg.label(), if has(&tg) { " (shown)" } else { "" });
        if ui.button(label).clicked() {
            pick = Some(tg);
            ui.close_menu();
        }
    }
    // A MIDI track's instrument plug-in, then each plug-in in the chain:
    // their own controls, by the names the plug-in gives them.
    if t.kind == TrackKind::Midi {
        if let Some(s) = t.instrument_plugin.as_ref().filter(|s| is_plugin(&s.effect)) {
            let title = format!("Instrument: {}", crate::plugins::slot_name(s));
            if let Some(id) = plugin_lane_menu(ui, &title, s, ti, &has) {
                pick = Some(plugin_target(s, id, PENDING_INSTRUMENT));
            }
        }
    }
    for (si, s) in t.chain.iter().enumerate() {
        if is_plugin(&s.effect) {
            let title = format!("{} {}", si + 1, crate::plugins::slot_name(s));
            if let Some(id) = plugin_lane_menu(ui, &title, s, ti, &has) {
                pick = Some(plugin_target(s, id, &format!("pending-{si}")));
            }
            continue;
        }
        let names: Vec<String> = match af2_dsp::Effect::from_id(&s.effect) {
            Some(e) => e.params().iter().map(|p| p.name.to_string()).collect(),
            None => af2_fx::info(&s.effect)
                .map(|i| i.params.iter().map(|p| p.name.to_string()).collect())
                .unwrap_or_default(),
        };
        if names.is_empty() {
            continue;
        }
        let title = af2_dsp::Effect::from_id(&s.effect)
            .map(|e| e.name().to_string())
            .or_else(|| af2_fx::info(&s.effect).map(|i| i.name.to_string()))
            .unwrap_or_else(|| s.effect.clone());
        ui.menu_button(format!("{} {}", si + 1, title), |ui| {
            for (pi, name) in names.iter().enumerate() {
                if ui.button(name).clicked() {
                    pick = Some(LaneTarget::Effect {
                        slot: s.ext_id.clone().unwrap_or_else(|| format!("pending-{si}")),
                        param: pi,
                        key: af2_fx::info(&s.effect)
                            .and_then(|i| i.params.get(pi).map(|p| p.key.to_string())),
                    });
                    ui.close_menu();
                }
            }
        });
    }
    if let Some(mut tg) = pick {
        // An effect lane names its slot by id: give the slot one.
        if let LaneTarget::Effect { slot, .. } = &mut tg {
            if let Some(si) = slot
                .strip_prefix("pending-")
                .and_then(|n| n.parse::<usize>().ok())
            {
                let id = af2_interop::audfrm::new_uuid();
                st.session.tracks[ti].chain[si].ext_id = Some(id.clone());
                *slot = id;
            } else if slot == PENDING_INSTRUMENT {
                let id = af2_interop::audfrm::new_uuid();
                if let Some(s) = st.session.tracks[ti].instrument_plugin.as_mut() {
                    s.ext_id = Some(id.clone());
                }
                *slot = id;
            }
        }
        st.add_lane(ti, tg);
    }
}

// ------------------------------------------------------------ plug-in lanes
//
// Lanes on a VST3 plug-in's controls are effect lanes whose slot is a
// plug-in (the MIDI track's instrument or one in the chain) and whose
// value is the control's VST3 id; they hold the control's normalized
// value (0..1), as Audiform saves them (`effect_param`, key `p<id>`).

/// An instrument lane picked before the instrument slot has an id.
const PENDING_INSTRUMENT: &str = "pending-instrument";

fn is_plugin(effect: &str) -> bool {
    af2_core::plugin::is_plugin(effect)
}

/// The slots a track's effect lanes can name: its chain, then (a MIDI
/// track) its instrument plug-in.
pub fn lane_slots(t: &af2_core::Track) -> Vec<af2_core::EffectSlot> {
    let mut v = t.chain.clone();
    if t.kind == TrackKind::Midi {
        v.extend(t.instrument_plugin.clone());
    }
    v
}

/// The plug-in slot with id `slot` among `slots`.
fn plugin_in<'a>(
    slots: &'a [af2_core::EffectSlot],
    slot: &str,
) -> Option<&'a af2_core::EffectSlot> {
    slots
        .iter()
        .find(|s| is_plugin(&s.effect) && s.ext_id.as_deref() == Some(slot))
}

/// The plug-in slot with id `slot` on track `t` (chain or instrument).
fn plugin_slot_of<'a>(t: &'a af2_core::Track, slot: &str) -> Option<&'a af2_core::EffectSlot> {
    let inst = t.instrument_plugin.iter().filter(|_| t.kind == TrackKind::Midi);
    t.chain
        .iter()
        .chain(inst)
        .find(|s| is_plugin(&s.effect) && s.ext_id.as_deref() == Some(slot))
}

/// A lane target on control `id` of plug-in slot `s` (`pending` names
/// the slot until it has an id).
pub(crate) fn plugin_target(s: &af2_core::EffectSlot, id: u32, pending: &str) -> LaneTarget {
    LaneTarget::Effect {
        slot: s.ext_id.clone().unwrap_or_else(|| pending.to_string()),
        param: id as usize,
        key: Some(af2_core::plugin::param_key(id)),
    }
}

type ParamList = std::rc::Rc<Vec<af2_vst3::protocol::ParamInfo>>;

thread_local! {
    /// Each plug-in copy's controls, fetched at most once a second (a
    /// plug-in can have thousands).
    static PARAMS: std::cell::RefCell<HashMap<String, (std::time::Instant, ParamList)>> =
        Default::default();
}

/// A plug-in slot's controls: as its running copy reported them, else as
/// the scan read them (empty when it is not installed).
pub fn plugin_params(s: &af2_core::EffectSlot) -> ParamList {
    let inst = s.plugin_data().inst;
    let key = format!("{}#{}", s.effect, inst);
    PARAMS.with(|m| {
        let mut m = m.borrow_mut();
        if let Some((at, list)) = m.get(&key) {
            if at.elapsed().as_secs_f64() < 1.0 {
                return list.clone();
            }
        }
        let list: ParamList = std::rc::Rc::new(
            af2_vst3::manager()
                .get(&inst)
                .map(|i| i.params())
                .filter(|p| !p.is_empty())
                .or_else(|| crate::plugins::found(&s.effect).map(|f| f.info.params))
                .unwrap_or_default(),
        );
        m.insert(key, (std::time::Instant::now(), list.clone()));
        list
    })
}

/// Control `id` of plug-in slot `s`, when it is known.
fn plugin_param(s: &af2_core::EffectSlot, id: u32) -> Option<af2_vst3::protocol::ParamInfo> {
    plugin_params(s).iter().find(|p| p.id == id).cloned()
}

/// Control `id` of plug-in slot `s` now (0..1): the running copy's, the
/// saved one, or the control's default.
pub(crate) fn plugin_value(s: &af2_core::EffectSlot, id: u32) -> f64 {
    let d = s.plugin_data();
    af2_vst3::manager()
        .get(&d.inst)
        .and_then(|i| i.value(id))
        .or_else(|| d.param(id))
        .or_else(|| plugin_param(s, id).map(|p| p.default))
        .unwrap_or(0.0)
}

/// A plug-in control's normalized value as the readout shows it: on or
/// off for a switch, the step for a stepped control, else a percentage.
pub fn plugin_value_text(p: Option<&af2_vst3::protocol::ParamInfo>, v: f64) -> String {
    let v = v.clamp(0.0, 1.0);
    match p.map_or(0, |p| p.steps) {
        1 => if v >= 0.5 { "On" } else { "Off" }.into(),
        n if n > 1 => format!(
            "Step {} of {}",
            (v * n as f64).round() as i64 + 1,
            n as i64 + 1
        ),
        _ => format!("{:.1} %", v * 100.0),
    }
}

/// A value on a stepped plug-in control's nearest step (others as they
/// are).
fn plugin_snap(chain: &[af2_core::EffectSlot], target: &LaneTarget, raw: f64) -> Option<f64> {
    let LaneTarget::Effect { slot, param, .. } = target else {
        return None;
    };
    let s = plugin_in(chain, slot)?;
    let n = plugin_param(s, *param as u32)?.steps;
    (n > 0).then(|| (raw.clamp(0.0, 1.0) * n as f64).round() / n as f64)
}

/// The control of plug-in copy `inst` last moved in its own window, if
/// that was in the last two minutes.
fn last_touched(inst: &str) -> Option<u32> {
    let (id, at) = af2_vst3::manager().get(inst)?.last_turned()?;
    (at.elapsed().as_secs() < 120).then_some(id)
}

/// One plug-in's controls as a submenu: the control last moved in its
/// own window first, a search box for long lists. Returns the id picked.
pub(crate) fn plugin_lane_menu(
    ui: &mut egui::Ui,
    title: &str,
    s: &af2_core::EffectSlot,
    ti: usize,
    has: &dyn Fn(&LaneTarget) -> bool,
) -> Option<u32> {
    let mut picked = None;
    ui.menu_button(title, |ui| {
        let all = plugin_params(s);
        let params: Vec<&af2_vst3::protocol::ParamInfo> =
            all.iter().filter(|p| !p.hidden).collect();
        if params.is_empty() {
            ui.label("No controls to automate (the plug-in is not running or not installed).");
            return;
        }
        let shown = |id: u32| {
            if has(&plugin_target(s, id, "")) {
                " (shown)"
            } else {
                ""
            }
        };
        let inst = s.plugin_data().inst;
        let touched = last_touched(&inst).and_then(|id| params.iter().find(|p| p.id == id));
        match touched {
            Some(p) => {
                if ui
                    .button(format!("Last touched: {}{}", p.title, shown(p.id)))
                    .on_hover_text("The control you last moved in the plug-in's own window.")
                    .clicked()
                {
                    picked = Some(p.id);
                    ui.close_menu();
                }
            }
            None => {
                ui.label(
                    egui::RichText::new(
                        "Move a control in the plug-in's window, then open this menu again to pick it.",
                    )
                    .small()
                    .weak(),
                );
            }
        }
        ui.separator();
        let fid = egui::Id::new(("af2-plugin-lane-find", ti, inst.as_str()));
        let mut find: String = ui.data(|d| d.get_temp(fid)).unwrap_or_default();
        if params.len() > 12 {
            ui.add(
                egui::TextEdit::singleline(&mut find)
                    .hint_text("Find a control")
                    .desired_width(180.0),
            );
            ui.data_mut(|d| d.insert_temp(fid, find.clone()));
        }
        let needle = find.to_lowercase();
        crate::plugins::tall_list(ui, fid.with("list"), |ui| {
            for p in params
                .iter()
                .filter(|p| needle.is_empty() || p.title.to_lowercase().contains(&needle))
            {
                let name = if p.title.is_empty() {
                    format!("Control {}", p.id)
                } else {
                    p.title.clone()
                };
                if ui.button(format!("{name}{}", shown(p.id))).clicked() {
                    picked = Some(p.id);
                    ui.close_menu();
                }
            }
        });
    });
    picked
}

#[cfg(test)]
mod plugin_lane_tests {
    use super::*;
    use af2_vst3::protocol::ParamInfo;

    fn plug(id: &str, params: &[(u32, f64)]) -> af2_core::EffectSlot {
        let mut d = af2_core::plugin::PluginData {
            inst: format!("test-{id}"),
            ..Default::default()
        };
        for &(k, v) in params {
            d.set_param(k, v);
        }
        af2_core::EffectSlot {
            position: None,
            effect: "vst3:00000000000000000000000000000000".into(),
            values: Vec::new(),
            bypass: false,
            sidechain: None,
            data: Some(d.to_json()),
            ext_id: Some(id.into()),
        }
    }

    #[test]
    fn instrument_and_chain_plugin_lanes_read_their_controls() {
        let mut s = Session::new("t", 48_000, 0);
        let tid = s.add_track_of(TrackKind::Midi);
        let t = s.track_mut(tid).unwrap();
        t.instrument_plugin = Some(plug("inst", &[(42, 0.3)]));
        t.chain.push(plug("fx", &[(7, 0.8)]));
        let t = t.clone();
        let inst = plugin_target(t.instrument_plugin.as_ref().unwrap(), 42, "x");
        let fx = plugin_target(&t.chain[0], 7, "x");
        assert_eq!(
            inst,
            LaneTarget::Effect {
                slot: "inst".into(),
                param: 42,
                key: Some("p42".into())
            }
        );
        // The saved values (no running copy in tests).
        assert!((control_value(&t, &inst) - 0.3).abs() < 1e-9);
        assert!((control_value(&t, &fx) - 0.8).abs() < 1e-9);
        let slots = lane_slots(&t);
        assert_eq!(lane_range_in(&slots, &inst), (0.0, 1.0));
        assert_eq!(lane_range_in(&slots, &fx), (0.0, 1.0));
        // Not installed here: named by id, resting at 0.
        assert_eq!(lane_label_in(&slots, &inst), "Control 42");
        assert_eq!(lane_default_in(&slots, &inst), 0.0);
        assert_eq!(lane_value_text_in(&slots, &fx, 0.25), "25.0 %");
        // An audio track's instrument slot is not a lane target.
        let mut a = t.clone();
        a.kind = TrackKind::Audio;
        assert_eq!(lane_slots(&a).len(), 1);
        assert_eq!(control_value(&a, &inst), 0.0);
    }

    #[test]
    fn plugin_values_read_as_switches_steps_or_percent() {
        let info = |steps| ParamInfo {
            id: 1,
            title: "Cutoff".into(),
            units: String::new(),
            steps,
            default: 0.5,
            hidden: false,
        };
        assert_eq!(plugin_value_text(Some(&info(1)), 0.7), "On");
        assert_eq!(plugin_value_text(Some(&info(1)), 0.2), "Off");
        assert_eq!(plugin_value_text(Some(&info(3)), 2.0 / 3.0), "Step 3 of 4");
        assert_eq!(plugin_value_text(Some(&info(0)), 0.5), "50.0 %");
        assert_eq!(plugin_value_text(None, 1.5), "100.0 %");
    }
}
