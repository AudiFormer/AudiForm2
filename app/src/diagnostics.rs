// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Diagnostics: scan the file in the Waveform editor (or its selection)
//! for long pauses, clicks, clipped peaks or the parts with sound, review
//! what was found, then fix it: delete or turn down the pauses (keeping a
//! short gap so speech does not run together), redraw clicks and clipped
//! peaks, or drop range markers. Every fix is one undoable edit.
//!
//! The finders are the ones the Click Repair and Clip Repair effects use
//! (`af27_dsp::repair`), so the scan finds what the effects would fix.

use af27_core::time::format_time;
use af27_core::{AudioBuffer, Marker};
use af27_dsp::repair::{self, SilenceRule, Span};
use egui::RichText;

use crate::state::{DocId, Mode, State};
use crate::theme::c;

/// Window the silence scan measures levels in.
const WINDOW_MS: f32 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Silence,
    Clicks,
    Clipping,
    Audio,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Silence, Kind::Clicks, Kind::Clipping, Kind::Audio];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Silence => "Pauses and Dead Air",
            Kind::Clicks => "Clicks and Mouth Noise",
            Kind::Clipping => "Clipped Peaks",
            Kind::Audio => "Mark Sound",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Kind::Silence => "Pause",
            Kind::Clicks => "Click",
            Kind::Clipping => "Clip",
            Kind::Audio => "Sound",
        }
    }
}

/// What fixing a pause does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseFix {
    /// Cut it out, leaving `keep_ms` of it.
    Delete,
    /// Lower it by `turn_down_db`, nothing moves.
    TurnDown,
    /// Drop a range marker over it.
    Mark,
}

impl PauseFix {
    const ALL: [PauseFix; 3] = [PauseFix::Delete, PauseFix::TurnDown, PauseFix::Mark];
    fn name(self) -> &'static str {
        match self {
            PauseFix::Delete => "Delete (keep a short gap)",
            PauseFix::TurnDown => "Turn down",
            PauseFix::Mark => "Mark",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub quiet_db: f32,
    pub quiet_ms: f32,
    pub sound_db: f32,
    pub sound_ms: f32,
    pub keep_ms: f32,
    pub turn_down_db: f32,
    pub pause_fix: PauseFix,
    pub sensitivity: f32,
    pub click_ms: f32,
    pub clip_db: f32,
    pub clip_ms: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            quiet_db: -50.0,
            quiet_ms: 700.0,
            sound_db: -40.0,
            sound_ms: 250.0,
            keep_ms: 250.0,
            turn_down_db: -15.0,
            pause_fix: PauseFix::Delete,
            sensitivity: 50.0,
            click_ms: 1.0,
            clip_db: -0.3,
            clip_ms: 3.0,
        }
    }
}

/// Starting points for the pause scan: (name, quiet dB, quiet ms, sound
/// dB, sound ms, keep ms, fix).
pub const PAUSE_PRESETS: [(&str, f32, f32, f32, f32, f32, PauseFix); 4] = [
    ("Trim long pauses", -50.0, 1200.0, -40.0, 250.0, 400.0, PauseFix::Delete),
    ("Tighten a conversation", -48.0, 600.0, -40.0, 300.0, 180.0, PauseFix::Delete),
    ("Strip dead air", -60.0, 2000.0, -50.0, 200.0, 500.0, PauseFix::Delete),
    ("Turn down pauses and breaths", -38.0, 250.0, -30.0, 150.0, 0.0, PauseFix::TurnDown),
];

/// One thing found, in seconds from the start of the file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Finding {
    pub start: f64,
    pub len: f64,
    pub fixed: bool,
    pub picked: bool,
}

impl Finding {
    fn end(&self) -> f64 {
        self.start + self.len
    }
}

/// The panel's state.
pub struct Diagnostics {
    pub kind: Kind,
    pub set: Settings,
    pub found: Vec<Finding>,
    /// The file scanned and its version after the scan (or our last fix),
    /// so an edit made elsewhere shows the list is out of date.
    pub scanned: Option<(DocId, u64)>,
    pub show_settings: bool,
}

impl Default for Diagnostics {
    fn default() -> Self {
        Self {
            kind: Kind::Silence,
            set: Settings::default(),
            found: Vec::new(),
            scanned: None,
            show_settings: false,
        }
    }
}

// ------------------------------------------------------------- logic

fn rule(set: &Settings, rate: u32) -> SilenceRule {
    let win = |ms: f32| (ms / WINDOW_MS).round().max(1.0) as usize;
    let _ = rate;
    SilenceRule {
        quiet_db: set.quiet_db,
        min_quiet: win(set.quiet_ms),
        sound_db: set.sound_db,
        min_sound: win(set.sound_ms),
    }
}

fn window(rate: u32) -> usize {
    ((WINDOW_MS / 1000.0) * rate as f32).round().max(1.0) as usize
}

/// Merges spans found on several channels into one list.
fn merge(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by_key(|s| s.start);
    let mut out: Vec<Span> = Vec::new();
    for s in spans {
        match out.last_mut() {
            Some(l) if s.start <= l.end() => {
                let end = l.end().max(s.end());
                l.len = end - l.start;
            }
            _ => out.push(s),
        }
    }
    out
}

/// Scans frames `from..to` of `buf`.
pub fn scan(buf: &AudioBuffer, from: usize, to: usize, kind: Kind, set: &Settings) -> Vec<Finding> {
    let to = to.min(buf.frames());
    if from >= to {
        return Vec::new();
    }
    let rate = buf.rate;
    let chans: Vec<&[f32]> = buf.channels.iter().map(|c| &c[from..to]).collect();
    let (spans, unit) = match kind {
        Kind::Silence | Kind::Audio => {
            let w = window(rate);
            let levels = repair::window_levels(&chans, w);
            let r = rule(set, rate);
            let s = if kind == Kind::Silence {
                repair::find_silences(&levels, r)
            } else {
                repair::find_sound(&levels, r)
            };
            (s, w)
        }
        Kind::Clicks => (
            merge(
                chans
                    .iter()
                    .flat_map(|c| repair::find_clicks(c, rate, set.sensitivity, set.click_ms))
                    .collect(),
            ),
            1,
        ),
        Kind::Clipping => (
            merge(
                chans
                    .iter()
                    .flat_map(|c| repair::find_clipped(c, rate, set.clip_db, set.clip_ms))
                    .collect(),
            ),
            1,
        ),
    };
    let len = to - from;
    spans
        .into_iter()
        .map(|s| {
            let a = (s.start * unit).min(len);
            let b = (s.end() * unit).min(len);
            Finding {
                start: (from + a) as f64 / rate as f64,
                len: (b - a) as f64 / rate as f64,
                fixed: false,
                picked: false,
            }
        })
        .filter(|f| f.len > 0.0)
        .collect()
}

/// Suggested quiet and sound levels for the pause scan, from the audio.
pub fn suggest(buf: &AudioBuffer, from: usize, to: usize) -> Option<(f32, f32)> {
    let to = to.min(buf.frames());
    if from >= to {
        return None;
    }
    let chans: Vec<&[f32]> = buf.channels.iter().map(|c| &c[from..to]).collect();
    repair::suggest_levels(&repair::window_levels(&chans, window(buf.rate)))
}

/// What a fix did.
#[derive(Debug, Default)]
pub struct Outcome {
    /// The new audio (None when only markers changed).
    pub buf: Option<AudioBuffer>,
    pub markers: Vec<Marker>,
    /// Audio taken out: (where, in the old timing, and how long).
    pub removed: Vec<(f64, f64)>,
}

/// Fixes `items` (start, length in seconds) found by a scan of `kind`.
pub fn fix(buf: &AudioBuffer, kind: Kind, set: &Settings, items: &[(f64, f64)], first_no: usize) -> Outcome {
    let rate = buf.rate as f64;
    let fr = |t: f64| ((t * rate).round().max(0.0) as usize).min(buf.frames());
    let mut items: Vec<(f64, f64)> = items.to_vec();
    items.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mark = |items: &[(f64, f64)]| -> Vec<Marker> {
        items
            .iter()
            .enumerate()
            .map(|(i, (a, l))| Marker {
                name: format!("{} {:02}", kind.noun(), first_no + i),
                time: *a,
                length: *l,
                ext_id: None,
                kind: af27_core::session::MarkerKind::Range,
            })
            .collect()
    };
    match kind {
        Kind::Audio => Outcome {
            markers: mark(&items),
            ..Default::default()
        },
        Kind::Silence if set.pause_fix == PauseFix::Mark => Outcome {
            markers: mark(&items),
            ..Default::default()
        },
        Kind::Clicks | Kind::Clipping => {
            let mut b = buf.clone();
            for (a, l) in &items {
                let (s, e) = (fr(*a), fr(a + l));
                if e > s {
                    for ch in &mut b.channels {
                        repair::fill(ch, Span { start: s, len: e - s });
                    }
                }
            }
            Outcome {
                buf: Some(b),
                ..Default::default()
            }
        }
        Kind::Silence if set.pause_fix == PauseFix::TurnDown => {
            let mut b = buf.clone();
            let g = 10f32.powf(set.turn_down_db / 20.0);
            let ramp = (0.01 * rate) as usize;
            for (a, l) in &items {
                let (s, e) = (fr(*a), fr(a + l));
                let n = e.saturating_sub(s);
                let r = ramp.min(n / 2).max(1);
                for ch in &mut b.channels {
                    for (k, v) in ch[s..e].iter_mut().enumerate() {
                        let edge = k.min(n - 1 - k);
                        let t = (edge as f32 / r as f32).min(1.0);
                        *v *= 1.0 + (g - 1.0) * t;
                    }
                }
            }
            Outcome {
                buf: Some(b),
                ..Default::default()
            }
        }
        Kind::Silence => {
            // Delete, last first so earlier positions hold; a short
            // crossfade where the two sides meet.
            let mut b = buf.clone();
            let mut removed = Vec::new();
            let keep = set.keep_ms as f64 / 1000.0;
            let xf = (0.005 * rate) as usize;
            for (a, l) in items.iter().rev() {
                let k = keep.min(*l);
                let cut_a = a + k / 2.0;
                let cut_b = a + l - k / 2.0;
                let (s, e) = (fr(cut_a), fr(cut_b));
                if e <= s {
                    continue;
                }
                let x = xf.min(s).min(b.frames() - e);
                for ch in &mut b.channels {
                    for i in 0..x {
                        let t = (i as f32 + 0.5) / x as f32;
                        let out_side = ch[s - x + i];
                        let in_side = ch[e - x + i];
                        ch[s - x + i] = out_side * (1.0 - t) + in_side * t;
                    }
                }
                b.cut(af27_core::Range::new(s, e));
                removed.push((cut_a, (e - s) as f64 / rate));
            }
            removed.reverse();
            Outcome {
                buf: Some(b),
                removed,
                ..Default::default()
            }
        }
    }
}

/// Moves times after the audio `removed` earlier.
pub fn shift(t: f64, removed: &[(f64, f64)]) -> f64 {
    let mut out = t;
    for (at, len) in removed {
        if t >= at + len {
            out -= len;
        } else if t > *at {
            out -= t - at;
        }
    }
    out
}

// ---------------------------------------------------------------- UI

fn db_drag(ui: &mut egui::Ui, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, unit: &str) {
    ui.label(label);
    ui.add(egui::DragValue::new(v).range(range).speed(0.2).suffix(unit));
    ui.end_row();
}

pub fn panel(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    let dim = |t: &str| RichText::new(t).size(10.0).color(c(th.text_dim));
    if st.mode != Mode::Waveform || st.doc().is_none() {
        ui.label(dim(
            "Diagnostics scans and repairs the file open in the Waveform editor. Double-click a clip or a file to open it there.",
        ));
        return;
    }
    let mut d = std::mem::take(&mut st.diag);
    let (doc_id, version) = {
        let doc = st.doc().unwrap();
        (doc.id, doc.version)
    };
    let stale = d.scanned.is_some_and(|s| s != (doc_id, version)) && !d.found.is_empty();

    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("diag-kind")
            .selected_text(d.kind.name())
            .show_ui(ui, |ui| {
                for k in Kind::ALL {
                    if ui.selectable_value(&mut d.kind, k, k.name()).changed() {
                        d.found.clear();
                        d.scanned = None;
                    }
                }
            });
        ui.toggle_value(&mut d.show_settings, "⚙ Settings");
    });

    if d.show_settings {
        settings_ui(ui, st, &mut d);
    }

    ui.horizontal(|ui| {
        let what = if st.doc().unwrap().sel.is_some_and(|s| s.1 - s.0 > 1e-6) {
            "Scan Selection"
        } else {
            "Scan File"
        };
        if ui.button(what).clicked() {
            let doc = st.doc().unwrap();
            let r = doc.target();
            d.found = scan(&doc.buf, r.start, r.end, d.kind, &d.set);
            d.scanned = Some((doc_id, version));
        }
        let open = d.found.iter().filter(|f| !f.fixed).count();
        let fixed = d.found.len() - open;
        ui.label(dim(&format!("{} found, {fixed} done", d.found.len())));
    });
    if stale {
        ui.label(
            RichText::new("The file changed since the scan: scan again before fixing.")
                .size(10.0)
                .color(c(th.meter_high)),
        );
    }

    let verb = match d.kind {
        Kind::Silence => match d.set.pause_fix {
            PauseFix::Delete => "Delete",
            PauseFix::TurnDown => "Turn Down",
            PauseFix::Mark => "Mark",
        },
        Kind::Clicks | Kind::Clipping => "Repair",
        Kind::Audio => "Mark",
    };
    let mut act: Option<bool> = None; // Some(all)
    ui.horizontal(|ui| {
        let any_picked = d.found.iter().any(|f| f.picked && !f.fixed);
        let any_open = d.found.iter().any(|f| !f.fixed);
        if ui
            .add_enabled(!stale && any_picked, egui::Button::new(verb))
            .clicked()
        {
            act = Some(false);
        }
        if ui
            .add_enabled(!stale && any_open, egui::Button::new(format!("{verb} All")))
            .clicked()
        {
            act = Some(true);
        }
        if ui
            .add_enabled(d.found.iter().any(|f| f.fixed), egui::Button::new("Clear Done"))
            .clicked()
        {
            d.found.retain(|f| !f.fixed);
        }
    });

    let mut select: Option<(f64, f64)> = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("diag-found")
                .striped(true)
                .num_columns(4)
                .show(ui, |ui| {
                    for h in ["", "Start", "Length", ""] {
                        ui.label(dim(h));
                    }
                    ui.end_row();
                    let ctrl = ui.input(|i| i.modifiers.command);
                    let mut picked_now = None;
                    for (i, f) in d.found.iter().enumerate() {
                        ui.label(if f.fixed { "✔" } else { "" });
                        let r = ui.selectable_label(
                            f.picked,
                            RichText::new(format_time(f.start)).monospace(),
                        );
                        ui.label(RichText::new(format!("{:.3} s", f.len)).monospace());
                        ui.label("");
                        if r.clicked() {
                            picked_now = Some(i);
                            select = Some((f.start, f.end()));
                        }
                        ui.end_row();
                    }
                    if let Some(i) = picked_now {
                        if !ctrl {
                            d.found.iter_mut().for_each(|f| f.picked = false);
                        }
                        d.found[i].picked = !d.found[i].picked || !ctrl;
                    }
                });
        });

    if let Some((a, b)) = select {
        if let Some(doc) = st.doc_mut() {
            doc.sel = Some((a, b));
        }
        st.set_cursor(a);
    }

    if let Some(all) = act {
        apply(st, &mut d, all, verb);
    }
    st.diag = d;
}

fn apply(st: &mut State, d: &mut Diagnostics, all: bool, verb: &str) {
    let idx: Vec<usize> = d
        .found
        .iter()
        .enumerate()
        .filter(|(_, f)| !f.fixed && (all || f.picked))
        .map(|(i, _)| i)
        .collect();
    if idx.is_empty() {
        return;
    }
    let items: Vec<(f64, f64)> = idx.iter().map(|&i| (d.found[i].start, d.found[i].len)).collect();
    let Some(doc) = st.doc_mut() else {
        return;
    };
    let first_no = doc.markers.len() + 1;
    let out = fix(&doc.buf, d.kind, &d.set, &items, first_no);
    let n = items.len();
    if let Some(b) = out.buf {
        let label = format!("{verb} {} ({n})", d.kind.noun().to_lowercase());
        doc.set_buffer(&label, b);
    }
    if !out.markers.is_empty() {
        doc.markers.extend(out.markers);
        doc.markers.sort_by(|a, b| a.time.total_cmp(&b.time));
    }
    if !out.removed.is_empty() {
        for m in &mut doc.markers {
            m.time = shift(m.time, &out.removed);
        }
        doc.sel = None;
    }
    let version = doc.version;
    let id = doc.id;
    for &i in &idx {
        d.found[i].fixed = true;
        d.found[i].picked = false;
    }
    for f in &mut d.found {
        let end = shift(f.end(), &out.removed);
        f.start = shift(f.start, &out.removed);
        f.len = (end - f.start).max(0.0);
    }
    d.scanned = Some((id, version));
    st.toast(format!("{verb}: {n} done"));
}

fn settings_ui(ui: &mut egui::Ui, st: &mut State, d: &mut Diagnostics) {
    let s = &mut d.set;
    egui::Grid::new("diag-set").num_columns(2).show(ui, |ui| match d.kind {
        Kind::Silence | Kind::Audio => {
            if d.kind == Kind::Silence {
                ui.label("Preset");
                egui::ComboBox::from_id_salt("diag-preset")
                    .selected_text("Choose…")
                    .show_ui(ui, |ui| {
                        for p in PAUSE_PRESETS {
                            if ui.selectable_label(false, p.0).clicked() {
                                (s.quiet_db, s.quiet_ms, s.sound_db, s.sound_ms, s.keep_ms, s.pause_fix) =
                                    (p.1, p.2, p.3, p.4, p.5, p.6);
                            }
                        }
                    });
                ui.end_row();
            }
            db_drag(ui, "Quiet below", &mut s.quiet_db, -96.0..=-6.0, " dB");
            db_drag(ui, "Quiet for at least", &mut s.quiet_ms, 20.0..=30_000.0, " ms");
            db_drag(ui, "Sound above", &mut s.sound_db, -96.0..=-3.0, " dB");
            db_drag(ui, "Sound for at least", &mut s.sound_ms, 10.0..=5000.0, " ms");
            ui.label("");
            if ui.button("Find Levels").on_hover_text("Set the quiet and sound levels from this recording's own noise floor and speaking level").clicked() {
                if let Some(doc) = st.doc() {
                    let r = doc.target();
                    match suggest(&doc.buf, r.start, r.end) {
                        Some((q, l)) => {
                            s.quiet_db = q.round();
                            s.sound_db = l.round();
                        }
                        None => st.toast("Not enough difference between speech and pauses to suggest levels."),
                    }
                }
            }
            ui.end_row();
            if d.kind == Kind::Silence {
                ui.label("Fixing");
                egui::ComboBox::from_id_salt("diag-fix")
                    .selected_text(s.pause_fix.name())
                    .show_ui(ui, |ui| {
                        for f in PauseFix::ALL {
                            ui.selectable_value(&mut s.pause_fix, f, f.name());
                        }
                    });
                ui.end_row();
                match s.pause_fix {
                    PauseFix::Delete => db_drag(ui, "Keep a gap of", &mut s.keep_ms, 0.0..=2000.0, " ms"),
                    PauseFix::TurnDown => db_drag(ui, "Turn down by", &mut s.turn_down_db, -60.0..=-1.0, " dB"),
                    PauseFix::Mark => {}
                }
            }
        }
        Kind::Clicks => {
            db_drag(ui, "Sensitivity", &mut s.sensitivity, 0.0..=100.0, " %");
            ui.label("Longest click");
            ui.add(egui::DragValue::new(&mut s.click_ms).range(0.1..=3.0).speed(0.01).suffix(" ms"));
            ui.end_row();
        }
        Kind::Clipping => {
            ui.label("Clipped above");
            ui.add(egui::DragValue::new(&mut s.clip_db).range(-12.0..=0.0).speed(0.01).suffix(" dB"));
            ui.end_row();
            ui.label("Longest flat run");
            ui.add(egui::DragValue::new(&mut s.clip_ms).range(0.1..=10.0).speed(0.01).suffix(" ms"));
            ui.end_row();
        }
    });
    ui.separator();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Speech-like bursts with pauses: 1 s on, `gap` s off, 1 s on.
    fn talk(gap: f64) -> AudioBuffer {
        let rate = 48_000;
        let mut v = Vec::new();
        let tone = |n: usize, amp: f32| -> Vec<f32> {
            (0..n)
                .map(|i| (i as f32 * 0.05).sin() * amp)
                .collect()
        };
        v.extend(tone(rate, 0.3));
        v.extend(tone((gap * rate as f64) as usize, 0.0005));
        v.extend(tone(rate, 0.3));
        AudioBuffer {
            rate: rate as u32,
            channels: vec![v.clone(), v],
        }
    }

    #[test]
    fn finds_a_long_pause_and_deletes_it_keeping_a_gap() {
        let b = talk(2.0);
        let set = Settings::default();
        let f = scan(&b, 0, b.frames(), Kind::Silence, &set);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!((f[0].start - 1.0).abs() < 0.02 && (f[0].len - 2.0).abs() < 0.03);
        let out = fix(&b, Kind::Silence, &set, &[(f[0].start, f[0].len)], 1);
        let nb = out.buf.unwrap();
        let secs = nb.duration_secs();
        assert!((secs - (2.0 + 0.25)).abs() < 0.04, "{secs}");
        assert_eq!(out.removed.len(), 1);
        // A marker after the pause moves earlier by what was taken out.
        let moved = shift(3.5, &out.removed);
        assert!((moved - (3.5 - 1.75)).abs() < 0.03, "{moved}");
    }

    #[test]
    fn short_pauses_are_left_alone() {
        let b = talk(0.3);
        assert!(scan(&b, 0, b.frames(), Kind::Silence, &Settings::default()).is_empty());
        let snd = scan(&b, 0, b.frames(), Kind::Audio, &Settings::default());
        assert_eq!(snd.len(), 1, "one stretch of sound");
    }

    #[test]
    fn turn_down_keeps_the_length_and_marks_add_ranges() {
        let b = talk(2.0);
        let mut set = Settings {
            pause_fix: PauseFix::TurnDown,
            ..Default::default()
        };
        let f = scan(&b, 0, b.frames(), Kind::Silence, &set);
        let out = fix(&b, Kind::Silence, &set, &[(f[0].start, f[0].len)], 1);
        let nb = out.buf.unwrap();
        assert_eq!(nb.frames(), b.frames());
        let mid = 48_000 * 2;
        assert!((nb.channels[0][mid] / b.channels[0][mid] - 10f32.powf(-0.75)).abs() < 0.01);
        set.pause_fix = PauseFix::Mark;
        let out = fix(&b, Kind::Silence, &set, &[(f[0].start, f[0].len)], 3);
        assert!(out.buf.is_none());
        assert_eq!(out.markers[0].name, "Pause 03");
        assert!(out.markers[0].length > 1.9);
    }

    #[test]
    fn clicks_are_found_in_either_channel_and_repaired() {
        let mut b = talk(0.3);
        b.channels[1][30_000] += 0.8;
        b.channels[1][30_001] -= 0.7;
        let set = Settings::default();
        // (The test signal's hard on/off edges are clicks too.)
        let at = 30_000.0 / 48_000.0;
        let hit = |f: &Finding| f.start <= at && f.end() >= at;
        let f = scan(&b, 0, b.frames(), Kind::Clicks, &set);
        let one = *f.iter().find(|f| hit(f)).expect("the planted click");
        let out = fix(&b, Kind::Clicks, &set, &[(one.start, one.len)], 1);
        let nb = out.buf.unwrap();
        assert!(!scan(&nb, 0, nb.frames(), Kind::Clicks, &set).iter().any(hit));
    }

    #[test]
    fn a_selection_scan_reports_file_times() {
        let b = talk(2.0);
        let f = scan(&b, 24_000, b.frames(), Kind::Silence, &Settings::default());
        assert!((f[0].start - 1.0).abs() < 0.02);
    }
}
