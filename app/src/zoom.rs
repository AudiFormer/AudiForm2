// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Zoom rules shared by the Waveform editor, its spectral display and the
//! Multitrack: the user's zoom preferences, the wheel's step and anchor,
//! the close-zoom limit, the Follow Playhead hand-off, and the spectral
//! display's frequency view (range, scale, analysis size).
//!
//! Everything here is plain math so it can be tested without a window.

use crate::state::View;

/// Where the plain wheel zooms around over the waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WheelAnchor {
    /// The time under the pointer keeps its place.
    Pointer,
    /// Halfway between the pointer and the middle of the view.
    BetweenPointerAndMiddle,
    /// The playhead keeps its place (the pointer when the playhead is out of view).
    Playhead,
}

// The Settings window (built on another branch) lists these.
#[allow(dead_code)]
impl WheelAnchor {
    pub const ALL: [WheelAnchor; 3] = [
        WheelAnchor::Pointer,
        WheelAnchor::BetweenPointerAndMiddle,
        WheelAnchor::Playhead,
    ];

    pub fn label(self) -> &'static str {
        match self {
            WheelAnchor::Pointer => "Around the pointer",
            WheelAnchor::BetweenPointerAndMiddle => "Between the pointer and the middle",
            WheelAnchor::Playhead => "Around the playhead",
        }
    }
}

/// The user's zoom preferences, saved as `zoom.json` in the data folder.
/// The Settings window reads and writes these fields directly and calls
/// [`ZoomPrefs::save`] when they change.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ZoomPrefs {
    /// What the plain wheel zooms around over the waveform (Ctrl+wheel
    /// too). The ruler and the overview always zoom toward the pointer.
    pub wheel_anchor: WheelAnchor,
    /// How much one wheel notch zooms, in percent (1 to 100).
    pub step_percent: f32,
    /// The plain wheel zooms over the Waveform editor's waveform and
    /// spectrum; off, it scrolls in time there (Ctrl+wheel still zooms).
    pub wheel_zooms_waveform: bool,
}

impl Default for ZoomPrefs {
    fn default() -> Self {
        ZoomPrefs {
            wheel_anchor: WheelAnchor::Pointer,
            step_percent: 25.0,
            wheel_zooms_waveform: true,
        }
    }
}

impl ZoomPrefs {
    pub fn path() -> std::path::PathBuf {
        crate::state::data_dir().join("zoom.json")
    }

    /// The saved preferences, or the defaults.
    pub fn load() -> ZoomPrefs {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str::<ZoomPrefs>(&s).ok())
            .map(ZoomPrefs::sanitized)
            .unwrap_or_default()
    }

    /// Called by the Settings window when a field changes.
    #[allow(dead_code)]
    pub fn save(&self) {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&p, serde_json::to_string_pretty(self).unwrap_or_default());
    }

    pub fn sanitized(mut self) -> ZoomPrefs {
        if !self.step_percent.is_finite() {
            self.step_percent = 25.0;
        }
        self.step_percent = self.step_percent.clamp(1.0, 100.0);
        self
    }

    /// The factor the view length is multiplied by for `points` of wheel
    /// (egui points; one notch is [`NOTCH`]). Wheel away from you
    /// (positive) zooms in.
    pub fn wheel_factor(&self, points: f32) -> f64 {
        wheel_factor(points, self.step_percent)
    }
}

/// egui's points per wheel notch on native (its `line_scroll_speed`).
pub const NOTCH: f32 = 40.0;

/// The most the view zooms in: 32 pixels per sample.
pub const MAX_PX_PER_SAMPLE: f64 = 32.0;

/// Display-only amplitude zoom limit.
pub const MAX_AMP: f32 = 16.0;

/// The length factor for `points` of wheel with a `step_percent` step:
/// each notch toward zooming in divides the length by `1 + step`.
pub fn wheel_factor(points: f32, step_percent: f32) -> f64 {
    let step = (step_percent.clamp(1.0, 100.0) / 100.0) as f64;
    (1.0 + step).powf(-(points / NOTCH) as f64)
}

/// Turns egui's Ctrl+wheel zoom factor back into wheel points, so
/// Ctrl+wheel zooms by the user's step like the plain wheel.
pub fn ctrl_points(zoom_delta: f32) -> f32 {
    // egui's default scroll_zoom_speed is 1/200.
    zoom_delta.max(1e-6).ln() * 200.0
}

/// The shortest view, in seconds, for `px` pixels at `rate` samples a
/// second: 32 pixels per sample.
pub fn min_len(px: f32, rate: f64) -> f64 {
    (px.max(1.0) as f64 / (MAX_PX_PER_SAMPLE * rate.max(1.0))).max(1e-7)
}

/// The time the wheel zooms around.
pub fn anchor_time(anchor: WheelAnchor, pointer: f64, view: &View, playhead: f64) -> f64 {
    match anchor {
        WheelAnchor::Pointer => pointer,
        WheelAnchor::BetweenPointerAndMiddle => (pointer + view.start + view.len / 2.0) / 2.0,
        WheelAnchor::Playhead => {
            if playhead >= view.start && playhead <= view.start + view.len {
                playhead
            } else {
                pointer
            }
        }
    }
}

/// The anchor for Zoom In / Zoom Out (buttons and Ctrl+= / Ctrl+-): the
/// playhead when it is in view, otherwise the middle of the view.
pub fn button_anchor(view: &View, playhead: f64) -> f64 {
    if playhead >= view.start && playhead <= view.start + view.len {
        playhead
    } else {
        view.start + view.len / 2.0
    }
}

/// Follow Playhead: on or off, and whether a hand-made scroll or zoom
/// during playback has let go of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Follow {
    pub on: bool,
    paused: bool,
}

impl Default for Follow {
    fn default() -> Self {
        Follow {
            on: true,
            paused: false,
        }
    }
}

impl Follow {
    /// The view was scrolled or zoomed by hand. During playback that
    /// stops the following until [`Follow::resume`].
    pub fn hand_moved(&mut self, playing: bool) {
        if playing {
            self.paused = true;
        }
    }

    /// Playback started again or the playhead was moved.
    pub fn resume(&mut self) {
        self.paused = false;
    }

    /// Switches following on or off; switching it on picks it up again.
    pub fn set_on(&mut self, on: bool) {
        self.on = on;
        self.paused = false;
    }

    /// Whether the view should follow the playhead now.
    pub fn active(&self) -> bool {
        self.on && !self.paused
    }
}

// ------------------------------------------------------- spectral view

/// The narrowest frequency range the spectral display zooms to.
pub const MIN_SPAN_HZ: f64 = 40.0;
/// Spectral resolution limits (analysis frame length in samples).
pub const FFT_MIN: usize = 512;
pub const FFT_MAX: usize = 16384;
pub const FFT_DEFAULT: usize = 2048;
/// Steps between Full Linear (0) and Full Logarithmic (this).
pub const LOG_STEPS: u8 = 4;

/// The spectral display's view: the frequency range shown, the scale and
/// the analysis size. It lasts for the run of the app, across files.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpecView {
    /// Shown range in Hz; None is the whole range, 0 Hz to Nyquist.
    pub range: Option<(f64, f64)>,
    /// 0 = linear .. [`LOG_STEPS`] = logarithmic.
    pub log_step: u8,
    /// Note names on the scale instead of Hertz.
    pub notes: bool,
    /// Analysis frame length in samples.
    pub fft: usize,
}

impl Default for SpecView {
    fn default() -> Self {
        SpecView {
            range: None,
            log_step: LOG_STEPS,
            notes: false,
            fft: FFT_DEFAULT,
        }
    }
}

/// The offset (Hz) of the log warp `ln(f + c)` for each scale step; a
/// big offset is nearly linear, a small one nearly logarithmic (and keeps
/// 0 Hz on the scale). Step 0 is exactly linear.
fn warp_offset(step: u8) -> f64 {
    match step {
        0 => f64::INFINITY,
        1 => 4000.0,
        2 => 800.0,
        3 => 150.0,
        _ => 20.0,
    }
}

fn warp(f: f64, step: u8) -> f64 {
    let c = warp_offset(step);
    if c.is_infinite() {
        f
    } else {
        (f + c).ln()
    }
}

fn unwarp(g: f64, step: u8) -> f64 {
    let c = warp_offset(step);
    if c.is_infinite() {
        g
    } else {
        g.exp() - c
    }
}

impl SpecView {
    /// The range shown for a file whose Nyquist frequency is `ny`.
    pub fn bounds(&self, ny: f64) -> (f64, f64) {
        let ny = ny.max(MIN_SPAN_HZ);
        match self.range {
            None => (0.0, ny),
            Some((lo, hi)) => clamp_range(lo, hi, ny),
        }
    }

    /// Where `f` sits on the scale: 0 at the bottom, 1 at the top.
    pub fn pos(&self, f: f64, ny: f64) -> f64 {
        let (lo, hi) = self.bounds(ny);
        let (a, b) = (warp(lo, self.log_step), warp(hi, self.log_step));
        (warp(f.max(0.0), self.log_step) - a) / (b - a)
    }

    /// The frequency at `pos` (0 bottom, 1 top).
    pub fn freq_at(&self, pos: f64, ny: f64) -> f64 {
        let (lo, hi) = self.bounds(ny);
        let (a, b) = (warp(lo, self.log_step), warp(hi, self.log_step));
        unwarp(a + (b - a) * pos, self.log_step)
    }

    /// Zooms the frequencies by `factor` (below 1 zooms in), keeping
    /// `anchor_hz` at its place on the scale, within 0 Hz..`ny` and never
    /// narrower than [`MIN_SPAN_HZ`].
    pub fn zoom(&mut self, factor: f64, anchor_hz: f64, ny: f64) {
        let ny = ny.max(MIN_SPAN_HZ);
        let (lo, hi) = self.bounds(ny);
        let s = self.log_step;
        let anchor_hz = anchor_hz.clamp(lo, hi);
        let (a, b, g) = (warp(lo, s), warp(hi, s), warp(anchor_hz, s));
        let factor = factor.max(1e-6);
        let mut nlo = unwarp(g - (g - a) * factor, s);
        let mut nhi = unwarp(g + (b - g) * factor, s);
        if nhi - nlo < MIN_SPAN_HZ {
            // Too narrow: the narrowest span, around the anchor, at the
            // anchor's share of the old range.
            let frac = if hi > lo {
                (anchor_hz - lo) / (hi - lo)
            } else {
                0.5
            };
            nlo = anchor_hz - MIN_SPAN_HZ * frac;
            nhi = nlo + MIN_SPAN_HZ;
        }
        let (nlo, nhi) = clamp_range(nlo, nhi, ny);
        self.range = if nlo <= 0.0 && nhi >= ny {
            None
        } else {
            Some((nlo, nhi))
        };
    }

    /// Zoom In / Zoom Out from the menu: around the middle of the scale.
    pub fn zoom_step(&mut self, zoom_in: bool, ny: f64) {
        let mid = self.freq_at(0.5, ny);
        self.zoom(if zoom_in { 0.5 } else { 2.0 }, mid, ny);
    }

    pub fn reset_zoom(&mut self) {
        self.range = None;
    }

    pub fn more_log(&mut self) {
        self.log_step = (self.log_step + 1).min(LOG_STEPS);
    }

    pub fn more_linear(&mut self) {
        self.log_step = self.log_step.saturating_sub(1);
    }

    pub fn finer(&mut self) {
        self.fft = (self.fft * 2).min(FFT_MAX);
    }

    pub fn coarser(&mut self) {
        self.fft = (self.fft / 2).max(FFT_MIN);
    }

    /// Scale labels for a scale `px` pixels tall: (frequency, label),
    /// at least `gap` pixels apart.
    pub fn ticks(&self, ny: f64, px: f32, gap: f32) -> Vec<(f64, String)> {
        let (lo, hi) = self.bounds(ny);
        let mut cands: Vec<(f64, String)> = Vec::new();
        if self.notes {
            // Every semitone; the filter keeps what fits, C first.
            let first = (hz_to_midi(lo.max(8.0))).ceil() as i32;
            let last = (hz_to_midi(hi.max(8.0))).floor() as i32;
            let mut cs = Vec::new();
            let mut rest = Vec::new();
            for m in first..=last.min(first + 400) {
                let f = midi_to_hz(m as f64);
                if m.rem_euclid(12) == 0 {
                    cs.push((f, note_name(m)));
                } else {
                    rest.push((f, note_name(m)));
                }
            }
            cands.extend(cs);
            cands.extend(rest);
        } else {
            // 1-2-5 decades (for the log end of the scale) and an even
            // step across the range (for the linear end).
            let mut dec = Vec::new();
            let mut p = 10.0;
            while p <= hi * 1.001 {
                for m in [1.0, 2.0, 5.0] {
                    let f = p * m;
                    if f >= lo && f <= hi {
                        dec.push(f);
                    }
                }
                p *= 10.0;
            }
            let step = nice_step((hi - lo) / 8.0);
            let mut lin = Vec::new();
            let mut f = (lo / step).ceil() * step;
            while f <= hi && lin.len() < 200 {
                lin.push(f);
                f += step;
            }
            for f in dec.into_iter().chain(lin) {
                cands.push((f, hz_label(f)));
            }
        }
        let mut kept: Vec<(f64, String, f32)> = Vec::new();
        for (f, label) in cands {
            let y = self.pos(f, ny) as f32 * px;
            if !(0.0..=px).contains(&y) {
                continue;
            }
            if kept.iter().all(|k| (k.2 - y).abs() >= gap) {
                kept.push((f, label, y));
            }
        }
        kept.sort_by(|a, b| a.0.total_cmp(&b.0));
        kept.into_iter().map(|(f, l, _)| (f, l)).collect()
    }
}

/// Keeps a frequency range inside 0..`ny` and at least [`MIN_SPAN_HZ`]
/// wide, shifting it rather than squeezing it where it can.
pub fn clamp_range(lo: f64, hi: f64, ny: f64) -> (f64, f64) {
    let ny = ny.max(MIN_SPAN_HZ);
    let (mut lo, mut hi) = if lo.is_finite() && hi.is_finite() && hi > lo {
        (lo, hi)
    } else {
        (0.0, ny)
    };
    if hi - lo < MIN_SPAN_HZ {
        let mid = (lo + hi) / 2.0;
        lo = mid - MIN_SPAN_HZ / 2.0;
        hi = mid + MIN_SPAN_HZ / 2.0;
    }
    if lo < 0.0 {
        hi -= lo;
        lo = 0.0;
    }
    if hi > ny {
        lo -= hi - ny;
        hi = ny;
    }
    (lo.max(0.0), hi.min(ny))
}

fn nice_step(x: f64) -> f64 {
    let x = x.max(1e-3);
    let p = 10f64.powf(x.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if p * m >= x {
            return p * m;
        }
    }
    p * 10.0
}

pub fn hz_label(f: f64) -> String {
    if f >= 1000.0 {
        let k = f / 1000.0;
        if (k - k.round()).abs() < 1e-6 {
            format!("{}k", k.round())
        } else {
            format!("{:.1}k", k)
        }
    } else if (f - f.round()).abs() < 1e-6 {
        format!("{}", f.round())
    } else {
        format!("{:.1}", f)
    }
}

pub fn hz_to_midi(f: f64) -> f64 {
    69.0 + 12.0 * (f / 440.0).log2()
}

pub fn midi_to_hz(m: f64) -> f64 {
    440.0 * 2f64.powf((m - 69.0) / 12.0)
}

pub fn note_name(m: i32) -> String {
    const N: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", N[m.rem_euclid(12) as usize], m.div_euclid(12) - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_steps_by_the_percent_setting() {
        let p = ZoomPrefs::default();
        // One notch in: the length shrinks by 1/1.25; one notch out grows by 1.25.
        assert!((p.wheel_factor(NOTCH) - 1.0 / 1.25).abs() < 1e-9);
        assert!((p.wheel_factor(-NOTCH) - 1.25).abs() < 1e-9);
        // Spread over frames, the parts multiply to one notch.
        let f: f64 = (0..4).map(|_| p.wheel_factor(NOTCH / 4.0)).product();
        assert!((f - 0.8).abs() < 1e-9);
        // Ctrl+wheel's egui factor maps back to the same points.
        let z = (NOTCH / 200.0).exp();
        assert!((ctrl_points(z) - NOTCH).abs() < 1e-3);
    }

    #[test]
    fn anchors() {
        let v = View {
            start: 10.0,
            len: 4.0,
        };
        assert_eq!(anchor_time(WheelAnchor::Pointer, 11.0, &v, 13.0), 11.0);
        assert_eq!(
            anchor_time(WheelAnchor::BetweenPointerAndMiddle, 11.0, &v, 13.0),
            11.5
        );
        assert_eq!(anchor_time(WheelAnchor::Playhead, 11.0, &v, 13.0), 13.0);
        // Playhead out of view: the pointer.
        assert_eq!(anchor_time(WheelAnchor::Playhead, 11.0, &v, 20.0), 11.0);
        assert_eq!(button_anchor(&v, 13.0), 13.0);
        assert_eq!(button_anchor(&v, 2.0), 12.0);
    }

    #[test]
    fn zooming_keeps_the_anchor_in_place_and_stops_at_32_px_per_sample() {
        let mut v = View {
            start: 10.0,
            len: 4.0,
        };
        let t = 11.0;
        let frac = (t - v.start) / v.len;
        v.zoom_min(0.5, t, 100.0, 0.0);
        assert!(((t - v.start) / v.len - frac).abs() < 1e-12);
        assert!((v.len - 2.0).abs() < 1e-12);
        let min = min_len(1000.0, 48_000.0);
        assert!((min * 48_000.0 - 1000.0 / 32.0).abs() < 1e-9);
        for _ in 0..100 {
            v.zoom_min(0.5, t, 100.0, min);
        }
        assert!((v.len - min).abs() < 1e-12);
        // Zooming out stops at the whole file.
        for _ in 0..100 {
            v.zoom_min(2.0, t, 100.0, min);
        }
        assert_eq!((v.start, v.len), (0.0, 100.0));
    }

    #[test]
    fn follow_lets_go_until_resumed() {
        let mut f = Follow::default();
        assert!(f.active());
        // A hand move while stopped does not stop following.
        f.hand_moved(false);
        assert!(f.active());
        f.hand_moved(true);
        assert!(!f.active());
        f.resume();
        assert!(f.active());
        f.hand_moved(true);
        f.set_on(false);
        assert!(!f.active());
        f.set_on(true);
        assert!(f.active());
    }

    #[test]
    fn frequency_zoom_limits() {
        let ny = 22_050.0;
        let mut s = SpecView::default();
        assert_eq!(s.bounds(ny), (0.0, ny));
        // Zoom in a lot: never narrower than 40 Hz, inside 0..Nyquist.
        for _ in 0..60 {
            s.zoom(0.5, 1000.0, ny);
            let (lo, hi) = s.bounds(ny);
            assert!(lo >= 0.0 && hi <= ny && hi - lo >= MIN_SPAN_HZ - 1e-9);
        }
        let (lo, hi) = s.bounds(ny);
        assert!((hi - lo - MIN_SPAN_HZ).abs() < 1e-6, "{lo} {hi}");
        assert!(lo <= 1000.0 && hi >= 1000.0);
        // Out all the way: back to the full range, never past it.
        for _ in 0..60 {
            s.zoom(2.0, 1000.0, ny);
        }
        assert_eq!(s.range, None);
        assert_eq!(s.bounds(ny), (0.0, ny));
        // Near the bottom and the top the range stays inside.
        s.zoom(0.001, 1.0, ny);
        let (lo, hi) = s.bounds(ny);
        assert!(lo >= 0.0 && (hi - lo - MIN_SPAN_HZ).abs() < 1e-6);
        s.reset_zoom();
        s.zoom(0.001, ny, ny);
        let (lo, hi) = s.bounds(ny);
        assert!(hi <= ny && (hi - ny).abs() < 1e-6 && hi - lo >= MIN_SPAN_HZ - 1e-6);
    }

    #[test]
    fn frequency_zoom_keeps_the_pointer_frequency_in_place() {
        let ny = 24_000.0;
        for step in 0..=LOG_STEPS {
            let mut s = SpecView {
                log_step: step,
                ..SpecView::default()
            };
            s.zoom(0.5, 3000.0, ny);
            let f = 3000.0;
            let before = s.pos(f, ny);
            s.zoom(0.7, f, ny);
            let after = s.pos(f, ny);
            assert!(
                (before - after).abs() < 1e-9,
                "step {step}: {before} {after}"
            );
            // pos and freq_at are inverses.
            for p in [0.0, 0.25, 0.5, 1.0] {
                assert!((s.pos(s.freq_at(p, ny), ny) - p).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn scale_steps_and_resolution_limits() {
        let mut s = SpecView::default();
        assert_eq!(s.log_step, LOG_STEPS);
        s.more_log();
        assert_eq!(s.log_step, LOG_STEPS);
        for _ in 0..10 {
            s.more_linear();
        }
        assert_eq!(s.log_step, 0);
        assert_eq!(s.fft, 2048);
        for _ in 0..10 {
            s.finer();
        }
        assert_eq!(s.fft, FFT_MAX);
        for _ in 0..10 {
            s.coarser();
        }
        assert_eq!(s.fft, FFT_MIN);
    }

    #[test]
    fn ticks_and_notes() {
        assert_eq!(note_name(69), "A4");
        assert_eq!(note_name(60), "C4");
        assert!((midi_to_hz(hz_to_midi(1234.0)) - 1234.0).abs() < 1e-9);
        let s = SpecView::default();
        let t = s.ticks(22_050.0, 300.0, 16.0);
        assert!(!t.is_empty());
        assert!(t.windows(2).all(|w| w[0].0 < w[1].0));
        let n = SpecView {
            notes: true,
            ..SpecView::default()
        };
        assert!(n.ticks(22_050.0, 300.0, 16.0).iter().any(|x| x.1 == "C4"));
    }
}
