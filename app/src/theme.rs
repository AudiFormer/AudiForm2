// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Themes. Every colour, gradient, corner radius and glow the app draws
//! comes from a [`Theme`]. Themes are plain JSON (`.af27theme`) so anyone
//! can make one: put it in `<data>/Audiform27/themes/` and pick it from
//! View > Theme. View > Theme > Save Current Theme As writes the active
//! theme out as a starting point.

use egui::{Color32, Rounding, Stroke, Visuals};
use serde::{Deserialize, Serialize};

pub type Rgb = [u8; 3];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Theme {
    pub name: String,
    pub author: String,
    /// Base egui widgets on the dark (true) or light (false) defaults.
    pub dark: bool,
    pub background: Rgb,
    pub panel: Rgb,
    pub panel_raised: Rgb,
    pub border: Rgb,
    pub text: Rgb,
    pub text_dim: Rgb,
    pub accent: Rgb,
    pub accent_alt: Rgb,
    /// Waveform fill, top of the channel to the centre line.
    pub wave_peak: Rgb,
    pub wave_centre: Rgb,
    pub wave_background: Rgb,
    pub grid: Rgb,
    pub selection: Rgb,
    /// 0..255 opacity of the selection wash.
    pub selection_alpha: u8,
    pub playhead: Rgb,
    pub marker: Rgb,
    /// Spectrogram colour stops, quiet to loud.
    pub spectrum: Vec<Rgb>,
    pub meter_low: Rgb,
    pub meter_mid: Rgb,
    pub meter_high: Rgb,
    /// Colours handed to new tracks in turn.
    pub track_colors: Vec<Rgb>,
    pub rounding: f32,
    /// 0 = off; 1 = a soft halo around waveforms, playhead and play button.
    pub glow: f32,
    pub font_scale: f32,
    /// Phase Wheel dot colours, quiet low notes to high notes (None: the
    /// built-in red to magenta).
    pub phase_low: Option<Rgb>,
    pub phase_high: Option<Rgb>,
    /// Draw the Phase Wheel coarse and blown up, like a low-resolution screen.
    pub pixel: bool,
    /// The backdrop behind the glass panels, top and bottom of the window.
    pub backdrop_top: Rgb,
    pub backdrop_bottom: Rgb,
    /// "perspective", "hex", "dots", "lines" or "none".
    pub grid_style: String,
    /// The grid's light colour and strength (0 = off, 1 = normal).
    pub grid_glow: Rgb,
    pub grid_strength: f32,
    /// How opaque the panels' glass is (0 clear, 1 solid).
    pub glass: f32,
    /// How far the panels' top-right and bottom-left corners are cut (px).
    pub chamfer: f32,
    /// HUD corner brackets on every panel.
    pub brackets: bool,
    /// CRT scan lines over everything (0 = off).
    pub scanlines: f32,
    /// Animation speed (0 = nothing moves on its own).
    pub motion: f32,
    /// How strongly the chrome reacts to the music (0 = not at all).
    pub reactive: f32,
    /// Play the boot sequence when the app starts.
    pub intro: bool,
}

pub fn c(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

pub fn ca(rgb: Rgb, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], a)
}

pub fn lerp(a: Rgb, b: Rgb, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2]))
}

impl Default for Theme {
    fn default() -> Self {
        Theme::zero_point()
    }
}

impl Theme {
    /// The default: a holographic instrument panel. Glass over a moving
    /// perspective grid, ice-cyan light with hot magenta, laser playhead.
    pub fn zero_point() -> Theme {
        Theme {
            name: "Zero Point".into(),
            author: "Audiform27".into(),
            dark: true,
            background: [3, 4, 10],
            panel: [8, 11, 24],
            panel_raised: [17, 22, 44],
            border: [34, 50, 88],
            text: [214, 232, 255],
            text_dim: [112, 132, 178],
            accent: [0, 234, 255],
            accent_alt: [255, 46, 166],
            wave_peak: [126, 66, 255],
            wave_centre: [0, 240, 255],
            wave_background: [3, 5, 12],
            grid: [18, 28, 54],
            selection: [0, 234, 255],
            selection_alpha: 34,
            playhead: [255, 46, 166],
            marker: [255, 196, 0],
            spectrum: vec![
                [3, 5, 12],
                [14, 10, 60],
                [70, 20, 160],
                [200, 30, 190],
                [255, 90, 120],
                [255, 200, 110],
                [255, 255, 255],
            ],
            meter_low: [0, 234, 200],
            meter_mid: [255, 210, 60],
            meter_high: [255, 46, 110],
            track_colors: vec![
                [0, 234, 255],
                [255, 46, 166],
                [126, 66, 255],
                [60, 255, 170],
                [255, 170, 40],
                [70, 140, 255],
                [255, 80, 80],
                [210, 255, 70],
            ],
            rounding: 2.0,
            glow: 1.2,
            font_scale: 1.0,
            phase_low: None,
            phase_high: None,
            pixel: false,
            backdrop_top: [5, 8, 24],
            backdrop_bottom: [1, 1, 5],
            grid_style: "perspective".into(),
            grid_glow: [40, 120, 255],
            grid_strength: 1.0,
            glass: 0.82,
            chamfer: 9.0,
            brackets: true,
            scanlines: 0.05,
            motion: 1.0,
            reactive: 0.8,
            intro: true,
        }
    }

    /// Amber military HUD over a hex mesh.
    pub fn solar_flare() -> Theme {
        Theme {
            name: "Solar Flare".into(),
            background: [8, 4, 2],
            panel: [16, 9, 4],
            panel_raised: [32, 18, 8],
            border: [80, 44, 16],
            text: [255, 228, 196],
            text_dim: [170, 120, 80],
            accent: [255, 158, 24],
            accent_alt: [255, 60, 40],
            wave_peak: [255, 60, 40],
            wave_centre: [255, 190, 60],
            wave_background: [6, 3, 1],
            grid: [40, 22, 8],
            selection: [255, 158, 24],
            playhead: [255, 230, 120],
            marker: [255, 80, 60],
            spectrum: vec![
                [6, 3, 1],
                [50, 10, 4],
                [150, 40, 10],
                [255, 110, 20],
                [255, 200, 90],
                [255, 250, 230],
            ],
            meter_low: [255, 170, 40],
            backdrop_top: [18, 8, 2],
            backdrop_bottom: [3, 1, 0],
            grid_style: "hex".into(),
            grid_glow: [255, 120, 30],
            chamfer: 12.0,
            ..Theme::zero_point()
        }
    }
    /// The default: near-black blue, electric cyan into violet, magenta accents.
    pub fn neon_abyss() -> Theme {
        Theme {
            name: "Neon Abyss".into(),
            author: "Audiform27".into(),
            dark: true,
            background: [7, 8, 14],
            panel: [13, 15, 25],
            panel_raised: [21, 24, 39],
            border: [36, 40, 66],
            text: [220, 228, 255],
            text_dim: [120, 130, 170],
            accent: [56, 240, 255],
            accent_alt: [255, 56, 172],
            wave_peak: [140, 92, 255],
            wave_centre: [56, 240, 255],
            wave_background: [5, 6, 11],
            grid: [24, 28, 46],
            selection: [56, 240, 255],
            selection_alpha: 38,
            playhead: [255, 56, 172],
            marker: [255, 200, 60],
            spectrum: vec![
                [5, 6, 11],
                [30, 10, 70],
                [100, 20, 160],
                [220, 40, 160],
                [255, 120, 80],
                [255, 230, 120],
                [255, 255, 255],
            ],
            meter_low: [56, 240, 200],
            meter_mid: [255, 220, 60],
            meter_high: [255, 56, 100],
            track_colors: vec![
                [56, 240, 255],
                [255, 56, 172],
                [140, 92, 255],
                [80, 255, 160],
                [255, 170, 50],
                [90, 150, 255],
                [255, 90, 90],
                [220, 255, 80],
            ],
            rounding: 6.0,
            glow: 1.0,
            font_scale: 1.0,
            phase_low: None,
            phase_high: None,
            pixel: false,
            backdrop_top: [7, 8, 14],
            backdrop_bottom: [7, 8, 14],
            grid_style: "dots".into(),
            grid_glow: [56, 240, 255],
            grid_strength: 0.6,
            glass: 1.0,
            chamfer: 0.0,
            brackets: false,
            scanlines: 0.0,
            motion: 0.6,
            reactive: 0.5,
            intro: true,
        }
    }

    pub fn synth_sunset() -> Theme {
        Theme {
            name: "Synth Sunset".into(),
            background: [18, 8, 28],
            panel: [28, 12, 40],
            panel_raised: [42, 18, 58],
            border: [70, 30, 90],
            text: [255, 225, 240],
            text_dim: [180, 130, 170],
            accent: [255, 120, 60],
            accent_alt: [255, 40, 140],
            wave_peak: [255, 40, 140],
            wave_centre: [255, 190, 70],
            wave_background: [14, 6, 22],
            grid: [44, 20, 60],
            selection: [255, 160, 60],
            playhead: [80, 230, 255],
            spectrum: vec![
                [14, 6, 22],
                [60, 10, 80],
                [180, 20, 120],
                [255, 90, 60],
                [255, 200, 80],
                [255, 250, 220],
            ],
            backdrop_top: [40, 8, 50],
            backdrop_bottom: [10, 2, 18],
            grid_style: "perspective".into(),
            grid_glow: [255, 60, 170],
            glass: 0.8,
            chamfer: 10.0,
            ..Theme::zero_point()
        }
    }

    pub fn phosphor() -> Theme {
        Theme {
            name: "Phosphor".into(),
            background: [2, 8, 4],
            panel: [5, 14, 8],
            panel_raised: [9, 22, 13],
            border: [20, 50, 28],
            text: [170, 255, 190],
            text_dim: [80, 150, 100],
            accent: [60, 255, 120],
            accent_alt: [200, 255, 80],
            wave_peak: [20, 120, 50],
            wave_centre: [90, 255, 140],
            wave_background: [1, 6, 3],
            grid: [10, 34, 18],
            selection: [60, 255, 120],
            playhead: [220, 255, 120],
            marker: [220, 255, 120],
            spectrum: vec![
                [1, 6, 3],
                [5, 40, 15],
                [20, 110, 45],
                [60, 200, 90],
                [160, 255, 170],
                [240, 255, 240],
            ],
            meter_low: [60, 255, 120],
            rounding: 2.0,
            pixel: true,
            backdrop_top: [2, 10, 5],
            backdrop_bottom: [0, 3, 1],
            grid_style: "lines".into(),
            grid_glow: [40, 255, 110],
            chamfer: 5.0,
            scanlines: 0.35,
            ..Theme::zero_point()
        }
    }

    pub fn daylight() -> Theme {
        Theme {
            name: "Daylight".into(),
            dark: false,
            background: [236, 238, 245],
            panel: [246, 247, 252],
            panel_raised: [255, 255, 255],
            border: [205, 210, 228],
            text: [25, 28, 45],
            text_dim: [110, 115, 140],
            accent: [30, 110, 255],
            accent_alt: [230, 40, 120],
            wave_peak: [120, 80, 230],
            wave_centre: [30, 110, 255],
            wave_background: [252, 252, 255],
            grid: [228, 231, 242],
            selection: [30, 110, 255],
            selection_alpha: 45,
            playhead: [230, 40, 120],
            marker: [220, 140, 0],
            spectrum: vec![
                [252, 252, 255],
                [190, 200, 255],
                [110, 120, 240],
                [150, 40, 200],
                [230, 40, 100],
                [40, 0, 40],
            ],
            glow: 0.0,
            backdrop_top: [226, 230, 242],
            backdrop_bottom: [206, 212, 230],
            grid_style: "dots".into(),
            grid_glow: [30, 110, 255],
            grid_strength: 0.0,
            glass: 0.94,
            scanlines: 0.0,
            motion: 0.6,
            ..Theme::zero_point()
        }
    }

    pub fn builtins() -> Vec<Theme> {
        vec![
            Theme::zero_point(),
            Theme::solar_flare(),
            Theme::neon_abyss(),
            Theme::synth_sunset(),
            Theme::phosphor(),
            Theme::daylight(),
        ]
    }

    /// Spectrogram colour for a level in 0..1.
    pub fn spectrum_color(&self, t: f32) -> [u8; 3] {
        let stops = if self.spectrum.len() >= 2 {
            &self.spectrum[..]
        } else {
            &[[0, 0, 0], [255, 255, 255]][..]
        };
        let t = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
        let i = (t.floor() as usize).min(stops.len() - 2);
        let f = t - i as f32;
        let a = stops[i];
        let b = stops[i + 1];
        let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * f) as u8;
        [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
    }

    pub fn meter_color(&self, db: f32) -> Color32 {
        if db > -3.0 {
            c(self.meter_high)
        } else if db > -12.0 {
            lerp(self.meter_low, self.meter_mid, (db + 12.0) / 9.0)
        } else {
            c(self.meter_low)
        }
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = if self.dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        let r = Rounding::same(self.rounding);
        v.override_text_color = Some(c(self.text));
        v.panel_fill = c(self.panel);
        v.window_fill = c(self.panel_raised);
        v.extreme_bg_color = c(self.wave_background);
        v.faint_bg_color = c(self.panel_raised);
        v.code_bg_color = c(self.panel_raised);
        v.hyperlink_color = c(self.accent);
        v.selection.bg_fill = ca(self.accent, 90);
        v.selection.stroke = Stroke::new(1.0_f32, c(self.accent));
        v.window_rounding = r;
        v.menu_rounding = r;
        v.window_stroke = Stroke::new(1.0_f32, c(self.border));
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = c(self.panel);
        w.noninteractive.weak_bg_fill = c(self.panel);
        w.noninteractive.bg_stroke = Stroke::new(1.0_f32, c(self.border));
        w.noninteractive.fg_stroke = Stroke::new(1.0_f32, c(self.text_dim));
        w.inactive.bg_fill = c(self.panel_raised);
        w.inactive.weak_bg_fill = c(self.panel_raised);
        w.inactive.bg_stroke = Stroke::new(1.0_f32, c(self.border));
        w.inactive.fg_stroke = Stroke::new(1.0_f32, c(self.text));
        w.hovered.bg_fill = lerp(self.panel_raised, self.accent, 0.18);
        w.hovered.weak_bg_fill = lerp(self.panel_raised, self.accent, 0.18);
        w.hovered.bg_stroke = Stroke::new(1.0_f32, c(self.accent));
        w.hovered.fg_stroke = Stroke::new(1.5_f32, c(self.text));
        w.active.bg_fill = lerp(self.panel_raised, self.accent, 0.35);
        w.active.weak_bg_fill = lerp(self.panel_raised, self.accent, 0.35);
        w.active.bg_stroke = Stroke::new(1.0_f32, c(self.accent));
        w.active.fg_stroke = Stroke::new(2.0_f32, c(self.text));
        w.open.bg_fill = lerp(self.panel_raised, self.accent, 0.25);
        w.open.weak_bg_fill = lerp(self.panel_raised, self.accent, 0.25);
        for s in [
            &mut w.noninteractive,
            &mut w.inactive,
            &mut w.hovered,
            &mut w.active,
            &mut w.open,
        ] {
            s.rounding = Rounding::same((self.rounding * 0.7).max(0.0));
        }
        ctx.set_visuals(v);
        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(7.0, 3.0);
        for f in style.text_styles.values_mut() {
            f.size = (f.size * self.font_scale).clamp(6.0, 40.0);
        }
        ctx.set_style(style);
    }

    /// Paints a soft halo behind a shape: concentric translucent strokes.
    pub fn glow_rect(&self, p: &egui::Painter, rect: egui::Rect, color: Rgb, strength: f32) {
        let g = self.glow * strength;
        if g <= 0.0 {
            return;
        }
        for i in 1..=4 {
            let a = (28.0 * g / i as f32) as u8;
            p.rect_stroke(
                rect.expand(i as f32 * 1.5),
                Rounding::same(self.rounding + i as f32),
                Stroke::new(1.5_f32, ca(color, a)),
            );
        }
    }
}

/// Built-ins first, then any `.af27theme` files in the user's folder.
pub fn load_all(dir: &std::path::Path) -> Vec<Theme> {
    let mut list = Theme::builtins();
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut paths: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "af27theme"))
            .collect();
        paths.sort();
        for p in paths {
            if let Some(t) = std::fs::read_to_string(&p)
                .ok()
                .and_then(|s| serde_json::from_str::<Theme>(&s).ok())
            {
                list.retain(|x| x.name != t.name);
                list.push(t);
            }
        }
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_theme_files_fill_in_defaults() {
        let t: Theme = serde_json::from_str(r#"{"name":"Mine","accent":[1,2,3]}"#).unwrap();
        assert_eq!(t.name, "Mine");
        assert_eq!(t.accent, [1, 2, 3]);
        assert_eq!(t.panel, Theme::zero_point().panel);
        // Theme files from before the HUD fields still load.
        assert_eq!(t.grid_style, "perspective");
    }

    #[test]
    fn spectrum_ramp_hits_its_ends() {
        let t = Theme::neon_abyss();
        assert_eq!(t.spectrum_color(0.0), t.spectrum[0]);
        assert_eq!(t.spectrum_color(1.0), *t.spectrum.last().unwrap());
    }
}
