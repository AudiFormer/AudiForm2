// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The desktop's own look, for the System theme and the System font:
//! KDE Plasma's colour scheme and font (`kdeglobals`), GNOME's light or
//! dark style, accent and font (the XDG Settings portal, or `gsettings`),
//! Windows' light or dark mode and accent colour (the registry), and
//! macOS's appearance and accent. Read when asked; nothing is written.
//!
//! While the System theme is shown, [`watch`] keeps a background thread
//! following the desktop: on Linux the portal's `SettingChanged` signal
//! (KDE's portal passes on colour-scheme changes too), elsewhere a light
//! poll every few seconds. The UI thread only picks up a finished reading
//! ([`take_change`]), so nothing here ever makes it wait.
//!
//! The live portal listening, the WCAG contrast guard and the fuller KDE
//! colour mapping come from b1g-b8lls' System theme work, tested on
//! Plasma 6.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::theme::Rgb;

/// The desktop's colours. Anything the desktop does not say is None.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DesktopColors {
    pub dark: Option<bool>,
    pub accent: Option<Rgb>,
    pub window: Option<Rgb>,
    pub window_text: Option<Rgb>,
    pub view: Option<Rgb>,
    pub button: Option<Rgb>,
    pub dim_text: Option<Rgb>,
    pub highlight_text: Option<Rgb>,
    /// Behind the panels (KDE's alternate window background).
    pub background: Option<Rgb>,
    pub selection: Option<Rgb>,
    /// A second accent (KDE's hover or visited-link colour).
    pub accent_alt: Option<Rgb>,
    /// Text on the view (waveform) background, for its grid.
    pub view_text: Option<Rgb>,
    /// The desktop's bad, caution and good colours: the playhead, the
    /// markers and the meters.
    pub negative: Option<Rgb>,
    pub neutral: Option<Rgb>,
    pub positive: Option<Rgb>,
}

/// The desktop's UI font: its family, and its size in points when known.
#[derive(Debug, Clone, PartialEq)]
pub struct DesktopFont {
    pub family: String,
    pub points: Option<f32>,
}

pub fn colors() -> DesktopColors {
    if cfg!(windows) {
        return windows_colors();
    }
    if cfg!(target_os = "macos") {
        return mac_colors();
    }
    let portal = portal_appearance();
    let plasma = crate::file_dialog::is_plasma();
    if let Some(k) = kde_globals() {
        let c = kde_colors(&k);
        if c.window.is_some() {
            return c;
        }
        // Plasma's default scheme writes no colour groups to kdeglobals:
        // it is Breeze, light or dark as the portal says.
        if plasma {
            return DesktopColors {
                accent: c.accent.or(portal.and_then(|p| p.accent)).or(Some(BREEZE_BLUE)),
                ..breeze(portal.is_some_and(|p| p.dark))
            };
        }
    } else if plasma {
        return breeze(portal.is_some_and(|p| p.dark));
    }
    match portal {
        Some(p) => adwaita(p.dark, p.accent),
        None => gnome_colors(),
    }
}

pub fn font() -> Option<DesktopFont> {
    if cfg!(windows) {
        return Some(DesktopFont {
            family: "Segoe UI".into(),
            points: None,
        });
    }
    if cfg!(target_os = "macos") {
        return Some(DesktopFont {
            family: "SF Pro".into(),
            points: None,
        });
    }
    if let Some(f) = kde_globals().and_then(|k| ini_get(&k, "General", "font").map(kde_font)) {
        return Some(f);
    }
    let name = gsettings("org.gnome.desktop.interface", "font-name")?;
    Some(gnome_font(&name))
}

/// The files of the desktop's UI font, regular and bold, with their face
/// index: found with fontconfig on Linux, at the system's own paths on
/// Windows and macOS.
pub fn font_files() -> Option<((Vec<u8>, u32), Option<(Vec<u8>, u32)>)> {
    let read = |p: &str| std::fs::read(p).ok().map(|b| (b, 0u32));
    if cfg!(windows) {
        let regular = read("C:\\Windows\\Fonts\\segoeui.ttf")?;
        return Some((regular, read("C:\\Windows\\Fonts\\segoeuib.ttf")));
    }
    if cfg!(target_os = "macos") {
        let regular = read("/System/Library/Fonts/SFNS.ttf")
            .or_else(|| read("/System/Library/Fonts/HelveticaNeue.ttc"))
            .or_else(|| read("/System/Library/Fonts/Helvetica.ttc"))?;
        return Some((regular, None));
    }
    let f = font()?;
    let regular = fc_match(&f.family, false)?;
    let bold = fc_match(&f.family, true).filter(|b| b.0 != regular.0 || b.1 != regular.1);
    let load = |(p, i): (PathBuf, u32)| std::fs::read(p).ok().map(|b| (b, i));
    Some((load(regular)?, bold.and_then(load)))
}

// ------------------------------------------------------------ KDE

/// Breeze's blue, KDE's accent when nothing else is said.
const BREEZE_BLUE: Rgb = [61, 174, 233];

fn kde_globals() -> Option<String> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    std::fs::read_to_string(base.join("kdeglobals")).ok()
}

/// A value from an INI text: `key` in `[section]`. Keys may carry flags,
/// as KDE writes them (`Key[$e]=...`).
fn ini_get<'a>(text: &'a str, section: &str, key: &str) -> Option<&'a str> {
    let mut inside = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            inside = l == format!("[{section}]");
            continue;
        }
        if inside && !l.starts_with('#') {
            if let Some((k, v)) = l.split_once('=') {
                if k.split('[').next().unwrap_or(k).trim() == key {
                    return Some(v.trim());
                }
            }
        }
    }
    None
}

/// `r,g,b` (KDE's colour form; a fourth alpha number is ignored), or
/// `#rrggbb`.
fn parse_rgb(s: &str) -> Option<Rgb> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix('#') {
        if h.len() >= 6 {
            let n = u32::from_str_radix(&h[h.len() - 6..], 16).ok()?;
            return Some([(n >> 16) as u8, (n >> 8) as u8, n as u8]);
        }
        return None;
    }
    let v: Vec<u8> = s
        .split(',')
        .map(|x| x.trim().parse::<u8>())
        .collect::<Result<_, _>>()
        .ok()?;
    (v.len() >= 3).then(|| [v[0], v[1], v[2]])
}

fn kde_colors(k: &str) -> DesktopColors {
    let get = |sec: &str, key: &str| ini_get(k, sec, key).and_then(parse_rgb);
    // A colour from a group, else the Window group's, as KDE itself does.
    let view = |key: &str| get("Colors:View", key).or_else(|| get("Colors:Window", key));
    let window = get("Colors:Window", "BackgroundNormal");
    let accent = get("General", "AccentColor")
        .or_else(|| get("Colors:Selection", "BackgroundNormal"))
        .or_else(|| view("DecorationFocus"));
    DesktopColors {
        dark: window.map(|w| luminance(w) < 0.18),
        accent,
        window,
        window_text: get("Colors:Window", "ForegroundNormal"),
        view: get("Colors:View", "BackgroundNormal"),
        button: get("Colors:Button", "BackgroundNormal"),
        dim_text: get("Colors:Window", "ForegroundInactive"),
        highlight_text: get("Colors:Selection", "ForegroundNormal"),
        background: get("Colors:Window", "BackgroundAlternate"),
        selection: get("Colors:Selection", "BackgroundNormal"),
        accent_alt: view("DecorationHover")
            .filter(|h| Some(*h) != accent)
            .or_else(|| view("ForegroundVisited")),
        view_text: view("ForegroundNormal"),
        negative: view("ForegroundNegative"),
        neutral: view("ForegroundNeutral"),
        positive: view("ForegroundPositive"),
    }
}

/// Plasma's Breeze scheme, for a desktop that has not changed it.
fn breeze(dark: bool) -> DesktopColors {
    let (window, view, text, button, dim, alt) = if dark {
        ([32, 35, 38], [20, 22, 24], [252, 252, 252], [41, 44, 48], [161, 169, 177], [27, 30, 32])
    } else {
        ([239, 240, 241], [255, 255, 255], [35, 38, 41], [252, 252, 252], [112, 125, 138], [227, 229, 231])
    };
    DesktopColors {
        dark: Some(dark),
        accent: Some(BREEZE_BLUE),
        window: Some(window),
        window_text: Some(text),
        view: Some(view),
        button: Some(button),
        dim_text: Some(dim),
        highlight_text: Some([252, 252, 252]),
        background: Some(alt),
        selection: Some(BREEZE_BLUE),
        accent_alt: Some([155, 89, 182]),
        view_text: Some(text),
        negative: Some([218, 68, 83]),
        neutral: Some([246, 116, 0]),
        positive: Some([39, 174, 96]),
    }
}

/// `Noto Sans,10,-1,5,400,0,0,0,0,0,0,0,0,0,0,1` → Noto Sans, 10 pt.
fn kde_font(v: &str) -> DesktopFont {
    let mut parts = v.split(',');
    let family = parts.next().unwrap_or("").trim().to_string();
    let points = parts.next().and_then(|p| p.trim().parse::<f32>().ok()).filter(|p| *p > 0.0);
    DesktopFont { family, points }
}

// ------------------------------------------------------------ GNOME

fn gsettings(schema: &str, key: &str) -> Option<String> {
    let out = std::process::Command::new("gsettings")
        .args(["get", schema, key])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().trim_matches('\'').to_string();
    (!s.is_empty()).then_some(s)
}

fn gnome_colors() -> DesktopColors {
    let scheme = gsettings("org.gnome.desktop.interface", "color-scheme");
    let accent = gsettings("org.gnome.desktop.interface", "accent-color")
        .as_deref()
        .and_then(gnome_accent);
    let dark = match scheme.as_deref() {
        Some("prefer-dark") => Some(true),
        Some("prefer-light") => Some(false),
        // No stated preference: GTK desktops name a dark theme `...-dark`.
        _ => gsettings("org.gnome.desktop.interface", "gtk-theme").map(|n| n.to_ascii_lowercase().contains("dark")),
    };
    match dark {
        Some(d) => adwaita(d, accent),
        None => DesktopColors {
            accent,
            ..DesktopColors::default()
        },
    }
}

/// GNOME's (Adwaita's) neutral greys and status colours.
fn adwaita(dark: bool, accent: Option<Rgb>) -> DesktopColors {
    let (positive, neutral, negative) = if dark {
        ([46, 194, 126], [246, 211, 45], [237, 51, 59])
    } else {
        ([38, 162, 105], [229, 165, 10], [224, 27, 36])
    };
    DesktopColors {
        dark: Some(dark),
        accent,
        window: Some(if dark { [34, 34, 38] } else { [250, 250, 251] }),
        view: Some(if dark { [29, 29, 32] } else { [255, 255, 255] }),
        button: Some(if dark { [50, 50, 54] } else { [235, 235, 237] }),
        background: Some(if dark { [24, 24, 27] } else { [235, 235, 237] }),
        window_text: Some(if dark { [255, 255, 255] } else { [51, 51, 56] }),
        dim_text: Some(if dark { [154, 154, 157] } else { [120, 120, 125] }),
        positive: Some(positive),
        neutral: Some(neutral),
        negative: Some(negative),
        ..DesktopColors::default()
    }
}

/// GNOME's named accents (GNOME 47 and later).
fn gnome_accent(name: &str) -> Option<Rgb> {
    Some(match name {
        "blue" => [0x35, 0x84, 0xe4],
        "teal" => [0x21, 0x90, 0xa4],
        "green" => [0x3a, 0x94, 0x4a],
        "yellow" => [0xc8, 0x88, 0x00],
        "orange" => [0xed, 0x5b, 0x00],
        "red" => [0xe6, 0x2d, 0x42],
        "pink" => [0xd5, 0x61, 0x99],
        "purple" => [0x91, 0x41, 0xac],
        "slate" => [0x6f, 0x83, 0x96],
        _ => return None,
    })
}

/// `Cantarell 11` → Cantarell, 11 pt.
fn gnome_font(v: &str) -> DesktopFont {
    match v.rsplit_once(' ') {
        Some((fam, size)) if size.parse::<f32>().is_ok() => DesktopFont {
            family: fam.trim().to_string(),
            points: size.parse().ok(),
        },
        _ => DesktopFont {
            family: v.trim().to_string(),
            points: None,
        },
    }
}

/// fontconfig's file for a family: (path, face index).
fn fc_match(family: &str, bold: bool) -> Option<(PathBuf, u32)> {
    if family.is_empty() {
        return None;
    }
    let pattern = if bold {
        format!("{family}:bold")
    } else {
        format!("{family}:regular")
    };
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}\n%{index}\n", &pattern])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let file = PathBuf::from(lines.next()?.trim());
    let index = lines.next().and_then(|i| i.trim().parse().ok()).unwrap_or(0);
    // egui reads TrueType and OpenType, not Type 1 or bitmap fonts.
    let ext = file.extension()?.to_string_lossy().to_ascii_lowercase();
    matches!(ext.as_str(), "ttf" | "otf" | "ttc" | "otc").then_some((file, index))
}

// ------------------------------------------------------------ Windows

/// A DWORD under HKEY_CURRENT_USER, read straight from the registry (no
/// `reg` process).
#[cfg(windows)]
fn reg_dword(key: &str, value: &str) -> Option<u32> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    hkcu.open_subkey(key).ok()?.get_value::<u32, _>(value).ok()
}

#[cfg(not(windows))]
fn reg_dword(_key: &str, _value: &str) -> Option<u32> {
    None
}

fn windows_colors() -> DesktopColors {
    let light = reg_dword(
        r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
        "AppsUseLightTheme",
    );
    let dark = light.map(|l| l == 0);
    // AccentColor is 0xAABBGGRR.
    let accent = reg_dword(r"Software\Microsoft\Windows\DWM", "AccentColor")
        .map(|v| [v as u8, (v >> 8) as u8, (v >> 16) as u8]);
    DesktopColors {
        dark,
        accent,
        // Windows 11's own window and content backgrounds.
        window: dark.map(|d| if d { [32, 32, 32] } else { [243, 243, 243] }),
        view: dark.map(|d| if d { [39, 39, 39] } else { [251, 251, 251] }),
        ..DesktopColors::default()
    }
}

// ------------------------------------------------------------ macOS

fn defaults_read(key: &str) -> Option<String> {
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", key])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn mac_colors() -> DesktopColors {
    let dark = Some(defaults_read("AppleInterfaceStyle").is_some_and(|s| s == "Dark"));
    let accent = match defaults_read("AppleAccentColor").and_then(|s| s.parse::<i32>().ok()) {
        Some(-1) => [0x8c, 0x8c, 0x8c],
        Some(0) => [0xff, 0x52, 0x57],
        Some(1) => [0xf7, 0x82, 0x1b],
        Some(2) => [0xff, 0xc6, 0x00],
        Some(3) => [0x62, 0xba, 0x46],
        Some(5) => [0xa5, 0x50, 0xa7],
        Some(6) => [0xf7, 0x4f, 0x9e],
        _ => [0x00, 0x7a, 0xff],
    };
    DesktopColors {
        dark,
        accent: Some(accent),
        window: dark.map(|d| if d { [40, 40, 40] } else { [236, 236, 236] }),
        view: dark.map(|d| if d { [30, 30, 30] } else { [255, 255, 255] }),
        ..DesktopColors::default()
    }
}

// ------------------------------------------------------------ contrast

/// 0 (black) to 1 (white), by eye (gamma-encoded).
#[cfg(test)]
pub fn luma(c: Rgb) -> f32 {
    (0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32) / 255.0
}

/// WCAG relative luminance, 0 (black) to 1 (white).
pub fn luminance(c: Rgb) -> f32 {
    let lin = |x: u8| {
        let s = x as f32 / 255.0;
        if s <= 0.040_45 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

/// WCAG contrast ratio, 1 to 21.
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

/// `fg`, moved toward white or black (whichever stands further from `bg`)
/// just far enough to reach `min` contrast against `bg`.
pub fn ensure_contrast(fg: Rgb, bg: Rgb, min: f32) -> Rgb {
    if contrast(fg, bg) >= min {
        return fg;
    }
    let target = if contrast([255; 3], bg) >= contrast([0; 3], bg) {
        [255; 3]
    } else {
        [0; 3]
    };
    for step in 1..=20 {
        let c = mix(fg, target, step as f32 / 20.0);
        if contrast(c, bg) >= min {
            return c;
        }
    }
    target
}

/// Keeps a theme readable whatever the desktop's colours: text against
/// the panels (4.5:1; dim text and the accent 3:1), the waveform,
/// playhead, markers and meters against the view (3:1), and a selection
/// wash that can be seen.
pub fn guard(t: &mut crate::theme::Theme) {
    t.text = ensure_contrast(t.text, t.panel, 4.5);
    t.text_dim = ensure_contrast(t.text_dim, t.panel, 3.0);
    t.accent = ensure_contrast(t.accent, t.panel, 3.0);
    let bg = t.wave_background;
    for c in [
        &mut t.wave_peak,
        &mut t.wave_centre,
        &mut t.playhead,
        &mut t.marker,
        &mut t.meter_low,
        &mut t.meter_mid,
        &mut t.meter_high,
    ] {
        *c = ensure_contrast(*c, bg, 3.0);
    }
    // The selection's edge is drawn nearly opaque, its wash faint.
    t.selection = ensure_contrast(t.selection, bg, 3.0);
    while t.selection_alpha < 110 {
        let wash = mix(bg, t.selection, t.selection_alpha as f32 / 255.0);
        if contrast(wash, bg) >= 1.25 {
            break;
        }
        t.selection_alpha += 2;
    }
}

// ------------------------------------------------------------ live

/// What the XDG Settings portal says about the desktop's look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Appearance {
    pub dark: bool,
    pub accent: Option<Rgb>,
}

/// The portal's latest answer (None until it answers, or with no portal).
static PORTAL: Mutex<Option<Appearance>> = Mutex::new(None);
/// The latest reading, waiting for the UI thread.
static LATEST: Mutex<Option<DesktopColors>> = Mutex::new(None);
static CTX: OnceLock<egui::Context> = OnceLock::new();
/// Milliseconds (from `epoch`, plus one) of the portal's last relevant
/// signal; 0 when nothing waits.
static SIGNAL: AtomicU64 = AtomicU64::new(0);
/// The portal is listening, so there is no need to poll.
static PORTAL_LIVE: AtomicBool = AtomicBool::new(false);

/// Desktops write a change as a burst of settings: wait this long after
/// the last one.
const SETTLE: Duration = Duration::from_millis(200);
/// How often the desktop is read when nothing says it changed.
const POLL: Duration = Duration::from_secs(3);

fn epoch() -> Instant {
    static E: OnceLock<Instant> = OnceLock::new();
    *E.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    epoch().elapsed().as_millis() as u64 + 1
}

fn portal_appearance() -> Option<Appearance> {
    PORTAL.lock().ok().and_then(|p| *p)
}

/// Starts following the desktop (once; later calls do nothing). Called
/// every frame the System theme is shown; nothing runs until then.
pub fn watch(ctx: &egui::Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let _ = CTX.set(ctx.clone());
    epoch();
    #[cfg(target_os = "linux")]
    portal::start();
    let _ = std::thread::Builder::new()
        .name("system-look".into())
        .spawn(|| {
            let mut last = colors();
            let mut polled = Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(100));
                let sig = SIGNAL.load(Ordering::Relaxed);
                let signalled = sig != 0
                    && now_ms().saturating_sub(sig) >= SETTLE.as_millis() as u64
                    && SIGNAL
                        .compare_exchange(sig, 0, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok();
                let poll = !PORTAL_LIVE.load(Ordering::Relaxed) && polled.elapsed() >= POLL;
                if !(signalled || poll) {
                    continue;
                }
                polled = Instant::now();
                let now = colors();
                if now != last {
                    last = now.clone();
                    if let Ok(mut l) = LATEST.lock() {
                        *l = Some(now);
                    }
                    if let Some(ctx) = CTX.get() {
                        ctx.request_repaint();
                    }
                }
            }
        });
}

/// The desktop's colours, if they changed since last asked.
pub fn take_change() -> Option<DesktopColors> {
    LATEST.lock().ok()?.take()
}

/// Something the portal said: a new appearance, or a KDE scheme change.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn signal(appearance: Option<Appearance>) {
    if let Some(a) = appearance {
        if let Ok(mut p) = PORTAL.lock() {
            *p = Some(a);
        }
    }
    SIGNAL.store(now_ms(), Ordering::Relaxed);
}

/// The XDG Settings portal, on its own thread: light or dark and the
/// accent, and a signal whenever the desktop's look changes.
#[cfg(target_os = "linux")]
mod portal {
    use super::*;

    pub fn start() {
        let _ = std::thread::Builder::new()
            .name("system-look-portal".into())
            .spawn(|| {
                pollster::block_on(listen());
                PORTAL_LIVE.store(false, Ordering::Relaxed);
            });
    }

    async fn read(s: &ashpd::desktop::settings::Settings<'_>) -> Appearance {
        use ashpd::desktop::settings::ColorScheme;
        let dark = match s.color_scheme().await {
            Ok(ColorScheme::PreferDark) => true,
            Ok(ColorScheme::PreferLight) => false,
            // No preference: GTK desktops name a dark theme `...-dark`.
            Ok(ColorScheme::NoPreference) => s
                .read::<String>("org.gnome.desktop.interface", "gtk-theme")
                .await
                .is_ok_and(|n| n.to_ascii_lowercase().contains("dark")),
            Err(_) => false,
        };
        let accent = s.accent_color().await.ok().and_then(|c| {
            let ch = [c.red(), c.green(), c.blue()];
            ch.iter()
                .all(|x| (0.0..=1.0).contains(x))
                .then(|| ch.map(|x| (x * 255.0).round() as u8))
        });
        Appearance { dark, accent }
    }

    async fn listen() -> Option<()> {
        use futures_util::StreamExt;
        let settings = ashpd::desktop::settings::Settings::new().await.ok()?;
        // Listen first, so nothing changes unseen between reading and
        // listening.
        let changes = settings.receive_setting_changed().await.ok();
        signal(Some(read(&settings).await));
        let mut changes = changes?;
        PORTAL_LIVE.store(true, Ordering::Relaxed);
        while let Some(s) = changes.next().await {
            let ns = s.namespace();
            if ns == "org.freedesktop.appearance" || ns == "org.gnome.desktop.interface" {
                signal(Some(read(&settings).await));
            } else if ns.starts_with("org.kde.kdeglobals") {
                signal(None);
            }
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    const BREEZE_DARK: &str = "[ColorEffects:Disabled]\nColor=56,56,56\n\n\
        [Colors:Button]\nBackgroundNormal=41,44,48\n\n\
        [Colors:Selection]\nBackgroundNormal=61,174,233\nForegroundNormal=252,252,252\n\n\
        [Colors:View]\nBackgroundNormal=20,22,24\nDecorationFocus=61,174,233\n\n\
        [Colors:Window]\nBackgroundNormal=32,35,38\nForegroundInactive=161,169,177\nForegroundNormal=252,252,252\n\n\
        [General]\nColorScheme=BreezeDark\nfont=Noto Sans,10,-1,5,400,0,0,0,0,0,0,0,0,0,0,1\n";

    // From b1g-b8lls' tests: a real OxygenDark kdeglobals and a trimmed
    // Breeze Light one.
    const OXYGEN_DARK: &str = "\
[ColorEffects:Disabled]
Color=56,56,56

[Colors:Button]
BackgroundAlternate=59,137,178
BackgroundNormal=48,55,60
DecorationFocus=61,174,233
DecorationHover=61,174,233
ForegroundInactive=126,138,146
ForegroundNormal=242,238,239

[Colors:Selection]
BackgroundNormal=51,130,171
ForegroundNormal=0,0,0

[Colors:View]
BackgroundAlternate=24,33,39
BackgroundNormal=21,31,37
DecorationFocus=61,174,233
DecorationHover=61,174,233
ForegroundInactive=126,138,146
ForegroundNegative=173,52,49
ForegroundNeutral=161,134,66
ForegroundNormal=241,234,236
ForegroundPositive=0,110,63
ForegroundVisited=90,79,155

[Colors:Window]
BackgroundAlternate=22,33,39
BackgroundNormal=29,38,44
DecorationFocus=61,174,233
ForegroundInactive=126,138,146
ForegroundNormal=241,236,237

[General]
AccentColor[$e]=61,174,233
ColorScheme=OxygenDark
";

    const BREEZE_LIGHT: &str = "\
[Colors:Button]
BackgroundNormal=252,252,252
[Colors:Selection]
BackgroundNormal=61,174,233
[Colors:View]
BackgroundNormal=255,255,255
ForegroundNormal=35,38,41
ForegroundNegative=218,68,83
ForegroundNeutral=246,116,0
ForegroundPositive=39,174,96
[Colors:Window]
BackgroundAlternate=227,229,231
BackgroundNormal=239,240,241
DecorationFocus=61,174,233
ForegroundInactive=112,125,138
ForegroundNormal=35,38,41
";

    fn readable(t: &Theme) {
        assert!(contrast(t.text, t.panel) >= 4.5, "text {:?} on {:?}", t.text, t.panel);
        assert!(contrast(t.text_dim, t.panel) >= 3.0);
        assert!(contrast(t.accent, t.panel) >= 3.0);
        for c in [t.wave_peak, t.wave_centre, t.playhead, t.marker, t.meter_low, t.meter_mid, t.meter_high, t.selection] {
            assert!(contrast(c, t.wave_background) >= 3.0, "{c:?} on {:?}", t.wave_background);
        }
    }

    #[test]
    fn plasma_colours_and_font_are_read_from_kdeglobals() {
        let c = kde_colors(BREEZE_DARK);
        assert_eq!(c.window, Some([32, 35, 38]));
        assert_eq!(c.view, Some([20, 22, 24]));
        assert_eq!(c.button, Some([41, 44, 48]));
        assert_eq!(c.window_text, Some([252, 252, 252]));
        assert_eq!(c.dim_text, Some([161, 169, 177]));
        assert_eq!(c.accent, Some([61, 174, 233]), "no AccentColor: the selection's");
        assert_eq!(c.dark, Some(true));
        let with_accent = format!("{BREEZE_DARK}AccentColor=233,100,61\n");
        assert_eq!(kde_colors(&with_accent).accent, Some([233, 100, 61]));
        let f = kde_font(ini_get(BREEZE_DARK, "General", "font").unwrap());
        assert_eq!(f.family, "Noto Sans");
        assert_eq!(f.points, Some(10.0));
    }

    #[test]
    fn oxygen_dark_maps_every_colour() {
        let c = kde_colors(OXYGEN_DARK);
        assert_eq!(c.accent, Some([61, 174, 233]), "AccentColor[$e] is still the accent");
        assert_eq!(c.background, Some([22, 33, 39]));
        assert_eq!(c.selection, Some([51, 130, 171]));
        // Hover is the accent itself: the visited colour instead.
        assert_eq!(c.accent_alt, Some([90, 79, 155]));
        assert_eq!(c.negative, Some([173, 52, 49]));
        assert_eq!(c.neutral, Some([161, 134, 66]));
        assert_eq!(c.positive, Some([0, 110, 63]));
        let t = Theme::system_from(&c);
        assert!(t.dark);
        assert_eq!(t.panel, [29, 38, 44]);
        assert_eq!(t.background, [22, 33, 39]);
        assert_eq!(t.wave_background, [21, 31, 37]);
        // The dark green "good" colour is lifted until it reads.
        assert_ne!(t.meter_low, [0, 110, 63]);
        readable(&t);
    }

    #[test]
    fn breeze_light_is_light_and_readable() {
        let c = kde_colors(BREEZE_LIGHT);
        assert_eq!(c.dark, Some(false));
        assert_eq!(c.accent, Some([61, 174, 233]), "the selection, with no AccentColor");
        let t = Theme::system_from(&c);
        assert!(!t.dark);
        assert_eq!(t.panel, [239, 240, 241]);
        assert_eq!(t.text, [35, 38, 41]);
        readable(&t);
        // Breeze's own defaults (no colour groups) match the file.
        let b = breeze(false);
        assert_eq!(b.window, c.window);
        assert_eq!(b.negative, c.negative);
        assert_eq!(breeze(true).window, kde_colors(BREEZE_DARK).window);
    }

    #[test]
    fn the_system_theme_takes_the_desktops_colours() {
        let t = Theme::system_from(&kde_colors(BREEZE_DARK));
        assert_eq!(t.name, "System");
        assert!(crate::theme::is_system(&t));
        assert!(t.dark);
        assert_eq!(t.panel, [32, 35, 38]);
        assert_eq!(t.text, [252, 252, 252]);
        assert_eq!(t.accent, [61, 174, 233], "bright enough already");
        assert_eq!(t.font, "system");
        let light = DesktopColors {
            dark: Some(false),
            accent: Some([0, 120, 215]),
            ..DesktopColors::default()
        };
        let t = Theme::system_from(&light);
        assert!(!t.dark);
        assert!(luma(t.panel) > 0.8 && luma(t.text) < 0.3, "dark text on light panels");
        readable(&t);
        // A dim accent on a dark desktop is lifted so it reads.
        let dim = DesktopColors {
            dark: Some(true),
            accent: Some([90, 70, 10]),
            ..DesktopColors::default()
        };
        readable(&Theme::system_from(&dim));
        // Nothing known: a dark theme, still readable.
        let t = Theme::system_from(&DesktopColors::default());
        assert!(luma(t.text) - luma(t.panel) > 0.5);
        readable(&t);
    }

    #[test]
    fn a_pale_accent_on_white_is_deepened() {
        for d in [adwaita(false, Some([250, 240, 200])), adwaita(true, Some([20, 20, 60]))] {
            let t = Theme::system_from(&d);
            readable(&t);
            // Its hue stays: still more red/green than blue for the pale
            // yellow, still bluest for the navy.
            let a = t.accent;
            if d.dark == Some(false) {
                assert!(a[0] > a[2]);
            } else {
                assert!(a[2] >= a[0]);
            }
        }
    }

    #[test]
    fn contrast_follows_wcag() {
        assert!((contrast([0; 3], [255; 3]) - 21.0).abs() < 0.01);
        assert!((contrast([7; 3], [7; 3]) - 1.0).abs() < 1e-6);
        let c = ensure_contrast([120, 120, 120], [128, 128, 128], 4.5);
        assert!(contrast(c, [128, 128, 128]) >= 4.5);
        assert_eq!(ensure_contrast([255; 3], [0; 3], 4.5), [255; 3], "already fine: untouched");
    }

    #[test]
    fn guard_makes_a_selection_wash_visible() {
        let mut t = Theme::system_from(&adwaita(false, Some([200, 200, 205])));
        t.selection_alpha = 4;
        guard(&mut t);
        let bg = t.wave_background;
        let wash = mix(bg, t.selection, t.selection_alpha as f32 / 255.0);
        assert!(contrast(wash, bg) >= 1.25 || t.selection_alpha >= 110);
    }

    #[test]
    fn gnome_font_names_and_colour_forms_parse() {
        assert_eq!(gnome_font("Cantarell 11").family, "Cantarell");
        assert_eq!(gnome_font("Adwaita Sans 10.5").points, Some(10.5));
        assert_eq!(gnome_font("Inter").family, "Inter");
        assert_eq!(parse_rgb("#3584e4"), Some([0x35, 0x84, 0xe4]));
        assert_eq!(parse_rgb("1, 2, 3, 255"), Some([1, 2, 3]));
        assert_eq!(parse_rgb("nope"), None);
        assert_eq!(gnome_accent("blue"), Some([0x35, 0x84, 0xe4]));
    }

    #[test]
    fn flagged_and_commented_keys() {
        let t = "[General]\n# AccentColor=1,2,3\nAccentColor[$e]=4,5,6\n";
        assert_eq!(ini_get(t, "General", "AccentColor"), Some("4,5,6"));
    }
}
