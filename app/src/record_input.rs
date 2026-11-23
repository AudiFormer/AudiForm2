// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Where recordings come from: the microphone, everything the computer
//! plays, or chosen applications, per track.
//!
//! The source by the Record button is the default for the Waveform editor
//! and for every track that hasn't picked its own. Each armed track
//! records its own source at the same time (a game on one track, a voice
//! chat on another); tracks that share a source share one capture.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use af2_core::{AudioBuffer, TrackId};
use af2_engine::{
    AudioApp, CaptureSupport, DeviceInfo, InputPick, OutputDevice, RecordSource, Take,
};
use egui::{RichText, Ui};

use crate::state::{data_dir, Mode, RecordTarget, State};

type Listing = (
    Result<Vec<AudioApp>, String>,
    Result<Vec<OutputDevice>, String>,
    Vec<DeviceInfo>,
);

pub struct RecordInputs {
    /// The source by the Record button.
    pub default: RecordSource,
    /// Tracks that chose their own.
    pub per_track: HashMap<TrackId, RecordSource>,
    /// While recording: the tracks each take goes to, in source order.
    plan: Vec<Vec<TrackId>>,
    support: CaptureSupport,
    apps: Vec<AudioApp>,
    outputs: Vec<OutputDevice>,
    /// The system driver's input devices and their channels.
    inputs: Vec<DeviceInfo>,
    /// On ASIO: the driver's inputs (the only input device there).
    asio: Option<DeviceInfo>,
    list_error: Option<String>,
    listed_at: Option<Instant>,
    pending: Option<mpsc::Receiver<Listing>>,
}

impl RecordInputs {
    /// While recording: the tracks take `i` goes to.
    pub fn tracks_of_take(&self, i: usize) -> &[TrackId] {
        self.plan.get(i).map_or(&[], Vec::as_slice)
    }
}

impl Default for RecordInputs {
    fn default() -> Self {
        Self::load()
    }
}

fn prefs_path() -> std::path::PathBuf {
    data_dir().join("record_source.txt")
}

impl RecordInputs {
    pub fn load() -> RecordInputs {
        let support = af2_engine::capture::support();
        let saved = std::fs::read_to_string(prefs_path()).unwrap_or_default();
        let default = match saved.trim() {
            "system" if support.loopback => RecordSource::SystemExceptSelf,
            "system-all" if support.loopback => RecordSource::System(None),
            other => parse_channels(other).map_or(RecordSource::Input, RecordSource::Channels),
        };
        RecordInputs {
            default,
            per_track: HashMap::new(),
            plan: Vec::new(),
            support,
            apps: Vec::new(),
            outputs: Vec::new(),
            inputs: Vec::new(),
            asio: None,
            list_error: None,
            listed_at: None,
            pending: None,
        }
    }

    fn save_default(&self) {
        let v = match &self.default {
            RecordSource::SystemExceptSelf => "system".to_string(),
            RecordSource::System(None) => "system-all".to_string(),
            // Input channels (a mic on input 1 of a four-input interface)
            // are kept: every track that follows the Record button records
            // them from then on. If the device is gone, recording says so.
            RecordSource::Channels(p) => channels_line(p),
            // Applications change between runs.
            _ => "input".to_string(),
        };
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(prefs_path(), v);
    }

    /// The source `track` records from.
    pub fn source_for(&self, track: TrackId) -> &RecordSource {
        self.per_track.get(&track).unwrap_or(&self.default)
    }

    /// Lists applications and outputs on a worker thread; results land in
    /// [`Self::poll`]. Stale lists (over 3 s old) are refreshed when a menu
    /// opens.
    fn refresh(&mut self, force: bool) {
        if self.pending.is_some() {
            return;
        }
        let fresh = self
            .listed_at
            .is_some_and(|t| t.elapsed() < Duration::from_secs(3));
        if fresh && !force {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("af2-list-sources".into())
            .spawn(move || {
                let apps = af2_engine::capture::list_audio_apps();
                let outs = af2_engine::capture::list_output_devices();
                let ins =
                    af2_engine::devices::list(af2_engine::HostKind::System, None, None).inputs;
                let _ = tx.send((apps, outs, ins));
            });
        match spawned {
            Ok(_) => self.pending = Some(rx),
            Err(e) => self.list_error = Some(format!("Could not list sources: {e}")),
        }
    }

    fn poll(&mut self) {
        let Some(rx) = &self.pending else { return };
        match rx.try_recv() {
            Ok((apps, outs, ins)) => {
                self.inputs = ins;
                self.pending = None;
                self.listed_at = Some(Instant::now());
                self.list_error = None;
                match apps {
                    Ok(a) => self.apps = a,
                    Err(e) => self.list_error = Some(e),
                }
                match outs {
                    Ok(o) => self.outputs = o,
                    Err(e) => self.list_error = Some(e),
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending = None;
                self.list_error = Some("Listing sources failed.".into());
            }
        }
    }
}

impl RecordInputs {
    /// Follows the driver the output opened with: on ASIO the track input
    /// menus offer the driver's inputs, elsewhere the system's devices.
    pub fn follow_driver(&mut self, asio: Option<&DeviceInfo>) {
        if self.asio.as_ref() != asio {
            self.asio = asio.cloned();
        }
    }

    /// The input devices the menus offer, with their channels.
    pub fn input_devices(&self) -> Vec<DeviceInfo> {
        match &self.asio {
            Some(a) => vec![a.clone()],
            None => self.inputs.clone(),
        }
    }
}

/// Same capture? Applications match by process, whatever their state.
fn same_source(a: &RecordSource, b: &RecordSource) -> bool {
    match (a, b) {
        (RecordSource::App(x), RecordSource::App(y)) => x.pid == y.pid,
        (RecordSource::System(Some(x)), RecordSource::System(Some(y))) => x.id == y.id,
        _ => a == b,
    }
}

/// Groups `tracks` by the source each records, keeping first-seen order.
pub fn plan_sources(
    tracks: &[TrackId],
    source: impl Fn(TrackId) -> RecordSource,
) -> Vec<(RecordSource, Vec<TrackId>)> {
    let mut out: Vec<(RecordSource, Vec<TrackId>)> = Vec::new();
    for &t in tracks {
        let s = source(t);
        match out.iter_mut().find(|(o, _)| same_source(o, &s)) {
            Some((_, ts)) => ts.push(t),
            None => out.push((s, vec![t])),
        }
    }
    out
}

/// The name a take gets: "Recording 3", or "Recording 3 (Discord)" for a
/// system source.
pub fn take_name(n: usize, source: &RecordSource) -> String {
    if source.is_system() {
        format!("Recording {n} ({})", source.label())
    } else {
        format!("Recording {n}")
    }
}

/// The tracks to hear live: in the Multitrack, every audio track with its
/// monitor button on that is armed, each with the input it records from.
pub fn monitor_lanes(st: &State) -> Vec<af2_engine::MonitorLane> {
    if st.mode != Mode::Multitrack {
        return Vec::new();
    }
    st.session
        .tracks
        .iter()
        .filter(|t| t.monitor && t.armed && t.kind == af2_core::TrackKind::Audio)
        .map(|t| af2_engine::MonitorLane {
            track: t.id,
            source: st.rec_inputs.source_for(t.id).clone(),
        })
        .collect()
}

/// Every frame: hands the engine the tracks to monitor (it opens and
/// closes inputs only when they change) and shows why one can't be heard.
pub fn sync_monitor(st: &mut State) {
    let lanes = monitor_lanes(st);
    let Some(e) = &mut st.engine else { return };
    e.set_monitor(lanes);
    if let Some(note) = e.take_monitor_note() {
        st.error(note);
    }
}

/// Picks the sources to open for `target` and remembers which tracks get
/// which take.
pub fn begin(st: &mut State, target: RecordTarget) -> Vec<RecordSource> {
    let inputs = &mut st.rec_inputs;
    match target {
        RecordTarget::File(..) => {
            inputs.plan = vec![Vec::new()];
            vec![inputs.default.clone()]
        }
        RecordTarget::Track(first, _) => {
            let mut tracks: Vec<TrackId> = st
                .session
                .tracks
                .iter()
                .filter(|t| t.armed)
                .map(|t| t.id)
                .collect();
            if tracks.is_empty() {
                tracks.push(first);
            }
            let plan = plan_sources(&tracks, |t| inputs.source_for(t).clone());
            inputs.plan = plan.iter().map(|(_, ts)| ts.clone()).collect();
            plan.into_iter().map(|(s, _)| s).collect()
        }
    }
}

/// Below this peak a channel counts as silent (-66 dBFS: an unplugged
/// input's hiss, well under any real signal).
const SILENT_PEAK: f32 = 0.0005;

/// A two-channel take that holds one signal, as one channel: one side
/// silent while the other has sound (that side is kept), or both sides
/// the same. None when it really is stereo (or silent, or not two
/// channels).
pub fn mono_if_one_sided(buf: &AudioBuffer) -> Option<AudioBuffer> {
    if buf.num_channels() != 2 || buf.frames() == 0 {
        return None;
    }
    let peak = |c: &[f32]| c.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let (l, r) = (&buf.channels[0], &buf.channels[1]);
    let (pl, pr) = (peak(l), peak(r));
    let keep = if pl.max(pr) < SILENT_PEAK {
        return None;
    } else if pr < SILENT_PEAK {
        l
    } else if pl < SILENT_PEAK {
        r
    } else if l.iter().zip(r).all(|(a, b)| (a - b).abs() <= 1e-6) {
        l
    } else {
        return None;
    };
    Some(AudioBuffer { rate: buf.rate, channels: vec![keep.clone()] })
}

/// `buf` with `channels` channels at `rate`: resampled, and channels
/// shared out (mono to every channel, several averaged into mono, the rest
/// taken in order with the last repeated).
pub fn fit_take(buf: AudioBuffer, rate: u32, channels: usize) -> AudioBuffer {
    let mut buf = if buf.rate != rate && rate > 0 && buf.frames() > 0 {
        af2_dsp::resample::resample(&buf, rate)
    } else {
        buf
    };
    let have = buf.num_channels();
    let channels = channels.max(1);
    if have == channels || have == 0 {
        return buf;
    }
    let frames = buf.frames();
    let chans: Vec<Vec<f32>> = if channels == 1 {
        let mut m = vec![0.0f32; frames];
        for c in &buf.channels {
            for (o, s) in m.iter_mut().zip(c) {
                *o += s / have as f32;
            }
        }
        vec![m]
    } else {
        (0..channels).map(|k| buf.channels[k.min(have - 1)].clone()).collect()
    };
    buf.channels = chans;
    buf
}

/// The first take into the open file `doc` from `at` seconds: an empty
/// file takes it whole, otherwise it records over what is there and runs
/// on past the end. One undo step.
fn take_into_file(st: &mut State, doc: crate::state::DocId, at: f64, take: AudioBuffer) -> bool {
    let Some(d) = st.docs.iter_mut().find(|d| d.id == doc) else {
        return false;
    };
    let rate = d.buf.rate;
    let channels = d.buf.num_channels();
    let take = fit_take(take, rate, channels);
    if d.buf.frames() == 0 {
        d.set_buffer("Record", take);
    } else {
        let start = ((at.max(0.0) * rate as f64).round() as usize).min(d.buf.frames());
        let mut b = (*d.buf).clone();
        let end = start + take.frames();
        for (c, src) in b.channels.iter_mut().zip(&take.channels) {
            if c.len() < end {
                c.resize(end, 0.0);
            }
            c[start..end].copy_from_slice(src);
        }
        d.set_buffer("Record", b);
    }
    st.current = Some(doc);
    st.mode = Mode::Waveform;
    true
}

/// Writes a take's file in the background: the take is open and playable
/// at once, its file name appears when the write is done (a long take used
/// to freeze the window while it was encoded).
fn save_in_background(st: &mut State, doc: crate::state::DocId, path: std::path::PathBuf, bits: u16) {
    let Some(buf) = st.docs.iter().find(|d| d.id == doc).map(|d| d.buf.clone()) else {
        return;
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let p = path.clone();
    std::thread::spawn(move || {
        let _ = tx.send(crate::new_dialogs::write_lossless(&p, &buf, bits));
    });
    st.take_saves.push((doc, path, rx));
}

/// Takes whose files finished writing: the file takes its path.
pub fn poll_saves(st: &mut State) {
    if st.take_saves.is_empty() {
        return;
    }
    let mut left = Vec::new();
    for (doc, path, rx) in std::mem::take(&mut st.take_saves) {
        match rx.try_recv() {
            Ok(Ok(())) => {
                if let Some(d) = st.docs.iter_mut().find(|d| d.id == doc) {
                    if d.path.is_none() {
                        d.path = Some(path);
                    }
                }
            }
            Ok(Err(e)) => st.error(format!("The recording could not be saved: {e}")),
            Err(std::sync::mpsc::TryRecvError::Empty) => left.push((doc, path, rx)),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                st.error("The recording could not be saved.".to_string());
            }
        }
    }
    st.take_saves = left;
}

/// Saves each take, opens it, and places it on its tracks.
pub fn finish(st: &mut State, takes: Vec<Take>, target: Option<RecordTarget>) {
    let mut takes = takes;
    // Into the open file: the first take goes there (the rest, from other
    // inputs, open as files of their own).
    if let Some(RecordTarget::File(doc, at)) = target {
        if !takes.is_empty() {
            let first = takes.remove(0);
            if let Some(n) = &first.note {
                st.toast(format!("{}: {n}", first.source.label()));
            }
            if take_into_file(st, doc, at, first.buf) {
                if takes.is_empty() {
                    return;
                }
            }
        }
    }
    let plan = std::mem::take(&mut st.rec_inputs.plan);
    let dir = data_dir().join("recordings");
    let _ = std::fs::create_dir_all(&dir);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut first_doc = None;
    for (i, take) in takes.into_iter().enumerate() {
        if let Some(n) = &take.note {
            st.toast(format!("{}: {n}", take.source.label()));
        }
        let mut buf = take.buf;
        let to_track = matches!(target, Some(RecordTarget::Track(..)));
        // A mic in one input of a stereo interface: the other side is
        // silent (or both are the same), so the file is kept mono.
        if to_track {
            if let Some(m) = mono_if_one_sided(&buf) {
                buf = m;
            }
        }
        if to_track && buf.rate != st.session.rate && st.session.rate > 0 && buf.frames() > 0 {
            buf = af2_dsp::resample::resample(&buf, st.session.rate);
        }
        let n = st.docs.len() + 1;
        let suffix = if i == 0 {
            String::new()
        } else {
            format!("-{}", i + 1)
        };
        let ext = crate::new_dialogs::lossless_ext(&buf);
        let path = dir.join(format!("Recording {stamp}{suffix}.{ext}"));
        let bits = if to_track { st.session.bit_depth } else { 24 };
        let id = st.add_doc(take_name(n, &take.source), None, buf);
        save_in_background(st, id, path, bits);
        first_doc.get_or_insert(id);
        if let Some(RecordTarget::Track(_, at)) = target {
            for &t in plan.get(i).map(Vec::as_slice).unwrap_or(&[]) {
                st.place_doc(id, t, at);
            }
        }
    }
    if !matches!(target, Some(RecordTarget::Track(..))) {
        if let Some(id) = first_doc {
            st.current = Some(id);
            st.mode = Mode::Waveform;
        }
    }
}

// ----------------------------------------------------------------- UI

/// At most `n` characters, with an ellipsis when cut.
fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn short(s: &RecordSource) -> String {
    let t: String = match s {
        RecordSource::Input => "Mic".into(),
        RecordSource::Channels(p) => p.channels_label(),
        RecordSource::SystemExceptSelf => "System".into(),
        RecordSource::System(None) => "System (all)".into(),
        RecordSource::System(Some(d)) => d.name.clone(),
        RecordSource::App(a) => a.name.clone(),
    };
    clip(&t, 18)
}

const PER_APP_NOTE: &str = "Recording one application needs Windows 10 build 20348 or \
     Windows 11. This system can record the whole desktop only.";

/// The menu of sources. `per_track`: a track's own menu, which also offers
/// going back to the default. Returns the choice, if one was made.
fn source_menu(
    ui: &mut Ui,
    inputs: &mut RecordInputs,
    current: &RecordSource,
    per_track: bool,
) -> Option<Option<RecordSource>> {
    inputs.poll();
    inputs.refresh(false);
    let mut pick: Option<Option<RecordSource>> = None;
    if per_track {
        let d = short(&inputs.default);
        if ui
            .button(format!("Same as Record button ({d})"))
            .on_hover_text("Use the source chosen by the Record button")
            .clicked()
        {
            pick = Some(None);
            ui.close_menu();
        }
        ui.separator();
    }
    let mut item = |ui: &mut Ui, s: RecordSource, text: &str, tip: &str| {
        let on = same_source(current, &s);
        if ui.selectable_label(on, text).on_hover_text(tip).clicked() {
            pick = Some(Some(s));
            ui.close_menu();
        }
    };
    item(
        ui,
        RecordSource::Input,
        "Microphone / line in",
        "The default recording device",
    );
    let devices = inputs.input_devices();
    ui.menu_button("Input channels…", |ui| {
        if devices.is_empty() {
            let text = if inputs.pending.is_some() {
                "Listing…"
            } else {
                "No recording devices were found."
            };
            ui.label(RichText::new(text).weak());
        }
        for d in &devices {
            let picks = af2_engine::routing::input_choices(d.channels);
            let text = format!("{} ({} in)", d.name, d.channels);
            ui.menu_button(text, |ui| {
                for channels in picks {
                    let p = InputPick {
                        device: Some(d.name.clone()),
                        channels,
                    };
                    let label = p.channels_label();
                    let tip = format!("Record {}", p.label());
                    item(ui, RecordSource::Channels(p), &label, &tip);
                }
            });
        }
    });
    ui.separator();
    ui.label(RichText::new("System audio").strong());
    let sup = inputs.support;
    if !sup.loopback {
        ui.label(RichText::new("Only available on Windows.").weak());
        return pick;
    }
    if sup.per_app {
        item(
            ui,
            RecordSource::SystemExceptSelf,
            "Everything (except AudiForm 2)",
            "Everything the computer plays, without AudiForm 2's own playback, \
             so monitoring doesn't feed back",
        );
    }
    item(
        ui,
        RecordSource::System(None),
        "Everything",
        "Everything the default output device plays, AudiForm 2 included",
    );
    let outputs = inputs.outputs.clone();
    ui.menu_button("Output device…", |ui| {
        if outputs.is_empty() {
            ui.label(RichText::new("Listing…").weak());
        }
        for d in outputs {
            let text = if d.is_default {
                format!("{} (default)", d.name)
            } else {
                d.name.clone()
            };
            item(
                ui,
                RecordSource::System(Some(d)),
                &text,
                "Everything this output device plays",
            );
        }
    });
    ui.menu_button("Application…", |ui| {
        if !sup.per_app {
            ui.label(RichText::new(PER_APP_NOTE).weak());
            return;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Apps with sound").weak());
            if ui
                .small_button("⟳ Refresh")
                .on_hover_text("List again")
                .clicked()
            {
                inputs.refresh(true);
            }
        });
        if let Some(e) = &inputs.list_error {
            ui.label(RichText::new(e).weak());
        }
        if inputs.apps.is_empty() {
            let text = if inputs.pending.is_some() {
                "Listing…"
            } else {
                "No application has an audio session yet. Start playing \
                 something, then Refresh."
            };
            ui.label(RichText::new(text).weak());
        }
        for a in inputs.apps.clone() {
            let text = if a.active {
                format!("▶ {}", a.name)
            } else {
                format!("   {}", a.name)
            };
            let tip = format!(
                "{} (process {}), with its child processes{}",
                a.exe,
                a.pid,
                if a.active { "; playing now" } else { "" }
            );
            item(ui, RecordSource::App(a), &text, &tip);
        }
    });
    if !sup.per_app {
        ui.label(RichText::new(PER_APP_NOTE).weak().small());
    }
    pick
}

/// The saved form of an input-channels pick: `channels<TAB>device<TAB>0,1`
/// (an empty device is the one in Settings).
fn channels_line(p: &InputPick) -> String {
    let ch: Vec<String> = p.channels.iter().map(|c| c.to_string()).collect();
    format!("channels\t{}\t{}", p.device.clone().unwrap_or_default(), ch.join(","))
}

fn parse_channels(line: &str) -> Option<InputPick> {
    let mut it = line.split('\t');
    if it.next()? != "channels" {
        return None;
    }
    let device = it.next()?.to_string();
    let channels: Vec<u16> = it.next()?.split(',').filter_map(|c| c.trim().parse().ok()).collect();
    if channels.is_empty() || channels.len() > 2 {
        return None;
    }
    Some(InputPick { device: (!device.is_empty()).then_some(device), channels })
}

/// A track's input chooser, for the track header.
pub fn track_input(ui: &mut Ui, inputs: &mut RecordInputs, track: TrackId) {
    let own = inputs.per_track.get(&track).cloned();
    let current = own.clone().unwrap_or_else(|| inputs.default.clone());
    let text = match &own {
        Some(s) => short(s),
        None => format!("{} (default)", short(&current)),
    };
    ui.label(RichText::new("In").size(10.0).weak());
    let r = ui.menu_button(RichText::new(text).size(10.0), |ui| {
        ui.set_min_width(220.0);
        source_menu(ui, inputs, &current, true)
    });
    r.response
        .on_hover_text(format!("Records from: {}", current.label()));
    match r.inner.flatten() {
        Some(Some(s)) => {
            inputs.per_track.insert(track, s);
        }
        Some(None) => {
            inputs.per_track.remove(&track);
        }
        None => {}
    }
}

/// The default source, next to the Record button.
pub fn default_input(ui: &mut Ui, inputs: &mut RecordInputs) {
    let current = inputs.default.clone();
    let r = ui.menu_button(RichText::new("⏷").size(11.0), |ui| {
        ui.set_min_width(220.0);
        ui.label(RichText::new("Record from").strong());
        source_menu(ui, inputs, &current, false)
    });
    r.response.on_hover_text(format!(
        "Record from: {} (tracks can choose their own)",
        current.label()
    ));
    if let Some(Some(s)) = r.inner.flatten() {
        inputs.default = s;
        inputs.save_default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_input_channels_default_is_kept_between_runs() {
        let p = InputPick { device: Some("SSL 2+ (ASIO)".into()), channels: vec![0] };
        assert_eq!(parse_channels(&channels_line(&p)), Some(p));
        let q = InputPick { device: None, channels: vec![2, 3] };
        assert_eq!(parse_channels(&channels_line(&q)), Some(q));
        assert_eq!(parse_channels("input"), None);
        assert_eq!(parse_channels("channels\tx\t"), None);
    }

    #[test]
    fn a_mic_in_one_input_of_a_stereo_interface_records_mono() {
        let n = 1000;
        let sig: Vec<f32> = (0..n).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let hiss: Vec<f32> = (0..n).map(|i| if i % 2 == 0 { 0.0001 } else { -0.0001 }).collect();
        let b = |l: &Vec<f32>, r: &Vec<f32>| AudioBuffer { rate: 48_000, channels: vec![l.clone(), r.clone()] };
        // Input 1 only (input 2 open, with a little hiss): the left, mono.
        assert_eq!(mono_if_one_sided(&b(&sig, &hiss)).unwrap().channels, vec![sig.clone()]);
        // Input 2 only: the right.
        assert_eq!(mono_if_one_sided(&b(&hiss, &sig)).unwrap().channels, vec![sig.clone()]);
        // The same on both sides: one of them.
        assert_eq!(mono_if_one_sided(&b(&sig, &sig)).unwrap().num_channels(), 1);
        // Real stereo, silence, and mono takes stay as they are.
        let other: Vec<f32> = (0..n).map(|i| (i as f32 * 0.07).cos() * 0.3).collect();
        assert!(mono_if_one_sided(&b(&sig, &other)).is_none());
        assert!(mono_if_one_sided(&b(&hiss, &hiss)).is_none());
        assert!(mono_if_one_sided(&AudioBuffer { rate: 48_000, channels: vec![sig] }).is_none());
    }
    use af2_core::AudioBuffer;

    fn app(pid: u32, name: &str, active: bool) -> RecordSource {
        RecordSource::App(AudioApp {
            pid,
            name: name.into(),
            exe: format!("{name}.exe"),
            active,
        })
    }

    #[test]
    fn tracks_sharing_a_source_share_one_capture() {
        let mut per = HashMap::new();
        per.insert(2, app(10, "Game", true));
        per.insert(3, app(20, "Discord", true));
        // Same app, seen while idle: still the same capture.
        per.insert(4, app(10, "Game", false));
        let default = RecordSource::Input;
        let plan = plan_sources(&[1, 2, 3, 4, 5], |t| {
            per.get(&t).cloned().unwrap_or(default.clone())
        });
        let shape: Vec<(String, Vec<TrackId>)> =
            plan.iter().map(|(s, t)| (s.label(), t.clone())).collect();
        assert_eq!(
            shape,
            vec![
                ("Microphone / line in".to_string(), vec![1, 5]),
                ("Game".to_string(), vec![2, 4]),
                ("Discord".to_string(), vec![3]),
            ]
        );
    }

    #[test]
    fn system_and_except_self_are_different_captures() {
        let plan = plan_sources(&[1, 2], |t| {
            if t == 1 {
                RecordSource::SystemExceptSelf
            } else {
                RecordSource::System(None)
            }
        });
        assert_eq!(plan.len(), 2);
    }

    #[test]
    fn long_names_are_clipped() {
        assert_eq!(clip("Discord", 18), "Discord");
        assert_eq!(clip("Steam game: Baldur's Gate 3", 12), "Steam game:…");
    }

    #[test]
    fn take_names_say_where_system_audio_came_from() {
        assert_eq!(take_name(3, &RecordSource::Input), "Recording 3");
        assert_eq!(
            take_name(4, &app(1, "Discord", true)),
            "Recording 4 (Discord)"
        );
    }

    #[test]
    fn empty_takes_resample_safely() {
        let b = AudioBuffer {
            rate: 44_100,
            channels: vec![vec![0.0; 441], vec![0.0; 441]],
        };
        let r = af2_dsp::resample::resample(&b, 48_000);
        assert_eq!(r.rate, 48_000);
        assert_eq!(r.num_channels(), 2);
        assert!((r.frames() as i64 - 480).abs() <= 1);
    }
}

#[cfg(test)]
mod into_file_tests {
    use super::*;

    fn tone(rate: u32, ch: usize, frames: usize, v: f32) -> AudioBuffer {
        let mut b = AudioBuffer::new(rate, ch, frames);
        for c in &mut b.channels {
            c.fill(v);
        }
        b
    }

    #[test]
    fn armed_audio_tracks_with_monitor_on_are_heard_in_the_multitrack_only() {
        let mut st = State::new();
        st.mode = Mode::Multitrack;
        let a = st.session.add_track_of(af2_core::TrackKind::Audio);
        let b = st.session.add_track_of(af2_core::TrackKind::Audio);
        let m = st.session.add_track_of(af2_core::TrackKind::Midi);
        for id in [a, b, m] {
            let t = st.session.track_mut(id).unwrap();
            t.monitor = true;
            t.armed = id != b;
        }
        let pick = af2_engine::InputPick {
            device: Some("SSL 2+ ASIO".into()),
            channels: vec![0],
        };
        st.rec_inputs.per_track.insert(a, RecordSource::Channels(pick.clone()));
        let lanes = monitor_lanes(&st);
        // b waits for its arm; MIDI tracks don't take audio in.
        assert_eq!(lanes.len(), 1);
        assert_eq!(lanes[0].track, a);
        assert_eq!(lanes[0].source, RecordSource::Channels(pick));
        st.session.track_mut(a).unwrap().monitor = false;
        assert!(monitor_lanes(&st).is_empty());
        st.session.track_mut(a).unwrap().monitor = true;
        st.mode = Mode::Waveform;
        assert!(monitor_lanes(&st).is_empty());
    }

    #[test]
    fn record_with_no_file_asks_for_its_format_first() {
        let mut st = State::new();
        st.mode = Mode::Waveform;
        st.start_recording();
        assert!(!st.is_recording(), "nothing records before the file is made");
        assert!(st.new_file_ui.as_ref().is_some_and(|d| d.record_after), "the New Audio File dialog, to record");
    }

    #[test]
    fn a_four_input_take_gives_a_stereo_file_its_first_two_inputs() {
        let mut take = tone(48_000, 4, 100, 0.0);
        for (k, c) in take.channels.iter_mut().enumerate() {
            c.fill(k as f32 + 1.0);
        }
        let fitted = fit_take(take, 48_000, 2);
        assert_eq!(fitted.num_channels(), 2);
        assert_eq!((fitted.channels[0][5], fitted.channels[1][5]), (1.0, 2.0));
    }

    #[test]
    fn takes_are_fitted_to_the_file() {
        let up = fit_take(tone(48_000, 1, 480, 0.5), 48_000, 2);
        assert_eq!(up.num_channels(), 2);
        assert!(up.channels.iter().all(|c| c.iter().all(|&s| s == 0.5)), "mono to both sides");
        let mut st = tone(48_000, 2, 480, 0.0);
        st.channels[0].fill(1.0);
        let down = fit_take(st, 48_000, 1);
        assert_eq!(down.num_channels(), 1);
        assert!((down.channels[0][10] - 0.5).abs() < 1e-6, "sides averaged");
        let re = fit_take(tone(48_000, 2, 48_000, 0.25), 88_200, 2);
        assert_eq!(re.rate, 88_200);
        assert!((re.frames() as i64 - 88_200).abs() < 200, "{}", re.frames());
    }

    #[test]
    fn an_empty_file_takes_the_whole_take_and_a_full_one_is_recorded_over() {
        let mut st = State::new();
        let id = st.add_doc("Untitled".into(), None, AudioBuffer::new(88_200, 2, 0));
        assert!(take_into_file(&mut st, id, 0.0, tone(48_000, 2, 48_000, 0.5)));
        let d = st.docs.iter().find(|d| d.id == id).unwrap();
        assert_eq!(d.buf.rate, 88_200, "the file keeps its rate");
        assert!((d.buf.frames() as i64 - 88_200).abs() < 200);
        assert_eq!(st.current, Some(id));

        // A 1 s file, recorded over from 0.5 s with 1 s: 1.5 s, the first
        // half kept.
        let id2 = st.add_doc("Full".into(), None, tone(48_000, 1, 48_000, 0.1));
        assert!(take_into_file(&mut st, id2, 0.5, tone(48_000, 1, 48_000, 0.9)));
        let d = st.docs.iter_mut().find(|d| d.id == id2).unwrap();
        assert_eq!(d.buf.frames(), 72_000);
        assert_eq!(d.buf.channels[0][1_000], 0.1);
        assert_eq!(d.buf.channels[0][30_000], 0.9);
        assert_eq!(d.buf.channels[0][71_000], 0.9);
        // One undo step back to the original.
        assert!(d.undo());
        assert_eq!(d.buf.frames(), 48_000);
    }

    #[test]
    fn a_take_saves_in_the_background_and_the_file_gets_its_path() {
        let mut st = State::new();
        let id = st.add_doc("Recording 1".into(), None, tone(48_000, 2, 48_000, 0.25));
        let dir = std::env::temp_dir().join(format!("af2-takesave-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("take.flac");
        save_in_background(&mut st, id, path.clone(), 24);
        assert_eq!(st.docs.iter().find(|d| d.id == id).unwrap().path, None, "open at once");
        for _ in 0..200 {
            poll_saves(&mut st);
            if st.take_saves.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(st.take_saves.is_empty());
        assert_eq!(st.docs.iter().find(|d| d.id == id).unwrap().path.as_deref(), Some(path.as_path()));
        assert!(path.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Records a second for real through the machine's audio device with a
/// file open in the Waveform editor, the way Shift+Space does, and checks
/// the take went into that file. Run with --ignored (needs a device).
#[cfg(test)]
mod live_record_check {
    use super::*;

    #[test]
    #[ignore]
    fn shift_space_records_into_the_open_file() {
        let mut st = State::new();
        if st.engine.is_none() {
            eprintln!("no audio device");
            return;
        }
        let id = st.add_doc("Song".into(), None, AudioBuffer::new(48_000, 2, 48_000 * 3));
        st.current = Some(id);
        st.mode = Mode::Waveform;
        let before = st.docs.len();
        st.start_recording();
        eprintln!("target after start: {:?}, recording {}", st.recording_target, st.is_recording());
        let t0 = std::time::Instant::now();
        while t0.elapsed().as_secs_f64() < 1.2 {
            st.tick();
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        eprintln!("target before stop: {:?}", st.recording_target);
        st.stop_recording();
        eprintln!("docs {} -> {}; current {:?}; song frames {}", before, st.docs.len(), st.current,
            st.docs.iter().find(|d| d.id == id).map_or(0, |d| d.buf.frames()));
        assert_eq!(st.docs.len(), before, "no new file");
    }
}
