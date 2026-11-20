// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! A clip's own automation, drawn over the clip in the Multitrack: the
//! shown lane's line (volume, pan or one of the clip's effect values)
//! with its points. A click on the line adds a point, a point drags
//! (between its neighbours, inside the clip), right-click removes it and
//! double-click switches it between a ramp and a hold. The lanes count
//! from the clip's left edge, so they move with the clip.

use std::collections::{HashMap, HashSet};

use af27_core::automation::{Curve, Lane, LanePoint, LaneTarget};
use af27_core::{ClipId, Session};
use egui::{pos2, vec2, Color32, CursorIcon, Rect, Sense, Stroke};

use crate::state::{State, View};
use crate::waveform::{time_to_x, x_to_time};

/// Which lane each clip shows (UI only, not saved).
#[derive(Default)]
pub struct ClipLanesUi {
    /// The lane shown, by index into the clip's lanes (the first when
    /// not set).
    pub shown: HashMap<ClipId, usize>,
    /// Clips whose lanes are hidden.
    pub hidden: HashSet<ClipId>,
}

impl State {
    /// The lane clip `id` shows over itself, if any.
    pub fn shown_clip_lane(&self, id: ClipId) -> Option<usize> {
        if self.clip_lanes.hidden.contains(&id) {
            return None;
        }
        let (ti, ci) = self.session.find_clip(id)?;
        let n = self.session.tracks[ti].clips[ci].props.lanes.len();
        if n == 0 {
            return None;
        }
        Some(
            self.clip_lanes
                .shown
                .get(&id)
                .copied()
                .unwrap_or(0)
                .min(n - 1),
        )
    }

    /// Adds (or shows) a lane for `target` on clip `id`.
    pub fn add_clip_lane(&mut self, id: ClipId, target: LaneTarget) {
        let before = self.session.clone();
        let Some((ti, ci)) = self.session.find_clip(id) else {
            return;
        };
        let lanes = &mut self.session.tracks[ti].clips[ci].props.lanes;
        let k = match lanes.iter().position(|l| l.target == target) {
            Some(k) => k,
            None => {
                lanes.push(Lane::new(target));
                lanes.len() - 1
            }
        };
        self.clip_lanes.shown.insert(id, k);
        self.clip_lanes.hidden.remove(&id);
        if self.session != before {
            self.commit_session("Add Clip Automation", before);
        }
    }
}

/// A lane value as the line's tooltip shows it.
fn value_text(target: &LaneTarget, v: f64) -> String {
    match target {
        LaneTarget::Volume => format!("{v:+.1} dB"),
        LaneTarget::Pan => {
            let p = (v * 100.0).round();
            if p == 0.0 {
                "Centre".into()
            } else if p < 0.0 {
                format!("L {:.0}", -p)
            } else {
                format!("R {p:.0}")
            }
        }
        _ => format!("{v:.2}"),
    }
}

/// The value an empty lane of `target` draws at: unity, centre, or the
/// effect value as set.
fn resting(c: &af27_core::Clip, target: &LaneTarget) -> f64 {
    match target {
        LaneTarget::Effect { slot, param, .. } => c
            .props
            .chain
            .iter()
            .find(|s| s.ext_id.as_deref() == Some(slot.as_str()))
            .and_then(|s| s.values.get(*param))
            .map_or(0.0, |v| *v as f64),
        _ => 0.0,
    }
}

/// Draws clip `id`'s shown lane over it (in track row `row`) and edits it.
pub fn overlay(ui: &mut egui::Ui, st: &mut State, id: ClipId, row: Rect, v: &View, lanes: Rect) {
    let Some(k) = st.shown_clip_lane(id) else {
        return;
    };
    let Some((ti, ci)) = st.session.find_clip(id) else {
        return;
    };
    let clip = st.session.tracks[ti].clips[ci].clone();
    let lane = &clip.props.lanes[k];
    let x0 = time_to_x(v, lanes, clip.start);
    let x1 = time_to_x(v, lanes, clip.end());
    // Under the clip's title bar, clear of its bottom edge.
    let body = Rect::from_min_max(pos2(x0, row.top() + 21.0), pos2(x1, row.bottom() - 7.0));
    if body.width() < 10.0 || body.height() < 12.0 {
        return;
    }
    let th = st.theme.clone();
    let (lo, hi) = crate::mixing::lane_range_in(&clip.props.chain, &lane.target);
    let len = clip.length;
    let y_of = |val: f64| {
        let x = ((val - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0);
        body.bottom() - x as f32 * body.height()
    };
    let val_of = |y: f32| {
        let x = ((body.bottom() - y) / body.height().max(1.0)).clamp(0.0, 1.0);
        lo + (hi - lo) * x as f64
    };
    let x_of = |t: f64| time_to_x(v, lanes, clip.start + t);
    let t_of = |x: f32| (x_to_time(v, lanes, x) - clip.start).clamp(0.0, len);
    let col = if lane.enabled {
        Color32::from_rgb(th.accent_alt[0], th.accent_alt[1], th.accent_alt[2])
    } else {
        Color32::from_gray(130)
    };
    let painter = ui.painter_at(lanes.intersect(body.expand(5.0)));
    let pts = &lane.points;
    // The line across the clip: the value at each edge, the points
    // inside, and the steps of held points.
    let rest = resting(&clip, &lane.target);
    let at = |t: f64| lane.value_at(t).unwrap_or(rest);
    let inside: Vec<(usize, &LanePoint)> = pts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.time >= 0.0 && p.time <= len)
        .collect();
    let mut line = vec![pos2(x0, y_of(at(0.0)))];
    for (i, p) in &inside {
        if *i > 0 && pts[i - 1].curve == Curve::Hold {
            line.push(pos2(x_of(p.time), y_of(pts[i - 1].value)));
        }
        line.push(pos2(x_of(p.time), y_of(p.value)));
    }
    line.push(pos2(x1, y_of(at(len))));
    painter.add(egui::Shape::line(line, Stroke::new(1.5_f32, col)));
    for (_, p) in &inside {
        painter.circle_filled(pos2(x_of(p.time), y_of(p.value)), 3.0, col);
    }
    if body.width() > 60.0 {
        painter.text(
            body.left_top() + vec2(4.0, 1.0),
            egui::Align2::LEFT_TOP,
            lane.target.label(),
            egui::FontId::proportional(9.5),
            col,
        );
    }

    // Editing.
    let drag_key = egui::Id::new(("af2-clip-lane-drag", id));
    let mut edited: Option<(&str, Session)> = None;
    let mut on_point = false;
    for (i, p) in inside.iter().map(|(i, p)| (*i, **p)) {
        let at_pt = pos2(x_of(p.time), y_of(p.value));
        // Only inside the lanes (never over the track headers); a point
        // scrolled out of view keeps its id, so a drag carries on.
        let r = ui.interact(
            Rect::from_center_size(at_pt, vec2(11.0, 11.0)).intersect(lanes),
            egui::Id::new(("af2-clip-pt", id, k, i)),
            Sense::click_and_drag(),
        );
        if r.hovered() || r.dragged() {
            on_point = true;
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
            painter.circle_stroke(at_pt, 5.0, Stroke::new(1.5_f32, col));
        }
        if r.drag_started() {
            let before = st.session.clone();
            ui.data_mut(|d| d.insert_temp(drag_key, before));
        }
        if r.dragged() {
            if let Some(pos) = r.interact_pointer_pos() {
                let (lo_t, hi_t) = (
                    i.checked_sub(1).map_or(0.0, |j| pts[j].time.max(0.0)),
                    pts.get(i + 1).map_or(len, |q| q.time.min(len)),
                );
                let pl = &mut st.session.tracks[ti].clips[ci].props.lanes[k].points[i];
                pl.time = t_of(pos.x).clamp(lo_t, hi_t.max(lo_t));
                pl.value = val_of(pos.y);
                st.session_version += 1;
            }
            r.clone().on_hover_text(value_text(
                &lane.target,
                val_of(ui.input(|i| i.pointer.hover_pos().map_or(at_pt.y, |p| p.y))),
            ));
        }
        if r.drag_stopped() {
            if let Some(b) = ui.data_mut(|d| d.remove_temp::<Session>(drag_key)) {
                edited = Some(("Move Clip Automation Point", b));
            }
        }
        if r.secondary_clicked() {
            let before = st.session.clone();
            st.session.tracks[ti].clips[ci].props.lanes[k]
                .points
                .remove(i);
            edited = Some(("Remove Clip Automation Point", before));
            break;
        }
        if r.double_clicked() {
            let before = st.session.clone();
            let pm = &mut st.session.tracks[ti].clips[ci].props.lanes[k].points[i];
            pm.curve = match pm.curve {
                Curve::Linear => Curve::Hold,
                _ => Curve::Linear,
            };
            edited = Some(("Edit Clip Automation", before));
            break;
        }
        if !r.dragged() {
            r.on_hover_text(format!(
                "{}. Drag to move, right-click to remove, double-click for a hold.",
                value_text(&lane.target, p.value)
            ));
        }
    }
    // Near the line (not on a point): a click adds a point there.
    let hover = ui.input(|i| i.pointer.hover_pos());
    if let (Some(hp), false, None) = (hover, on_point, edited.as_ref()) {
        let near_line = body.expand2(vec2(0.0, 4.0)).contains(hp)
            && lanes.contains(hp)
            && (hp.y - y_of(at(t_of(hp.x)))).abs() < 5.0;
        if near_line {
            let r = ui.interact(
                Rect::from_center_size(hp, vec2(12.0, 12.0)),
                egui::Id::new(("af2-clip-line", id, k)),
                Sense::click(),
            );
            if r.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
            }
            if r.clicked() {
                let before = st.session.clone();
                let t = t_of(hp.x);
                // On the line: the value it has there (a click never jumps).
                let value = if pts.is_empty() { rest } else { at(t) };
                let l = &mut st.session.tracks[ti].clips[ci].props.lanes[k];
                l.points.push(LanePoint {
                    time: t,
                    value,
                    curve: Curve::Linear,
                });
                l.sort();
                edited = Some(("Add Clip Automation Point", before));
            }
            r.on_hover_text(format!(
                "{}: {}. Click to add a point.",
                lane.target.label(),
                value_text(&lane.target, at(t_of(hp.x)))
            ));
        }
    }
    if let Some((label, before)) = edited {
        st.commit_session(label, before);
    }
}

/// The clip menu's Clip Automation entries: show or add volume, pan and
/// each value of the clip's own effects; pick the lane shown; hide;
/// remove.
pub fn menu(ui: &mut egui::Ui, st: &mut State, id: ClipId) {
    let Some((ti, ci)) = st.session.find_clip(id) else {
        return;
    };
    let c = st.session.tracks[ti].clips[ci].clone();
    let shown = st.shown_clip_lane(id);
    let mut pick: Option<LaneTarget> = None;
    let mut targets = vec![LaneTarget::Volume];
    if st.session.space.is_none() {
        targets.push(LaneTarget::Pan);
    }
    for tg in targets {
        let k = c.props.lanes.iter().position(|l| l.target == tg);
        let on = k.is_some() && k == shown;
        if ui.selectable_label(on, tg.label()).clicked() {
            pick = Some(tg);
            ui.close_menu();
        }
    }
    for (si, s) in c.props.chain.iter().enumerate() {
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
                let new = af27_core::session::new_uuid();
                st.session.tracks[ti].clips[ci].props.chain[si].ext_id = Some(new.clone());
                *slot = new;
            }
        }
        st.add_clip_lane(id, tg);
    }
    if c.props.lanes.is_empty() {
        return;
    }
    ui.separator();
    let hidden = st.clip_lanes.hidden.contains(&id);
    if ui
        .selectable_label(!hidden, "Show Clip Automation")
        .clicked()
    {
        if hidden {
            st.clip_lanes.hidden.remove(&id);
        } else {
            st.clip_lanes.hidden.insert(id);
        }
        ui.close_menu();
    }
    ui.menu_button("Remove Lane", |ui| {
        for (k, l) in c.props.lanes.iter().enumerate() {
            if ui.button(l.target.label()).clicked() {
                let before = st.session.clone();
                if let Some((ti, ci)) = st.session.find_clip(id) {
                    st.session.tracks[ti].clips[ci].props.lanes.remove(k);
                    st.clip_lanes.shown.remove(&id);
                    st.commit_session("Remove Clip Automation", before);
                }
                ui.close_menu();
            }
        }
    });
}
