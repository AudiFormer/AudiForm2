// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The pictures themes can put behind everything, built into the app.
//! Taken from Audiform's theme files (see `assets/themes/README.md` for
//! where each comes from). Decoded the first time a theme shows one, then
//! kept on the GPU.

use egui::{Context, TextureHandle, TextureOptions, TextureWrapMode};

pub fn bytes(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "moonlight" => include_bytes!("../../assets/themes/moonlight.jpg"),
        "prism" => include_bytes!("../../assets/themes/prism.jpg"),
        "water" => include_bytes!("../../assets/themes/water.jpg"),
        "walnut" => include_bytes!("../../assets/themes/walnut.jpg"),
        "marble-day" => include_bytes!("../../assets/themes/marble-day.jpg"),
        "marble-night" => include_bytes!("../../assets/themes/marble-night.jpg"),
        "mall-day-capital" => include_bytes!("../../assets/themes/mall-day-capital.png"),
        "mall-day-shaft" => include_bytes!("../../assets/themes/mall-day-shaft.png"),
        "mall-day-base" => include_bytes!("../../assets/themes/mall-day-base.png"),
        "mall-day-meander" => include_bytes!("../../assets/themes/mall-day-meander.png"),
        "mall-night-capital" => include_bytes!("../../assets/themes/mall-night-capital.png"),
        "mall-night-shaft" => include_bytes!("../../assets/themes/mall-night-shaft.png"),
        "mall-night-base" => include_bytes!("../../assets/themes/mall-night-base.png"),
        "mall-night-meander" => include_bytes!("../../assets/themes/mall-night-meander.png"),
        "mall-sun" => include_bytes!("../../assets/themes/mall-sun.png"),
        "mall-moon" => include_bytes!("../../assets/themes/mall-moon.png"),
        "title-undercurrent" => include_bytes!("../../assets/themes/title-undercurrent.png"),
        // AUDIFORM as on the water: warped (the menu bar), and its straight
        // letters as a mask, which the splash warps live with the water.
        "title-audiform" => include_bytes!("../../assets/themes/title-audiform.png"),
        "title-audiform-mask" => include_bytes!("../../assets/themes/title-audiform-mask.png"),
        "mall-day-tiles" => include_bytes!("../../assets/themes/mall-day-tiles.png"),
        "mall-night-tiles" => include_bytes!("../../assets/themes/mall-night-tiles.png"),
        "mall-day-real-capital" => include_bytes!("../../assets/themes/mall-day-real-capital.png"),
        "mall-day-real-shaft" => include_bytes!("../../assets/themes/mall-day-real-shaft.png"),
        "mall-day-real-base" => include_bytes!("../../assets/themes/mall-day-real-base.png"),
        "mall-night-real-capital" => {
            include_bytes!("../../assets/themes/mall-night-real-capital.png")
        }
        "mall-night-real-shaft" => include_bytes!("../../assets/themes/mall-night-real-shaft.png"),
        "mall-night-real-base" => include_bytes!("../../assets/themes/mall-night-real-base.png"),
        _ => return user_file(name),
    })
}

static EMBEDDED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, &'static [u8]>>> =
    std::sync::OnceLock::new();

/// A picture carried inside a user's theme file (its `assets`), by name.
fn embedded(name: &str) -> Option<&'static [u8]> {
    EMBEDDED.get()?.lock().ok()?.get(name).copied()
}

/// Keeps the pictures a theme file carries (`"assets": {name: data}`,
/// each a `data:image/...;base64,` URI or plain base64), so the theme can
/// name them like built-in pictures. Kept for the run of the app.
pub fn register_assets(assets: &serde_json::Map<String, serde_json::Value>) {
    let Ok(mut map) = EMBEDDED.get_or_init(Default::default).lock() else {
        return;
    };
    for (name, v) in assets {
        let Some(text) = v.as_str() else { continue };
        let b64 = text.split_once(";base64,").map_or(text, |(_, d)| d);
        if map.contains_key(name) {
            continue;
        }
        if let Some(bytes) = af2_vst3::protocol::base64_decode(b64.trim()) {
            map.insert(name.clone(), &*Box::leak(bytes.into_boxed_slice()));
        }
    }
}

/// A picture file beside the user's own themes (in the themes folder),
/// named with its extension: `"my-wall.jpg"`. Read once and kept for the
/// run of the app. Plain file names only, nothing outside that folder.
fn user_file(name: &str) -> Option<&'static [u8]> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    let plain = !name.is_empty()
        && !name.contains(['/', '\\', ':'])
        && !name.starts_with('.')
        && [".png", ".jpg", ".jpeg", ".webp"]
            .iter()
            .any(|e| name.to_ascii_lowercase().ends_with(e));
    if !plain {
        return None;
    }
    if let Some(b) = embedded(name) {
        return Some(b);
    }
    static FILES: OnceLock<Mutex<HashMap<String, Option<&'static [u8]>>>> = OnceLock::new();
    let mut files = FILES.get_or_init(Default::default).lock().ok()?;
    *files.entry(name.to_string()).or_insert_with(|| {
        std::fs::read(crate::theme::user_dir().join(name))
            .ok()
            .map(|b| &*Box::leak(b.into_boxed_slice()))
    })
}

/// A picture being decoded on a worker thread, then its texture.
#[derive(Clone)]
enum Slot {
    Decoding(std::sync::Arc<std::sync::Mutex<Option<Option<egui::ColorImage>>>>),
    Ready(Option<TextureHandle>),
}

/// The picture as a texture no wider than `want_px` needs (rounded up to
/// a step, so resizing the window rarely decodes again). Small screens and
/// slow GPUs never decode or hold a 4K picture they cannot show.
///
/// Decoding runs on its own thread so the window never stalls: until it is
/// done this returns None and the theme's gradient shows instead. None
/// too for an unknown name or a picture that will not decode.
/// Tiling textures repeat; a single picture (a title, the sun, a column's
/// capital, a backdrop) clamps at its edges, so smoothing at its border
/// never pulls in the opposite edge as a faint line.
pub fn wrap_for(name: &str) -> TextureWrapMode {
    let tiles = ["shaft", "meander", "tiles", "marble", "walnut"];
    if tiles.iter().any(|t| name.contains(t)) {
        TextureWrapMode::Repeat
    } else {
        TextureWrapMode::ClampToEdge
    }
}

pub fn texture(ctx: &Context, name: &str, want_px: f32) -> Option<TextureHandle> {
    let step = [960u32, 1280, 1920, 2560, 3840]
        .into_iter()
        .find(|&s| s as f32 >= want_px)
        .unwrap_or(u32::MAX);
    let id = egui::Id::new(("af2-picture", name, step));
    match ctx.data(|d| d.get_temp::<Slot>(id)) {
        Some(Slot::Ready(t)) => t,
        Some(Slot::Decoding(job)) => {
            let done = job.lock().ok().and_then(|mut g| g.take());
            match done {
                Some(pixels) => {
                    let opts = TextureOptions {
                        wrap_mode: wrap_for(name),
                        ..TextureOptions::LINEAR
                    };
                    let tex = pixels
                        .map(|px| ctx.load_texture(format!("picture-{name}-{step}"), px, opts));
                    ctx.data_mut(|d| d.insert_temp(id, Slot::Ready(tex.clone())));
                    tex
                }
                None => None,
            }
        }
        None => {
            let b = bytes(name)?;
            let job = std::sync::Arc::new(std::sync::Mutex::new(None));
            ctx.data_mut(|d| d.insert_temp(id, Slot::Decoding(job.clone())));
            let ctx2 = ctx.clone();
            let spawned = std::thread::Builder::new()
                .name("picture".into())
                .spawn(move || {
                    let pixels = decode(b, step);
                    if let Ok(mut g) = job.lock() {
                        *g = Some(pixels);
                    }
                    ctx2.request_repaint();
                });
            if spawned.is_err() {
                // No thread to be had: decode here instead.
                let tex = decode(b, step).map(|px| {
                    ctx.load_texture(format!("picture-{name}-{step}"), px, TextureOptions::LINEAR)
                });
                ctx.data_mut(|d| d.insert_temp(id, Slot::Ready(tex.clone())));
                return tex;
            }
            None
        }
    }
}

/// Decodes a picture and shrinks it to at most `max_w` pixels wide.
fn decode(b: &[u8], max_w: u32) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(b).ok()?;
    let img = if img.width() > max_w {
        img.resize(max_w, u32::MAX, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let img = img.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn titles_clamp_and_tiles_repeat() {
        use egui::TextureWrapMode::*;
        assert_eq!(super::wrap_for("title-undercurrent"), ClampToEdge);
        assert_eq!(super::wrap_for("mall-sun"), ClampToEdge);
        assert_eq!(super::wrap_for("mall-day-real-capital"), ClampToEdge);
        assert_eq!(super::wrap_for("mall-day-real-shaft"), Repeat);
        assert_eq!(super::wrap_for("marble-night"), Repeat);
        assert_eq!(super::wrap_for("mall-night-tiles"), Repeat);
    }

    #[test]
    fn every_picture_decodes_and_shrinks_for_small_screens() {
        for n in [
            "moonlight",
            "prism",
            "water",
            "walnut",
            "marble-day",
            "marble-night",
        ] {
            let b = super::bytes(n).unwrap();
            let full = super::decode(b, u32::MAX).unwrap();
            assert!(full.size[0] >= 2048, "{n} is {:?}", full.size);
            let small = super::decode(b, 960).unwrap();
            assert!(small.size[0] <= 960, "{n}");
        }
    }
}
