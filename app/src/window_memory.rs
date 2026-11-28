// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The main window's size, place and whether it was maximized, kept from
//! one run to the next (window.json beside the preferences).

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::state::data_dir;

/// The window as it was left (points; the unmaximized size and place).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowMemory {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub maximized: bool,
}

/// The smallest window the app opens at.
pub const MIN: [f32; 2] = [900.0, 560.0];

impl WindowMemory {
    fn path() -> std::path::PathBuf {
        data_dir().join("window.json")
    }

    pub fn load() -> Option<WindowMemory> {
        let m: WindowMemory = serde_json::from_str(&std::fs::read_to_string(Self::path()).ok()?).ok()?;
        m.sane().then_some(m)
    }

    fn save(&self) {
        let _ = std::fs::create_dir_all(data_dir());
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), s);
        }
    }

    /// Worth restoring: a real size, and a place not wildly off any screen.
    fn sane(&self) -> bool {
        let finite = [self.x, self.y, self.w, self.h].iter().all(|v| v.is_finite());
        finite
            && self.w >= MIN[0] * 0.5
            && self.h >= MIN[1] * 0.5
            && self.w < 20_000.0
            && self.h < 20_000.0
            && self.x.abs() < 20_000.0
            && self.y.abs() < 20_000.0
    }

    /// The size to open at, never under the minimum.
    pub fn size(&self) -> [f32; 2] {
        [self.w.max(MIN[0]), self.h.max(MIN[1])]
    }
}

/// Follows the window each frame and writes it down once it settles.
#[derive(Default)]
pub struct Tracker {
    now: Option<WindowMemory>,
    saved: Option<WindowMemory>,
    changed: Option<Instant>,
    frames: u32,
    /// Left maximized last time: maximized once the window is up (asking
    /// for it when the window is made does nothing on Windows).
    restore_max: bool,
    /// Screenshot runs set their own size: nothing is kept from them.
    off: bool,
}

impl Tracker {
    pub fn new() -> Tracker {
        let saved = WindowMemory::load();
        Tracker {
            restore_max: saved.is_some_and(|m| m.maximized),
            saved,
            off: std::env::var_os("AF2_WINDOW").is_some() || std::env::var_os("AF2_SCREENSHOT").is_some(),
            ..Tracker::default()
        }
    }

    pub fn tick(&mut self, ctx: &egui::Context) {
        if self.off {
            return;
        }
        self.frames += 1;
        let (outer, inner, maximized, minimized, monitor) = ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect, v.inner_rect, v.maximized, v.minimized, v.monitor_size)
        });
        // Brought back off every screen (a screen since unplugged): put it
        // where it can be seen.
        if self.frames == 3 && monitor.is_none() {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(60.0, 60.0)));
        }
        if std::env::var_os("AF2_TRACE_WIN").is_some() && self.frames % 30 == 1 {
            eprintln!("window: frame {} max {maximized:?} min {minimized:?} outer {outer:?} inner {inner:?} monitor {monitor:?}", self.frames);
        }
        if self.restore_max {
            if maximized != Some(true) {
                if self.frames == 2 || self.frames % 20 == 0 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
                }
                // Nothing is kept until it is (or a few seconds pass).
                if self.frames < 300 {
                    ctx.request_repaint();
                    return;
                }
            }
            self.restore_max = false;
        }
        if minimized == Some(true) {
            return;
        }
        let (Some(outer), Some(inner)) = (outer, inner) else { return };
        let mut m = self.now.or(self.saved).unwrap_or(WindowMemory {
            x: outer.left(),
            y: outer.top(),
            w: inner.width(),
            h: inner.height(),
            maximized: false,
        });
        if maximized == Some(true) {
            // Keep the size and place it comes back to.
            m.maximized = true;
        } else {
            // Whole points, so scaling's fractions don't count as a move.
            m = WindowMemory {
                x: outer.left().round(),
                y: outer.top().round(),
                w: inner.width().round(),
                h: inner.height().round(),
                maximized: false,
            };
        }
        if self.now != Some(m) {
            self.now = Some(m);
            self.changed = Some(Instant::now());
        }
        // Written once it has stayed put a second (not on every frame of a drag).
        if self.changed.is_some_and(|t| t.elapsed() > Duration::from_secs(1)) {
            self.flush();
        } else if self.changed.is_some() {
            ctx.request_repaint_after(Duration::from_millis(1100));
        }
    }

    /// Writes down the window now (also on quitting).
    pub fn flush(&mut self) {
        self.changed = None;
        if let Some(m) = self.now {
            if self.saved != Some(m) && m.sane() {
                m.save();
                self.saved = Some(m);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_sensible_window_is_restored() {
        let ok = WindowMemory { x: 100.0, y: 50.0, w: 1400.0, h: 900.0, maximized: false };
        assert!(ok.sane());
        assert!(!WindowMemory { w: 10.0, ..ok }.sane());
        assert!(!WindowMemory { x: f32::NAN, ..ok }.sane());
        assert!(!WindowMemory { y: -90_000.0, ..ok }.sane());
        // Opened no smaller than the minimum.
        assert_eq!(WindowMemory { w: 600.0, h: 400.0, ..ok }.size(), MIN);
    }
}
