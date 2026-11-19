// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! For documentation and the website: `AF2_SCREENSHOT=<file.png>` makes the
//! app photograph its own window a few seconds after start (exact pixels,
//! straight from the renderer), save it, and quit. `AF2_SCREENSHOT_AFTER`
//! sets the delay in seconds (default 6). `AF2_WINDOW=WxH` sets the window's
//! size at start.

use std::sync::atomic::{AtomicBool, Ordering};

static ASKED: AtomicBool = AtomicBool::new(false);

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
        let [w, h] = img.size;
        let mut bytes = Vec::with_capacity(w * h * 4);
        for p in &img.pixels {
            bytes.extend_from_slice(&p.to_srgba_unmultiplied());
        }
        if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
            let _ = buf.save(&path);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}
