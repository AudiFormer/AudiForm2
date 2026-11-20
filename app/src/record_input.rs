// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

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

use af27_core::TrackId;
use af27_engine::{
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
        let support = af27_engine::capture::support();
        let saved = std::fs::read_to_string(prefs_path()).unwrap_or_default();
        let default = match saved.trim() {
            "system" if support.loopback => RecordSource::SystemExceptSelf,
            "system-all" if support.loopback => RecordSource::System(None),
            _ => RecordSource::Input,
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
            RecordSource::SystemExceptSelf => "system",
            RecordSource::System(None) => "system-all",
            // Devices and applications change between runs.
            _ => "input",
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
                let apps = af27_engine::capture::list_audio_apps();
                let outs = af27_engine::capture::list_output_devices();
                let ins =
                    af27_engine::devices::list(af27_engine::HostKind::System, None, None).inputs;
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

/// Picks the sources to open for `target` and remembers which tracks get
/// which take.
pub fn begin(st: &mut State, target: RecordTarget) -> Vec<RecordSource> {
    let inputs = &mut st.rec_inputs;
    match target {
        RecordTarget::NewFile => {
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

/// Saves each take, opens it, and places it on its tracks.
pub fn finish(st: &mut State, takes: Vec<Take>, target: Option<RecordTarget>) {
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
        if to_track && buf.rate != st.session.rate && st.session.rate > 0 && buf.frames() > 0 {
            buf = af27_dsp::resample::resample(&buf, st.session.rate);
        }
        let n = st.docs.len() + 1;
        let suffix = if i == 0 {
            String::new()
        } else {
            format!("-{}", i + 1)
        };
        let path = dir.join(format!("Recording {stamp}{suffix}.wav"));
        let saved = af27_io::write_wav(&path, &buf, af27_io::WavFormat::Float32).is_ok();
        let id = st.add_doc(take_name(n, &take.source), saved.then_some(path), buf);
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
            let picks = af27_engine::routing::input_choices(d.channels);
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
    use af27_core::AudioBuffer;

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
        let r = af27_dsp::resample::resample(&b, 48_000);
        assert_eq!(r.rate, 48_000);
        assert_eq!(r.num_channels(), 2);
        assert!((r.frames() as i64 - 480).abs() <= 1);
    }
}
