// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! For documentation and the website: `AF2_SCREENSHOT=<file.png>` makes the
//! app photograph its own window a few seconds after start (exact pixels,
//! straight from the renderer), save it, and quit. `AF2_SCREENSHOT_AFTER`
//! sets the delay in seconds (default 6). `AF2_WINDOW=WxH` sets the window's
//! size at start. `AF2_SCREENSHOT_FRAMES=n` saves n frames in a row
//! (`<file>-0.png`, `<file>-1.png`...) instead of one, to catch flicker.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static ASKED: AtomicBool = AtomicBool::new(false);
static TAKEN: AtomicUsize = AtomicUsize::new(0);

/// The window size asked for with `AF2_WINDOW`, if any.
pub fn window_size() -> Option<[f32; 2]> {
    let v = std::env::var("AF2_WINDOW").ok()?;
    let (w, h) = v.split_once('x')?;
    Some([w.trim().parse().ok()?, h.trim().parse().ok()?])
}

/// Call once per frame with the seconds since start.
pub fn tick(ctx: &egui::Context, since_start: f64) {
    let Ok(path) = std::env::var("AF2_SCREENSHOT") else {
        return;
    };
    let after: f64 = std::env::var("AF2_SCREENSHOT_AFTER")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6.0);
    // Keep frames coming so the shot is taken on time.
    ctx.request_repaint();
    if since_start >= after && !ASKED.swap(true, Ordering::Relaxed) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
    }
    let shot = ctx.input(|i| {
        i.raw.events.iter().find_map(|e| match e {
            egui::Event::Screenshot { image, .. } => Some(image.clone()),
            _ => None,
        })
    });
    if let Some(img) = shot {
        let frames: usize = std::env::var("AF2_SCREENSHOT_FRAMES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1)
            .max(1);
        let i = TAKEN.fetch_add(1, Ordering::Relaxed);
        let path = if frames > 1 {
            let p = std::path::Path::new(&path);
            p.with_file_name(format!(
                "{}-{i}.png",
                p.file_stem().and_then(|s| s.to_str()).unwrap_or("shot")
            ))
            .to_string_lossy()
            .into_owned()
        } else {
            path
        };
        let [w, h] = img.size;
        let mut bytes = Vec::with_capacity(w * h * 4);
        for p in &img.pixels {
            bytes.extend_from_slice(&p.to_srgba_unmultiplied());
        }
        if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
            let _ = buf.save(&path);
        }
        if i + 1 >= frames {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
    }
}
