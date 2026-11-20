// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Spectral selection and editing in the Waveform editor's spectral
//! display: the Box, Lasso and Brush selection tools, Spot Repair, the
//! actions on a selection (Reduce 12 dB / 24 dB, Remove, Repair), the
//! floating bar over a selection, the divider between waveform and
//! spectrum, and the spectrogram's own colours.
//!
//! A selection is a set of shapes in time x frequency. Frequencies are
//! kept on a fixed internal logarithmic axis ([`hz_to_u`]), not on the
//! screen, so a selection stays on the same sounds while the display
//! zooms or changes scale. Each channel has its own band in the display;
//! a selection drawn in one band shows in every band, and an edit reaches
//! every channel (Audiform 2 has no per-channel enable yet), one channel
//! at a time, in a background job.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use af27_core::AudioBuffer;
use af27_dsp::spectral_edit::{edit_channel, Action, Grid};
use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke};

use crate::state::{DocId, State, Tool, View};
use crate::theme::ca;
use crate::zoom::SpecView;

// ------------------------------------------------------------- model

/// The internal frequency axis: octaves above 50 Hz, roughly (log2 of
/// 1 + f / 50), so it is logarithmic over the audible range and still
/// holds 0 Hz.
pub fn hz_to_u(f: f64) -> f64 {
    (1.0 + f.max(0.0) / 50.0).log2()
}

pub fn u_to_hz(u: f64) -> f64 {
    (u.exp2() - 1.0) * 50.0
}

/// A point of a selection: seconds, and frequency on the internal axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SPoint {
    pub t: f64,
    pub u: f64,
}

/// One shape of a spectral selection.
#[derive(Debug, Clone, PartialEq)]
pub enum Region {
    /// Two opposite corners.
    Box { a: SPoint, b: SPoint },
    /// A closed outline (even-odd fill).
    Lasso(Vec<SPoint>),
    /// A stroke of round dabs, radius `rt` seconds by `ru` axis units.
    Brush { pts: Vec<SPoint>, rt: f64, ru: f64 },
}

impl Region {
    /// (t0, t1, u0, u1) around the shape.
    pub fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        let (pts, rt, ru): (Vec<SPoint>, f64, f64) = match self {
            Region::Box { a, b } => (vec![*a, *b], 0.0, 0.0),
            Region::Lasso(p) => (p.clone(), 0.0, 0.0),
            Region::Brush { pts, rt, ru } => (pts.clone(), *rt, *ru),
        };
        if pts.is_empty() {
            return None;
        }
        let mut b = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for p in pts {
            b = (b.0.min(p.t), b.1.max(p.t), b.2.min(p.u), b.3.max(p.u));
        }
        Some((b.0 - rt, b.1 + rt, b.2 - ru, b.3 + ru))
    }

    /// Whether the shape covers anything at all.
    fn usable(&self) -> bool {
        match self {
            Region::Box { a, b } => (a.t - b.t).abs() > 1e-9 && (a.u - b.u).abs() > 1e-9,
            Region::Lasso(p) => p.len() >= 3,
            Region::Brush { pts, rt, ru } => !pts.is_empty() && *rt > 0.0 && *ru > 0.0,
        }
    }
}

static NEXT_VERSION: AtomicU64 = AtomicU64::new(1);

/// A document's spectral selection.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpecSel {
    pub regions: Vec<Region>,
    /// Changes whenever the selection does (unique across documents).
    pub version: u64,
}

impl SpecSel {
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    fn bump(&mut self) {
        self.version = NEXT_VERSION.fetch_add(1, Ordering::Relaxed);
    }

    pub fn clear(&mut self) {
        if !self.regions.is_empty() {
            self.regions.clear();
            self.bump();
        }
    }

    pub fn push(&mut self, r: Region) {
        self.regions.push(r);
        self.bump();
    }

    fn last_mut(&mut self) -> Option<&mut Region> {
        self.bump();
        self.regions.last_mut()
    }

    /// (t0, t1, u0, u1) around every shape.
    pub fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        self.regions
            .iter()
            .filter_map(Region::bounds)
            .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1), a.2.min(b.2), a.3.max(b.3)))
    }
}

// --------------------------------------------------------- rasterising

/// Where the horizontal line at `u` crosses `r`: intervals in seconds.
fn row_intervals(r: &Region, u: f64, out: &mut Vec<(f64, f64)>) {
    match r {
        Region::Box { a, b } => {
            if u >= a.u.min(b.u) && u <= a.u.max(b.u) {
                out.push((a.t.min(b.t), a.t.max(b.t)));
            }
        }
        Region::Lasso(pts) => {
            let n = pts.len();
            if n < 3 {
                return;
            }
            let mut xs: Vec<f64> = Vec::new();
            for i in 0..n {
                let (p, q) = (pts[i], pts[(i + 1) % n]);
                if (p.u > u) != (q.u > u) {
                    xs.push(p.t + (u - p.u) * (q.t - p.t) / (q.u - p.u));
                }
            }
            xs.sort_by(f64::total_cmp);
            let (pairs, _) = xs.as_chunks::<2>();
            for w in pairs {
                out.push((w[0], w[1]));
            }
        }
        Region::Brush { pts, rt, ru } => {
            if *rt <= 0.0 || *ru <= 0.0 {
                return;
            }
            // In units of the radius every dab is a unit disc and every
            // step between dabs a capsule.
            let y = u / ru;
            let pt = |p: &SPoint| (p.t / rt, p.u / ru);
            let disc = |(cx, cy): (f64, f64)| -> Option<(f64, f64)> {
                let dy = y - cy;
                (dy.abs() <= 1.0).then(|| {
                    let h = (1.0 - dy * dy).sqrt();
                    (cx - h, cx + h)
                })
            };
            for (i, p) in pts.iter().enumerate() {
                let a = pt(p);
                let mut span = disc(a);
                if i > 0 {
                    let b = pt(&pts[i - 1]);
                    let merge = |s: Option<(f64, f64)>, o: Option<(f64, f64)>| match (s, o) {
                        (Some(s), Some(o)) => Some((s.0.min(o.0), s.1.max(o.1))),
                        (s, None) => s,
                        (None, o) => o,
                    };
                    span = merge(span, disc(b));
                    span = merge(span, strip(a, b, y));
                }
                if let Some((x0, x1)) = span {
                    out.push((x0 * rt, x1 * rt));
                }
            }
        }
    }
}

/// The X range on the line Y = `y` within distance 1 of segment `a`-`b`
/// whose foot falls inside the segment.
fn strip(a: (f64, f64), b: (f64, f64), y: f64) -> Option<(f64, f64)> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx.hypot(dy);
    if len < 1e-12 {
        return None;
    }
    let (ex, ey) = (dx / len, dy / len);
    let (nx, ny) = (-ey, ex);
    // lo <= k * X + c <= hi, solved for X.
    let solve = |k: f64, c: f64, lo: f64, hi: f64| -> Option<(f64, f64)> {
        if k.abs() < 1e-12 {
            (c >= lo && c <= hi).then_some((f64::NEG_INFINITY, f64::INFINITY))
        } else {
            let (p, q) = ((lo - c) / k, (hi - c) / k);
            Some((p.min(q), p.max(q)))
        }
    };
    let across = solve(nx, -nx * a.0 + ny * (y - a.1), -1.0, 1.0)?;
    let along = solve(ex, -ex * a.0 + ey * (y - a.1), 0.0, len)?;
    let (lo, hi) = (across.0.max(along.0), across.1.min(along.1));
    (lo <= hi && lo.is_finite() && hi.is_finite()).then_some((lo, hi))
}

/// Rasterises `regions` onto a grid of `us.len()` rows (frequency on the
/// internal axis) by `times.len()` columns (seconds, ascending): 1 inside,
/// 0 outside, row by row. A shape narrower than a column still marks the
/// nearest column.
pub fn rasterise(regions: &[Region], times: &[f64], us: &[f64]) -> Vec<f32> {
    let cols = times.len();
    let mut out = vec![0.0f32; cols * us.len()];
    if cols == 0 {
        return out;
    }
    let half = if cols > 1 {
        (times[1] - times[0]).abs() / 2.0
    } else {
        f64::INFINITY
    };
    let mut iv = Vec::new();
    for (row, &u) in us.iter().enumerate() {
        iv.clear();
        for r in regions {
            row_intervals(r, u, &mut iv);
        }
        let line = &mut out[row * cols..(row + 1) * cols];
        for &(t0, t1) in &iv {
            let i0 = times.partition_point(|&x| x < t0);
            let i1 = times.partition_point(|&x| x <= t1);
            if i0 < i1 {
                line[i0..i1].iter_mut().for_each(|v| *v = 1.0);
            } else if t1 >= times[0] - half && t0 <= times[cols - 1] + half {
                let mid = (t0 + t1) / 2.0;
                let i = times.partition_point(|&x| x < mid).min(cols - 1);
                let j = if i > 0 && (times[i - 1] - mid).abs() < (times[i] - mid).abs() {
                    i - 1
                } else {
                    i
                };
                line[j] = 1.0;
            }
        }
    }
    out
}

/// The weight grid for an edit of a file `len` samples long at `rate`,
/// with `fft`-sample frames at 75 % overlap. Frames are taken as inside
/// when their centre is, widened by one hop in time so the frames whose
/// windows reach well into the area take part, then feathered outward
/// (one frame in time, two bins in frequency) for soft edges. The view
/// plays no part, so the grid is the same at any zoom or scale.
pub fn edit_grid(regions: &[Region], rate: f64, len: usize, fft: usize) -> Option<Grid> {
    let usable: Vec<Region> = regions.iter().filter(|r| r.usable()).cloned().collect();
    if usable.is_empty() || len == 0 {
        return None;
    }
    let (t0, t1, _, _) = usable
        .iter()
        .filter_map(Region::bounds)
        .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1), a.2.min(b.2), a.3.max(b.3)))?;
    let probe = Grid::new(fft, 0, 0);
    let (n, hop) = (probe.fft, probe.hop);
    let last = ((len - 1) / hop) as i64;
    let k_lo = (((t0 * rate) / hop as f64).floor() as i64 - 3).clamp(0, last);
    let k_hi = (((t1 * rate) / hop as f64).ceil() as i64 + 3).clamp(0, last);
    if k_hi < k_lo {
        return None;
    }
    let frames = (k_hi - k_lo + 1) as usize;
    let bins = n / 2 + 1;
    let times: Vec<f64> = (0..frames)
        .map(|i| ((k_lo + i as i64) * hop as i64) as f64 / rate)
        .collect();
    let us: Vec<f64> = (0..bins)
        .map(|b| hz_to_u(b as f64 * rate / n as f64))
        .collect();
    // rows = bins, columns = frames.
    let hard = rasterise(&usable, &times, &us);
    let at = |f: i64, b: i64| -> f32 {
        if f < 0 || b < 0 || f >= frames as i64 || b >= bins as i64 {
            0.0
        } else {
            hard[b as usize * frames + f as usize]
        }
    };
    // Widen by one frame each way.
    let wide: Vec<f32> = (0..bins as i64)
        .flat_map(|b| (0..frames as i64).map(move |f| (b, f)))
        .map(|(b, f)| at(f - 1, b).max(at(f, b)).max(at(f + 1, b)))
        .collect();
    let wat = |f: i64, b: i64| -> f32 {
        if f < 0 || b < 0 || f >= frames as i64 || b >= bins as i64 {
            0.0
        } else {
            wide[b as usize * frames + f as usize]
        }
    };
    const FT: [f32; 3] = [0.5, 1.0, 0.5];
    const FF: [f32; 5] = [1.0 / 3.0, 2.0 / 3.0, 1.0, 2.0 / 3.0, 1.0 / 3.0];
    let mut g = Grid::new(n, k_lo, frames);
    for f in 0..frames as i64 {
        for b in 0..bins as i64 {
            let mut w = 0.0f32;
            for (i, ft) in FT.iter().enumerate() {
                for (j, ff) in FF.iter().enumerate() {
                    let v = wat(f + i as i64 - 1, b + j as i64 - 2) * ft * ff;
                    w = w.max(v);
                }
            }
            g.w[f as usize * bins + b as usize] = w;
        }
    }
    Some(g)
}

// -------------------------------------------------------- screen math

/// The spectral display's bands (one per channel, top to bottom) and the
/// mapping between the screen and selection points.
#[derive(Debug, Clone, Copy)]
pub struct Mapper {
    pub area: Rect,
    pub bands: usize,
    pub view: View,
    pub spec: SpecView,
    pub ny: f64,
    pub dur: f64,
}

impl Mapper {
    pub fn band_h(&self) -> f32 {
        self.area.height() / self.bands.max(1) as f32
    }

    pub fn band_rect(&self, k: usize) -> Rect {
        let h = self.band_h();
        Rect::from_min_size(
            pos2(self.area.left(), self.area.top() + h * k as f32),
            vec2(self.area.width(), h),
        )
    }

    pub fn band_of(&self, y: f32) -> usize {
        (((y - self.area.top()) / self.band_h()).floor().max(0.0) as usize)
            .min(self.bands.max(1) - 1)
    }

    /// The selection point under `pos`, kept inside band `k`: past the
    /// band's top or bottom it sits on that edge.
    pub fn point(&self, k: usize, pos: Pos2) -> SPoint {
        let r = self.band_rect(k);
        let y = pos.y.clamp(r.top(), r.bottom());
        let frac = ((r.bottom() - y) / r.height().max(1.0)) as f64;
        let x = pos.x.clamp(self.area.left(), self.area.right());
        let t = self.view.start
            + ((x - self.area.left()) / self.area.width().max(1.0)) as f64 * self.view.len;
        SPoint {
            t: t.clamp(0.0, self.dur),
            u: hz_to_u(self.spec.freq_at(frac, self.ny)),
        }
    }

    pub fn x(&self, t: f64) -> f32 {
        self.area.left() + ((t - self.view.start) / self.view.len) as f32 * self.area.width()
    }

    pub fn y(&self, k: usize, u: f64) -> f32 {
        let r = self.band_rect(k);
        r.bottom() - self.spec.pos(u_to_hz(u), self.ny) as f32 * r.height()
    }

    pub fn screen(&self, k: usize, p: SPoint) -> Pos2 {
        pos2(self.x(p.t), self.y(k, p.u))
    }

    /// The brush's radius in selection units for a `px`-pixel radius at `pos`.
    fn brush_radius(&self, k: usize, pos: Pos2, px: f32) -> (f64, f64) {
        let rt = px as f64 / self.area.width().max(1.0) as f64 * self.view.len;
        let a = self.point(k, pos - vec2(0.0, px));
        let b = self.point(k, pos + vec2(0.0, px));
        (rt.max(1e-6), ((a.u - b.u).abs() / 2.0).max(1e-4))
    }
}

fn mapper(st: &State, area: Rect) -> Option<Mapper> {
    let d = st.doc()?;
    Some(Mapper {
        area,
        bands: d.buf.num_channels().clamp(1, crate::waveform::SPEC_MAX_CH),
        view: d.view,
        spec: st.spec,
        ny: d.buf.rate as f64 / 2.0,
        dur: d.duration(),
    })
}

// ------------------------------------------------------ interaction

#[derive(Debug, Clone, Copy)]
struct Drag {
    band: usize,
    tool: Tool,
    /// Last point added to a lasso or stroke, on screen.
    last: Pos2,
}

fn drag_id() -> egui::Id {
    egui::Id::new("af27-spec-drag")
}

/// The width of the spectrum's frequency scale (as the Waveform editor
/// draws it).
const SCALE_W: f32 = 44.0;

/// The spectral tools' pointer handling over the spectrum `sa`. Only
/// while a spectral tool is chosen; the response it returns covers the
/// spectrum, so the editor's time selection does not see those drags.
pub fn overlay(ui: &mut egui::Ui, st: &mut State, sa: Rect) -> Option<egui::Response> {
    if !st.tool.is_spectral() || st.doc().is_none() {
        return None;
    }
    let resp = ui.interact(sa, egui::Id::new("af27-spec-sel"), Sense::click_and_drag());
    let m = mapper(st, sa)?;
    let shift = ui.input(|i| i.modifiers.shift);
    let tool = st.tool;
    let brush_r = st.brush_px / 2.0;
    if resp.drag_started() {
        if let Some(pp) = resp.interact_pointer_pos() {
            let k = m.band_of(pp.y);
            let p = m.point(k, pp);
            let region = match tool {
                Tool::BoxSelect => Region::Box { a: p, b: p },
                Tool::LassoSelect => Region::Lasso(vec![p]),
                _ => {
                    let (rt, ru) = m.brush_radius(k, pp, brush_r);
                    Region::Brush {
                        pts: vec![p],
                        rt,
                        ru,
                    }
                }
            };
            let d = st.doc_mut().unwrap();
            if tool == Tool::SpotRepair || !shift {
                d.spec_sel.clear();
            }
            d.spec_sel.push(region);
            d.sel = None;
            ui.data_mut(|x| {
                x.insert_temp(
                    drag_id(),
                    Drag {
                        band: k,
                        tool,
                        last: pp,
                    },
                )
            });
        }
    }
    let drag: Option<Drag> = ui.data(|x| x.get_temp(drag_id()));
    if let (Some(mut g), Some(pp)) = (drag, resp.interact_pointer_pos()) {
        if resp.dragged() {
            let p = m.point(g.band, pp);
            let spacing = match g.tool {
                Tool::LassoSelect => 3.0,
                _ => (brush_r / 3.0).max(1.5),
            };
            let far = (pp - g.last).length() >= spacing;
            let d = st.doc_mut().unwrap();
            match (g.tool, d.spec_sel.regions.last()) {
                (Tool::BoxSelect, Some(Region::Box { .. })) => {
                    if let Some(Region::Box { b, .. }) = d.spec_sel.last_mut() {
                        *b = p;
                    }
                }
                (_, Some(Region::Lasso(_))) | (_, Some(Region::Brush { .. })) if far => {
                    match d.spec_sel.last_mut() {
                        Some(Region::Lasso(v)) => v.push(p),
                        Some(Region::Brush { pts, .. }) => pts.push(p),
                        _ => {}
                    }
                    g.last = pp;
                    ui.data_mut(|x| x.insert_temp(drag_id(), g));
                }
                _ => {}
            }
        }
    }
    if resp.drag_stopped() {
        if let Some(g) = drag {
            ui.data_mut(|x| x.remove::<Drag>(drag_id()));
            let d = st.doc_mut().unwrap();
            if d.spec_sel.regions.last().is_some_and(|r| !r.usable()) {
                d.spec_sel.regions.pop();
                d.spec_sel.bump();
            }
            if g.tool == Tool::SpotRepair && !d.spec_sel.is_empty() {
                run(st, Action::Repair, "Spot Repair", true);
            }
        }
    }
    if resp.clicked() && !shift {
        if let Some(pp) = resp.interact_pointer_pos() {
            let t = m.point(0, pp).t;
            let d = st.doc_mut().unwrap();
            d.spec_sel.clear();
            d.sel = None;
            st.set_cursor(t);
            if st.is_playing() {
                st.play();
            }
        }
    }
    if let Some(hp) = resp.hover_pos() {
        if hp.x < sa.right() - SCALE_W {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }
    Some(resp)
}

/// The draggable divider between the waveform and the spectrum, at `y`
/// across `body`. Sets [`State::spec_split`].
pub fn divider(ui: &mut egui::Ui, st: &mut State, body: Rect, y: f32) {
    let r = Rect::from_min_max(pos2(body.left(), y - 3.0), pos2(body.right(), y + 3.0));
    let resp = ui.interact(r, egui::Id::new("af27-spec-divider"), Sense::drag());
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    if resp.dragged() {
        if let Some(pp) = resp.interact_pointer_pos() {
            st.spec_split = ((pp.y - body.top()) / body.height().max(1.0)).clamp(0.1, 0.9);
        }
    }
    if resp.double_clicked() {
        st.spec_split = 0.5;
    }
    let col = if resp.hovered() || resp.dragged() {
        ca(st.theme.accent, 200)
    } else {
        ca(st.theme.border, 220)
    };
    let p = ui.painter_at(r);
    p.line_segment(
        [pos2(body.left(), y), pos2(body.right(), y)],
        Stroke::new(2.0_f32, col),
    );
    let mid = body.center().x;
    p.line_segment(
        [pos2(mid - 14.0, y), pos2(mid + 14.0, y)],
        Stroke::new(3.0_f32, col),
    );
}

// ------------------------------------------------------------ drawing

/// Draws the selection in every band of the spectrum `sa`, the channel
/// names on the scale, and the brush outline under the pointer.
pub fn draw(ui: &egui::Ui, st: &State, sa: Rect, p: &egui::Painter) {
    let Some(m) = mapper(st, sa) else { return };
    let d = st.doc().unwrap();
    let th = &st.theme;
    let nch = d.buf.num_channels();
    // Band names on the frequency scale.
    if m.bands > 1 {
        for k in 0..m.bands {
            let r = m.band_rect(k);
            p.text(
                pos2(r.right() - 4.0, r.top() + 2.0),
                Align2::RIGHT_TOP,
                crate::waveform::channel_name(nch, k),
                FontId::proportional(10.0),
                ca(th.accent, 230),
            );
        }
    }
    if !d.spec_sel.is_empty() {
        let tex = selection_texture(ui, st, &m);
        for k in 0..m.bands {
            let r = m.band_rect(k);
            let pc = p.with_clip_rect(r.intersect(p.clip_rect()));
            if let Some(tex) = &tex {
                pc.image(
                    tex.id(),
                    r,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            let s = Stroke::new(1.0_f32, ca(th.selection, 230));
            for reg in &d.spec_sel.regions {
                match reg {
                    Region::Box { a, b } => {
                        let (x0, x1) = (m.x(a.t.min(b.t)), m.x(a.t.max(b.t)));
                        let (y0, y1) = (m.y(k, a.u.max(b.u)), m.y(k, a.u.min(b.u)));
                        pc.rect_stroke(Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)), 0.0, s);
                    }
                    Region::Lasso(pts) if pts.len() > 1 => {
                        let v: Vec<Pos2> = pts.iter().map(|q| m.screen(k, *q)).collect();
                        pc.add(egui::Shape::closed_line(v, s));
                    }
                    _ => {}
                }
            }
        }
    }
    if matches!(st.tool, Tool::BrushSelect | Tool::SpotRepair) {
        if let Some(hp) = ui.ctx().pointer_hover_pos() {
            if sa.contains(hp) && hp.x < sa.right() - SCALE_W {
                p.circle_stroke(
                    hp,
                    st.brush_px / 2.0,
                    Stroke::new(1.0_f32, ca(th.selection, 220)),
                );
            }
        }
    }
}

/// The selection, rasterised at half the screen's resolution for one band.
fn selection_texture(ui: &egui::Ui, st: &State, m: &Mapper) -> Option<egui::TextureHandle> {
    let d = st.doc()?;
    let cols = ((m.area.width() / 2.0) as usize).clamp(8, 2048);
    let rows = ((m.band_h() / 2.0) as usize).clamp(8, 1024);
    let key = format!(
        "{}|{}|{}|{}|{:?}|{}|{}|{:?}",
        d.id,
        d.spec_sel.version,
        m.view.start,
        m.view.len,
        m.spec,
        cols,
        rows,
        (st.theme.selection, st.theme.selection_alpha),
    );
    let id = egui::Id::new("af27-spec-sel-tex");
    if let Some((k, tex)) = ui
        .ctx()
        .data(|x| x.get_temp::<(String, egui::TextureHandle)>(id))
    {
        if k == key {
            return Some(tex);
        }
    }
    let times: Vec<f64> = (0..cols)
        .map(|c| m.view.start + (c as f64 + 0.5) / cols as f64 * m.view.len)
        .collect();
    // Rows top first: high frequencies.
    let us: Vec<f64> = (0..rows)
        .map(|r| {
            let frac = 1.0 - (r as f64 + 0.5) / rows as f64;
            hz_to_u(m.spec.freq_at(frac, m.ny))
        })
        .collect();
    let mask = rasterise(&d.spec_sel.regions, &times, &us);
    let [r, g, b] = st.theme.selection;
    let a = (st.theme.selection_alpha as f32 * 1.4).clamp(40.0, 150.0) as u8;
    let mut img = egui::ColorImage::new([cols, rows], Color32::TRANSPARENT);
    for (i, v) in mask.iter().enumerate() {
        if *v > 0.0 {
            img.pixels[i] = Color32::from_rgba_unmultiplied(r, g, b, a);
        }
    }
    let tex = ui
        .ctx()
        .load_texture("spectral-selection", img, egui::TextureOptions::LINEAR);
    ui.ctx().data_mut(|x| x.insert_temp(id, (key, tex.clone())));
    Some(tex)
}

/// The small floating bar over a selection: its actions.
pub fn action_bar(ui: &egui::Ui, st: &mut State, sa: Rect) {
    let dragging = ui.data(|x| x.get_temp::<Drag>(drag_id())).is_some();
    let Some(m) = mapper(st, sa) else { return };
    let Some((_, t1, _, u1)) = st.doc().and_then(|d| d.spec_sel.bounds()) else {
        return;
    };
    if dragging || st.tool == Tool::SpotRepair {
        return;
    }
    let busy = st.spectral_job.is_some();
    // (max then min: never panics on a small editor.)
    let x = m
        .x(t1)
        .min(sa.right() - SCALE_W - 4.0)
        .max(sa.left() + 300.0);
    let y = m.y(0, u1).max(sa.top() + 30.0).min(sa.bottom());
    let mut pick: Option<(Action, &str)> = None;
    let mut clear = false;
    egui::Area::new(egui::Id::new("af27-spec-bar"))
        .order(egui::Order::Foreground)
        .pivot(Align2::RIGHT_BOTTOM)
        .fixed_pos(pos2(x, y - 4.0))
        .constrain_to(sa)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (label, tip, act, name) in ACTIONS {
                        if ui
                            .add_enabled(!busy, egui::Button::new(label).small())
                            .on_hover_text(tip)
                            .clicked()
                        {
                            pick = Some((act, name));
                        }
                    }
                    if ui
                        .add(egui::Button::new("✕").small())
                        .on_hover_text("Clear the spectral selection")
                        .clicked()
                    {
                        clear = true;
                    }
                });
            });
        });
    if let Some((a, name)) = pick {
        run(st, a, name, false);
    }
    if clear {
        if let Some(d) = st.doc_mut() {
            d.spec_sel.clear();
        }
    }
}

/// (bar label, tooltip, action, menu and undo name).
const ACTIONS: [(&str, &str, Action, &str); 4] = [
    (
        "−12 dB",
        "Reduce Selected Area 12 dB",
        Action::Gain(0.251_188_643_150_958),
        "Reduce Selected Area 12 dB",
    ),
    (
        "−24 dB",
        "Reduce Selected Area 24 dB",
        Action::Gain(0.063_095_734_448_019_3),
        "Reduce Selected Area 24 dB",
    ),
    (
        "Remove",
        "Remove Selected Area",
        Action::Gain(0.0),
        "Remove Selected Area",
    ),
    (
        "Repair",
        "Fill the area from the level just before and after it",
        Action::Repair,
        "Repair",
    ),
];

/// Effects > Spectrum.
pub fn effects_submenu(ui: &mut egui::Ui, st: &mut State) {
    let has = st.doc().is_some_and(|d| !d.spec_sel.is_empty()) && st.spectral_job.is_none();
    let why = "Make a spectral selection first (Box, Lasso or Brush Select)";
    for (_, _, act, name) in ACTIONS {
        if ui
            .add_enabled(has, egui::Button::new(name))
            .on_disabled_hover_text(why)
            .clicked()
        {
            run(st, act, name, false);
            ui.close_menu();
        }
    }
    ui.separator();
    if ui
        .add_enabled(st.doc().is_some(), egui::Button::new("Spot Repair Tool"))
        .on_hover_text("Paint over a sound in the spectrum; it is repaired when you let go")
        .clicked()
    {
        st.choose_tool(Tool::SpotRepair);
        ui.close_menu();
    }
}

// ------------------------------------------------------------ colours

/// Spectrogram Colors, for the spectrum's right-click menu: the theme's
/// gradient, or two colours blended around the colour wheel.
pub fn colors_menu(ui: &mut egui::Ui, st: &mut State) {
    ui.menu_button("Spectrogram Colors", |ui| {
        if ui.radio(st.spec_colors.is_none(), "Theme Colors").clicked() {
            st.spec_colors = None;
        }
        if ui
            .radio(st.spec_colors.is_some(), "Two Colors (around the wheel)")
            .clicked()
            && st.spec_colors.is_none()
        {
            st.spec_colors = Some(theme_pair(&st.theme));
        }
        if let Some(pair) = &mut st.spec_colors {
            ui.horizontal(|ui| {
                ui.color_edit_button_srgb(&mut pair[0]);
                ui.label("Quiet");
            });
            ui.horizontal(|ui| {
                ui.color_edit_button_srgb(&mut pair[1]);
                ui.label("Loud");
            });
            if ui.button("Swap").clicked() {
                pair.swap(0, 1);
            }
            if ui.button("Theme Defaults").clicked() {
                *pair = theme_pair(&st.theme);
            }
        }
    });
}

/// The two colours a theme suggests: a dark one from low in its spectrum
/// gradient and its loudest colour.
pub fn theme_pair(th: &crate::theme::Theme) -> [[u8; 3]; 2] {
    let s = &th.spectrum;
    match s.len() {
        0 => [[20, 0, 60], [255, 240, 120]],
        1 => [[0, 0, 0], s[0]],
        2 => [s[0], s[1]],
        n => [s[1], s[n - 1]],
    }
}

/// Blends `a` to `b` by `f`, hue going forward around the wheel.
pub fn wheel_blend(a: [u8; 3], b: [u8; 3], f: f32) -> [u8; 3] {
    use egui::ecolor::Hsva;
    let ha = Hsva::from(Color32::from_rgb(a[0], a[1], a[2]));
    let hb = Hsva::from(Color32::from_rgb(b[0], b[1], b[2]));
    let f = f.clamp(0.0, 1.0);
    let dh = (hb.h - ha.h).rem_euclid(1.0);
    let h = (ha.h + dh * f).rem_euclid(1.0);
    let c = Color32::from(Hsva::new(
        h,
        ha.s + (hb.s - ha.s) * f,
        ha.v + (hb.v - ha.v) * f,
        1.0,
    ));
    [c.r(), c.g(), c.b()]
}

/// 256 spectrogram colours, quiet to loud, from the chosen colours.
pub fn color_lut(st: &State) -> Vec<[u8; 3]> {
    (0..256)
        .map(|i| {
            let f = i as f32 / 255.0;
            match st.spec_colors {
                Some([a, b]) => wheel_blend(a, b, f),
                None => st.theme.spectrum_color(f),
            }
        })
        .collect()
}

// ---------------------------------------------------------------- jobs

/// A spectral edit running in the background.
pub struct SpecJob {
    pub doc: DocId,
    /// The document's version when the edit started.
    pub version: u64,
    pub label: String,
    pub clear_after: bool,
    progress: Arc<AtomicU32>,
    rx: Receiver<Option<AudioBuffer>>,
}

/// How far the running spectral edit is, 0..1.
pub fn progress(st: &State) -> Option<f32> {
    st.spectral_job
        .as_ref()
        .map(|j| f32::from_bits(j.progress.load(Ordering::Relaxed)))
}

/// The edit itself: every channel, one at a time.
pub fn process(
    buf: &AudioBuffer,
    regions: &[Region],
    fft: usize,
    action: Action,
    progress: &mut dyn FnMut(f32),
) -> Option<AudioBuffer> {
    let grid = edit_grid(regions, buf.rate as f64, buf.frames(), fft)?;
    let nch = buf.num_channels();
    let mut out = buf.clone();
    let mut changed = false;
    for ch in 0..nch {
        let mut p = |f: f32| progress((ch as f32 + f) / nch as f32);
        if let Some((first, seg)) = edit_channel(&buf.channels[ch], &grid, action, &mut p) {
            out.channels[ch][first..first + seg.len()].copy_from_slice(&seg);
            changed = true;
        }
    }
    changed.then_some(out)
}

/// Starts `action` on the current document's spectral selection. The
/// result lands as one undoable edit named `label`.
pub fn run(st: &mut State, action: Action, label: &str, clear_after: bool) {
    if st.spectral_job.is_some() {
        st.error("A spectral edit is already running.");
        return;
    }
    let fft = st.spec.fft.clamp(512, 8192);
    let Some(d) = st.doc() else {
        st.error("Open a file in the Waveform editor first.");
        return;
    };
    if d.spec_sel.is_empty() {
        st.toast("Make a spectral selection first (Box, Lasso or Brush Select).");
        return;
    }
    let regions = d.spec_sel.regions.clone();
    let buf = d.buf.clone();
    let (doc, version) = (d.id, d.version);
    let progress = Arc::new(AtomicU32::new(0));
    let p2 = progress.clone();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let r = process(&buf, &regions, fft, action, &mut |f| {
            p2.store(f.to_bits(), Ordering::Relaxed)
        });
        let _ = tx.send(r);
    });
    st.spectral_job = Some(SpecJob {
        doc,
        version,
        label: label.to_string(),
        clear_after,
        progress,
        rx,
    });
}

/// Picks up a finished spectral edit (called every frame).
pub fn poll(st: &mut State) {
    let Some(job) = &st.spectral_job else { return };
    let r = match job.rx.try_recv() {
        Ok(r) => r,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => {
            st.spectral_job = None;
            st.error("The spectral edit stopped unexpectedly.");
            return;
        }
    };
    let job = st.spectral_job.take().unwrap();
    let Some(buf) = r else {
        st.toast("Nothing in the spectral selection to change.");
        return;
    };
    let Some(d) = st.docs.iter_mut().find(|d| d.id == job.doc) else {
        return;
    };
    if d.version != job.version {
        st.error("The file changed during the spectral edit, so it was not applied.");
        return;
    }
    d.set_buffer_on(&job.label, buf);
    if job.clear_after {
        d.spec_sel.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapper(spec: SpecView, bands: usize) -> Mapper {
        Mapper {
            area: Rect::from_min_size(pos2(0.0, 100.0), vec2(1000.0, 400.0)),
            bands,
            view: View {
                start: 0.0,
                len: 10.0,
            },
            spec,
            ny: 24_000.0,
            dur: 10.0,
        }
    }

    #[test]
    fn axis_round_trip() {
        for f in [0.0, 20.0, 440.0, 1000.0, 12_345.0, 24_000.0] {
            assert!((u_to_hz(hz_to_u(f)) - f).abs() < 1e-6);
        }
    }

    #[test]
    fn rasterises_box_lasso_and_brush() {
        let times: Vec<f64> = (0..10).map(|i| i as f64).collect();
        let us: Vec<f64> = (0..10).map(|i| i as f64).collect();
        let p = |t, u| SPoint { t, u };
        let b = rasterise(
            &[Region::Box {
                a: p(2.5, 6.5),
                b: p(5.5, 3.5),
            }],
            &times,
            &us,
        );
        let on = |g: &[f32], t: usize, u: usize| g[u * 10 + t] > 0.5;
        assert!(on(&b, 3, 4) && on(&b, 5, 6) && !on(&b, 2, 4) && !on(&b, 3, 7));
        assert_eq!(b.iter().filter(|v| **v > 0.5).count(), 3 * 3);
        // A concave (L-shaped) lasso leaves its notch empty.
        let l = rasterise(
            &[Region::Lasso(vec![
                p(0.5, 0.5),
                p(8.5, 0.5),
                p(8.5, 3.5),
                p(3.5, 3.5),
                p(3.5, 8.5),
                p(0.5, 8.5),
            ])],
            &times,
            &us,
        );
        assert!(on(&l, 1, 1) && on(&l, 7, 2) && on(&l, 2, 7));
        assert!(!on(&l, 6, 6), "the notch of the L is outside");
        // A stroke from (1,5) to (8,5), radius 1.5: a band around u = 5.
        let s = rasterise(
            &[Region::Brush {
                pts: vec![p(1.0, 5.0), p(8.0, 5.0)],
                rt: 1.5,
                ru: 1.5,
            }],
            &times,
            &us,
        );
        assert!(on(&s, 4, 5) && on(&s, 4, 4) && on(&s, 4, 6) && on(&s, 0, 5));
        assert!(!on(&s, 4, 7) && !on(&s, 4, 3) && !on(&s, 9, 7));
        // A diagonal stroke covers the cells along it.
        let d = rasterise(
            &[Region::Brush {
                pts: vec![p(1.0, 1.0), p(8.0, 8.0)],
                rt: 0.8,
                ru: 0.8,
            }],
            &times,
            &us,
        );
        for i in 1..=8 {
            assert!(on(&d, i, i));
        }
        assert!(!on(&d, 1, 8) && !on(&d, 8, 1));
    }

    #[test]
    fn edit_grid_has_a_solid_middle_and_soft_edges() {
        let rate = 48_000.0;
        let r = Region::Box {
            a: SPoint {
                t: 0.5,
                u: hz_to_u(800.0),
            },
            b: SPoint {
                t: 1.0,
                u: hz_to_u(1600.0),
            },
        };
        let g = edit_grid(&[r], rate, 96_000, 2048).unwrap();
        let bin = |hz: f64| (hz * 2048.0 / rate).round() as usize;
        let frame = |t: f64| (t * rate / g.hop as f64).round() as i64;
        assert_eq!(g.at(frame(0.75), bin(1200.0)), 1.0);
        assert_eq!(g.at(frame(0.75), bin(3000.0)), 0.0);
        assert_eq!(g.at(frame(0.2), bin(1200.0)), 0.0);
        // Just past the top edge in frequency: partly in.
        let top = (1600.0 * 2048.0 / rate).floor() as usize;
        let w = g.at(frame(0.75), top + 2);
        assert!(w > 0.0 && w < 1.0, "{w}");
    }

    #[test]
    fn a_selection_survives_a_change_of_scale_and_zoom() {
        let mut spec = SpecView::default();
        let m = mapper(spec, 1);
        let a = m.point(0, pos2(200.0, 200.0));
        let b = m.point(0, pos2(400.0, 300.0));
        let (fa, fb) = (u_to_hz(a.u), u_to_hz(b.u));
        let sel = vec![Region::Box { a, b }];
        let before = edit_grid(&sel, 48_000.0, 480_000, 2048).unwrap();
        // Another scale and a frequency zoom: the shape keeps its
        // frequencies and the grid an edit uses is the same.
        spec.log_step = 0;
        spec.zoom(0.5, 3000.0, 24_000.0);
        let m2 = mapper(spec, 1);
        assert_ne!(m.y(0, a.u), m2.y(0, a.u));
        let back = m2.point(0, m2.screen(0, a));
        assert!((u_to_hz(back.u) - fa).abs() / fa < 1e-6);
        let back = m2.point(0, m2.screen(0, b));
        assert!((u_to_hz(back.u) - fb).abs() / fb < 1e-6);
        let after = edit_grid(&sel, 48_000.0, 480_000, 2048).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn drags_clamp_to_the_band_they_started_in() {
        let spec = SpecView::default();
        let m = mapper(spec, 2);
        // Band 0 is y 100..300, band 1 is 300..500.
        assert_eq!(m.band_of(150.0), 0);
        assert_eq!(m.band_of(350.0), 1);
        assert_eq!(m.band_of(9999.0), 1);
        let (lo, hi) = spec.bounds(24_000.0);
        // Past band 0's bottom (into band 1): band 0's lowest frequency.
        let p = m.point(0, pos2(500.0, 420.0));
        assert!((u_to_hz(p.u) - lo).abs() < 1e-6);
        // Above the top: its highest.
        let p = m.point(0, pos2(500.0, 20.0));
        assert!((u_to_hz(p.u) - hi).abs() < 1e-3);
        // Band 1 dragged above its top stops at its own top.
        let p = m.point(1, pos2(500.0, 150.0));
        assert!((u_to_hz(p.u) - hi).abs() < 1e-3);
        // Time clamps to the file.
        let p = m.point(1, pos2(-50.0, 400.0));
        assert_eq!(p.t, 0.0);
        // The same point shows at the same height in every band.
        let q = m.point(0, pos2(500.0, 180.0));
        assert!((m.y(1, q.u) - m.y(0, q.u) - m.band_h()).abs() < 1e-3);
    }

    #[test]
    fn process_edits_every_channel_and_keeps_the_rest() {
        let rate = 48_000u32;
        let n = 96_000;
        let mut buf = AudioBuffer::new(rate, 2, n);
        for ch in 0..2 {
            for i in 0..n {
                let t = i as f64 / rate as f64;
                buf.channels[ch][i] = (0.3 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()
                    + 0.3 * (2.0 * std::f64::consts::PI * 7000.0 * t).sin())
                    as f32;
            }
        }
        let sel = [Region::Box {
            a: SPoint {
                t: 0.8,
                u: hz_to_u(700.0),
            },
            b: SPoint {
                t: 1.2,
                u: hz_to_u(1400.0),
            },
        }];
        let out = process(&buf, &sel, 2048, Action::Gain(0.0), &mut |_| {}).unwrap();
        for ch in 0..2 {
            // Far from the box: the very same samples.
            assert_eq!(&out.channels[ch][..20_000], &buf.channels[ch][..20_000]);
            assert_eq!(&out.channels[ch][80_000..], &buf.channels[ch][80_000..]);
            // In the box the 1 kHz tone is gone, the 7 kHz tone stays.
            let lvl = |x: &[f32], hz: f64| {
                let (mut s, mut c) = (0.0, 0.0);
                for (i, &v) in x.iter().enumerate().take(50_000).skip(46_000) {
                    let ph = 2.0 * std::f64::consts::PI * hz * i as f64 / rate as f64;
                    s += v as f64 * ph.sin();
                    c += v as f64 * ph.cos();
                }
                2.0 * f64::hypot(s, c) / 4000.0
            };
            assert!(lvl(&out.channels[ch], 1000.0) < 0.003);
            assert!((lvl(&out.channels[ch], 7000.0) - 0.3).abs() < 0.003);
        }
    }

    #[test]
    fn wheel_blend_goes_forward_around_the_wheel() {
        // Red to blue going forward passes green; ends are exact.
        assert_eq!(wheel_blend([255, 0, 0], [0, 0, 255], 0.0), [255, 0, 0]);
        let end = wheel_blend([255, 0, 0], [0, 0, 255], 1.0);
        assert!(end[2] > 250 && end[0] < 5);
        let mid = wheel_blend([255, 0, 0], [0, 0, 255], 0.5);
        assert!(mid[1] > 200, "{mid:?}");
    }
}
