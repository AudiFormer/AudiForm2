// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

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
//! `af27_core::automation::AutoWriter`). The lanes being written play
//! from the live control, so nothing is rebuilt while writing; the
//! written points go into the session when play stops, as one undo step
//! with the moves that made them.

use std::collections::HashMap;

use af27_core::automation::{
    AutoWriter, AutomationMode, Curve, Lane, LanePoint, LaneTarget, Owner, Written,
};
use af27_core::{Session, TrackId, TrackKind};
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

fn kind_of(target: &LaneTarget) -> Option<af27_engine::mix::LaneKind> {
    match target {
        LaneTarget::Volume => Some(af27_engine::mix::LaneKind::Volume),
        LaneTarget::Pan => Some(af27_engine::mix::LaneKind::Pan),
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
        af27_core::automation::apply(&mut self.session, &done);
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
pub fn control_value(t: &af27_core::Track, target: &LaneTarget) -> f64 {
    match target {
        LaneTarget::Volume => t.volume_db as f64,
        LaneTarget::Pan => t.pan as f64,
        LaneTarget::Effect { slot, param, .. } => t
            .chain
            .iter()
            .find(|s| s.ext_id.as_deref() == Some(slot.as_str()))
            .and_then(|s| s.values.get(*param))
            .map_or(0.0, |v| *v as f64),
        LaneTarget::Space(_) => 0.0,
    }
}

/// The range a lane is drawn over: its target's, widened for effect
/// values to the effect's own range when known.
fn lane_range(t: &af27_core::Track, target: &LaneTarget) -> (f64, f64) {
    if let LaneTarget::Effect { slot, param, .. } = target {
        if let Some(s) = t
            .chain
            .iter()
            .find(|s| s.ext_id.as_deref() == Some(slot.as_str()))
        {
            if let Some(p) = af27_dsp::Effect::from_id(&s.effect)
                .map(|e| e.params())
                .and_then(|ps| ps.get(*param).map(|p| (p.min as f64, p.max as f64)))
            {
                return p;
            }
            if let Some(p) =
                af27_fx::info(&s.effect).and_then(|i| i.params.get(*param).map(|p| (p.min, p.max)))
            {
                return p;
            }
        }
    }
    target.range()
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
    let y_of = |val: f64| {
        let x = ((val - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0);
        area.bottom() - x as f32 * area.height()
    };
    let val_of = |y: f32| {
        let x = ((area.bottom() - y) / area.height().max(1.0)).clamp(0.0, 1.0);
        lo + (hi - lo) * x as f64
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
        format!("{} ({})", lane.target.label(), t.automation.label()),
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
        let lane = &mut st.session.tracks[ti].lanes[k];
        if let Some(p) = lane.points.get_mut(*i) {
            p.time = x_to_time(v, lanes, pos.x).max(0.0);
            p.value = val_of(pos.y);
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
        "Click: add a point. Drag: move one. Right-click: remove. Double-click: hold or ramp.",
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
    for (si, s) in t.chain.iter().enumerate() {
        let names: Vec<String> = match af27_dsp::Effect::from_id(&s.effect) {
            Some(e) => e.params().iter().map(|p| p.name.to_string()).collect(),
            None => af27_fx::info(&s.effect)
                .map(|i| i.params.iter().map(|p| p.name.to_string()).collect())
                .unwrap_or_default(),
        };
        if names.is_empty() {
            continue;
        }
        let title = af27_dsp::Effect::from_id(&s.effect)
            .map(|e| e.name().to_string())
            .or_else(|| af27_fx::info(&s.effect).map(|i| i.name.to_string()))
            .unwrap_or_else(|| s.effect.clone());
        ui.menu_button(format!("{} {}", si + 1, title), |ui| {
            for (pi, name) in names.iter().enumerate() {
                if ui.button(name).clicked() {
                    pick = Some(LaneTarget::Effect {
                        slot: s.ext_id.clone().unwrap_or_else(|| format!("pending-{si}")),
                        param: pi,
                        key: af27_fx::info(&s.effect)
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
                let id = af27_interop::audfrm::new_uuid();
                st.session.tracks[ti].chain[si].ext_id = Some(id.clone());
                *slot = id;
            }
        }
        st.add_lane(ti, tg);
    }
}
