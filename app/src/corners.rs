// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Four-corner (and T) dragging: where a vertical divider between panels
//! meets a horizontal one, grabbing the meeting point moves both at once,
//! in every direction, as Audiform did.

use egui::{CursorIcon, Pos2, Rect, Sense, Vec2};
use egui_dock::{DockState, Node, NodeIndex};

/// How near the meeting point the pointer must be, in points.
const GRAB: f32 = 7.0;

/// A divider: the split node it belongs to, whether it is vertical (a
/// left/right split), and its line.
#[derive(Clone, Copy, Debug)]
struct Divider {
    node: usize,
    vertical: bool,
    at: f32,
    from: f32,
    to: f32,
}

fn dividers<T>(dock: &DockState<T>) -> Vec<Divider> {
    let mut out = Vec::new();
    for (i, node) in dock.main_surface().iter().enumerate() {
        match node {
            Node::Horizontal { rect, fraction } => out.push(Divider {
                node: i,
                vertical: true,
                at: rect.left() + rect.width() * fraction,
                from: rect.top(),
                to: rect.bottom(),
            }),
            Node::Vertical { rect, fraction } => out.push(Divider {
                node: i,
                vertical: false,
                at: rect.top() + rect.height() * fraction,
                from: rect.left(),
                to: rect.right(),
            }),
            _ => {}
        }
    }
    out
}

/// Where vertical divider `v` meets horizontal divider `h`, if they do
/// (a cross, or a T where one ends on the other).
fn meeting(v: &Divider, h: &Divider) -> Option<Pos2> {
    let x_ok = v.at >= h.from - GRAB && v.at <= h.to + GRAB;
    let y_ok = h.at >= v.from - GRAB && h.at <= v.to + GRAB;
    (x_ok && y_ok).then(|| Pos2::new(v.at, h.at))
}

/// All the meeting points, with the two split nodes at each.
fn junctions<T>(dock: &DockState<T>) -> Vec<(Pos2, usize, usize)> {
    let d = dividers(dock);
    let mut out = Vec::new();
    for v in d.iter().filter(|d| d.vertical) {
        for h in d.iter().filter(|d| !d.vertical) {
            if let Some(p) = meeting(v, h) {
                out.push((p, v.node, h.node));
            }
        }
    }
    out
}

fn split_rect<T>(dock: &DockState<T>, i: usize) -> Option<Rect> {
    dock.main_surface().iter().nth(i).and_then(|n| n.rect())
}

fn nudge<T>(dock: &mut DockState<T>, i: usize, by: f32) {
    if let Node::Horizontal { fraction, .. } | Node::Vertical { fraction, .. } =
        &mut dock.main_surface_mut()[NodeIndex(i)]
    {
        *fraction = (*fraction + by).clamp(0.05, 0.95);
    }
}

/// Call right after the dock is shown, inside the same `Ui`.
pub fn corners<T>(ui: &mut egui::Ui, dock: &mut DockState<T>) {
    let mem = egui::Id::new("af2-corner-drag");
    let held: Option<(usize, usize)> = ui.memory(|m| m.data.get_temp(mem));
    let list = junctions(dock);
    for (k, (p, v, h)) in list.iter().enumerate() {
        let area = Rect::from_center_size(*p, Vec2::splat(GRAB * 2.0));
        let resp = ui.interact(area, mem.with(("corner", *v, *h, k)), Sense::drag());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::Move);
        }
        if resp.drag_started() {
            ui.memory_mut(|m| m.data.insert_temp(mem, (*v, *h)));
        }
    }
    // Move both dividers of the held meeting point by the pointer's motion.
    if let Some((v, h)) = held {
        let down = ui.input(|i| i.pointer.primary_down());
        if !down {
            ui.memory_mut(|m| m.data.remove::<(usize, usize)>(mem));
            return;
        }
        ui.ctx().set_cursor_icon(CursorIcon::Move);
        let d = ui.input(|i| i.pointer.delta());
        if let (Some(rv), Some(rh)) = (split_rect(dock, v), split_rect(dock, h)) {
            if d.x != 0.0 && rv.width() > 1.0 {
                nudge(dock, v, d.x / rv.width());
            }
            if d.y != 0.0 && rh.height() > 1.0 {
                nudge(dock, h, d.y / rh.height());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crosses_and_tees_meet_but_apart_lines_do_not() {
        let v = Divider {
            node: 0,
            vertical: true,
            at: 100.0,
            from: 0.0,
            to: 200.0,
        };
        let cross = Divider {
            node: 1,
            vertical: false,
            at: 50.0,
            from: 0.0,
            to: 300.0,
        };
        let tee = Divider {
            node: 2,
            vertical: false,
            at: 200.0,
            from: 0.0,
            to: 100.0,
        };
        let apart = Divider {
            node: 3,
            vertical: false,
            at: 50.0,
            from: 150.0,
            to: 300.0,
        };
        assert_eq!(meeting(&v, &cross), Some(Pos2::new(100.0, 50.0)));
        assert_eq!(meeting(&v, &tee), Some(Pos2::new(100.0, 200.0)));
        assert_eq!(meeting(&v, &apart), None);
    }

    #[test]
    fn the_default_workspace_has_meeting_points() {
        let mut d = crate::workspace::Workspace::Music.build();
        // Lay it out so the split rects are real.
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui_dock::DockArea::new(&mut d).show_inside(ui, &mut Dummy);
            });
        });
        assert!(!junctions(&d).is_empty());
    }

    struct Dummy;
    impl egui_dock::TabViewer for Dummy {
        type Tab = crate::ui::Tab;
        fn title(&mut self, t: &mut Self::Tab) -> egui::WidgetText {
            t.title().into()
        }
        fn ui(&mut self, _: &mut egui::Ui, _: &mut Self::Tab) {}
    }
}
