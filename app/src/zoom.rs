// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Zoom rules shared by the Waveform editor, its spectral display and the
//! Multitrack: the user's zoom preferences, the wheel's step and anchor,
//! the close-zoom limit, the Follow Playhead hand-off, and the spectral
//! display's frequency view (range, scale, analysis size).
//!
//! Everything here is plain math so it can be tested without a window.

use crate::state::View;

/// Where wheel zoom over the lanes and the waveform zooms around (the
/// rulers and the overviews always zoom at the pointer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WheelAnchor {
    /// The time under the pointer keeps its place. A saved "between the
    /// pointer and the middle" (an earlier choice that drifted) reads as
    /// this.
    #[serde(alias = "BetweenPointerAndMiddle")]
    Pointer,
    /// The playhead keeps its place (held at the nearest edge of the view
    /// when it is out of view).
    Playhead,
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
    /// Settings revision, for one-time moves of old defaults.
    #[serde(default)]
    pub rev: u32,
}

impl Default for ZoomPrefs {
    fn default() -> Self {
        ZoomPrefs {
            wheel_anchor: WheelAnchor::Pointer,
            step_percent: 60.0,
            rev: 2,
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
    pub fn save(&self) {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&p, serde_json::to_string_pretty(self).unwrap_or_default());
    }

    pub fn sanitized(mut self) -> ZoomPrefs {
        if !self.step_percent.is_finite() {
            self.step_percent = 60.0;
        }
        if self.rev < 2 {
            if (self.step_percent - 25.0).abs() < 0.01 {
                self.step_percent = 60.0;
            }
            self.rev = 2;
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
        WheelAnchor::Playhead => playhead.clamp(view.start, view.start + view.len),
    }
}

// ------------------------------------------------------------ time axis

/// The Multitrack zooms out to at least this much time across the lanes
/// (four hours), or the content plus a tenth when that is longer.
pub const SESSION_MIN_SPAN: f64 = 4.0 * 3600.0;

/// The longest view of a session whose content ends at `content_end`.
pub fn max_session_len(content_end: f64) -> f64 {
    SESSION_MIN_SPAN.max(content_end * 1.1)
}

/// How far a time view may zoom and scroll.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeLimits {
    /// The closest zoom (seconds across the view).
    pub min_len: f64,
    /// The farthest zoom.
    pub max_len: f64,
    /// Where the view must stop on the right: the end of the file in the
    /// Waveform editor; none in the Multitrack (scroll on past the end).
    pub end: Option<f64>,
}

impl TimeLimits {
    /// A file `total` seconds long: the closest zoom `min_len`, the whole
    /// file at the farthest, never scrolled past its end.
    pub fn file(total: f64, min_len: f64) -> TimeLimits {
        let total = total.max(1e-7);
        TimeLimits {
            min_len: min_len.min(total),
            max_len: total,
            end: Some(total),
        }
    }

    /// A session whose content ends at `content_end`.
    pub fn session(content_end: f64, min_len: f64) -> TimeLimits {
        TimeLimits {
            min_len,
            max_len: max_session_len(content_end).max(min_len),
            end: None,
        }
    }
}

impl View {
    /// Zooms by `factor` (above 1 zooms out) keeping `anchor` at the same
    /// place in the view. The length is limited first, so at a limit the
    /// step is smaller and the anchor still holds; then the view is kept
    /// inside the limits (which can move it near the start or the end).
    pub fn zoom_at(&mut self, factor: f64, anchor: f64, lim: &TimeLimits) {
        let factor = if factor.is_finite() && factor > 0.0 {
            factor
        } else {
            1.0
        };
        let len = self.len.max(1e-12);
        let new_len = (len * factor).clamp(lim.min_len, lim.max_len.max(lim.min_len));
        self.start = anchor - (anchor - self.start) * (new_len / len);
        self.len = new_len;
        self.limit(lim);
    }

    /// Scrolls by `px` pixels of a view `width` pixels wide (positive
    /// moves later in time).
    pub fn pan_px(&mut self, px: f64, width: f64, lim: &TimeLimits) {
        self.start += px / width.max(1.0) * self.len;
        self.limit(lim);
    }

    /// Keeps the view inside `lim`.
    pub fn limit(&mut self, lim: &TimeLimits) {
        if !self.len.is_finite() || self.len <= 0.0 {
            self.len = lim.max_len;
        }
        self.len = self.len.clamp(lim.min_len, lim.max_len.max(lim.min_len));
        if !self.start.is_finite() {
            self.start = 0.0;
        }
        if let Some(end) = lim.end {
            self.start = self.start.min(end - self.len);
        }
        self.start = self.start.max(0.0);
    }

    /// The view that shows `len` seconds with `room` to spare after it
    /// (1.05 leaves 5 %), from the start.
    pub fn fit_room(len: f64, room: f64, lim: &TimeLimits) -> View {
        let mut v = View {
            start: 0.0,
            len: if len > 0.0 { len * room } else { 60.0 },
        };
        v.limit(lim);
        v
    }

    /// Zoom to Selection: `a..b` fills the view with `margin` of its length
    /// before it (and the rest of the 5 % room after it).
    pub fn show_range(a: f64, b: f64, margin: f64, lim: &TimeLimits) -> View {
        let span = (b - a).abs();
        let mut v = View {
            start: a.min(b) - span * margin,
            len: span * 1.05,
        };
        v.limit(lim);
        v
    }
}

/// Follow Playhead paging: the new view start when the playhead at `pos`
/// has run off the page, or None. The Multitrack turns the page when the
/// playhead passes 92 % of the width (or is left of the view) and puts it
/// at 8 %; the Waveform editor turns it 20 pixels before the right edge
/// and puts it 20 pixels from the left.
pub fn follow_page(v: &View, pos: f64, width_px: f64, multitrack: bool) -> Option<f64> {
    let width_px = width_px.max(1.0);
    let (turn_at, land_at) = if multitrack {
        (0.92, 0.08)
    } else {
        let edge = (20.0 / width_px).min(0.25);
        (1.0 - edge, edge)
    };
    if pos > v.start + v.len * turn_at || pos < v.start {
        Some(pos - v.len * land_at)
    } else {
        None
    }
}

// ------------------------------------------------------ wheel gestures

/// Where the wheel is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// Multitrack: the lanes (clips and empty lanes).
    Lanes,
    /// Multitrack: the track headers.
    Headers,
    /// Multitrack: the time ruler.
    Ruler,
    /// Either editor: the overview strip above the ruler.
    Overview,
    /// Multitrack: the track bar on the right of the lanes.
    TrackBar,
    /// Waveform editor: the waveform, its level scale or the spectrum.
    Wave,
    /// Waveform editor: the time ruler.
    WaveRuler,
    /// Waveform editor: the spectrum's frequency scale.
    FreqScale,
}

/// Where a time zoom keeps its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// Always the pointer.
    Pointer,
    /// The pointer or the playhead, per the "Wheel zoom centers on" setting.
    Setting,
    /// The middle of the view.
    #[allow(dead_code)]
    Center,
}

/// What one wheel movement does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelAction {
    None,
    ZoomTime(Anchor),
    ScrollTime,
    ScrollTracks,
    /// Every track the same height, taller or shorter.
    ZoomTracksEven,
    /// The track under the pointer taller or shorter.
    ResizeOneTrack,
    /// Every track scaled by the same factor, keeping their differences.
    ScaleAllTracks,
    /// The drawn waveforms' amplitude (display only).
    ZoomAmp,
    /// The spectrum's frequencies, at the pointer.
    ZoomFreq,
}

/// The modifiers held with the wheel. `ctrl` is also set for a trackpad
/// pinch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WheelMods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// The wheel rules, one table for both editors (Audiform's final rules):
///
/// | Region | Plain | Ctrl | Shift | Alt | Alt+Shift | Sideways |
/// |---|---|---|---|---|---|---|
/// | Lanes | scroll tracks | zoom time (setting) | scroll time | one track | amplitude | scroll time |
/// | Headers | every track evenly | zoom time (middle) | nothing | one track | scale all tracks | nothing |
/// | Ruler | zoom time (pointer) | zoom time (pointer) | scroll time | nothing | amplitude | scroll time |
/// | Overview | zoom time (pointer) | same | same | same | same | scroll time |
/// | Track bar | every track evenly | same | same | same | same | nothing |
/// | Wave | zoom (setting) or scroll time | zoom time (setting) | scroll time | amplitude | amplitude | scroll time |
/// | Wave ruler | zoom time (pointer) | same | same | amplitude | amplitude | scroll time |
/// | Frequency scale | zoom frequency | same | same | same | same | nothing |
///
/// `sideways` means a sideways wheel or swipe whose sideways part is the
/// larger, with Shift up. `wheel_zooms` is the Waveform editor's "the
/// wheel zooms" setting.
pub fn wheel_action(r: Region, m: WheelMods, sideways: bool, wheel_zooms: bool) -> WheelAction {
    use WheelAction as A;
    let zoom_ptr = A::ZoomTime(Anchor::Pointer);
    let zoom_set = A::ZoomTime(Anchor::Setting);
    match r {
        Region::Lanes => {
            if m.ctrl {
                A::ScrollTracks
            } else if m.alt && m.shift {
                A::ZoomAmp
            } else if m.alt {
                A::ResizeOneTrack
            } else if m.shift || sideways {
                A::ScrollTime
            } else {
                // The plain wheel over the tracks does nothing: zoom is over
                // the headers (heights) and the ruler (time).
                A::None
            }
        }
        Region::Headers => {
            if m.ctrl {
                A::ScrollTracks
            } else if m.alt && m.shift {
                A::ScaleAllTracks
            } else if m.alt {
                A::ResizeOneTrack
            } else if m.shift || sideways {
                A::None
            } else {
                A::ZoomTracksEven
            }
        }
        Region::Ruler => {
            if m.ctrl {
                zoom_ptr
            } else if m.alt && m.shift {
                A::ZoomAmp
            } else if m.alt {
                A::None
            } else if m.shift || sideways {
                A::ScrollTime
            } else {
                zoom_ptr
            }
        }
        Region::Overview => {
            if sideways && !m.ctrl {
                A::ScrollTime
            } else {
                zoom_ptr
            }
        }
        Region::TrackBar => {
            if sideways && !m.ctrl {
                A::None
            } else {
                A::ZoomTracksEven
            }
        }
        Region::Wave => {
            if m.ctrl {
                zoom_set
            } else if m.alt {
                A::ZoomAmp
            } else if m.shift || sideways {
                A::ScrollTime
            } else if wheel_zooms {
                zoom_set
            } else {
                A::ScrollTime
            }
        }
        Region::WaveRuler => {
            if m.ctrl {
                zoom_ptr
            } else if m.alt {
                A::ZoomAmp
            } else if sideways {
                A::ScrollTime
            } else {
                zoom_ptr
            }
        }
        Region::FreqScale => {
            if sideways && !m.ctrl {
                A::None
            } else {
                A::ZoomFreq
            }
        }
    }
}

/// One frame's wheel input, read from egui: the modifiers, whether it is
/// sideways, and the amount in egui points (one notch is [`NOTCH`];
/// positive is the wheel turned away from you). egui sends Ctrl+wheel and
/// trackpad pinches as a zoom factor, and Shift+wheel as sideways
/// scrolling; this puts them back as wheel points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WheelInput {
    pub mods: WheelMods,
    pub sideways: bool,
    /// For zooms: the whole movement in points (the vertical part, or the
    /// sideways part for Shift and sideways swipes).
    pub points: f32,
    /// For scrolling: the movement in points on each axis.
    pub scroll: [f32; 2],
}

impl WheelInput {
    /// From egui's `smooth_scroll_delta`, `zoom_delta()` and modifiers.
    /// None when nothing moved (a zero movement never zooms).
    pub fn read(
        scroll: [f32; 2],
        zoom_delta: f32,
        ctrl: bool,
        shift: bool,
        alt: bool,
    ) -> Option<Self> {
        let pinch = zoom_delta != 1.0 && zoom_delta.is_finite() && zoom_delta > 0.0;
        if pinch {
            return Some(WheelInput {
                mods: WheelMods {
                    ctrl: true,
                    shift,
                    alt,
                },
                sideways: false,
                points: ctrl_points(zoom_delta),
                scroll: [0.0, 0.0],
            });
        }
        let [x, y] = scroll;
        if x == 0.0 && y == 0.0 {
            return None;
        }
        let sideways = !shift && x.abs() > y.abs();
        // egui moves Shift+wheel to the sideways axis.
        let points = if shift || sideways {
            x + if shift { y } else { 0.0 }
        } else {
            y
        };
        Some(WheelInput {
            mods: WheelMods { ctrl, shift, alt },
            sideways,
            points,
            scroll: [x, y],
        })
    }

    /// The movement in wheel notches (fractions for trackpads and smooth
    /// wheels).
    pub fn notches(&self) -> f64 {
        (self.points / NOTCH) as f64
    }
}

/// Track heights: the limits, the folded height is not used yet.
pub const TRACK_MIN_H: f32 = 40.0;
pub const TRACK_MAX_H: f32 = 600.0;
/// Each wheel notch over the headers or the track bar, or with Alt over a
/// track, makes tracks this much taller (or shorter).
pub const TRACK_WHEEL_STEP: f32 = 1.3;
/// Taller Tracks / Shorter Tracks (keys and buttons).
pub const TRACK_KEY_STEP: f32 = 1.25;
/// How long the wheel must rest before a track height change becomes one
/// undo step.
pub const HEIGHT_COMMIT_QUIET: f64 = 0.35;

/// A track height after `notches` of wheel (away from you is taller).
pub fn wheel_height(h: f32, notches: f64) -> f32 {
    (h * TRACK_WHEEL_STEP.powf(notches as f32)).clamp(TRACK_MIN_H, TRACK_MAX_H)
}

/// The scroll that keeps the spot `y` pixels into the view at the same
/// pixel when the rows go from heights `old` to `new` (same count).
pub fn scroll_keeping_rows(old: &[f32], new: &[f32], scroll: f32, y: f32) -> f32 {
    let spot = scroll + y;
    let mut top_old = 0.0;
    let mut top_new = 0.0;
    for (i, &h) in old.iter().enumerate() {
        let nh = new.get(i).copied().unwrap_or(h);
        if spot < top_old + h {
            let frac = ((spot - top_old) / h.max(1.0)).clamp(0.0, 1.0);
            return (top_new + frac * nh - y).max(0.0);
        }
        top_old += h;
        top_new += nh;
    }
    // Past the last row: keep the distance past the end.
    (top_new + (spot - top_old) - y).max(0.0)
}

/// The one height at which `n` tracks fill `view_h` pixels besides
/// `fixed` pixels of rows that do not change.
pub fn fit_height(view_h: f32, fixed: f32, n: usize) -> f32 {
    ((view_h - fixed) / n.max(1) as f32)
        .floor()
        .clamp(TRACK_MIN_H, TRACK_MAX_H)
}

/// A change gathered while the wheel turns and kept as one undo step once
/// it rests: holds the state from before the first movement.
#[derive(Debug, Clone, Default)]
pub struct Debounced<T> {
    pending: Option<(T, f64)>,
}

impl<T> Debounced<T> {
    /// A movement at `now` (seconds); `before` is called only for the
    /// first one of a gesture.
    pub fn touch(&mut self, now: f64, before: impl FnOnce() -> T) {
        match &mut self.pending {
            Some((_, at)) => *at = now,
            None => self.pending = Some((before(), now)),
        }
    }

    /// The state from before the gesture, once it has rested `quiet`
    /// seconds.
    pub fn due(&mut self, now: f64, quiet: f64) -> Option<T> {
        match &self.pending {
            Some((_, at)) if now - at >= quiet => self.pending.take().map(|p| p.0),
            _ => None,
        }
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Ends the gesture now, if one is under way.
    pub fn take(&mut self) -> Option<T> {
        self.pending.take().map(|p| p.0)
    }
}

/// Amplitude zoom after `notches` of wheel: twice as tall per notch,
/// from 1 to [`MAX_AMP`].
pub fn wheel_amp(amp: f32, notches: f64) -> f32 {
    (amp * 2f32.powf(notches as f32)).clamp(1.0, MAX_AMP)
}

/// The spectrum's frequency zoom per wheel notch (the span times 0.8 in,
/// 1.25 out).
pub fn freq_wheel_factor(notches: f64) -> f64 {
    1.25f64.powf(-notches)
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
/// during playback has let go of it. It picks up again once the view is
/// left alone for [`FOLLOW_QUIET`] seconds with the playhead in it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Follow {
    pub on: bool,
    paused: bool,
    /// Seconds since the last hand-made move.
    quiet: f64,
}

/// How long the view must be left alone before following picks up again.
pub const FOLLOW_QUIET: f64 = 0.4;

impl Default for Follow {
    fn default() -> Self {
        Follow {
            on: true,
            paused: false,
            quiet: 0.0,
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
        self.quiet = 0.0;
    }

    /// `dt` seconds passed with the view untouched; `in_view` says whether
    /// the playhead is in it. Left alone long enough with the playhead in
    /// view, following picks up again.
    pub fn idle(&mut self, dt: f64, in_view: bool) {
        self.quiet += dt;
        if self.paused && in_view && self.quiet >= FOLLOW_QUIET {
            self.paused = false;
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

/// The scale files open at: a quarter of the way to logarithmic (about
/// 1 kHz at an eighth of the height, 10 kHz at two thirds).
pub const DEFAULT_LOG_STEP: u8 = 1;

impl Default for SpecView {
    fn default() -> Self {
        SpecView {
            range: None,
            log_step: DEFAULT_LOG_STEP,
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

    /// Drags the scale: `d` is the pointer's movement as a share of the
    /// scale's height (down is positive and brings higher frequencies in,
    /// as if pulling the scale). Only a zoomed view moves; it stops at
    /// 0 Hz and at Nyquist, keeping its size on the scale.
    pub fn pan(&mut self, d: f64, ny: f64) {
        let Some(_) = self.range else { return };
        let ny = ny.max(MIN_SPAN_HZ);
        let (lo, hi) = self.bounds(ny);
        let s = self.log_step;
        let (a, b) = (warp(lo, s), warp(hi, s));
        let (floor, top) = (warp(0.0, s), warp(ny, s));
        let shift = ((b - a) * d).clamp(floor - a, top - b);
        let (nlo, nhi) = (unwarp(a + shift, s), unwarp(b + shift, s));
        let (nlo, nhi) = clamp_range(nlo, nhi, ny);
        self.range = Some((nlo, nhi));
    }

    /// Whether the scale can be dragged (the view is zoomed in).
    pub fn zoomed(&self) -> bool {
        self.range.is_some()
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
        // One notch in: the length shrinks by 1/1.6; one notch out grows by 1.6.
        assert!((p.wheel_factor(NOTCH) - 1.0 / 1.6).abs() < 1e-6);
        assert!((p.wheel_factor(-NOTCH) - 1.6).abs() < 1e-6);
        // Spread over frames, the parts multiply to one notch.
        let f: f64 = (0..4).map(|_| p.wheel_factor(NOTCH / 4.0)).product();
        assert!((f - 1.0 / 1.6).abs() < 1e-6);
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
        assert_eq!(anchor_time(WheelAnchor::Playhead, 11.0, &v, 13.0), 13.0);
        // Playhead out of view: held at the nearest edge.
        assert_eq!(anchor_time(WheelAnchor::Playhead, 11.0, &v, 20.0), 14.0);
        assert_eq!(anchor_time(WheelAnchor::Playhead, 11.0, &v, 2.0), 10.0);
        // A saved "between" reads as the pointer.
        let p: ZoomPrefs =
            serde_json::from_str(r#"{"wheel_anchor":"BetweenPointerAndMiddle"}"#).unwrap();
        assert_eq!(p.wheel_anchor, WheelAnchor::Pointer);
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
        v.zoom_at(0.5, t, &TimeLimits::file(100.0, 0.0));
        assert!(((t - v.start) / v.len - frac).abs() < 1e-12);
        assert!((v.len - 2.0).abs() < 1e-12);
        let min = min_len(1000.0, 48_000.0);
        assert!((min * 48_000.0 - 1000.0 / 32.0).abs() < 1e-9);
        for _ in 0..100 {
            v.zoom_at(0.5, t, &TimeLimits::file(100.0, min));
        }
        assert!((v.len - min).abs() < 1e-12);
        // Zooming out stops at the whole file.
        for _ in 0..100 {
            v.zoom_at(2.0, t, &TimeLimits::file(100.0, min));
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
    fn follow_picks_up_when_the_view_is_left_alone_with_the_playhead_in_it() {
        let mut f = Follow::default();
        f.hand_moved(true);
        // Playhead out of view: stays let go however long.
        f.idle(5.0, false);
        assert!(!f.active());
        f.hand_moved(true);
        f.idle(0.3, true);
        assert!(!f.active());
        f.idle(0.3, true);
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
        assert_eq!(s.log_step, DEFAULT_LOG_STEP);
        // Opening scale: about 1 kHz at 1/8 of the height, 10 kHz at 2/3.
        for ny in [22_050.0, 24_000.0] {
            assert!((s.pos(1000.0, ny) - 0.125).abs() < 0.015);
            assert!((s.pos(10_000.0, ny) - 0.667).abs() < 0.03);
        }
        for _ in 0..10 {
            s.more_log();
        }
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
            log_step: LOG_STEPS,
            ..SpecView::default()
        };
        assert!(n.ticks(22_050.0, 300.0, 16.0).iter().any(|x| x.1 == "C4"));
    }

    // ------------------------------------------------ time axis and limits

    fn px_of(v: &View, t: f64, w: f64) -> f64 {
        (t - v.start) / v.len * w
    }

    #[test]
    fn zoom_at_keeps_the_anchor_pixel_and_halves_the_length() {
        let lim = TimeLimits::session(60.0, 1e-6);
        let mut v = View {
            start: 10.0,
            len: 8.0,
        };
        let w = 1000.0;
        let t = 10.0 + 8.0 * 0.2; // 200 px in
        v.zoom_at(0.5, t, &lim);
        assert!((v.len - 4.0).abs() < 1e-12);
        assert!((px_of(&v, t, w) - 200.0).abs() < 1e-9);
    }

    #[test]
    fn zoom_stops_at_the_limits_and_never_before_zero() {
        let min = min_len(1000.0, 48_000.0);
        let lim = TimeLimits::session(60.0, min);
        let mut v = View {
            start: 30.0,
            len: 10.0,
        };
        for _ in 0..200 {
            v.zoom_at(0.5, 31.0, &lim);
        }
        assert!((v.len - min).abs() < 1e-15);
        // The anchor still holds at the closest zoom.
        assert!(31.0 >= v.start && 31.0 <= v.start + v.len);
        // Zooming out near the start: the view stops at 0.
        let mut v = View {
            start: 1.0,
            len: 10.0,
        };
        v.zoom_at(4.0, 9.0, &lim);
        assert_eq!(v.start, 0.0);
        assert!((v.len - 40.0).abs() < 1e-12);
    }

    #[test]
    fn gathered_steps_equal_single_steps() {
        let lim = TimeLimits::session(600.0, 1e-6);
        let start = View {
            start: 100.0,
            len: 50.0,
        };
        let t = 120.0;
        let mut one = start;
        for _ in 0..5 {
            one.zoom_at(0.8, t, &lim);
        }
        let mut all = start;
        all.zoom_at(0.8f64.powi(5), t, &lim);
        assert!((one.len - all.len).abs() < 1e-9);
        assert!((one.start - all.start).abs() < 1e-6);
        // Fractional notches: twenty fifths of a notch make one notch.
        let p = ZoomPrefs::default();
        let f: f64 = (0..20).map(|_| p.wheel_factor(NOTCH / 20.0)).product();
        assert!((f - p.wheel_factor(NOTCH)).abs() < 1e-6);
    }

    #[test]
    fn sub_sample_scroll_keeps_the_anchor_at_32_px_per_sample() {
        let rate = 48_000.0;
        let w = 1000.0;
        let min = min_len(w as f32, rate);
        let lim = TimeLimits::session(3600.0, min);
        let mut v = View {
            start: 100.0,
            len: min,
        };
        let x = 377.0;
        let t = v.start + x / w * v.len;
        for i in 0..100 {
            let f = if i % 2 == 0 { 1.25 } else { 0.8 };
            v.zoom_at(f, v.start + x / w * v.len, &lim);
        }
        assert!((px_of(&v, t, w) - x).abs() < 0.01, "{}", px_of(&v, t, w));
    }

    #[test]
    fn session_limits_four_hours_or_content_plus_a_tenth() {
        assert_eq!(max_session_len(60.0), 4.0 * 3600.0);
        assert!((max_session_len(6.0 * 3600.0) - 6.0 * 3600.0 * 1.1).abs() < 1e-6);
        let lim = TimeLimits::session(60.0, 1e-6);
        let mut v = View::fit(60.0);
        v.zoom_at(1e9, 30.0, &lim);
        assert_eq!(v.len, 4.0 * 3600.0);
        // Scrolls on past the content.
        v.pan_px(5000.0, 1000.0, &lim);
        assert!(v.start > 60.0);
        // A file: never wider than it, never past its end.
        let lim = TimeLimits::file(4.12, 1e-6);
        let mut v = View::fit(4.12);
        v.zoom_at(0.25, 4.0, &lim);
        v.pan_px(1e6, 1000.0, &lim);
        assert!((v.start + v.len - 4.12).abs() < 1e-9);
        v.zoom_at(100.0, 1.0, &lim);
        assert_eq!((v.start, v.len), (0.0, 4.12));
    }

    #[test]
    fn fit_and_show_range() {
        let lim = TimeLimits::session(60.0, 1e-6);
        let v = View::fit_room(60.0, 1.05, &lim);
        assert_eq!(v.start, 0.0);
        assert!((v.len - 63.0).abs() < 1e-9);
        // The file fits exactly.
        let v = View::fit_room(4.0, 1.0, &TimeLimits::file(4.0, 1e-6));
        assert_eq!((v.start, v.len), (0.0, 4.0));
        // Zoom to Selection: 2.5 % margin before.
        let v = View::show_range(10.0, 20.0, 0.025, &lim);
        assert!((v.start - 9.75).abs() < 1e-9 && (v.len - 10.5).abs() < 1e-9);
        let v = View::show_range(0.0, 10.0, 0.025, &lim);
        assert_eq!(v.start, 0.0);
    }

    #[test]
    fn follow_pages_at_92_percent_or_20_px() {
        let v = View {
            start: 0.0,
            len: 10.0,
        };
        assert_eq!(follow_page(&v, 9.0, 1000.0, true), None);
        let s = follow_page(&v, 9.3, 1000.0, true).unwrap();
        assert!((s - 8.5).abs() < 1e-9);
        assert!(follow_page(&v, -1.0, 1000.0, true).is_some());
        // Waveform: 20 px of 1000 is 0.2 s here.
        assert_eq!(follow_page(&v, 9.7, 1000.0, false), None);
        let s = follow_page(&v, 9.85, 1000.0, false).unwrap();
        assert!((s - 9.65).abs() < 1e-9);
    }

    // ------------------------------------------------------------ wheel

    #[test]
    fn wheel_input_reads_notches_and_ignores_zero() {
        assert_eq!(WheelInput::read([0.0, 0.0], 1.0, false, false, false), None);
        let w = WheelInput::read([0.0, NOTCH], 1.0, false, false, false).unwrap();
        assert!((w.notches() - 1.0).abs() < 1e-9 && !w.sideways);
        // Shift: egui moved it sideways; still one notch, not "sideways".
        let w = WheelInput::read([NOTCH, 0.0], 1.0, false, true, false).unwrap();
        assert!((w.notches() - 1.0).abs() < 1e-9 && !w.sideways && w.mods.shift);
        // A sideways swipe.
        let w = WheelInput::read([-30.0, 4.0], 1.0, false, false, false).unwrap();
        assert!(w.sideways);
        // Ctrl+wheel (a zoom factor from egui) and a pinch read as Ctrl.
        let z = (NOTCH / 200.0).exp();
        let w = WheelInput::read([0.0, 0.0], z, false, false, false).unwrap();
        assert!(w.mods.ctrl && (w.notches() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn wheel_table_multitrack() {
        use Anchor::*;
        use Region::*;
        use WheelAction as A;
        let m = |ctrl, shift, alt| WheelMods { ctrl, shift, alt };
        let plain = m(false, false, false);
        let ctrl = m(true, false, false);
        let shift = m(false, true, false);
        let alt = m(false, false, true);
        let alt_shift = m(false, true, true);
        let t = |r, mm, side| wheel_action(r, mm, side, true);
        // Lanes.
        // Over the tracks the wheel zooms time at the pointer, as over the
        // ruler; Ctrl scrolls up and down.
        assert_eq!(t(Lanes, plain, false), A::None);
        assert_eq!(t(Lanes, ctrl, false), A::ScrollTracks);
        assert_eq!(t(Lanes, shift, false), A::ScrollTime);
        assert_eq!(t(Lanes, alt, false), A::ResizeOneTrack);
        assert_eq!(t(Lanes, alt_shift, false), A::ZoomAmp);
        assert_eq!(t(Lanes, plain, true), A::ScrollTime);
        // Headers.
        assert_eq!(t(Headers, plain, false), A::ZoomTracksEven);
        assert_eq!(t(Headers, ctrl, false), A::ScrollTracks);
        assert_eq!(t(Headers, shift, false), A::None);
        assert_eq!(t(Headers, alt, false), A::ResizeOneTrack);
        assert_eq!(t(Headers, alt_shift, false), A::ScaleAllTracks);
        assert_eq!(t(Headers, plain, true), A::None);
        // Ruler: the plain wheel zooms exactly as Ctrl+wheel over the lanes,
        // but always at the pointer.
        assert_eq!(t(Ruler, plain, false), A::ZoomTime(Pointer));
        assert_eq!(t(Ruler, ctrl, false), A::ZoomTime(Pointer));
        assert_eq!(t(Ruler, shift, false), A::ScrollTime);
        assert_eq!(t(Ruler, alt, false), A::None);
        assert_eq!(t(Ruler, alt_shift, false), A::ZoomAmp);
        assert_eq!(t(Ruler, plain, true), A::ScrollTime);
        // Overview.
        for mm in [plain, ctrl, shift, alt, alt_shift] {
            assert_eq!(t(Overview, mm, false), A::ZoomTime(Pointer));
        }
        assert_eq!(t(Overview, plain, true), A::ScrollTime);
        // Track bar.
        for mm in [plain, ctrl, shift, alt, alt_shift] {
            assert_eq!(t(TrackBar, mm, false), A::ZoomTracksEven);
        }
        assert_eq!(t(TrackBar, plain, true), A::None);
    }

    #[test]
    fn wheel_table_waveform() {
        use Anchor::*;
        use Region::*;
        use WheelAction as A;
        let m = |ctrl, shift, alt| WheelMods { ctrl, shift, alt };
        let plain = m(false, false, false);
        let ctrl = m(true, false, false);
        let shift = m(false, true, false);
        let alt = m(false, false, true);
        // Wave, with "the wheel zooms" on and off.
        assert_eq!(wheel_action(Wave, plain, false, true), A::ZoomTime(Setting));
        assert_eq!(wheel_action(Wave, plain, false, false), A::ScrollTime);
        assert_eq!(wheel_action(Wave, ctrl, false, false), A::ZoomTime(Setting));
        assert_eq!(wheel_action(Wave, shift, false, true), A::ScrollTime);
        assert_eq!(wheel_action(Wave, alt, false, true), A::ZoomAmp);
        assert_eq!(wheel_action(Wave, plain, true, true), A::ScrollTime);
        // Ruler: always zooms at the pointer, even with the setting off;
        // a sideways swipe scrolls (never zooms in).
        for on in [true, false] {
            assert_eq!(
                wheel_action(WaveRuler, plain, false, on),
                A::ZoomTime(Pointer)
            );
            assert_eq!(
                wheel_action(WaveRuler, shift, false, on),
                A::ZoomTime(Pointer)
            );
        }
        assert_eq!(
            wheel_action(WaveRuler, ctrl, false, true),
            A::ZoomTime(Pointer)
        );
        assert_eq!(wheel_action(WaveRuler, alt, false, true), A::ZoomAmp);
        assert_eq!(wheel_action(WaveRuler, plain, true, true), A::ScrollTime);
        // Frequency scale: modifiers ignored.
        for mm in [plain, ctrl, shift, alt] {
            assert_eq!(wheel_action(FreqScale, mm, false, true), A::ZoomFreq);
        }
        assert_eq!(wheel_action(FreqScale, plain, true, true), A::None);
    }

    // ------------------------------------------------------ track stack

    #[test]
    fn track_heights_step_and_stay_in_limits() {
        assert!((wheel_height(100.0, 1.0) - 130.0).abs() < 1e-3);
        assert!((wheel_height(130.0, -1.0) - 100.0).abs() < 1e-3);
        assert_eq!(wheel_height(590.0, 5.0), TRACK_MAX_H);
        assert_eq!(wheel_height(45.0, -5.0), TRACK_MIN_H);
        assert_eq!(fit_height(400.0, 40.0, 4), 90.0);
        assert_eq!(fit_height(100.0, 0.0, 50), TRACK_MIN_H);
    }

    #[test]
    fn height_zoom_keeps_the_content_under_the_pointer() {
        let old = [100.0, 50.0, 100.0];
        let new = [200.0, 100.0, 200.0];
        // 40 px into the view, 80 px scrolled: 20 px (40 %) into row 2.
        let s = scroll_keeping_rows(&old, &new, 80.0, 40.0);
        assert!((s + 40.0 - (200.0 + 0.4 * 100.0)).abs() < 1e-3, "{s}");
        // Never above the top.
        assert_eq!(scroll_keeping_rows(&old, &[40.0; 3], 0.0, 10.0), 0.0);
        // Past the last row: the distance past the end is kept.
        let s = scroll_keeping_rows(&old, &new, 260.0, 0.0);
        assert!((s - 510.0).abs() < 1e-3);
    }

    #[test]
    fn a_wheel_gesture_is_one_change_once_it_rests() {
        let mut d: Debounced<u32> = Debounced::default();
        let mut made = 0;
        d.touch(0.0, || {
            made += 1;
            7
        });
        d.touch(0.2, || {
            made += 1;
            8
        });
        assert_eq!(made, 1);
        assert_eq!(d.due(0.4, HEIGHT_COMMIT_QUIET), None);
        assert_eq!(d.due(0.6, HEIGHT_COMMIT_QUIET), Some(7));
        assert!(!d.is_pending());
    }

    #[test]
    fn amplitude_and_frequency_wheel_steps() {
        assert_eq!(wheel_amp(1.0, 1.0), 2.0);
        assert_eq!(wheel_amp(16.0, 1.0), MAX_AMP);
        assert_eq!(wheel_amp(2.0, -3.0), 1.0);
        assert!((freq_wheel_factor(1.0) - 0.8).abs() < 1e-12);
        assert!((freq_wheel_factor(-1.0) - 1.25).abs() < 1e-12);
    }

    #[test]
    fn dragging_the_frequency_scale_pans_a_zoomed_view_and_stops_at_the_ends() {
        let ny = 24_000.0;
        let mut s = SpecView {
            log_step: 0,
            ..SpecView::default()
        };
        // Not zoomed: nothing moves.
        s.pan(0.5, ny);
        assert_eq!(s.range, None);
        s.range = Some((1000.0, 3000.0));
        s.pan(0.5, ny);
        let (lo, hi) = s.bounds(ny);
        assert!((lo - 2000.0).abs() < 1e-6 && (hi - 4000.0).abs() < 1e-6);
        // Stops at Nyquist and at 0 Hz, keeping its size.
        s.pan(100.0, ny);
        let (lo, hi) = s.bounds(ny);
        assert!((hi - ny).abs() < 1e-6 && (hi - lo - 2000.0).abs() < 1e-6);
        s.pan(-100.0, ny);
        let (lo, hi) = s.bounds(ny);
        assert!(lo.abs() < 1e-6 && (hi - 2000.0).abs() < 1e-6);
        // On a log-like scale the size on the scale is kept.
        let mut s = SpecView {
            log_step: LOG_STEPS,
            range: Some((200.0, 800.0)),
            ..SpecView::default()
        };
        let span = warp(800.0, LOG_STEPS) - warp(200.0, LOG_STEPS);
        s.pan(0.25, ny);
        let (lo, hi) = s.bounds(ny);
        assert!(((warp(hi, LOG_STEPS) - warp(lo, LOG_STEPS)) - span).abs() < 1e-9);
        assert!(lo > 200.0);
    }
}
