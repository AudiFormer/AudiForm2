// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Shared drawing for the small graphs in effect and generator windows:
//! a recessed plot area, a log (or linear) frequency axis with its grid
//! and labels, a value axis, and a filled or stroked curve. Everything is
//! drawn from the theme.

use egui::{pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Rounding, Shape, Stroke};

use crate::theme::{c, ca, lerp, Theme};

/// A plot: its area on screen and what its edges stand for.
#[derive(Clone, Copy)]
pub struct Plot {
    pub rect: Rect,
    pub f_lo: f64,
    pub f_hi: f64,
    pub linear: bool,
    pub y_lo: f64,
    pub y_hi: f64,
}

impl Plot {
    pub fn x(&self, hz: f64) -> f32 {
        let t = if self.linear {
            (hz - self.f_lo) / (self.f_hi - self.f_lo)
        } else {
            (hz.max(1e-9) / self.f_lo).ln() / (self.f_hi / self.f_lo).ln()
        };
        self.rect.left() + self.rect.width() * t.clamp(0.0, 1.0) as f32
    }
    pub fn hz(&self, x: f32) -> f64 {
        let t = ((x - self.rect.left()) / self.rect.width()).clamp(0.0, 1.0) as f64;
        if self.linear {
            self.f_lo + t * (self.f_hi - self.f_lo)
        } else {
            self.f_lo * (self.f_hi / self.f_lo).powf(t)
        }
    }
    pub fn y(&self, v: f64) -> f32 {
        let t = (v - self.y_lo) / (self.y_hi - self.y_lo);
        self.rect.bottom() - self.rect.height() * t.clamp(0.0, 1.0) as f32
    }
    pub fn value(&self, y: f32) -> f64 {
        let t = ((self.rect.bottom() - y) / self.rect.height()).clamp(0.0, 1.0) as f64;
        self.y_lo + t * (self.y_hi - self.y_lo)
    }
}

/// The recessed background of a plot.
pub fn backdrop(p: &Painter, rect: Rect, th: &Theme) {
    let square = th.bevel.is_some();
    let r = Rounding::same(if square { 0.0 } else { 3.0 });
    p.rect_filled(rect, r, c(th.wave_background));
    if square {
        crate::skin::bevel(p, rect.expand(2.0), th, true);
    } else {
        p.rect_stroke(rect, r, Stroke::new(1.0_f32, ca(th.border, 230)));
    }
}

/// The frequencies a log axis labels.
const LOG_TICKS: [f64; 10] = [
    20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0, 20_000.0,
];

fn hz_label(f: f64) -> String {
    if f >= 1000.0 {
        format!("{}k", f / 1000.0)
    } else {
        format!("{f}")
    }
}

/// The frequency grid with labels under the plot, and the value grid at
/// `y_ticks` with labels at the right (`unit` after each).
pub fn grid(p: &Painter, plot: &Plot, th: &Theme, y_ticks: &[f64], unit: &str) {
    let r = plot.rect;
    let faint = Stroke::new(1.0_f32, ca(th.grid, 70));
    let strong = Stroke::new(1.0_f32, ca(th.grid, 140));
    let font = FontId::proportional(9.0);
    let ticks: Vec<f64> = if plot.linear {
        let step = ((plot.f_hi - plot.f_lo) / 10.0).max(1.0);
        (0..=10).map(|k| plot.f_lo + k as f64 * step).collect()
    } else {
        LOG_TICKS
            .iter()
            .copied()
            .filter(|f| *f >= plot.f_lo && *f <= plot.f_hi)
            .collect()
    };
    for f in ticks {
        let x = plot.x(f);
        let major = [100.0, 1000.0, 10_000.0].contains(&f);
        p.line_segment(
            [pos2(x, r.top()), pos2(x, r.bottom())],
            if major { strong } else { faint },
        );
        let align = if x <= r.left() + 4.0 {
            Align2::LEFT_TOP
        } else if x >= r.right() - 4.0 {
            Align2::RIGHT_TOP
        } else {
            Align2::CENTER_TOP
        };
        p.text(
            pos2(x, r.bottom() + 2.0),
            align,
            hz_label(f),
            font.clone(),
            c(th.text_dim),
        );
    }
    for v in y_ticks {
        let y = plot.y(*v);
        p.line_segment(
            [pos2(r.left(), y), pos2(r.right(), y)],
            if v.abs() < 1e-9 { strong } else { faint },
        );
        p.text(
            pos2(r.right() + 3.0, y),
            Align2::LEFT_CENTER,
            format!("{v}{unit}"),
            font.clone(),
            c(th.text_dim),
        );
    }
}

/// A curve through `pts`, filled down to the bottom of the plot with the
/// theme's light fading out (`fill`), and stroked on top.
pub fn curve(p: &Painter, plot: &Plot, pts: &[Pos2], th: &Theme, fill: bool) {
    if pts.len() < 2 {
        return;
    }
    if fill {
        let mut m = egui::Mesh::default();
        let base = plot.rect.bottom();
        let top = c(th.accent).gamma_multiply(0.45);
        let bottom = c(th.accent).gamma_multiply(0.06);
        for (k, q) in pts.iter().enumerate() {
            m.colored_vertex(*q, top);
            m.colored_vertex(pos2(q.x, base), bottom);
            if k > 0 {
                let i = (k * 2) as u32;
                m.add_triangle(i - 2, i - 1, i);
                m.add_triangle(i - 1, i + 1, i);
            }
        }
        p.add(Shape::mesh(m));
    }
    if th.dark && th.glow > 0.0 {
        p.add(Shape::line(
            pts.to_vec(),
            Stroke::new(5.0_f32, ca(th.accent, (35.0 * th.glow) as u8)),
        ));
    }
    p.add(Shape::line(
        pts.to_vec(),
        Stroke::new(2.0_f32, c(th.accent)),
    ));
}

/// A small square handle (a graph point), lit when `hot`.
pub fn handle(p: &Painter, at: Pos2, th: &Theme, hot: bool, label: Option<&str>) {
    let r = Rect::from_center_size(at, vec2(9.0, 9.0));
    p.rect_filled(
        r,
        Rounding::same(1.5),
        if hot {
            c(th.accent)
        } else {
            lerp(th.panel_raised, th.text, 0.6)
        },
    );
    p.rect_stroke(
        r,
        Rounding::same(1.5),
        Stroke::new(1.0_f32, Color32::from_black_alpha(160)),
    );
    if let Some(t) = label {
        p.text(
            at + vec2(0.0, -9.0),
            Align2::CENTER_BOTTOM,
            t,
            FontId::proportional(9.0),
            c(th.text),
        );
    }
}

/// A short readout in a dark tag (for the point under the pointer).
pub fn tag(p: &Painter, at: Pos2, text: &str, th: &Theme, bounds: Rect) {
    let g = p.layout_no_wrap(text.to_string(), FontId::proportional(10.5), c(th.text));
    let size = g.size() + vec2(10.0, 4.0);
    let mut r = Rect::from_min_size(at + vec2(8.0, -size.y - 6.0), size);
    if r.right() > bounds.right() {
        r = r.translate(vec2(-(size.x + 16.0), 0.0));
    }
    if r.top() < bounds.top() {
        r = r.translate(vec2(0.0, size.y + 12.0));
    }
    p.rect_filled(r, Rounding::same(2.0), ca(th.background, 230));
    p.rect_stroke(
        r,
        Rounding::same(2.0),
        Stroke::new(1.0_f32, ca(th.accent, 160)),
    );
    p.galley(r.min + vec2(5.0, 2.0), g, c(th.text));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plot_axes_round_trip() {
        let plot = Plot {
            rect: Rect::from_min_size(pos2(10.0, 10.0), vec2(400.0, 200.0)),
            f_lo: 20.0,
            f_hi: 20_000.0,
            linear: false,
            y_lo: -30.0,
            y_hi: 30.0,
        };
        for f in [20.0, 100.0, 632.0, 5000.0, 20_000.0] {
            assert!((plot.hz(plot.x(f)) / f - 1.0).abs() < 1e-3, "{f}");
        }
        assert!((plot.x(632.45) - 210.0).abs() < 0.5);
        assert!((plot.value(plot.y(12.0)) - 12.0).abs() < 1e-3);
    }
}
