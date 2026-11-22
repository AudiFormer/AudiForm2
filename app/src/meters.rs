// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Level meters: how fast they rise and fall, what they show, and the
//! Levels panel's bar.
//!
//! The engine hands over the loudest sample (and the mean square) since
//! the last frame. Each frame the shown level jumps up to a new peak at
//! once and otherwise falls at a steady number of dB a second, measured
//! with the frame's real duration, so the meter moves the same at 60 and
//! at 240 frames a second. RMS is the mean square averaged over a short
//! window (again in seconds, not frames). A peak-hold line sits at the
//! highest recent level, waits, then falls.

use egui::{pos2, vec2, Align2, Color32, FontId, Mesh, Painter, Rect, Rounding, Stroke};
use serde::{Deserialize, Serialize};

use crate::theme::{c, ca, Theme};

/// The quietest level a meter tracks; anything below reads as this.
pub const FLOOR_DB: f32 = -120.0;

/// The ranges the meter's menu offers (dB below full scale).
pub const RANGES: [f32; 5] = [96.0, 72.0, 60.0, 48.0, 24.0];

/// How quickly a meter falls back after a peak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Response {
    Fast,
    #[default]
    Normal,
    Slow,
}

impl Response {
    pub const ALL: [Response; 3] = [Response::Fast, Response::Normal, Response::Slow];

    /// How fast the shown peak falls, dB a second.
    pub fn fall_db_per_s(self) -> f32 {
        match self {
            Response::Fast => 40.0,
            Response::Normal => 20.0,
            Response::Slow => 8.0,
        }
    }

    /// The RMS averaging window, seconds.
    pub fn rms_window(self) -> f32 {
        match self {
            Response::Fast => 0.15,
            Response::Normal => 0.3,
            Response::Slow => 0.6,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Response::Fast => "Fast (40 dB/s)",
            Response::Normal => "Normal (20 dB/s)",
            Response::Slow => "Slow (8 dB/s)",
        }
    }
}

/// How long the peak-hold line stays before it falls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PeakHold {
    Off,
    #[default]
    Short,
    Long,
    /// Until clicked.
    Infinite,
}

impl PeakHold {
    pub const ALL: [PeakHold; 4] = [
        PeakHold::Off,
        PeakHold::Short,
        PeakHold::Long,
        PeakHold::Infinite,
    ];

    /// Seconds held; None for Off and Infinite.
    pub fn secs(self) -> Option<f32> {
        match self {
            PeakHold::Short => Some(1.5),
            PeakHold::Long => Some(3.0),
            PeakHold::Off | PeakHold::Infinite => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PeakHold::Off => "Off",
            PeakHold::Short => "1.5 s",
            PeakHold::Long => "3 s",
            PeakHold::Infinite => "Infinite (click the meter to reset)",
        }
    }
}

/// What the bar measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MeterKind {
    #[default]
    Peak,
    Rms,
    /// RMS as the solid bar, the peak as a thinner, lighter one.
    PeakRms,
}

impl MeterKind {
    pub const ALL: [MeterKind; 3] = [MeterKind::Peak, MeterKind::Rms, MeterKind::PeakRms];

    pub fn label(self) -> &'static str {
        match self {
            MeterKind::Peak => "Peak",
            MeterKind::Rms => "RMS",
            MeterKind::PeakRms => "Peak + RMS",
        }
    }
}

/// The level meters' settings (the Levels panel's right-click menu).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MeterPrefs {
    /// dB below full scale at the meter's left end.
    pub range_db: f32,
    pub response: Response,
    pub hold: PeakHold,
    pub kind: MeterKind,
    /// A red latch at the right end once a channel reaches full scale.
    pub clip: bool,
}

impl Default for MeterPrefs {
    fn default() -> Self {
        MeterPrefs {
            range_db: 60.0,
            response: Response::Normal,
            hold: PeakHold::Short,
            kind: MeterKind::Peak,
            clip: true,
        }
    }
}

impl MeterPrefs {
    /// The level at the meter's left end (a hand-edited range is kept sane).
    pub fn floor(&self) -> f32 {
        let r = if self.range_db.is_finite() {
            self.range_db.abs().clamp(12.0, 120.0)
        } else {
            60.0
        };
        -r
    }
}

pub fn lin_to_db(x: f32) -> f32 {
    if x > 0.0 {
        (20.0 * x.log10()).max(FLOOR_DB)
    } else {
        FLOOR_DB
    }
}

pub fn db_to_lin(db: f32) -> f32 {
    if db <= FLOOR_DB {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// A plain peak meter's fall: the larger of the fresh peak and the shown
/// level fallen by `db_per_s` over `dt` seconds (both linear). For meters
/// that keep only a number.
pub fn fall_lin(shown: f32, fresh: f32, dt: f32, db_per_s: f32) -> f32 {
    let dt = dt.clamp(0.0, 0.5);
    let fallen = shown * 10f32.powf(-db_per_s * dt / 20.0);
    let v = fresh.max(fallen);
    if v < 1e-6 {
        0.0
    } else {
        v
    }
}

/// One channel of a meter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Channel {
    /// The shown peak level, dB.
    pub peak_db: f32,
    /// The averaged mean square (linear power).
    pub ms: f32,
    /// Time since the mean square was last fed, seconds.
    ms_wait: f32,
    /// The peak-hold line, dB.
    pub hold_db: f32,
    /// How long the hold line has stayed put, seconds.
    hold_age: f32,
    /// Reached full scale since the latch was last cleared.
    pub clipped: bool,
}

impl Default for Channel {
    fn default() -> Self {
        Channel {
            peak_db: FLOOR_DB,
            ms: 0.0,
            ms_wait: 0.0,
            hold_db: FLOOR_DB,
            hold_age: 0.0,
            clipped: false,
        }
    }
}

impl Channel {
    /// The shown RMS level, dB.
    pub fn rms_db(&self) -> f32 {
        if self.ms > 0.0 {
            (10.0 * self.ms.log10()).max(FLOOR_DB)
        } else {
            FLOOR_DB
        }
    }

    /// The level the bar reaches for this meter type (the hold line
    /// follows it).
    pub fn top_db(&self, kind: MeterKind) -> f32 {
        match kind {
            MeterKind::Rms => self.rms_db(),
            MeterKind::Peak | MeterKind::PeakRms => self.peak_db,
        }
    }

    /// Moves the meter on by `dt` seconds. `peak` is the loudest sample
    /// (linear) since the last step; `ms` the mean square since then, or
    /// None when no audio came through in between.
    pub fn step(&mut self, peak: f32, ms: Option<f32>, dt: f32, p: &MeterPrefs) {
        let dt = if dt.is_finite() {
            dt.clamp(0.0, 0.5)
        } else {
            0.0
        };
        let fall = p.response.fall_db_per_s() * dt;
        // Up at once, down at a steady rate.
        self.peak_db = lin_to_db(peak).max(self.peak_db - fall).max(FLOOR_DB);
        // RMS: a running average over the window, in seconds. Frames with
        // no new audio wait, and the next reading makes up for the time.
        self.ms_wait += dt;
        if let Some(ms) = ms {
            let ms = if ms.is_finite() { ms.max(0.0) } else { 0.0 };
            let a = 1.0 - (-self.ms_wait / p.response.rms_window()).exp();
            self.ms += (ms - self.ms) * a;
            if self.ms < 1e-12 {
                self.ms = 0.0;
            }
            self.ms_wait = 0.0;
        }
        if peak >= 1.0 {
            self.clipped = true;
        }
        let top = self.top_db(p.kind);
        match p.hold {
            PeakHold::Off => {
                self.hold_db = top;
                self.hold_age = 0.0;
            }
            PeakHold::Infinite => {
                if top >= self.hold_db {
                    self.hold_db = top;
                    self.hold_age = 0.0;
                }
            }
            PeakHold::Short | PeakHold::Long => {
                let wait = p.hold.secs().unwrap_or(0.0);
                if top >= self.hold_db {
                    self.hold_db = top;
                    self.hold_age = 0.0;
                } else {
                    self.hold_age += dt;
                    let over = (self.hold_age - wait).clamp(0.0, dt);
                    if over > 0.0 {
                        self.hold_db = (self.hold_db - p.response.fall_db_per_s() * over).max(top);
                    }
                }
            }
        }
    }

    pub fn reset_hold(&mut self, kind: MeterKind) {
        self.hold_db = self.top_db(kind);
        self.hold_age = 0.0;
    }
}

/// The main output meter: left and right.
#[derive(Debug, Clone, Default)]
pub struct Stereo {
    pub ch: [Channel; 2],
}

impl Stereo {
    pub fn step(&mut self, peaks: (f32, f32), ms: Option<(f32, f32)>, dt: f32, p: &MeterPrefs) {
        self.ch[0].step(peaks.0, ms.map(|m| m.0), dt, p);
        self.ch[1].step(peaks.1, ms.map(|m| m.1), dt, p);
    }

    /// The shown peaks, linear (what other readouts follow).
    pub fn peaks(&self) -> (f32, f32) {
        (db_to_lin(self.ch[0].peak_db), db_to_lin(self.ch[1].peak_db))
    }

    /// Still moving on screen: worth drawing another frame.
    pub fn active(&self, p: &MeterPrefs) -> bool {
        let floor = p.floor();
        self.ch.iter().any(|c| {
            c.peak_db > floor
                || c.rms_db() > floor
                || (p.hold != PeakHold::Infinite && c.hold_db > floor)
        })
    }

    pub fn reset_holds(&mut self, kind: MeterKind) {
        for c in &mut self.ch {
            c.reset_hold(kind);
        }
    }

    pub fn clear_clips(&mut self) {
        for c in &mut self.ch {
            c.clipped = false;
        }
    }
}

// ------------------------------------------------------------- the look

/// The meter's colour at `db`: the theme's low colour, blending into its
/// middle colour toward -6 dB, and its high colour above -3 dB.
pub fn color_at(th: &Theme, db: f32) -> Color32 {
    if db > -3.0 {
        c(th.meter_high)
    } else if db > -18.0 {
        crate::theme::lerp(th.meter_low, th.meter_mid, (db + 18.0) / 12.0)
    } else {
        c(th.meter_low)
    }
}

/// A solid gradient bar in `r` from its left (`floor` dB) to `db`, `full`
/// at the right end being 0 dB. `alpha` 255 is opaque.
pub fn gradient_bar(p: &Painter, r: Rect, floor: f32, db: f32, th: &Theme, alpha: u8) {
    if db <= floor || r.width() <= 0.0 {
        return;
    }
    let x_of = |d: f32| r.left() + ((d - floor) / -floor).clamp(0.0, 1.0) * r.width();
    let end = x_of(db);
    // Colour stops: flat low, blend to the middle colour by -6, hard step
    // to the high colour at -3.
    let mut stops: Vec<(f32, Color32)> = vec![(r.left(), color_at(th, floor))];
    for d in [-18.0f32, -6.0, -3.0] {
        if d > floor {
            stops.push((x_of(d), color_at(th, d - 0.001)));
            if d == -3.0 {
                stops.push((x_of(d), c(th.meter_high)));
            }
        }
    }
    stops.push((r.right(), c(th.meter_high)));
    let fade = |col: Color32| {
        if alpha == 255 {
            col
        } else {
            Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha)
        }
    };
    let mut m = Mesh::default();
    for w in stops.windows(2) {
        let ((x0, c0), (x1, c1)) = (w[0], w[1]);
        if x0 >= end {
            break;
        }
        let x1c = x1.min(end);
        if x1c <= x0 {
            continue;
        }
        let t = if x1 > x0 { (x1c - x0) / (x1 - x0) } else { 1.0 };
        let c1c = lerp_color(c0, c1, t);
        let i = m.vertices.len() as u32;
        m.colored_vertex(pos2(x0, r.top()), fade(c0));
        m.colored_vertex(pos2(x1c, r.top()), fade(c1c));
        m.colored_vertex(pos2(x1c, r.bottom()), fade(c1c));
        m.colored_vertex(pos2(x0, r.bottom()), fade(c0));
        m.add_triangle(i, i + 1, i + 2);
        m.add_triangle(i, i + 2, i + 3);
    }
    p.add(egui::Shape::mesh(m));
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// Draws one channel's bar in `r` (its background, level, hold line).
pub fn draw_channel(p: &Painter, r: Rect, ch: &Channel, prefs: &MeterPrefs, th: &Theme) {
    let floor = prefs.floor();
    p.rect_filled(r, Rounding::same(2.0), c(th.wave_background));
    let x_of = |d: f32| r.left() + ((d - floor) / -floor).clamp(0.0, 1.0) * r.width();
    match prefs.kind {
        MeterKind::Peak => gradient_bar(p, r, floor, ch.peak_db, th, 255),
        MeterKind::Rms => gradient_bar(p, r, floor, ch.rms_db(), th, 255),
        MeterKind::PeakRms => {
            let inset = (r.height() * 0.25).floor();
            let thin = Rect::from_min_max(
                pos2(r.left(), r.top() + inset),
                pos2(r.right(), r.bottom() - inset),
            );
            gradient_bar(p, thin, floor, ch.peak_db, th, 130);
            gradient_bar(p, r, floor, ch.rms_db(), th, 255);
        }
    }
    if prefs.hold != PeakHold::Off && ch.hold_db > floor {
        let hx = x_of(ch.hold_db).max(r.left() + 1.0).min(r.right() - 1.0);
        p.line_segment(
            [pos2(hx, r.top()), pos2(hx, r.bottom())],
            Stroke::new(2.0_f32, color_at(th, ch.hold_db)),
        );
    }
}

/// The spacing between labelled ticks for a range drawn `width` wide.
pub fn tick_step(range: f32, width: f32) -> f32 {
    for step in [1.0f32, 2.0, 3.0, 6.0, 12.0, 24.0, 48.0] {
        if width / (range / step) >= 36.0 {
            return step;
        }
    }
    96.0
}

/// The scale in the strip between the two bars: tick marks running in
/// from both bars' edges and the dB labels centred between them. `strip`
/// spans the bars' width and the gap between them.
pub fn draw_scale(p: &Painter, strip: Rect, floor: f32, th: &Theme) {
    let step = tick_step(-floor, strip.width());
    let x_of = |d: f32| strip.left() + ((d - floor) / -floor).clamp(0.0, 1.0) * strip.width();
    let tick = Stroke::new(1.0_f32, ca(th.text_dim, 150));
    let mut d = 0.0f32;
    while d >= floor - 0.01 {
        let x = x_of(d).round() + 0.5;
        p.line_segment([pos2(x, strip.top()), pos2(x, strip.top() + 2.0)], tick);
        p.line_segment(
            [pos2(x, strip.bottom() - 2.0), pos2(x, strip.bottom())],
            tick,
        );
        let label = format!("{}", d as i32);
        // Keep the end labels inside the bars' span.
        let align = if x - strip.left() < 6.0 {
            Align2::LEFT_CENTER
        } else if strip.right() - x < 6.0 {
            Align2::RIGHT_CENTER
        } else {
            Align2::CENTER_CENTER
        };
        p.text(
            pos2(x, strip.center().y),
            align,
            label,
            FontId::proportional(9.0),
            c(th.text_dim),
        );
        d -= step;
    }
}

/// The clip latch beside a bar.
pub fn clip_latch(bar: Rect) -> Rect {
    Rect::from_min_size(pos2(bar.right() + 3.0, bar.top()), vec2(7.0, bar.height()))
}

pub fn draw_clip_latch(p: &Painter, r: Rect, on: bool, th: &Theme) {
    if on {
        p.rect_filled(r, Rounding::same(1.5), c(th.meter_high));
    } else {
        p.rect_filled(r, Rounding::same(1.5), ca(th.meter_high, 38));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs() -> MeterPrefs {
        MeterPrefs::default()
    }

    /// Steps a channel through `secs` of audio at `fps`, the audio given as
    /// a peak (linear) for each moment.
    fn run(ch: &mut Channel, fps: f32, secs: f32, p: &MeterPrefs, level: impl Fn(f32) -> f32) {
        let dt = 1.0 / fps;
        let n = (secs * fps).round() as usize;
        for i in 0..n {
            let t = i as f32 * dt;
            let pk = level(t);
            ch.step(pk, Some(pk * pk * 0.5), dt, p);
        }
    }

    #[test]
    fn a_peak_shows_at_once() {
        let mut ch = Channel::default();
        ch.step(0.5, Some(0.125), 1.0 / 60.0, &prefs());
        assert!((ch.peak_db - lin_to_db(0.5)).abs() < 1e-4, "{}", ch.peak_db);
    }

    #[test]
    fn it_falls_at_the_responses_rate() {
        for r in Response::ALL {
            let p = MeterPrefs {
                response: r,
                ..prefs()
            };
            let mut ch = Channel::default();
            ch.step(1.0, None, 0.01, &p);
            assert!(ch.peak_db.abs() < 1e-4);
            // Half a second of silence.
            run(&mut ch, 120.0, 0.5, &p, |_| 0.0);
            let want = -r.fall_db_per_s() * 0.5;
            assert!(
                (ch.peak_db - want).abs() < 0.2,
                "{r:?}: {} vs {want}",
                ch.peak_db
            );
        }
    }

    #[test]
    fn the_hold_line_waits_then_falls() {
        let p = MeterPrefs {
            hold: PeakHold::Short,
            ..prefs()
        };
        let mut ch = Channel::default();
        ch.step(1.0, None, 0.01, &p);
        // Still held just before 1.5 s.
        run(&mut ch, 100.0, 1.4, &p, |_| 0.0);
        assert!(ch.hold_db.abs() < 1e-4, "{}", ch.hold_db);
        assert!(ch.peak_db < -20.0);
        // Half a second after the wait it has fallen by about the rate.
        run(&mut ch, 100.0, 0.6, &p, |_| 0.0);
        let fallen = -ch.hold_db;
        let want = p.response.fall_db_per_s() * 0.5;
        assert!((fallen - want).abs() < 0.5, "{fallen} vs {want}");
        // Never below the bar itself.
        assert!(ch.hold_db >= ch.peak_db);
    }

    #[test]
    fn an_infinite_hold_stays_until_reset() {
        let p = MeterPrefs {
            hold: PeakHold::Infinite,
            ..prefs()
        };
        let mut ch = Channel::default();
        ch.step(0.5, None, 0.01, &p);
        run(&mut ch, 60.0, 10.0, &p, |_| 0.0);
        assert!((ch.hold_db - lin_to_db(0.5)).abs() < 1e-4);
        ch.reset_hold(p.kind);
        assert!(ch.hold_db <= p.floor());
    }

    #[test]
    fn hold_off_follows_the_bar() {
        let p = MeterPrefs {
            hold: PeakHold::Off,
            ..prefs()
        };
        let mut ch = Channel::default();
        ch.step(1.0, None, 0.01, &p);
        run(&mut ch, 60.0, 0.5, &p, |_| 0.0);
        assert_eq!(ch.hold_db, ch.peak_db);
    }

    #[test]
    fn sixty_and_two_hundred_forty_frames_a_second_look_the_same() {
        let p = MeterPrefs {
            kind: MeterKind::PeakRms,
            ..prefs()
        };
        // Bursts: loud for 50 ms every 400 ms, quiet between.
        let level = |t: f32| if (t % 0.4) < 0.05 { 0.8 } else { 0.01 };
        let (mut a, mut b) = (Channel::default(), Channel::default());
        for k in 1..=12 {
            run(&mut a, 60.0, 0.25, &p, level);
            run(&mut b, 240.0, 0.25, &p, level);
            let t = k as f32 * 0.25;
            assert!(
                (a.peak_db - b.peak_db).abs() < 0.8,
                "peak at {t}: {} vs {}",
                a.peak_db,
                b.peak_db
            );
            assert!(
                (a.rms_db() - b.rms_db()).abs() < 1.0,
                "rms at {t}: {} vs {}",
                a.rms_db(),
                b.rms_db()
            );
            assert!((a.hold_db - b.hold_db).abs() < 0.8, "hold at {t}");
        }
    }

    #[test]
    fn rms_settles_on_the_signals_power_and_waits_out_empty_frames() {
        let p = prefs();
        // A full-scale sine: power 0.5, RMS about -3 dB.
        let mut a = Channel::default();
        for _ in 0..240 {
            a.step(1.0, Some(0.5), 1.0 / 60.0, &p);
        }
        assert!((a.rms_db() - (-3.01)).abs() < 0.05, "{}", a.rms_db());
        // Fed only every third frame (audio blocks longer than a frame):
        // the same as being fed every frame.
        let (mut x, mut y) = (Channel::default(), Channel::default());
        for i in 0..90 {
            x.step(0.5, Some(0.1), 1.0 / 240.0, &p);
            let fed = (i % 3 == 2).then_some(0.1);
            y.step(0.5, fed, 1.0 / 240.0, &p);
            if i % 3 == 2 {
                assert!((x.ms - y.ms).abs() < 1e-6, "{} vs {}", x.ms, y.ms);
            }
        }
    }

    #[test]
    fn full_scale_latches_the_clip_light() {
        let mut s = Stereo::default();
        s.step((0.5, 1.0), None, 0.01, &prefs());
        assert!(!s.ch[0].clipped && s.ch[1].clipped);
        s.step((0.0, 0.0), None, 5.0, &prefs());
        assert!(s.ch[1].clipped);
        s.clear_clips();
        assert!(!s.ch[1].clipped);
    }

    #[test]
    fn old_prefs_files_still_load() {
        let p: MeterPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(p, MeterPrefs::default());
        assert_eq!(p.floor(), -60.0);
        let odd = MeterPrefs {
            range_db: f32::NAN,
            ..prefs()
        };
        assert_eq!(odd.floor(), -60.0);
    }

    #[test]
    fn ticks_stay_readable() {
        assert_eq!(tick_step(60.0, 2400.0), 1.0);
        assert_eq!(tick_step(60.0, 1600.0), 2.0);
        assert_eq!(tick_step(60.0, 800.0), 3.0);
        assert_eq!(tick_step(96.0, 600.0), 6.0);
    }

    #[test]
    fn plain_meters_fall_by_time_not_frames() {
        let mut a = 1.0f32;
        let mut b = 1.0f32;
        for _ in 0..60 {
            a = fall_lin(a, 0.0, 1.0 / 60.0, 20.0);
        }
        for _ in 0..240 {
            b = fall_lin(b, 0.0, 1.0 / 240.0, 20.0);
        }
        assert!((lin_to_db(a) - -20.0).abs() < 0.01);
        assert!((lin_to_db(a) - lin_to_db(b)).abs() < 0.01);
    }
}
