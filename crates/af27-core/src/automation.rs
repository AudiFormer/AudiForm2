// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Automation lanes: what they move, their points, how they play, and the
//! writer that records control moves into them while the session plays
//! (Read, Write, Latch, Touch).
//!
//! Units: points sit at timeline seconds. Volume lanes hold dB, pan lanes
//! -1 (left) to 1 (right), effect lanes the effect's own value. Values
//! hold flat before the first point and after the last; between points
//! they move in a straight line, or hold (step) when the earlier point's
//! curve is `Hold`. `Spline` plays as a straight line.

use serde::{Deserialize, Serialize};

/// At or below this a volume lane is silence.
pub const SILENT_DB: f64 = -59.9;
/// Touch: a control counts as held this long after its last move.
pub const TOUCH_RELEASE: f64 = 0.3;
/// Written stretches are thinned to within this share of the lane's range.
pub const THIN_TOLERANCE: f64 = 1.0 / 400.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationMode {
    /// Lanes play; controls do not write.
    #[default]
    Read,
    /// From the moment play starts, every lane of the owner records its
    /// control's value.
    Write,
    /// Writes from a control's first move and holds the last value until
    /// play stops.
    Latch,
    /// Writes while the control is held, then the lane plays again.
    Touch,
}

impl AutomationMode {
    pub const ALL: [AutomationMode; 4] = [
        AutomationMode::Read,
        AutomationMode::Write,
        AutomationMode::Latch,
        AutomationMode::Touch,
    ];
    pub fn label(self) -> &'static str {
        match self {
            AutomationMode::Read => "Read",
            AutomationMode::Write => "Write",
            AutomationMode::Latch => "Latch",
            AutomationMode::Touch => "Touch",
        }
    }
    pub fn writes(self) -> bool {
        self != AutomationMode::Read
    }
}

/// What a lane moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LaneTarget {
    Volume,
    Pan,
    /// One value of an effect in the owner's chain. `slot` is the slot's
    /// `ext_id`; `param` the value's index; `key` the name Audiform uses
    /// for it, when it has one.
    Effect {
        slot: String,
        param: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
    },
    /// A 3D Space setting (by Audiform's name), kept but not played here.
    Space(String),
}

impl LaneTarget {
    /// The value range a lane of this target covers (for drawing and for
    /// thinning).
    pub fn range(&self) -> (f64, f64) {
        match self {
            LaneTarget::Volume => (-60.0, 12.0),
            LaneTarget::Pan => (-1.0, 1.0),
            LaneTarget::Effect { .. } | LaneTarget::Space(_) => (0.0, 1.0),
        }
    }
    pub fn label(&self) -> String {
        match self {
            LaneTarget::Volume => "Volume".into(),
            LaneTarget::Pan => "Pan".into(),
            LaneTarget::Effect { key, param, .. } => {
                key.clone().unwrap_or(format!("Value {param}"))
            }
            LaneTarget::Space(p) => format!("3D {p}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Curve {
    #[default]
    Linear,
    Hold,
    Spline,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LanePoint {
    /// Timeline seconds.
    pub time: f64,
    pub value: f64,
    #[serde(default, skip_serializing_if = "is_linear")]
    pub curve: Curve,
}

fn is_linear(c: &Curve) -> bool {
    *c == Curve::Linear
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

/// An automation lane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lane {
    pub target: LaneTarget,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// Sorted by time.
    #[serde(default)]
    pub points: Vec<LanePoint>,
    /// Height on screen; None is the standard height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext_id: Option<String>,
}

impl Lane {
    pub fn new(target: LaneTarget) -> Lane {
        Lane {
            target,
            enabled: true,
            points: Vec::new(),
            height: None,
            ext_id: None,
        }
    }

    /// Plays: on and with at least one point.
    pub fn plays(&self) -> bool {
        self.enabled && !self.points.is_empty()
    }

    /// The value at timeline time `t` (None with no points).
    pub fn value_at(&self, t: f64) -> Option<f64> {
        value_at(&self.points, t)
    }

    /// Puts the points in time order.
    pub fn sort(&mut self) {
        self.points.sort_by(|a, b| a.time.total_cmp(&b.time));
    }

    /// Replaces every point from `a` to `b` (inclusive) with `with`.
    pub fn replace_span(&mut self, a: f64, b: f64, with: &[LanePoint]) {
        let (a, b) = (a.min(b), a.max(b));
        self.points.retain(|p| p.time < a || p.time > b);
        self.points.extend_from_slice(with);
        self.sort();
    }
}

/// The value of sorted `points` at `t`.
pub fn value_at(points: &[LanePoint], t: f64) -> Option<f64> {
    let first = points.first()?;
    if t <= first.time {
        return Some(first.value);
    }
    let last = points.last()?;
    if t >= last.time {
        return Some(last.value);
    }
    // The first point after t.
    let k = points.partition_point(|p| p.time <= t);
    let (a, b) = (points[k - 1], points[k]);
    if a.curve == Curve::Hold || b.time <= a.time {
        return Some(a.value);
    }
    let x = (t - a.time) / (b.time - a.time);
    Some(a.value + (b.value - a.value) * x)
}

/// The part of sorted `points` from `a` to `b`, moved to start at 0, with
/// a point at each end where the line crosses it (none for an empty
/// lane). A clip's lanes cut to one part of the clip.
pub fn crop(points: &[LanePoint], a: f64, b: f64) -> Vec<LanePoint> {
    if points.is_empty() {
        return Vec::new();
    }
    let eps = 1e-9;
    let inside: Vec<LanePoint> = points
        .iter()
        .filter(|p| p.time >= a - eps && p.time <= b + eps)
        .map(|p| LanePoint {
            time: (p.time - a).clamp(0.0, (b - a).max(0.0)),
            ..*p
        })
        .collect();
    let edge = |t: f64| {
        // The curve of the point before the edge carries on.
        let k = points.partition_point(|p| p.time <= t);
        let curve = k.checked_sub(1).map_or(Curve::Linear, |i| points[i].curve);
        LanePoint {
            time: t - a,
            value: value_at(points, t).unwrap_or(0.0),
            curve,
        }
    };
    let mut out = Vec::with_capacity(inside.len() + 2);
    if inside.first().is_none_or(|p| p.time > eps) {
        out.push(edge(a));
    }
    out.extend(inside);
    if out.last().is_none_or(|p| p.time < b - a - eps) {
        out.push(edge(b));
    }
    out
}

/// Thins a run of points by furthest-point splitting: keeps the ends and
/// every point further than `tol` from the line between kept neighbours.
pub fn thin(points: &[LanePoint], tol: f64) -> Vec<LanePoint> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0usize, points.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (points[a], points[b]);
        let mut far = (0.0f64, a);
        for (i, p) in points.iter().enumerate().take(b).skip(a + 1) {
            let line = if pb.time > pa.time {
                pa.value + (pb.value - pa.value) * (p.time - pa.time) / (pb.time - pa.time)
            } else {
                pa.value
            };
            let d = (p.value - line).abs();
            if d > far.0 {
                far = (d, i);
            }
        }
        if far.0 > tol {
            keep[far.1] = true;
            stack.push((a, far.1));
            stack.push((far.1, b));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

/// Who owns a lane: a track (or bus) by id, or the master.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Owner {
    Track(crate::session::TrackId),
    Master,
}

/// One lane being written in the current pass.
#[derive(Debug, Clone)]
struct Pass {
    owner: Owner,
    target: LaneTarget,
    mode: AutomationMode,
    /// The control's value now.
    value: f64,
    /// Wall-clock seconds of the last move (Touch).
    moved_at: f64,
    /// Writing now (Touch lets go after `TOUCH_RELEASE`).
    writing: bool,
    /// Where the current written stretch began.
    from: f64,
    /// Points written in the current stretch, in time order.
    written: Vec<LanePoint>,
}

/// A finished stretch: replace the lane's points from `from` to `to`
/// with `points`.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    pub owner: Owner,
    pub target: LaneTarget,
    pub from: f64,
    pub to: f64,
    pub points: Vec<LanePoint>,
}

/// Records control moves into lanes while the session plays.
///
/// The app tells it when play starts ([`AutoWriter::start`], with the
/// owners in Write mode and their current control values), every move of
/// a control on an owner in a writing mode ([`AutoWriter::moved`]), and
/// the playhead each frame ([`AutoWriter::tick`]). [`AutoWriter::live`]
/// says which lanes are being written: those play from the live control,
/// not from their points. Finished stretches come back from `tick` (a
/// Touch let go) and from [`AutoWriter::stop`], thinned, to be applied
/// with [`apply`] as one undo step for the whole pass.
#[derive(Debug, Default, Clone)]
pub struct AutoWriter {
    passes: Vec<Pass>,
    last_t: f64,
}

impl AutoWriter {
    pub fn is_active(&self) -> bool {
        !self.passes.is_empty()
    }

    /// Play started at timeline `t` (wall clock `now`). `write` lists,
    /// for owners in Write mode, every lane target they write and the
    /// control's current value.
    pub fn start(&mut self, t: f64, now: f64, write: &[(Owner, LaneTarget, f64)]) {
        self.passes.clear();
        self.last_t = t;
        for (owner, target, value) in write {
            self.passes.push(Pass {
                owner: *owner,
                target: target.clone(),
                mode: AutomationMode::Write,
                value: *value,
                moved_at: now,
                writing: true,
                from: t,
                written: vec![LanePoint {
                    time: t,
                    value: *value,
                    curve: Curve::Linear,
                }],
            });
        }
    }

    /// A control moved to `value` at timeline `t` (wall clock `now`) on an
    /// owner in `mode`. Read mode is ignored.
    pub fn moved(
        &mut self,
        owner: Owner,
        target: LaneTarget,
        mode: AutomationMode,
        value: f64,
        t: f64,
        now: f64,
    ) {
        if !mode.writes() {
            return;
        }
        let point = LanePoint {
            time: t,
            value,
            curve: Curve::Linear,
        };
        if let Some(p) = self
            .passes
            .iter_mut()
            .find(|p| p.owner == owner && p.target == target)
        {
            p.value = value;
            p.moved_at = now;
            if !p.writing {
                p.writing = true;
                p.from = t;
                p.written.clear();
            }
            push_point(&mut p.written, point);
            return;
        }
        self.passes.push(Pass {
            owner,
            target,
            mode,
            value,
            moved_at: now,
            writing: true,
            from: t,
            written: vec![point],
        });
    }

    /// The playhead is at `t` (wall clock `now`): held values are carried
    /// to it, and Touch lanes let go after `TOUCH_RELEASE` without a move.
    /// Returns the stretches that finished.
    pub fn tick(&mut self, t: f64, now: f64) -> Vec<Written> {
        self.last_t = t;
        let mut done = Vec::new();
        for p in &mut self.passes {
            if !p.writing {
                continue;
            }
            push_point(
                &mut p.written,
                LanePoint {
                    time: t,
                    value: p.value,
                    curve: Curve::Linear,
                },
            );
            if p.mode == AutomationMode::Touch && now - p.moved_at > TOUCH_RELEASE {
                p.writing = false;
                done.push(finish(p, t));
            }
        }
        done
    }

    /// Play stopped at `t`: every stretch still being written ends.
    pub fn stop(&mut self, t: f64) -> Vec<Written> {
        let t = t.max(self.last_t);
        let out = self
            .passes
            .iter_mut()
            .filter(|p| p.writing)
            .map(|p| {
                push_point(
                    &mut p.written,
                    LanePoint {
                        time: t,
                        value: p.value,
                        curve: Curve::Linear,
                    },
                );
                finish(p, t)
            })
            .collect();
        self.passes.clear();
        out
    }

    /// The lanes being written now: they play from their live control.
    pub fn live(&self) -> Vec<(Owner, LaneTarget)> {
        self.passes
            .iter()
            .filter(|p| p.writing)
            .map(|p| (p.owner, p.target.clone()))
            .collect()
    }

    /// What has been written so far, for drawing (owner, target, points).
    pub fn drafts(&self) -> impl Iterator<Item = (Owner, &LaneTarget, &[LanePoint])> {
        self.passes
            .iter()
            .filter(|p| p.writing)
            .map(|p| (p.owner, &p.target, p.written.as_slice()))
    }
}

/// Adds a point, dropping the middle of three equal values in a row so a
/// held value adds no point per step.
fn push_point(v: &mut Vec<LanePoint>, p: LanePoint) {
    if let Some(last) = v.last() {
        if p.time < last.time {
            return;
        }
    }
    let n = v.len();
    if n >= 2 && v[n - 1].value == p.value && v[n - 2].value == p.value {
        v[n - 1] = p;
    } else {
        v.push(p);
    }
}

fn finish(p: &mut Pass, t: f64) -> Written {
    let (lo, hi) = p.target.range();
    let points = thin(&p.written, (hi - lo) * THIN_TOLERANCE);
    p.written.clear();
    Written {
        owner: p.owner,
        target: p.target.clone(),
        from: p.from,
        to: t,
        points,
    }
}

/// Applies written stretches to the session's lanes, making a lane for a
/// control that has none.
pub fn apply(session: &mut crate::session::Session, written: &[Written]) {
    for w in written {
        let lanes = match w.owner {
            Owner::Master => &mut session.master_lanes,
            Owner::Track(id) => match session.track_mut(id) {
                Some(t) => &mut t.lanes,
                None => continue,
            },
        };
        let lane = match lanes.iter().position(|l| l.target == w.target) {
            Some(i) => &mut lanes[i],
            None => {
                lanes.push(Lane::new(w.target.clone()));
                lanes.last_mut().unwrap()
            }
        };
        lane.replace_span(w.from, w.to, &w.points);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(time: f64, value: f64) -> LanePoint {
        LanePoint {
            time,
            value,
            curve: Curve::Linear,
        }
    }

    #[test]
    fn crop_cuts_with_edge_points() {
        let pts = vec![pt(1.0, 0.0), pt(3.0, 10.0), pt(5.0, 10.0)];
        let c = crop(&pts, 2.0, 4.0);
        assert_eq!(
            c.iter().map(|p| (p.time, p.value)).collect::<Vec<_>>(),
            vec![(0.0, 5.0), (1.0, 10.0), (2.0, 10.0)]
        );
        // A point on the edge is not doubled; before the first point the
        // first value holds.
        let c = crop(&pts, 0.0, 1.0);
        assert_eq!(
            c.iter().map(|p| (p.time, p.value)).collect::<Vec<_>>(),
            vec![(0.0, 0.0), (1.0, 0.0)]
        );
        assert!(crop(&[], 0.0, 1.0).is_empty());
    }

    #[test]
    fn values_hold_outside_and_move_straight_between() {
        let mut pts = vec![pt(1.0, 0.0), pt(3.0, 10.0)];
        assert_eq!(value_at(&pts, 0.0), Some(0.0));
        assert_eq!(value_at(&pts, 2.0), Some(5.0));
        assert_eq!(value_at(&pts, 9.0), Some(10.0));
        pts[0].curve = Curve::Hold;
        assert_eq!(value_at(&pts, 2.9), Some(0.0));
        assert_eq!(value_at(&[], 1.0), None);
    }

    #[test]
    fn thinning_keeps_corners_and_drops_straight_runs() {
        let pts: Vec<LanePoint> = (0..=10)
            .map(|i| pt(i as f64, if i <= 5 { i as f64 } else { 5.0 }))
            .collect();
        let t = thin(&pts, 0.01);
        assert_eq!(t, vec![pt(0.0, 0.0), pt(5.0, 5.0), pt(10.0, 5.0)]);
    }

    const T: Owner = Owner::Track(7);

    #[test]
    fn touch_writes_only_while_held() {
        let mut w = AutoWriter::default();
        w.start(0.0, 0.0, &[]);
        w.moved(
            T,
            LaneTarget::Volume,
            AutomationMode::Touch,
            -6.0,
            1.0,
            10.0,
        );
        assert_eq!(w.live(), vec![(T, LaneTarget::Volume)]);
        assert!(w.tick(1.1, 10.1).is_empty());
        // Let go: after the release time without a move the stretch ends.
        let done = w.tick(1.5, 10.5);
        assert_eq!(done.len(), 1);
        assert_eq!((done[0].from, done[0].to), (1.0, 1.5));
        assert!(w.live().is_empty());
        // Nothing more is written until the next move.
        assert!(w.tick(3.0, 12.0).is_empty());
        assert!(w.stop(4.0).is_empty());
    }

    #[test]
    fn latch_holds_until_stop() {
        let mut w = AutoWriter::default();
        w.start(0.0, 0.0, &[]);
        w.moved(T, LaneTarget::Pan, AutomationMode::Latch, 0.5, 2.0, 1.0);
        assert!(w.tick(5.0, 100.0).is_empty());
        let done = w.stop(6.0);
        assert_eq!(done.len(), 1);
        assert_eq!((done[0].from, done[0].to), (2.0, 6.0));
        let pts = &done[0].points;
        assert_eq!(pts.first().unwrap().value, 0.5);
        assert_eq!(pts.last().unwrap().time, 6.0);
    }

    #[test]
    fn write_covers_every_lane_from_play_start() {
        let mut w = AutoWriter::default();
        w.start(
            1.0,
            0.0,
            &[(T, LaneTarget::Volume, -3.0), (T, LaneTarget::Pan, 0.0)],
        );
        assert_eq!(w.live().len(), 2);
        w.tick(2.0, 1.0);
        w.moved(
            T,
            LaneTarget::Volume,
            AutomationMode::Write,
            -10.0,
            2.5,
            1.5,
        );
        let done = w.stop(4.0);
        assert_eq!(done.len(), 2);
        let vol = done
            .iter()
            .find(|d| d.target == LaneTarget::Volume)
            .unwrap();
        assert_eq!(vol.from, 1.0);
        assert_eq!(vol.points.first().unwrap().value, -3.0);
        assert_eq!(vol.points.last().unwrap().value, -10.0);
        // Applied as lanes; a lane is made for a control that had none.
        let mut s = crate::session::Session::new("t", 48_000, 0);
        let id = s.add_track();
        let done: Vec<Written> = done
            .into_iter()
            .map(|mut d| {
                d.owner = Owner::Track(id);
                d
            })
            .collect();
        apply(&mut s, &done);
        let t = s.track_mut(id).unwrap();
        assert_eq!(t.lanes.len(), 2);
        assert_eq!(t.lanes[0].value_at(3.0), Some(-10.0));
    }

    #[test]
    fn held_values_add_no_points() {
        let mut w = AutoWriter::default();
        w.start(0.0, 0.0, &[(T, LaneTarget::Volume, -3.0)]);
        for i in 1..100 {
            w.tick(i as f64 * 0.05, i as f64 * 0.05);
        }
        let done = w.stop(5.0);
        assert_eq!(done[0].points.len(), 2);
    }
}
