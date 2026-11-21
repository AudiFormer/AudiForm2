// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Multitrack tools for spoken word, under Multitrack > Spoken Word:
//!
//! * **Auto Duck** listens to the voice tracks and writes volume
//!   automation on the music (or ambience) tracks so they dip under the
//!   talking and come back up in the gaps: fades before each phrase and
//!   after it, short gaps between phrases bridged so the bed does not
//!   pump. Any automation already there is kept, with the dips on top.
//! * **Speaker Balance** measures each voice track's own loudness (its
//!   clips as recorded, before effects and faders) and changes clip gain
//!   so every speaker sits at the same level: the quietest one, or a
//!   level you pick.
//!
//! Each is one undo step.

use std::sync::Arc;

use af27_core::automation::{Lane, LanePoint, LaneTarget};
use af27_core::{AudioBuffer, MediaId, Session, Track, TrackId, TrackKind};
use af27_dsp::loudness::LiveLoudness;
use egui::RichText;

use crate::state::State;
use crate::theme::c;

/// Window the voice is listened to in.
const WIN: f64 = 0.01;

// ------------------------------------------------------------- logic

/// The track's clips as recorded (clip gain and fades, no effects or
/// fader), at `rate`, from `from` to `to` seconds, handed out in chunks
/// of stereo frames. Each media file is read at its own rate (nearest
/// sample): good enough to measure, never played.
pub fn each_dry_chunk(
    track: &Track,
    media: &dyn Fn(MediaId) -> Option<Arc<AudioBuffer>>,
    rate: u32,
    from: f64,
    to: f64,
    mut each: impl FnMut(&[[f32; 2]]),
) {
    const CHUNK: usize = 8192;
    let total = ((to - from).max(0.0) * rate as f64).round() as usize;
    let bufs: Vec<Option<Arc<AudioBuffer>>> = track.clips.iter().map(|c| media(c.media)).collect();
    let mut out = vec![[0.0f32; 2]; CHUNK];
    let mut done = 0;
    while done < total {
        let n = (total - done).min(CHUNK);
        let frame = &mut out[..n];
        frame.fill([0.0; 2]);
        let t0 = from + done as f64 / rate as f64;
        let t1 = t0 + n as f64 / rate as f64;
        for (clip, buf) in track.clips.iter().zip(&bufs) {
            let Some(buf) = buf else { continue };
            if clip.muted || clip.end() <= t0 || clip.start >= t1 || buf.frames() == 0 {
                continue;
            }
            let ch = buf.num_channels();
            let mr = buf.rate as f64;
            let a = (((clip.start - t0) * rate as f64).ceil().max(0.0)) as usize;
            let b = (((clip.end() - t0) * rate as f64).floor().max(0.0) as usize).min(n);
            for (i, f) in frame.iter_mut().enumerate().take(b).skip(a) {
                let t = t0 + i as f64 / rate as f64;
                let src = (clip.timeline_to_source(t) * mr).round();
                if src < 0.0 || src as usize >= buf.frames() {
                    continue;
                }
                let s = src as usize;
                let g = clip.gain_at(t);
                let l = buf.channels[0][s];
                let r = if ch > 1 { buf.channels[1][s] } else { l };
                f[0] += l * g;
                f[1] += r * g;
            }
        }
        each(frame);
        done += n;
    }
}

/// The track's level in 10 ms windows (dBFS RMS), from `from` to `to`.
pub fn dry_levels(
    track: &Track,
    media: &dyn Fn(MediaId) -> Option<Arc<AudioBuffer>>,
    rate: u32,
    from: f64,
    to: f64,
) -> Vec<f32> {
    let win = ((WIN * rate as f64).round() as usize).max(1);
    let mut out = Vec::new();
    let (mut acc, mut k) = (0.0f64, 0usize);
    each_dry_chunk(track, media, rate, from, to, |frames| {
        for f in frames {
            acc += (f[0] as f64 * f[0] as f64 + f[1] as f64 * f[1] as f64) * 0.5;
            k += 1;
            if k == win {
                out.push((10.0 * (acc / win as f64).max(1e-20).log10()) as f32);
                acc = 0.0;
                k = 0;
            }
        }
    });
    out
}

/// Integrated loudness of the track's clips as recorded.
pub fn dry_loudness(
    track: &Track,
    media: &dyn Fn(MediaId) -> Option<Arc<AudioBuffer>>,
    rate: u32,
    from: f64,
    to: f64,
) -> Option<f64> {
    let mut m = LiveLoudness::new(rate);
    each_dry_chunk(track, media, rate, from, to, |f| m.feed(f));
    m.integrated()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DuckSettings {
    /// The voice counts as talking above this (dBFS RMS).
    pub threshold_db: f32,
    /// How far the bed goes down.
    pub reduce_db: f32,
    /// Fade down, ending as the voice starts.
    pub fade_down: f64,
    /// Fade back up, starting as the voice stops.
    pub fade_up: f64,
    /// Gaps in the talking shorter than this stay ducked.
    pub bridge: f64,
}

impl Default for DuckSettings {
    fn default() -> Self {
        Self {
            threshold_db: -40.0,
            reduce_db: -15.0,
            fade_down: 0.3,
            fade_up: 0.8,
            bridge: 1.0,
        }
    }
}

/// Where the talking is, in seconds, from window levels starting at
/// `from` (several voices: the loudest at each moment).
pub fn talking(levels: &[f32], from: f64, s: &DuckSettings) -> Vec<(f64, f64)> {
    let mut runs: Vec<(f64, f64)> = Vec::new();
    let mut i = 0;
    while i < levels.len() {
        if levels[i] > s.threshold_db {
            let mut j = i;
            while j < levels.len() && levels[j] > s.threshold_db {
                j += 1;
            }
            let (a, b) = (from + i as f64 * WIN, from + j as f64 * WIN);
            // A blip under 50 ms is a click, not talking.
            if b - a >= 0.05 {
                match runs.last_mut() {
                    Some(l) if a - l.1 < s.bridge => l.1 = b,
                    _ => runs.push((a, b)),
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    runs
}

/// The dip, as (time, dB) points: 0 dB away from the talking.
pub fn duck_curve(runs: &[(f64, f64)], s: &DuckSettings, from: f64, to: f64) -> Vec<(f64, f64)> {
    let r = s.reduce_db as f64;
    let mut pts: Vec<(f64, f64)> = Vec::new();
    // Join runs whose fades would meet.
    let mut joined: Vec<(f64, f64)> = Vec::new();
    for &(a, b) in runs {
        match joined.last_mut() {
            Some(l) if a - s.fade_down <= l.1 + s.fade_up => l.1 = b,
            _ => joined.push((a, b)),
        }
    }
    for (a, b) in joined {
        let down = (a - s.fade_down).max(from);
        let up = (b + s.fade_up).min(to);
        if down > from || a > from {
            pts.push((down, 0.0));
        }
        pts.push((a.max(from), r));
        pts.push((b.min(to), r));
        if up > b {
            pts.push((up, 0.0));
        }
    }
    pts
}

/// Value of a piecewise-linear curve (holding at its ends; 0 if empty).
fn curve_at(pts: &[(f64, f64)], t: f64) -> f64 {
    match pts.iter().position(|p| p.0 > t) {
        None => pts.last().map_or(0.0, |p| p.1),
        Some(0) => pts[0].1,
        Some(i) => {
            let (a, b) = (pts[i - 1], pts[i]);
            let k = if b.0 > a.0 { (t - a.0) / (b.0 - a.0) } else { 1.0 };
            a.1 + (b.1 - a.1) * k
        }
    }
}

/// Writes the dip into the track's volume lane, on top of what is there,
/// between `from` and `to`.
pub fn write_duck(track: &mut Track, dip: &[(f64, f64)], from: f64, to: f64) {
    if dip.is_empty() {
        return;
    }
    let idx = match track.lanes.iter().position(|l| l.target == LaneTarget::Volume) {
        Some(i) => i,
        None => {
            track.lanes.push(Lane::new(LaneTarget::Volume));
            track.lanes.len() - 1
        }
    };
    let lane = &mut track.lanes[idx];
    let had: Vec<LanePoint> = lane.points.clone();
    let base = |t: f64| af27_core::automation::value_at(&had, t).unwrap_or(0.0);
    let mut times: Vec<f64> = dip.iter().map(|p| p.0).collect();
    times.extend(had.iter().map(|p| p.time).filter(|t| (from..=to).contains(t)));
    times.push(from);
    times.push(to);
    times.sort_by(f64::total_cmp);
    times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    let pts: Vec<LanePoint> = times
        .into_iter()
        .map(|t| LanePoint {
            time: t,
            value: (base(t) + curve_at(dip, t)).max(af27_core::automation::SILENT_DB - 0.1),
            curve: Default::default(),
        })
        .collect();
    lane.replace_span(from, to, &pts);
    lane.enabled = true;
    track.show_lanes = true;
}

/// The clip gain change for each track to reach `target` (None: the
/// quietest of them). Changes under 0.5 LU are left out.
pub fn balance(loud: &[(TrackId, f64)], target: Option<f64>) -> Vec<(TrackId, f32)> {
    let Some(goal) = target.or_else(|| loud.iter().map(|l| l.1).min_by(f64::total_cmp)) else {
        return Vec::new();
    };
    loud.iter()
        .map(|(id, l)| (*id, (goal - l) as f32))
        .filter(|(_, d)| d.abs() >= 0.5)
        .collect()
}

/// A guess from the name: music, beds, themes and effects tracks are
/// what gets ducked; the rest are voices.
pub fn sounds_like_music(name: &str) -> bool {
    let n = name.to_lowercase();
    ["music", "bed", "theme", "intro", "outro", "jingle", "sfx", "effect", "ambien", "song", "stinger"]
        .iter()
        .any(|w| n.contains(w))
}

// ---------------------------------------------------------------- UI

#[derive(Default)]
pub struct Dialogs {
    pub duck_open: bool,
    pub balance_open: bool,
    pub duck: DuckSettings,
    /// Track ids ticked as voices and as what gets ducked.
    voices: Vec<TrackId>,
    ducked: Vec<TrackId>,
    /// Speaker Balance: tracks ticked, and the level to reach (None:
    /// the quietest).
    balance_tracks: Vec<TrackId>,
    pub balance_target: Option<f64>,
    seeded_for: u64,
}

impl Dialogs {
    fn seed(&mut self, s: &Session, version: u64) {
        if self.seeded_for == version + 1 {
            return;
        }
        self.seeded_for = version + 1;
        let audio: Vec<&Track> = s.tracks.iter().filter(|t| t.kind == TrackKind::Audio && !t.clips.is_empty()).collect();
        self.voices = audio.iter().filter(|t| !sounds_like_music(&t.name)).map(|t| t.id).collect();
        self.ducked = audio.iter().filter(|t| sounds_like_music(&t.name)).map(|t| t.id).collect();
        self.balance_tracks = self.voices.clone();
    }
}

fn span(st: &State) -> (f64, f64) {
    match st.session_sel {
        Some((a, b)) if b - a > 1e-3 => (a, b),
        _ => (0.0, st.session.duration()),
    }
}

fn track_ticks(ui: &mut egui::Ui, id: &str, s: &Session, list: &mut Vec<TrackId>, skip: &[TrackId]) {
    egui::ScrollArea::vertical().id_salt(id).max_height(140.0).show(ui, |ui| {
        for t in s.tracks.iter().filter(|t| t.kind == TrackKind::Audio && !skip.contains(&t.id)) {
            let mut on = list.contains(&t.id);
            if ui.checkbox(&mut on, &t.name).changed() {
                if on {
                    list.push(t.id);
                } else {
                    list.retain(|x| *x != t.id);
                }
            }
        }
    });
}

pub fn show(ctx: &egui::Context, st: &mut State) {
    let mut d = std::mem::take(&mut st.podcast);
    if d.duck_open || d.balance_open {
        d.seed(&st.session, st.session_version);
    } else {
        d.seeded_for = 0;
    }
    let th = st.theme.clone();
    if d.duck_open {
        let mut open = true;
        let mut run = false;
        egui::Window::new("Auto Duck").open(&mut open).resizable(false).show(ctx, |ui| {
            ui.label(RichText::new("Writes volume automation that dips the beds under the talking.").size(11.0).color(c(th.text_dim)));
            ui.columns(2, |col| {
                col[0].label(RichText::new("Voices").strong());
                track_ticks(&mut col[0], "duck-v", &st.session, &mut d.voices, &d.ducked.clone());
                col[1].label(RichText::new("Duck these").strong());
                track_ticks(&mut col[1], "duck-m", &st.session, &mut d.ducked, &d.voices.clone());
            });
            egui::Grid::new("duck-set").num_columns(2).show(ui, |ui| {
                let s = &mut d.duck;
                ui.label("Talking above");
                ui.add(egui::DragValue::new(&mut s.threshold_db).range(-80.0..=-10.0).speed(0.2).suffix(" dB"));
                ui.end_row();
                ui.label("Turn down by");
                ui.add(egui::DragValue::new(&mut s.reduce_db).range(-40.0..=-1.0).speed(0.2).suffix(" dB"));
                ui.end_row();
                ui.label("Fade down");
                ui.add(egui::DragValue::new(&mut s.fade_down).range(0.0..=5.0).speed(0.01).suffix(" s"));
                ui.end_row();
                ui.label("Fade up");
                ui.add(egui::DragValue::new(&mut s.fade_up).range(0.0..=10.0).speed(0.01).suffix(" s"));
                ui.end_row();
                ui.label("Stay down through gaps under");
                ui.add(egui::DragValue::new(&mut s.bridge).range(0.0..=10.0).speed(0.01).suffix(" s"));
                ui.end_row();
            });
            let (a, b) = span(st);
            ui.label(RichText::new(format!(
                "Over {} ({}).",
                if st.session_sel.is_some_and(|(x, y)| y - x > 1e-3) { "the time selection" } else { "the whole session" },
                af27_core::time::format_time(b - a)
            )).size(10.0).color(c(th.text_dim)));
            if ui.add_enabled(!d.voices.is_empty() && !d.ducked.is_empty(), egui::Button::new("Duck")).clicked() {
                run = true;
            }
        });
        if run {
            run_duck(st, &d);
            open = false;
        }
        d.duck_open = open;
    }
    if d.balance_open {
        let mut open = true;
        let mut run = false;
        egui::Window::new("Speaker Balance").open(&mut open).resizable(false).show(ctx, |ui| {
            ui.label(RichText::new("Brings every voice to the same loudness with clip gain, measured from the recordings themselves.").size(11.0).color(c(th.text_dim)));
            track_ticks(ui, "bal-t", &st.session, &mut d.balance_tracks, &[]);
            ui.horizontal(|ui| {
                let mut fixed = d.balance_target.is_some();
                ui.radio_value(&mut fixed, false, "Match the quietest");
                ui.radio_value(&mut fixed, true, "Match");
                if fixed {
                    let mut v = d.balance_target.unwrap_or(-19.0);
                    ui.add(egui::DragValue::new(&mut v).range(-40.0..=-6.0).speed(0.1).suffix(" LUFS"));
                    d.balance_target = Some(v);
                } else {
                    d.balance_target = None;
                }
            });
            if ui.add_enabled(d.balance_tracks.len() >= if d.balance_target.is_some() { 1 } else { 2 }, egui::Button::new("Balance")).clicked() {
                run = true;
            }
        });
        if run {
            run_balance(st, &d);
            open = false;
        }
        d.balance_open = open;
    }
    st.podcast = d;
}

fn run_duck(st: &mut State, d: &Dialogs) {
    let (from, to) = span(st);
    let rate = st.session.rate;
    let media = |m: MediaId| st.media_buffer(m);
    // The loudest voice at each moment.
    let mut levels: Vec<f32> = Vec::new();
    for t in st.session.tracks.iter().filter(|t| d.voices.contains(&t.id)) {
        let lv = dry_levels(t, &media, rate, from, to);
        if levels.len() < lv.len() {
            levels.resize(lv.len(), -200.0);
        }
        for (a, b) in levels.iter_mut().zip(lv) {
            *a = a.max(b);
        }
    }
    let runs = talking(&levels, from, &d.duck);
    if runs.is_empty() {
        st.toast("No talking found above the level set: nothing ducked.");
        return;
    }
    let dip = duck_curve(&runs, &d.duck, from, to);
    let before = st.session.clone();
    for t in st.session.tracks.iter_mut().filter(|t| d.ducked.contains(&t.id)) {
        write_duck(t, &dip, from, to);
    }
    st.commit_session("Auto Duck", before);
    st.toast(format!("Ducked under {} stretches of talking", runs.len()));
}

fn run_balance(st: &mut State, d: &Dialogs) {
    let (from, to) = span(st);
    let rate = st.session.rate;
    let media = |m: MediaId| st.media_buffer(m);
    let loud: Vec<(TrackId, f64)> = st
        .session
        .tracks
        .iter()
        .filter(|t| d.balance_tracks.contains(&t.id))
        .filter_map(|t| dry_loudness(t, &media, rate, from, to).map(|l| (t.id, l)))
        .collect();
    let changes = balance(&loud, d.balance_target);
    if changes.is_empty() {
        st.toast("The voices are already within half a LU of each other.");
        return;
    }
    let before = st.session.clone();
    for (id, delta) in &changes {
        if let Some(t) = st.session.track_mut(*id) {
            for c in &mut t.clips {
                c.gain_db += delta;
            }
        }
    }
    st.commit_session("Speaker Balance", before);
    st.toast(format!("Changed clip gain on {} tracks", changes.len()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn talking_bridges_short_gaps_and_ignores_blips() {
        let mut lv = vec![-60.0f32; 100];
        lv.extend(vec![-20.0; 100]); // 1..2 s
        lv.extend(vec![-60.0; 50]); // 0.5 s gap: bridged
        lv.extend(vec![-20.0; 100]); // 2.5..3.5 s
        lv.extend(vec![-60.0; 300]);
        lv.extend(vec![-20.0; 2]); // 20 ms blip
        lv.extend(vec![-60.0; 100]);
        let r = talking(&lv, 0.0, &DuckSettings::default());
        assert_eq!(r.len(), 1, "{r:?}");
        assert!((r[0].0 - 1.0).abs() < 1e-6 && (r[0].1 - 3.5).abs() < 1e-6);
    }

    #[test]
    fn the_dip_fades_down_before_and_up_after() {
        let s = DuckSettings::default();
        let pts = duck_curve(&[(2.0, 4.0)], &s, 0.0, 10.0);
        assert_eq!(pts, vec![(1.7, 0.0), (2.0, -15.0), (4.0, -15.0), (4.8, 0.0)]);
        assert_eq!(curve_at(&pts, 3.0), -15.0);
        assert_eq!(curve_at(&pts, 0.5), 0.0);
        assert!((curve_at(&pts, 4.4) + 7.5).abs() < 1e-9);
        // Runs whose fades would meet become one dip.
        let pts = duck_curve(&[(2.0, 4.0), (4.9, 6.0)], &s, 0.0, 10.0);
        assert_eq!(pts.len(), 4);
    }

    #[test]
    fn ducking_keeps_existing_automation_underneath() {
        let mut t = Session::new("t", 48_000, 1).tracks[0].clone();
        let mut lane = Lane::new(LaneTarget::Volume);
        lane.points = vec![
            LanePoint { time: 0.0, value: -3.0, curve: Default::default() },
            LanePoint { time: 10.0, value: -3.0, curve: Default::default() },
        ];
        t.lanes.push(lane);
        let dip = duck_curve(&[(2.0, 4.0)], &DuckSettings::default(), 0.0, 10.0);
        write_duck(&mut t, &dip, 0.0, 10.0);
        let l = &t.lanes[0];
        assert_eq!(l.value_at(3.0), Some(-18.0));
        assert_eq!(l.value_at(6.0), Some(-3.0));
        assert!(t.show_lanes);
    }

    #[test]
    fn balance_brings_voices_to_the_quietest_or_a_level() {
        let loud = vec![(1, -18.0), (2, -24.0), (3, -24.3)];
        let b = balance(&loud, None);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].0, 1);
        assert!((b[0].1 + 6.3).abs() < 1e-4);
        let b = balance(&loud, Some(-19.0));
        assert_eq!(b.len(), 3);
    }

    #[test]
    fn dry_levels_follow_the_clips() {
        let mut t = Session::new("t", 48_000, 1).tracks[0].clone();
        t.clips.push(af27_core::Clip {
            props: Default::default(),
            id: 1,
            media: 7,
            name: "c".into(),
            start: 1.0,
            offset: 0.0,
            length: 1.0,
            gain_db: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
            ext_id: None,
        });
        let buf = Arc::new(AudioBuffer {
            rate: 48_000,
            channels: vec![vec![0.5; 48_000]],
        });
        let media = |_m: MediaId| Some(buf.clone());
        let lv = dry_levels(&t, &media, 48_000, 0.0, 3.0);
        assert_eq!(lv.len(), 300);
        assert!(lv[50] < -100.0 && lv[250] < -100.0);
        assert!((lv[150] - 20.0 * 0.5f32.log10()).abs() < 0.1, "{}", lv[150]);
        assert!(sounds_like_music("Music Bed") && !sounds_like_music("Guest"));
    }
}
