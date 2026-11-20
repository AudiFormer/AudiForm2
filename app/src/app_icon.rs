// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The window's icon (title bar and taskbar), in the theme's colours.
//!
//! `assets/icon/make_icon.py` draws the AudiForm 2 icon separately for
//! each size (small ones simplified and pixel-hinted) and stores it as
//! masks rather than colours:
//!
//! - `mask-N.png`: R = first light, G = second light, B = ink, A = tile;
//! - `fx-N.png`: R = first light's glow, G = second light's glow,
//!   B = the light along the tile's edge, A = the halo around the tile.
//!
//! [`composite`] paints those in a theme's colours (the first light in its
//! `accent`, the second in `accent_alt`, the tile in its background, or
//! for light themes its panel with the text as ink). It runs once per
//! theme change, never per frame. A theme can name its own picture
//! instead (`Theme::icon`). On Windows the big (taskbar, Alt+Tab) and
//! small (title bar) icons are set separately, each from the drawing made
//! for its size; elsewhere the 256 px picture goes to the window.
//!
//! The exe's own icon (Explorer, shortcuts) is the Zero Point one, built
//! in by `app/build.rs` from `assets/icon/audiform2.ico`.

#![cfg_attr(not(windows), allow(dead_code))]

use crate::theme::{Rgb, Theme};
use std::sync::Arc;

/// The sizes drawn on their own pixel grids.
pub const SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

macro_rules! per_size {
    ($dir:literal, $prefix:literal) => {
        [
            include_bytes!(concat!("../../assets/icon/", $dir, $prefix, "-16.png")).as_slice(),
            include_bytes!(concat!("../../assets/icon/", $dir, $prefix, "-20.png")).as_slice(),
            include_bytes!(concat!("../../assets/icon/", $dir, $prefix, "-24.png")).as_slice(),
            include_bytes!(concat!("../../assets/icon/", $dir, $prefix, "-32.png")).as_slice(),
            include_bytes!(concat!("../../assets/icon/", $dir, $prefix, "-40.png")).as_slice(),
            include_bytes!(concat!("../../assets/icon/", $dir, $prefix, "-48.png")).as_slice(),
            include_bytes!(concat!("../../assets/icon/", $dir, $prefix, "-64.png")).as_slice(),
            include_bytes!(concat!("../../assets/icon/", $dir, $prefix, "-128.png")).as_slice(),
            include_bytes!(concat!("../../assets/icon/", $dir, $prefix, "-256.png")).as_slice(),
        ]
    };
}

const MASK: [&[u8]; 9] = per_size!("masks/", "mask");
const FX: [&[u8]; 9] = per_size!("masks/", "fx");

/// The themes' own icon pictures: (name, one PNG per size in [`SIZES`]).
const PICTURES: [(&str, [&[u8]; 9]); 3] = [
    ("pastel", per_size!("pictures/", "pastel")),
    ("pastel-night", per_size!("pictures/", "pastel-night")),
    ("undercurrent", per_size!("pictures/", "undercurrent")),
];

/// The Zero Point icon at 256 px, for the window before the theme is known.
pub const DEFAULT_256: &[u8] = include_bytes!("../../assets/icon/audiform2-256.png");

/// The Zero Point icon as egui wants it.
pub fn default_icon_data() -> Option<egui::IconData> {
    let img = image::load_from_memory(DEFAULT_256).ok()?.to_rgba8();
    Some(egui::IconData {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}

// ------------------------------------------------------------------ colour
// Mirrors `palette` and `composite` in assets/icon/make_icon.py.

type F3 = [f32; 3];

/// The icon's colours for one theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub tile: F3,
    pub ink: F3,
    pub main: F3,
    pub second: F3,
    pub rim: F3,
    pub halo: F3,
    /// A dark tile (light marks) rather than a light one.
    pub dark: bool,
}

fn f3(c: Rgb) -> F3 {
    [c[0] as f32, c[1] as f32, c[2] as f32]
}

fn mix(a: F3, b: F3, t: f32) -> F3 {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn lum(c: F3) -> f32 {
    let ch = |v: f32| {
        let v = v / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])
}

fn contrast(a: F3, b: F3) -> f32 {
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `c` pushed toward white (dark tile) or black (light tile) until it
/// stands out from the tile by `want`.
fn legible(c: F3, tile: F3, want: f32) -> F3 {
    let target = if lum(tile) < 0.18 {
        [255.0; 3]
    } else {
        [0.0; 3]
    };
    for i in 0..=20 {
        let m = mix(c, target, i as f32 / 20.0);
        if contrast(m, tile) >= want {
            return m;
        }
    }
    target
}

impl Palette {
    pub fn for_theme(th: &Theme) -> Palette {
        let dark = lum(f3(th.background)) < 0.18;
        if dark {
            let mut tile = f3(th.background);
            if lum(tile) > 0.03 {
                tile = mix(tile, [0.0; 3], 0.4);
            }
            let text = f3(th.text);
            let ink = if contrast(text, tile) >= 7.0 {
                text
            } else {
                [242.0, 248.0, 255.0]
            };
            let main = legible(f3(th.accent), tile, 3.0);
            let second = legible(f3(th.accent_alt), tile, 3.0);
            Palette {
                tile,
                ink,
                main,
                second,
                rim: mix(main, [255.0; 3], 0.35),
                halo: main,
                dark,
            }
        } else {
            let tile = f3(th.panel);
            let ink = f3(th.text);
            Palette {
                tile,
                ink,
                main: legible(f3(th.accent), tile, 3.0),
                second: legible(f3(th.accent_alt), tile, 3.0),
                rim: mix(tile, ink, 0.55),
                halo: ink,
                dark,
            }
        }
    }
}

/// Paints one size's masks (`mask`, `fx`: RGBA, same size) in `p`'s
/// colours; straight-alpha RGBA out.
pub fn composite(mask: &[u8], fx: &[u8], p: &Palette) -> Vec<u8> {
    let (glow_k, halo_k) = if p.dark { (0.55, 0.6) } else { (0.25, 0.35) };
    let mut out = Vec::with_capacity(mask.len());
    let px = |v: &[u8], j: usize| [v[j], v[j + 1], v[j + 2], v[j + 3]].map(|v| v as f32 / 255.0);
    for j in (0..mask.len().min(fx.len()) / 4 * 4).step_by(4) {
        let (m, f) = (px(mask, j), px(fx, j));
        let mut tile = mix(p.tile, p.main, f[0] * glow_k);
        tile = mix(tile, p.second, f[1] * glow_k);
        tile = mix(tile, p.rim, f[2] * 0.6);
        // Premultiplied "over", bottom to top.
        let mut rgb = [0.0f32; 3];
        let mut a = 0.0f32;
        let mut over = |col: F3, cov: f32| {
            for i in 0..3 {
                rgb[i] = col[i] * cov + rgb[i] * (1.0 - cov);
            }
            a = cov + a * (1.0 - cov);
        };
        over(p.halo, f[3] * halo_k);
        over(tile, m[3]);
        over(p.second, m[1]);
        over(p.main, m[0]);
        over(p.ink, m[2]);
        let q = |v: f32| v.round().clamp(0.0, 255.0) as u8;
        if a > 0.0 {
            out.extend_from_slice(&[q(rgb[0] / a), q(rgb[1] / a), q(rgb[2] / a)]);
        } else {
            out.extend_from_slice(&[0, 0, 0]);
        }
        out.push(q(a * 255.0));
    }
    out
}

/// A square straight-alpha RGBA picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub size: u32,
    pub rgba: Vec<u8>,
}

fn decode(png: &[u8]) -> Option<image::RgbaImage> {
    Some(image::load_from_memory(png).ok()?.to_rgba8())
}

/// Index of the drawing to use for `size`: that size, or the next larger
/// one (shrunk to fit), or the largest.
fn source_index(size: u32) -> usize {
    SIZES
        .iter()
        .position(|&s| s >= size)
        .unwrap_or(SIZES.len() - 1)
}

/// The theme's icon at `size` px.
pub fn build(th: &Theme, size: u32) -> Option<Picture> {
    let i = source_index(size);
    let img = match PICTURES.iter().find(|(n, _)| *n == th.icon) {
        Some((_, pngs)) => decode(pngs[i])?,
        None => {
            let mask = decode(MASK[i])?;
            let fx = decode(FX[i])?;
            let rgba = composite(mask.as_raw(), fx.as_raw(), &Palette::for_theme(th));
            image::RgbaImage::from_raw(mask.width(), mask.height(), rgba)?
        }
    };
    let img = if img.width() == size {
        img
    } else {
        image::imageops::resize(&img, size, size, image::imageops::FilterType::Lanczos3)
    };
    Some(Picture {
        size,
        rgba: img.into_raw(),
    })
}

/// What the current icon was made from, so a theme change that leaves it
/// alone does nothing.
fn key(th: &Theme) -> String {
    if PICTURES.iter().any(|(n, _)| *n == th.icon) {
        format!("picture:{}", th.icon)
    } else {
        format!("{:?}", Palette::for_theme(th))
    }
}

thread_local! {
    static LAST: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Puts the theme's icon on the window, if it differs from the last one.
pub fn apply(ctx: &egui::Context, th: &Theme) {
    let k = key(th);
    if LAST.with(|l| *l.borrow() == k) {
        return;
    }
    if set_native(th) {
        LAST.with(|l| *l.borrow_mut() = k);
        return;
    }
    if let Some(p) = build(th, 256) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Icon(Some(Arc::new(
            egui::IconData {
                width: p.size,
                height: p.size,
                rgba: p.rgba,
            },
        ))));
        LAST.with(|l| *l.borrow_mut() = k);
    }
}

#[cfg(not(windows))]
pub fn install(_cc: &eframe::CreationContext) {}

#[cfg(not(windows))]
fn set_native(_th: &Theme) -> bool {
    false
}

#[cfg(not(windows))]
pub fn frame() {}

#[cfg(windows)]
use win::set_native;
#[cfg(windows)]
pub use win::{frame, install};

#[cfg(windows)]
#[allow(non_snake_case, clippy::upper_case_acronyms)]
mod win {
    use super::*;
    use std::sync::atomic::{AtomicIsize, Ordering};

    type HWND = isize;
    type HANDLE = isize;

    #[link(name = "user32")]
    extern "system" {
        fn SendMessageW(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize;
        fn CreateIconFromResourceEx(
            bits: *const u8,
            len: u32,
            icon: i32,
            ver: u32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> HANDLE;
        fn DestroyIcon(icon: HANDLE) -> i32;
        fn GetSystemMetrics(index: i32) -> i32;
    }

    const WM_SETICON: u32 = 0x0080;
    const WM_GETICON: u32 = 0x007F;
    const ICON_SMALL: usize = 0;
    const ICON_BIG: usize = 1;
    const SM_CXICON: i32 = 11;
    const SM_CXSMICON: i32 = 49;

    static HWND_: AtomicIsize = AtomicIsize::new(0);
    static BIG: AtomicIsize = AtomicIsize::new(0);
    static SMALL: AtomicIsize = AtomicIsize::new(0);

    pub fn install(cc: &eframe::CreationContext) {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(h) = cc.window_handle() {
            if let RawWindowHandle::Win32(h) = h.as_raw() {
                HWND_.store(h.hwnd.get(), Ordering::Relaxed);
            }
        }
    }

    /// An HICON from a picture, through a PNG (which
    /// `CreateIconFromResourceEx` reads directly, alpha and all).
    fn hicon(p: &Picture) -> HANDLE {
        let Some(img) = image::RgbaImage::from_raw(p.size, p.size, p.rgba.clone()) else {
            return 0;
        };
        let mut png = Vec::new();
        if img
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .is_err()
        {
            return 0;
        }
        let s = p.size as i32;
        // SAFETY: reads `png` only; the icon is ours to destroy.
        unsafe { CreateIconFromResourceEx(png.as_ptr(), png.len() as u32, 1, 0x0003_0000, s, s, 0) }
    }

    pub(super) fn set_native(th: &Theme) -> bool {
        let hwnd = HWND_.load(Ordering::Relaxed);
        if hwnd == 0 {
            return false;
        }
        let (big_px, small_px) =
            unsafe { (GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CXSMICON)) };
        let big_px = if big_px > 0 { big_px as u32 } else { 32 };
        let small_px = if small_px > 0 { small_px as u32 } else { 16 };
        let (Some(b), Some(s)) = (build(th, big_px), build(th, small_px)) else {
            return false;
        };
        let (hb, hs) = (hicon(&b), hicon(&s));
        if hb == 0 || hs == 0 {
            unsafe {
                if hb != 0 {
                    DestroyIcon(hb);
                }
                if hs != 0 {
                    DestroyIcon(hs);
                }
            }
            return false;
        }
        // The big one first: Windows otherwise shrinks it for the small.
        unsafe {
            SendMessageW(hwnd, WM_SETICON, ICON_BIG, hb);
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL, hs);
        }
        for (slot, new) in [(&BIG, hb), (&SMALL, hs)] {
            let old = slot.swap(new, Ordering::Relaxed);
            if old != 0 {
                unsafe { DestroyIcon(old) };
            }
        }
        true
    }

    /// Once a frame: eframe sets its own (shrunk) icons from the viewport's
    /// picture when the window first becomes active, which may come after
    /// ours; put ours back if so. One message to our own window.
    pub fn frame() {
        let hwnd = HWND_.load(Ordering::Relaxed);
        let (big, small) = (BIG.load(Ordering::Relaxed), SMALL.load(Ordering::Relaxed));
        if hwnd == 0 || big == 0 {
            return;
        }
        unsafe {
            if SendMessageW(hwnd, WM_GETICON, ICON_BIG, 0) != big {
                SendMessageW(hwnd, WM_SETICON, ICON_BIG, big);
                SendMessageW(hwnd, WM_SETICON, ICON_SMALL, small);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(mask: [u8; 4], fx: [u8; 4], p: &Palette) -> [u8; 4] {
        let v = composite(&mask, &fx, p);
        [v[0], v[1], v[2], v[3]]
    }

    fn q(c: F3) -> [u8; 3] {
        c.map(|v| v.round() as u8)
    }

    #[test]
    fn fully_covered_pixels_take_the_theme_colours() {
        for th in Theme::builtins() {
            let p = Palette::for_theme(&th);
            let tile = one([0, 0, 0, 255], [0; 4], &p);
            assert_eq!(tile[..3], q(p.tile), "{}", th.name);
            assert_eq!(tile[3], 255);
            let main = one([255, 0, 0, 255], [0; 4], &p);
            assert_eq!(main[..3], q(p.main), "{}", th.name);
            let second = one([0, 255, 0, 255], [0; 4], &p);
            assert_eq!(second[..3], q(p.second), "{}", th.name);
            let ink = one([255, 255, 255, 255], [0; 4], &p);
            assert_eq!(ink[..3], q(p.ink), "{}", th.name);
            // Marks off the tile keep their own coverage as alpha.
            assert_eq!(one([0, 0, 0, 0], [0; 4], &p)[3], 0);
            assert_eq!(one([0, 0, 0, 128], [0; 4], &p)[3], 128);
            assert_eq!(one([0, 128, 0, 0], [0; 4], &p)[3], 128);
        }
        // Zero Point's lights are the theme's accents unchanged.
        let p = Palette::for_theme(&Theme::zero_point());
        assert_eq!(q(p.main), [0, 234, 255]);
        assert_eq!(q(p.second), [255, 46, 166]);
    }

    #[test]
    fn the_lights_stand_out_from_the_tile() {
        for th in Theme::builtins() {
            let p = Palette::for_theme(&th);
            assert!(contrast(p.main, p.tile) >= 2.99, "{}", th.name);
            assert!(contrast(p.second, p.tile) >= 2.99, "{}", th.name);
            assert!(contrast(p.ink, p.tile) >= 3.0, "{}", th.name);
        }
    }

    #[test]
    fn every_size_decodes_at_its_size() {
        for (i, &s) in SIZES.iter().enumerate() {
            for png in [MASK[i], FX[i]]
                .into_iter()
                .chain(PICTURES.iter().map(|p| p.1[i]))
            {
                let img = decode(png).unwrap();
                assert_eq!((img.width(), img.height()), (s, s));
            }
        }
        let d = default_icon_data().unwrap();
        assert_eq!((d.width, d.height, d.rgba.len()), (256, 256, 256 * 256 * 4));
        for size in [16, 20, 24, 32, 40, 48, 64, 96, 256, 300] {
            let p = build(&Theme::zero_point(), size).unwrap();
            assert_eq!(p.rgba.len(), (size * size * 4) as usize);
        }
    }

    #[test]
    fn rust_and_the_generator_agree_on_zero_point() {
        // audiform2-N.png were painted by make_icon.py from the same masks.
        let th = Theme::zero_point();
        for (i, &s) in SIZES.iter().enumerate() {
            let want = decode(match s {
                32 => include_bytes!("../../assets/icon/audiform2-32.png").as_slice(),
                256 => DEFAULT_256,
                _ => continue,
            })
            .unwrap();
            let mask = decode(MASK[i]).unwrap();
            let fx = decode(FX[i]).unwrap();
            let got = composite(mask.as_raw(), fx.as_raw(), &Palette::for_theme(&th));
            let worst = got
                .iter()
                .zip(want.as_raw())
                .enumerate()
                // colour under (almost) no alpha is meaningless
                .filter(|(j, _)| want.as_raw()[j / 4 * 4 + 3] > 8)
                .map(|(_, (a, b))| (*a as i32 - *b as i32).abs())
                .max()
                .unwrap();
            assert!(worst <= 2, "size {s}: off by {worst}");
        }
    }

    #[test]
    fn theme_pictures_are_used_by_name() {
        let mut th = Theme::zero_point();
        let plain = build(&th, 32).unwrap();
        th.icon = "pastel".into();
        let pastel = build(&th, 32).unwrap();
        assert_ne!(plain, pastel);
        for t in Theme::builtins().iter().filter(|t| !t.icon.is_empty()) {
            assert!(PICTURES.iter().any(|(n, _)| *n == t.icon), "{}", t.name);
        }
        th.icon = "no-such-picture".into();
        assert_eq!(build(&th, 32).unwrap(), plain);
    }
}
