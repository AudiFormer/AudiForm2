// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! A clip's own automation, drawn over the clip in the Multitrack: every
//! audio clip's volume and pan lines (resting at unity and centre until
//! the clip has a lane for them), and the effect value picked in the clip
//! menu, with their points. A click on a line adds a point (the first
//! makes the clip's lane, so an untouched clip saves nothing new), a
//! point drags (between its neighbours, inside the clip), right-click
//! removes it and double-click switches it between a ramp and a hold.
//! The lanes count from the clip's left edge, so they move with the clip.

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

/// One line drawn over a clip: its target, the lane holding it (None
/// while the clip has no lane for it yet: the line rests at unity or
/// centre, and the lane is made by the first point) and its colour.
struct Line {
    target: LaneTarget,
    lane: Option<usize>,
    col: Color32,
}

/// The lines clip `c` shows: its volume and pan lines (every audio clip
/// has them, while they are on in the clip menu), and the lane picked in
/// the clip menu when that is one of its effect values.
fn lines(st: &State, c: &af27_core::Clip) -> Vec<Line> {
    if st.clip_lanes.hidden.contains(&c.id) {
        return Vec::new();
    }
    let th = &st.theme;
    let rgb = |x: [u8; 3]| Color32::from_rgb(x[0], x[1], x[2]);
    let shown = st.shown_clip_lane(c.id);
    let mut out = Vec::new();
    if st.prefs.clip_lines {
        let mut targets = vec![(LaneTarget::Volume, rgb(th.accent_alt))];
        if st.session.space.is_none() {
            targets.push((LaneTarget::Pan, rgb(th.accent)));
        }
        for (target, col) in targets {
            let lane = c.props.lanes.iter().position(|l| l.target == target);
            out.push(Line { target, lane, col });
        }
        if let Some(k) = shown {
            let t = &c.props.lanes[k].target;
            if !matches!(t, LaneTarget::Volume | LaneTarget::Pan) {
                out.push(Line {
                    target: t.clone(),
                    lane: Some(k),
                    col: rgb(th.marker),
                });
            }
        }
    } else if let Some(k) = shown {
        // Off: only the lane picked in the clip menu.
        out.push(Line {
            target: c.props.lanes[k].target.clone(),
            lane: Some(k),
            col: rgb(th.accent_alt),
        });
    }
    out
}

/// Draws clip `id`'s lines over it (in track row `row`) and edits them:
/// a click on a line adds a point (the clip's first point on a line makes
/// its lane), points drag, right-click removes one, double-click holds.
/// `free` is false while the pointer is on one of the clip's handles or
/// edges, or something else is being dragged: the lines then leave the
/// pointer alone (a point already being dragged carries on).
pub fn overlay(
    ui: &mut egui::Ui,
    st: &mut State,
    id: ClipId,
    row: Rect,
    v: &View,
    lanes: Rect,
    free: bool,
) {
    let Some((ti, ci)) = st.session.find_clip(id) else {
        return;
    };
    let clip = st.session.tracks[ti].clips[ci].clone();
    let shown = lines(st, &clip);
    if shown.is_empty() {
        return;
    }
    let x0 = time_to_x(v, lanes, clip.start);
    let x1 = time_to_x(v, lanes, clip.end());
    // Under the clip's title bar, clear of its bottom edge.
    let body = Rect::from_min_max(pos2(x0, row.top() + 21.0), pos2(x1, row.bottom() - 7.0));
    if body.width() < 10.0 || body.height() < 12.0 {
        return;
    }
    let len = clip.length;
    let x_of = |t: f64| time_to_x(v, lanes, clip.start + t);
    let t_of = |x: f32| (x_to_time(v, lanes, x) - clip.start).clamp(0.0, len);
    let painter = ui.painter_at(lanes.intersect(body.expand(5.0)));
    let hover = ui.input(|i| i.pointer.hover_pos());
    let mut edited: Option<(&str, Session)> = None;
    let mut on_point = false;
    // The line nearest the pointer, for a click to add a point to:
    // (distance, line index, value there).
    let mut nearest: Option<(f32, usize, f64)> = None;
    for (li, line) in shown.iter().enumerate() {
        let empty = Lane::new(line.target.clone());
        let lane = line.lane.map_or(&empty, |k| &clip.props.lanes[k]);
        let (lo, hi) = crate::mixing::lane_range_in(&clip.props.chain, &lane.target);
        let y_of = |val: f64| {
            let x = ((val - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0);
            body.bottom() - x as f32 * body.height()
        };
        let val_of = |y: f32| {
            let x = ((body.bottom() - y) / body.height().max(1.0)).clamp(0.0, 1.0);
            lo + (hi - lo) * x as f64
        };
        let pts = &lane.points;
        // A line with no points yet is drawn quieter.
        let col = if !lane.enabled {
            Color32::from_gray(130)
        } else if pts.is_empty() {
            line.col.gamma_multiply(0.6)
        } else {
            line.col
        };
        // The line across the clip: the value at each edge, the points
        // inside, and the steps of held points.
        let rest = resting(&clip, &lane.target);
        let at = |t: f64| lane.value_at(t).unwrap_or(rest);
        let inside: Vec<(usize, LanePoint)> = pts
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, p)| p.time >= 0.0 && p.time <= len)
            .collect();
        let mut path = vec![pos2(x0, y_of(at(0.0)))];
        for (i, p) in &inside {
            if *i > 0 && pts[i - 1].curve == Curve::Hold {
                path.push(pos2(x_of(p.time), y_of(pts[i - 1].value)));
            }
            path.push(pos2(x_of(p.time), y_of(p.value)));
        }
        path.push(pos2(x1, y_of(at(len))));
        let width = if pts.is_empty() { 1.0_f32 } else { 1.5 };
        painter.add(egui::Shape::line(path, Stroke::new(width, col)));
        for (_, p) in &inside {
            painter.circle_filled(pos2(x_of(p.time), y_of(p.value)), 3.0, col);
        }
        // Effect lanes (not volume or pan) say what they are.
        if body.width() > 60.0 && !matches!(lane.target, LaneTarget::Volume | LaneTarget::Pan) {
            painter.text(
                body.left_top() + vec2(4.0, 1.0),
                egui::Align2::LEFT_TOP,
                lane.target.label(),
                egui::FontId::proportional(9.5),
                col,
            );
        }
        // How close the pointer is to this line (a click adds a point to
        // the nearest one), with the value the line has there (so a new
        // point never jumps).
        if let Some(hp) = hover {
            let t = t_of(hp.x);
            let d = (hp.y - y_of(at(t))).abs();
            if nearest.is_none_or(|n| d < n.0) {
                nearest = Some((d, li, at(t)));
            }
        }
        let Some(k) = line.lane else {
            // No lane yet: nothing to drag.
            continue;
        };
        if edited.is_some() {
            continue;
        }

        // The points.
        let drag_key = egui::Id::new(("af2-clip-lane-drag", id, k));
        for (i, p) in inside.iter().copied() {
            let at_pt = pos2(x_of(p.time), y_of(p.value));
            let pid = egui::Id::new(("af2-clip-pt", id, k, i));
            // Never over a handle, unless already being dragged.
            if !free && !ui.ctx().is_being_dragged(pid) {
                continue;
            }
            // Only inside the lanes (never over the track headers); a
            // point scrolled out of view keeps its id, so a drag carries on.
            let r = ui.interact(
                Rect::from_center_size(at_pt, vec2(11.0, 11.0)).intersect(lanes),
                pid,
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
                st.selected_clip = Some(id);
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
                    "{} {}. Drag to move, right-click to remove, double-click for a hold.",
                    lane.target.label(),
                    value_text(&lane.target, p.value)
                ));
            }
        }
    }
    // Near a line (not on a point): a click adds a point there.
    if let (true, false, None, Some(hp), Some((d, li, value))) =
        (free, on_point, edited.as_ref(), hover, nearest)
    {
        let near_line = d < 4.5 && body.expand2(vec2(0.0, 4.0)).contains(hp) && lanes.contains(hp);
        if near_line {
            let line = &shown[li];
            let r = ui.interact(
                Rect::from_center_size(hp, vec2(12.0, 12.0)),
                egui::Id::new(("af2-clip-line", id)),
                Sense::click(),
            );
            if r.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                painter.circle_stroke(hp, 3.5, Stroke::new(1.0_f32, line.col));
            }
            if r.clicked() {
                let before = st.session.clone();
                let t = t_of(hp.x);
                let ls = &mut st.session.tracks[ti].clips[ci].props.lanes;
                // The clip's first point on this line makes its lane.
                let k = match line.lane {
                    Some(k) => k,
                    None => {
                        ls.push(Lane::new(line.target.clone()));
                        ls.len() - 1
                    }
                };
                ls[k].points.push(LanePoint {
                    time: t,
                    value,
                    curve: Curve::Linear,
                });
                ls[k].sort();
                st.selected_clip = Some(id);
                edited = Some(("Add Clip Automation Point", before));
            }
            r.on_hover_text(format!(
                "{}: {}. Click to add a point.",
                line.target.label(),
                value_text(&line.target, value)
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
    ui.separator();
    let hidden = st.clip_lanes.hidden.contains(&id);
    if ui
        .selectable_label(!hidden, "Show on This Clip")
        .on_hover_text("This clip's volume and pan lines and its shown lane")
        .clicked()
    {
        if hidden {
            st.clip_lanes.hidden.remove(&id);
        } else {
            st.clip_lanes.hidden.insert(id);
        }
        ui.close_menu();
    }
    if ui
        .selectable_label(st.prefs.clip_lines, "Volume and Pan Lines on All Clips")
        .clicked()
    {
        st.prefs.clip_lines = !st.prefs.clip_lines;
        st.prefs.save();
        ui.close_menu();
    }
    if c.props.lanes.is_empty() {
        return;
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
