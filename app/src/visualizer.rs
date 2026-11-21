// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.
// Ported from Audiform's Visualizer (the project lead's own work).

//! Visualizer: moving pictures that follow what is playing. Pick a look
//! (Lava Lamp, Bars, Waves, Plasma, Ripples, Starfield or Radial), a set
//! of colors, how strongly the picture reacts, and how many pictures a
//! second it draws (15 and 30 are lighter on slower computers). "Change
//! every 30 seconds" steps through the looks on its own. While stopped it
//! keeps moving gently and shows the audio just before the playhead. It
//! only draws while its tab is in front. Its choices are kept between runs.
//!
//! Everything is drawn on the CPU into a small RGB canvas (the original's
//! 2D canvas: device pixels, at most 1.5 per point) and shown as a texture.
//! Every constant and formula matches the original.

use std::time::{Duration, Instant};

use egui::{pos2, Color32, Rect, RichText, Sense};
use serde::{Deserialize, Serialize};

use crate::state::{data_dir, State};
use crate::theme::{c, Rgb};

// ---------------------------------------------------------------- analysis

pub const BANDS: usize = 64;

/// dBFS to 0..1: -90 dB is nothing, 0 dB is full; louder than quiet reads
/// as more than a straight line.
pub fn level_of(db: f32, sensitivity: f32) -> f32 {
    if !db.is_finite() {
        return 0.0;
    }
    let x = ((db + 90.0) / 90.0).clamp(0.0, 1.0);
    (x.powf(1.6) * sensitivity * 1.25).min(1.0)
}

fn mean(a: &[f32], from: usize, to: usize) -> f32 {
    if to > from {
        a[from..to].iter().sum::<f32>() / (to - from) as f32
    } else {
        0.0
    }
}

/// What the pictures need from one spectrum.
#[derive(Debug, Clone, Copy)]
pub struct Analysis {
    /// 0 to 1 per band, low to high.
    pub levels: [f32; BANDS],
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub level: f32,
    /// True for the one picture where the bass jumped.
    pub beat: bool,
    /// Seconds since the pictures began.
    pub time: f64,
    /// Seconds since the last picture.
    pub dt: f32,
}

impl Analysis {
    /// `time * k`, wrapped to 0..1 (palette positions wrap anyway) so long
    /// runs keep their precision in f32.
    fn tk(&self, k: f64) -> f32 {
        (self.time * k).rem_euclid(1.0) as f32
    }
}

/// Turns spectra (dBFS, 20 Hz to 20 kHz, even in pitch) into levels from
/// 0 to 1, bass/mid/treble, the overall level and a beat when the bass
/// jumps above where it has been.
#[derive(Debug, Clone)]
pub struct Analyser {
    bass_average: f32,
    since_beat: f32,
    time: f64,
    smooth: [f32; BANDS],
    /// The loudest band lately: pictures are scaled against it, so quiet
    /// music still moves them.
    ceiling: f32,
}

impl Default for Analyser {
    fn default() -> Self {
        Analyser {
            bass_average: 0.0,
            since_beat: 1.0,
            time: 0.0,
            smooth: [0.0; BANDS],
            ceiling: 0.35,
        }
    }
}

impl Analyser {
    /// `db` may be empty (nothing playing): everything reads as silence.
    /// `calm` (0 to 1) scales it down, for when stopped.
    pub fn feed(&mut self, db: &[f32], sensitivity: f32, dt: f32, calm: f32) -> Analysis {
        self.time += dt as f64;
        self.since_beat += dt;
        let at = |i: usize| db.get(i).copied().unwrap_or(-120.0);
        // The ceiling jumps up to a loud band and drifts down slowly, never below a floor.
        let mut top = 0.0f32;
        for i in 0..BANDS {
            top = top.max(level_of(at(i), 1.0));
        }
        self.ceiling = if top > self.ceiling {
            top
        } else {
            (self.ceiling - dt * 0.12).max(0.3)
        };
        let gain = (0.9 / self.ceiling) * sensitivity;
        let mut levels = [0.0f32; BANDS];
        for (i, out) in levels.iter_mut().enumerate() {
            let raw = (level_of(at(i), 1.0) * gain).min(1.0) * calm;
            // Rise fast, fall slowly, so pictures breathe instead of flicker.
            let prev = self.smooth[i];
            let next = if raw > prev {
                prev + (raw - prev) * 0.85
            } else {
                prev + (raw - prev) * (dt * 7.0).min(1.0)
            };
            self.smooth[i] = next;
            *out = next;
        }
        let bass = mean(&levels, 0, 10);
        let mid = mean(&levels, 10, 36);
        let treble = mean(&levels, 36, BANDS);
        let level = mean(&levels, 0, BANDS);
        self.bass_average += (bass - self.bass_average) * (dt * 1.5).min(1.0);
        let mut beat = false;
        if calm >= 0.5
            && bass > 0.15
            && bass > self.bass_average * 1.18 + 0.03
            && self.since_beat > 0.17
        {
            beat = true;
            self.since_beat = 0.0;
        }
        Analysis {
            levels,
            bass,
            mid,
            treble,
            level,
            beat,
            time: self.time,
            dt,
        }
    }
}

// ---------------------------------------------------------------- spectrum

/// The Visualizer's spectrum: `BANDS` levels in dBFS, 20 Hz to 20 kHz
/// evenly in pitch, from a Hann-windowed FFT of the newest `SIZE` frames
/// (mid of left and right). A band covering FFT bins reads its loudest
/// bin; a low band narrower than a bin reads the bins either side of its
/// centre, interpolated. Buffers are kept between calls.
#[derive(Default)]
pub struct Spectrum {
    win: Vec<f64>,
    norm: f64,
    re: Vec<f64>,
    im: Vec<f64>,
}

impl Spectrum {
    pub const SIZE: usize = 2048;

    pub fn size(&self) -> usize {
        Self::SIZE
    }

    pub fn bands(&mut self, frames: &[[f32; 2]], rate: u32, out: &mut [f32]) {
        let n = Self::SIZE;
        if self.win.len() != n {
            self.win = af27_dsp::fft::hann(n);
            self.norm = 2.0 / self.win.iter().sum::<f64>();
            self.re = vec![0.0; n];
            self.im = vec![0.0; n];
        }
        if rate == 0 {
            out.fill(-120.0);
            return;
        }
        let take = frames.len().min(n);
        let src = &frames[frames.len() - take..];
        let off = n - take;
        for k in 0..n {
            let s = if k < off {
                0.0
            } else {
                let f = src[k - off];
                (f[0] as f64 + f[1] as f64) * 0.5
            };
            self.re[k] = s * self.win[k];
            self.im[k] = 0.0;
        }
        af27_dsp::fft::fft(&mut self.re, &mut self.im, false);
        let half = n / 2;
        for b in 0..half {
            self.re[b] = self.re[b].hypot(self.im[b]) * self.norm;
        }
        let mag = &self.re[..half];
        let bw = rate as f64 / n as f64;
        let count = out.len().max(1) as f64;
        for (i, o) in out.iter_mut().enumerate() {
            let f0 = 20.0 * 1000f64.powf(i as f64 / count);
            let f1 = 20.0 * 1000f64.powf((i + 1) as f64 / count);
            let lo = (f0 / bw).ceil() as usize;
            let hi = ((f1 / bw).ceil() as usize).min(half);
            let m = if lo >= half {
                0.0
            } else if lo < hi {
                mag[lo..hi].iter().copied().fold(0.0, f64::max)
            } else {
                let x = ((f0 * f1).sqrt() / bw).min((half - 1) as f64);
                let i0 = (x.floor() as usize).min(half - 2);
                let fr = x - i0 as f64;
                mag[i0] * (1.0 - fr) + mag[i0 + 1] * fr
            };
            *o = if m > 0.0 {
                ((20.0 * m.log10()) as f32).max(-120.0)
            } else {
                -120.0
            };
        }
    }
}

// ----------------------------------------------------------------- palette

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scheme {
    #[default]
    Theme,
    Rainbow,
    Fire,
    Ocean,
    Aurora,
}

pub const SCHEMES: [(Scheme, &str); 5] = [
    (Scheme::Theme, "Theme colors"),
    (Scheme::Rainbow, "Rainbow"),
    (Scheme::Fire, "Fire"),
    (Scheme::Ocean, "Ocean"),
    (Scheme::Aurora, "Aurora"),
];

impl Scheme {
    fn label(self) -> &'static str {
        SCHEMES.iter().find(|s| s.0 == self).map_or("", |s| s.1)
    }
    fn id(self) -> &'static str {
        match self {
            Scheme::Theme => "theme",
            Scheme::Rainbow => "rainbow",
            Scheme::Fire => "fire",
            Scheme::Ocean => "ocean",
            Scheme::Aurora => "aurora",
        }
    }
}

const FIRE: [[f32; 3]; 5] = [
    [40.0, 0.0, 10.0],
    [190.0, 20.0, 20.0],
    [255.0, 110.0, 20.0],
    [255.0, 210.0, 70.0],
    [255.0, 245.0, 200.0],
];
const OCEAN: [[f32; 3]; 5] = [
    [4.0, 12.0, 60.0],
    [18.0, 70.0, 160.0],
    [40.0, 170.0, 200.0],
    [150.0, 230.0, 225.0],
    [235.0, 250.0, 245.0],
];
const AURORA: [[f32; 3]; 5] = [
    [10.0, 30.0, 70.0],
    [20.0, 150.0, 110.0],
    [120.0, 230.0, 120.0],
    [170.0, 110.0, 230.0],
    [20.0, 150.0, 110.0],
];
/// The built-in pair the theme scheme uses when the theme has none.
pub const DEFAULT_LOW: Rgb = [255, 80, 80];
pub const DEFAULT_HIGH: Rgb = [230, 80, 255];

fn wrap(t: f32) -> f32 {
    ((t % 1.0) + 1.0) % 1.0
}

fn stops(list: &[[f32; 3]], t: f32) -> [f32; 3] {
    let x = wrap(t) * (list.len() - 1) as f32;
    let i = (list.len() - 2).min(x.floor().max(0.0) as usize);
    let f = x - i as f32;
    let (a, b) = (list[i], list[i + 1]);
    [
        a[0] + (b[0] - a[0]) * f,
        a[1] + (b[1] - a[1]) * f,
        a[2] + (b[2] - a[2]) * f,
    ]
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

/// Hue 0 to 360, saturation and lightness 0 to 1.
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let hue = h.rem_euclid(360.0) / 360.0;
    if s == 0.0 {
        return [l * 255.0; 3];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let f = |t: f32| {
        let u = t.rem_euclid(1.0);
        if u < 1.0 / 6.0 {
            p + (q - p) * 6.0 * u
        } else if u < 0.5 {
            q
        } else if u < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - u) * 6.0
        } else {
            p
        }
    };
    [
        f(hue + 1.0 / 3.0) * 255.0,
        f(hue) * 255.0,
        f(hue - 1.0 / 3.0) * 255.0,
    ]
}

/// A colour for a position t from 0 to 1 (it wraps, so drifting t is fine).
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    scheme: Scheme,
    /// The theme scheme's ends in HSL, hue of `high` lifted above `low`.
    lo: [f32; 3],
    hi: [f32; 3],
}

impl Palette {
    /// `low` and `high` are the theme's own two analysis colours (None for
    /// the built-in pair).
    pub fn new(scheme: Scheme, low: Option<Rgb>, high: Option<Rgb>) -> Palette {
        let lo = rgb_to_hsl(low.unwrap_or(DEFAULT_LOW));
        let mut hi = rgb_to_hsl(high.unwrap_or(DEFAULT_HIGH));
        if hi[0] < lo[0] {
            hi[0] += 360.0;
        }
        Palette { scheme, lo, hi }
    }

    pub fn at(&self, t: f32) -> [f32; 3] {
        match self.scheme {
            Scheme::Rainbow => hsl_to_rgb(wrap(t) * 360.0, 0.85, 0.55),
            Scheme::Fire => stops(&FIRE, t),
            Scheme::Ocean => stops(&OCEAN, t),
            Scheme::Aurora => stops(&AURORA, t),
            Scheme::Theme => {
                let u = (0.5 - 0.5 * (wrap(t) * std::f32::consts::TAU).cos()).clamp(0.0, 1.0);
                let (a, b) = (self.lo, self.hi);
                hsl_to_rgb(
                    a[0] + (b[0] - a[0]) * u,
                    a[1] + (b[1] - a[1]) * u,
                    a[2] + (b[2] - a[2]) * u,
                )
            }
        }
    }

    /// As a CSS colour string would carry it: whole numbers.
    fn css(&self, t: f32) -> [f32; 3] {
        let [r, g, b] = self.at(t);
        [r.round(), g.round(), b.round()]
    }
}

// ------------------------------------------------------------------ canvas

const BACKDROP: [f32; 3] = [4.0, 7.0, 18.0];
/// The drawing area's own background (behind the canvas in the original).
const AREA: Color32 = Color32::from_rgb(0x04, 0x07, 0x1a);

fn erf(x: f32) -> f32 {
    // Abramowitz and Stegun 7.1.26.
    let s = x.signum();
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_4 * t - 1.453_152_1) * t) + 1.421_413_8) * t - 0.284_496_74) * t
            + 0.254_829_6)
            * t
            * (-x * x).exp();
    s * y
}

/// How much of pixel `i` the span `a..b` covers along one axis.
fn overlap(i: usize, a: f32, b: f32) -> f32 {
    let p = i as f32;
    (b.min(p + 1.0) - a.max(p)).clamp(0.0, 1.0)
}

/// An opaque RGB canvas with the few 2D-canvas operations the looks use,
/// all anti-aliased and blended source-over. Buffers are reused.
#[derive(Default)]
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    px: Vec<[f32; 3]>,
    /// Coverage of the path being stroked and of its shadow (Waves).
    mask: Vec<f32>,
    glow: Vec<f32>,
    lut: Vec<f32>,
    /// Bilinear tables for showing a Field.
    xs: Vec<(usize, usize, f32)>,
}

impl Canvas {
    /// A new size clears the canvas, as resizing a canvas does.
    pub fn resize(&mut self, w: usize, h: usize) {
        self.w = w;
        self.h = h;
        self.px.clear();
        self.px.resize(w * h, BACKDROP);
        self.mask.clear();
        self.glow.clear();
    }

    #[inline]
    fn blend(&mut self, i: usize, col: [f32; 3], a: f32) {
        let p = &mut self.px[i];
        let k = 1.0 - a;
        p[0] = p[0] * k + col[0] * a;
        p[1] = p[1] * k + col[1] * a;
        p[2] = p[2] * k + col[2] * a;
    }

    /// Fades what is on the canvas toward the backdrop, leaving trails; 1 clears it.
    pub fn fade(&mut self, amount: f32) {
        let k = 1.0 - amount;
        let add = [
            BACKDROP[0] * amount,
            BACKDROP[1] * amount,
            BACKDROP[2] * amount,
        ];
        for p in &mut self.px {
            p[0] = p[0] * k + add[0];
            p[1] = p[1] * k + add[1];
            p[2] = p[2] * k + add[2];
        }
    }

    /// `fillRect` with a colour per row (by the row's centre y), so it
    /// also does vertical gradients. Negative sizes are normalised.
    fn fill_rect_with(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        a: f32,
        color: impl Fn(f32) -> [f32; 3],
    ) {
        let (x0, x1) = if w < 0.0 { (x + w, x) } else { (x, x + w) };
        let (y0, y1) = if h < 0.0 { (y + h, y) } else { (y, y + h) };
        if x1 <= x0 || y1 <= y0 || a <= 0.0 || !(x0.is_finite() && y0.is_finite()) {
            return;
        }
        let ix0 = x0.floor().max(0.0) as usize;
        let ix1 = x1.ceil().min(self.w as f32).max(0.0) as usize;
        let iy0 = y0.floor().max(0.0) as usize;
        let iy1 = y1.ceil().min(self.h as f32).max(0.0) as usize;
        for iy in iy0..iy1 {
            let cy = overlap(iy, y0, y1);
            let col = color(iy as f32 + 0.5);
            for ix in ix0..ix1 {
                let cov = overlap(ix, x0, x1) * cy;
                if cov > 0.0 {
                    self.blend(iy * self.w + ix, col, a * cov);
                }
            }
        }
    }

    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, col: [f32; 3], a: f32) {
        self.fill_rect_with(x, y, w, h, a, |_| col);
    }

    fn bounds(&self, x0: f32, x1: f32, y0: f32, y1: f32) -> (usize, usize, usize, usize) {
        (
            x0.floor().max(0.0) as usize,
            x1.ceil().min(self.w as f32).max(0.0) as usize,
            y0.floor().max(0.0) as usize,
            y1.ceil().min(self.h as f32).max(0.0) as usize,
        )
    }

    /// One stroked line: round caps or butt caps.
    #[allow(clippy::too_many_arguments)]
    fn segment(
        &mut self,
        (ax, ay): (f32, f32),
        (bx, by): (f32, f32),
        width: f32,
        round: bool,
        col: [f32; 3],
        a: f32,
    ) {
        if a <= 0.0 || !(ax.is_finite() && ay.is_finite() && bx.is_finite() && by.is_finite()) {
            return;
        }
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        if !round && len2 <= 0.0 {
            return;
        }
        let len = len2.sqrt();
        let half = width / 2.0;
        let reach = half + 1.0;
        let (x0, x1, y0, y1) = self.bounds(
            ax.min(bx) - reach,
            ax.max(bx) + reach,
            ay.min(by) - reach,
            ay.max(by) + reach,
        );
        for py in y0..y1 {
            let qy = py as f32 + 0.5 - ay;
            for px in x0..x1 {
                let qx = px as f32 + 0.5 - ax;
                let cov = if round {
                    let t = if len2 > 0.0 {
                        ((qx * dx + qy * dy) / len2).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let d = (qx - t * dx).hypot(qy - t * dy);
                    (half + 0.5 - d).clamp(0.0, 1.0)
                } else {
                    let s = (qx * dx + qy * dy) / len;
                    let along = (s.min(len - s) + 0.5).clamp(0.0, 1.0);
                    let perp = (qx * dy - qy * dx).abs() / len;
                    (half + 0.5 - perp).clamp(0.0, 1.0) * along
                };
                if cov > 0.0 {
                    self.blend(py * self.w + px, col, a * cov);
                }
            }
        }
    }

    /// A stroked full circle, visiting only the pixels near the ring.
    fn ring(&mut self, cx: f32, cy: f32, r: f32, width: f32, col: [f32; 3], a: f32) {
        if r <= 0.0 || a <= 0.0 || !(cx.is_finite() && cy.is_finite() && r.is_finite()) {
            return;
        }
        let half = width / 2.0;
        let ro = r + half + 0.5;
        let ri = (r - half - 0.5).max(0.0);
        let (_, _, y0, y1) = self.bounds(0.0, 0.0, cy - ro, cy + ro);
        let wf = self.w as f32;
        for y in y0..y1 {
            let dy = y as f32 + 0.5 - cy;
            let dy2 = dy * dy;
            if dy2 >= ro * ro {
                continue;
            }
            let xo = (ro * ro - dy2).sqrt();
            let xi = if dy2 < ri * ri {
                (ri * ri - dy2).sqrt()
            } else {
                0.0
            };
            let clip = |v: f32| v.clamp(0.0, wf) as usize;
            let lx0 = clip((cx - xo).floor());
            let lx1 = clip((cx - xi).ceil());
            let rx0 = clip((cx + xi).floor()).max(lx1);
            let rx1 = clip((cx + xo).ceil()).max(rx0);
            for x in (lx0..lx1).chain(rx0..rx1) {
                let d = (x as f32 + 0.5 - cx).hypot(dy);
                let cov = (half + 0.5 - (d - r).abs()).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(y * self.w + x, col, a * cov);
                }
            }
        }
    }

    /// A filled circle.
    fn disc(&mut self, cx: f32, cy: f32, r: f32, col: [f32; 3], a: f32) {
        if r <= 0.0 || a <= 0.0 || !(cx.is_finite() && cy.is_finite() && r.is_finite()) {
            return;
        }
        let ro = r + 0.5;
        let (_, _, y0, y1) = self.bounds(0.0, 0.0, cy - ro, cy + ro);
        let wf = self.w as f32;
        for y in y0..y1 {
            let dy = y as f32 + 0.5 - cy;
            if dy * dy >= ro * ro {
                continue;
            }
            let xo = (ro * ro - dy * dy).sqrt();
            let x0 = (cx - xo).floor().clamp(0.0, wf) as usize;
            let x1 = (cx + xo).ceil().clamp(0.0, wf) as usize;
            for x in x0..x1 {
                let d = (x as f32 + 0.5 - cx).hypot(dy);
                let cov = (ro - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(y * self.w + x, col, a * cov);
                }
            }
        }
    }

    /// Strokes a polyline once (overlapping segments do not double up),
    /// with a canvas-style shadow under it: the shadow is the stroke
    /// blurred by a Gaussian of sigma `blur / 2`, taken per pixel from the
    /// nearest segment's exact blurred profile.
    fn glow_line(&mut self, pts: &[(f32, f32)], width: f32, blur: f32, col: [f32; 3], a: f32) {
        let n = self.w * self.h;
        if self.mask.len() != n {
            self.mask.clear();
            self.mask.resize(n, 0.0);
            self.glow.clear();
            self.glow.resize(n, 0.0);
        }
        let half = width / 2.0;
        let sigma = blur / 2.0;
        let reach = if sigma > 0.0 {
            (half + 3.0 * sigma).max(half + 1.0)
        } else {
            half + 1.0
        };
        // The blurred profile of a line of this width, every quarter pixel.
        self.lut.clear();
        let k = if sigma > 0.0 {
            1.0 / (sigma * std::f32::consts::SQRT_2)
        } else {
            0.0
        };
        for i in 0..(reach * 4.0).ceil() as usize + 2 {
            let d = i as f32 * 0.25;
            self.lut.push(if sigma > 0.0 {
                0.5 * (erf((d + half) * k) - erf((d - half) * k))
            } else {
                0.0
            });
        }
        let reach2 = reach * reach;
        let (mut bx0, mut bx1, mut by0, mut by1) = (usize::MAX, 0, usize::MAX, 0);
        for seg in pts.windows(2) {
            let ((ax, ay), (bx, by)) = (seg[0], seg[1]);
            if !(ax.is_finite() && ay.is_finite() && bx.is_finite() && by.is_finite()) {
                continue;
            }
            let (dx, dy) = (bx - ax, by - ay);
            let len2 = dx * dx + dy * dy;
            let inv = if len2 > 0.0 { 1.0 / len2 } else { 0.0 };
            let (x0, x1, y0, y1) = self.bounds(
                ax.min(bx) - reach,
                ax.max(bx) + reach,
                ay.min(by) - reach,
                ay.max(by) + reach,
            );
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            bx0 = bx0.min(x0);
            bx1 = bx1.max(x1);
            by0 = by0.min(y0);
            by1 = by1.max(y1);
            for py in y0..y1 {
                let qy = py as f32 + 0.5 - ay;
                let row = py * self.w;
                for px in x0..x1 {
                    let qx = px as f32 + 0.5 - ax;
                    let t = ((qx * dx + qy * dy) * inv).clamp(0.0, 1.0);
                    let (ex, ey) = (qx - t * dx, qy - t * dy);
                    let d2 = ex * ex + ey * ey;
                    if d2 >= reach2 {
                        continue;
                    }
                    let d = d2.sqrt();
                    let i = row + px;
                    let cov = (half + 0.5 - d).clamp(0.0, 1.0);
                    if cov > self.mask[i] {
                        self.mask[i] = cov;
                    }
                    let g = self.lut[(d * 4.0) as usize];
                    if g > self.glow[i] {
                        self.glow[i] = g;
                    }
                }
            }
        }
        for py in by0..by1 {
            for px in bx0..bx1 {
                let i = py * self.w + px;
                let (g, m) = (self.glow[i], self.mask[i]);
                if g > 0.0 {
                    self.blend(i, col, a * g);
                    self.glow[i] = 0.0;
                }
                if m > 0.0 {
                    self.blend(i, col, a * m);
                    self.mask[i] = 0.0;
                }
            }
        }
    }

    /// Draws a Field stretched over the whole canvas, smoothed (bilinear).
    fn show_field(&mut self, f: &Field) {
        if f.w == 0 || f.h == 0 || self.w == 0 || self.h == 0 {
            return;
        }
        let sx = f.w as f32 / self.w as f32;
        let sy = f.h as f32 / self.h as f32;
        let tap = |o: f32, n: usize| {
            let u = (o.max(0.0)).min((n - 1) as f32);
            let i0 = u.floor() as usize;
            let i1 = (i0 + 1).min(n - 1);
            (i0, i1, u - i0 as f32)
        };
        self.xs.clear();
        for x in 0..self.w {
            self.xs.push(tap((x as f32 + 0.5) * sx - 0.5, f.w));
        }
        let px = &mut self.px;
        for y in 0..self.h {
            let (y0, y1, fy) = tap((y as f32 + 0.5) * sy - 0.5, f.h);
            let (r0, r1) = (&f.data[y0 * f.w..][..f.w], &f.data[y1 * f.w..][..f.w]);
            let out = &mut px[y * self.w..][..self.w];
            for (o, &(x0, x1, fx)) in out.iter_mut().zip(&self.xs) {
                for k in 0..3 {
                    let top = r0[x0][k] + (r0[x1][k] - r0[x0][k]) * fx;
                    let bot = r1[x0][k] + (r1[x1][k] - r1[x0][k]) * fx;
                    o[k] = top + (bot - top) * fy;
                }
            }
        }
    }

    pub fn image(&self) -> egui::ColorImage {
        egui::ColorImage {
            size: [self.w, self.h],
            pixels: self
                .px
                .iter()
                // `as u8` saturates; adding a half rounds (f32::round is a slow call here).
                .map(|p| {
                    Color32::from_rgb((p[0] + 0.5) as u8, (p[1] + 0.5) as u8, (p[2] + 0.5) as u8)
                })
                .collect(),
        }
    }
}

/// A small canvas drawn pixel by pixel, then scaled up smoothly: cheap
/// soft pictures. Values are clamped to 0..255 as a byte canvas would.
#[derive(Default)]
struct Field {
    w: usize,
    h: usize,
    data: Vec<[f32; 3]>,
}

impl Field {
    fn fit(&mut self, width: usize, height: usize, cell: f32) {
        let w = ((width as f32 / cell).round() as usize).max(8);
        let h = ((height as f32 / cell).round() as usize).max(8);
        if w != self.w || h != self.h {
            self.w = w;
            self.h = h;
            self.data.clear();
            self.data.resize(w * h, [0.0; 3]);
        }
    }
    #[inline]
    fn set(&mut self, i: usize, r: f32, g: f32, b: f32) {
        self.data[i] = [
            r.clamp(0.0, 255.0),
            g.clamp(0.0, 255.0),
            b.clamp(0.0, 255.0),
        ];
    }
}

// ------------------------------------------------------------------- looks

/// The original's seeded generator (Park and Miller).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = (self.0 * 16807) % 2_147_483_647;
        (self.0 as f64 / 2_147_483_647.0) as f32
    }
}

trait Visual {
    fn draw(&mut self, cv: &mut Canvas, a: &Analysis, pal: &Palette);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Lava,
    Bars,
    Waves,
    Plasma,
    Ripples,
    Stars,
    Radial,
    Dancer,
}

/// In the order the list shows them.
pub const LOOKS: [Look; 8] = [
    Look::Lava,
    Look::Bars,
    Look::Waves,
    Look::Plasma,
    Look::Ripples,
    Look::Stars,
    Look::Radial,
    Look::Dancer,
];

impl Look {
    pub fn id(self) -> &'static str {
        match self {
            Look::Lava => "lava",
            Look::Bars => "bars",
            Look::Waves => "waves",
            Look::Plasma => "plasma",
            Look::Ripples => "ripples",
            Look::Stars => "stars",
            Look::Radial => "radial",
            Look::Dancer => "dancer",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Look::Lava => "Lava Lamp",
            Look::Bars => "Bars",
            Look::Waves => "Waves",
            Look::Plasma => "Plasma",
            Look::Ripples => "Ripples",
            Look::Stars => "Starfield",
            Look::Radial => "Radial",
            Look::Dancer => "TV Dancer",
        }
    }
    /// The look with this id, or the first.
    pub fn by_id(id: &str) -> Look {
        LOOKS.into_iter().find(|l| l.id() == id).unwrap_or(LOOKS[0])
    }
    fn create(self) -> Box<dyn Visual> {
        match self {
            Look::Lava => Box::new(Lava::default()),
            Look::Bars => Box::new(Bars::default()),
            Look::Waves => Box::new(Waves::default()),
            Look::Plasma => Box::new(Plasma::default()),
            Look::Ripples => Box::new(Ripples::default()),
            Look::Stars => Box::new(Stars::default()),
            Look::Radial => Box::new(Radial::default()),
            Look::Dancer => Box::new(Dancer::default()),
        }
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// Lava Lamp: warm blobs of wax that heat at the bottom, rise, cool at the
// top and sink, merging and parting like the real thing. The bass pushes
// them up and swells them; the colors drift and follow the music.

struct Blob {
    x: f32,
    y: f32,
    r: f32,
    vy: f32,
    heat: f32,
    hue: f32,
    phase: f32,
}

const LAVA_COUNT: usize = 10;

struct Lava {
    field: Field,
    blobs: Vec<Blob>,
    rng: Lcg,
}

impl Default for Lava {
    fn default() -> Self {
        Lava {
            field: Field::default(),
            blobs: Vec::new(),
            rng: Lcg(1),
        }
    }
}

impl Visual for Lava {
    fn draw(&mut self, cv: &mut Canvas, a: &Analysis, pal: &Palette) {
        if self.blobs.is_empty() {
            for i in 0..LAVA_COUNT {
                let x = 0.2 + self.rng.next() * 0.6;
                let y = self.rng.next();
                let r = 0.1 + self.rng.next() * 0.07;
                let heat = self.rng.next();
                // 6.28, as the original (not exactly two pi).
                #[allow(clippy::approx_constant)]
                let phase = self.rng.next() * 6.28;
                self.blobs.push(Blob {
                    x,
                    y,
                    r,
                    vy: 0.0,
                    heat,
                    hue: i as f32 / LAVA_COUNT as f32,
                    phase,
                });
            }
        }
        let dt = a.dt.min(0.1);
        // Heat from the bottom, cooling at the top; the music adds warmth and, on a beat, a push.
        for b in &mut self.blobs {
            let bottom = smoothstep(0.7, 1.0, b.y);
            let top = smoothstep(0.3, 0.0, b.y);
            b.heat += (bottom * (0.45 + a.level * 2.0) - top * 0.5 - 0.06) * dt * 1.2;
            b.heat = b.heat.clamp(0.0, 1.0);
            let lift = (b.heat - 0.5) * 0.22 + a.bass * 0.34;
            b.vy += (-lift - b.vy) * (dt * 1.6).min(1.0);
            if a.beat {
                b.vy -= 0.18 + self.rng.next() * 0.2;
            }
            b.y += b.vy * dt;
            if b.y < -0.05 {
                b.y = -0.05;
                b.vy = b.vy.abs() * 0.3;
            }
            if b.y > 1.05 {
                b.y = 1.05;
                b.vy = -b.vy.abs() * 0.3;
            }
            b.x += ((a.time * 0.4) as f32 + b.phase).sin() * 0.03 * dt;
            b.x = b.x.clamp(0.12, 0.88);
        }
        let fld = &mut self.field;
        fld.fit(cv.w, cv.h, 6.0);
        let (w, h) = (fld.w, fld.h);
        let aspect = cv.w as f32 / cv.h as f32;
        let sw = 1.0 + a.bass * 0.8 + a.level * 0.3;
        let drift = a.tk(0.02) + a.mid * 0.4;
        // Per blob, the radius squared this picture (it does not change per pixel).
        let mut r2 = [0.0f32; LAVA_COUNT];
        for (o, b) in r2.iter_mut().zip(&self.blobs) {
            let r = b.r * sw * (0.8 + b.heat * 0.5);
            *o = r * r;
        }
        for py in 0..h {
            let y = py as f32 / h as f32;
            for px in 0..w {
                let x = px as f32 / w as f32;
                let mut sum = 0.0;
                let mut hue = 0.0;
                for (b, rr) in self.blobs.iter().zip(&r2) {
                    let dx = (x - b.x) * aspect;
                    let dy = y - b.y;
                    let v = rr / (dx * dx + dy * dy + 1e-4);
                    sum += v;
                    hue += v * b.hue;
                }
                let wsum = sum;
                let c = pal.at(((if wsum > 0.0 { hue / wsum } else { 0.0 }) + drift) % 1.0);
                let body = smoothstep(0.85, 1.25, sum);
                let glow = smoothstep(0.25, 0.95, sum) * 0.55;
                // Lit from below: the glass glows warm at the bottom, dark wax shows against it.
                let lamp = 0.08 + 0.2 * y;
                let rim = smoothstep(1.25, 0.9, sum) * body;
                let m = (body * 0.85 + glow * 0.5).min(1.0);
                let lit = 0.6 + 0.5 * a.level + 0.3 * y;
                fld.set(
                    py * w + px,
                    (14.0 + lamp * 90.0 + (c[0] * m + 90.0 * rim) * lit).min(255.0),
                    (8.0 + lamp * 45.0 + (c[1] * m + 60.0 * rim) * lit).min(255.0),
                    (20.0 + lamp * 20.0 + c[2] * m * lit).min(255.0),
                );
            }
        }
        cv.show_field(fld);
    }
}

// Bars: the spectrum as columns with a falling cap on each.

#[derive(Default)]
struct Bars {
    caps: Vec<f32>,
    speed: Vec<f32>,
}

impl Visual for Bars {
    fn draw(&mut self, cv: &mut Canvas, a: &Analysis, pal: &Palette) {
        let (w, h) = (cv.w as f32, cv.h as f32);
        cv.fill_rect(0.0, 0.0, w, h, BACKDROP, 1.0);
        let n = ((w / 9.0).floor() as usize).clamp(12, 64);
        if self.caps.len() < n {
            self.caps.resize(n, 0.0);
            self.speed.resize(n, 0.0);
        }
        let gap = (w / n as f32 / 6.0).floor().max(1.0);
        let bw = w / n as f32;
        let len = a.levels.len();
        let c0 = pal.css(0.0);
        let cap_col = pal.css(0.9);
        for i in 0..n {
            let from = i * len / n;
            let to = ((i + 1) * len / n).max(from + 1);
            let v = a.levels[from..to].iter().copied().fold(0.0, f32::max);
            let bh = v * (h - 6.0);
            let x = i as f32 * bw;
            // The gradient runs from the floor (stop 0) to the bar's top (stop 1), clamped beyond.
            let g = bh.max(2.0);
            let c1 = pal.css(0.35 + v * 0.65);
            cv.fill_rect_with(x + gap / 2.0, h - bh, bw - gap, bh, 1.0, |y| {
                let t = ((h - y) / g).clamp(0.0, 1.0);
                [
                    c0[0] + (c1[0] - c0[0]) * t,
                    c0[1] + (c1[1] - c0[1]) * t,
                    c0[2] + (c1[2] - c0[2]) * t,
                ]
            });
            let cap = self.caps[i];
            let mut sp = self.speed[i];
            if bh >= cap {
                self.caps[i] = bh;
                sp = 0.0;
            } else {
                sp += a.dt * 260.0;
                self.caps[i] = bh.max(cap - sp * a.dt);
            }
            self.speed[i] = sp;
            cv.fill_rect(
                x + gap / 2.0,
                h - self.caps[i] - 4.0,
                bw - gap,
                2.0,
                cap_col,
                1.0,
            );
        }
    }
}

// Waves: a glowing line that ripples with the sound, with a mirrored echo.

#[derive(Default)]
struct Waves {
    phase: f32,
    pts: Vec<(f32, f32)>,
}

impl Visual for Waves {
    fn draw(&mut self, cv: &mut Canvas, a: &Analysis, pal: &Palette) {
        let (w, h) = (cv.w as f32, cv.h as f32);
        cv.fade(0.22);
        self.phase += a.dt * (1.5 + a.level * 6.0);
        let mid = h / 2.0;
        let steps = ((w / 3.0).floor() as usize).max(40);
        for pass in 0..3usize {
            self.pts.clear();
            let pf = pass as f32;
            for s in 0..=steps {
                let x = s as f32 / steps as f32;
                let mut y = 0.0;
                // A few of the spectrum's bands, each a sine of its own pitch: it moves like a scope trace.
                for k in 0..8usize {
                    let amp = a.levels.get(k * 7 + pass).copied().unwrap_or(0.0);
                    let kf = k as f32;
                    y += amp
                        * (x * std::f32::consts::PI * 2.0 * (kf + 1.0) * (1.0 + pf * 0.5)
                            + self.phase * (0.7 + kf * 0.3)
                            + pf)
                            .sin();
                }
                let env = (x * std::f32::consts::PI).sin();
                let py =
                    mid + (y / 4.0) * mid * env * (0.35 + 0.65 * if pass == 0 { 1.0 } else { 0.7 });
                self.pts.push((x * w, py));
            }
            let col = pal.css(a.tk(0.05) + pf * 0.18);
            let (width, blur, alpha) = if pass == 0 {
                (3.0, 14.0, 0.95)
            } else {
                (1.5, 6.0, 0.55)
            };
            cv.glow_line(&self.pts, width, blur, col, alpha);
        }
    }
}

// Plasma: drifting color clouds that churn faster and stretch with the bass.

#[derive(Default)]
struct Plasma {
    field: Field,
    t: f32,
}

impl Visual for Plasma {
    fn draw(&mut self, cv: &mut Canvas, a: &Analysis, pal: &Palette) {
        self.t += a.dt * (0.4 + a.level * 2.4);
        let fld = &mut self.field;
        fld.fit(cv.w, cv.h, 7.0);
        let (w, h) = (fld.w, fld.h);
        let zoom = 3.0 + a.bass * 2.2;
        let t = self.t;
        let aspect = cv.w as f32 / cv.h as f32;
        let drift = a.tk(0.03) + a.mid * 0.2;
        let bright = (a.level * 1.3 + 0.15).min(1.0);
        let (cx, cy) = (2.0 - (t * 0.5).sin(), 1.5 - (t * 0.4).cos());
        for py in 0..h {
            let y = (py as f32 / h as f32) * zoom;
            for px in 0..w {
                let x = (px as f32 / w as f32) * zoom * aspect;
                let v = (x * 2.1 + t).sin()
                    + (y * 2.7 - t * 1.3).sin()
                    + ((x + y) * 1.7 + t * 0.7).sin()
                    + ((x - cx).hypot(y - cy) * 3.1 - t * 1.1).sin();
                let c = pal.at((v + 4.0) / 8.0 + drift);
                let k = 0.45 + 0.75 * bright * (0.6 + 0.4 * (v * 1.2).sin());
                fld.set(py * w + px, c[0] * k, c[1] * k, c[2] * k);
            }
        }
        cv.show_field(fld);
    }
}

// Ripples: rings spread from a point on each beat, like drops on water.

struct Ring {
    x: f32,
    y: f32,
    r: f32,
    speed: f32,
    life: f32,
    hue: f32,
    width: f32,
}

struct Ripples {
    rings: Vec<Ring>,
    rng: Lcg,
    since: f32,
}

impl Default for Ripples {
    fn default() -> Self {
        Ripples {
            rings: Vec::with_capacity(41),
            rng: Lcg(7),
            since: 0.0,
        }
    }
}

impl Ripples {
    fn drop(&mut self, w: f32, h: f32, strength: f32, hue: f32) {
        let x = w * (0.15 + self.rng.next() * 0.7);
        let y = h * (0.15 + self.rng.next() * 0.7);
        self.rings.push(Ring {
            x,
            y,
            r: 2.0,
            speed: w.min(h) * (0.25 + strength * 0.5),
            life: 1.0,
            hue,
            width: 1.5 + strength * 4.0,
        });
        if self.rings.len() > 40 {
            self.rings.remove(0);
        }
    }
}

impl Visual for Ripples {
    fn draw(&mut self, cv: &mut Canvas, a: &Analysis, pal: &Palette) {
        let (w, h) = (cv.w as f32, cv.h as f32);
        cv.fade(0.18);
        self.since += a.dt;
        if a.beat {
            let hue = self.rng.next();
            self.drop(w, h, a.bass, hue);
        } else if a.treble > 0.3 && self.since > 0.35 {
            self.since = 0.0;
            let hue = self.rng.next();
            self.drop(w, h, a.treble * 0.5, hue);
        } else if a.level < 0.02 && self.since > 2.5 {
            // Quiet: a slow drop now and then keeps it alive.
            self.since = 0.0;
            let hue = self.rng.next();
            self.drop(w, h, 0.1, hue);
        }
        for ring in &mut self.rings {
            ring.r += ring.speed * a.dt;
            ring.life -= a.dt * 0.45;
            if ring.life <= 0.0 {
                continue;
            }
            for k in 0..3 {
                let kf = k as f32;
                // A circle of no size is skipped (the original's canvas refuses one).
                let r = ring.r - kf * ring.width * 1.6;
                let width = (ring.width * (1.0 - kf * 0.25)).max(0.5);
                let col = pal.css(ring.hue + kf * 0.05);
                let alpha = ring.life.max(0.0) * (1.0 - kf * 0.3);
                if r > 0.0 {
                    cv.ring(ring.x, ring.y, r, width, col, alpha);
                }
            }
        }
        self.rings.retain(|r| r.life > 0.0);
    }
}

// Starfield: flying through stars that stream faster as it gets louder
// and flash on a beat.

struct Star {
    x: f32,
    y: f32,
    z: f32,
    hue: f32,
}

const STAR_COUNT: usize = 220;

struct Stars {
    stars: Vec<Star>,
    rng: Lcg,
    flash: f32,
}

impl Default for Stars {
    fn default() -> Self {
        Stars {
            stars: Vec::new(),
            rng: Lcg(3),
            flash: 0.0,
        }
    }
}

fn spawn(rng: &mut Lcg, s: &mut Star) {
    s.x = rng.next() * 2.0 - 1.0;
    s.y = rng.next() * 2.0 - 1.0;
    s.z = 0.2 + rng.next() * 0.8;
    s.hue = rng.next();
}

impl Visual for Stars {
    fn draw(&mut self, cv: &mut Canvas, a: &Analysis, pal: &Palette) {
        let (w, h) = (cv.w as f32, cv.h as f32);
        if self.stars.is_empty() {
            for _ in 0..STAR_COUNT {
                let mut s = Star {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    hue: 0.0,
                };
                spawn(&mut self.rng, &mut s);
                self.stars.push(s);
            }
        }
        cv.fade(0.3);
        if a.beat {
            self.flash = 1.0;
        }
        self.flash = (self.flash - a.dt * 3.0).max(0.0);
        let speed = 0.12 + a.level * 1.1 + a.bass * 0.6;
        let (cx, cy) = (w / 2.0, h / 2.0);
        let scale = w.max(h) * 0.6;
        let drift = a.tk(0.04);
        for s in &mut self.stars {
            let px = (s.x / s.z) * scale + cx;
            let py = (s.y / s.z) * scale + cy;
            s.z -= speed * a.dt;
            if s.z <= 0.02 || px < -20.0 || px > w + 20.0 || py < -20.0 || py > h + 20.0 {
                spawn(&mut self.rng, s);
                s.z = 1.0;
                continue;
            }
            let nx = (s.x / s.z) * scale + cx;
            let ny = (s.y / s.z) * scale + cy;
            let near = 1.0 - s.z;
            let col = pal.css(s.hue + drift);
            let alpha = (0.25 + near + self.flash * 0.4).min(1.0);
            let width = 0.6 + near * 2.4 + self.flash;
            cv.segment((px, py), (nx, ny), width, false, col, alpha);
        }
    }
}

// Radial: the spectrum in a ring that turns slowly and swells on the bass.

#[derive(Default)]
struct Radial {
    angle: f32,
}

impl Visual for Radial {
    fn draw(&mut self, cv: &mut Canvas, a: &Analysis, pal: &Palette) {
        use std::f32::consts::PI;
        let (w, h) = (cv.w as f32, cv.h as f32);
        cv.fade(0.25);
        self.angle += a.dt * (0.15 + a.mid * 0.8);
        let (cx, cy) = (w / 2.0, h / 2.0);
        let base = w.min(h) * (0.18 + a.bass * 0.06);
        let reach = w.min(h) * 0.3;
        let n = a.levels.len();
        let width = ((PI * 2.0 * base) / n as f32 / 1.6).max(2.0);
        let drift = a.tk(0.04);
        for mirror in 0..2 {
            for i in 0..n {
                let t = i as f32 / n as f32;
                let ang = self.angle
                    + (if mirror == 0 { t } else { -t }) * PI
                    + if mirror == 0 { 0.0 } else { PI };
                let len = a.levels[i] * reach + 2.0;
                let (s, c) = ang.sin_cos();
                cv.segment(
                    (cx + c * base, cy + s * base),
                    (cx + c * (base + len), cy + s * (base + len)),
                    width,
                    true,
                    pal.css(t + drift),
                    0.9,
                );
            }
        }
        cv.disc(
            cx,
            cy,
            base * (0.55 + a.bass * 0.4),
            pal.css(a.tk(0.05) + 0.5),
            0.35 + a.bass * 0.5,
        );
    }
}

// TV Dancer: a little alien of our own on an old television, floating and
// doing slow, goofy dance moves. The bob and sway follow the level, each
// beat moves it on to its next move, its antennae wobble on the highs and
// its mouth sings with the loudness. Quiet, it just floats and blinks.

#[derive(Default)]
struct Dancer {
    /// Where the dance is: one unit per move.
    phase: f32,
    /// The move being danced, and the one before (blended over a beat).
    step: usize,
    blend: f32,
    /// Seconds to the next blink, and how far through one.
    blink_in: f32,
    blink: f32,
    /// Smoothed loudness, so the moves swell rather than twitch.
    energy: f32,
    wobble: f32,
}

/// A pose: arm and leg angles (radians, 0 = straight down, outward
/// positive) and how far the body leans and dips.
#[derive(Clone, Copy)]
struct Pose {
    upper_l: f32,
    lower_l: f32,
    upper_r: f32,
    lower_r: f32,
    hip_l: f32,
    knee_l: f32,
    hip_r: f32,
    knee_r: f32,
    lean: f32,
    dip: f32,
}

/// The moves: each a pair of poses it swings between.
const MOVES: [[Pose; 2]; 4] = [
    // The noodle: arms rolling out and in, knees bent wide.
    [
        Pose { upper_l: 1.1, lower_l: -1.4, upper_r: 0.6, lower_r: -0.5, hip_l: 0.35, knee_l: -0.5, hip_r: 0.35, knee_r: -0.5, lean: -0.08, dip: 0.0 },
        Pose { upper_l: 0.6, lower_l: -0.5, upper_r: 1.1, lower_r: -1.4, hip_l: 0.35, knee_l: -0.5, hip_r: 0.35, knee_r: -0.5, lean: 0.08, dip: 0.6 },
    ],
    // The wave: one arm up over the head, swaying.
    [
        Pose { upper_l: 2.6, lower_l: 0.5, upper_r: 0.4, lower_r: -0.3, hip_l: 0.2, knee_l: -0.2, hip_r: 0.45, knee_r: -0.8, lean: -0.12, dip: 0.3 },
        Pose { upper_l: 2.2, lower_l: -0.6, upper_r: 0.5, lower_r: -0.6, hip_l: 0.45, knee_l: -0.8, hip_r: 0.2, knee_r: -0.2, lean: 0.1, dip: 0.7 },
    ],
    // The squat bounce: hands on knees, sinking and rising.
    [
        Pose { upper_l: 0.5, lower_l: -0.9, upper_r: 0.5, lower_r: -0.9, hip_l: 0.55, knee_l: -1.0, hip_r: 0.55, knee_r: -1.0, lean: 0.0, dip: 1.0 },
        Pose { upper_l: 0.7, lower_l: -0.3, upper_r: 0.7, lower_r: -0.3, hip_l: 0.25, knee_l: -0.3, hip_r: 0.25, knee_r: -0.3, lean: 0.0, dip: 0.0 },
    ],
    // The sprinkler: both arms out, turning side to side.
    [
        Pose { upper_l: 1.6, lower_l: 0.2, upper_r: 1.5, lower_r: 1.2, hip_l: 0.3, knee_l: -0.4, hip_r: 0.3, knee_r: -0.4, lean: -0.15, dip: 0.2 },
        Pose { upper_l: 1.5, lower_l: 1.2, upper_r: 1.6, lower_r: 0.2, hip_l: 0.3, knee_l: -0.4, hip_r: 0.3, knee_r: -0.4, lean: 0.15, dip: 0.5 },
    ],
];

fn mix_pose(a: &Pose, b: &Pose, t: f32) -> Pose {
    let m = |x: f32, y: f32| x + (y - x) * t;
    Pose {
        upper_l: m(a.upper_l, b.upper_l),
        lower_l: m(a.lower_l, b.lower_l),
        upper_r: m(a.upper_r, b.upper_r),
        lower_r: m(a.lower_r, b.lower_r),
        hip_l: m(a.hip_l, b.hip_l),
        knee_l: m(a.knee_l, b.knee_l),
        hip_r: m(a.hip_r, b.hip_r),
        knee_r: m(a.knee_r, b.knee_r),
        lean: m(a.lean, b.lean),
        dip: m(a.dip, b.dip),
    }
}

/// The pose of move `step` at `phase` (0..1 swings there and back, eased).
fn pose_of(step: usize, phase: f32) -> Pose {
    let mv = &MOVES[step % MOVES.len()];
    let swing = 0.5 - 0.5 * (phase * std::f32::consts::TAU).cos();
    mix_pose(&mv[0], &mv[1], swing)
}

impl Visual for Dancer {
    fn draw(&mut self, cv: &mut Canvas, a: &Analysis, pal: &Palette) {
        let (w, h) = (cv.w as f32, cv.h as f32);
        let dt = a.dt.clamp(0.0, 0.1);
        // The room behind the set.
        cv.fill_rect(0.0, 0.0, w, h, [24.0, 22.0, 30.0], 1.0);
        cv.fill_rect(0.0, h * 0.8, w, h * 0.2, [34.0, 30.0, 40.0], 1.0);

        // The television.
        let tv_w = (w * 0.86).min(h * 1.15);
        let tv_h = tv_w * 0.78;
        let (tx, ty) = ((w - tv_w) / 2.0, (h - tv_h) / 2.0 - h * 0.02);
        let body = [150.0, 152.0, 160.0];
        cv.fill_rect(tx + tv_w * 0.18, ty + tv_h, tv_w * 0.64, tv_h * 0.05, [40.0, 40.0, 46.0], 1.0);
        cv.fill_rect(tx, ty, tv_w, tv_h, body, 1.0);
        cv.fill_rect(tx, ty + tv_h * 0.86, tv_w, tv_h * 0.14, [128.0, 130.0, 138.0], 1.0);
        // Speaker grilles either side of the screen.
        let grille = [118.0, 120.0, 128.0];
        for side in [0.0f32, 1.0] {
            let gx = tx + tv_w * (0.035 + side * 0.865);
            for row in 0..14 {
                for col in 0..3 {
                    cv.disc(
                        gx + col as f32 * tv_w * 0.025 + tv_w * 0.012,
                        ty + tv_h * 0.1 + row as f32 * tv_h * 0.052,
                        tv_w * 0.005,
                        grille,
                        1.0,
                    );
                }
            }
        }
        // Knobs and the tape slot along the bottom.
        cv.ring(tx + tv_w * 0.16, ty + tv_h * 0.93, tv_w * 0.018, 2.0, [60.0, 60.0, 66.0], 1.0);
        cv.ring(tx + tv_w * 0.2, ty + tv_h * 0.93, tv_w * 0.012, 2.0, [60.0, 60.0, 66.0], 1.0);
        cv.fill_rect(tx + tv_w * 0.3, ty + tv_h * 0.905, tv_w * 0.42, tv_h * 0.05, [110.0, 112.0, 120.0], 1.0);
        // The screen, faintly lit in the scheme's colour.
        let (sx, sy, sw, sh) = (tx + tv_w * 0.16, ty + tv_h * 0.07, tv_w * 0.68, tv_h * 0.74);
        cv.fill_rect(sx - 4.0, sy - 4.0, sw + 8.0, sh + 8.0, [70.0, 70.0, 80.0], 1.0);
        cv.fill_rect(sx, sy, sw, sh, [6.0, 6.0, 10.0], 1.0);
        let tint = pal.css(a.tk(0.02));
        cv.fill_rect(sx, sy, sw, sh, tint, 0.04 + a.level * 0.05);

        // The dance.
        // Up quickly with the music, down slowly, like a meter.
        let target = a.level.max(a.bass * 0.8);
        let rate = if target > self.energy { 8.0 } else { 0.8 };
        self.energy += (target - self.energy) * (dt * rate).min(1.0);
        let lively = self.energy.clamp(0.0, 1.0);
        // Slow, floaty moves: quicker when it's louder, never frantic.
        self.phase += dt * (0.22 + lively * 0.35);
        if a.beat && self.blend >= 1.0 && lively > 0.15 {
            self.step = (self.step + 1) % MOVES.len();
            self.blend = 0.0;
        }
        self.blend = (self.blend + dt * 1.2).min(1.0);
        let now = pose_of(self.step, self.phase);
        let was = pose_of(self.step + MOVES.len() - 1, self.phase);
        let calm = Pose { upper_l: 0.35, lower_l: -0.2, upper_r: 0.35, lower_r: -0.2, hip_l: 0.15, knee_l: -0.15, hip_r: 0.15, knee_r: -0.15, lean: 0.0, dip: 0.2 };
        let dancing = mix_pose(&was, &now, self.blend);
        let p = mix_pose(&calm, &dancing, (0.25 + lively * 1.5).min(1.0));
        self.wobble += dt * (2.0 + a.treble * 14.0);
        // Blinks.
        if self.blink_in <= 0.0 {
            self.blink_in = 2.5 + (a.time * 7.13).sin().abs() as f32 * 3.0;
            self.blink = 1.0;
        }
        self.blink_in -= dt;
        self.blink = (self.blink - dt * 7.0).max(0.0);

        // Its size and where it floats.
        let u = sh / 9.0;
        let float = (a.time as f32 * 0.9).sin() * u * 0.25 + p.dip * u * 0.35;
        let cx = sx + sw / 2.0 + p.lean * u * 2.0;
        let hip_y = sy + sh * 0.62 + float;
        let skin = {
            let t = pal.css(0.35);
            [t[0] * 0.55 + 90.0, t[1] * 0.55 + 110.0, t[2] * 0.55 + 80.0]
        };
        let shade = [skin[0] * 0.72, skin[1] * 0.72, skin[2] * 0.72];
        let limb = u * 0.32;
        // A limb from `from` at `ang` (0 down; `dir` -1 left, +1 right).
        let joint = |from: (f32, f32), ang: f32, len: f32, dir: f32| {
            (from.0 + ang.sin() * len * dir, from.1 + ang.cos() * len)
        };
        // Legs.
        for (dir, hip, knee) in [(-1.0f32, p.hip_l, p.knee_l), (1.0, p.hip_r, p.knee_r)] {
            let top = (cx + dir * u * 0.35, hip_y);
            let k = joint(top, hip, u * 1.35, dir);
            let f = joint(k, hip + knee, u * 1.3, dir);
            cv.segment(top, k, limb, true, shade, 1.0);
            cv.segment(k, f, limb * 0.9, true, shade, 1.0);
            // A little three-toed foot, toes pointed down (it floats).
            for t in [-0.35f32, 0.0, 0.35] {
                cv.segment(f, (f.0 + dir * u * 0.15 + t * u * 0.3, f.1 + u * 0.3), limb * 0.45, true, shade, 1.0);
            }
        }
        // Body: a soft pear.
        let neck = (cx + p.lean * u * 0.6, hip_y - u * 2.0);
        cv.segment((cx, hip_y - u * 0.2), neck, u * 1.25, true, skin, 1.0);
        cv.disc(cx, hip_y - u * 0.55, u * 0.78, skin, 1.0);
        // Arms.
        for (dir, upper, lower) in [(-1.0f32, p.upper_l, p.lower_l), (1.0, p.upper_r, p.lower_r)] {
            let sh_pt = (neck.0 + dir * u * 0.55, neck.1 + u * 0.25);
            let e = joint(sh_pt, upper, u * 1.15, dir);
            let hand = joint(e, upper + lower, u * 1.1, dir);
            cv.segment(sh_pt, e, limb * 0.9, true, skin, 1.0);
            cv.segment(e, hand, limb * 0.8, true, skin, 1.0);
            // Three long fingers, splayed.
            let fa = upper + lower;
            for k in [-0.45f32, 0.0, 0.45] {
                let tip = joint(hand, fa + k, u * 0.42, dir);
                cv.segment(hand, tip, limb * 0.35, true, skin, 1.0);
            }
        }
        // Head: big and round, a little tilted with the sway.
        let head = (neck.0 + p.lean * u * 1.2, neck.1 - u * 1.25);
        let hr = u * 1.15;
        cv.disc(head.0, head.1, hr, skin, 1.0);
        cv.disc(head.0, head.1 + hr * 0.45, hr * 0.62, skin, 1.0);
        // Antennae with bobbles, wobbling on the highs.
        for dir in [-1.0f32, 1.0] {
            let base = (head.0 + dir * hr * 0.45, head.1 - hr * 0.8);
            let sway = (self.wobble + dir).sin() * (0.15 + a.treble * 0.5);
            let tip = (base.0 + dir * u * 0.5 + sway * u * 0.6, base.1 - u * 1.0);
            cv.segment(base, tip, limb * 0.35, true, shade, 1.0);
            cv.disc(tip.0, tip.1, u * 0.18, pal.css(0.7 + dir * 0.1), 1.0);
        }
        // Eyes: huge, glossy, almond-ish (two discs each), with a blink.
        let open = 1.0 - (self.blink * std::f32::consts::PI).sin().max(0.0) * 0.9;
        for dir in [-1.0f32, 1.0] {
            let ex = head.0 + dir * hr * 0.42;
            let ey = head.1 + hr * 0.02;
            let er = hr * 0.3 * open.max(0.15);
            cv.disc(ex, ey, er, [8.0, 8.0, 14.0], 1.0);
            cv.disc(ex + dir * er * 0.35, ey - er * 0.2, er * 0.9, [8.0, 8.0, 14.0], 1.0);
            if open > 0.5 {
                cv.disc(ex - er * 0.3, ey - er * 0.35, er * 0.22, [240.0, 240.0, 250.0], 0.9);
            }
        }
        // A little round mouth that sings with the loudness.
        let mouth = u * (0.08 + lively * 0.22);
        cv.disc(head.0, head.1 + hr * 0.62, mouth, [40.0, 20.0, 30.0], 1.0);

        // The screen's own look: scan lines and a soft vignette.
        let mut y = sy;
        while y < sy + sh {
            cv.fill_rect(sx, y, sw, 1.0, [0.0, 0.0, 0.0], 0.22);
            y += 3.0;
        }
        for k in 0..6 {
            let inset = k as f32 * u * 0.12;
            cv.fill_rect(sx + inset, sy + inset, sw - inset * 2.0, u * 0.12, [0.0, 0.0, 0.0], 0.12);
            cv.fill_rect(sx + inset, sy + sh - inset - u * 0.12, sw - inset * 2.0, u * 0.12, [0.0, 0.0, 0.0], 0.12);
        }
    }
}

// ------------------------------------------------------------------- panel

const FPS: [u32; 3] = [15, 30, 60];
/// How much it moves while stopped.
const CALM: f32 = 0.28;
/// The canvas is at most this many pixels; bigger panels draw coarser.
const MAX_PIXELS: usize = 1_000_000;
const AUTO: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VizPrefs {
    pub preset: String,
    pub scheme: Scheme,
    pub sens: f32,
    pub fps: u32,
    pub auto: bool,
}

impl Default for VizPrefs {
    fn default() -> Self {
        VizPrefs {
            preset: LOOKS[0].id().to_string(),
            scheme: Scheme::Theme,
            sens: 1.0,
            fps: 30,
            auto: false,
        }
    }
}

impl VizPrefs {
    fn path() -> std::path::PathBuf {
        data_dir().join("visualizer.json")
    }

    /// Each choice is checked on its own; a bad one falls back alone.
    fn from_value(v: &serde_json::Value) -> VizPrefs {
        let d = VizPrefs::default();
        VizPrefs {
            preset: v
                .get("preset")
                .and_then(|x| x.as_str())
                .map_or(d.preset, str::to_string),
            scheme: v
                .get("scheme")
                .and_then(|x| x.as_str())
                .and_then(|s| SCHEMES.iter().find(|(k, _)| k.id() == s))
                .map_or(d.scheme, |(k, _)| *k),
            sens: v
                .get("sens")
                .and_then(|x| x.as_f64())
                .filter(|s| (0.4..=2.5).contains(s))
                .map_or(d.sens, |s| s as f32),
            fps: v
                .get("fps")
                .and_then(|x| x.as_u64())
                .filter(|f| FPS.contains(&(*f as u32)))
                .map_or(d.fps, |f| f as u32),
            auto: v.get("auto").and_then(|x| x.as_bool()).unwrap_or(d.auto),
        }
    }

    pub fn load() -> VizPrefs {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|v| Self::from_value(&v))
            .unwrap_or_default()
    }

    fn save(&self) {
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(
            Self::path(),
            serde_json::to_string_pretty(self).unwrap_or_default(),
        );
    }
}

/// The Visualizer's choices and everything it keeps between pictures.
pub struct Visualizer {
    pub prefs: VizPrefs,
    analyser: Analyser,
    spectrum: Spectrum,
    db: [f32; BANDS],
    canvas: Canvas,
    visual: Option<(Look, Box<dyn Visual>)>,
    tex: Option<egui::TextureHandle>,
    last: Option<Instant>,
    switched: Instant,
}

impl Default for Visualizer {
    fn default() -> Self {
        Visualizer {
            prefs: VizPrefs::default(),
            analyser: Analyser::default(),
            spectrum: Spectrum::default(),
            db: [-120.0; BANDS],
            canvas: Canvas::default(),
            visual: None,
            tex: None,
            last: None,
            switched: Instant::now(),
        }
    }
}

impl Visualizer {
    /// With the choices kept from the last run.
    pub fn load() -> Visualizer {
        Visualizer {
            prefs: VizPrefs::load(),
            ..Default::default()
        }
    }

    /// One picture of `look` at `w` x `h`. A new size starts the picture
    /// afresh (and clears the canvas); a new look starts afresh over what
    /// is on the canvas.
    fn render(&mut self, look: Look, w: usize, h: usize, a: &Analysis, pal: &Palette) {
        if self.canvas.w != w || self.canvas.h != h {
            self.canvas.resize(w, h);
            self.visual = None;
        }
        if self.visual.as_ref().map(|v| v.0) != Some(look) {
            self.visual = Some((look, look.create()));
        }
        if let Some((_, v)) = &mut self.visual {
            v.draw(&mut self.canvas, a, pal);
        }
    }
}

/// The Visualizer panel. Its state lives in `State::viz`.
// Until the dock offers its tab (ui.rs), nothing calls it.
#[allow(dead_code)]
pub fn show(ui: &mut egui::Ui, st: &mut State) {
    let mut v = std::mem::take(&mut st.viz);
    panel(ui, st, &mut v);
    st.viz = v;
}

fn panel(ui: &mut egui::Ui, st: &mut State, v: &mut Visualizer) {
    let dim = c(st.theme.text_dim);
    let before = v.prefs.clone();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Look").color(dim));
        let cur = Look::by_id(&v.prefs.preset);
        egui::ComboBox::from_id_salt("viz-look")
            .selected_text(cur.name())
            .show_ui(ui, |ui| {
                for l in LOOKS {
                    if ui.selectable_label(cur == l, l.name()).clicked() {
                        v.prefs.preset = l.id().to_string();
                    }
                }
            });
        ui.label(RichText::new("Colors").color(dim));
        egui::ComboBox::from_id_salt("viz-colors")
            .selected_text(v.prefs.scheme.label())
            .show_ui(ui, |ui| {
                for (s, label) in SCHEMES {
                    ui.selectable_value(&mut v.prefs.scheme, s, label);
                }
            });
        ui.label(RichText::new("Reaction").color(dim))
            .on_hover_text("How strongly the picture reacts to the sound.");
        ui.scope(|ui| {
            ui.spacing_mut().slider_width = 80.0;
            ui.add(
                crate::widgets::Slider::new(&mut v.prefs.sens, 0.4..=2.5)
                    .step_by(0.1)
                    .show_value(false),
            );
        });
        ui.label(RichText::new("Pictures a second").color(dim))
            .on_hover_text("15 and 30 are lighter on slower computers.");
        egui::ComboBox::from_id_salt("viz-fps")
            .selected_text(v.prefs.fps.to_string())
            .width(48.0)
            .show_ui(ui, |ui| {
                for f in FPS {
                    ui.selectable_value(&mut v.prefs.fps, f, f.to_string());
                }
            });
        ui.checkbox(&mut v.prefs.auto, "Change every 30 seconds");
    });
    if v.prefs != before {
        v.prefs.save();
    }

    let avail = ui.available_rect_before_wrap();
    let (rect, _) = ui.allocate_exact_size(avail.size(), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, AREA);
    let dpr = ui.ctx().pixels_per_point().min(1.5);
    let (mut w, mut h) = (
        (rect.width() * dpr).floor().max(0.0) as usize,
        (rect.height() * dpr).floor().max(0.0) as usize,
    );
    // Draws only while it has room (an inactive tab is not shown at all).
    if w < 4 || h < 4 {
        return;
    }
    if w * h > MAX_PIXELS {
        let k = (MAX_PIXELS as f32 / (w * h) as f32).sqrt();
        w = ((w as f32 * k).floor() as usize).max(4);
        h = ((h as f32 * k).floor() as usize).max(4);
    }
    let fps = v.prefs.fps.max(1);
    let step = Duration::from_secs_f32(1.0 / fps as f32);
    let now = Instant::now();
    let since = v.last.map(|t| now - t);
    let due = since.is_none_or(|s| s + Duration::from_millis(2) >= step);
    if due {
        // After a long gap (the tab was hidden) it starts again from one picture's step.
        let dt = match since {
            Some(s) if s.as_secs_f32() <= 1.0 => s.as_secs_f32().min(0.2),
            _ => 1.0 / fps as f32,
        };
        v.last = Some(now);
        st.scope_spectrum(&mut v.spectrum, &mut v.db);
        let playing = st.is_playing();
        let look = Look::by_id(&v.prefs.preset);
        let a = v
            .analyser
            .feed(&v.db, v.prefs.sens, dt, if playing { 1.0 } else { CALM });
        if v.prefs.auto && now.duration_since(v.switched) > AUTO {
            v.switched = now;
            let i = LOOKS.iter().position(|l| *l == look).unwrap_or(0);
            v.prefs.preset = LOOKS[(i + 1) % LOOKS.len()].id().to_string();
            v.prefs.save();
        }
        let pal = Palette::new(
            v.prefs.scheme,
            st.phase.low.or(st.theme.phase_low),
            st.phase.high.or(st.theme.phase_high),
        );
        v.render(look, w, h, &a, &pal);
        let img = v.canvas.image();
        match &mut v.tex {
            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
            None => {
                v.tex = Some(
                    ui.ctx()
                        .load_texture("visualizer", img, egui::TextureOptions::LINEAR),
                )
            }
        }
    }
    if let Some(t) = &v.tex {
        ui.painter_at(rect).image(
            t.id(),
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    let next = v
        .last
        .map_or(step, |t| step.saturating_sub(Instant::now() - t));
    ui.ctx().request_repaint_after(next);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> Analysis {
        Analyser::default().feed(&[], 1.0, 0.03, 1.0)
    }

    /// The TV Dancer dances to music (changing moves on beats) and only
    /// floats and blinks in silence; its picture stays sane.
    #[test]
    fn the_tv_dancer_dances_to_the_music() {
        let mut cv = Canvas::default();
        cv.resize(320, 240);
        let pal = Palette::new(Scheme::Theme, None, None);
        let mut d = Dancer::default();
        let q = quiet();
        for _ in 0..60 {
            d.draw(&mut cv, &q, &pal);
        }
        assert_eq!(d.step, 0, "no moves in silence");
        let mut a = Analyser::default();
        let quiet_db = [-80.0f32; BANDS];
        let mut loud = [-80.0f32; BANDS];
        loud[..10].fill(-6.0);
        for k in 0..400 {
            let x = a.feed(if k % 20 < 3 { &loud } else { &quiet_db }, 1.0, 0.033, 1.0);
            d.draw(&mut cv, &x, &pal);
        }
        assert!(d.step != 0 || d.blend < 1.0, "the beats moved it on");
        assert!(d.energy > 0.0);
        let img = cv.image();
        assert_eq!(img.size, [320, 240]);
    }

    #[test]
    fn maps_db_to_0_1() {
        assert_eq!(level_of(-120.0, 1.0), 0.0);
        assert_eq!(level_of(f32::NAN, 1.0), 0.0);
        assert_eq!(level_of(0.0, 1.0), 1.0);
        assert!(level_of(-30.0, 1.0) > level_of(-60.0, 1.0));
    }

    #[test]
    fn calls_a_jump_in_the_bass_a_beat_once() {
        let mut a = Analyser::default();
        let quiet = [-80.0f32; BANDS];
        let mut loud = [-80.0f32; BANDS];
        loud[..10].fill(-6.0);
        for _ in 0..20 {
            a.feed(&quiet, 1.0, 0.033, 1.0);
        }
        assert!(a.feed(&loud, 1.0, 0.033, 1.0).beat);
        assert!(!a.feed(&loud, 1.0, 0.033, 1.0).beat);
    }

    #[test]
    fn reads_silence_from_an_empty_spectrum() {
        let r = quiet();
        assert_eq!(r.level, 0.0);
        assert!(!r.beat);
    }

    #[test]
    fn lists_distinct_looks_and_falls_back_to_the_first() {
        let ids: std::collections::HashSet<_> = LOOKS.iter().map(|l| l.id()).collect();
        assert_eq!(ids.len(), LOOKS.len());
        assert_eq!(Look::by_id("nope"), LOOKS[0]);
        assert!(LOOKS.iter().any(|l| l.name() == "Lava Lamp"));
    }

    #[test]
    fn gives_every_color_scheme_a_color_for_any_position() {
        for (s, _) in SCHEMES {
            let pal = Palette::new(s, None, None);
            for t in [-0.3, 0.0, 0.5, 1.0, 7.25] {
                let c = pal.at(t);
                assert!(
                    c.iter().all(|v| v.is_finite() && *v >= -1.0 && *v <= 256.0),
                    "{s:?} {t} {c:?}"
                );
            }
        }
    }

    #[test]
    fn the_theme_scheme_runs_from_low_to_high_and_back() {
        let pal = Palette::new(Scheme::Theme, None, None);
        let lo = pal.at(0.0);
        let hi = pal.at(0.5);
        for k in 0..3 {
            assert!((lo[k] - DEFAULT_LOW[k] as f32).abs() < 1.0, "{lo:?}");
            assert!((hi[k] - DEFAULT_HIGH[k] as f32).abs() < 1.0, "{hi:?}");
        }
    }

    #[test]
    fn the_spectrum_puts_a_tone_in_its_band() {
        let rate = 48_000u32;
        let frames: Vec<[f32; 2]> = (0..4096)
            .map(|i| {
                let s = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / rate as f32).sin();
                [s, s]
            })
            .collect();
        let mut sp = Spectrum::default();
        let mut out = [0.0f32; BANDS];
        sp.bands(&frames, rate, &mut out);
        let peak = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        // 1 kHz is 56.6% of the way up 20 Hz..20 kHz in pitch.
        assert_eq!(peak.0, (BANDS as f64 * 50f64.ln() / 1000f64.ln()) as usize);
        assert!(
            peak.1.abs() < 1.5,
            "full scale reads about 0 dBFS: {}",
            peak.1
        );
        assert!(out.iter().all(|v| v.is_finite() && *v >= -120.0));
        // Silence, and too few frames, read as nothing.
        sp.bands(&[], rate, &mut out);
        assert!(out.iter().all(|v| *v == -120.0));
    }

    #[test]
    fn prefs_fall_back_one_by_one() {
        let v = serde_json::json!({"preset": "bars", "scheme": "nope", "sens": 9.0, "fps": 60, "auto": true});
        let p = VizPrefs::from_value(&v);
        assert_eq!(p.preset, "bars");
        assert_eq!(p.scheme, Scheme::Theme);
        assert_eq!(p.sens, 1.0);
        assert_eq!(p.fps, 60);
        assert!(p.auto);
        let round: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(VizPrefs::from_value(&round), p);
    }

    /// A busy, changing spectrum with a beat every few pictures.
    fn music(i: usize) -> [f32; BANDS] {
        let mut db = [-70.0f32; BANDS];
        for (k, d) in db.iter_mut().enumerate() {
            *d = -60.0 + 40.0 * ((i as f32 * 0.3 + k as f32 * 0.2).sin() * 0.5 + 0.5);
        }
        if i % 8 < 2 {
            db[..10].fill(-3.0);
        }
        db
    }

    #[test]
    fn every_look_draws_at_any_size() {
        for look in LOOKS {
            for (w, h) in [(1, 1), (37, 11), (640, 360)] {
                let mut v = Visualizer::default();
                let pal = Palette::new(Scheme::Rainbow, None, None);
                for i in 0..12 {
                    let db = if i < 6 { music(i).to_vec() } else { Vec::new() };
                    let a = v.analyser.feed(&db, 2.5, 0.05, 1.0);
                    v.render(look, w, h, &a, &pal);
                }
                let img = v.canvas.image();
                assert_eq!(img.size, [w, h], "{look:?}");
                assert!(
                    v.canvas.px.iter().flatten().all(|c| c.is_finite()),
                    "{look:?} {w}x{h}"
                );
            }
        }
    }

    #[test]
    fn looks_change_with_the_sound() {
        for look in LOOKS {
            let mut v = Visualizer::default();
            let pal = Palette::new(Scheme::Theme, None, None);
            for i in 0..20 {
                let a = v.analyser.feed(&music(i), 1.0, 1.0 / 30.0, 1.0);
                v.render(look, 160, 90, &a, &pal);
            }
            let lit = v
                .canvas
                .px
                .iter()
                .filter(|p| {
                    (p[0] - BACKDROP[0]).abs()
                        + (p[1] - BACKDROP[1]).abs()
                        + (p[2] - BACKDROP[2]).abs()
                        > 30.0
                })
                .count();
            assert!(lit > 20, "{look:?} drew {lit} pixels");
        }
    }

    /// `cargo test --release -p audiform2 visualizer_cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn visualizer_cost() {
        let (w, h) = (800, 400);
        for look in LOOKS {
            let mut v = Visualizer::default();
            let pal = Palette::new(Scheme::Theme, None, None);
            let frames = 300;
            let mut draw = Duration::ZERO;
            let mut upload = Duration::ZERO;
            for i in 0..frames + 30 {
                let a = v.analyser.feed(&music(i), 1.0, 1.0 / 60.0, 1.0);
                let t0 = Instant::now();
                v.render(look, w, h, &a, &pal);
                let t1 = Instant::now();
                let img = v.canvas.image();
                std::hint::black_box(&img);
                if i >= 30 {
                    draw += t1 - t0;
                    upload += t1.elapsed();
                }
            }
            println!(
                "{:<10} {w}x{h}: draw {:.3} ms, to image {:.3} ms per frame",
                look.name(),
                draw.as_secs_f64() * 1000.0 / frames as f64,
                upload.as_secs_f64() * 1000.0 / frames as f64
            );
        }
    }
}
