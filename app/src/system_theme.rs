// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The System theme (Linux only): a flat theme in the desktop's colours,
//! first in View > Theme.
//!
//! - On KDE Plasma it takes the whole colour scheme from `kdeglobals`:
//!   windows, panels, text, accent, selection, playhead, markers, meters.
//! - Elsewhere (GNOME, COSMIC, Xfce, ...) the XDG Settings portal gives
//!   light or dark and, where the desktop has one, an accent colour; the
//!   rest is a neutral palette close to GNOME's own.
//! - With neither, it is the neutral dark palette with a blue accent.
//!
//! The waveform follows the accent; the spectrogram and track colours are
//! the theme's own. A contrast check keeps everything readable whatever
//! the desktop's colours are. A background thread listens to the portal's
//! `SettingChanged` signal so a change in the desktop's settings shows at
//! once. Nothing here runs unless the System theme is chosen.

use crate::theme::{Rgb, Theme};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The System theme as the desktop has it now. Reads `kdeglobals` on KDE;
/// elsewhere the first call waits up to 200 ms for the portal's answer.
pub fn current() -> Theme {
    let mut t = match palette() {
        Some(p) => {
            let dark = p
                .get("Window", "BackgroundNormal")
                .is_some_and(|bg| luminance(bg) < 0.18);
            let mut t = base(dark, p.accent().unwrap_or(BLUE));
            p.paint(&mut t);
            t
        }
        None => {
            let a = appearance();
            base(a.dark, a.accent.unwrap_or(BLUE))
        }
    };
    guard(&mut t);
    t
}

/// The entry in the theme list, before the desktop has been asked.
pub fn placeholder() -> Theme {
    base(true, BLUE)
}

// ------------------------------------------------------- the palette

/// GNOME's blue, the accent when the desktop has none.
const BLUE: Rgb = [53, 132, 228];

/// A flat theme in neutral greys close to GNOME's (Adwaita), with
/// `accent` for the accent, selection and waveform.
fn base(dark: bool, accent: Rgb) -> Theme {
    let (background, panel, raised, view, text, text_dim) = if dark {
        (
            [24, 24, 27],
            [34, 34, 38],
            [50, 50, 54],
            [29, 29, 32],
            [255, 255, 255],
            [154, 154, 157],
        )
    } else {
        (
            [235, 235, 237],
            [250, 250, 251],
            [235, 235, 237],
            [255, 255, 255],
            [51, 51, 56],
            [120, 120, 125],
        )
    };
    let (green, yellow, red) = if dark {
        ([46, 194, 126], [246, 211, 45], [237, 51, 59])
    } else {
        ([38, 162, 105], [229, 165, 10], [224, 27, 36])
    };
    Theme {
        name: crate::theme::SYSTEM_NAME.into(),
        author: crate::theme::SYSTEM_AUTHOR.into(),
        dark,
        background,
        panel,
        panel_raised: raised,
        border: mix(panel, text, 0.15),
        text,
        text_dim,
        accent,
        accent_alt: mix(accent, text, 0.4),
        wave_peak: mix(accent, text, 0.45),
        wave_centre: accent,
        wave_background: view,
        grid: mix(view, text, 0.08),
        selection: accent,
        selection_alpha: 50,
        playhead: red,
        marker: yellow,
        // Inferno: dark to bright, readable on light and dark alike.
        spectrum: vec![
            [0, 0, 4],
            [40, 11, 84],
            [101, 21, 110],
            [159, 42, 99],
            [212, 72, 66],
            [245, 125, 21],
            [250, 193, 39],
            [252, 255, 164],
        ],
        meter_low: green,
        meter_mid: yellow,
        meter_high: red,
        track_colors: vec![
            [53, 132, 228],
            [46, 194, 126],
            [245, 194, 17],
            [255, 120, 0],
            [224, 27, 36],
            [145, 65, 172],
            [152, 106, 68],
            [33, 144, 164],
        ],
        rounding: 4.0,
        glow: 0.0,
        backdrop_top: panel,
        backdrop_bottom: panel,
        grid_style: "none".into(),
        grid_glow: accent,
        grid_strength: 0.0,
        glass: 1.0,
        chamfer: 0.0,
        brackets: false,
        scanlines: 0.0,
        motion: 0.0,
        reactive: 0.0,
        intro: false,
        vignette: 0.0,
        realistic: false,
        ..Theme::zero_point()
    }
}

// ---------------------------------------------------------------- KDE

/// Whether this is a KDE Plasma session. `XDG_CURRENT_DESKTOP` is a
/// colon-separated list; without it, a readable `kdeglobals` counts.
pub fn is_kde() -> bool {
    match std::env::var("XDG_CURRENT_DESKTOP") {
        Ok(v) => desktop_is_kde(&v),
        Err(_) => path().is_some_and(|p| p.is_file()),
    }
}

fn desktop_is_kde(v: &str) -> bool {
    v.split(':').any(|d| d.trim().eq_ignore_ascii_case("KDE"))
}

fn path() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".config")))?;
    Some(dir.join("kdeglobals"))
}

/// KDE's colours, by group (`Colors:View`, `General`, ...) and key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Palette {
    groups: HashMap<String, HashMap<String, Rgb>>,
}

/// The current KDE scheme, or None when this isn't KDE or the file has no
/// usable colours.
pub fn palette() -> Option<Palette> {
    if !is_kde() {
        return None;
    }
    Palette::parse(&std::fs::read_to_string(path()?).ok()?)
}

fn parse_rgb(v: &str) -> Option<Rgb> {
    let mut it = v.split(',').map(|x| x.trim().parse::<u8>());
    let rgb = [it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?];
    Some(rgb)
}

impl Palette {
    /// Reads the colour entries of a `kdeglobals` file. None when the
    /// Window group lacks its background or text colour.
    pub fn parse(text: &str) -> Option<Palette> {
        let mut groups: HashMap<String, HashMap<String, Rgb>> = HashMap::new();
        let mut group = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                group = line
                    .strip_prefix('[')
                    .and_then(|l| l.strip_suffix(']'))
                    .unwrap_or("")
                    .to_string();
                continue;
            }
            if line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            // Keys can carry flags like `Key[$e]`.
            let k = k.split('[').next().unwrap_or(k).trim();
            if let Some(rgb) = parse_rgb(v) {
                groups
                    .entry(group.clone())
                    .or_default()
                    .insert(k.to_string(), rgb);
            }
        }
        let p = Palette { groups };
        p.raw("Colors:Window", "BackgroundNormal")?;
        p.raw("Colors:Window", "ForegroundNormal")?;
        Some(p)
    }

    fn raw(&self, group: &str, key: &str) -> Option<Rgb> {
        self.groups.get(group)?.get(key).copied()
    }

    /// A colour from `group`, else the Window group's.
    fn get(&self, group: &str, key: &str) -> Option<Rgb> {
        self.raw(&format!("Colors:{group}"), key)
            .or_else(|| self.raw("Colors:Window", key))
    }

    /// The accent: `[General] AccentColor`, else the focus colour.
    fn accent(&self) -> Option<Rgb> {
        self.raw("General", "AccentColor")
            .or_else(|| self.get("Window", "DecorationFocus"))
    }

    /// Puts KDE's colours on `t`. The waveform, spectrogram and track
    /// colours stay as they are.
    pub fn paint(&self, t: &mut Theme) {
        let set = |field: &mut Rgb, group: &str, key: &str| {
            if let Some(v) = self.get(group, key) {
                *field = v;
            }
        };
        set(&mut t.panel, "Window", "BackgroundNormal");
        set(&mut t.background, "Window", "BackgroundAlternate");
        set(&mut t.panel_raised, "Button", "BackgroundNormal");
        set(&mut t.text, "Window", "ForegroundNormal");
        set(&mut t.text_dim, "Window", "ForegroundInactive");
        set(&mut t.wave_background, "View", "BackgroundNormal");
        set(&mut t.selection, "Selection", "BackgroundNormal");
        set(&mut t.playhead, "View", "ForegroundNegative");
        set(&mut t.marker, "View", "ForegroundNeutral");
        set(&mut t.meter_low, "View", "ForegroundPositive");
        set(&mut t.meter_mid, "View", "ForegroundNeutral");
        set(&mut t.meter_high, "View", "ForegroundNegative");
        if let Some(a) = self.accent() {
            t.accent = a;
        }
        t.accent_alt = match self.get("View", "DecorationHover") {
            Some(h) if h != t.accent => h,
            _ => self
                .get("View", "ForegroundVisited")
                .unwrap_or(t.accent_alt),
        };
        t.dark = luminance(t.panel) < 0.18;
        t.border = mix(t.panel, t.text, 0.25);
        let view_fg = self.get("View", "ForegroundNormal").unwrap_or(t.text);
        t.grid = mix(t.wave_background, view_fg, 0.12);
        t.backdrop_top = t.panel;
        t.backdrop_bottom = t.panel;
        t.grid_glow = t.accent;
    }
}

// ------------------------------------------------------------ contrast

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
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

/// Keeps the theme readable: text against panels, the waveform, playhead,
/// markers and meters against the view background, and a selection wash
/// that can be seen.
pub fn guard(t: &mut Theme) {
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

// ------------------------------------------------- the portal, live

/// What the portal says about the desktop's look.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Appearance {
    dark: bool,
    accent: Option<Rgb>,
}

/// Without a portal: dark, with no accent of the desktop's.
const NO_PORTAL: Appearance = Appearance {
    dark: true,
    accent: None,
};

static APPEARANCE: Mutex<Option<Appearance>> = Mutex::new(None);
/// The listener has given its first answer (or given up).
static READY: AtomicBool = AtomicBool::new(false);
/// For a repaint when the desktop changes.
static CTX: OnceLock<egui::Context> = OnceLock::new();

/// Milliseconds since `epoch()` of the last relevant change, plus one
/// (0: nothing waiting).
static LAST_SIGNAL: AtomicU64 = AtomicU64::new(0);

fn epoch() -> Instant {
    static E: OnceLock<Instant> = OnceLock::new();
    *E.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    epoch().elapsed().as_millis() as u64 + 1
}

/// Desktops write a change as a burst of settings: wait this long after
/// the last one.
const SETTLE: Duration = Duration::from_millis(200);

/// What the portal says, starting the listener on first use and waiting
/// for its first answer (at most 200 ms; a missing portal answers at once).
fn appearance() -> Appearance {
    start();
    let until = Instant::now() + Duration::from_millis(200);
    while !READY.load(Ordering::Acquire) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    APPEARANCE.lock().ok().and_then(|a| *a).unwrap_or(NO_PORTAL)
}

/// Lets the listener wake the UI when the desktop changes, and starts it.
pub fn attach(ctx: &egui::Context) {
    if CTX.get().is_none() {
        let _ = CTX.set(ctx.clone());
    }
    start();
}

fn start() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    epoch();
    let spawned = std::thread::Builder::new()
        .name("system-theme".into())
        .spawn(|| {
            pollster::block_on(listen());
            READY.store(true, Ordering::Release);
        });
    if spawned.is_err() {
        READY.store(true, Ordering::Release);
    }
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
        Err(_) => NO_PORTAL.dark,
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
    // Listen first, so nothing changes unseen between reading and listening.
    let changes = settings.receive_setting_changed().await.ok();
    let a = read(&settings).await;
    if let Ok(mut slot) = APPEARANCE.lock() {
        *slot = Some(a);
    }
    READY.store(true, Ordering::Release);
    let mut changes = changes?;
    while let Some(s) = changes.next().await {
        let ns = s.namespace();
        if ns == "org.freedesktop.appearance" || ns == "org.gnome.desktop.interface" {
            let a = read(&settings).await;
            if let Ok(mut slot) = APPEARANCE.lock() {
                *slot = Some(a);
            }
        } else if !ns.starts_with("org.kde.kdeglobals") {
            continue;
        }
        LAST_SIGNAL.store(now_ms(), Ordering::Relaxed);
        if let Some(ctx) = CTX.get() {
            ctx.request_repaint_after(SETTLE + Duration::from_millis(50));
        }
    }
    Some(())
}

/// True once per settled burst of changes to the desktop's look.
pub fn changed() -> bool {
    let t = LAST_SIGNAL.load(Ordering::Relaxed);
    if t == 0 || now_ms().saturating_sub(t) < SETTLE.as_millis() as u64 {
        return false;
    }
    LAST_SIGNAL
        .compare_exchange(t, 0, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

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
AccentColor=61,174,233
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

    fn kde(text: &str) -> Theme {
        let p = Palette::parse(text).unwrap();
        let dark = luminance(p.get("Window", "BackgroundNormal").unwrap()) < 0.18;
        let mut t = base(dark, p.accent().unwrap_or(BLUE));
        p.paint(&mut t);
        t
    }

    #[test]
    fn desktops() {
        assert!(desktop_is_kde("KDE"));
        assert!(desktop_is_kde("ubuntu:KDE"));
        assert!(desktop_is_kde("kde"));
        assert!(!desktop_is_kde("GNOME"));
        assert!(!desktop_is_kde("ubuntu:GNOME"));
        assert!(!desktop_is_kde(""));
    }

    #[test]
    fn the_entry_is_the_system_theme() {
        let t = placeholder();
        assert!(crate::theme::is_system(&t));
        assert!(t.chrome.is_empty() && t.picture.is_empty());
        assert_eq!(t.grid_style, "none");
        assert!(crate::theme::is_system(&current()));
    }

    #[test]
    fn parses_and_maps_a_dark_kde_scheme() {
        let t = kde(OXYGEN_DARK);
        assert!(t.dark);
        assert_eq!(t.panel, [29, 38, 44]);
        assert_eq!(t.background, [22, 33, 39]);
        assert_eq!(t.panel_raised, [48, 55, 60]);
        assert_eq!(t.text, [241, 236, 237]);
        assert_eq!(t.accent, [61, 174, 233]);
        // Hover equals the accent here, so the second light is Visited.
        assert_eq!(t.accent_alt, [90, 79, 155]);
        assert_eq!(t.wave_background, [21, 31, 37]);
        assert_eq!(t.selection, [51, 130, 171]);
        assert_eq!(t.meter_low, [0, 110, 63]);
        // The waveform follows KDE's accent.
        assert_eq!(t.wave_centre, [61, 174, 233]);
    }

    #[test]
    fn light_kde_scheme_and_missing_groups_fall_back_to_window() {
        let t = kde(BREEZE_LIGHT);
        assert!(!t.dark);
        // No AccentColor: the window's focus colour.
        assert_eq!(t.accent, [61, 174, 233]);
        assert_eq!(t.text_dim, [112, 125, 138]);
    }

    #[test]
    fn unusable_files_give_nothing() {
        assert!(Palette::parse("").is_none());
        assert!(Palette::parse("[Colors:Window]\nBackgroundNormal=1,2,3\n").is_none());
        assert!(
            Palette::parse("[Colors:Window]\nBackgroundNormal=a,b\nForegroundNormal=1,2,3")
                .is_none()
        );
        assert!(
            Palette::parse("[Colors:Window]\nBackgroundNormal=1,2,3\nForegroundNormal=4,5,6")
                .is_some()
        );
    }

    #[test]
    fn every_palette_is_readable() {
        assert!((contrast([0; 3], [255; 3]) - 21.0).abs() < 0.01);
        let yellow_accent = [249, 240, 107];
        for mut t in [
            kde(OXYGEN_DARK),
            kde(BREEZE_LIGHT),
            base(true, BLUE),
            base(false, BLUE),
            // A pale accent on white.
            base(false, yellow_accent),
        ] {
            guard(&mut t);
            let bg = t.wave_background;
            for c in [
                t.wave_peak,
                t.wave_centre,
                t.playhead,
                t.marker,
                t.meter_low,
                t.meter_mid,
                t.meter_high,
                t.selection,
            ] {
                assert!(contrast(c, bg) >= 3.0, "{c:?} on {bg:?}");
            }
            assert!(contrast(t.text, t.panel) >= 4.5);
            assert!(contrast(t.text_dim, t.panel) >= 3.0);
        }
        // OxygenDark's green meter is too dark on its own view until guarded.
        let t = kde(OXYGEN_DARK);
        assert!(contrast(t.meter_low, t.wave_background) < 3.0);
        // Already readable colours are left as they are.
        assert_eq!(ensure_contrast([255; 3], [0; 3], 4.5), [255; 3]);
    }

    #[test]
    fn selection_wash_is_visible() {
        let mut t = base(true, BLUE);
        t.wave_background = [21, 31, 37];
        t.selection = [51, 130, 171];
        t.selection_alpha = 10;
        guard(&mut t);
        let wash = mix(
            t.wave_background,
            t.selection,
            t.selection_alpha as f32 / 255.0,
        );
        assert!(contrast(wash, t.wave_background) >= 1.25);
    }

    /// Needs a desktop session: `cargo test -- --ignored portal`.
    #[test]
    #[ignore]
    fn portal_answers() {
        use ashpd::desktop::settings::Settings;
        pollster::block_on(async {
            let s = Settings::new().await.expect("portal");
            let _changes = s.receive_setting_changed().await.expect("subscribe");
            let a = read(&s).await;
            eprintln!("portal: {a:?}");
        });
        let a = appearance();
        assert!(READY.load(Ordering::Acquire));
        eprintln!("cached: {a:?}");
    }
}
