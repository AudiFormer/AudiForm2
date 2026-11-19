// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The transport's rules, kept apart from the window so they can be
//! tested: the Loop / Skip mode and the plan it gives the audio thread,
//! where Stop leaves the playhead, a new loop taking over while playing,
//! and the time display formats (the clock, the ruler and typed times).

use af27_core::midi::Tempo;
use af27_engine::{Plan, Region};
use serde::{Deserialize, Serialize};

// ------------------------------------------------------------ play mode

/// Normal, Loop Playback or Skip Selection. The loop (or the skipped part)
/// is always the time selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PlayMode {
    #[default]
    Normal,
    Loop,
    Skip,
}

impl PlayMode {
    pub const ALL: [PlayMode; 3] = [PlayMode::Normal, PlayMode::Loop, PlayMode::Skip];

    pub fn label(self) -> &'static str {
        match self {
            PlayMode::Normal => "Normal Playback",
            PlayMode::Loop => "Loop Playback",
            PlayMode::Skip => "Skip Selection",
        }
    }

    /// What the Loop button steps to.
    pub fn next(self) -> PlayMode {
        match self {
            PlayMode::Normal => PlayMode::Loop,
            PlayMode::Loop => PlayMode::Skip,
            PlayMode::Skip => PlayMode::Normal,
        }
    }

    /// Ctrl+L: loop on, or back to normal.
    pub fn toggle_loop(self) -> PlayMode {
        if self == PlayMode::Loop {
            PlayMode::Normal
        } else {
            PlayMode::Loop
        }
    }
}

/// Where a fresh Play starts: from the end, back at the start.
pub fn fresh_start(cursor: f64, len: f64) -> f64 {
    if len > 0.0 && cursor >= len - 0.01 {
        0.0
    } else {
        cursor.max(0.0)
    }
}

/// What playback from `from` does, in `mode`, with time selection `sel`,
/// on a file or session `len` long. Returns the start and the plan.
///
/// - Recording never stops at the end and never jumps (back or on).
/// - Normal, starting inside the selection: to the selection's end.
///   Otherwise to the end of the file or of the last clip.
/// - Loop: the selection repeats (no selection: 0 to the end). Starting
///   after it plays straight on to the end.
/// - Skip: the selection is jumped over; starting inside it starts at its end.
pub fn plan_for(
    mode: PlayMode,
    sel: Option<(f64, f64)>,
    from: f64,
    len: f64,
    recording: bool,
) -> (f64, Plan) {
    let from = from.max(0.0);
    if recording {
        return (from, Plan::default());
    }
    let sel = sel.filter(|(a, b)| b - a > 1e-6);
    let plan = match (mode, sel) {
        (PlayMode::Normal, Some((a, b))) if from >= a && from < b => Plan::new(b, Region::None),
        (PlayMode::Loop, _) => {
            let (a, b) = sel.unwrap_or((0.0, len));
            Plan::new(len.max(b), Region::Loop { start: a, end: b })
        }
        (PlayMode::Skip, Some((a, b))) => Plan::new(len, Region::Skip { start: a, end: b }),
        _ => Plan::new(len, Region::None),
    };
    (plan.start_at(from), plan)
}

/// Where Stop leaves the playhead: where playback reached, or back at the
/// start marker with Return to Start on Stop.
pub fn stop_position(reached: f64, start_marker: Option<f64>, return_on_stop: bool) -> f64 {
    match start_marker {
        Some(s) if return_on_stop => s,
        _ => reached,
    }
}

/// A new loop `a..b` taking over while playing: the jump to make (to its
/// start, when the playhead is outside it), or None to carry on.
pub fn takeover(pos: f64, a: f64, b: f64) -> Option<f64> {
    (pos < a || pos >= b).then_some(a)
}

/// How long a new selection must stay still before it becomes the loop.
pub const SETTLE_SECS: f64 = 0.2;

/// Watches the time selection while looping, and says when a changed one
/// has settled.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Settle {
    /// The selection the audio thread loops now.
    applied: Option<(f64, f64)>,
    /// A different one, and when it last changed.
    pending: Option<(Option<(f64, f64)>, f64)>,
}

impl Settle {
    /// Playback started looping `sel`.
    pub fn start(&mut self, sel: Option<(f64, f64)>) {
        self.applied = sel;
        self.pending = None;
    }

    /// The selection is `sel` at `now` (seconds). Returns the new loop once
    /// it has stayed the same for [`SETTLE_SECS`].
    pub fn update(&mut self, sel: Option<(f64, f64)>, now: f64) -> Option<Option<(f64, f64)>> {
        if sel == self.applied {
            self.pending = None;
            return None;
        }
        match self.pending {
            Some((p, since)) if p == sel => {
                if now - since >= SETTLE_SECS {
                    self.start(sel);
                    return Some(sel);
                }
            }
            _ => self.pending = Some((sel, now)),
        }
        None
    }
}

/// Held Left/Right while playing: every this many seconds playback jumps.
pub const SCRUB_STEP: f64 = 0.1;
/// Held Left/Right move the playhead at this many times real time.
pub const SCRUB_SPEED: f64 = 4.0;

/// The jump to make each [`SCRUB_STEP`] while playing, so playback (which
/// moves on by itself) covers ground at [`SCRUB_SPEED`] times real time.
pub fn scrub_jump(dir: f64) -> f64 {
    (SCRUB_SPEED * dir - 1.0) * SCRUB_STEP
}

// ---------------------------------------------------------- time format

/// How times are shown and typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TimeFormat {
    /// 1:05.250
    #[default]
    MinSec,
    /// 65.250
    Seconds,
    /// bar.beat.tick, 960 ticks a beat, at the session tempo.
    BarsBeats,
    /// 3132000
    Samples,
    /// 00:01:05:07 at 30 frames a second.
    Timecode,
}

/// Timecode frames a second.
pub const FPS: f64 = 30.0;

impl TimeFormat {
    pub const ALL: [TimeFormat; 5] = [
        TimeFormat::MinSec,
        TimeFormat::Seconds,
        TimeFormat::BarsBeats,
        TimeFormat::Samples,
        TimeFormat::Timecode,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TimeFormat::MinSec => "Minutes and Seconds",
            TimeFormat::Seconds => "Seconds",
            TimeFormat::BarsBeats => "Bars and Beats",
            TimeFormat::Samples => "Samples",
            TimeFormat::Timecode => "Timecode (30 fps)",
        }
    }

    /// A few letters for the clock's format button.
    pub fn short(self) -> &'static str {
        match self {
            TimeFormat::MinSec => "M:S",
            TimeFormat::Seconds => "SEC",
            TimeFormat::BarsBeats => "BAR",
            TimeFormat::Samples => "SMP",
            TimeFormat::Timecode => "TC",
        }
    }

    pub fn next(self) -> TimeFormat {
        let i = TimeFormat::ALL.iter().position(|f| *f == self).unwrap_or(0);
        TimeFormat::ALL[(i + 1) % TimeFormat::ALL.len()]
    }
}

/// A time format with what it needs: the sample rate and the tempo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeCtx {
    pub format: TimeFormat,
    pub rate: u32,
    pub tempo: Tempo,
}

impl TimeCtx {
    fn rate(&self) -> f64 {
        self.rate.max(1) as f64
    }

    fn bpb(&self) -> f64 {
        self.tempo.beats_per_bar.max(1) as f64
    }

    /// The clock's text for `secs`.
    pub fn format(&self, secs: f64) -> String {
        let secs = secs.max(0.0);
        match self.format {
            TimeFormat::MinSec => af27_core::time::format_time(secs),
            TimeFormat::Seconds => format!("{secs:.3}"),
            TimeFormat::BarsBeats => self.tempo.format(secs),
            TimeFormat::Samples => format!("{}", (secs * self.rate()).round() as u64),
            TimeFormat::Timecode => {
                let frames = (secs * FPS + 1e-6).floor() as u64;
                let f = frames % FPS as u64;
                let s = frames / FPS as u64;
                format!(
                    "{:02}:{:02}:{:02}:{:02}",
                    s / 3600,
                    (s / 60) % 60,
                    s % 60,
                    f
                )
            }
        }
    }

    /// Reads a typed time in this format (or, failing that, as
    /// `h:mm:ss.mmm` or plain seconds).
    pub fn parse(&self, text: &str) -> Option<f64> {
        let t = text.trim();
        let nums = |sep: char| -> Option<Vec<f64>> {
            t.split(sep)
                .map(|p| {
                    p.trim()
                        .parse::<f64>()
                        .ok()
                        .filter(|v| v.is_finite() && *v >= 0.0)
                })
                .collect()
        };
        let own = match self.format {
            TimeFormat::MinSec => None,
            TimeFormat::Seconds => t.parse::<f64>().ok().filter(|v| *v >= 0.0),
            TimeFormat::BarsBeats => nums('.').filter(|v| v.len() <= 3).map(|v| {
                let bar = v.first().copied().unwrap_or(1.0).max(1.0) - 1.0;
                let beat = v.get(1).copied().unwrap_or(1.0).max(1.0) - 1.0;
                let tick = v.get(2).copied().unwrap_or(0.0);
                self.tempo.secs(bar * self.bpb() + beat + tick / 960.0)
            }),
            TimeFormat::Samples => t.parse::<u64>().ok().map(|s| s as f64 / self.rate()),
            TimeFormat::Timecode => nums(':')
                .filter(|v| v.len() == 4)
                .map(|v| v[0] * 3600.0 + v[1] * 60.0 + v[2] + v[3] / FPS),
        };
        own.or_else(|| parse_plain(t))
    }

    /// The ruler's spacing (seconds) for `len` seconds across `px` pixels:
    /// labels about 90 pixels apart, on the format's own round numbers.
    pub fn ruler_step(&self, len: f64, px: f32) -> f64 {
        let target = len / (px as f64 / 90.0).max(1.0);
        match self.format {
            TimeFormat::MinSec | TimeFormat::Seconds => pick(&SECS_STEPS, target),
            TimeFormat::Samples => {
                let want = target * self.rate();
                let mut k = 1.0;
                loop {
                    for m in [1.0, 2.0, 5.0] {
                        if m * k >= want {
                            return m * k / self.rate();
                        }
                    }
                    k *= 10.0;
                }
            }
            TimeFormat::Timecode => {
                let frames = [1.0, 2.0, 5.0, 10.0, 15.0].map(|f| f / FPS);
                frames
                    .into_iter()
                    .find(|s| *s >= target)
                    .unwrap_or_else(|| pick(&SECS_STEPS[9..], target))
            }
            TimeFormat::BarsBeats => {
                let beat = self.tempo.secs(1.0);
                let bpb = self.bpb();
                let mut beats = vec![0.25, 0.5, 1.0, bpb];
                let mut bars = 2.0;
                while bars <= 4096.0 {
                    beats.push(bars * bpb);
                    bars *= 2.0;
                }
                beats
                    .into_iter()
                    .map(|b| b * beat)
                    .find(|s| *s >= target)
                    .unwrap_or(4096.0 * bpb * beat)
            }
        }
    }

    /// A ruler label at `t` for marks `step` apart.
    pub fn ruler_label(&self, t: f64, step: f64) -> String {
        match self.format {
            TimeFormat::MinSec => minsec_label(t, step),
            TimeFormat::Seconds => format!("{:.*}", decimals(step), t),
            TimeFormat::Samples => format!("{}", (t * self.rate()).round() as u64),
            TimeFormat::Timecode => {
                let full = self.format(t);
                if step < 1.0 {
                    full
                } else {
                    full[..8].to_string()
                }
            }
            TimeFormat::BarsBeats => {
                let full = self.tempo.format(t + 1e-9);
                let beat = self.tempo.secs(1.0);
                let mut parts = full.split('.');
                let (bar, b) = (parts.next().unwrap_or("1"), parts.next().unwrap_or("1"));
                if step >= beat * self.bpb() - 1e-9 {
                    bar.to_string()
                } else if step >= beat - 1e-9 {
                    format!("{bar}.{b}")
                } else {
                    full
                }
            }
        }
    }
}

const SECS_STEPS: [f64; 21] = [
    0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0,
    120.0, 300.0, 600.0, 1800.0, 3600.0,
];

fn pick(steps: &[f64], target: f64) -> f64 {
    steps
        .iter()
        .copied()
        .find(|s| *s >= target)
        .unwrap_or(steps[steps.len() - 1])
}

fn decimals(step: f64) -> usize {
    if step < 0.01 {
        3
    } else if step < 0.1 {
        2
    } else if step < 1.0 {
        1
    } else {
        0
    }
}

fn minsec_label(t: f64, step: f64) -> String {
    if step < 1.0 {
        let m = (t / 60.0).floor();
        let s = t - m * 60.0;
        let dec = decimals(step);
        format!("{}:{:0w$.dec$}", m as u64, s, w = dec + 3, dec = dec)
    } else {
        let t = t.round() as u64;
        if t >= 3600 {
            format!("{}:{:02}:{:02}", t / 3600, (t / 60) % 60, t % 60)
        } else {
            format!("{}:{:02}", t / 60, t % 60)
        }
    }
}

/// `h:mm:ss.mmm`, `m:ss.mmm` or plain seconds.
pub fn parse_plain(text: &str) -> Option<f64> {
    let mut secs = 0.0;
    for part in text.trim().split(':') {
        let v: f64 = part.trim().parse().ok()?;
        if !v.is_finite() || v < 0.0 {
            return None;
        }
        secs = secs * 60.0 + v;
    }
    Some(secs)
}

/// The ruler mark nearest `t` when it is within `tol` seconds.
pub fn snap_to_marks(t: f64, step: f64, tol: f64) -> f64 {
    if step <= 0.0 {
        return t;
    }
    let m = (t / step).round() * step;
    if (m - t).abs() <= tol {
        m
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEN: f64 = 1000.0;

    #[test]
    fn normal_play_stops_at_the_selection_end_only_when_starting_inside() {
        let sel = Some((100.0, 200.0));
        let (from, p) = plan_for(PlayMode::Normal, sel, 150.0, LEN, false);
        assert_eq!((from, p.stop, p.region), (150.0, 200.0, Region::None));
        let (_, p) = plan_for(PlayMode::Normal, sel, 50.0, LEN, false);
        assert_eq!(p.stop, LEN);
        let (_, p) = plan_for(PlayMode::Normal, None, 50.0, LEN, false);
        assert_eq!(p.stop, LEN);
    }

    #[test]
    fn loop_is_the_selection_or_the_whole_file() {
        let (_, p) = plan_for(PlayMode::Loop, Some((100.0, 200.0)), 150.0, LEN, false);
        assert_eq!(
            p.region,
            Region::Loop {
                start: 100.0,
                end: 200.0
            }
        );
        // The reference positions, through the plan the app builds.
        assert_eq!(p.advance(150.0, 60.0), Some(110.0));
        let (_, p) = plan_for(PlayMode::Loop, Some((100.0, 200.0)), 50.0, LEN, false);
        assert_eq!(p.advance(50.0, 40.0), Some(90.0));
        assert_eq!(p.advance(50.0, 150.0), Some(100.0));
        assert_eq!(p.advance(50.0, 275.0), Some(125.0));
        assert_eq!(p.advance(300.0, 10.0), Some(310.0));
        let (_, p) = plan_for(PlayMode::Loop, None, 990.0, LEN, false);
        assert_eq!(
            p.region,
            Region::Loop {
                start: 0.0,
                end: LEN
            }
        );
        assert_eq!(p.advance(990.0, 20.0), Some(10.0));
    }

    #[test]
    fn skip_starts_after_the_selection_from_inside() {
        let (from, p) = plan_for(PlayMode::Skip, Some((100.0, 200.0)), 150.0, LEN, false);
        assert_eq!(from, 200.0);
        let (from, p2) = plan_for(PlayMode::Skip, Some((100.0, 200.0)), 50.0, LEN, false);
        assert_eq!(from, 50.0);
        assert_eq!(p2.advance(50.0, 60.0), Some(210.0));
        assert_eq!(p.stop, LEN);
    }

    #[test]
    fn recording_never_jumps_or_stops() {
        for m in PlayMode::ALL {
            let (from, p) = plan_for(m, Some((100.0, 200.0)), 150.0, LEN, true);
            assert_eq!(from, 150.0);
            assert_eq!(p, Plan::default());
            assert_eq!(p.advance(150.0, 5000.0), Some(5150.0));
        }
    }

    #[test]
    fn play_from_the_end_starts_over() {
        assert_eq!(fresh_start(LEN, LEN), 0.0);
        assert_eq!(fresh_start(500.0, LEN), 500.0);
    }

    #[test]
    fn stop_folds_the_start_marker_into_the_playhead() {
        assert_eq!(stop_position(250.0, Some(100.0), false), 250.0);
        assert_eq!(stop_position(250.0, Some(100.0), true), 100.0);
        assert_eq!(stop_position(250.0, None, true), 250.0);
    }

    #[test]
    fn a_new_loop_takes_over_once_it_settles() {
        let mut s = Settle::default();
        s.start(Some((1.0, 2.0)));
        assert_eq!(s.update(Some((1.0, 2.0)), 0.0), None);
        // Dragging: each move restarts the wait.
        assert_eq!(s.update(Some((3.0, 4.0)), 1.0), None);
        assert_eq!(s.update(Some((3.0, 5.0)), 1.1), None);
        assert_eq!(s.update(Some((3.0, 5.0)), 1.25), None);
        assert_eq!(s.update(Some((3.0, 5.0)), 1.31), Some(Some((3.0, 5.0))));
        assert_eq!(s.update(Some((3.0, 5.0)), 2.0), None);
        // Outside the new loop: to its start. Inside: carry on.
        assert_eq!(takeover(2.5, 3.0, 5.0), Some(3.0));
        assert_eq!(takeover(5.0, 3.0, 5.0), Some(3.0));
        assert_eq!(takeover(4.0, 3.0, 5.0), None);
    }

    #[test]
    fn mode_buttons_cycle() {
        assert_eq!(PlayMode::Normal.next(), PlayMode::Loop);
        assert_eq!(PlayMode::Loop.next(), PlayMode::Skip);
        assert_eq!(PlayMode::Skip.next(), PlayMode::Normal);
        assert_eq!(PlayMode::Skip.toggle_loop(), PlayMode::Loop);
        assert_eq!(PlayMode::Loop.toggle_loop(), PlayMode::Normal);
    }

    #[test]
    fn held_arrows_cover_four_times_real_time() {
        // Forward: playback's own 0.1 s plus the jump.
        assert!((scrub_jump(1.0) + SCRUB_STEP - 0.4).abs() < 1e-12);
        assert!((scrub_jump(-1.0) + SCRUB_STEP + 0.4).abs() < 1e-12);
    }

    fn ctx(format: TimeFormat) -> TimeCtx {
        TimeCtx {
            format,
            rate: 48_000,
            tempo: Tempo::default(),
        }
    }

    #[test]
    fn time_formats() {
        let t = 65.25;
        assert_eq!(ctx(TimeFormat::MinSec).format(t), "1:05.250");
        assert_eq!(ctx(TimeFormat::Seconds).format(t), "65.250");
        // 120 BPM in 4/4: 130.5 beats = bar 33, beat 3, half a beat.
        assert_eq!(ctx(TimeFormat::BarsBeats).format(t), "33.3.480");
        assert_eq!(ctx(TimeFormat::Samples).format(t), "3132000");
        assert_eq!(ctx(TimeFormat::Timecode).format(t), "00:01:05:07");
    }

    #[test]
    fn typed_times_read_back_in_each_format() {
        for f in TimeFormat::ALL {
            let c = ctx(f);
            for t in [0.0, 1.5, 65.25, 3725.5] {
                let back = c.parse(&c.format(t)).unwrap();
                // Timecode keeps whole frames.
                let tol = if f == TimeFormat::Timecode {
                    1.0 / FPS
                } else {
                    1e-3
                };
                assert!((back - t).abs() <= tol, "{f:?} {t} -> {back}");
            }
            // Plain seconds and m:ss always work.
            if f != TimeFormat::BarsBeats && f != TimeFormat::Samples {
                assert_eq!(c.parse("1:30"), Some(90.0), "{f:?}");
            }
        }
        assert_eq!(ctx(TimeFormat::BarsBeats).parse("2"), Some(2.0));
        assert_eq!(ctx(TimeFormat::Samples).parse("48000"), Some(1.0));
        assert_eq!(ctx(TimeFormat::Seconds).parse("x"), None);
    }

    #[test]
    fn ruler_marks_fall_on_each_formats_round_numbers() {
        let c = ctx(TimeFormat::BarsBeats);
        // 60 s over 900 px: about 6 s a label, so 4 bars (8 s) at 120 BPM.
        assert_eq!(c.ruler_step(60.0, 900.0), 8.0);
        assert_eq!(c.ruler_label(16.0, 8.0), "9");
        assert_eq!(c.ruler_label(16.5, 0.5), "9.2");
        let c = ctx(TimeFormat::Samples);
        assert_eq!(c.ruler_step(1.0, 900.0) * 48_000.0, 5000.0);
        let c = ctx(TimeFormat::Timecode);
        assert_eq!(c.ruler_step(1.0, 900.0), 5.0 / FPS);
        assert_eq!(c.ruler_label(2.0, 5.0), "00:00:02");
        assert_eq!(ctx(TimeFormat::Seconds).ruler_label(65.25, 0.05), "65.25");
        assert_eq!(ctx(TimeFormat::MinSec).ruler_label(65.0, 5.0), "1:05");
    }

    #[test]
    fn snapping_reaches_only_nearby_marks() {
        assert_eq!(snap_to_marks(9.8, 5.0, 0.3), 10.0);
        assert_eq!(snap_to_marks(9.0, 5.0, 0.3), 9.0);
    }
}
