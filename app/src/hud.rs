// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The HUD layer: everything that makes Audiform27 look like an instrument
//! panel from the future rather than a form. A living backdrop (a moving
//! grid with drifting light behind glass panels), chamfered glass cards with
//! corner brackets and a light that runs around the focused one, a laser
//! playhead, vector-drawn transport controls, a glowing time readout and a
//! boot sequence. All of it reads the [`Theme`], reacts to the music through
//! [`Fx`], and stops moving when the theme's `motion` is 0.
//!
//! Glow is drawn additively: in egui's premultiplied colours, a colour with
//! alpha 0 adds light instead of covering what is beneath.

use egui::epaint::Mesh;
use egui::{
    Align2, Color32, FontId, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2,
};

use crate::theme::{ca, Rgb, Theme};

/// Per-frame motion and music state for the chrome.
#[derive(Debug, Clone, Default)]
pub struct Fx {
    /// Seconds of animation (scaled by the theme's `motion`).
    pub t: f32,
    pub dt: f32,
    /// Smoothed output level, 0..1.
    pub level: f32,
    /// Jumps to 1 on a transient in the output and decays: a beat flash.
    pub pulse: f32,
    avg: f32,
    since: f32,
    /// Seconds since the app started (real time, for the boot sequence).
    pub clock: f32,
    pub pointer: Option<Pos2>,
}

impl Fx {
    /// Advances one frame. `peak` is the louder side of the output meter.
    pub fn update(&mut self, ctx: &egui::Context, th: &Theme, peak: f32) {
        let now = ctx.input(|i| i.time) as f32;
        let dt = (now - self.clock).clamp(0.0, 0.1);
        self.clock = now;
        self.dt = dt;
        self.t += dt * th.motion.max(0.0);
        let target = (peak.max(1e-6).log10() * 20.0 + 48.0).clamp(0.0, 48.0) / 48.0 * th.reactive;
        let k = if target > self.level { 0.5 } else { (dt * 3.0).min(1.0) };
        self.level += (target - self.level) * k;
        self.avg += (self.level - self.avg) * (dt * 1.5).min(1.0);
        self.since += dt;
        if self.level > 0.25 && self.level > self.avg * 1.25 + 0.04 && self.since > 0.18 {
            self.pulse = 1.0;
            self.since = 0.0;
        }
        self.pulse = (self.pulse - dt * 2.5).max(0.0);
        self.pointer = ctx.input(|i| i.pointer.hover_pos());
    }

    /// True while something on screen is moving on its own.
    pub fn animating(&self, th: &Theme) -> bool {
        th.motion > 0.0
    }
}

/// Additive light: `rgb` scaled by `k` (0..1), adding to what is beneath.
pub fn light(rgb: Rgb, k: f32) -> Color32 {
    let k = k.clamp(0.0, 1.0);
    Color32::from_rgba_premultiplied(
        (rgb[0] as f32 * k) as u8,
        (rgb[1] as f32 * k) as u8,
        (rgb[2] as f32 * k) as u8,
        0,
    )
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

/// The corners of `r` with the top-right and bottom-left corners cut by
/// `cut`: the angled card the whole UI is built from.
pub fn chamfer(r: Rect, cut: f32) -> Vec<Pos2> {
    let c = cut.min(r.width() * 0.25).min(r.height() * 0.25).max(0.0);
    if c < 0.5 {
        return vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
    }
    vec![
        r.left_top(),
        Pos2::new(r.right() - c, r.top()),
        Pos2::new(r.right(), r.top() + c),
        r.right_bottom(),
        Pos2::new(r.left() + c, r.bottom()),
        Pos2::new(r.left(), r.bottom() - c),
    ]
}

/// A convex polygon filled with a vertical gradient.
fn gradient_poly(p: &Painter, pts: &[Pos2], top: Color32, bottom: Color32) {
    if pts.len() < 3 {
        return;
    }
    let (y0, y1) = pts.iter().fold((f32::MAX, f32::MIN), |(a, b), q| (a.min(q.y), b.max(q.y)));
    let h = (y1 - y0).max(1.0);
    let lerp = |y: f32| {
        let t = ((y - y0) / h).clamp(0.0, 1.0);
        let f = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
        let (a, b) = (top.to_array(), bottom.to_array());
        Color32::from_rgba_premultiplied(f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]), f(a[3], b[3]))
    };
    let mut m = Mesh::default();
    for q in pts {
        m.colored_vertex(*q, lerp(q.y));
    }
    for i in 1..pts.len() as u32 - 1 {
        m.add_triangle(0, i, i + 1);
    }
    p.add(Shape::mesh(m));
}

/// A soft round light (additive), brightest in the middle.
fn blob(p: &Painter, centre: Pos2, radius: f32, rgb: Rgb, k: f32) {
    if k <= 0.002 || radius < 1.0 {
        return;
    }
    let mut m = Mesh::default();
    m.colored_vertex(centre, light(rgb, k));
    let n = 40u32;
    for i in 0..n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        m.colored_vertex(centre + Vec2::angled(a) * radius, Color32::TRANSPARENT);
    }
    for i in 0..n {
        m.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    p.add(Shape::mesh(m));
}

/// The living background behind every panel.
pub fn backdrop(p: &Painter, rect: Rect, th: &Theme, fx: &Fx) {
    // Deep gradient.
    let mut m = Mesh::default();
    let (top, bottom) = (crate::theme::c(th.backdrop_top), crate::theme::c(th.backdrop_bottom));
    m.colored_vertex(rect.left_top(), top);
    m.colored_vertex(rect.right_top(), top);
    m.colored_vertex(rect.right_bottom(), bottom);
    m.colored_vertex(rect.left_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(Shape::mesh(m));

    // Drifting aurora light, swelling with the music.
    let t = fx.t;
    let w = rect.width();
    let h = rect.height();
    let g = th.glow.max(0.0);
    let swell = 1.0 + fx.level * 0.8 + fx.pulse * 0.6;
    blob(
        p,
        Pos2::new(rect.left() + w * (0.25 + 0.08 * (t * 0.07).sin()), rect.top() + h * 0.3),
        w.max(h) * 0.45,
        th.accent,
        0.07 * g * swell,
    );
    blob(
        p,
        Pos2::new(rect.left() + w * (0.78 + 0.06 * (t * 0.05 + 2.0).cos()), rect.top() + h * 0.75),
        w.max(h) * 0.4,
        th.accent_alt,
        0.06 * g * swell,
    );

    match th.grid_style.as_str() {
        "perspective" => perspective_grid(p, rect, th, fx),
        "hex" => hex_grid(p, rect, th, fx),
        "dots" => dot_grid(p, rect, th, fx),
        "lines" => line_grid(p, rect, th, fx),
        _ => {}
    }

    // Vignette: the edges fall away into the dark.
    let dark = Color32::from_black_alpha(150);
    let edge = (w.min(h) * 0.18).max(40.0);
    let side = |a: Pos2, b: Pos2, c: Pos2, d: Pos2| {
        let mut m = Mesh::default();
        m.colored_vertex(a, dark);
        m.colored_vertex(b, dark);
        m.colored_vertex(c, Color32::TRANSPARENT);
        m.colored_vertex(d, Color32::TRANSPARENT);
        m.add_triangle(0, 1, 2);
        m.add_triangle(0, 2, 3);
        p.add(Shape::mesh(m));
    };
    let r = rect;
    side(r.left_top(), r.right_top(), r.right_top() + Vec2::new(0.0, edge), r.left_top() + Vec2::new(0.0, edge));
    side(r.left_bottom(), r.right_bottom(), r.right_bottom() - Vec2::new(0.0, edge), r.left_bottom() - Vec2::new(0.0, edge));
    side(r.left_top(), r.left_bottom(), r.left_bottom() + Vec2::new(edge, 0.0), r.left_top() + Vec2::new(edge, 0.0));
    side(r.right_top(), r.right_bottom(), r.right_bottom() - Vec2::new(edge, 0.0), r.right_top() - Vec2::new(edge, 0.0));
}

/// Extra light near the pointer, so the grid wakes up where you look.
fn near_pointer(fx: &Fx, q: Pos2) -> f32 {
    fx.pointer.map_or(0.0, |m| {
        let d = m.distance(q);
        (1.0 - d / 180.0).max(0.0).powi(2)
    })
}

fn perspective_grid(p: &Painter, rect: Rect, th: &Theme, fx: &Fx) {
    let horizon = rect.top() + rect.height() * 0.46;
    let cx = rect.center().x;
    let col = th.grid_glow;
    let k = 0.22 * th.grid_strength;
    // The floor and the ceiling: lines run to a vanishing point.
    for (y_far, y_near, dir) in [(horizon, rect.bottom(), 1.0f32), (horizon, rect.top(), -1.0)] {
        let depth = (y_near - y_far).abs();
        let n = 22;
        let spread = rect.width() * 1.6;
        for i in -n..=n {
            let f = i as f32 / n as f32;
            let a = Pos2::new(cx + f * rect.width() * 0.08, y_far);
            let b = Pos2::new(cx + f * spread, y_near);
            let s = k * (0.55 + 0.45 * (1.0 - f.abs())) * if dir < 0.0 { 0.5 } else { 1.0 };
            p.line_segment([a, b], Stroke::new(1.0_f32, light(col, s)));
        }
        // Cross lines rush towards you, faster when it is loud.
        let rows = 16;
        let phase = (fx.t * (0.25 + fx.level * 0.6)).fract();
        for r in 0..rows {
            let z = (r as f32 + phase) / rows as f32;
            let d = z * z;
            let y = y_far + dir * depth * d;
            let s = k * d.sqrt() * if dir < 0.0 { 0.5 } else { 1.0 };
            let boost = near_pointer(fx, Pos2::new(fx.pointer.map_or(cx, |m| m.x), y)) * 0.5;
            p.line_segment(
                [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                Stroke::new(1.0_f32, light(col, s + boost * k)),
            );
        }
    }
    // The horizon burns, and flares on a beat.
    let flare = 0.35 + fx.level * 0.4 + fx.pulse * 0.5;
    for (wdt, a) in [(9.0_f32, 0.06_f32), (4.0, 0.14), (1.2, 0.5)] {
        p.line_segment(
            [Pos2::new(rect.left(), horizon), Pos2::new(rect.right(), horizon)],
            Stroke::new(wdt, light(th.accent, a * flare * th.grid_strength)),
        );
    }
}

fn hex_grid(p: &Painter, rect: Rect, th: &Theme, fx: &Fx) {
    let size = 26.0;
    let w = size * 3f32.sqrt();
    let hstep = size * 1.5;
    let drift = (fx.t * 6.0) % hstep;
    let base = 0.07 * th.grid_strength;
    let cols = (rect.width() / w) as i32 + 3;
    let rows = (rect.height() / hstep) as i32 + 3;
    for r in -1..rows {
        for q in -1..cols {
            let cx = rect.left() + q as f32 * w + if r % 2 == 0 { 0.0 } else { w / 2.0 };
            let cy = rect.top() + r as f32 * hstep - drift;
            let c = Pos2::new(cx, cy);
            let wave = 0.5 + 0.5 * ((cx * 0.004 + cy * 0.006) - fx.t * 0.8).sin();
            let s = base * (0.4 + 0.6 * wave) + near_pointer(fx, c) * 0.35 + fx.pulse * 0.05;
            if s < 0.01 {
                continue;
            }
            let pts: Vec<Pos2> = (0..6)
                .map(|i| {
                    let a = std::f32::consts::FRAC_PI_6 + i as f32 * std::f32::consts::FRAC_PI_3;
                    c + Vec2::angled(a) * (size - 2.0)
                })
                .collect();
            p.add(Shape::closed_line(pts, Stroke::new(1.0_f32, light(th.grid_glow, s))));
        }
    }
}

fn dot_grid(p: &Painter, rect: Rect, th: &Theme, fx: &Fx) {
    let step = 22.0;
    let base = 0.25 * th.grid_strength;
    let mut y = rect.top() + (fx.t * 4.0) % step;
    while y < rect.bottom() {
        let mut x = rect.left() + 6.0;
        while x < rect.right() {
            let q = Pos2::new(x, y);
            let wave = 0.5 + 0.5 * (x * 0.01 - fx.t * 1.3 + (y * 0.013).sin() * 2.0).sin();
            let s = base * (0.25 + 0.75 * wave.powi(3)) + near_pointer(fx, q) * 0.6;
            if s > 0.02 {
                p.circle_filled(q, 1.1, light(th.grid_glow, s));
            }
            x += step;
        }
        y += step;
    }
}

fn line_grid(p: &Painter, rect: Rect, th: &Theme, fx: &Fx) {
    let step = 32.0;
    let s = 0.1 * th.grid_strength;
    let off = (fx.t * 8.0) % step;
    let mut x = rect.left() - off;
    while x < rect.right() {
        p.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], Stroke::new(1.0_f32, light(th.grid_glow, s)));
        x += step;
    }
    let mut y = rect.top() - off;
    while y < rect.bottom() {
        p.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], Stroke::new(1.0_f32, light(th.grid_glow, s)));
        y += step;
    }
}

/// CRT scan lines and a slow refresh band, over everything.
pub fn scanlines(p: &Painter, rect: Rect, th: &Theme, fx: &Fx) {
    if th.scanlines <= 0.0 {
        return;
    }
    let a = (th.scanlines * 70.0) as u8;
    let mut y = rect.top();
    while y < rect.bottom() {
        p.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], Stroke::new(1.0_f32, Color32::from_black_alpha(a)));
        y += 3.0;
    }
    // A band of light rolls down the screen every few seconds.
    let period = 7.0;
    let band = ((fx.t / period).fract()) * (rect.height() + 200.0) - 100.0 + rect.top();
    let mut m = Mesh::default();
    let glow = light(th.accent, 0.05 * th.scanlines * 4.0);
    m.colored_vertex(Pos2::new(rect.left(), band - 60.0), Color32::TRANSPARENT);
    m.colored_vertex(Pos2::new(rect.right(), band - 60.0), Color32::TRANSPARENT);
    m.colored_vertex(Pos2::new(rect.right(), band), glow);
    m.colored_vertex(Pos2::new(rect.left(), band), glow);
    m.colored_vertex(Pos2::new(rect.right(), band + 4.0), Color32::TRANSPARENT);
    m.colored_vertex(Pos2::new(rect.left(), band + 4.0), Color32::TRANSPARENT);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    m.add_triangle(3, 2, 4);
    m.add_triangle(3, 4, 5);
    p.add(Shape::mesh(m));
}

/// The glass of a card, drawn beneath its contents.
pub fn glass(p: &Painter, rect: Rect, th: &Theme) {
    let pts = chamfer(rect, th.chamfer);
    let a = (th.glass.clamp(0.0, 1.0) * 255.0) as u8;
    gradient_poly(
        p,
        &pts,
        ca(mix(th.panel, th.panel_raised, 0.55), a),
        ca(th.panel, a),
    );
}

/// The card's edge, brackets and running light, drawn over its contents.
pub fn frame(p: &Painter, rect: Rect, th: &Theme, fx: &Fx, focused: bool, serial: usize) {
    let pts = chamfer(rect, th.chamfer);
    let edge = if focused { ca(th.accent, 150) } else { ca(th.border, 200) };
    p.add(Shape::closed_line(pts.clone(), Stroke::new(1.0_f32, edge)));
    if focused && th.glow > 0.0 {
        p.add(Shape::closed_line(pts.clone(), Stroke::new(4.0_f32, light(th.accent, 0.12 * th.glow))));
    }
    if th.brackets {
        let len = 12.0f32.min(rect.width() * 0.2);
        let col = if focused { crate::theme::c(th.accent) } else { ca(th.accent, 110) };
        let s = Stroke::new(2.0_f32, col);
        let tl = rect.left_top() + Vec2::new(-1.5, -1.5);
        p.add(Shape::line(vec![tl + Vec2::new(0.0, len), tl, tl + Vec2::new(len, 0.0)], s));
        let br = rect.right_bottom() + Vec2::new(1.5, 1.5);
        p.add(Shape::line(vec![br - Vec2::new(0.0, len), br, br - Vec2::new(len, 0.0)], s));
        // A serial number etched in the corner, because why not.
        if rect.width() > 160.0 && rect.height() > 80.0 {
            p.text(
                rect.right_bottom() - Vec2::new(8.0, 4.0),
                Align2::RIGHT_BOTTOM,
                format!("AF2-{serial:02}"),
                FontId::monospace(8.0),
                ca(th.text_dim, 70),
            );
        }
    }
    if focused && th.motion > 0.0 {
        running_light(p, &pts, th, fx);
    }
}

/// A short bright segment that travels around a closed outline.
fn running_light(p: &Painter, pts: &[Pos2], th: &Theme, fx: &Fx) {
    let n = pts.len();
    let lens: Vec<f32> = (0..n).map(|i| pts[i].distance(pts[(i + 1) % n])).collect();
    let total: f32 = lens.iter().sum();
    if total < 10.0 {
        return;
    }
    let seg = 90.0f32.min(total * 0.2);
    let head = (fx.t * 0.22).fract() * total;
    let at = |d: f32| {
        let mut d = d.rem_euclid(total);
        for i in 0..n {
            if d <= lens[i] {
                return pts[i] + (pts[(i + 1) % n] - pts[i]) * (d / lens[i].max(1e-3));
            }
            d -= lens[i];
        }
        pts[0]
    };
    let steps = 14;
    for k in 0..steps {
        let d0 = head - seg * k as f32 / steps as f32;
        let d1 = head - seg * (k + 1) as f32 / steps as f32;
        let fade = 1.0 - k as f32 / steps as f32;
        p.line_segment([at(d0), at(d1)], Stroke::new(2.0_f32, light(th.accent, 0.9 * fade)));
        p.line_segment([at(d0), at(d1)], Stroke::new(6.0_f32, light(th.accent, 0.15 * fade * th.glow)));
    }
}

/// Light under the active tab of a card.
pub fn tab_light(p: &Painter, rect: Rect, th: &Theme, fx: &Fx, focused: bool) {
    let y = rect.bottom() - 1.0;
    let k = if focused { 1.0 } else { 0.55 };
    let breathe = 0.85 + 0.15 * (fx.t * 2.0).sin() + fx.pulse * 0.3;
    p.line_segment(
        [Pos2::new(rect.left() + 2.0, y), Pos2::new(rect.right() - 2.0, y)],
        Stroke::new(2.0_f32, light(th.accent, k * breathe)),
    );
    if th.glow > 0.0 {
        // A glow rising from the underline.
        let mut m = Mesh::default();
        let h = rect.height() * 0.9;
        let col = light(th.accent, 0.16 * k * th.glow * breathe);
        m.colored_vertex(Pos2::new(rect.left(), y), col);
        m.colored_vertex(Pos2::new(rect.right(), y), col);
        m.colored_vertex(Pos2::new(rect.right(), y - h), Color32::TRANSPARENT);
        m.colored_vertex(Pos2::new(rect.left(), y - h), Color32::TRANSPARENT);
        m.add_triangle(0, 1, 2);
        m.add_triangle(0, 2, 3);
        p.add(Shape::mesh(m));
    }
}

/// The playhead: a laser with a hot core, a wide bloom and a sparkle that
/// runs along it.
pub fn laser(p: &Painter, x: f32, top: f32, bottom: f32, th: &Theme, fx: &Fx) {
    let col = th.playhead;
    let g = th.glow.max(0.0);
    if g > 0.0 {
        for (w, k) in [(14.0_f32, 0.05_f32), (7.0, 0.12), (3.0, 0.35)] {
            p.line_segment(
                [Pos2::new(x, top), Pos2::new(x, bottom)],
                Stroke::new(w, light(col, k * g * (1.0 + fx.pulse * 0.5))),
            );
        }
    }
    p.line_segment(
        [Pos2::new(x, top), Pos2::new(x, bottom)],
        Stroke::new(1.0_f32, crate::theme::lerp(col, [255, 255, 255], 0.6)),
    );
    if th.motion > 0.0 && g > 0.0 {
        let span = (bottom - top).max(1.0);
        let y = top + ((fx.t * 0.6).fract()) * span;
        blob(p, Pos2::new(x, y), 10.0, col, 0.5 * g);
    }
    // The head: a diamond with a glow.
    let head = Pos2::new(x, top + 5.0);
    let d = 6.0;
    let pts = vec![
        head + Vec2::new(0.0, -d),
        head + Vec2::new(d, 0.0),
        head + Vec2::new(0.0, d),
        head + Vec2::new(-d, 0.0),
    ];
    if g > 0.0 {
        blob(p, head, 16.0, col, 0.5 * g);
    }
    p.add(Shape::convex_polygon(pts, crate::theme::c(col), Stroke::new(1.0_f32, Color32::WHITE)));
}

/// What a transport button draws.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    Stop,
    Record,
    ToStart,
    ToEnd,
    Loop,
    Metronome,
    Rewind,
    FastForward,
}

fn draw_icon(p: &Painter, icon: Icon, c: Pos2, s: f32, col: Color32) {
    let stroke = Stroke::new(1.8_f32, col);
    match icon {
        Icon::Play => {
            let pts = vec![c + Vec2::new(-s * 0.4, -s * 0.5), c + Vec2::new(s * 0.55, 0.0), c + Vec2::new(-s * 0.4, s * 0.5)];
            p.add(Shape::convex_polygon(pts, col, Stroke::NONE));
        }
        Icon::Pause => {
            for dx in [-0.25, 0.25] {
                p.rect_filled(Rect::from_center_size(c + Vec2::new(dx * s, 0.0), Vec2::new(s * 0.22, s)), 1.0, col);
            }
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_center_size(c, Vec2::splat(s * 0.78)), 1.5, col);
        }
        Icon::Record => {
            p.circle_filled(c, s * 0.42, col);
        }
        Icon::ToStart | Icon::ToEnd => {
            let k = if icon == Icon::ToStart { -1.0 } else { 1.0 };
            let bar = c + Vec2::new(k * s * 0.42, 0.0);
            p.rect_filled(Rect::from_center_size(bar, Vec2::new(s * 0.14, s * 0.8)), 0.5, col);
            let tip = c + Vec2::new(k * s * 0.32, 0.0);
            let back = c + Vec2::new(-k * s * 0.42, 0.0);
            let pts = vec![tip, back + Vec2::new(0.0, -s * 0.4), back + Vec2::new(0.0, s * 0.4)];
            p.add(Shape::convex_polygon(pts, col, Stroke::NONE));
        }
        Icon::Loop => {
            let r = s * 0.38;
            let n = 20;
            let arc = |a0: f32, a1: f32| -> Vec<Pos2> {
                (0..=n).map(|i| c + Vec2::angled(a0 + (a1 - a0) * i as f32 / n as f32) * r).collect()
            };
            let pi = std::f32::consts::PI;
            p.add(Shape::line(arc(-pi * 0.95, -pi * 0.1), stroke));
            p.add(Shape::line(arc(pi * 0.05, pi * 0.9), stroke));
            for (a, dir) in [(-pi * 0.1, 1.0f32), (pi * 0.9, 1.0)] {
                let tip = c + Vec2::angled(a) * r;
                let t = Vec2::angled(a + pi / 2.0 * dir);
                let pts = vec![tip + t * 4.0, tip + Vec2::angled(a) * 3.5 - t * 1.5, tip - Vec2::angled(a) * 3.5 - t * 1.5];
                p.add(Shape::convex_polygon(pts, col, Stroke::NONE));
            }
        }
        Icon::Rewind | Icon::FastForward => {
            let k = if icon == Icon::Rewind { -1.0 } else { 1.0 };
            for dx in [-0.22, 0.22] {
                let tip = c + Vec2::new((dx + k * 0.22) * s, 0.0);
                let back = c + Vec2::new((dx - k * 0.22) * s, 0.0);
                let pts = vec![tip, back + Vec2::new(0.0, -s * 0.38), back + Vec2::new(0.0, s * 0.38)];
                p.add(Shape::convex_polygon(pts, col, Stroke::NONE));
            }
        }
        Icon::Metronome => {
            let pts = vec![c + Vec2::new(-s * 0.35, s * 0.45), c + Vec2::new(s * 0.35, s * 0.45), c + Vec2::new(s * 0.12, -s * 0.5), c + Vec2::new(-s * 0.12, -s * 0.5)];
            p.add(Shape::closed_line(pts, stroke));
            p.line_segment([c + Vec2::new(0.0, s * 0.3), c + Vec2::new(s * 0.32, -s * 0.38)], stroke);
        }
    }
}

/// A chamfered transport button with a vector icon. `big` makes the round
/// play button with its spinning ring.
pub fn button(ui: &mut Ui, icon: Icon, tip: &str, on: bool, rgb: Rgb, th: &Theme, fx: &Fx, big: bool) -> Response {
    let size = if big { Vec2::splat(44.0) } else { Vec2::new(36.0, 32.0) };
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_text(tip);
    let p = ui.painter_at(rect.expand(24.0));
    let hover = ui.ctx().animate_bool(resp.id, resp.hovered());
    let col = crate::theme::c(rgb);
    let g = th.glow.max(0.0);
    if big {
        let c = rect.center();
        let r = rect.width() * 0.5 - 2.0;
        let fill = if on { ca(rgb, 60) } else { ca(th.panel_raised, 230) };
        p.circle_filled(c, r, fill);
        p.circle_stroke(c, r, Stroke::new(1.5_f32, if on || hover > 0.0 { col } else { ca(rgb, 140) }));
        if g > 0.0 {
            let k = 0.18 + 0.25 * hover + if on { 0.25 + fx.level * 0.5 + fx.pulse * 0.4 } else { 0.0 };
            blob(&p, c, r * 2.2, rgb, k * g);
        }
        // A dashed ring that turns while playing.
        let spin = if on { fx.t * 2.4 } else { fx.t * 0.3 };
        let n = 16;
        for i in 0..n {
            if i % 2 == 1 {
                continue;
            }
            let a0 = spin + i as f32 / n as f32 * std::f32::consts::TAU;
            let a1 = a0 + std::f32::consts::TAU / n as f32 * 0.8;
            let pts: Vec<Pos2> = (0..=4).map(|k| c + Vec2::angled(a0 + (a1 - a0) * k as f32 / 4.0) * (r + 5.0)).collect();
            p.add(Shape::line(pts, Stroke::new(1.5_f32, ca(rgb, if on { 220 } else { 90 }))));
        }
        draw_icon(&p, icon, c + Vec2::new(if icon == Icon::Play { 2.0 } else { 0.0 }, 0.0), 18.0, if on { col } else { crate::theme::c(th.text) });
    } else {
        let pts = chamfer(rect, 6.0);
        let fill = if on { ca(rgb, 70) } else { ca(th.panel_raised, (200.0 + 40.0 * hover) as u8) };
        p.add(Shape::convex_polygon(pts.clone(), fill, Stroke::NONE));
        let edge = if on { col } else { crate::theme::lerp(th.border, rgb, hover) };
        p.add(Shape::closed_line(pts.clone(), Stroke::new(1.0_f32, edge)));
        if g > 0.0 && (on || hover > 0.0) {
            p.add(Shape::closed_line(pts, Stroke::new(5.0_f32, light(rgb, (0.12 * hover + if on { 0.2 } else { 0.0 }) * g))));
        }
        draw_icon(&p, icon, rect.center(), 15.0, if on { col } else { crate::theme::lerp(th.text, rgb, hover * 0.6) });
    }
    resp
}

/// A flat square transport button, laid out and drawn the way long-time
/// users of the leading editor know them: a plain icon, a quiet face that
/// lights on hover, the theme's colour when on.
pub fn transport_button(ui: &mut Ui, icon: Icon, tip: &str, on: bool, rgb: Rgb, th: &Theme) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(30.0, 26.0), Sense::click_and_drag());
    let resp = resp.on_hover_text(tip);
    let p = ui.painter_at(rect.expand(2.0));
    let hover = ui.ctx().animate_bool(resp.id, resp.hovered());
    let down = resp.is_pointer_button_down_on();
    let face = if on || down {
        crate::theme::lerp(th.panel_raised, rgb, 0.28)
    } else {
        crate::theme::lerp(th.panel_raised, th.text, 0.06 * hover)
    };
    p.rect_filled(rect, 2.0, face);
    p.rect_stroke(rect, 2.0, Stroke::new(1.0_f32, if on { crate::theme::c(rgb) } else { ca(th.border, 220) }));
    let col = if on || icon == Icon::Record {
        crate::theme::c(rgb)
    } else {
        crate::theme::lerp(th.text_dim, th.text, 0.5 + 0.5 * hover)
    };
    draw_icon(&p, icon, rect.center(), 13.0, col);
    resp
}

/// Text with extra letter spacing, painted.
pub fn spaced(p: &Painter, pos: Pos2, align: Align2, text: &str, size: f32, spacing: f32, col: Color32, mono: bool) -> Rect {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: if mono { FontId::monospace(size) } else { FontId::proportional(size) },
            color: col,
            extra_letter_spacing: spacing,
            ..Default::default()
        },
    );
    let galley = p.layout_job(job);
    let r = align.anchor_size(pos, galley.size());
    p.galley(r.min, galley, col);
    r
}

/// The big time readout: ghost segments behind glowing digits, a caption
/// and a level trace along the bottom.
pub fn readout(ui: &mut Ui, text: &str, caption: &str, th: &Theme, fx: &Fx, levels: (f32, f32)) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(214.0, 44.0), Sense::hover());
    let p = ui.painter_at(rect.expand(16.0));
    let pts = chamfer(rect, 9.0);
    gradient_poly(&p, &pts, ca(th.wave_background, 235), ca(th.panel, 235));
    p.add(Shape::closed_line(pts.clone(), Stroke::new(1.0_f32, ca(th.accent, 120))));
    if th.glow > 0.0 {
        p.add(Shape::closed_line(pts, Stroke::new(5.0_f32, light(th.accent, 0.08 * th.glow))));
    }
    spaced(&p, rect.left_top() + Vec2::new(10.0, 4.0), Align2::LEFT_TOP, caption, 8.5, 2.0, ca(th.text_dim, 200), true);
    let ghost: String = text.chars().map(|ch| if ch.is_ascii_digit() { '8' } else { ch }).collect();
    let at = rect.left_center() + Vec2::new(10.0, 4.0);
    p.text(at, Align2::LEFT_CENTER, ghost, FontId::monospace(23.0), ca(th.accent, 22));
    if th.glow > 0.0 {
        for d in [Vec2::new(-1.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0), Vec2::new(0.0, -1.0)] {
            p.text(at + d, Align2::LEFT_CENTER, text, FontId::monospace(23.0), light(th.accent, (0.25 + 0.2 * fx.pulse) * th.glow));
        }
    }
    p.text(at, Align2::LEFT_CENTER, text, FontId::monospace(23.0), crate::theme::lerp(th.accent, [255, 255, 255], 0.25));
    // Level traces, left above right.
    let y = rect.bottom() - 5.0;
    let w = rect.width() - 20.0;
    for (k, v) in [levels.0, levels.1].into_iter().enumerate() {
        let db = (v.max(1e-6).log10() * 20.0).clamp(-60.0, 0.0);
        let f = (db + 60.0) / 60.0;
        let yy = y - k as f32 * 3.0;
        p.line_segment([Pos2::new(rect.left() + 10.0, yy), Pos2::new(rect.left() + 10.0 + w, yy)], Stroke::new(1.5_f32, ca(th.border, 160)));
        if f > 0.0 {
            p.line_segment(
                [Pos2::new(rect.left() + 10.0, yy), Pos2::new(rect.left() + 10.0 + w * f, yy)],
                Stroke::new(1.5_f32, th.meter_color(db)),
            );
        }
    }
}

/// A segmented switch with a highlight that slides between choices.
/// Returns the index clicked, if any.
pub fn segmented(ui: &mut Ui, id: &str, labels: &[&str], current: usize, th: &Theme, fx: &Fx) -> Option<usize> {
    let font = FontId::proportional(12.5);
    let widths: Vec<f32> = labels
        .iter()
        .map(|l| ui.painter().layout_no_wrap(l.to_uppercase(), font.clone(), Color32::WHITE).size().x + 26.0)
        .collect();
    let total: f32 = widths.iter().sum();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total, 26.0), Sense::hover());
    let p = ui.painter_at(rect.expand(12.0));
    let pts = chamfer(rect, 7.0);
    p.add(Shape::convex_polygon(pts.clone(), ca(th.wave_background, 220), Stroke::new(1.0_f32, ca(th.border, 220))));
    let mut x = rect.left();
    let mut starts = Vec::new();
    for w in &widths {
        starts.push(x);
        x += w;
    }
    let target = starts[current.min(starts.len() - 1)];
    let slide = ui.ctx().animate_value_with_time(egui::Id::new(id), target, 0.18);
    let sw = widths[current.min(widths.len() - 1)];
    let hl = Rect::from_min_size(Pos2::new(slide, rect.top()), Vec2::new(sw, rect.height())).shrink(2.0);
    let hp = chamfer(hl, 5.0);
    p.add(Shape::convex_polygon(hp.clone(), ca(th.accent, 46), Stroke::new(1.0_f32, crate::theme::c(th.accent))));
    if th.glow > 0.0 {
        p.add(Shape::closed_line(hp, Stroke::new(5.0_f32, light(th.accent, (0.14 + fx.pulse * 0.1) * th.glow))));
    }
    let mut clicked = None;
    for (i, l) in labels.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(starts[i], rect.top()), Vec2::new(widths[i], rect.height()));
        let resp = ui.interact(r, ui.id().with((id, i)), Sense::click());
        let on = i == current;
        let col = if on {
            crate::theme::lerp(th.accent, [255, 255, 255], 0.35)
        } else if resp.hovered() {
            crate::theme::c(th.text)
        } else {
            crate::theme::c(th.text_dim)
        };
        spaced(&p, r.center(), Align2::CENTER_CENTER, &l.to_uppercase(), 11.5, 1.6, col, false);
        if resp.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// The AudiForm 2 mark, drawn into the square `rect`: a broken frame,
/// an A with one solid leg and one leg in knocked-aside spectral slices,
/// a laser crossbar that fires out through the frame, and an outlined 2
/// split into the theme's two colours. `reveal` (0..1) builds it up, for
/// the boot sequence; the slices jolt on a beat.
pub fn logo_mark(p: &Painter, rect: Rect, th: &Theme, fx: &Fx, reveal: f32) {
    let s = rect.width() / 512.0;
    let at = |x: f32, y: f32| rect.left_top() + Vec2::new(x * s, y * s);
    let white = Color32::from_rgb(242, 248, 255);
    let a = |k: f32| (reveal.clamp(0.0, 1.0) * k * 255.0) as u8;
    let fade = |c: Color32, k: f32| {
        let [r, g, b, _] = c.to_array();
        Color32::from_rgba_unmultiplied(r, g, b, a(k))
    };
    let w = (2.0 * s).max(1.0);
    // The frame, broken where the laser leaves it.
    let bar_y: f32 = 304.0;
    let tile: [(f32, f32); 6] = [(60.0, 60.0), (404.0, 60.0), (452.0, 108.0), (452.0, 452.0), (108.0, 452.0), (60.0, 404.0)];
    for k in 0..tile.len() {
        let (x0, y0) = tile[k];
        let (x1, y1) = tile[(k + 1) % tile.len()];
        // The left and right sides have a gap at the crossbar.
        if (x0 - x1).abs() < 0.1 && (y0.min(y1)..y0.max(y1)).contains(&bar_y) {
            let gap = 16.0;
            let (lo, hi) = (y0.min(y1), y0.max(y1));
            p.line_segment([at(x0, lo), at(x0, bar_y - gap)], Stroke::new(w * 1.2, fade(white, 0.9)));
            p.line_segment([at(x0, bar_y + gap), at(x0, hi)], Stroke::new(w * 1.2, fade(white, 0.9)));
        } else {
            p.line_segment([at(x0, y0), at(x1, y1)], Stroke::new(w * 1.2, fade(white, 0.9)));
        }
    }
    let br = Stroke::new(w * 1.5, fade(crate::theme::c(th.accent), 1.0));
    p.add(Shape::line(vec![at(48.0, 84.0), at(48.0, 48.0), at(84.0, 48.0)], br));
    p.add(Shape::line(vec![at(464.0, 428.0), at(464.0, 464.0), at(428.0, 464.0)], br));
    // The 2, outlined, in the two colours, a little apart.
    let two = |dx: f32, dy: f32| -> Vec<Pos2> {
        let mut pts = Vec::new();
        let (cx, cy, r) = (362.0, 214.0 + 0.0, 54.0);
        for k in 0..=24 {
            let ang = std::f32::consts::PI + k as f32 / 24.0 * std::f32::consts::PI * 1.25;
            pts.push(at(cx + r * ang.cos() + dx, cy + r * ang.sin() + dy));
        }
        pts.push(at(318.0 + dx, 372.0 + dy));
        pts.push(at(426.0 + dx, 372.0 + dy));
        pts
    };
    let tw = (11.0 * s).max(1.5);
    p.add(Shape::line(two(6.0, 4.0), Stroke::new(tw, light(th.accent_alt, 0.9 * reveal))));
    p.add(Shape::line(two(-3.0, -2.0), Stroke::new(tw, light(th.accent, 0.95 * reveal))));
    // The A.
    let (apex_y, foot_y, apex_x, lf, rf, half) = (104.0f32, 404.0f32, 256.0f32, 146.0f32, 366.0f32, 27.0f32);
    let cl = |y: f32| apex_x - (y - apex_y) * (apex_x - lf) / (foot_y - apex_y);
    let cr = |y: f32| apex_x + (y - apex_y) * (rf - apex_x) / (foot_y - apex_y);
    let left = |dx: f32| {
        vec![
            at(apex_x - 22.0 + dx, apex_y),
            at(apex_x + 22.0 + dx, apex_y),
            at(cl(foot_y) + half + dx, foot_y),
            at(cl(foot_y) - half + dx, foot_y),
        ]
    };
    let jolt = fx.pulse * 14.0;
    let offsets = [0.0, 10.0, -7.0, 18.0, -12.0, 4.0, 26.0, -4.0];
    let slices = |dx: f32| -> Vec<Vec<Pos2>> {
        let mut out = Vec::new();
        let mut y = apex_y + 14.0;
        for (k, off) in offsets.iter().enumerate() {
            if y >= foot_y {
                break;
            }
            let (y0, y1) = (y, (y + 31.0).min(foot_y));
            let o = off + dx + if k % 2 == 0 { jolt } else { -jolt };
            out.push(vec![
                at(cr(y0) - half + o, y0),
                at(cr(y0) + half + o, y0),
                at(cr(y1) + half + o, y1),
                at(cr(y1) - half + o, y1),
            ]);
            y += 37.0;
        }
        out
    };
    for (rgb, dx) in [(th.accent_alt, 4.0), (th.accent, -4.0)] {
        let col = light(rgb, 0.7 * reveal);
        p.add(Shape::convex_polygon(left(dx), col, Stroke::NONE));
        for sl in slices(dx) {
            p.add(Shape::convex_polygon(sl, col, Stroke::NONE));
        }
    }
    p.add(Shape::convex_polygon(left(0.0), fade(white, 1.0), Stroke::NONE));
    for sl in slices(0.0) {
        p.add(Shape::convex_polygon(sl, fade(white, 1.0), Stroke::NONE));
    }
    // The laser crossbar fires out through the frame.
    let reach = 24.0 + (476.0 - 24.0) * ((reveal - 0.3) / 0.7).clamp(0.0, 1.0);
    if reach > 30.0 {
        let k = 1.0 + fx.pulse * 0.6;
        p.line_segment([at(24.0, bar_y), at(reach, bar_y)], Stroke::new(14.0 * s, light(th.accent_alt, 0.25 * k)));
        p.line_segment([at(24.0, bar_y), at(reach, bar_y)], Stroke::new(5.0 * s, crate::theme::c(th.accent_alt)));
        p.line_segment([at(24.0, bar_y), at(reach, bar_y)], Stroke::new((1.5 * s).max(1.0), Color32::WHITE));
        let d = 12.0;
        p.add(Shape::convex_polygon(
            vec![at(reach + 12.0, bar_y), at(reach, bar_y - d), at(reach - 12.0, bar_y), at(reach, bar_y + d)],
            crate::theme::c(th.accent_alt),
            Stroke::new(w, Color32::WHITE),
        ));
    }
}

/// The wordmark: the mark, then AUDIFORM with light passing across it and
/// a raised 2 split into the theme's two colours.
pub fn wordmark(ui: &mut Ui, th: &Theme, fx: &Fx) {
    let text = "AUDIFORM";
    let size = 14.0;
    let spacing = 4.0;
    let font = FontId::monospace(size);
    let cw = ui.painter().layout_no_wrap("M".into(), font.clone(), Color32::WHITE).size().x + spacing;
    let w = cw * text.len() as f32;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w + 58.0, 26.0), Sense::hover());
    let p = ui.painter_at(rect.expand(20.0));
    logo_mark(&p, Rect::from_min_size(rect.left_top() + Vec2::new(0.0, -1.0), Vec2::splat(28.0)), th, fx, 1.0);
    let x0 = rect.left() + 34.0;
    let sweep = (fx.t * 0.35).fract() * 1.8 - 0.4;
    for (i, ch) in text.chars().enumerate() {
        let f = i as f32 / text.len() as f32;
        let shine = (1.0 - ((f - sweep).abs() * 6.0)).max(0.0);
        let base = crate::theme::lerp(th.text, [255, 255, 255], 0.2);
        let b = base.to_array();
        let m = |x: u8| (x as f32 + (255.0 - x as f32) * shine) as u8;
        let col = Color32::from_rgb(m(b[0]), m(b[1]), m(b[2]));
        let pos = Pos2::new(x0 + i as f32 * cw, rect.center().y);
        if th.glow > 0.0 && shine > 0.0 {
            p.text(pos, Align2::LEFT_CENTER, ch, FontId::monospace(size + 1.0), light(th.accent, 0.4 * th.glow * shine));
        }
        p.text(pos, Align2::LEFT_CENTER, ch, font.clone(), col);
    }
    let two = Pos2::new(x0 + w + 2.0, rect.center().y - 4.0);
    p.text(two + Vec2::new(1.5, 1.0), Align2::LEFT_CENTER, "2", FontId::monospace(size + 4.0), light(th.accent_alt, 0.9));
    p.text(two - Vec2::new(1.0, 0.5), Align2::LEFT_CENTER, "2", FontId::monospace(size + 4.0), light(th.accent, 0.9));
    resp.on_hover_text("AudiForm 2");
}

/// The boot sequence over the whole window for the first moments. Returns
/// true while it is still showing. Any key or click skips it.
pub fn boot(ctx: &egui::Context, th: &Theme, fx: &Fx, skipped: &mut bool) -> bool {
    const LEN: f32 = 2.1;
    let t = fx.clock;
    if *skipped || !th.intro || t > LEN {
        return false;
    }
    if ctx.input(|i| i.pointer.any_click() || !i.keys_down.is_empty()) {
        *skipped = true;
        return false;
    }
    let screen = ctx.screen_rect();
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("af27-boot")));
    let fade = if t > LEN - 0.45 { (LEN - t) / 0.45 } else { 1.0 };
    p.rect_filled(screen, 0.0, Color32::from_black_alpha((250.0 * fade) as u8));
    let c = screen.center();
    // Rings ripple out.
    for k in 0..3 {
        let rt = (t - k as f32 * 0.18).max(0.0);
        let r = rt * 520.0;
        let a = (1.0 - rt / 1.4).max(0.0) * fade;
        if a > 0.0 {
            p.circle_stroke(c, r, Stroke::new(1.5_f32, light(th.accent, 0.6 * a)));
        }
    }
    // A grid draws itself in.
    let gt = (t / 0.9).min(1.0);
    for i in -10..=10 {
        let x = c.x + i as f32 * 60.0;
        let len = screen.height() * 0.5 * gt;
        p.line_segment([Pos2::new(x, c.y - len), Pos2::new(x, c.y + len)], Stroke::new(1.0_f32, light(th.grid_glow, 0.12 * fade)));
    }
    // The wordmark decodes letter by letter.
    logo_mark(&p, Rect::from_center_size(c - Vec2::new(0.0, 150.0), Vec2::splat(180.0)), th, fx, (t / 1.0).min(1.0) * fade);
    let word = "AUDIFORM 2";
    let glyphs = b"#%&@$<>/\\|+*=01";
    let mut shown = String::new();
    for (i, ch) in word.chars().enumerate() {
        let at = 0.25 + i as f32 * 0.07;
        if t >= at + 0.25 {
            shown.push(ch);
        } else if t >= at {
            let k = ((t * 40.0) as usize + i * 7) % glyphs.len();
            shown.push(glyphs[k] as char);
        } else {
            shown.push(' ');
        }
    }
    let col = crate::theme::lerp(th.accent, [255, 255, 255], 0.3);
    let a = (fade * 255.0) as u8;
    let mut colr = col.to_array();
    colr[3] = a;
    let main = Color32::from_rgba_unmultiplied(colr[0], colr[1], colr[2], a);
    for d in [-2.0, 2.0] {
        spaced(&p, c + Vec2::new(d, 0.0), Align2::CENTER_CENTER, &shown, 44.0, 14.0, light(th.accent, 0.3 * fade), true);
    }
    spaced(&p, c, Align2::CENTER_CENTER, &shown, 44.0, 14.0, main, true);
    // A light bar sweeps open under it, and the tagline types itself out.
    let bt = ((t - 0.6) / 0.6).clamp(0.0, 1.0);
    let half = 260.0 * bt;
    p.line_segment([c + Vec2::new(-half, 38.0), c + Vec2::new(half, 38.0)], Stroke::new(2.0_f32, light(th.accent_alt, 0.9 * fade)));
    p.line_segment([c + Vec2::new(-half, 38.0), c + Vec2::new(half, 38.0)], Stroke::new(8.0_f32, light(th.accent_alt, 0.2 * fade)));
    let tag = "SIGNAL PATH ONLINE  //  ALL CHANNELS NOMINAL";
    let n = (((t - 0.9) / 0.7).clamp(0.0, 1.0) * tag.len() as f32) as usize;
    let cursor = if (t * 8.0) as i32 % 2 == 0 { "_" } else { " " };
    spaced(
        &p,
        c + Vec2::new(0.0, 60.0),
        Align2::CENTER_CENTER,
        &format!("{}{}", &tag[..n], cursor),
        10.0,
        3.0,
        Color32::from_rgba_unmultiplied(th.text_dim[0], th.text_dim[1], th.text_dim[2], a),
        true,
    );
    ctx.request_repaint();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chamfer_cuts_two_corners_and_degrades_gracefully() {
        let r = Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 50.0));
        assert_eq!(chamfer(r, 8.0).len(), 6);
        assert_eq!(chamfer(r, 0.0).len(), 4);
        // Never cuts more than a quarter of a side.
        let tiny = Rect::from_min_size(Pos2::ZERO, Vec2::new(8.0, 8.0));
        let pts = chamfer(tiny, 50.0);
        assert!(pts.iter().all(|p| p.x >= 0.0 && p.x <= 8.0 && p.y >= 0.0 && p.y <= 8.0));
    }

    #[test]
    fn light_is_additive() {
        assert_eq!(light([200, 100, 50], 0.5).a(), 0);
        assert_eq!(light([200, 100, 50], 0.5).r(), 100);
    }

    #[test]
    fn fx_pulses_on_a_jump_and_settles() {
        let ctx = egui::Context::default();
        let th = Theme::default();
        let mut fx = Fx::default();
        for _ in 0..30 {
            fx.update(&ctx, &th, 0.01);
        }
        assert!(fx.level < 0.9);
    }
}
