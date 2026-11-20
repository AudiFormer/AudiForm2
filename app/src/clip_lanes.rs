// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! A clip's own automation, drawn over the clip in the Multitrack. Each
//! audio clip shows the lines picked in the small menu at the right of
//! its title bar (or under Clip Automation in its right-click menu, the
//! same list): its volume and pan (on unless switched off), any value of
//! its own effects, and its effects rack's input, output, mix and power.
//! Which lines show is not saved; showing one adds nothing to the
//! session. A click on a line adds a point (the first makes the clip's
//! lane, so an untouched clip saves nothing new), a point drags (between
//! its neighbours, inside the clip, its value shown beside it, snapping
//! to the line's default unless Alt is held), right-click removes it and
//! double-click switches it between a ramp and a hold. The lanes count
//! from the clip's left edge, so they move with the clip.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use af27_core::automation::{Curve, Lane, LanePoint, LaneTarget, RackParam};
use af27_core::{ClipId, Session};
use egui::{pos2, vec2, Color32, CursorIcon, Rect, Sense, Stroke};

use crate::mixing::{drag_value, draw_readout, lane_label_in, lane_value_text_in};
use crate::state::{State, View};
use crate::waveform::{time_to_x, x_to_time};

/// Which lines each clip shows (UI only, not saved).
#[derive(Default)]
pub struct ClipLanesUi {
    /// The lines of clips whose list was changed from the default (see
    /// [`State::clip_lines_shown`]).
    pub shown: HashMap<ClipId, Vec<LaneTarget>>,
}

impl State {
    /// The lines clip `id` shows: as picked, or by default its volume and
    /// pan (while Volume and Pan Lines on All Clips is on) and any other
    /// lane it has points on. No pan in a 3D Space session.
    pub fn clip_lines_shown(&self, id: ClipId) -> Vec<LaneTarget> {
        let mut out = match self.clip_lanes.shown.get(&id) {
            Some(v) => v.clone(),
            None => {
                let mut v = Vec::new();
                if self.prefs.clip_lines {
                    v.push(LaneTarget::Volume);
                    v.push(LaneTarget::Pan);
                }
                if let Some((ti, ci)) = self.session.find_clip(id) {
                    for l in &self.session.tracks[ti].clips[ci].props.lanes {
                        if !l.points.is_empty() && !v.contains(&l.target) {
                            v.push(l.target.clone());
                        }
                    }
                }
                v
            }
        };
        if self.session.space.is_some() {
            out.retain(|t| *t != LaneTarget::Pan);
        }
        out
    }

    /// Shows or hides line `target` on clip `id`.
    pub fn toggle_clip_line(&mut self, id: ClipId, target: LaneTarget) {
        let mut v = self.clip_lines_shown(id);
        match v.iter().position(|t| *t == target) {
            Some(k) => {
                v.remove(k);
            }
            None => v.push(target),
        }
        self.clip_lanes.shown.insert(id, v);
    }
}

/// The short name a line goes by in the title bar's menu button.
fn short_name(c: &af27_core::Clip, target: &LaneTarget) -> String {
    match target {
        LaneTarget::Volume => "Vol".into(),
        LaneTarget::Pan => "Pan".into(),
        LaneTarget::Rack(RackParam::Input) => "In".into(),
        LaneTarget::Rack(RackParam::Output) => "Out".into(),
        LaneTarget::Rack(RackParam::Mix) => "Mix".into(),
        LaneTarget::Rack(RackParam::Power) => "Power".into(),
        _ => {
            let name = lane_label_in(&c.props.chain, target);
            if name.chars().count() > 10 {
                format!("{}.", name.chars().take(9).collect::<String>())
            } else {
                name
            }
        }
    }
}

/// The menu button's label: the lines shown ("Vol/Pan", "Mix", "Vol/Pan+2").
fn button_label(c: &af27_core::Clip, shown: &[LaneTarget]) -> String {
    let names: Vec<String> = shown.iter().map(|t| short_name(c, t)).collect();
    match names.len() {
        0 => "Lanes".into(),
        1 | 2 => names.join("/"),
        n => format!("{}/{}+{}", names[0], names[1], n - 2),
    }
}

/// An effect slot's title in the menus.
fn slot_title(s: &af27_core::EffectSlot) -> String {
    af27_dsp::Effect::from_id(&s.effect)
        .map(|e| e.name().to_string())
        .or_else(|| af27_fx::info(&s.effect).map(|i| i.name.to_string()))
        .unwrap_or_else(|| s.effect.clone())
}

/// An effect slot's value names.
fn slot_params(s: &af27_core::EffectSlot) -> Vec<String> {
    match af27_dsp::Effect::from_id(&s.effect) {
        Some(e) => e.params().iter().map(|p| p.name.to_string()).collect(),
        None => af27_fx::info(&s.effect)
            .map(|i| i.params.iter().map(|p| p.name.to_string()).collect())
            .unwrap_or_default(),
    }
}

/// The target of value `pi` of slot `si` of `chain`: by the slot's id, or
/// `pending-<si>` while it has none (it is given one with the line's
/// first point, so showing a line changes nothing saved).
fn effect_target(chain: &[af27_core::EffectSlot], si: usize, pi: usize) -> LaneTarget {
    let s = &chain[si];
    LaneTarget::Effect {
        slot: s.ext_id.clone().unwrap_or_else(|| format!("pending-{si}")),
        param: pi,
        key: af27_fx::info(&s.effect).and_then(|i| i.params.get(pi).map(|p| p.key.to_string())),
    }
}

/// `target` with a `pending-` slot named by the slot's id once it has one.
fn resolved(chain: &[af27_core::EffectSlot], target: &LaneTarget) -> LaneTarget {
    if let LaneTarget::Effect { slot, param, key } = target {
        if let Some(id) = slot
            .strip_prefix("pending-")
            .and_then(|n| n.parse::<usize>().ok())
            .and_then(|i| chain.get(i))
            .and_then(|s| s.ext_id.clone())
        {
            return LaneTarget::Effect {
                slot: id,
                param: *param,
                key: key.clone(),
            };
        }
    }
    target.clone()
}

/// The value an empty lane of `target` draws at: the clip's rack as set,
/// the effect value as set, or unity and centre.
fn resting(c: &af27_core::Clip, target: &LaneTarget) -> f64 {
    match target {
        LaneTarget::Effect { slot, param, .. } => {
            let by_id = c
                .props
                .chain
                .iter()
                .find(|s| s.ext_id.as_deref() == Some(slot.as_str()));
            let pending = || {
                slot.strip_prefix("pending-")
                    .and_then(|n| n.parse::<usize>().ok())
                    .and_then(|i| c.props.chain.get(i))
            };
            by_id
                .or_else(pending)
                .and_then(|s| s.values.get(*param))
                .map_or(0.0, |v| *v as f64)
        }
        LaneTarget::Rack(p) => p.value_in(&c.props.rack),
        _ => 0.0,
    }
}

/// One line drawn over a clip: its target, the lane holding it (None
/// while the clip has no lane for it yet: the line rests where its
/// control is, and the lane is made by the first point) and its colour.
struct Line {
    target: LaneTarget,
    lane: Option<usize>,
    col: Color32,
}

/// The lines clip `c` shows (see [`State::clip_lines_shown`]).
fn lines(st: &State, c: &af27_core::Clip) -> Vec<Line> {
    let th = &st.theme;
    let rgb = |x: [u8; 3]| Color32::from_rgb(x[0], x[1], x[2]);
    // Lines other than volume and pan take these in turn.
    let others = [
        rgb(th.marker),
        Color32::from_rgb(120, 205, 160),
        Color32::from_rgb(215, 140, 230),
        Color32::from_rgb(235, 175, 80),
    ];
    let mut n = 0;
    st.clip_lines_shown(c.id)
        .iter()
        .map(|t| {
            let target = resolved(&c.props.chain, t);
            let lane = c.props.lanes.iter().position(|l| l.target == target);
            let col = match target {
                LaneTarget::Volume => rgb(th.accent_alt),
                LaneTarget::Pan => rgb(th.accent),
                _ => {
                    n += 1;
                    others[(n - 1) % others.len()]
                }
            };
            Line { target, lane, col }
        })
        .collect()
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
    let th = st.theme.clone();
    let chain = &clip.props.chain;
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
    // The readout may sit a little outside the clip.
    let readout_area = lanes.intersect(row.expand2(vec2(80.0, 0.0)));
    let readout_painter = ui.painter_at(readout_area);
    let hover = ui.input(|i| i.pointer.hover_pos());
    let mut edited: Option<(&str, Session)> = None;
    let mut on_point = false;
    let mut readout: Option<(egui::Pos2, String)> = None;
    // The line nearest the pointer, for a click to add a point to:
    // (distance, line index, value there).
    let mut nearest: Option<(f32, usize, f64)> = None;
    let mut named = 0;
    for (li, line) in shown.iter().enumerate() {
        let empty = Lane::new(line.target.clone());
        let lane = line.lane.map_or(&empty, |k| &clip.props.lanes[k]);
        let (lo, hi) = crate::mixing::lane_range_in(chain, &lane.target);
        let y_of = |val: f64| {
            let x = ((val - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0);
            body.bottom() - x as f32 * body.height()
        };
        let val_of = |y: f32| {
            let x = ((body.bottom() - y) / body.height().max(1.0)).clamp(0.0, 1.0);
            lo + (hi - lo) * x as f64
        };
        let px_per_unit = body.height() as f64 / (hi - lo).max(1e-9);
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
        // Lines other than volume and pan say what they are.
        if body.width() > 60.0 && !matches!(lane.target, LaneTarget::Volume | LaneTarget::Pan) {
            painter.text(
                body.left_top() + vec2(4.0, 1.0 + 11.0 * named as f32),
                egui::Align2::LEFT_TOP,
                lane_label_in(chain, &lane.target),
                egui::FontId::proportional(9.5),
                col,
            );
            named += 1;
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
                let mut shown_at = (at_pt, p.value);
                if let Some(pos) = r.interact_pointer_pos() {
                    let (lo_t, hi_t) = (
                        i.checked_sub(1).map_or(0.0, |j| pts[j].time.max(0.0)),
                        pts.get(i + 1).map_or(len, |q| q.time.min(len)),
                    );
                    let value = drag_value(ui, chain, &lane.target, val_of(pos.y), px_per_unit);
                    let pl = &mut st.session.tracks[ti].clips[ci].props.lanes[k].points[i];
                    pl.time = t_of(pos.x).clamp(lo_t, hi_t.max(lo_t));
                    pl.value = value;
                    shown_at = (pos2(x_of(pl.time), y_of(value)), value);
                    st.session_version += 1;
                }
                readout = Some((
                    shown_at.0,
                    lane_value_text_in(chain, &lane.target, shown_at.1),
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
                    "{} {}. Drag to move (snaps to the default; hold Alt to move freely), right-click to remove, double-click for a hold.",
                    lane_label_in(chain, &lane.target),
                    lane_value_text_in(chain, &lane.target, p.value)
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
                let mut target = line.target.clone();
                // An effect value names its slot by id: give the slot one
                // (with this first point, never just for showing it).
                if let LaneTarget::Effect { slot, .. } = &mut target {
                    if let Some(si) = slot
                        .strip_prefix("pending-")
                        .and_then(|n| n.parse::<usize>().ok())
                    {
                        let new = af27_core::session::new_uuid();
                        st.session.tracks[ti].clips[ci].props.chain[si].ext_id = Some(new.clone());
                        *slot = new;
                        let old = line.target.clone();
                        let mut v = st.clip_lines_shown(id);
                        for x in v.iter_mut().filter(|x| **x == old) {
                            *x = target.clone();
                        }
                        st.clip_lanes.shown.insert(id, v);
                    }
                }
                let hold = matches!(target, LaneTarget::Rack(RackParam::Power));
                let ls = &mut st.session.tracks[ti].clips[ci].props.lanes;
                // The clip's first point on this line makes its lane.
                let k = match ls.iter().position(|l| l.target == target) {
                    Some(k) => k,
                    None => {
                        ls.push(Lane::new(target.clone()));
                        ls.len() - 1
                    }
                };
                ls[k].points.push(LanePoint {
                    time: t,
                    value: if hold {
                        (value >= 0.5) as u8 as f64
                    } else {
                        value
                    },
                    curve: if hold { Curve::Hold } else { Curve::Linear },
                });
                ls[k].sort();
                st.selected_clip = Some(id);
                edited = Some(("Add Clip Automation Point", before));
            }
            r.on_hover_text(format!(
                "{}: {}. Click to add a point.",
                lane_label_in(chain, &line.target),
                lane_value_text_in(chain, &line.target, value)
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

/// Opens the first clip's lines menu once (`AF2_OPEN_CLIP_LANES=1`, for
/// screenshots).
static OPEN_FOR_SHOT: AtomicBool = AtomicBool::new(false);

/// The menu button at the right of clip `id`'s title bar (`title`), when
/// the clip is wide enough: it names the lines shown and its menu picks
/// them. `right_pad` keeps it clear of a corner handle. Returns whether it
/// was drawn.
pub fn title_button(
    ui: &mut egui::Ui,
    st: &mut State,
    id: ClipId,
    title: Rect,
    right_pad: f32,
    lanes: Rect,
) -> bool {
    let Some((ti, ci)) = st.session.find_clip(id) else {
        return false;
    };
    let c = &st.session.tracks[ti].clips[ci];
    let label = format!("{} \u{25BE}", button_label(c, &st.clip_lines_shown(id)));
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
    // Room for the clip's name beside it, and wholly in view.
    if title.width() < w + right_pad + 64.0 || !lanes.contains_rect(r) {
        return false;
    }
    let th = st.theme.clone();
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("af2-clip-lanes-button", id))
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
    let inner = egui::menu::menu_custom_button(&mut child, button, |ui| menu(ui, st, id));
    let resp = inner.response;
    let button_id = resp.id;
    // A press on the button never starts a clip drag underneath.
    let _ = ui.interact(
        r,
        egui::Id::new(("af2-clip-lanes-block", id)),
        Sense::drag(),
    );
    resp.on_hover_text("The lines shown on this clip: volume, pan, its effects' values and its effects rack's controls");
    if std::env::var_os("AF2_OPEN_CLIP_LANES").is_some()
        && !OPEN_FOR_SHOT.swap(true, Ordering::Relaxed)
    {
        let bar_id = child.id();
        let mut bs = egui::menu::BarState::load(ui.ctx(), bar_id);
        **bs = Some(egui::menu::MenuRoot::new(r.left_bottom(), button_id));
        bs.store(ui.ctx(), bar_id);
        ui.ctx().request_repaint();
    }
    true
}

/// The clip's lines menu (the title bar's menu button and Clip
/// Automation in the clip's right-click menu): which lines show, each a
/// tick; the clip's effects rack; removing a lane.
pub fn menu(ui: &mut egui::Ui, st: &mut State, id: ClipId) {
    let Some((ti, ci)) = st.session.find_clip(id) else {
        return;
    };
    let c = st.session.tracks[ti].clips[ci].clone();
    let shown = st.clip_lines_shown(id);
    let is_shown = |t: &LaneTarget| shown.iter().any(|s| resolved(&c.props.chain, s) == *t);
    let mut toggle: Option<LaneTarget> = None;
    let mut tick =
        |ui: &mut egui::Ui, target: LaneTarget, label: &str, enabled: bool, hint: &str| {
            let mut on = is_shown(&target);
            let r = ui.add_enabled(enabled, egui::Checkbox::new(&mut on, label));
            let r = if hint.is_empty() {
                r
            } else {
                r.on_hover_text(hint)
            };
            if r.clicked() {
                toggle = Some(target);
            }
        };
    if ui.button("Add/Remove Effects\u{2026}").clicked() {
        st.selected_clip = Some(id);
        st.rack_tab = crate::rack::RackTab::Clip;
        st.want_effects = true;
        ui.close_menu();
    }
    ui.separator();
    tick(ui, LaneTarget::Volume, "Volume", true, "");
    if st.session.space.is_none() {
        tick(ui, LaneTarget::Pan, "Pan", true, "");
    }
    ui.separator();
    let has_fx = !c.props.chain.is_empty();
    for p in RackParam::ALL {
        tick(
            ui,
            LaneTarget::Rack(p),
            p.label(),
            true,
            if has_fx {
                ""
            } else {
                "Plays once the clip has effects"
            },
        );
    }
    if has_fx {
        ui.separator();
    }
    for (si, s) in c.props.chain.iter().enumerate() {
        let names = slot_params(s);
        if names.is_empty() {
            continue;
        }
        ui.menu_button(format!("{} {}", si + 1, slot_title(s)), |ui| {
            for (pi, name) in names.iter().enumerate() {
                let t = effect_target(&c.props.chain, si, pi);
                tick(ui, t, name, true, "");
            }
        });
    }
    if let Some(t) = toggle {
        st.toggle_clip_line(id, t);
    }
    ui.separator();
    if ui
        .selectable_label(st.prefs.clip_lines, "Volume and Pan Lines on All Clips")
        .on_hover_text("Whether clips show their volume and pan lines unless picked otherwise")
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
            if ui
                .button(lane_label_in(&c.props.chain, &l.target))
                .clicked()
            {
                let before = st.session.clone();
                if let Some((ti, ci)) = st.session.find_clip(id) {
                    st.session.tracks[ti].clips[ci].props.lanes.remove(k);
                    st.commit_session("Remove Clip Automation", before);
                }
                ui.close_menu();
            }
        }
    });
}
