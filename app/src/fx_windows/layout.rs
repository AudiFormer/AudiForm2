// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The grid every effect face is laid out on, so the windows come out
//! even: one margin and gap scale, rows of equal cells that fill the
//! face's width with each control centred in its cell, framed groups whose
//! frames in a row share one height, and the hero row (a big dial either
//! side of a centred display).
//!
//! Each face picks its width once (from what its widest row needs, never
//! from the window, so a window fits its face and never grows on its own)
//! and every row is laid out across that width.
//!
//! Centring needs a block's size before it is drawn; egui lays out in one
//! pass, so each cell remembers the size its contents took last frame and
//! asks for one more frame when that changes (a window settles on the
//! frame after it opens, or after a control appears or goes).

use egui::{pos2, vec2, Align, FontId, Layout, Rect, RichText, Shape, Stroke, Vec2};

use crate::theme::{c, ca, Theme};

/// The gap between controls in a group.
pub const GAP_S: f32 = 8.0;
/// The gap between groups, and between rows of a face.
pub const GAP: f32 = 12.0;
/// The gap between a window's columns.
pub const GAP_L: f32 = 16.0;
/// A group frame's inner margin.
pub const PAD: f32 = 8.0;
/// The height of a group's heading.
const TITLE_H: f32 = 16.0;

/// A small dial's size and the cell it wants.
pub const KNOB: f32 = 56.0;
pub const CELL: f32 = 78.0;
/// A hero dial's size and the side column it wants.
pub const HERO: f32 = 104.0;
pub const SIDE: f32 = 124.0;
/// The narrowest face (about as wide as the presets row above it).
pub const MIN_W: f32 = 344.0;

/// A size remembered under `id` from the last frame (zero the first
/// time); stores `now` for the next one and asks for a frame when it moved.
fn remember(ui: &egui::Ui, id: egui::Id, now: Option<Vec2>) -> Vec2 {
    let prev: Vec2 = ui.ctx().data(|d| d.get_temp(id)).unwrap_or(Vec2::ZERO);
    if let Some(now) = now {
        if (now - prev).abs().max_elem() > 0.5 {
            ui.ctx().data_mut(|d| d.insert_temp(id, now));
            ui.ctx().request_repaint();
        }
    }
    prev
}

/// Draws `add` centred (both ways) in `area`, without moving `ui`'s
/// cursor. Returns the size it took.
fn put(ui: &mut egui::Ui, id: egui::Id, area: Rect, add: impl FnOnce(&mut egui::Ui)) -> Vec2 {
    let prev = remember(ui, id, None);
    let off = ((area.size() - prev) / 2.0).max(Vec2::ZERO);
    let at = Rect::from_min_max(
        area.min + off,
        pos2(
            area.max.x.max(area.min.x + off.x + 1.0),
            area.min.y + 4000.0,
        ),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(id)
            .max_rect(at)
            .layout(Layout::top_down(Align::Min)),
    );
    add(&mut child);
    let size = child.min_rect().size();
    remember(ui, id, Some(size));
    size
}

/// A stable id for the next layout block in `ui`.
fn next_id(ui: &mut egui::Ui, salt: &str) -> egui::Id {
    let id = ui.next_auto_id().with(salt);
    ui.skip_ahead_auto_ids(1);
    id
}

/// A row `w` wide cut into cells in proportion to `weights`; cell `i`'s
/// contents (from `add(ui, i)`) are centred in it, across and down (to
/// the tallest cell). Returns the row's height.
pub fn row(
    ui: &mut egui::Ui,
    w: f32,
    weights: &[f32],
    mut add: impl FnMut(&mut egui::Ui, usize),
) -> f32 {
    let id = next_id(ui, "row");
    let top = ui.cursor().min;
    let total: f32 = weights.iter().sum::<f32>().max(1e-3);
    let tall_prev = remember(ui, id, None).y;
    let mut x = top.x;
    let mut tall = 0.0f32;
    for (i, wt) in weights.iter().enumerate() {
        let cw = w * wt / total;
        let area = Rect::from_min_size(pos2(x, top.y), vec2(cw, tall_prev));
        tall = tall.max(put(ui, id.with(i), area, |ui| add(ui, i)).y);
        x += cw;
    }
    remember(ui, id, Some(vec2(w, tall)));
    ui.advance_cursor_after_rect(Rect::from_min_size(top, vec2(w, tall)));
    tall
}

/// `n` equal cells across `w`, each centred (a row of dials).
pub fn cells(ui: &mut egui::Ui, w: f32, n: usize, add: impl FnMut(&mut egui::Ui, usize)) -> f32 {
    row(ui, w, &vec![1.0; n.max(1)], add)
}

/// `n` cells in rows of at most `per_row`, every cell `w / per_row` wide,
/// a short last row centred under the others.
pub fn grid(
    ui: &mut egui::Ui,
    w: f32,
    n: usize,
    per_row: usize,
    mut add: impl FnMut(&mut egui::Ui, usize),
) {
    let per = per_row.clamp(1, n.max(1));
    let cw = w / per as f32;
    let mut k = 0;
    while k < n {
        let here = (n - k).min(per);
        let side = (w - cw * here as f32) / 2.0;
        let mut weights = vec![cw; here];
        if side > 0.5 {
            weights.insert(0, side);
            weights.push(side);
        }
        let lead = usize::from(side > 0.5);
        let base = k;
        row(ui, w, &weights, |ui, i| {
            if i >= lead && i < lead + here {
                add(ui, base + i - lead);
            }
        });
        k += here;
    }
}

/// One block centred across `w`.
pub fn centred(ui: &mut egui::Ui, w: f32, add: impl FnOnce(&mut egui::Ui)) -> f32 {
    let mut add = Some(add);
    row(ui, w, &[1.0], |ui, _| {
        if let Some(f) = add.take() {
            f(ui);
        }
    })
}

/// The hero row: a side column, the display, the other side column, each
/// centred (the sides against the display's height). `add` gets 0, 1, 2.
pub fn hero(ui: &mut egui::Ui, w: f32, side: f32, add: impl FnMut(&mut egui::Ui, usize)) -> f32 {
    row(ui, w, &[side, (w - 2.0 * side).max(1.0), side], add)
}

/// A line of text wrapped to `w` and centred.
pub fn note(ui: &mut egui::Ui, w: f32, text: RichText) {
    centred(ui, w, |ui| {
        ui.set_max_width(w);
        ui.with_layout(Layout::top_down(Align::Center), |ui| {
            ui.add(egui::Label::new(text).wrap());
        });
    });
}

/// The frame and heading every group of controls shares.
fn group_frame(th: &Theme, rect: Rect, title: &str, p: &egui::Painter) -> [Shape; 2] {
    let r = if th.bevel.is_some() { 0.0 } else { 4.0 };
    let frame = Shape::Rect(egui::epaint::RectShape::new(
        rect,
        r,
        ca(th.background, 70),
        Stroke::new(1.0_f32, ca(th.border, 150)),
    ));
    // Letter spacing as the faceplate headings have it.
    let text = if title.is_empty() {
        Shape::Noop
    } else {
        let mut job = egui::text::LayoutJob::default();
        job.append(
            &title.to_uppercase(),
            0.0,
            egui::TextFormat {
                font_id: FontId::proportional(9.5),
                color: c(th.text_dim),
                extra_letter_spacing: 1.5,
                ..Default::default()
            },
        );
        let g = p.layout_job(job);
        Shape::galley(rect.left_top() + vec2(PAD, PAD - 2.0), g, c(th.text_dim))
    };
    [frame, text]
}

/// A row of framed groups `w` wide, widths in proportion to `items`'
/// weights, every frame as tall as the tallest (and at least `min_h`),
/// each group's contents centred in it. Returns the row's height.
pub fn groups_min(
    ui: &mut egui::Ui,
    th: &Theme,
    w: f32,
    items: &[(&str, f32)],
    min_h: f32,
    mut add: impl FnMut(&mut egui::Ui, usize),
) -> f32 {
    let id = next_id(ui, "groups");
    let top = ui.cursor().min;
    let n = items.len().max(1);
    let total: f32 = items.iter().map(|g| g.1).sum::<f32>().max(1e-3);
    let free = w - GAP * (n - 1) as f32;
    let head = |t: &str| if t.is_empty() { 0.0 } else { TITLE_H };
    let body_prev = remember(ui, id, None).y;
    let mut boxes = Vec::with_capacity(n);
    let mut x = top.x;
    let mut body = (min_h - 2.0 * PAD).max(0.0);
    for (i, (title, wt)) in items.iter().enumerate() {
        let gw = free * wt / total;
        let slot = ui.painter().add(Shape::Noop);
        let label = ui.painter().add(Shape::Noop);
        let h = head(title);
        let area = Rect::from_min_size(
            pos2(x + PAD, top.y + PAD + h),
            vec2(gw - 2.0 * PAD, (body_prev - h).max(0.0)),
        );
        let used = put(ui, id.with(i), area, |ui| add(ui, i));
        body = body.max(used.y + h);
        boxes.push((slot, label, x, gw, *title));
        x += gw + GAP;
    }
    remember(ui, id, Some(vec2(w, body)));
    let tall = body + 2.0 * PAD;
    let p = ui.painter().clone();
    for (slot, label, x, gw, title) in boxes {
        let rect = Rect::from_min_size(pos2(x, top.y), vec2(gw, tall));
        let [frame, text] = group_frame(th, rect, title, &p);
        p.set(slot, frame);
        p.set(label, text);
    }
    ui.advance_cursor_after_rect(Rect::from_min_size(top, vec2(w, tall)));
    tall
}

/// A row of framed groups (see [`groups_min`]).
pub fn groups(
    ui: &mut egui::Ui,
    th: &Theme,
    w: f32,
    items: &[(&str, f32)],
    add: impl FnMut(&mut egui::Ui, usize),
) -> f32 {
    groups_min(ui, th, w, items, 0.0, add)
}

/// Columns of the given widths side by side, `gap` apart, top-aligned,
/// each laid out by `add(ui, i)` from its left edge. Returns each
/// column's height.
pub fn columns(
    ui: &mut egui::Ui,
    widths: &[f32],
    gap: f32,
    mut add: impl FnMut(&mut egui::Ui, usize),
) -> Vec<f32> {
    let top = ui.cursor().min;
    let mut x = top.x;
    let mut hs = Vec::with_capacity(widths.len());
    for (i, cw) in widths.iter().enumerate() {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(("col", i))
                .max_rect(Rect::from_min_size(pos2(x, top.y), vec2(*cw, 4000.0)))
                .layout(Layout::top_down(Align::Min)),
        );
        child.set_width(*cw);
        add(&mut child, i);
        hs.push(child.min_rect().height());
        x += cw + gap;
    }
    let w = (x - gap - top.x).max(0.0);
    let h = hs.iter().copied().fold(0.0, f32::max);
    ui.advance_cursor_after_rect(Rect::from_min_size(top, vec2(w, h)));
    hs
}

/// A height remembered between frames under a name of `ui`'s (for a
/// column that should match another one drawn after it).
pub fn last_height(ui: &egui::Ui, salt: &str, now: Option<f32>) -> f32 {
    let id = ui.id().with(("last-h", salt));
    remember(ui, id, now.map(|h| vec2(0.0, h))).y
}
