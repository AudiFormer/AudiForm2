// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Theme pointers as real OS cursors (Windows).
//!
//! A pointer painted into the egui frame trails the mouse by a frame or
//! more. On Windows the theme's pictures are instead turned into hardware
//! cursors (`HCURSOR`s), which the OS moves with zero lag:
//!
//! - one cursor per pointer kind of the active set, sized for the window's
//!   DPI (about the OS cursor's 32 px at 100%), rebuilt when the set or the
//!   DPI changes;
//! - the window is subclassed so that `WM_SETCURSOR` over the client area
//!   shows the theme's cursor (winit would otherwise put its own there);
//!   the resize borders and everything else still go to winit and Windows,
//!   so the sizing cursors stay the OS's own;
//! - each frame the cursor is picked from the icon egui wants, and the
//!   window is told to refresh it at once when it changes.
//!
//! The picture work (scaling, BGRA, mask, hotspot) is plain Rust and works
//! everywhere; only the Win32 calls are Windows-only. Anything that fails
//! leaves the software pointer in `pointers.rs` to do the job.

#![cfg_attr(not(windows), allow(dead_code))]

use crate::pointers::{Pointer, Set, SCALES};

/// A cursor picture ready for the OS.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    /// Straight-alpha BGRA, top row first.
    pub bgra: Vec<u8>,
    /// Monochrome AND mask, rows padded to 16 bits, most significant bit
    /// first: 1 where the picture is fully transparent.
    pub mask: Vec<u8>,
    /// Hotspot in cursor pixels.
    pub hot: [u32; 2],
}

/// Screen pixels per CSS pixel of the set's pictures at `ppp` screen pixels
/// per point. Pixel-art sets keep each art cell (two CSS pixels) a whole
/// number of screen pixels.
pub fn density(set: &Set, ppp: f32) -> f32 {
    let d = set.scale * ppp;
    if set.pixel {
        (d * 2.0).round().max(1.0) / 2.0
    } else {
        d
    }
}

/// Which PNG scale (index into `SCALES`) to draw from at `density`.
pub fn scale_index(density: f32) -> usize {
    if density <= SCALES[0] as f32 {
        0
    } else {
        1
    }
}

/// The pointer's cursor picture at `density` screen pixels per CSS pixel.
pub fn build(set: &Set, p: &Pointer, density: f32) -> Option<Image> {
    let i = scale_index(density);
    let src = image::load_from_memory(p.png[i]).ok()?.to_rgba8();
    let f = SCALES[i] as f32;
    let (sw, sh) = (src.width(), src.height());
    let w = ((sw as f32 / f) * density).round().max(1.0) as u32;
    let h = ((sh as f32 / f) * density).round().max(1.0) as u32;
    let rgba = if (w, h) == (sw, sh) {
        src.into_raw()
    } else if set.pixel {
        nearest(src.as_raw(), sw, sh, w, h)
    } else {
        smooth(src, w, h)
    };
    Some(Image {
        w,
        h,
        mask: and_mask(&rgba, w, h),
        bgra: to_bgra(&rgba),
        hot: hotspot(p.hot, density, w, h),
    })
}

/// The hotspot in cursor pixels, kept inside the picture.
pub fn hotspot(hot: [f32; 2], density: f32, w: u32, h: u32) -> [u32; 2] {
    let at = |v: f32, max: u32| ((v * density).round().max(0.0) as u32).min(max.saturating_sub(1));
    [at(hot[0], w), at(hot[1], h)]
}

/// Nearest-neighbour scaling, sampling each target pixel's centre.
pub fn nearest(rgba: &[u8], sw: u32, sh: u32, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let sy = (((y as f32 + 0.5) * sh as f32 / h as f32) as u32).min(sh - 1);
        for x in 0..w {
            let sx = (((x as f32 + 0.5) * sw as f32 / w as f32) as u32).min(sw - 1);
            let o = ((sy * sw + sx) * 4) as usize;
            out.extend_from_slice(&rgba[o..o + 4]);
        }
    }
    out
}

/// Smooth scaling, done on premultiplied colour so edges don't darken.
fn smooth(mut src: image::RgbaImage, w: u32, h: u32) -> Vec<u8> {
    for px in src.pixels_mut() {
        let a = px[3] as u32;
        for c in 0..3 {
            px[c] = ((px[c] as u32 * a + 127) / 255) as u8;
        }
    }
    let mut out = image::imageops::resize(&src, w, h, image::imageops::FilterType::Triangle);
    for px in out.pixels_mut() {
        let a = px[3] as u32;
        for c in 0..3 {
            px[c] = (px[c] as u32 * 255 + a / 2)
                .checked_div(a)
                .map_or(0, |v| v.min(255) as u8);
        }
    }
    out.into_raw()
}

/// RGBA to BGRA.
pub fn to_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for i in (0..out.len() / 4 * 4).step_by(4) {
        out.swap(i, i + 2);
    }
    out
}

/// The monochrome AND mask: 1 (screen shows through) where alpha is 0.
pub fn and_mask(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let stride = w.div_ceil(16) as usize * 2;
    let mut mask = vec![0xFF; stride * h as usize];
    for y in 0..h as usize {
        for x in 0..w as usize {
            if rgba[(y * w as usize + x) * 4 + 3] > 0 {
                mask[y * stride + x / 8] &= !(0x80 >> (x % 8));
            }
        }
    }
    mask
}

#[cfg(all(windows, target_pointer_width = "64"))]
pub use win::{alt_down, frame, install, system_pointer};

/// Not Windows: the pointer comes from egui.
#[cfg(not(all(windows, target_pointer_width = "64")))]
pub fn system_pointer() -> Option<(f32, f32)> {
    None
}

/// Not Windows: the modifier comes from egui.
#[cfg(not(all(windows, target_pointer_width = "64")))]
pub fn alt_down() -> bool {
    false
}

/// Not Windows: there is no hardware path; the software pointer draws.
#[cfg(not(all(windows, target_pointer_width = "64")))]
pub fn install(_cc: &eframe::CreationContext) {}

#[cfg(not(all(windows, target_pointer_width = "64")))]
pub fn frame(_ctx: &egui::Context, _set: Option<&'static Set>) -> bool {
    false
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[allow(non_snake_case, clippy::upper_case_acronyms)]
mod win {
    use super::*;
    use crate::pointers::{kind_for, Kind};
    use egui::CursorIcon;
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicIsize, Ordering};

    type HWND = isize;
    type HANDLE = isize;

    #[repr(C)]
    struct ICONINFO {
        fIcon: i32,
        xHotspot: u32,
        yHotspot: u32,
        hbmMask: HANDLE,
        hbmColor: HANDLE,
    }

    #[repr(C)]
    struct BITMAPINFOHEADER {
        biSize: u32,
        biWidth: i32,
        biHeight: i32,
        biPlanes: u16,
        biBitCount: u16,
        biCompression: u32,
        biSizeImage: u32,
        biXPelsPerMeter: i32,
        biYPelsPerMeter: i32,
        biClrUsed: u32,
        biClrImportant: u32,
    }

    #[repr(C)]
    struct BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER,
        bmiColors: [u32; 1],
    }

    #[repr(C)]
    #[derive(Default)]
    struct POINT {
        x: i32,
        y: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn CreateIconIndirect(info: *const ICONINFO) -> HANDLE;
        fn DestroyIcon(icon: HANDLE) -> i32;
        fn SetCursor(cursor: HANDLE) -> HANDLE;
        fn GetCursorPos(p: *mut POINT) -> i32;
        fn WindowFromPoint(p: POINT) -> HWND;
        fn SendMessageW(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize;
        fn PostMessageW(hwnd: HWND, msg: u32, w: usize, l: isize) -> i32;
        fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
        fn CallWindowProcW(prev: isize, hwnd: HWND, msg: u32, w: usize, l: isize) -> isize;
        fn DefWindowProcW(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize;
        fn ScreenToClient(hwnd: HWND, p: *mut POINT) -> i32;
        fn GetAsyncKeyState(key: i32) -> i16;
    }

    /// The pointer in the window's client area, in physical pixels, read
    /// from the system. While files are dragged in from the file manager
    /// the window hears no mouse moves, so egui's pointer is stale.
    pub fn system_pointer() -> Option<(f32, f32)> {
        HW.with(|hw| {
            let hwnd = hw.borrow().as_ref()?.hwnd;
            let mut p = POINT::default();
            unsafe {
                if GetCursorPos(&mut p) == 0 || ScreenToClient(hwnd, &mut p) == 0 {
                    return None;
                }
            }
            Some((p.x as f32, p.y as f32))
        })
    }

    /// Alt is held, read from the system (the window hears no keys while
    /// files are dragged in from the file manager).
    pub fn alt_down() -> bool {
        const VK_MENU: i32 = 0x12;
        (unsafe { GetAsyncKeyState(VK_MENU) } as u16) & 0x8000 != 0
    }

    #[link(name = "gdi32")]
    extern "system" {
        fn CreateDIBSection(
            hdc: HANDLE,
            bmi: *const BITMAPINFO,
            usage: u32,
            bits: *mut *mut c_void,
            section: HANDLE,
            offset: u32,
        ) -> HANDLE;
        fn CreateBitmap(w: i32, h: i32, planes: u32, bpp: u32, bits: *const c_void) -> HANDLE;
        fn DeleteObject(obj: HANDLE) -> i32;
    }

    const GWLP_WNDPROC: i32 = -4;
    const WM_SETCURSOR: u32 = 0x0020;
    const WM_NCHITTEST: u32 = 0x0084;
    const WM_MOUSEMOVE: u32 = 0x0200;
    /// Ours: "put the right cursor up now".
    const WM_AF2_CURSOR: u32 = 0x8000 + 0x2F7;
    const HTCLIENT: isize = 1;

    /// The window's original procedure.
    static OLD_PROC: AtomicIsize = AtomicIsize::new(0);
    /// The theme cursor for the client area; 0: leave it to winit.
    static CURRENT: AtomicIsize = AtomicIsize::new(0);

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
        match msg {
            WM_SETCURSOR => {
                let cur = CURRENT.load(Ordering::Relaxed);
                if cur != 0 && (l & 0xFFFF) == HTCLIENT {
                    SetCursor(cur);
                    return 1;
                }
            }
            WM_AF2_CURSOR => {
                refresh(hwnd);
                return 0;
            }
            _ => {}
        }
        match OLD_PROC.load(Ordering::Relaxed) {
            0 => DefWindowProcW(hwnd, msg, w, l),
            old => CallWindowProcW(old, hwnd, msg, w, l),
        }
    }

    /// Re-runs the cursor choice for where the mouse is now, as if it had
    /// just moved: our cursor in the client area, the OS's elsewhere.
    unsafe fn refresh(hwnd: HWND) {
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt) == 0 {
            return;
        }
        let (x, y) = (pt.x, pt.y);
        if WindowFromPoint(pt) != hwnd {
            return;
        }
        let xy = ((y as u16 as u32) << 16 | x as u16 as u32) as isize;
        let hit = SendMessageW(hwnd, WM_NCHITTEST, 0, xy);
        let l = ((WM_MOUSEMOVE as isize) << 16) | (hit & 0xFFFF);
        SendMessageW(hwnd, WM_SETCURSOR, hwnd as usize, l);
    }

    /// One `HCURSOR` from a picture (0 on failure).
    fn make_cursor(img: &Image) -> HANDLE {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: img.w as i32,
                biHeight: -(img.h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [0],
        };
        unsafe {
            let mut bits: *mut c_void = std::ptr::null_mut();
            let color = CreateDIBSection(0, &bmi, 0, &mut bits, 0, 0);
            if color == 0 || bits.is_null() {
                if color != 0 {
                    DeleteObject(color);
                }
                return 0;
            }
            std::ptr::copy_nonoverlapping(img.bgra.as_ptr(), bits as *mut u8, img.bgra.len());
            let mask = CreateBitmap(
                img.w as i32,
                img.h as i32,
                1,
                1,
                img.mask.as_ptr() as *const c_void,
            );
            if mask == 0 {
                DeleteObject(color);
                return 0;
            }
            let info = ICONINFO {
                fIcon: 0,
                xHotspot: img.hot[0],
                yHotspot: img.hot[1],
                hbmMask: mask,
                hbmColor: color,
            };
            let cursor = CreateIconIndirect(&info);
            DeleteObject(color);
            DeleteObject(mask);
            cursor
        }
    }

    struct Hw {
        hwnd: HWND,
        /// Set name and density the cursors were built for.
        key: Option<(&'static str, u32)>,
        cursors: Vec<(Kind, HANDLE)>,
        /// The egui icon last seen while the mouse was over the window.
        last_icon: Option<CursorIcon>,
        /// Gave up: something would not build; the software pointer draws.
        broken: bool,
    }

    impl Hw {
        fn destroy(&mut self) {
            for (_, c) in self.cursors.drain(..) {
                unsafe {
                    DestroyIcon(c);
                }
            }
            self.key = None;
        }
    }

    thread_local! {
        static HW: RefCell<Option<Hw>> = const { RefCell::new(None) };
    }

    /// Takes over the window's cursor (call once, from app creation).
    pub fn install(cc: &eframe::CreationContext) {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = cc.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(h) = handle.as_raw() else {
            return;
        };
        let hwnd = h.hwnd.get();
        HW.with(|hw| {
            let mut hw = hw.borrow_mut();
            if hw.is_some() {
                return;
            }
            let old =
                unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, wndproc as *const () as isize) };
            if old == 0 {
                return;
            }
            OLD_PROC.store(old, Ordering::Relaxed);
            *hw = Some(Hw {
                hwnd,
                key: None,
                cursors: Vec::new(),
                last_icon: None,
                broken: false,
            });
        });
    }

    /// Picks this frame's cursor. `set` is the theme's pointer set when
    /// theme pointers are on. Returns false when the hardware path is not
    /// working, so the software pointer should draw instead.
    pub fn frame(ctx: &egui::Context, set: Option<&'static Set>) -> bool {
        HW.with(|hw| {
            let mut hw = hw.borrow_mut();
            let Some(hw) = hw.as_mut() else {
                return false;
            };
            if hw.broken {
                return false;
            }
            let old_cur = CURRENT.load(Ordering::Relaxed);
            let icon = ctx.output(|o| o.cursor_icon);
            let mut cur = 0;
            let mut stale = Vec::new();
            if let Some(set) = set {
                let ppp = ctx
                    .native_pixels_per_point()
                    .unwrap_or_else(|| ctx.pixels_per_point());
                let d = density(set, ppp);
                let key = (set.name, d.to_bits());
                if hw.key != Some(key) {
                    // Swap in the new cursors first; the old ones go once
                    // the window shows the new one.
                    stale = std::mem::take(&mut hw.cursors);
                    hw.key = Some(key);
                    for p in set.pointers {
                        let c = build(set, p, d).map(|img| make_cursor(&img)).unwrap_or(0);
                        if c == 0 {
                            hw.broken = true;
                            break;
                        }
                        hw.cursors.push((p.kind, c));
                    }
                    if hw.broken {
                        CURRENT.store(0, Ordering::Relaxed);
                        unsafe { refresh(hw.hwnd) };
                        for (_, c) in stale {
                            unsafe { DestroyIcon(c) };
                        }
                        hw.destroy();
                        return false;
                    }
                }
                if let Some(kind) = kind_for(icon) {
                    let want = set.pointer(kind).kind;
                    cur = hw
                        .cursors
                        .iter()
                        .find(|(k, _)| *k == want)
                        .map_or(0, |(_, c)| *c);
                }
            }
            CURRENT.store(cur, Ordering::Relaxed);
            let over = ctx.input(|i| i.pointer.hover_pos()).is_some();
            let shown = over.then_some(icon);
            let icon_changed = shown != hw.last_icon;
            hw.last_icon = shown;
            if cur != old_cur || !stale.is_empty() {
                unsafe { refresh(hw.hwnd) };
            }
            for (_, c) in stale {
                unsafe { DestroyIcon(c) };
            }
            if cur != old_cur || icon_changed {
                // winit puts its own cursor up when egui's icon changes,
                // after this frame; this lands after it.
                unsafe { PostMessageW(hw.hwnd, WM_AF2_CURSOR, 0, 0) };
            }
            true
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pointers::{set, Kind, SETS};

    #[test]
    fn bgra_swaps_red_and_blue() {
        assert_eq!(
            to_bgra(&[1, 2, 3, 4, 10, 20, 30, 40]),
            vec![3, 2, 1, 4, 30, 20, 10, 40]
        );
    }

    #[test]
    fn mask_marks_transparent_pixels() {
        // 3x2: opaque, clear, faint / clear, clear, opaque.
        let a = [255u8, 0, 1, 0, 0, 200];
        let rgba: Vec<u8> = a.iter().flat_map(|&a| [9, 9, 9, a]).collect();
        let m = and_mask(&rgba, 3, 2);
        // Rows padded to 16 bits.
        assert_eq!(m.len(), 4);
        assert_eq!(m[0], 0b0101_1111);
        assert_eq!(m[1], 0xFF);
        assert_eq!(m[2], 0b1101_1111);
        assert_eq!(and_mask(&[0; 17 * 4], 17, 1).len(), 4);
    }

    #[test]
    fn hotspot_scales_and_stays_inside() {
        assert_eq!(hotspot([3.0, 2.0], 1.25, 26, 26), [4, 3]);
        assert_eq!(hotspot([10.0, 10.0], 2.0, 42, 42), [20, 20]);
        assert_eq!(hotspot([30.0, 30.0], 2.0, 42, 42), [41, 41]);
    }

    #[test]
    fn nearest_keeps_hard_pixels() {
        // 2x1 red/blue doubled to 4x2.
        let src = [255, 0, 0, 255, 0, 0, 255, 255];
        let out = nearest(&src, 2, 1, 4, 2);
        assert_eq!(out.len(), 4 * 2 * 4);
        assert_eq!(&out[0..8], &[255, 0, 0, 255, 255, 0, 0, 255]);
        assert_eq!(&out[8..16], &[0, 0, 255, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn densities_and_scales() {
        let ph = set("phosphor").unwrap();
        let gl = set("glass").unwrap();
        assert_eq!(density(gl, 1.0), 1.0); // natural size, as Audiform 1 showed it
        assert_eq!(density(ph, 1.0), 1.0);
        assert_eq!(density(ph, 1.3), 1.5);
        assert_eq!(scale_index(1.25), 0);
        assert_eq!(scale_index(2.0), 0);
        assert_eq!(scale_index(2.5), 1);
    }

    #[test]
    fn every_pointer_builds_at_common_dpis() {
        for s in &SETS {
            for ppp in [1.0, 1.25, 1.5, 2.0, 3.0] {
                let d = density(s, ppp);
                for p in s.pointers {
                    let img = build(s, p, d).expect("builds");
                    assert_eq!(img.bgra.len(), (img.w * img.h * 4) as usize);
                    assert_eq!(img.mask.len(), (img.w.div_ceil(16) * 2 * img.h) as usize);
                    assert!(img.hot[0] < img.w && img.hot[1] < img.h);
                    assert!(img.bgra.chunks(4).any(|c| c[3] > 0));
                    // About the OS's 32 px cursor box at 100%.
                    if ppp == 1.0 && p.kind == Kind::Arrow {
                        assert!((12..=40).contains(&img.w), "{} {}", s.name, img.w);
                    }
                }
            }
        }
    }
}
