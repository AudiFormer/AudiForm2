// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The `win95` chrome: flat grey windows with two-pixel bevels on a teal
//! desktop, navy captions, raised tabs, sunken white fields and navy menu
//! highlights, as on a desktop computer of 1995. A theme turns it on with
//! `"chrome": "win95"`.
//!
//! egui draws its widgets as plain rectangles with one outline colour, so
//! this chrome hands egui outline colours that mean something ("a raised
//! button", "a sunken field", "a window") and, at the end of each frame,
//! [`restyle`] swaps every rectangle carrying one of them for the real
//! bevelled shape. Text over a navy highlight turns white on the way.

use egui::epaint::{CircleShape, ClippedShape, ColorMode, PathStroke, RectShape};
use egui::layers::{PaintList, ShapeIdx};
use egui::{Color32, Order, Painter, Pos2, Rect, Rounding, Shape, Stroke, Vec2};

use crate::theme::Theme;

pub const FACE: Color32 = Color32::from_rgb(192, 192, 192);
pub const LIGHT: Color32 = Color32::from_rgb(223, 223, 223);
pub const WHITE: Color32 = Color32::WHITE;
pub const SHADOW: Color32 = Color32::from_rgb(128, 128, 128);
pub const DARK: Color32 = Color32::BLACK;
pub const NAVY: Color32 = Color32::from_rgb(0, 0, 128);
pub const CAPTION_END: Color32 = Color32::from_rgb(16, 132, 208);
pub const TOOLTIP: Color32 = Color32::from_rgb(255, 255, 225);

/// The outline colours that stand for bevelled shapes. Near-greys, so a
/// shape this pass never sees still looks like a plain grey outline.
pub const S_RAISED: Color32 = Color32::from_rgb(127, 128, 129);
pub const S_HOVER: Color32 = Color32::from_rgb(127, 128, 130);
pub const S_PRESSED: Color32 = Color32::from_rgb(127, 128, 131);
pub const S_OPEN: Color32 = Color32::from_rgb(127, 128, 132);
pub const S_FOCUS: Color32 = Color32::from_rgb(127, 128, 133);
pub const S_WINDOW: Color32 = Color32::from_rgb(127, 128, 134);
pub const S_GROOVE: Color32 = Color32::from_rgb(127, 128, 135);
/// The face of a widget under the pointer: grey to the eye, but in a menu
/// it becomes the navy bar.
pub const HOVER_FACE: Color32 = Color32::from_rgb(192, 192, 191);
/// Widgets' foreground (ticks, arrows, slider and scroll bar thumbs).
pub const S_FG: Color32 = Color32::from_rgb(1, 2, 3);

/// Height of a panel's caption, and how far the tab strip moves down to
/// make room for it.
pub const CAPTION: f32 = 18.0;
/// The tab strip's height with the caption above it.
pub const TAB_BAR: f32 = 44.0;

pub fn on(th: &Theme) -> bool {
    th.chrome == "win95"
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    /// A window, panel or menu.
    Raised,
    /// A push button.
    Button,
    /// A push button held down.
    Pressed,
    /// A text field, check box or the time display.
    Well,
    /// An etched frame round a group.
    Groove,
    /// One-pixel raised edge (a hovered flat tool button, a status bar).
    ThinRaised,
    /// One-pixel sunken edge (an open menu title, a status field).
    ThinSunken,
}

/// The four colours of an edge: outer top-left, outer bottom-right, inner
/// top-left, inner bottom-right (None: one pixel only).
fn colours(e: Edge) -> (Color32, Color32, Option<(Color32, Color32)>) {
    match e {
        Edge::Raised => (LIGHT, DARK, Some((WHITE, SHADOW))),
        Edge::Button => (WHITE, DARK, Some((LIGHT, SHADOW))),
        Edge::Pressed => (DARK, WHITE, Some((SHADOW, LIGHT))),
        Edge::Well => (SHADOW, WHITE, Some((DARK, LIGHT))),
        Edge::Groove => (SHADOW, WHITE, Some((WHITE, SHADOW))),
        Edge::ThinRaised => (WHITE, SHADOW, None),
        Edge::ThinSunken => (SHADOW, WHITE, None),
    }
}

/// One screen pixel in points, at least one point's worth of pixels.
fn unit(ppp: f32) -> f32 {
    ppp.round().max(1.0) / ppp
}

/// `r` with its edges on whole screen pixels.
pub fn snap(r: Rect, ppp: f32) -> Rect {
    let s = |v: f32| (v * ppp).round() / ppp;
    Rect::from_min_max(
        Pos2::new(s(r.min.x), s(r.min.y)),
        Pos2::new(s(r.max.x), s(r.max.y)),
    )
}

fn strip(out: &mut Vec<Shape>, r: Rect, c: Color32) {
    if r.width() > 0.0 && r.height() > 0.0 {
        out.push(Shape::rect_filled(r, 0.0, c));
    }
}

/// The edge's shapes inside `r` (crisp, a whole pixel per line).
pub fn edge_shapes(out: &mut Vec<Shape>, r: Rect, e: Edge, ppp: f32) {
    let u = unit(ppp);
    let r = snap(r, ppp);
    let (tl, br, inner) = colours(e);
    let ring = |out: &mut Vec<Shape>, r: Rect, tl: Color32, br: Color32| {
        // Bottom and right whole; top and left stop short of them.
        strip(
            out,
            Rect::from_min_max(Pos2::new(r.left(), r.bottom() - u), r.max),
            br,
        );
        strip(
            out,
            Rect::from_min_max(Pos2::new(r.right() - u, r.top()), r.max),
            br,
        );
        strip(
            out,
            Rect::from_min_max(r.min, Pos2::new(r.right() - u, r.top() + u)),
            tl,
        );
        strip(
            out,
            Rect::from_min_max(r.min, Pos2::new(r.left() + u, r.bottom() - u)),
            tl,
        );
    };
    ring(out, r, tl, br);
    if let Some((tl2, br2)) = inner {
        ring(out, r.shrink(u), tl2, br2);
    }
}

/// Paints a face and its edge.
pub fn panel(p: &Painter, r: Rect, face: Option<Color32>, e: Edge) {
    let ppp = p.ctx().pixels_per_point();
    let mut v = Vec::new();
    if let Some(f) = face {
        v.push(Shape::rect_filled(snap(r, ppp), 0.0, f));
    }
    edge_shapes(&mut v, r, e, ppp);
    p.extend(v);
}

/// A window's caption: navy into blue when it has the focus, greys when
/// not, the title in white (doubled for bold), and the three caption
/// buttons, which are only for show.
pub fn caption(p: &Painter, r: Rect, title: &str, focused: bool) {
    let ppp = p.ctx().pixels_per_point();
    let r = snap(r, ppp);
    let (a, b, text) = if focused {
        (NAVY, CAPTION_END, WHITE)
    } else {
        (
            SHADOW,
            Color32::from_rgb(181, 181, 181),
            Color32::from_rgb(212, 208, 200),
        )
    };
    let mut m = egui::epaint::Mesh::default();
    m.colored_vertex(r.left_top(), a);
    m.colored_vertex(r.right_top(), b);
    m.colored_vertex(r.right_bottom(), b);
    m.colored_vertex(r.left_bottom(), a);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(Shape::mesh(m));
    // Three buttons, 16 by 14, the close one set apart.
    let bh = (r.height() - 4.0).max(8.0);
    let bw = bh + 2.0;
    let y = r.top() + 2.0;
    let close = Rect::from_min_size(Pos2::new(r.right() - 2.0 - bw, y), Vec2::new(bw, bh));
    let max = close.translate(Vec2::new(-bw - 2.0, 0.0));
    let min = max.translate(Vec2::new(-bw, 0.0));
    let u = unit(ppp);
    for (k, b) in [min, max, close].into_iter().enumerate() {
        panel(p, b, Some(FACE), Edge::Button);
        let c = snap(b.shrink2(Vec2::new(4.0, 3.0)), ppp);
        match k {
            0 => {
                p.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(c.left(), c.bottom() - 2.0 * u),
                        Pos2::new(c.left() + c.width() * 0.6, c.bottom()),
                    ),
                    0.0,
                    DARK,
                );
            }
            1 => {
                p.rect_stroke(c.shrink(u / 2.0), 0.0, Stroke::new(u, DARK));
                p.rect_filled(
                    Rect::from_min_max(c.min, Pos2::new(c.right(), c.top() + 2.0 * u)),
                    0.0,
                    DARK,
                );
            }
            _ => {
                let s = Stroke::new(1.6 * u, DARK);
                p.line_segment([c.left_top(), c.right_bottom()], s);
                p.line_segment([c.right_top(), c.left_bottom()], s);
            }
        }
    }
    let room = Rect::from_min_max(
        r.left_top() + Vec2::new(6.0, 0.0),
        Pos2::new(min.left() - 4.0, r.bottom()),
    );
    // Bold white type; drawn twice, a pixel apart, without a bold face.
    let font = egui::FontId::new(11.0, egui::FontFamily::Name(crate::skin::BOLD.into()));
    let p2 = p.with_clip_rect(room.intersect(p.clip_rect()));
    let passes: &[f32] = if crate::skin::has_bold(p.ctx()) {
        &[0.0]
    } else {
        &[0.0, 1.0]
    };
    for &dx in passes {
        let dx = dx * u;
        p2.text(
            room.left_center() + Vec2::new(dx, 0.0),
            egui::Align2::LEFT_CENTER,
            title,
            font.clone(),
            text,
        );
    }
}

/// A panel as a window: raised border, caption across the top with the
/// shown tab's title, the tabs as raised tabs (the chosen one taller and
/// joined to its page) and the page's edge.
pub fn window(
    p: &Painter,
    rect: Rect,
    tabs: &[Rect],
    active: Option<Rect>,
    title: &str,
    focused: bool,
) {
    let ppp = p.ctx().pixels_per_point();
    let u = unit(ppp);
    let rect = snap(rect, ppp);
    let inner = rect.shrink(2.0 * u);
    caption(
        p,
        Rect::from_min_max(
            inner.left_top() + Vec2::new(u, u),
            Pos2::new(inner.right() - u, inner.top() + u + CAPTION),
        ),
        title,
        focused,
    );
    // The page's top edge, broken under the chosen tab.
    let page_top = snap(
        Rect::from_min_size(Pos2::new(0.0, rect.top() + TAB_BAR - 2.0), Vec2::ZERO),
        ppp,
    )
    .top();
    let gap = active.map(|a| (a.left(), a.right()));
    let mut x0 = inner.left();
    let line = |p: &Painter, a: f32, b: f32| {
        if b > a {
            p.rect_filled(
                Rect::from_min_max(Pos2::new(a, page_top), Pos2::new(b, page_top + u)),
                0.0,
                WHITE,
            );
        }
    };
    if let Some((a, b)) = gap {
        line(p, x0, a.max(x0));
        x0 = b;
    }
    line(p, x0, inner.right());
    for t in tabs {
        let chosen = active.is_some_and(|a| (a.left() - t.left()).abs() < 0.5);
        let top = rect.top() + CAPTION + if chosen { 5.0 } else { 7.0 };
        let r = snap(
            Rect::from_min_max(
                Pos2::new(t.left() + if chosen { 0.0 } else { 1.0 }, top),
                Pos2::new(t.right() - if chosen { 0.0 } else { 1.0 }, page_top),
            ),
            ppp,
        );
        if r.width() < 4.0 {
            continue;
        }
        if chosen {
            // Cover the page's line beneath it so the tab and page are one.
            p.rect_filled(
                Rect::from_min_max(
                    Pos2::new(r.left() + u, page_top),
                    Pos2::new(r.right() - u, page_top + u),
                ),
                0.0,
                FACE,
            );
        }
        // Left and top in white, a pixel cut from the corners; right in
        // grey then black.
        p.rect_filled(
            Rect::from_min_max(
                Pos2::new(r.left(), r.top() + 2.0 * u),
                Pos2::new(r.left() + u, r.bottom() + if chosen { u } else { 0.0 }),
            ),
            0.0,
            WHITE,
        );
        p.rect_filled(
            Rect::from_min_max(
                Pos2::new(r.left() + u, r.top() + u),
                Pos2::new(r.left() + 2.0 * u, r.top() + 2.0 * u),
            ),
            0.0,
            WHITE,
        );
        p.rect_filled(
            Rect::from_min_max(
                Pos2::new(r.left() + 2.0 * u, r.top()),
                Pos2::new(r.right() - 2.0 * u, r.top() + u),
            ),
            0.0,
            WHITE,
        );
        p.rect_filled(
            Rect::from_min_max(
                Pos2::new(r.right() - 2.0 * u, r.top() + u),
                Pos2::new(r.right() - u, r.bottom()),
            ),
            0.0,
            SHADOW,
        );
        p.rect_filled(
            Rect::from_min_max(
                Pos2::new(r.right() - u, r.top() + 2.0 * u),
                Pos2::new(r.right(), r.bottom()),
            ),
            0.0,
            DARK,
        );
    }
    // The window's border over everything at its edge.
    let mut v = Vec::new();
    edge_shapes(&mut v, rect, Edge::Raised, ppp);
    p.extend(v);
}

/// What one pass over the frame's shapes keeps between shapes.
struct Pass {
    ppp: f32,
    order: Order,
    /// Navy highlights: text over them turns white.
    zones: Vec<Rect>,
}

fn plain(c: Color32) -> Color32 {
    if c == S_FG {
        DARK
    } else if is_sentinel(c) {
        SHADOW
    } else {
        c
    }
}

fn is_sentinel(c: Color32) -> bool {
    let [r, g, b, _] = c.to_array();
    r == 127 && g == 128 && (129..=135).contains(&b)
}

fn group(v: Vec<Shape>) -> Shape {
    Shape::Vec(v)
}

impl Pass {
    fn bevelled(&self, r: Rect, face: Color32, e: Edge) -> Shape {
        let mut v = vec![Shape::rect_filled(snap(r, self.ppp), 0.0, face)];
        edge_shapes(&mut v, r, e, self.ppp);
        group(v)
    }

    fn highlight(&mut self, r: Rect) -> Shape {
        self.zones.push(r);
        Shape::rect_filled(r, 0.0, NAVY)
    }

    /// The replacement for a rectangle egui drew, if it is one of ours.
    fn rect(&mut self, rs: &RectShape) -> Option<Shape> {
        let s = rs.stroke.color;
        let r = rs.rect;
        let popup = matches!(self.order, Order::Foreground);
        if rs.fill == HOVER_FACE {
            if popup {
                return Some(self.highlight(r));
            }
            if !is_sentinel(s) {
                return Some(Shape::rect_filled(r, rs.rounding, FACE));
            }
        }
        let f = if rs.fill == HOVER_FACE { FACE } else { rs.fill };
        let well = f == WHITE;
        Some(match s {
            _ if f == S_FG => {
                // A scroll bar's or slider's thumb.
                self.bevelled(r, FACE, Edge::Button)
            }
            S_RAISED if well => self.bevelled(r, WHITE, Edge::Well),
            S_RAISED if f == NAVY => self.highlight(r),
            S_RAISED => {
                if f == Color32::TRANSPARENT {
                    return Some(Shape::Noop);
                }
                self.bevelled(r, f, Edge::Button)
            }
            // In a menu (or a drop-down list) the item under the pointer
            // is a navy bar.
            S_HOVER | S_OPEN if popup || f == NAVY => self.highlight(r),
            S_HOVER | S_OPEN if well => self.bevelled(r, WHITE, Edge::Well),
            S_HOVER => self.bevelled(r, FACE, Edge::Button),
            S_OPEN => self.bevelled(r, FACE, Edge::ThinSunken),
            S_PRESSED | S_FOCUS if well => self.bevelled(r, WHITE, Edge::Well),
            // The current choice in a menu: a pale sunken bar.
            S_FOCUS if popup => self.bevelled(r, FACE, Edge::ThinSunken),
            S_PRESSED | S_FOCUS => self.bevelled(r, LIGHT, Edge::Pressed),
            S_WINDOW if self.order == Order::Tooltip => group(vec![
                Shape::rect_filled(r, 0.0, TOOLTIP),
                Shape::rect_stroke(r.shrink(0.5), 0.0, Stroke::new(1.0_f32, DARK)),
            ]),
            S_WINDOW => self.bevelled(r, FACE, Edge::Raised),
            S_GROOVE => {
                let mut v = Vec::new();
                if f != Color32::TRANSPARENT {
                    v.push(Shape::rect_filled(r, 0.0, f));
                }
                edge_shapes(&mut v, r, Edge::Groove, self.ppp);
                group(v)
            }
            _ if f == NAVY && rs.stroke.width <= 0.0 => {
                // Selected text or a selected row.
                self.zones.push(r);
                return None;
            }
            _ => return None,
        })
    }

    fn shape(&mut self, shape: &mut Shape) {
        match shape {
            Shape::Vec(v) => {
                for s in v {
                    self.shape(s);
                }
            }
            Shape::Rect(rs) => {
                if let Some(new) = self.rect(rs) {
                    *shape = new;
                } else {
                    rs.fill = plain(rs.fill);
                    rs.stroke.color = plain(rs.stroke.color);
                }
            }
            Shape::Circle(cs) => {
                if is_sentinel(cs.stroke.color) {
                    *shape = self.radio(cs);
                } else {
                    cs.fill = plain(cs.fill);
                    cs.stroke.color = plain(cs.stroke.color);
                }
            }
            Shape::LineSegment { points, stroke } => {
                if let ColorMode::Solid(c) = stroke.color {
                    if c == S_GROOVE {
                        // An etched separator: grey, then white beside it.
                        let u = unit(self.ppp);
                        let d = points[1] - points[0];
                        let off = if d.x.abs() > d.y.abs() {
                            Vec2::new(0.0, u)
                        } else {
                            Vec2::new(u, 0.0)
                        };
                        let a = [points[0], points[1]];
                        *shape = group(vec![
                            Shape::LineSegment {
                                points: a,
                                stroke: PathStroke::new(u, SHADOW),
                            },
                            Shape::LineSegment {
                                points: [a[0] + off, a[1] + off],
                                stroke: PathStroke::new(u, WHITE),
                            },
                        ]);
                    } else {
                        stroke.color = ColorMode::Solid(plain(c));
                    }
                }
            }
            Shape::Path(ps) => {
                ps.fill = plain(ps.fill);
                if let ColorMode::Solid(c) = ps.stroke.color {
                    ps.stroke.color = ColorMode::Solid(plain(c));
                }
            }
            Shape::Text(t) => {
                let r = t.galley.rect.translate(t.pos.to_vec2());
                if self.zones.iter().any(|z| z.contains(r.center())) {
                    t.override_text_color = Some(WHITE);
                }
                if t.fallback_color == S_FG {
                    t.fallback_color = DARK;
                }
            }
            _ => {}
        }
    }

    /// A radio button: a white well, round.
    fn radio(&self, cs: &CircleShape) -> Shape {
        let n = 24;
        let arc = |r: f32, a0: f32, a1: f32| -> Vec<Pos2> {
            (0..=n)
                .map(|i| {
                    let a = a0 + (a1 - a0) * i as f32 / n as f32;
                    cs.center + Vec2::angled(a) * r
                })
                .collect()
        };
        let pi = std::f32::consts::PI;
        let u = unit(self.ppp);
        let r = cs.radius;
        // Upper left dark, lower right light, the dividing line slanting.
        let (a0, a1) = (pi * 0.75, pi * 1.75);
        group(vec![
            Shape::circle_filled(cs.center, r, WHITE),
            Shape::line(arc(r - u * 0.5, a0, a1), Stroke::new(u, SHADOW)),
            Shape::line(arc(r - u * 1.5, a0, a1), Stroke::new(u, DARK)),
            Shape::line(arc(r - u * 0.5, a1, a0 + 2.0 * pi), Stroke::new(u, WHITE)),
            Shape::line(arc(r - u * 1.5, a1, a0 + 2.0 * pi), Stroke::new(u, LIGHT)),
        ])
    }
}

/// Where the tab strips' own shapes (their titles and close crosses) were
/// drawn this frame, so [`restyle`] can move them below the captions.
pub struct TabShift {
    pub layer: egui::LayerId,
    pub from: usize,
    pub to: usize,
    pub bars: Vec<Rect>,
}

/// The index the next shape in `layer` will get.
pub fn next_shape(ctx: &egui::Context, layer: egui::LayerId) -> usize {
    ctx.graphics_mut(|g| g.entry(layer).next_idx().0)
}

/// Swaps egui's flat widget rectangles for bevelled ones, all frame long.
/// Call at the end of the frame, while the theme's chrome is `win95`.
pub fn restyle(ctx: &egui::Context, shift: Option<&TabShift>) {
    let ppp = ctx.pixels_per_point();
    let mut layers: Vec<egui::LayerId> = ctx.memory(|m| m.layer_ids().collect());
    if !layers.contains(&egui::LayerId::background()) {
        layers.insert(0, egui::LayerId::background());
    }
    ctx.graphics_mut(|g| {
        for id in layers {
            let Some(list) = g.get_mut(id) else { continue };
            if let Some(sh) = shift.filter(|s| s.layer == id) {
                move_tab_titles(list, sh);
            }
            let mut pass = Pass {
                ppp,
                order: id.order,
                zones: Vec::new(),
            };
            // A thumb's track (white, drawn just before it) turns light
            // grey and sunken.
            let mut last_white: Option<(usize, Rect)> = None;
            for i in 0..list.next_idx().0 {
                let mut thumb = false;
                let mut white = None;
                list.mutate_shape(ShapeIdx(i), |cs: &mut ClippedShape| {
                    if let Shape::Rect(rs) = &cs.shape {
                        if rs.fill == S_FG {
                            thumb = true;
                        } else if rs.fill == WHITE && rs.stroke.width <= 0.0 {
                            white = Some(rs.rect);
                        }
                    }
                    pass.shape(&mut cs.shape);
                });
                if thumb {
                    if let Some((j, r)) = last_white.take() {
                        if j + 1 == i {
                            track(list, j, r, ppp);
                        }
                    }
                }
                last_white = white.map(|r| (i, r));
            }
        }
    });
}

fn track(list: &mut PaintList, j: usize, r: Rect, ppp: f32) {
    list.mutate_shape(ShapeIdx(j), |cs| {
        let mut v = vec![Shape::rect_filled(snap(r, ppp), 0.0, LIGHT)];
        if r.width().min(r.height()) < 10.0 {
            // A slider's groove: a thin sunken line.
            let c = r.center();
            let g = if r.width() > r.height() {
                Rect::from_center_size(c, Vec2::new(r.width(), 4.0))
            } else {
                Rect::from_center_size(c, Vec2::new(4.0, r.height()))
            };
            v = Vec::new();
            edge_shapes(&mut v, g, Edge::Well, ppp);
        }
        cs.shape = Shape::Vec(v);
    });
}

fn move_tab_titles(list: &mut PaintList, sh: &TabShift) {
    let dy = (CAPTION / 2.0).round() + 1.0;
    let to = sh.to.min(list.next_idx().0);
    for i in sh.from..to {
        list.mutate_shape(ShapeIdx(i), |cs| {
            let r = cs.shape.visual_bounding_rect();
            if !r.is_finite() || !sh.bars.iter().any(|b| b.contains_rect(r)) {
                return;
            }
            if matches!(cs.shape, Shape::Text(_) | Shape::LineSegment { .. }) {
                cs.shape.translate(Vec2::new(0.0, dy));
            }
        });
    }
}

/// The egui look for this chrome: every outline one of ours, square
/// corners, no shadows, solid scroll bars.
pub fn visuals(th: &Theme) -> egui::Visuals {
    let mut v = egui::Visuals::light();
    let one = |c: Color32| Stroke::new(1.0_f32, c);
    v.override_text_color = Some(crate::theme::c(th.text));
    v.panel_fill = FACE;
    v.window_fill = FACE;
    v.extreme_bg_color = WHITE;
    v.faint_bg_color = LIGHT;
    v.code_bg_color = WHITE;
    v.hyperlink_color = Color32::from_rgb(0, 0, 255);
    v.selection.bg_fill = NAVY;
    v.selection.stroke = one(S_FOCUS);
    v.window_rounding = Rounding::ZERO;
    v.menu_rounding = Rounding::ZERO;
    v.window_stroke = one(S_WINDOW);
    v.window_shadow = egui::epaint::Shadow::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;
    v.window_highlight_topmost = false;
    v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.55 };
    v.slider_trailing_fill = false;
    v.collapsing_header_frame = false;
    v.text_cursor.stroke = one(DARK);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = FACE;
    w.noninteractive.weak_bg_fill = FACE;
    w.noninteractive.bg_stroke = one(S_GROOVE);
    w.noninteractive.fg_stroke = one(DARK);
    for (s, edge) in [
        (&mut w.inactive, S_RAISED),
        (&mut w.hovered, S_HOVER),
        (&mut w.active, S_PRESSED),
        (&mut w.open, S_OPEN),
    ] {
        s.bg_fill = WHITE;
        s.weak_bg_fill = if edge == S_HOVER { HOVER_FACE } else { FACE };
        s.bg_stroke = one(edge);
        s.fg_stroke = one(S_FG);
    }
    for s in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        s.rounding = Rounding::ZERO;
        s.expansion = 0.0;
    }
    v
}

/// Solid scroll bars, wide enough for a thumb with edges.
pub fn scroll_style() -> egui::style::ScrollStyle {
    egui::style::ScrollStyle {
        bar_width: 14.0,
        handle_min_length: 16.0,
        bar_inner_margin: 2.0,
        foreground_color: true,
        ..egui::style::ScrollStyle::solid()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentinels_are_distinct_and_known() {
        let all = [
            S_RAISED, S_HOVER, S_PRESSED, S_OPEN, S_FOCUS, S_WINDOW, S_GROOVE,
        ];
        for (i, a) in all.iter().enumerate() {
            assert!(is_sentinel(*a));
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert!(!is_sentinel(SHADOW));
        assert_eq!(plain(S_FG), DARK);
    }

    #[test]
    fn edges_have_four_lines_and_stay_inside() {
        let r = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(40.0, 20.0));
        let mut v = Vec::new();
        edge_shapes(&mut v, r, Edge::Button, 1.0);
        assert_eq!(v.len(), 8);
        for s in &v {
            assert!(r.contains_rect(s.visual_bounding_rect()));
        }
        v.clear();
        edge_shapes(&mut v, r, Edge::ThinSunken, 1.0);
        assert_eq!(v.len(), 4);
    }

    #[test]
    fn rectangles_become_bevels_and_text_on_navy_turns_white() {
        let mut pass = Pass {
            ppp: 1.0,
            order: Order::Foreground,
            zones: Vec::new(),
        };
        let r = Rect::from_min_size(Pos2::ZERO, Vec2::new(50.0, 20.0));
        let mut s = Shape::Rect(RectShape::new(r, 0.0, FACE, one_px(S_RAISED)));
        pass.shape(&mut s);
        assert!(matches!(s, Shape::Vec(_)));
        // A hovered menu item in a popup is a navy highlight.
        let mut h = Shape::Rect(RectShape::new(r, 0.0, FACE, one_px(S_HOVER)));
        pass.shape(&mut h);
        assert_eq!(pass.zones.len(), 1);
    }

    fn one_px(c: Color32) -> Stroke {
        Stroke::new(1.0_f32, c)
    }
}
