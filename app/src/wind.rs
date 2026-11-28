// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Wind in a theme's backdrop picture (`Theme::wind`): foliage in a breeze.
//!
//! Warping the whole picture makes it wobble like jelly, deep shadows and
//! all. Foliage in wind holds still in its depths while the sprig tips that
//! catch the light shift and flicker. So the picture is split once into two
//! layers: the canopy (the picture with its small bright details softened),
//! which stays put, and the tips (just those details, on clear), which
//! flutter a few pixels and twinkle brighter and dimmer in patches.

use egui::epaint::{Mesh, Vertex};
use egui::{Color32, ColorImage, Context, Painter, Pos2, Rect, Shape, TextureHandle, TextureOptions};
use std::sync::{Arc, Mutex};

/// The two layers of a picture, ready or still being made.
#[derive(Clone)]
enum Slot {
    Making(Arc<Mutex<Option<Option<(ColorImage, ColorImage)>>>>),
    Ready(Option<(TextureHandle, TextureHandle)>),
}

/// The canopy and tips textures for picture `name` at about `want_px`
/// wide; None until they are made (on a worker thread) or if they can't be.
fn layers(ctx: &Context, name: &str, want_px: f32) -> Option<(TextureHandle, TextureHandle)> {
    let w = [960u32, 1280, 1920].into_iter().find(|&s| s as f32 >= want_px).unwrap_or(1920);
    let id = egui::Id::new(("af2-wind-layers", name, w));
    match ctx.data(|d| d.get_temp::<Slot>(id)) {
        Some(Slot::Ready(t)) => t,
        Some(Slot::Making(job)) => {
            let done = job.lock().ok().and_then(|mut g| g.take())?;
            let tex = done.map(|(canopy, tips)| {
                (
                    ctx.load_texture(format!("wind-canopy-{name}-{w}"), canopy, TextureOptions::LINEAR),
                    ctx.load_texture(format!("wind-tips-{name}-{w}"), tips, TextureOptions::LINEAR),
                )
            });
            ctx.data_mut(|d| d.insert_temp(id, Slot::Ready(tex.clone())));
            tex
        }
        None => {
            let bytes = crate::pictures::bytes(name)?;
            let job = Arc::new(Mutex::new(None));
            ctx.data_mut(|d| d.insert_temp(id, Slot::Making(job.clone())));
            let ctx2 = ctx.clone();
            let _ = std::thread::Builder::new().name("wind".into()).spawn(move || {
                let made = split(bytes, w);
                if let Ok(mut g) = job.lock() {
                    *g = Some(made);
                }
                ctx2.request_repaint();
            });
            None
        }
    }
}

/// A box blur of `src` (w x h, `ch` floats a pixel) with radius `r`, both
/// ways, twice (close to a Gaussian).
fn blur(src: &[f32], w: usize, h: usize, ch: usize, r: usize) -> Vec<f32> {
    let mut a = src.to_vec();
    let mut b = vec![0.0f32; a.len()];
    let n = (2 * r + 1) as f32;
    for _ in 0..2 {
        // Across.
        for y in 0..h {
            for c in 0..ch {
                let at = |x: isize| a[(y * w + x.clamp(0, w as isize - 1) as usize) * ch + c];
                let mut s: f32 = (-(r as isize)..=r as isize).map(at).sum();
                for x in 0..w {
                    b[(y * w + x) * ch + c] = s / n;
                    s += at(x as isize + r as isize + 1) - at(x as isize - r as isize);
                }
            }
        }
        // Down.
        for x in 0..w {
            for c in 0..ch {
                let at = |y: isize| b[(y.clamp(0, h as isize - 1) as usize * w + x) * ch + c];
                let mut s: f32 = (-(r as isize)..=r as isize).map(at).sum();
                for y in 0..h {
                    a[(y * w + x) * ch + c] = s / n;
                    s += at(y as isize + r as isize + 1) - at(y as isize - r as isize);
                }
            }
        }
    }
    a
}

/// The picture split into the still canopy and the moving tips.
fn split(bytes: &[u8], width: u32) -> Option<(ColorImage, ColorImage)> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width() > width { img.resize(width, u32::MAX, image::imageops::FilterType::Triangle) } else { img };
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let px: Vec<f32> = rgb.as_raw().iter().map(|&v| v as f32 / 255.0).collect();
    // Sprig-sized detail: what stands out from its surroundings a few
    // pixels across.
    let r = (w / 320).max(2);
    let soft = blur(&px, w, h, 3, r);
    let mut canopy = Vec::with_capacity(w * h);
    let mut tips = Vec::with_capacity(w * h);
    for i in 0..w * h {
        let (c, s) = (&px[i * 3..i * 3 + 3], &soft[i * 3..i * 3 + 3]);
        let l = 0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2];
        let ls = 0.3 * s[0] + 0.59 * s[1] + 0.11 * s[2];
        // Brighter than around it: a tip catching the light.
        let k = ((l - ls - 0.01) / 0.07).clamp(0.0, 1.0);
        let k = k * k * (3.0 - 2.0 * k);
        let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
        // The canopy: the tip softened into what is around it.
        canopy.push(Color32::from_rgb(
            to8(c[0] + (s[0] - c[0]) * k),
            to8(c[1] + (s[1] - c[1]) * k),
            to8(c[2] + (s[2] - c[2]) * k),
        ));
        // The tip on clear (premultiplied).
        tips.push(Color32::from_rgba_premultiplied(to8(c[0] * k), to8(c[1] * k), to8(c[2] * k), to8(k)));
    }
    Some((ColorImage { size: [w, h], pixels: canopy }, ColorImage { size: [w, h], pixels: tips }))
}

/// Draws picture `name` (texture points `uv`) over `rect` in a wind of
/// strength `wind`, `t` seconds of real time, tinted `tint`. False while
/// its layers are still being made (draw the plain picture meanwhile).
pub fn draw(p: &Painter, rect: Rect, uv: Rect, name: &str, wind: f32, t: f32, tint: Color32) -> bool {
    let want = rect.width() * p.ctx().pixels_per_point();
    let Some((canopy, tips)) = layers(p.ctx(), name, want) else {
        return false;
    };
    p.image(canopy.id(), rect, uv, tint);
    let t = t % 10_000.0;
    let noise = crate::splash::value_noise;
    let wind = wind.clamp(0.0, 2.0);
    // Gusts: the breeze rises and falls over a few seconds.
    let gust = 0.6 + 0.4 * noise(t * 0.25, 3.7, 901);
    let aspect = rect.width() / rect.height().max(1.0);
    // Movement in picture terms: a few pixels at most.
    let amp = 0.005 * wind * gust;
    // Small patches (about 1.8% of the width), so neighbouring sprigs move
    // each their own way: rustling, not a wave passing over.
    let f = 55.0 / aspect.max(0.1);
    let (nx, ny) = (300u32, (300.0 / aspect).clamp(80.0, 320.0) as u32);
    let [tr, tg, tb, ta] = tint.to_array();
    let mut m = Mesh::with_texture(tips.id());
    for j in 0..=ny {
        for i in 0..=nx {
            let (u, v) = (i as f32 / nx as f32, j as f32 / ny as f32);
            let (x, y) = (u * aspect * f, v * f);
            // A steady jostle, and a quicker flick on top.
            let du = 0.55 * noise(x + t * 1.9, y - t * 0.6, 911) + 0.25 * noise(x * 0.9 - t * 1.6, y * 0.9 + t * 1.3, 912)
                + 0.35 * noise(x * 1.3 + t * 4.2, y * 1.3, 918);
            let dv = 0.55 * noise(x - t * 1.8, y + t * 0.7, 914) + 0.25 * noise(x * 0.9 + t * 1.5, y * 0.9 - t * 1.2, 915)
                + 0.35 * noise(x * 1.3, y * 1.3 - t * 4.0, 919);
            // The tips turn in and out of the light: brighter and dimmer
            // in patches, more so in a gust.
            let glint = (0.8 + 0.7 * wind * gust * noise(x * 1.1 + t * 2.6, y * 1.1 - t * 2.0, 917)).clamp(0.15, 1.6);
            let k = |c: u8| ((c as f32 * glint).min(255.0)) as u8;
            let pu = u + du * amp;
            let pv = v + dv * amp * aspect;
            m.vertices.push(Vertex {
                pos: Pos2::new(rect.left() + u * rect.width(), rect.top() + v * rect.height()),
                uv: Pos2::new(uv.left() + pu * uv.width(), uv.top() + pv * uv.height()),
                color: Color32::from_rgba_premultiplied(k(tr), k(tg), k(tb), ((ta as f32) * glint.min(1.0)) as u8),
            });
        }
    }
    let row = nx + 1;
    for j in 0..ny {
        for i in 0..nx {
            let a = j * row + i;
            m.add_triangle(a, a + 1, a + row + 1);
            m.add_triangle(a, a + row + 1, a + row);
        }
    }
    p.add(Shape::mesh(m));
    true
}
