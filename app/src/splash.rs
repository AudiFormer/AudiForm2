// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

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

/// A title's letters as a grey mask (0..1), decoded once.
#[derive(Clone, Copy)]
struct Mask {
    w: usize,
    h: usize,
    px: &'static [f32],
}

fn water_mask(name: &str) -> Option<Mask> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Mask>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut c = cache.lock().ok()?;
    *c.entry(name.to_string()).or_insert_with(|| {
        let b = crate::pictures::bytes(name)?;
        let img = image::load_from_memory(b).ok()?.to_luma8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let px: Vec<f32> = img.pixels().map(|p| p.0[0] as f32 / 255.0).collect();
        Some(Mask { w, h, px: Box::leak(px.into_boxed_slice()) })
    })
}

impl Mask {
    /// Bilinear, at (x, y) in the mask's pixels; nothing outside it.
    fn at(&self, x: f32, y: f32) -> f32 {
        if x < 0.0 || y < 0.0 || x >= (self.w - 1) as f32 || y >= (self.h - 1) as f32 {
            return 0.0;
        }
        let (x0, y0) = (x as usize, y as usize);
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let i = y0 * self.w + x0;
        let top = self.px[i] * (1.0 - fx) + self.px[i + 1] * fx;
        let bot = self.px[i + self.w] * (1.0 - fx) + self.px[i + self.w + 1] * fx;
        top * (1.0 - fy) + bot * fy
    }
}

/// The title on the water, made this frame: the letters ride the water's
/// swell (bent too while still rising), and their shadow is a second copy
/// the water moves on its own, pushed down and to the right by an amount
/// that drifts along the word; the shadow is cut away under the letters,
/// which are see-through. Drawn into `r`.
#[allow(clippy::too_many_arguments)]
fn water_title(p: &Painter, screen: Rect, r: Rect, t: f32, bend: f32, m: Mask, fill: Rgb, alpha: f32, water: Option<&Pixels>) {
    let started = std::time::Instant::now();
    let aspect = screen.width() / screen.height().max(1.0);
    let centre = screen.center();
    let reach = screen.width().max(screen.height()) * 0.75;
    let ppp = p.ctx().pixels_per_point();
    let ow = (r.width() * ppp).clamp(64.0, 1100.0) as usize;
    let oh = ((ow as f32) * r.height() / r.width()).max(16.0) as usize;
    let (sw, sh) = (screen.width().max(1.0), screen.height().max(1.0));
    let (mw, mh) = (m.w as f32, m.h as f32);
    let fill = [fill[0] as f32, fill[1] as f32, fill[2] as f32];
    let dark = [6.0f32, 12.0, 40.0];
    // The letters fill about two thirds of the mask's height (the rest is
    // room for the waves and the shadow).
    let letter_h = r.height() * 0.674;
    let (tilt_s, tilt_c) = TILT.to_radians().sin_cos();
    // Everything that moves is smooth, so it is worked out on a coarse grid
    // (every GRID pixels) and blended between: for each node, the letters'
    // and the shadow's displacement and where the water under it is.
    const GRID: usize = 8;
    let (gw, gh) = (ow / GRID + 2, oh / GRID + 2);
    let node = |gx: usize, gy: usize| -> [f32; 7] {
        let u = (gx * GRID) as f32 / ow as f32;
        let v = (gy * GRID) as f32 / oh as f32;
        let x = r.left() + u * r.width();
        let y = r.top() + v * r.height();
        let (su, sv) = ((x - screen.left()) / sw, (y - screen.top()) / sh);
        // The letters: where the water has moved this point from, plus the
        // rendered logo's own waves.
        let s = water_swell(su, sv, t);
        let (wx, wy) = logo_waves(u * r.width(), v * r.height(), letter_h, t, 0.0);
        let lx = bend * (v * 8.0 + t * 2.6 + u * 3.0).sin() - RIDE * s / 0.86 * sw + wx;
        let ly = bend * 0.5 * (u * 6.5 + t * 2.1).cos() - RIDE * s * 0.7 / 0.86 * sh + wy;
        // The shadow: its own swell (elsewhere on the water, later in
        // time), its own waves, and a push that swings along the word.
        let s2 = water_swell(su + 0.13, sv + 0.07, t + 0.9);
        let push_x = r.height() * 0.030 * (1.0 + 0.9 * (u * 5.1 + 0.7 + t * 0.4).sin());
        let push_y = r.height() * 0.028 * (1.0 + 0.8 * (u * 3.7 + v * 2.0 + 1.3 - t * 0.3).cos());
        let fine = r.height() * 0.004;
        let (wx2, wy2) = logo_waves(u * r.width(), v * r.height(), letter_h, t, 2.3);
        let sx = push_x - RIDE * s2 / 0.86 * sw * 1.3 + fine * (v * 23.0 + u * 9.0 + t * 3.1).sin() + wx2 * 1.1;
        let sy = push_y - RIDE * s2 * 0.7 / 0.86 * sh * 1.3 + fine * (u * 19.0 - v * 7.0 + t * 2.7).cos() + wy2 * 1.1;
        // The water under it, and how dark the depth makes it there.
        let (wu, wv) = water_uv(su, sv, t, aspect);
        let d = ((x - centre.x).powi(2) + (y - centre.y).powi(2)).sqrt() / reach;
        let depth = (1.0 - 0.85 * d.min(1.0)) * 0.75 * 235.0 / 255.0;
        [lx, ly, sx, sy, wu, wv, depth]
    };
    let mut nodes = vec![[0.0f32; 7]; gw * gh];
    for gy in 0..gh {
        for gx in 0..gw {
            nodes[gy * gw + gx] = node(gx, gy);
        }
    }
    let mut out = vec![Color32::TRANSPARENT; ow * oh];
    // The pixels, a band of rows to each core.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
    let band = oh.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (bi, rows) in out.chunks_mut(band * ow).enumerate() {
            let nodes = &nodes;
            scope.spawn(move || {
                for (ri, row) in rows.chunks_mut(ow).enumerate() {
                    let oy = bi * band + ri;
                    let v = oy as f32 / oh as f32;
                    let gy = oy / GRID;
                    let fy = (oy % GRID) as f32 / GRID as f32;
                    for (ox, px) in row.iter_mut().enumerate() {
                        let u = ox as f32 / ow as f32;
                        let gx = ox / GRID;
                        let fx = (ox % GRID) as f32 / GRID as f32;
                        let (n00, n10) = (&nodes[gy * gw + gx], &nodes[gy * gw + gx + 1]);
                        let (n01, n11) = (&nodes[(gy + 1) * gw + gx], &nodes[(gy + 1) * gw + gx + 1]);
                        let mut q = [0.0f32; 7];
                        for k in 0..7 {
                            let top = n00[k] + (n10[k] - n00[k]) * fx;
                            let bot = n01[k] + (n11[k] - n01[k]) * fx;
                            q[k] = top + (bot - top) * fy;
                        }
                        let [lx, ly, sx, sy, wu, wv, depth] = q;
                        // The title tilted TILT counterclockwise: this point
                        // of the screen shows the title from (tu, tv).
                        let (ox_, oy_) = ((u - 0.5) * r.width(), (v - 0.5) * r.height());
                        let tu = 0.5 + (ox_ * tilt_c - oy_ * tilt_s) / r.width();
                        let tv = 0.5 + (ox_ * tilt_s + oy_ * tilt_c) / r.height();
                        let a_l = m.at((tu - lx / r.width()) * mw, (tv - ly / r.height()) * mh);
                        let a_s = m.at((tu - sx / r.width()) * mw, (tv - sy / r.height()) * mh) * (1.0 - a_l);
                        let lit = a_l * alpha;
                        let as_ = a_s * 0.62 * alpha;
                        if lit + as_ <= 0.002 {
                            continue;
                        }
                        // The letters raise the exposure of the water under
                        // them (it is added back EXPOSE times, with a little
                        // pale lift); the shadow darkens as usual.
                        let under = match water {
                            Some(w) => {
                                let c = w.at(wu, wv);
                                [c[0] * depth * 255.0, c[1] * depth * 255.0, c[2] * depth * 255.0]
                            }
                            None => [0.0; 3],
                        };
                        let a = a_l * COVER * alpha + as_;
                        let ch = |k: usize| {
                            (under[k] * EXPOSE * lit + fill[k] * LIFT * lit + dark[k] * as_).clamp(0.0, 255.0) as u8
                        };
                        *px = Color32::from_rgba_premultiplied(ch(0), ch(1), ch(2), (a * 255.0).clamp(0.0, 255.0) as u8);
                    }
                }
            });
        }
    });
    let img = egui::ColorImage { size: [ow, oh], pixels: out };
    let id = egui::Id::new("af2-water-title");
    let ctx = p.ctx();
    let tex = match ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        Some(mut h) => {
            h.set(img, egui::TextureOptions::LINEAR);
            h
        }
        None => {
            let h = ctx.load_texture("water-title", img, egui::TextureOptions::LINEAR);
            ctx.data_mut(|d| d.insert_temp(id, h.clone()));
            h
        }
    };
    p.image(tex.id(), r, FULL, Color32::WHITE);
    if std::env::var_os("AF2_TRACE_SPLASH").is_some() {
        eprintln!("water title {ow}x{oh}: {:.1} ms", started.elapsed().as_secs_f64() * 1e3);
    }
}

/// The waves of the rendered logo at point (x, y) of the title (points),
/// scaled to its letters' height `lh`: a fine ripple about an eighth of the
/// letters' height long and a swell about half of it, as offsets (points).
/// `ph` shifts the pattern (the shadow takes its own).
fn logo_waves(x: f32, y: f32, lh: f32, t: f32, ph: f32) -> (f32, f32) {
    // As in the rendered logo: smooth random noise, not regular waves; a
    // fine ripple about a quarter of the letters' height across and a
    // swell about the height, drifting slowly.
    let (u, v) = (x / lh, y / lh);
    let seed = (ph * 1000.0) as u32;
    let fine = lh * 0.022;
    let swell = lh * 0.055;
    let n = |sx: f32, sy: f32, k: u32| value_noise(sx, sy, seed.wrapping_add(k));
    let dx = fine * n(u * 4.0 + t * 0.35, v * 4.0 - t * 0.2, 1) + swell * n(u * 1.0 - t * 0.12, v * 1.0 + t * 0.08, 2);
    let dy = fine * n(u * 4.0 - t * 0.3, v * 4.0 + t * 0.25, 3) + swell * 1.3 * n(u * 1.0 + t * 0.1, v * 1.0 - t * 0.1, 4);
    (dx, dy)
}

/// Smooth value noise in -1..1: random heights on a unit grid, blended
/// with a smoothstep between them.
pub fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (s(fx), s(fy));
    let g = |i: i32, j: i32| {
        let k = (i as u32).wrapping_mul(0x8DA6_B343) ^ (j as u32).wrapping_mul(0xD816_3841) ^ seed.wrapping_mul(0xCB1A_B31F);
        h(k) * 2.0 - 1.0
    };
    let (i, j) = (x0 as i32, y0 as i32);
    let a = g(i, j) + (g(i + 1, j) - g(i, j)) * sx;
    let b = g(i, j + 1) + (g(i + 1, j + 1) - g(i, j + 1)) * sx;
    a + (b - a) * sy
}

/// The splash title's letters: how much light they add (of their colour)
/// and how much of the water behind them they hide.
const COVER: f32 = 0.04;
/// How many times over the water under the letters is added again (1.6:
/// the letters show it at 2.6 times its brightness).
const EXPOSE: f32 = 1.6;
/// A little of the letters' pale colour on top, so dark water under them
/// still reads as a letter.
const LIFT: f32 = 0.16;

/// The splash title (and its shadow) tilted this many degrees
/// counterclockwise.
const TILT: f32 = 5.0;

/// How far the title follows the water's swell (1 would move it as far
/// as the painted water moves; the letters read better with less).
const RIDE: f32 = 0.5;

/// A finer, uneven ripple over the water at (x, y) (x in screen heights),
/// in -1..1: smooth noise a few tenths of the screen across, drifting.
fn water_ripple(x: f32, y: f32, t: f32) -> f32 {
    0.65 * value_noise(x * 9.0 + t * 0.6, y * 9.0 - t * 0.4, 71)
        + 0.35 * value_noise(x * 23.0 - t * 0.9, y * 23.0 + t * 0.7, 72)
}

/// The colour streaks under the Undercurrent splash, made once: noise
/// swirled through itself, lit along a few thin bands in orange,
/// yellow-green and cyan (black elsewhere, for adding as light).
fn flare_texture(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::new("af2-splash-flares");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    let (w, h) = (640usize, 400usize);
    let mut px = vec![Color32::BLACK; w * h];
    // Few and soft: broad bands, and only where a slow patch lets them.
    let bands: [(f32, f32, [f32; 3]); 3] = [
        (0.30, 0.055, [235.0, 105.0, 35.0]),
        (0.50, 0.045, [60.0, 205.0, 235.0]),
        (0.70, 0.060, [175.0, 235.0, 55.0]),
    ];
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / h as f32 * 2.2, y as f32 / h as f32 * 2.2);
            // Swirled: the noise looked up where other noise points.
            let q = (value_noise(u, v, 11), value_noise(u + 5.2, v + 1.3, 12));
            let r2 = (
                value_noise(u + 3.0 * q.0 + 1.7, v + 3.0 * q.1 + 9.2, 13),
                value_noise(u + 3.0 * q.0 + 8.3, v + 3.0 * q.1 + 2.8, 14),
            );
            let f = 0.5 + 0.5 * value_noise(u * 1.6 + 3.5 * r2.0, v * 1.6 + 3.5 * r2.1, 15);
            // Fine ripples along the streaks.
            let f = f + 0.012 * value_noise(u * 40.0, v * 40.0, 16);
            let patch = (value_noise(u * 0.7 + 4.0, v * 0.7 + 7.0, 17) * 1.4 - 0.15).clamp(0.0, 1.0);
            let mut c = [0.0f32; 3];
            for (centre, width, col) in bands {
                let d = (f - centre) / width;
                let k = (-d * d).exp() * patch;
                for i in 0..3 {
                    c[i] += col[i] * k;
                }
            }
            px[y * w + x] = Color32::from_rgb(c[0].min(255.0) as u8, c[1].min(255.0) as u8, c[2].min(255.0) as u8);
        }
    }
    let img = egui::ColorImage { size: [w, h], pixels: px };
    let t = ctx.load_texture("splash-flares", img, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    Some(t)
}

/// Where in the painted water picture the Undercurrent splash shows at
/// screen point (u, v) (0..1 across): a vortex winding tighter near the
/// middle, with the swell and a finer ripple over it.
fn water_uv(u: f32, v: f32, t: f32, aspect: f32) -> (f32, f32) {
    let (dx, dy) = ((u - 0.5) * aspect, v - 0.5);
    let r = (dx * dx + dy * dy).sqrt();
    let ang = (0.15 + 0.22 * t) * (-r * 2.4).exp() + 0.04 * (t * 0.7 + r * 6.0).sin();
    let (s_, c_) = ang.sin_cos();
    let (rx, ry) = (dx * c_ - dy * s_, dx * s_ + dy * c_);
    let swell = water_swell(u, v, t) + water_ripple(u * aspect, v, t) * 0.0035;
    (0.5 + rx / aspect * 0.86 + swell, 0.5 + ry * 0.86 + swell * 0.7)
}

/// A picture's pixels on the CPU (RGB, 0..1), decoded once at about
/// `w` wide: the splash title samples the water under it.
struct Pixels {
    w: usize,
    h: usize,
    px: &'static [[f32; 3]],
}

fn cpu_picture(name: &str, w: u32) -> Option<&'static Pixels> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, Option<&'static Pixels>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut c = cache.lock().ok()?;
    *c.entry(name.to_string()).or_insert_with(|| {
        let img = image::load_from_memory(crate::pictures::bytes(name)?).ok()?;
        let img = img.resize(w, u32::MAX, image::imageops::FilterType::Triangle).to_rgb8();
        let px: Vec<[f32; 3]> = img
            .pixels()
            .map(|p| [p.0[0] as f32 / 255.0, p.0[1] as f32 / 255.0, p.0[2] as f32 / 255.0])
            .collect();
        let out = Pixels { w: img.width() as usize, h: img.height() as usize, px: Box::leak(px.into_boxed_slice()) };
        Some(&*Box::leak(Box::new(out)))
    })
}

impl Pixels {
    /// Bilinear at (u, v) in 0..1, the edges held.
    fn at(&self, u: f32, v: f32) -> [f32; 3] {
        let x = (u * (self.w - 1) as f32).clamp(0.0, (self.w - 2) as f32);
        let y = (v * (self.h - 1) as f32).clamp(0.0, (self.h - 2) as f32);
        let (x0, y0) = (x as usize, y as usize);
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let i = y0 * self.w + x0;
        let mut o = [0.0; 3];
        for k in 0..3 {
            let top = self.px[i][k] * (1.0 - fx) + self.px[i + 1][k] * fx;
            let bot = self.px[i + self.w][k] * (1.0 - fx) + self.px[i + self.w + 1][k] * fx;
            o[k] = top * (1.0 - fy) + bot * fy;
        }
        o
    }
}

/// The gentle swell over the Undercurrent water at screen point (u, v)
/// (0..1 across), as an offset into the picture (its x; y takes 0.7 of it).
fn water_swell(u: f32, v: f32, t: f32) -> f32 {
    0.006 * (v * 11.0 + t * 1.3).sin() + 0.004 * (u * 17.0 - t * 1.1).cos()
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
        warped(p, tex.id(), screen, 128, 80, |u, v| {
            let pos = Pos2::new(
                screen.left() + u * screen.width(),
                screen.top() + v * screen.height(),
            );
            let (wu, wv) = water_uv(u, v, t, aspect);
            (pos, Pos2::new(wu, wv), tint)
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
    // Colour flaring through the water: thin bright streaks, as of oil
    // on it, drifting with the swell; added light, so they brighten it.
    if let Some(tex) = flare_texture(p.ctx()) {
        let aspect = screen.width() / screen.height().max(1.0);
        let k = (a * 120.0) as u8;
        let tint = Color32::from_rgba_premultiplied(k, k, k, 0);
        warped(p, tex.id(), screen, 96, 60, |u, v| {
            let pos = Pos2::new(screen.left() + u * screen.width(), screen.top() + v * screen.height());
            let s = water_swell(u, v, t) + water_ripple(u * aspect, v, t + 3.0) * 0.004;
            let uv = Pos2::new(0.08 + u * 0.84 + s * 1.6 + t * 0.012, 0.08 + v * 0.84 + s * 1.1 - t * 0.008);
            (pos, uv, tint)
        });
    }
    // Light playing through the water, under the title.
    if th.caustics > 0.0 {
        let time = if th.motion > 0.0 { t * 0.5 } else { 0.0 };
        crate::caustics::paint(
            p,
            screen,
            th,
            time,
            // Softer than on the backdrop: no glass lies over it here.
            th.caustics * a * 0.55,
            crate::caustics::SLOT_SPLASH,
        );
    }

    // The title rises from the deep: dim, blue and bent by the water at
    // first, clearing and steadying as it reaches the surface.
    // A theme with a letter mask for its title (`<title>-mask`) has the
    // title built each frame on the water, its shadow moving on its own.
    let mask = water_mask(&format!("{}-mask", th.title_picture));
    if let Some(tex) = crate::pictures::texture(p.ctx(), &th.title_picture, f32::MAX) {
        let [tw, tht] = match mask {
            Some(m) => [m.w as f32, m.h as f32],
            None => tex.size_vec2().into(),
        };
        let rise = ease((t - 0.15) / 1.9);
        let w = (screen.width() * 0.44).min(tw * 1.7) * (0.82 + 0.18 * rise);
        let hgt = w * tht / tw;
        let r = Rect::from_center_size(c - Vec2::new(0.0, 20.0), Vec2::new(w, hgt));
        let bend = 14.0 * (1.0 - rise).powf(1.3);
        let deep: Rgb = [40, 110, 160];
        let col = mix(deep, [255, 255, 255], rise);
        let alpha = (rise.powf(1.4) * a).clamp(0.0, 1.0);
        let tint = rgba(col, alpha);
        // The title, riding the water. It rides the
        // same swell as the water under it (that swell moves the painted
        // water by `water_swell` of the picture, here turned to points),
        // with the rising bend on top while it is still under the surface.
        if let Some(m) = mask {
            // Pale cyan light, as the cover's letters.
            let water = cpu_picture(&th.picture, 960);
            water_title(p, screen, r, t, bend, m, mix(col, [150, 215, 240], 0.55), alpha, water);
            return;
        }
        warped(p, tex.id(), r, 60, 20, |u, v| {
            let x = r.left() + u * r.width();
            let y = r.top() + v * r.height();
            let (su, sv) = (
                (x - screen.left()) / screen.width(),
                (y - screen.top()) / screen.height(),
            );
            let s = water_swell(su, sv, t);
            let ride_x = -s / 0.86 * screen.width();
            let ride_y = -s * 0.7 / 0.86 * screen.height();
            let dx = bend * (v * 8.0 + t * 2.6 + u * 3.0).sin() + ride_x;
            let dy = bend * 0.5 * (u * 6.5 + t * 2.1).cos() + ride_y;
            (Pos2::new(x + dx, y + dy), Pos2::new(u, v), tint)
        });
    }
}

// ------------------------------------------------------- Letter pop

/// The letter pictures of a `pop:<name>` splash: `<name>-1.png`,
/// `<name>-2.png`, ... up to the first missing one. Each picture is the
/// whole wordmark's size with its letter alone on it, so laid over each
/// other they make the word.
fn pop_letters(prefix: &str) -> Vec<String> {
    (1..=24)
        .map(|i| format!("{prefix}-{i}.png"))
        .take_while(|n| crate::pictures::bytes(n).is_some())
        .collect()
}

/// Where a letter sits on its picture (0..1 across and down): the middle
/// of what it covers, found once from the picture's opaque pixels.
fn letter_centre(name: &str) -> Pos2 {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CENTRES: OnceLock<Mutex<HashMap<String, Pos2>>> = OnceLock::new();
    let Ok(mut map) = CENTRES.get_or_init(Default::default).lock() else {
        return Pos2::new(0.5, 0.5);
    };
    *map.entry(name.to_string()).or_insert_with(|| {
        let img = crate::pictures::bytes(name)
            .and_then(|b| image::load_from_memory(b).ok())
            .map(|i| i.to_rgba8());
        let Some(img) = img else { return Pos2::new(0.5, 0.5) };
        let (w, hh) = img.dimensions();
        let (mut x0, mut y0, mut x1, mut y1) = (w, hh, 0, 0);
        for (x, y, px) in img.enumerate_pixels() {
            if px[3] > 200 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
        if x1 < x0 {
            return Pos2::new(0.5, 0.5);
        }
        Pos2::new(
            (x0 + x1) as f32 / 2.0 / w as f32,
            (y0 + y1) as f32 / 2.0 / hh as f32,
        )
    })
}

/// Its size through a pop: from nothing up past full size, back under it,
/// and settling (a bubble blown up), over `d` seconds.
fn pop_scale(s: f32, d: f32) -> f32 {
    if s <= 0.0 {
        return 0.0;
    }
    let x = (s / d).min(1.0);
    if x >= 1.0 {
        return 1.0;
    }
    // A damped spring from 0 to 1.
    1.0 - (-6.5 * x).exp() * (x * 10.5).cos() * (1.0 - x).powf(0.3)
}

/// A splash that blows up a theme's lettering one letter at a time
/// (`splash: "pop:<name>"`, letters in `<name>-1.png`... beside the theme)
/// over its backdrop picture, each with a little bubble-burst ring.
pub fn pop(p: &Painter, screen: Rect, th: &Theme, t: f32, fade: f32, prefix: &str) {
    let a = fade;
    p.rect_filled(screen, 0.0, rgba(th.backdrop_bottom, a));
    let ppp = p.ctx().pixels_per_point();
    // The theme's picture, covering the window and creeping closer.
    if let Some(tex) = crate::pictures::texture(p.ctx(), &th.picture, screen.width() * ppp) {
        let [tw, tht] = tex.size_vec2().into();
        let (ra, pa) = (screen.width() / screen.height().max(1.0), tw / tht.max(1.0));
        let zoom = 1.0 / (1.0 + 0.04 * ease(t / LEN));
        let (mut uw, mut vh) = if ra > pa { (1.0, pa / ra) } else { (ra / pa, 1.0) };
        uw *= zoom;
        vh *= zoom;
        let uv = Rect::from_center_size(Pos2::new(0.5, 0.5), Vec2::new(uw, vh));
        let tint = Color32::from_white_alpha((a * 255.0) as u8);
        if th.wind > 0.0 {
            // The hedge in the breeze, as behind the app.
            let now = p.ctx().input(|i| i.time) as f32;
            if !crate::wind::draw(p, screen, uv, &th.picture, th.wind, now, tint) {
                p.image(tex.id(), screen, uv, tint);
            }
        } else {
            p.image(tex.id(), screen, uv, tint);
        }
        p.rect_filled(screen, 0.0, rgba(th.backdrop_top, 0.35 * a));
    }
    let letters = pop_letters(prefix);
    let Some(first) = letters
        .first()
        .and_then(|n| crate::pictures::texture(p.ctx(), n, f32::MAX))
    else {
        return;
    };
    let [lw, lh] = first.size_vec2().into();
    let w = (screen.width() * 0.62).min(screen.height() * 0.78 * lw / lh.max(1.0));
    let word = Rect::from_center_size(screen.center(), Vec2::new(w, w * lh / lw.max(1.0)));
    let (start, step, dur) = (0.25, 0.12, 0.55);
    for (i, name) in letters.iter().enumerate() {
        let s = t - start - step * i as f32;
        let k = pop_scale(s, dur);
        if k <= 0.0 {
            continue;
        }
        let Some(tex) = crate::pictures::texture(p.ctx(), name, f32::MAX) else {
            continue;
        };
        let c = letter_centre(name);
        let pivot = Pos2::new(
            word.left() + c.x * word.width(),
            word.top() + c.y * word.height(),
        );
        let r = Rect::from_min_max(
            pivot + (word.min - pivot) * k,
            pivot + (word.max - pivot) * k,
        );
        let alpha = (s / 0.08).clamp(0.0, 1.0) * a;
        p.image(tex.id(), r, FULL, Color32::from_white_alpha((alpha * 255.0) as u8));
        // The bubble bursting: specks fly out and fade.
        let burst = s / 0.35;
        if (0.0..1.0).contains(&burst) {
            let rad = word.height() * (0.06 + 0.22 * ease(burst));
            for j in 0..6 {
                let ang = j as f32 / 6.0 * std::f32::consts::TAU + h(i as u32 * 7 + j) * 0.8;
                let d = rad * (1.15 + 0.25 * h(i as u32 * 13 + j));
                p.circle_filled(
                    pivot + Vec2::new(ang.cos(), ang.sin()) * d,
                    2.5 * (1.0 - burst),
                    rgba(th.accent, (1.0 - burst) * a),
                );
            }
        }
    }
    // Once the word is whole, a shine sweeps across it.
    let whole = start + step * (letters.len() as f32 - 1.0) + dur;
    // An even sweep from wholly off the left edge to wholly off the right.
    let k = (t - whole + 0.1) / 0.9;
    if (0.0..1.0).contains(&k) {
        if let Some(tex) = white_word(p.ctx(), &letters) {
            shine(p, tex.id(), word, -0.4 + k * 1.8, a);
        }
    }
}

/// The letters together, all white (their own shapes), for the shine.
fn white_word(ctx: &egui::Context, letters: &[String]) -> Option<egui::TextureHandle> {
    let id = egui::Id::new(("af2-pop-white", letters.first()?.clone()));
    if let Some(t) = ctx.data(|d| d.get_temp::<Option<egui::TextureHandle>>(id)) {
        return t;
    }
    let mut out: Option<image::RgbaImage> = None;
    for name in letters {
        let Some(img) = crate::pictures::bytes(name).and_then(|b| image::load_from_memory(b).ok()) else {
            continue;
        };
        // Half size is plenty for a soft shine.
        let img = img
            .resize(img.width() / 2, img.height() / 2, image::imageops::FilterType::Triangle)
            .to_rgba8();
        let o = out.get_or_insert_with(|| image::RgbaImage::new(img.width(), img.height()));
        if o.dimensions() != img.dimensions() {
            continue;
        }
        for (dst, src) in o.pixels_mut().zip(img.pixels()) {
            dst.0 = [255, 255, 255, dst.0[3].max(src.0[3])];
        }
    }
    let tex = out.map(|o| {
        let size = [o.width() as usize, o.height() as usize];
        ctx.load_texture(
            "pop-white",
            egui::ColorImage::from_rgba_unmultiplied(size, o.as_raw()),
            egui::TextureOptions::LINEAR,
        )
    });
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

/// A soft, slanted band of light at `pos` (0 left .. 1 right) over the
/// white word `tex` laid on `r`: only the letters catch it.
fn shine(p: &Painter, tex: egui::TextureId, r: Rect, pos: f32, a: f32) {
    let (nx, ny) = (64u32, 16u32);
    let mut m = Mesh::with_texture(tex);
    for j in 0..=ny {
        for i in 0..=nx {
            let (u, v) = (i as f32 / nx as f32, j as f32 / ny as f32);
            // Slanted: the band leans forward toward the top.
            let d = (u + (1.0 - v) * 0.35 - 0.175 - pos) / 0.09;
            let glow = (-d * d).exp() * 0.38 * a;
            m.vertices.push(egui::epaint::Vertex {
                pos: Pos2::new(r.left() + u * r.width(), r.top() + v * r.height()),
                uv: Pos2::new(u, v),
                color: Color32::from_white_alpha((glow * 255.0) as u8),
            });
        }
    }
    let w = nx + 1;
    for j in 0..ny {
        for i in 0..nx {
            let k = j * w + i;
            m.add_triangle(k, k + 1, k + w + 1);
            m.add_triangle(k, k + w + 1, k + w);
        }
    }
    p.add(Shape::mesh(m));
}
