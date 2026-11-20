// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.
// Ported from Audiform's Phase Wheel panel (PhaseWheelPanel.svelte and
// colors.ts), designed and calibrated by its author (nwh212) and
// relicensed here by them. Drawing, sizes, opacities and colours match
// the original canvas.

//! Phase Wheel: how two signals of what is playing line up, frequency by
//! frequency. Each dot is one band of frequencies: its distance from the
//! center is the frequency (20 Hz in the middle, 20 kHz at the ring, the
//! same scale as its color, red to magenta by default; right-click to pick
//! the two ends), its angle the phase of "Compare to" against "Signal"
//! (straight up when they move together, clockwise when "Compare to" is
//! ahead, straight down when they are opposite and would cancel in mono),
//! its size how loud the band is and its brightness both the level and
//! how steady its phase is. A plain delay or phase shift draws one clean
//! curve. The bar under the wheel is the overall correlation: +1 the
//! same, 0 unrelated, -1 opposite. While stopped it shows the audio just
//! before the playhead. Its choices are kept between runs.

use egui::{pos2, vec2, Align2, Color32, FontId, Rect, RichText, Rounding, Sense, Stroke};
use serde::{Deserialize, Serialize};

use crate::state::{data_dir, State};
use crate::theme::{c, Rgb};
use af27_dsp::phase::{analyse, Bin, Signal, RATES, SIZES};

pub const PHASE_LOW: Rgb = [0xff, 0x2a, 0x2a];
pub const PHASE_HIGH: Rgb = [0xe0, 0x2a, 0xff];
/// Screen pixels per drawn pixel in the pixel look.
const PIXEL: f32 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sig {
    Left,
    Right,
    Mid,
    Side,
}

impl Sig {
    const ALL: [Sig; 4] = [Sig::Left, Sig::Right, Sig::Mid, Sig::Side];
    fn signal(self) -> Signal {
        match self {
            Sig::Left => Signal::Left,
            Sig::Right => Signal::Right,
            Sig::Mid => Signal::Mid,
            Sig::Side => Signal::Side,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhasePrefs {
    pub size: usize,
    pub fps: u32,
    pub a: Sig,
    pub b: Sig,
    pub details: bool,
    /// Chosen colours; None follows the theme (or the built-in red to magenta).
    pub low: Option<Rgb>,
    pub high: Option<Rgb>,
}

impl Default for PhasePrefs {
    fn default() -> Self {
        PhasePrefs {
            size: 4096,
            fps: 60,
            a: Sig::Left,
            b: Sig::Right,
            details: true,
            low: None,
            high: None,
        }
    }
}

impl PhasePrefs {
    fn path() -> std::path::PathBuf {
        data_dir().join("phase_wheel.json")
    }
    pub fn load() -> PhasePrefs {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save_now(&self) {
        self.save();
    }

    fn save(&self) {
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(
            Self::path(),
            serde_json::to_string_pretty(self).unwrap_or_default(),
        );
    }
}

fn rgb_to_hsl([r, g, b]: Rgb) -> [f32; 3] {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max == min {
        return [0.0, 0.0, l];
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [h * 60.0, s, l]
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let h = h.rem_euclid(360.0) / 360.0;
    if s == 0.0 {
        return [l * 255.0; 3];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        v * 255.0
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0)]
}

/// From `low` to `high` up the colour wheel (hue rising), as the original.
fn ramp(low: Rgb, high: Rgb, t: f32) -> [f32; 3] {
    let [h0, s0, l0] = rgb_to_hsl(low);
    let [h1raw, s1, l1] = rgb_to_hsl(high);
    let h1 = if h1raw < h0 { h1raw + 360.0 } else { h1raw };
    let u = t.clamp(0.0, 1.0);
    hsl_to_rgb(h0 + (h1 - h0) * u, s0 + (s1 - s0) * u, l0 + (l1 - l0) * u)
}

fn palette(low: Rgb, high: Rgb) -> Vec<[f32; 3]> {
    (0..64)
        .map(|i| {
            let [r, g, b] = ramp(low, high, i as f32 / 63.0);
            [r.round(), g.round(), b.round()]
        })
        .collect()
}

/// A small RGB canvas with alpha blending, like the original 2D canvas.
struct Canvas {
    w: usize,
    px: Vec<[f32; 3]>,
}

impl Canvas {
    fn blend(&mut self, x: i32, y: i32, col: [f32; 3], a: f32) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.w || a <= 0.0 {
            return;
        }
        let p = &mut self.px[y as usize * self.w + x as usize];
        for k in 0..3 {
            p[k] = p[k] * (1.0 - a) + col[k] * a;
        }
    }
    fn fill(&mut self, col: [f32; 3], a: f32) {
        for p in &mut self.px {
            for k in 0..3 {
                p[k] = p[k] * (1.0 - a) + col[k] * a;
            }
        }
    }
    fn disc(&mut self, cx: f32, cy: f32, rad: f32, col: [f32; 3], a: f32) {
        let (x0, x1) = (
            (cx - rad - 1.0).floor() as i32,
            (cx + rad + 1.0).ceil() as i32,
        );
        let (y0, y1) = (
            (cy - rad - 1.0).floor() as i32,
            (cy + rad + 1.0).ceil() as i32,
        );
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d =
                    ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt() - rad;
                self.blend(x, y, col, (0.5 - d).clamp(0.0, 1.0) * a);
            }
        }
    }
    fn square(&mut self, cx: f32, cy: f32, d: f32, col: [f32; 3], a: f32) {
        let (x0, y0) = ((cx - d / 2.0).round() as i32, (cy - d / 2.0).round() as i32);
        for y in y0..y0 + d as i32 {
            for x in x0..x0 + d as i32 {
                self.blend(x, y, col, a);
            }
        }
    }
    fn ring(&mut self, cx: f32, cy: f32, rad: f32, width: f32, col: [f32; 3], a: f32) {
        let reach = rad + width + 1.0;
        let (x0, x1) = ((cx - reach).floor() as i32, (cx + reach).ceil() as i32);
        let (y0, y1) = ((cy - reach).floor() as i32, (cy + reach).ceil() as i32);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let cov = (width / 2.0 + 0.5 - (d - rad).abs()).clamp(0.0, 1.0);
                self.blend(x, y, col, cov * a);
            }
        }
    }
    fn hline(&mut self, x0: f32, x1: f32, y: f32, width: f32, col: [f32; 3], a: f32) {
        let ys = (y - width / 2.0).floor() as i32;
        for yy in ys..ys + width.ceil().max(1.0) as i32 {
            for x in x0 as i32..x1 as i32 {
                self.blend(x, yy, col, a);
            }
        }
    }
    fn vline(&mut self, x: f32, y0: f32, y1: f32, width: f32, col: [f32; 3], a: f32) {
        let xs = (x - width / 2.0).floor() as i32;
        for xx in xs..xs + width.ceil().max(1.0) as i32 {
            for y in y0 as i32..y1 as i32 {
                self.blend(xx, y, col, a);
            }
        }
    }
    fn image(&self) -> egui::ColorImage {
        let mut img = egui::ColorImage::new([self.w, self.w], Color32::BLACK);
        for (o, p) in img.pixels.iter_mut().zip(&self.px) {
            *o = Color32::from_rgb(p[0] as u8, p[1] as u8, p[2] as u8);
        }
        img
    }
}

#[derive(Default)]
pub struct WheelCache {
    canvas: Option<Canvas>,
    tex: Option<egui::TextureHandle>,
    last: Option<std::time::Instant>,
    correlation: f32,
    still_key: Option<(usize, i64, u64, Sig, Sig, bool)>,
    colors_open: bool,
}

#[allow(clippy::too_many_arguments)]
fn draw(
    cv: &mut Canvas,
    side: f32,
    scale: f32,
    bins: &[Bin],
    fade: f32,
    pal: &[[f32; 3]],
    ring: Rgb,
    pixel: bool,
) {
    let mid = side / 2.0;
    let r_big = mid - 18.0;
    let s = |v: f32| v * scale;
    cv.fill([10.0, 10.0, 12.0], fade);
    let white = [255.0, 255.0, 255.0];
    let (ga, gw) = if pixel { (0.16, PIXEL) } else { (0.10, 1.0) };
    for f in [0.25, 0.5, 0.75] {
        cv.ring(s(mid), s(mid), s(r_big * f), s(gw), white, ga);
    }
    cv.hline(s(mid - r_big), s(mid + r_big), s(mid), s(gw), white, ga);
    cv.vline(s(mid), s(mid - r_big), s(mid + r_big), s(gw), white, ga);
    let rc = [ring[0] as f32, ring[1] as f32, ring[2] as f32];
    cv.ring(
        s(mid),
        s(mid),
        s(r_big),
        s(if pixel { PIXEL } else { 1.5 }),
        rc,
        1.0,
    );
    for b in bins {
        let r = (0.04 + 0.96 * b.pitch) * r_big;
        let x = mid + r * b.phase.sin();
        let y = mid - r * b.phase.cos();
        let col = pal[((b.pitch * 63.0).round() as usize).min(63)];
        let a = ((0.15 + 1.1 * b.level) * (0.5 + 0.5 * b.coherence)).min(0.85);
        if pixel {
            let d = ((1.2 + 2.4 * b.level) / PIXEL).round().max(1.0) * PIXEL;
            let gx = (x / PIXEL).round() * PIXEL;
            let gy = (y / PIXEL).round() * PIXEL;
            cv.square(s(gx), s(gy), s(d).max(1.0), col, a);
        } else {
            cv.disc(s(x), s(y), s(1.2 + 2.4 * b.level), col, a);
        }
    }
}

pub fn panel(ui: &mut egui::Ui, st: &mut State, cache: &mut WheelCache) {
    let th = st.theme.clone();
    let avail = ui.available_rect_before_wrap();
    let roomy = avail.height() >= 300.0 && avail.width() >= 220.0;
    let details = st.phase.details && roomy;
    let reserved = if details { 118.0 } else { 0.0 };
    let side = (avail.width().min(avail.height() - reserved))
        .max(60.0)
        .floor();
    let (wheel_rect, resp) = ui.allocate_exact_size(vec2(avail.width(), side), Sense::click());
    let sq = Rect::from_center_size(wheel_rect.center(), vec2(side, side));
    let pixel = th.pixel;
    let ppp = ui.ctx().pixels_per_point();
    let scale = if pixel { 1.0 / PIXEL } else { ppp };
    let w = ((side * scale).round() as usize).max(8);
    let low = st.phase.low.or(th.phase_low).unwrap_or(PHASE_LOW);
    let high = st.phase.high.or(th.phase_high).unwrap_or(PHASE_HIGH);
    let pal = palette(low, high);
    if cache.canvas.as_ref().map(|c| c.w) != Some(w) {
        cache.canvas = Some(Canvas {
            w,
            px: vec![[10.0, 10.0, 12.0]; w * w],
        });
        cache.still_key = None;
    }
    let playing = st.is_playing();
    let size = st.phase.size;
    let due = cache
        .last
        .is_none_or(|t| t.elapsed().as_secs_f32() * 1000.0 >= 1000.0 / st.phase.fps as f32 - 2.0);
    let still_key = (
        size,
        (st.cursor() * 1000.0) as i64,
        st.session_version + st.docs.iter().map(|d| d.version).sum::<u64>(),
        st.phase.a,
        st.phase.b,
        pixel,
    );
    let redraw = if playing {
        due
    } else {
        cache.still_key != Some(still_key)
    };
    if redraw {
        let (frames, rate) = st.scope_frames(size);
        let ph = analyse(&frames, st.phase.a.signal(), st.phase.b.signal(), rate);
        cache.correlation = ph.correlation;
        let cv = cache.canvas.as_mut().unwrap();
        draw(
            cv,
            side,
            scale,
            &ph.bins,
            if playing { 0.35 } else { 1.0 },
            &pal,
            th.accent,
            pixel,
        );
        let img = cv.image();
        let opts = if pixel {
            egui::TextureOptions::NEAREST
        } else {
            egui::TextureOptions::LINEAR
        };
        match &mut cache.tex {
            Some(t) => t.set(img, opts),
            None => cache.tex = Some(ui.ctx().load_texture("phase-wheel", img, opts)),
        }
        cache.last = Some(std::time::Instant::now());
        cache.still_key = if playing { None } else { Some(still_key) };
    }
    let p = ui.painter_at(wheel_rect);
    if let Some(t) = &cache.tex {
        p.image(
            t.id(),
            sq,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    let mid = sq.center();
    let r_big = side / 2.0 - 18.0;
    let lab = Color32::from_rgba_unmultiplied(255, 255, 255, 140);
    let font = FontId::proportional(if pixel { 11.0 } else { 10.0 });
    p.text(
        pos2(mid.x, sq.top() + 11.0),
        Align2::CENTER_BOTTOM,
        "in phase",
        font.clone(),
        lab,
    );
    p.text(
        pos2(mid.x, sq.bottom() - 4.0),
        Align2::CENTER_BOTTOM,
        "opposite",
        font.clone(),
        lab,
    );
    p.text(
        pos2(mid.x + r_big - 4.0, mid.y - 4.0),
        Align2::RIGHT_BOTTOM,
        "+90°",
        font.clone(),
        lab,
    );
    p.text(
        pos2(mid.x - r_big + 4.0, mid.y - 4.0),
        Align2::LEFT_BOTTOM,
        "-90°",
        font,
        lab,
    );
    if !playing {
        p.text(
            wheel_rect.left_top() + vec2(4.0, 2.0),
            Align2::LEFT_TOP,
            "Stopped: the audio at the playhead.",
            FontId::proportional(10.0),
            c(th.text_dim),
        );
    }
    let resp = resp
        .on_hover_text("Right-click for the colors and to show or hide the readouts and settings");
    resp.context_menu(|ui| {
        let label = if roomy {
            "Show Readouts and Settings"
        } else {
            "Show Readouts and Settings (hidden while the panel is small)"
        };
        if ui.checkbox(&mut st.phase.details, label).changed() {
            st.phase.save();
        }
        if ui.button("Colors...").clicked() {
            cache.colors_open = true;
            ui.close_menu();
        }
    });
    if cache.colors_open {
        let mut open = true;
        crate::ui::popup(
            ui.ctx(),
            egui::Id::new(("af2-popup", "phasewheel.rs", 0)),
            "Phase Wheel colors",
        )
        .open(&mut open)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            let mut lo = st.phase.low.unwrap_or(low);
            let mut hi = st.phase.high.unwrap_or(high);
            ui.horizontal(|ui| {
                ui.label("Low notes (20 Hz)");
                if ui.color_edit_button_srgb(&mut lo).changed() {
                    st.phase.low = Some(lo);
                    st.phase.save();
                }
            });
            ui.horizontal(|ui| {
                ui.label("High notes (20 kHz)");
                if ui.color_edit_button_srgb(&mut hi).changed() {
                    st.phase.high = Some(hi);
                    st.phase.save();
                }
            });
            if ui.button("Reset to the theme's colors").clicked() {
                st.phase.low = None;
                st.phase.high = None;
                st.phase.save();
            }
        });
        cache.colors_open = open;
    }
    if !details {
        return;
    }
    ui.add_space(6.0);
    // Legend: dot colours by frequency.
    ui.horizontal(|ui| {
        ui.label(RichText::new("20 Hz").color(c(th.text_dim)).size(10.0));
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width() - 44.0, 6.0), Sense::hover());
        let mut mesh = egui::Mesh::default();
        let stops: Vec<usize> = (0..64).filter(|i| i % 8 == 0 || *i == 63).collect();
        for (k, &i) in stops.iter().enumerate() {
            let x = r.left() + r.width() * k as f32 / (stops.len() - 1) as f32;
            let [cr, cg, cb] = pal[i];
            let col = Color32::from_rgb(cr as u8, cg as u8, cb as u8);
            mesh.colored_vertex(pos2(x, r.top()), col);
            mesh.colored_vertex(pos2(x, r.bottom()), col);
            if k > 0 {
                let b = (k * 2) as u32;
                mesh.add_triangle(b - 2, b - 1, b);
                mesh.add_triangle(b - 1, b, b + 1);
            }
        }
        ui.painter().add(egui::Shape::mesh(mesh));
        ui.label(RichText::new("20 kHz").color(c(th.text_dim)).size(10.0));
    })
    .response
    .on_hover_text("Dot colors by frequency. Right-click the wheel to change them.");
    // Correlation.
    let corr = cache.correlation;
    ui.horizontal(|ui| {
        ui.label(RichText::new("-1").color(c(th.text_dim)).size(10.0));
        let (r, _) =
            ui.allocate_exact_size(vec2(ui.available_width() - 70.0, 14.0), Sense::hover());
        let track = Rect::from_min_max(
            pos2(r.left(), r.center().y - 4.0),
            pos2(r.right(), r.center().y + 4.0),
        );
        let mut mesh = egui::Mesh::default();
        let cols = [
            Color32::from_rgb(0xb8, 0x44, 0x3c),
            Color32::from_rgb(0x3a, 0x3a, 0x40),
            Color32::from_rgb(0x3d, 0x8f, 0x6a),
        ];
        for (k, col) in cols.iter().enumerate() {
            let x = track.left() + track.width() * k as f32 / 2.0;
            mesh.colored_vertex(pos2(x, track.top()), *col);
            mesh.colored_vertex(pos2(x, track.bottom()), *col);
        }
        for k in 0..2u32 {
            let b = k * 2;
            mesh.add_triangle(b, b + 1, b + 2);
            mesh.add_triangle(b + 1, b + 2, b + 3);
        }
        let pt = ui.painter();
        pt.add(egui::Shape::mesh(mesh));
        pt.line_segment(
            [
                pos2(track.center().x, track.top() - 2.0),
                pos2(track.center().x, track.bottom() + 2.0),
            ],
            Stroke::new(1.0_f32, c(th.text_dim)),
        );
        let mx = track.left() + (corr + 1.0) / 2.0 * track.width();
        let neg = corr < 0.0;
        let red = Color32::from_rgb(0xe0, 0x57, 0x4d);
        pt.rect_filled(
            Rect::from_min_size(pos2(mx - 2.0, track.top() - 3.0), vec2(4.0, 14.0)),
            Rounding::same(2.0),
            if neg { red } else { c(th.text) },
        );
        ui.label(RichText::new("+1").color(c(th.text_dim)).size(10.0));
        let text = format!("{}{:.2}", if corr >= 0.0 { "+" } else { "" }, corr);
        let mut rt = RichText::new(text).monospace();
        if neg {
            rt = rt.background_color(red).color(Color32::WHITE);
        }
        ui.label(rt);
    })
    .response
    .on_hover_text("Correlation: +1 the same in both, 0 unrelated, -1 opposite (cancels in mono)");
    // Settings.
    let before = st.phase.clone();
    ui.horizontal_wrapped(|ui| {
        let pick = |ui: &mut egui::Ui, id: &str, v: &mut Sig| {
            egui::ComboBox::from_id_salt(id).selected_text(v.signal().label()).width(86.0).show_ui(ui, |ui| {
                for s in Sig::ALL {
                    ui.selectable_value(v, s, s.signal().label());
                }
            });
        };
        ui.label(RichText::new("Signal").color(c(th.text_dim)));
        pick(ui, "pw-a", &mut st.phase.a);
        ui.label(RichText::new("Compare to").color(c(th.text_dim)));
        pick(ui, "pw-b", &mut st.phase.b);
        ui.label(RichText::new("Samples").color(c(th.text_dim)))
            .on_hover_text("Frames analysed each time. Fewer is lighter on slower computers and reacts faster; more shows low frequencies in finer detail.");
        egui::ComboBox::from_id_salt("pw-size").selected_text(st.phase.size.to_string()).width(64.0).show_ui(ui, |ui| {
            for n in SIZES {
                ui.selectable_value(&mut st.phase.size, n, n.to_string());
            }
        });
        ui.label(RichText::new("Refresh").color(c(th.text_dim))).on_hover_text("Pictures a second, at most. Lower is lighter on slower computers.");
        egui::ComboBox::from_id_salt("pw-fps").selected_text(format!("{} fps", st.phase.fps)).width(64.0).show_ui(ui, |ui| {
            for r in RATES {
                ui.selectable_value(&mut st.phase.fps, r, format!("{r} fps"));
            }
        });
    });
    if st.phase != before {
        st.phase.save();
        cache.still_key = None;
    }
    ui.label(
        RichText::new("The app plays in stereo, so the choices are left, right, mid and side.")
            .size(10.0)
            .color(c(th.text_dim)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_ramp_climbs_the_hue_wheel() {
        let p = palette(PHASE_LOW, PHASE_HIGH);
        assert_eq!(p[0], [255.0, 42.0, 42.0]);
        assert_eq!(p[63], [224.0, 42.0, 255.0]);
        // A third of the way: past yellow, into green.
        let [r, g, b] = p[21];
        assert!(g > r && g > b, "{r} {g} {b}");
    }
}
