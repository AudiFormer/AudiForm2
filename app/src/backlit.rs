// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The `backlit` chrome (the Analog theme's buttons): the illuminated
//! plastic push-buttons of 1970s studio gear. Each is a translucent,
//! smoky cap in a dark bezel with a gloss across its top; off, it shows a
//! faint warm glow; on (or held down), it lights from behind in its colour
//! and casts a little light around it. Original drawing, generic to the
//! era.

use egui::{pos2, vec2, Color32, Mesh, Painter, Pos2, Rect, Shape, Stroke};

use crate::theme::{ca, Rgb, Theme};

/// The chrome is on for this theme.
pub fn on(th: &Theme) -> bool {
    th.chrome == "backlit"
}

/// Scales a colour's brightness.
fn shade(rgb: Rgb, k: f32) -> Color32 {
    let f = |v: u8| (v as f32 * k).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(f(rgb[0]), f(rgb[1]), f(rgb[2]))
}

/// A vertical gradient over `r`.
fn gradient(p: &Painter, r: Rect, top: Color32, bottom: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(r.left_top(), top);
    m.colored_vertex(r.right_top(), top);
    m.colored_vertex(r.right_bottom(), bottom);
    m.colored_vertex(r.left_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(Shape::mesh(m));
}

/// One button cap in `rect`. `lit` is the colour it glows (None: off);
/// `hover` 0 to 1 warms an unlit cap; `down` sinks it. Returns where its
/// legend (icon or text) goes and the legend's colour.
pub fn cap(p: &Painter, rect: Rect, lit: Option<Rgb>, hover: f32, down: bool, tint: Rgb) -> (Pos2, Color32) {
    let push = if down { vec2(0.0, 1.0) } else { vec2(0.0, 0.0) };
    // The light it throws around itself.
    if let Some(col) = lit {
        for (grow, a) in [(6.0, 18u8), (3.5, 34), (1.5, 60)] {
            p.rect_filled(rect.expand(grow), 4.0 + grow, ca(col, a));
        }
    }
    // The bezel the cap sits in.
    p.rect_filled(rect, 3.0, Color32::from_rgb(10, 9, 8));
    let capr = rect.shrink(2.0).translate(push);
    match lit {
        Some(col) => {
            // Lit from behind: bright in the middle, deeper at the rim.
            gradient(p, capr, shade(col, 1.08), shade(col, 0.78));
            let core = capr.shrink2(vec2(capr.width() * 0.18, capr.height() * 0.22));
            p.rect_filled(core, 3.0, Color32::from_rgba_unmultiplied(255, 246, 220, 70));
        }
        None => {
            // Smoky plastic, the lamp behind it off (or barely warm).
            let k = 0.20 + 0.10 * hover;
            gradient(p, capr, shade(tint, k + 0.06), shade(tint, k - 0.04));
        }
    }
    // The gloss across the top of the plastic, and its moulded edge.
    let gloss = Rect::from_min_max(capr.min + vec2(1.0, 1.0), pos2(capr.max.x - 1.0, capr.top() + capr.height() * 0.42));
    gradient(p, gloss, Color32::from_white_alpha(if lit.is_some() { 70 } else { 34 }), Color32::from_white_alpha(4));
    p.rect_stroke(capr, 2.0, Stroke::new(1.0_f32, Color32::from_black_alpha(140)));
    let legend = match lit {
        // The legend lit through: darker than the glowing cap.
        Some(col) => shade(col, 0.32),
        None => shade(tint, 0.8 + 0.2 * hover),
    };
    (capr.center(), legend)
}

/// Egui's own buttons in the same plastic: a smoky cap, warmer on hover,
/// lit amber while pressed or selected.
pub fn visuals(th: &Theme, v: &mut egui::Visuals) {
    let tint = th.accent;
    let w = &mut v.widgets;
    let bezel = Stroke::new(1.0_f32, Color32::from_rgb(10, 9, 8));
    w.inactive.bg_fill = shade(tint, 0.22);
    w.inactive.weak_bg_fill = shade(tint, 0.22);
    w.inactive.bg_stroke = bezel;
    w.hovered.bg_fill = shade(tint, 0.34);
    w.hovered.weak_bg_fill = shade(tint, 0.34);
    w.hovered.bg_stroke = Stroke::new(1.0_f32, shade(tint, 0.6));
    w.active.bg_fill = shade(tint, 1.0);
    w.active.weak_bg_fill = shade(tint, 1.0);
    w.active.bg_stroke = bezel;
    w.active.fg_stroke = Stroke::new(2.0_f32, shade(tint, 0.3));
    w.open.bg_fill = shade(tint, 0.5);
    w.open.weak_bg_fill = shade(tint, 0.5);
    // A selected toggle is a lit button.
    v.selection.bg_fill = shade(tint, 0.85);
    v.selection.stroke = Stroke::new(1.0_f32, shade(tint, 0.25));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analog_has_backlit_buttons_and_others_do_not() {
        assert!(on(&Theme::analog()));
        assert!(!on(&Theme::studio_lite()));
        assert!(!on(&Theme::zero_point()));
        // A lit legend is darker than its cap; an unlit one lighter.
        let a = Theme::analog().accent;
        assert!(shade(a, 0.32).r() < shade(a, 1.0).r());
        assert!(shade(a, 0.8).r() > shade(a, 0.22).r());
    }
}
