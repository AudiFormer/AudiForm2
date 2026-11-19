// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Theme mouse pointers: Audiform's glass, Phosphor pixel and Mall Pastel
//! pointers (see `assets/pointers/README.md`).
//!
//! On Windows the pictures become real OS cursors (`hwcursor.rs`), which
//! move with the mouse at no lag. Elsewhere, or if that fails, this draws a
//! software pointer: at the end of each frame it reads the cursor egui
//! wants, hides the OS cursor and paints the theme's picture for it on the
//! top-most layer. The OS cursor comes back as soon as the pointer leaves
//! the window, the theme has no pointers, or View > Theme Pointers is off.
//! A software pointer can trail the real one by a frame on slow machines,
//! which is why it can be turned off.

use egui::{Context, CursorIcon, TextureHandle, TextureOptions};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// What the pointer is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Arrow,
    Text,
    Ew,
    Ns,
    Nwse,
    Nesw,
    Grab,
    Grabbing,
    Busy,
    Move,
    /// Clip-edge trimming. Every set has them; egui has no cursor that
    /// asks for them yet.
    #[allow(dead_code)]
    TrimLeft,
    #[allow(dead_code)]
    TrimRight,
}

/// One pointer picture: its hotspot in CSS pixels of the 1x picture, and
/// the PNG at 2x and 3x.
pub struct Pointer {
    pub kind: Kind,
    pub hot: [f32; 2],
    pub png: [&'static [u8]; 2],
}

/// A theme's pointers.
pub struct Set {
    pub name: &'static str,
    /// Points per CSS pixel. 1.0 everywhere: each pointer at the size its
    /// picture was drawn, as Audiform 1 showed it (CSS cursors are never
    /// scaled up).
    pub scale: f32,
    /// Pixel art: scaled nearest-neighbour, in whole screen pixels.
    pub pixel: bool,
    pub pointers: &'static [Pointer],
}

/// The PNG scales built in, in order.
pub const SCALES: [u32; 2] = [2, 3];

macro_rules! p {
    ($set:literal, $kind:literal, $k:expr, $x:expr, $y:expr) => {
        Pointer {
            kind: $k,
            hot: [$x as f32, $y as f32],
            png: [
                include_bytes!(concat!(
                    "../../assets/pointers/",
                    $set,
                    "-",
                    $kind,
                    "@2x.png"
                )),
                include_bytes!(concat!(
                    "../../assets/pointers/",
                    $set,
                    "-",
                    $kind,
                    "@3x.png"
                )),
            ],
        }
    };
}

// Hotspots from Audiform's theme files: the arrow's from the theme css
// (`cursor: url(asset:...) X Y`), the rest from each theme's `pointers`.
pub static SETS: [Set; 3] = [
    Set {
        name: "glass",
        scale: 1.0,
        pixel: false,
        pointers: &[
            p!("glass", "arrow", Kind::Arrow, 3, 2),
            p!("glass", "text", Kind::Text, 10, 10),
            p!("glass", "ew", Kind::Ew, 10, 10),
            p!("glass", "ns", Kind::Ns, 10, 10),
            p!("glass", "nwse", Kind::Nwse, 10, 10),
            p!("glass", "nesw", Kind::Nesw, 10, 10),
            p!("glass", "grab", Kind::Grab, 10, 10),
            p!("glass", "grabbing", Kind::Grabbing, 10, 10),
            p!("glass", "move", Kind::Move, 10, 10),
            p!("glass", "trim-left", Kind::TrimLeft, 8, 8),
            p!("glass", "trim-right", Kind::TrimRight, 8, 8),
        ],
    },
    Set {
        name: "phosphor",
        scale: 1.0,
        pixel: true,
        pointers: &[
            p!("phosphor", "arrow", Kind::Arrow, 2, 2),
            p!("phosphor", "text", Kind::Text, 7, 10),
            p!("phosphor", "ew", Kind::Ew, 11, 5),
            p!("phosphor", "ns", Kind::Ns, 5, 11),
            p!("phosphor", "nwse", Kind::Nwse, 9, 9),
            p!("phosphor", "nesw", Kind::Nesw, 9, 9),
            p!("phosphor", "grab", Kind::Grab, 10, 9),
            p!("phosphor", "grabbing", Kind::Grabbing, 10, 7),
            p!("phosphor", "move", Kind::Move, 10, 10),
            p!("phosphor", "trim-left", Kind::TrimLeft, 8, 8),
            p!("phosphor", "trim-right", Kind::TrimRight, 8, 8),
        ],
    },
    Set {
        name: "mall",
        scale: 1.0,
        pixel: false,
        pointers: &[
            p!("mall", "arrow", Kind::Arrow, 2, 1),
            p!("mall", "text", Kind::Text, 8, 8),
            p!("mall", "ew", Kind::Ew, 8, 8),
            p!("mall", "ns", Kind::Ns, 8, 8),
            p!("mall", "nwse", Kind::Nwse, 8, 8),
            p!("mall", "nesw", Kind::Nesw, 8, 8),
            p!("mall", "grab", Kind::Grab, 8, 8),
            p!("mall", "grabbing", Kind::Grabbing, 8, 8),
            p!("mall", "busy", Kind::Busy, 8, 8),
            p!("mall", "move", Kind::Move, 8, 8),
            p!("mall", "trim-left", Kind::TrimLeft, 8, 8),
            p!("mall", "trim-right", Kind::TrimRight, 8, 8),
        ],
    },
];

pub fn set(name: &str) -> Option<&'static Set> {
    SETS.iter().find(|s| s.name == name)
}

impl Set {
    /// The picture for `kind`, or the arrow when the set has none.
    pub fn pointer(&self, kind: Kind) -> &'static Pointer {
        let all: &'static [Pointer] = self.pointers;
        all.iter()
            .find(|p| p.kind == kind)
            .or_else(|| all.iter().find(|p| p.kind == Kind::Arrow))
            .expect("every pointer set has an arrow")
    }
}

/// The pointer kind for the cursor egui asked for (None: draw nothing).
pub fn kind_for(icon: CursorIcon) -> Option<Kind> {
    Some(match icon {
        CursorIcon::None => return None,
        CursorIcon::Text | CursorIcon::VerticalText => Kind::Text,
        CursorIcon::ResizeHorizontal
        | CursorIcon::ResizeColumn
        | CursorIcon::ResizeEast
        | CursorIcon::ResizeWest => Kind::Ew,
        CursorIcon::ResizeVertical
        | CursorIcon::ResizeRow
        | CursorIcon::ResizeNorth
        | CursorIcon::ResizeSouth => Kind::Ns,
        CursorIcon::ResizeNwSe | CursorIcon::ResizeNorthWest | CursorIcon::ResizeSouthEast => {
            Kind::Nwse
        }
        CursorIcon::ResizeNeSw | CursorIcon::ResizeNorthEast | CursorIcon::ResizeSouthWest => {
            Kind::Nesw
        }
        CursorIcon::Grab => Kind::Grab,
        CursorIcon::Grabbing => Kind::Grabbing,
        CursorIcon::Move | CursorIcon::AllScroll => Kind::Move,
        CursorIcon::Wait | CursorIcon::Progress => Kind::Busy,
        _ => Kind::Arrow,
    })
}

/// The decoded picture, straight from its PNG.
pub fn decode(png: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(png).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

/// The pointer's texture at PNG scale index `i`, decoded the first time
/// it is asked for and then kept (None if it will not decode).
fn texture(ctx: &Context, set: &Set, p: &Pointer, i: usize) -> Option<TextureHandle> {
    let id = egui::Id::new(("af2-pointer", set.name, p.kind, i));
    if let Some(t) = ctx.data(|d| d.get_temp::<Option<TextureHandle>>(id)) {
        return t;
    }
    let opts = if set.pixel {
        TextureOptions::NEAREST
    } else {
        TextureOptions::LINEAR
    };
    let tex = decode(p.png[i]).map(|img| {
        ctx.load_texture(
            format!("pointer-{}-{:?}@{}x", set.name, p.kind, SCALES[i]),
            img,
            opts,
        )
    });
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

fn enabled_flag() -> &'static AtomicBool {
    static ON: OnceLock<AtomicBool> = OnceLock::new();
    ON.get_or_init(|| {
        let saved = std::fs::read_to_string(crate::state::data_dir().join("pointers.txt"))
            .unwrap_or_default();
        AtomicBool::new(saved.trim() != "0")
    })
}

/// View > Theme Pointers: on unless turned off; saved between runs.
pub fn menu_toggle(ui: &mut egui::Ui) {
    let mut on = enabled_flag().load(Ordering::Relaxed);
    if ui
        .checkbox(&mut on, "Theme Pointers")
        .on_hover_text(
            "Draw the theme's own mouse pointers. Turn off if the pointer \
             trails behind on this device.",
        )
        .changed()
    {
        enabled_flag().store(on, Ordering::Relaxed);
        let dir = crate::state::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("pointers.txt"), if on { "1" } else { "0" });
    }
}

/// Call once at the very end of the frame: swaps the OS cursor for the
/// theme's pointer while the pointer is over the window.
pub fn draw(ctx: &Context, theme_pointers: &str) {
    let on = enabled_flag().load(Ordering::Relaxed);
    let set = set(theme_pointers).filter(|_| on);
    if crate::hwcursor::frame(ctx, set) {
        return;
    }
    let Some(set) = set else {
        return;
    };
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let Some(kind) = kind_for(ctx.output(|o| o.cursor_icon)) else {
        return;
    };
    let p = set.pointer(kind);
    let ppp = ctx.pixels_per_point();
    // Screen pixels per CSS pixel of the picture.
    let density = crate::hwcursor::density(set, ppp);
    let i = crate::hwcursor::scale_index(density);
    let Some(tex) = texture(ctx, set, p, i) else {
        return;
    };
    let css = tex.size_vec2() / SCALES[i] as f32;
    let pts = density / ppp;
    let snap = |v: f32| (v * ppp).round() / ppp;
    let min = egui::pos2(snap(pos.x - p.hot[0] * pts), snap(pos.y - p.hot[1] * pts));
    let rect = egui::Rect::from_min_size(min, css * pts);
    ctx.set_cursor_icon(CursorIcon::None);
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Debug,
        egui::Id::new("af2-theme-pointer"),
    ));
    painter.image(
        tex.id(),
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_set_is_complete_and_decodes() {
        let core = [
            Kind::Arrow,
            Kind::Text,
            Kind::Ew,
            Kind::Ns,
            Kind::Nwse,
            Kind::Nesw,
            Kind::Grab,
            Kind::Grabbing,
            Kind::Move,
            Kind::TrimLeft,
            Kind::TrimRight,
        ];
        let mut used = 0;
        for th in crate::theme::Theme::builtins() {
            if th.pointers.is_empty() {
                continue;
            }
            used += 1;
            let s = set(&th.pointers)
                .unwrap_or_else(|| panic!("{}: no pointer set {:?}", th.name, th.pointers));
            for k in core {
                assert!(
                    s.pointers.iter().any(|p| p.kind == k),
                    "{} lacks {k:?}",
                    s.name
                );
            }
        }
        assert!(used >= 5, "only {used} themes have pointers");
        for s in &SETS {
            for p in s.pointers {
                let mut css: Option<(f32, f32)> = None;
                for (i, png) in p.png.iter().enumerate() {
                    let img = decode(png).unwrap_or_else(|| {
                        panic!("{}-{:?}@{}x will not decode", s.name, p.kind, SCALES[i])
                    });
                    let f = SCALES[i] as f32;
                    let [w, h] = [img.size[0] as f32 / f, img.size[1] as f32 / f];
                    // Both scales are the same picture.
                    if let Some((cw, ch)) = css {
                        assert!((w - cw).abs() < 0.5 && (h - ch).abs() < 0.5);
                    }
                    css = Some((w, h));
                    assert!(
                        p.hot[0] >= 0.0 && p.hot[0] < w && p.hot[1] >= 0.0 && p.hot[1] < h,
                        "{}-{:?}: hotspot {:?} outside {w}x{h}",
                        s.name,
                        p.kind,
                        p.hot
                    );
                    // Not an empty picture.
                    assert!(img.pixels.iter().any(|c| c.a() > 0));
                }
            }
        }
    }

    #[test]
    fn cursor_mapping() {
        assert_eq!(kind_for(CursorIcon::Default), Some(Kind::Arrow));
        assert_eq!(kind_for(CursorIcon::ResizeColumn), Some(Kind::Ew));
        assert_eq!(kind_for(CursorIcon::ResizeRow), Some(Kind::Ns));
        assert_eq!(kind_for(CursorIcon::AllScroll), Some(Kind::Move));
        assert_eq!(kind_for(CursorIcon::None), None);
        // Only Mall Pastel has a busy pointer; the others fall back.
        assert_eq!(set("mall").unwrap().pointer(Kind::Busy).kind, Kind::Busy);
        assert_eq!(set("glass").unwrap().pointer(Kind::Busy).kind, Kind::Arrow);
    }
}
