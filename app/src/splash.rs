// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Splash screens that belong to a theme (`Theme::splash`), shown in place
//! of the standard boot sequence: a vaporwave sunset for the Mall themes
//! and deep water for Undercurrent. Each runs [`LEN`] seconds; any click or
//! key skips it.

use egui::{Align2, Color32, FontId, Mesh, Painter, Pos2, Rect, Shape, Stroke, Vec2};

use crate::theme::{ca, Rgb, Theme};

pub const LEN: f32 = 2.8;

fn rgba(rgb: Rgb, a: f32) -> Color32 {
    ca(rgb, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn vgradient(p: &Painter, r: Rect, stops: &[(f32, Rgb)], a: f32) {
    let mut m = Mesh::default();
    for (i, (f, c)) in stops.iter().enumerate() {
        let y = r.top() + r.height() * f;
        m.colored_vertex(Pos2::new(r.left(), y), rgba(*c, a));
        m.colored_vertex(Pos2::new(r.right(), y), rgba(*c, a));
        if i > 0 {
            let k = (i as u32) * 2;
            m.add_triangle(k - 2, k - 1, k + 1);
            m.add_triangle(k - 2, k + 1, k);
        }
    }
    p.add(Shape::mesh(m));
}

fn image(p: &Painter, name: &str, r: Rect, uv: Rect, tint: Color32) {
    if let Some(t) = crate::pictures::texture(p.ctx(), name, r.width() * p.ctx().pixels_per_point())
    {
        p.image(t.id(), r, uv, tint);
    }
}

const FULL: Rect = Rect {
    min: Pos2::ZERO,
    max: Pos2 { x: 1.0, y: 1.0 },
};

/// A small deterministic hash for scattering stars and bubbles.
fn h(i: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

// ------------------------------------------------------------- Mall

/// The Mall Pastel '95 splash: a vaporwave sunset over a grid floor
/// between two marble columns. `night` swaps the sun for the moon.
pub fn mall(p: &Painter, screen: Rect, th: &Theme, t: f32, fade: f32, night: bool) {
    let a = fade;
    let horizon = screen.top() + screen.height() * 0.62;
    let sky = Rect::from_min_max(screen.min, Pos2::new(screen.right(), horizon));
    let floor = Rect::from_min_max(Pos2::new(screen.left(), horizon), screen.max);
    if night {
        vgradient(
            p,
            sky,
            &[
                (0.0, [12, 8, 30]),
                (0.55, [42, 24, 70]),
                (1.0, [120, 50, 120]),
            ],
            a,
        );
        vgradient(p, floor, &[(0.0, [30, 14, 52]), (1.0, [8, 20, 28])], a);
        // Stars twinkle in.
        for i in 0..140 {
            let x = screen.left() + h(i) * screen.width();
            let y = sky.top() + h(i + 999) * sky.height() * 0.9;
            let tw = 0.5 + 0.5 * (t * (2.0 + h(i + 7) * 4.0) + h(i + 3) * 6.0).sin();
            let on = ((t - h(i + 11) * 0.8) / 0.4).clamp(0.0, 1.0);
            p.circle_filled(
                Pos2::new(x, y),
                0.6 + h(i + 5) * 1.3,
                rgba([230, 220, 255], a * on * (0.35 + 0.65 * tw)),
            );
        }
    } else {
        vgradient(
            p,
            sky,
            &[
                (0.0, [255, 190, 222]),
                (0.45, [255, 214, 196]),
                (0.8, [255, 232, 214]),
                (1.0, [255, 200, 230]),
            ],
            a,
        );
        vgradient(
            p,
            floor,
            &[(0.0, [232, 214, 255]), (1.0, [190, 240, 226])],
            a,
        );
    }

    // The sun (or moon) rises out of the horizon, striped below its middle.
    let rise = ease(t / 1.3);
    let r = screen.height().min(screen.width()) * 0.26;
    let c = Pos2::new(screen.center().x, horizon + r * 0.35 - rise * r * 0.7);
    let (top, mid, bottom): (Rgb, Rgb, Rgb) = if night {
        ([236, 232, 255], [200, 170, 255], [255, 143, 199])
    } else {
        ([255, 196, 107], [255, 143, 199], [179, 156, 255])
    };
    if night {
        p.circle_filled(c, r * 1.35, rgba([255, 143, 199], 0.08 * a));
    }
    let step = 2.0;
    let mut y = c.y - r;
    while y < (c.y + r).min(horizon) {
        let f = (y - (c.y - r)) / (2.0 * r);
        // The gaps widen towards the bottom, as on a 1980s record sleeve.
        let gap = if f > 0.5 {
            let band = ((f - 0.5) * 12.0).fract();
            band < 0.12 + (f - 0.5) * 0.9
        } else {
            false
        };
        if !gap {
            let dy = y + step * 0.5 - c.y;
            let w = (r * r - dy * dy).max(0.0).sqrt();
            let col = if f < 0.5 {
                mix(top, mid, f * 2.0)
            } else {
                mix(mid, bottom, (f - 0.5) * 2.0)
            };
            p.rect_filled(
                Rect::from_min_max(Pos2::new(c.x - w, y), Pos2::new(c.x + w, y + step)),
                0.0,
                rgba(col, a),
            );
        }
        y += step;
    }

    // The grid floor, running towards you.
    let grid: Rgb = if night {
        [255, 120, 200]
    } else {
        [190, 120, 255]
    };
    let gt = ease((t - 0.2) / 0.8);
    let vanish = Pos2::new(screen.center().x, horizon);
    for i in -16..=16 {
        let x = screen.center().x + i as f32 * screen.width() / 10.0;
        let end = Pos2::new(x, screen.bottom());
        let s = vanish + (end - vanish) * gt;
        p.line_segment([vanish, s], Stroke::new(1.2_f32, rgba(grid, 0.55 * a)));
    }
    let scroll = (t * 0.9).fract();
    for k in 0..14 {
        let z = (k as f32 + scroll) / 14.0;
        let yy = horizon + (screen.bottom() - horizon) * z * z;
        p.line_segment(
            [Pos2::new(screen.left(), yy), Pos2::new(screen.right(), yy)],
            Stroke::new(1.0 + z * 1.5, rgba(grid, (0.15 + 0.5 * z) * a * gt)),
        );
    }
    p.line_segment(
        [
            Pos2::new(screen.left(), horizon),
            Pos2::new(screen.right(), horizon),
        ],
        Stroke::new(2.0_f32, rgba(grid, 0.9 * a)),
    );

    // Marble columns slide in from the edges.
    let set = if th.columns.is_empty() {
        "mall-day".to_string()
    } else {
        th.columns.clone()
    };
    let set = if th.realistic {
        format!("{set}-real")
    } else {
        set
    };
    let ch = screen.height() * 0.78;
    let cw = ch * 26.0 / 220.0;
    let slide = ease((t - 0.1) / 0.9);
    for left in [true, false] {
        let x = if left {
            screen.left() + screen.width() * 0.08 - (1.0 - slide) * screen.width() * 0.3
        } else {
            screen.right() - screen.width() * 0.08 - cw + (1.0 - slide) * screen.width() * 0.3
        };
        let r = Rect::from_min_size(
            Pos2::new(x, horizon + screen.height() * 0.08 - ch),
            Vec2::new(cw, ch),
        );
        let cap = Rect::from_min_size(r.min, Vec2::new(cw, cw * 30.0 / 26.0));
        let base = Rect::from_min_max(Pos2::new(r.left(), r.bottom() - cw * 24.0 / 26.0), r.max);
        let shaft = Rect::from_min_max(
            Pos2::new(r.left(), cap.bottom()),
            Pos2::new(r.right(), base.top()),
        );
        let tint = Color32::from_white_alpha((a * 255.0) as u8);
        image(
            p,
            &format!("{set}-shaft"),
            shaft,
            Rect::from_min_max(
                Pos2::ZERO,
                Pos2::new(1.0, shaft.height() / (cw * 12.0 / 26.0)),
            ),
            tint,
        );
        image(p, &format!("{set}-capital"), cap, FULL, tint);
        image(p, &format!("{set}-base"), base, FULL, tint);
    }

    // A row of glazed tiles along the bottom.
    let tiles = format!("{}-tiles", th.columns);
    let th_ = 28.0;
    let tr = Rect::from_min_max(Pos2::new(screen.left(), screen.bottom() - th_), screen.max);
    let wipe = ease((t - 0.5) / 0.7);
    let shown = Rect::from_min_max(
        tr.min,
        Pos2::new(tr.left() + tr.width() * wipe, tr.bottom()),
    );
    image(
        p,
        &tiles,
        shown,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(shown.width() / (th_ * 8.0), 1.0)),
        Color32::from_white_alpha((a * 255.0) as u8),
    );

    // The neon sign from the menu bar, larger: it flickers on, tube by
    // tube as it were, then holds.
    let tt = ease((t - 0.55) / 0.6);
    let size = (screen.width() / 14.0).clamp(36.0, 92.0);
    let at = Pos2::new(screen.center().x, screen.top() + screen.height() * 0.12);
    let sign = if night {
        "mall-night-neon-big"
    } else {
        "mall-day-neon-big"
    };
    if let Some(tex) = crate::skin::neon_sign(p.ctx(), sign) {
        let [w, h] = tex.size_vec2().into();
        let width = (screen.width() * 0.42).clamp(320.0, 900.0);
        let r = Rect::from_center_size(
            at + Vec2::new(0.0, size * 0.45),
            Vec2::new(width, width * h / w.max(1.0)),
        );
        // A neon strike: on, off, on, before it settles.
        let strike = match t - 0.55 {
            x if x < 0.0 => 0.0,
            x if x < 0.08 => 1.0,
            x if x < 0.16 => 0.25,
            x if x < 0.24 => 1.0,
            x if x < 0.3 => 0.5,
            _ => 1.0,
        };
        let k = (a * strike * tt.max(0.6)).clamp(0.0, 1.0);
        let v = (k * 255.0) as u8;
        p.image(
            tex.id(),
            r,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::from_rgba_premultiplied(v, v, v, v),
        );
    }

    // The Japanese lines type themselves out.
    let lines = if night {
        ["オーディオフォーム ・ ２", "ショッピングモール ・ ナイト"]
    } else {
        [
            "オーディオフォーム ・ ２",
            "ショッピングモール ・ ａｅｓｔｈｅｔｉｃ",
        ]
    };
    let sub: Rgb = if night {
        [255, 154, 213]
    } else {
        [106, 79, 179]
    };
    for (i, line) in lines.iter().enumerate() {
        let start = 0.95 + i as f32 * 0.35;
        let n = line.chars().count();
        let k = (((t - start) / 0.5).clamp(0.0, 1.0) * n as f32) as usize;
        let s: String = line.chars().take(k).collect();
        let y = at.y + size * 1.95 + i as f32 * size * 0.42;
        p.text(
            Pos2::new(screen.center().x - galley_half(p, line, size * 0.3), y),
            Align2::LEFT_CENTER,
            s,
            jp(size * 0.3),
            rgba(sub, a),
        );
    }
}

/// One Japanese face for a whole line.
fn jp(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name(crate::skin::JP.into()))
}

/// Half the width a line will take, so typed-out text stays put.
fn galley_half(p: &Painter, s: &str, size: f32) -> f32 {
    p.layout_no_wrap(s.to_string(), jp(size), Color32::WHITE)
        .size()
        .x
        * 0.5
}

// ------------------------------------------------------------- Undercurrent

/// A textured grid over `r`, each vertex's texture point and colour given
/// by `f(u, v) -> (uv, colour)` with u, v in 0..1 across the grid.
fn warped(
    p: &Painter,
    tex: egui::TextureId,
    r: Rect,
    nx: u32,
    ny: u32,
    f: impl Fn(f32, f32) -> (Pos2, Pos2, Color32),
) {
    let mut m = Mesh::with_texture(tex);
    for j in 0..=ny {
        for i in 0..=nx {
            let (u, v) = (i as f32 / nx as f32, j as f32 / ny as f32);
            let (pos, uv, col) = f(u, v);
            let _ = r;
            m.vertices.push(egui::epaint::Vertex {
                pos,
                uv,
                color: col,
            });
        }
    }
    let w = nx + 1;
    for j in 0..ny {
        for i in 0..nx {
            let a = j * w + i;
            m.add_triangle(a, a + 1, a + w + 1);
            m.add_triangle(a, a + w + 1, a + w);
        }
    }
    p.add(Shape::mesh(m));
}

/// The Undercurrent splash: looking straight down into deep water while
/// its colours swirl, the title rising out of the dark and settling at the
/// surface.
pub fn undercurrent(p: &Painter, screen: Rect, th: &Theme, t: f32, fade: f32) {
    let a = fade;
    p.rect_filled(screen, 0.0, rgba([2, 5, 20], a));
    let ppp = p.ctx().pixels_per_point();
    // The painted water, turning slowly about the middle: a vortex that
    // winds tighter near the centre, with a gentle swell over it.
    if let Some(tex) = crate::pictures::texture(p.ctx(), &th.picture, screen.width() * ppp) {
        let aspect = screen.width() / screen.height().max(1.0);
        let tint = Color32::from_white_alpha((a * 235.0) as u8);
        warped(p, tex.id(), screen, 48, 30, |u, v| {
            let pos = Pos2::new(
                screen.left() + u * screen.width(),
                screen.top() + v * screen.height(),
            );
            let (dx, dy) = ((u - 0.5) * aspect, v - 0.5);
            let r = (dx * dx + dy * dy).sqrt();
            let ang = (0.15 + 0.22 * t) * (-r * 2.4).exp() + 0.04 * (t * 0.7 + r * 6.0).sin();
            let (s_, c_) = ang.sin_cos();
            let (rx, ry) = (dx * c_ - dy * s_, dx * s_ + dy * c_);
            let swell = 0.006 * (v * 11.0 + t * 1.3).sin() + 0.004 * (u * 17.0 - t * 1.1).cos();
            let uv = Pos2::new(
                0.5 + rx / aspect * 0.86 + swell,
                0.5 + ry * 0.86 + swell * 0.7,
            );
            (pos, uv, tint)
        });
    }
    // Depth: the water darkens away from the middle, as if very deep.
    let c = screen.center();
    let rr = screen.width().max(screen.height()) * 0.75;
    let mut m = Mesh::default();
    m.colored_vertex(c, Color32::TRANSPARENT);
    let n = 48;
    for i in 0..=n {
        let ang = i as f32 / n as f32 * std::f32::consts::TAU;
        m.colored_vertex(
            c + Vec2::new(ang.cos(), ang.sin()) * rr,
            rgba([1, 3, 16], 0.85 * a),
        );
    }
    for i in 1..=n {
        m.add_triangle(0, i, i + 1);
    }
    p.add(Shape::mesh(m));
    p.rect_filled(screen, 0.0, rgba([2, 6, 28], 0.25 * a));

    // The title rises from the deep: dim, blue and bent by the water at
    // first, clearing and steadying as it reaches the surface.
    if let Some(tex) = crate::pictures::texture(p.ctx(), &th.title_picture, f32::MAX) {
        let [tw, tht] = tex.size_vec2().into();
        let rise = ease((t - 0.15) / 1.9);
        let w = (screen.width() * 0.44).min(tw * 1.7) * (0.82 + 0.18 * rise);
        let hgt = w * tht / tw;
        let r = Rect::from_center_size(c - Vec2::new(0.0, 20.0), Vec2::new(w, hgt));
        let bend = 14.0 * (1.0 - rise).powf(1.3);
        let deep: Rgb = [40, 110, 160];
        let col = mix(deep, [255, 255, 255], rise);
        let alpha = (rise.powf(1.4) * a).clamp(0.0, 1.0);
        let tint = rgba(col, alpha);
        warped(p, tex.id(), r, 30, 10, |u, v| {
            let dx = bend * (v * 8.0 + t * 2.6 + u * 3.0).sin();
            let dy = bend * 0.5 * (u * 6.5 + t * 2.1).cos();
            let pos = Pos2::new(r.left() + u * r.width() + dx, r.top() + v * r.height() + dy);
            (pos, Pos2::new(u, v), tint)
        });
        // As it breaks the surface, ripples spread out over the water:
        // uneven rings that wander, thin out and break up as they go.
        let rt = t - 1.85;
        if rt > 0.0 {
            for k in 0..5u32 {
                let q = rt - k as f32 * 0.13 - h(k + 70) * 0.12;
                if q <= 0.0 {
                    continue;
                }
                let life = (1.0 - q / 1.1).max(0.0);
                if life <= 0.0 {
                    continue;
                }
                let grow = 1.0 + q * (1.3 + h(k + 80) * 0.5);
                let base = Vec2::new(w * 0.56, hgt * 0.66) * grow;
                let (p1, p2, p3) = (h(k + 90) * 6.3, h(k + 91) * 6.3, h(k + 92) * 6.3);
                let n = 120;
                let pt = |i: u32| {
                    let ang = i as f32 / n as f32 * std::f32::consts::TAU;
                    let wob = 1.0
                        + 0.07 * (3.0 * ang + p1 + q * 1.7).sin()
                        + 0.045 * (5.0 * ang + p2 - q * 2.3).sin()
                        + 0.025 * (9.0 * ang + p3 + q * 3.1).sin();
                    Pos2::new(
                        r.center().x + ang.cos() * base.x * wob,
                        r.center().y + ang.sin() * base.y * wob,
                    )
                };
                for i in 0..n {
                    let ang = (i as f32 + 0.5) / n as f32 * std::f32::consts::TAU;
                    // Broken: the ring fades in and out around its length.
                    let gap = (0.5 + 0.5 * (4.0 * ang + p3 + q * 2.0).sin())
                        * (0.6 + 0.4 * (7.0 * ang + p1).cos());
                    let al = life * life * gap.clamp(0.0, 1.0) * a;
                    if al < 0.02 {
                        continue;
                    }
                    let (s0, s1) = (pt(i), pt(i + 1));
                    let soft = 2.0 + q * 6.0;
                    p.line_segment(
                        [s0, s1],
                        Stroke::new(soft, rgba([158, 230, 238], 0.10 * al)),
                    );
                    p.line_segment(
                        [s0, s1],
                        Stroke::new(1.2_f32, rgba([200, 245, 250], 0.38 * al)),
                    );
                }
            }
        }
    }
}
