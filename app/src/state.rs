// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) The Audiform27 contributors.

//! Everything the app knows that is not drawing: open files, the
//! multitrack session, undo histories, the transport and background jobs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Instant;

use af27_core::session::{Marker, MediaItem};
use af27_core::{AudioBuffer, History, MediaId, Range, Session};
use af27_dsp::{Effect, Peaks};
use af27_engine::Engine;

use crate::theme::Theme;

pub type DocId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Mode {
    Waveform,
    Multitrack,
}

#[derive(Debug, Clone, Copy)]
pub struct View {
    pub start: f64,
    pub len: f64,
}

impl View {
    pub fn fit(dur: f64) -> View {
        View {
            start: 0.0,
            len: dur.max(0.5) * 1.02,
        }
    }
    pub fn zoom(&mut self, factor: f64, anchor: f64, total: f64) {
        let new_len = (self.len * factor).clamp(0.0005, (total * 1.5).max(1.0));
        let frac = ((anchor - self.start) / self.len).clamp(0.0, 1.0);
        self.start = (anchor - new_len * frac).max(0.0);
        self.len = new_len;
    }
}

/// One audio file open for editing.
pub struct Doc {
    pub id: DocId,
    pub name: String,
    pub path: Option<PathBuf>,
    pub buf: Arc<AudioBuffer>,
    pub peaks: Arc<Peaks>,
    pub history: History<Arc<AudioBuffer>>,
    pub dirty: bool,
    pub view: View,
    /// Time selection in seconds.
    pub sel: Option<(f64, f64)>,
    pub cursor: f64,
    pub markers: Vec<Marker>,
    /// Bumps on every change, for caches (peaks, spectrogram, program).
    pub version: u64,
    /// Spectral display open under the waveform.
    pub spectral: bool,
}

impl Doc {
    pub fn new(id: DocId, name: String, path: Option<PathBuf>, buf: AudioBuffer) -> Doc {
        let peaks = Arc::new(Peaks::build(&buf));
        let dur = buf.duration_secs();
        Doc {
            id,
            name,
            path,
            buf: Arc::new(buf),
            peaks,
            history: History::new(100),
            dirty: false,
            view: View::fit(dur),
            sel: None,
            cursor: 0.0,
            markers: Vec::new(),
            version: 1,
            spectral: true,
        }
    }

    pub fn duration(&self) -> f64 {
        self.buf.duration_secs()
    }

    pub fn frames_of(&self, (a, b): (f64, f64)) -> Range {
        let r = self.buf.rate as f64;
        Range::new(
            (a * r).round().max(0.0) as usize,
            (b * r).round().max(0.0) as usize,
        )
        .clamp(self.buf.frames())
    }

    /// The selection, or the whole file when nothing is selected.
    pub fn target(&self) -> Range {
        match self.sel {
            Some(s) if (s.1 - s.0).abs() > 1e-6 => self.frames_of(s),
            _ => Range::new(0, self.buf.frames()),
        }
    }

    /// Replaces the audio, keeping the old version for undo.
    pub fn set_buffer(&mut self, label: &str, buf: AudioBuffer) {
        self.history.push(label, self.buf.clone());
        self.buf = Arc::new(buf);
        self.after_change();
    }

    fn after_change(&mut self) {
        self.peaks = Arc::new(Peaks::build(&self.buf));
        self.dirty = true;
        self.version += 1;
        let d = self.duration();
        self.cursor = self.cursor.min(d);
        if let Some((a, b)) = self.sel {
            self.sel = Some((a.min(d), b.min(d)));
        }
    }

    pub fn undo(&mut self) -> bool {
        match self.history.undo(self.buf.clone()) {
            Some(b) => {
                self.buf = b;
                self.after_change();
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.history.redo(self.buf.clone()) {
            Some(b) => {
                self.buf = b;
                self.after_change();
                true
            }
            None => false,
        }
    }
}

pub enum Job {
    /// A file decoding in the background.
    Load {
        path: PathBuf,
        rx: Receiver<Result<AudioBuffer, String>>,
        /// Put it on this track at this time when done.
        place: Option<(af27_core::TrackId, f64)>,
    },
}

pub struct Toast {
    pub text: String,
    pub at: Instant,
    pub error: bool,
}

#[derive(Default)]
pub struct SeparationUi {
    pub model: Option<String>,
    pub gpu: bool,
    pub overlap: u32,
    pub job: Option<af27_separation::Shared>,
    pub job_kind: String,
    pub cancel: Arc<AtomicBool>,
    pub engine_version: Option<String>,
    /// Stems from the last run, ready to add to the session.
    pub last_outputs: Vec<(String, PathBuf)>,
}

pub struct GenerateUi {
    pub open: bool,
    pub kind: usize,
    pub wave: usize,
    pub noise: usize,
    pub freq: f64,
    pub secs: f64,
    pub db: f32,
}

pub struct State {
    pub mode: Mode,
    pub docs: Vec<Doc>,
    pub current: Option<DocId>,
    next_doc: DocId,
    pub jobs: Vec<Job>,

    pub session: Session,
    pub session_history: History<Session>,
    pub session_path: Option<PathBuf>,
    pub session_dirty: bool,
    pub session_version: u64,
    pub session_view: View,
    pub session_sel: Option<(f64, f64)>,
    pub session_cursor: f64,
    pub selected_clip: Option<af27_core::ClipId>,
    pub selected_track: Option<af27_core::TrackId>,
    /// Session media id to open document.
    pub media_docs: HashMap<MediaId, DocId>,

    pub engine: Option<Engine>,
    pub engine_error: Option<String>,
    program_key: (Mode, DocId, u64, u64),
    pub play_started_at: f64,
    pub looping: bool,
    pub return_on_stop: bool,
    pub meter: (f32, f32),
    pub meter_hold: (f32, f32),

    pub clipboard: Option<AudioBuffer>,
    pub effect: Effect,
    pub effect_values: HashMap<Effect, Vec<f32>>,
    pub separation: SeparationUi,
    pub sep_dirs: af27_separation::Dirs,
    pub generate: GenerateUi,

    pub theme: Theme,
    pub themes: Vec<Theme>,
    pub theme_changed: bool,
    pub toasts: Vec<Toast>,
    pub stats: Option<(String, af27_dsp::loudness::Stats)>,
    pub tool: Tool,
    pub show_about: bool,
    pub last_autosave: Instant,
    pub recording_target: Option<RecordTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Move,
    Razor,
    TimeSelect,
}

#[derive(Debug, Clone, Copy)]
pub enum RecordTarget {
    NewFile,
    Track(af27_core::TrackId, f64),
}

pub fn data_dir() -> PathBuf {
    af27_separation::data_root().join("Audiform27")
}

impl State {
    pub fn new() -> State {
        let (engine, engine_error) = match Engine::new() {
            Ok(e) => (Some(e), None),
            Err(e) => (None, Some(e)),
        };
        let themes = crate::theme::load_all(&data_dir().join("themes"));
        let saved_theme = std::fs::read_to_string(data_dir().join("theme.txt")).unwrap_or_default();
        let theme = themes
            .iter()
            .find(|t| t.name == saved_theme.trim())
            .cloned()
            .unwrap_or_default();
        let mut session = Session::default();
        for (i, t) in session.tracks.iter_mut().enumerate() {
            t.color = theme.track_colors[i % theme.track_colors.len().max(1)];
        }
        State {
            mode: Mode::Waveform,
            docs: Vec::new(),
            current: None,
            next_doc: 1,
            jobs: Vec::new(),
            session,
            session_history: History::new(200),
            session_path: None,
            session_dirty: false,
            session_version: 1,
            session_view: View::fit(60.0),
            session_sel: None,
            session_cursor: 0.0,
            selected_clip: None,
            selected_track: None,
            media_docs: HashMap::new(),
            engine,
            engine_error,
            program_key: (Mode::Waveform, 0, 0, 0),
            play_started_at: 0.0,
            looping: false,
            return_on_stop: true,
            meter: (0.0, 0.0),
            meter_hold: (0.0, 0.0),
            clipboard: None,
            effect: Effect::Amplify,
            effect_values: HashMap::new(),
            separation: SeparationUi {
                gpu: true,
                overlap: 2,
                ..Default::default()
            },
            sep_dirs: af27_separation::Dirs::detect(),
            generate: GenerateUi {
                open: false,
                kind: 0,
                wave: 0,
                noise: 1,
                freq: 440.0,
                secs: 5.0,
                db: -6.0,
            },
            theme,
            themes,
            theme_changed: true,
            toasts: Vec::new(),
            stats: None,
            tool: Tool::Move,
            show_about: false,
            last_autosave: Instant::now(),
            recording_target: None,
        }
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast {
            text: text.into(),
            at: Instant::now(),
            error: false,
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast {
            text: text.into(),
            at: Instant::now(),
            error: true,
        });
    }

    pub fn doc(&self) -> Option<&Doc> {
        let id = self.current?;
        self.docs.iter().find(|d| d.id == id)
    }

    pub fn doc_mut(&mut self) -> Option<&mut Doc> {
        let id = self.current?;
        self.docs.iter_mut().find(|d| d.id == id)
    }

    pub fn doc_by_id(&self, id: DocId) -> Option<&Doc> {
        self.docs.iter().find(|d| d.id == id)
    }

    pub fn add_doc(&mut self, name: String, path: Option<PathBuf>, buf: AudioBuffer) -> DocId {
        let id = self.next_doc;
        self.next_doc += 1;
        self.docs.push(Doc::new(id, name, path, buf));
        id
    }

    // ------------------------------------------------------------ files

    pub fn open_paths(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(ext.as_str(), "af27" | "sesx" | "audfrm" | "altaudio") {
                self.open_session(&p);
            } else {
                self.load_file(p, None);
            }
        }
    }

    /// Decodes a file in the background; it opens when ready.
    pub fn load_file(&mut self, path: PathBuf, place: Option<(af27_core::TrackId, f64)>) {
        if let Some(d) = self.docs.iter().find(|d| d.path.as_deref() == Some(&path)) {
            let id = d.id;
            if let Some((track, at)) = place {
                self.place_doc(id, track, at);
            } else {
                self.current = Some(id);
            }
            return;
        }
        let (tx, rx) = channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(af27_io::decode_file(&p));
        });
        self.jobs.push(Job::Load { path, rx, place });
    }

    /// Picks up finished background loads.
    pub fn poll_jobs(&mut self) {
        let mut done = Vec::new();
        self.jobs.retain(|j| match j {
            Job::Load { path, rx, place } => match rx.try_recv() {
                Ok(r) => {
                    done.push((path.clone(), r, *place));
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true,
                Err(_) => false,
            },
        });
        for (path, r, place) in done {
            match r {
                Ok(buf) => {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Audio")
                        .to_string();
                    let id = self.add_doc(name, Some(path.clone()), buf);
                    // Session media waiting on this file.
                    let waiting: Vec<MediaId> = self
                        .session
                        .media
                        .iter()
                        .filter(|m| m.path.as_deref() == Some(&path))
                        .map(|m| m.id)
                        .collect();
                    for m in waiting {
                        self.link_media(m, id);
                    }
                    if let Some((track, at)) = place {
                        self.place_doc(id, track, at);
                    } else if self.current.is_none() || self.mode == Mode::Waveform {
                        self.current = Some(id);
                    }
                }
                Err(e) => self.error(e),
            }
        }
    }

    pub fn loading(&self) -> Vec<String> {
        self.jobs
            .iter()
            .map(|Job::Load { path, .. }| {
                path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            })
            .collect()
    }

    fn link_media(&mut self, media: MediaId, doc: DocId) {
        let Some(d) = self.doc_by_id(doc) else { return };
        let (dur, rate, ch) = (d.duration(), d.buf.rate, d.buf.num_channels() as u16);
        if let Some(m) = self.session.media.iter_mut().find(|m| m.id == media) {
            m.duration = dur;
            m.rate = rate;
            m.channels = ch;
        }
        self.media_docs.insert(media, doc);
        self.session_version += 1;
    }

    /// The session media item for a document, adding one if needed.
    pub fn media_for_doc(&mut self, doc: DocId) -> Option<MediaId> {
        if let Some((&m, _)) = self.media_docs.iter().find(|(_, &d)| d == doc) {
            return Some(m);
        }
        let d = self.doc_by_id(doc)?;
        let item = MediaItem {
            id: 0,
            name: d.name.clone(),
            path: d.path.clone(),
            duration: d.duration(),
            rate: d.buf.rate,
            channels: d.buf.num_channels() as u16,
        };
        let id = self.session.alloc_id();
        self.session.media.push(MediaItem { id, ..item });
        self.media_docs.insert(id, doc);
        Some(id)
    }

    pub fn place_doc(&mut self, doc: DocId, track: af27_core::TrackId, at: f64) {
        let before = self.session.clone();
        if let Some(m) = self.media_for_doc(doc) {
            if let Some(c) = self.session.add_clip(track, m, at) {
                self.selected_clip = Some(c);
                self.commit_session("Add clip", before);
            }
        }
    }

    pub fn close_doc(&mut self, id: DocId) {
        let used = self.media_docs.values().any(|&d| d == id)
            && self
                .session
                .tracks
                .iter()
                .flat_map(|t| &t.clips)
                .any(|c| self.media_docs.get(&c.media) == Some(&id));
        if used {
            self.error("That file is used in the session. Remove its clips first.");
            return;
        }
        self.docs.retain(|d| d.id != id);
        self.media_docs.retain(|_, d| *d != id);
        if self.current == Some(id) {
            self.current = self.docs.last().map(|d| d.id);
        }
    }

    pub fn save_doc_as(&mut self, path: PathBuf, fmt: af27_io::WavFormat) {
        let Some(doc) = self.doc_mut() else { return };
        match af27_io::write_wav(&path, &doc.buf, fmt) {
            Ok(()) => {
                doc.path = Some(path.clone());
                doc.name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                doc.dirty = false;
                let n = doc.name.clone();
                self.toast(format!("Saved {n}"));
            }
            Err(e) => self.error(e),
        }
    }

    // ---------------------------------------------------------- session

    /// Records `before` as an undo step for the session.
    pub fn commit_session(&mut self, label: &str, before: Session) {
        self.session_history.push(label, before);
        self.session_dirty = true;
        self.session_version += 1;
    }

    pub fn open_session(&mut self, path: &Path) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let result = if ext == "af27" {
            std::fs::read_to_string(path)
                .map_err(|e| e.to_string())
                .and_then(|t| Session::from_json(&t))
                .map(|s| (s, Vec::new()))
        } else {
            af27_interop::import(path).map(|i| (i.session, i.notices))
        };
        match result {
            Ok((session, notices)) => {
                self.session = session;
                self.session_history.clear();
                self.session_path = (ext == "af27").then(|| path.to_path_buf());
                self.session_dirty = false;
                self.session_version += 1;
                self.media_docs.clear();
                self.selected_clip = None;
                self.session_view = View::fit(self.session.duration().max(30.0));
                self.mode = Mode::Multitrack;
                let media: Vec<_> = self.session.media.clone();
                let mut missing = 0;
                for m in media {
                    match &m.path {
                        Some(p) if p.exists() => {
                            if let Some(d) = self
                                .docs
                                .iter()
                                .find(|d| d.path.as_deref() == Some(p.as_path()))
                            {
                                let id = d.id;
                                self.link_media(m.id, id);
                            } else if !self.jobs.iter().any(|Job::Load { path, .. }| path == p) {
                                self.load_file(p.clone(), None);
                            }
                        }
                        _ => missing += 1,
                    }
                }
                self.toast(format!(
                    "Opened {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
                if missing > 0 {
                    self.error(format!("{missing} media file(s) could not be found."));
                }
                for n in notices {
                    self.toast(n);
                }
            }
            Err(e) => self.error(e),
        }
    }

    pub fn save_session_to(&mut self, path: PathBuf) {
        match std::fs::write(&path, self.session.to_json()) {
            Ok(()) => {
                self.session_path = Some(path.clone());
                self.session_dirty = false;
                self.toast(format!("Saved {}", path.display()));
            }
            Err(e) => self.error(format!("Could not save the session: {e}")),
        }
    }

    pub fn autosave(&mut self) {
        if self.last_autosave.elapsed().as_secs() < 120 {
            return;
        }
        self.last_autosave = Instant::now();
        if self.session_dirty {
            let dir = data_dir().join("autosave");
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("session.af27"), self.session.to_json());
        }
    }

    pub fn media_buffer(&self, m: MediaId) -> Option<Arc<AudioBuffer>> {
        let d = self.media_docs.get(&m)?;
        self.doc_by_id(*d).map(|d| d.buf.clone())
    }

    // -------------------------------------------------------- transport

    pub fn cursor(&self) -> f64 {
        match self.mode {
            Mode::Waveform => self.doc().map_or(0.0, |d| d.cursor),
            Mode::Multitrack => self.session_cursor,
        }
    }

    pub fn set_cursor(&mut self, t: f64) {
        match self.mode {
            Mode::Waveform => {
                if let Some(d) = self.doc_mut() {
                    d.cursor = t.clamp(0.0, d.duration());
                }
            }
            Mode::Multitrack => self.session_cursor = t.max(0.0),
        }
    }

    pub fn selection(&self) -> Option<(f64, f64)> {
        match self.mode {
            Mode::Waveform => self.doc().and_then(|d| d.sel),
            Mode::Multitrack => self.session_sel,
        }
        .filter(|s| (s.1 - s.0).abs() > 1e-6)
    }

    pub fn program_len(&self) -> f64 {
        match self.mode {
            Mode::Waveform => self.doc().map_or(0.0, |d| d.duration()),
            Mode::Multitrack => self.session.duration(),
        }
    }

    /// Hands the audio thread a fresh program when what would play changed.
    pub fn sync_program(&mut self) {
        let key = (
            self.mode,
            self.current.unwrap_or(0),
            self.doc().map_or(0, |d| d.version),
            self.session_version + self.docs.iter().map(|d| d.version).sum::<u64>(),
        );
        if key == self.program_key {
            return;
        }
        self.program_key = key;
        let program = match self.mode {
            Mode::Waveform => match self.doc() {
                Some(d) => af27_engine::buffer_program(d.buf.clone()),
                None => return,
            },
            Mode::Multitrack => af27_engine::mix_program(&self.session, &|m| self.media_buffer(m)),
        };
        if let Some(e) = &mut self.engine {
            e.set_program(program);
        }
    }

    pub fn is_playing(&self) -> bool {
        self.engine.as_ref().is_some_and(|e| e.is_playing())
    }

    pub fn play(&mut self) {
        self.sync_program();
        let len = self.program_len();
        let (from, to) = match self.selection() {
            Some((a, b)) => {
                let c = self.cursor();
                let start = if c >= a && c < b { c } else { a };
                (start, Some(b))
            }
            None => {
                let c = self.cursor();
                (if c >= len - 0.01 { 0.0 } else { c }, None)
            }
        };
        self.play_started_at = from;
        let looping = self.looping;
        if let Some(e) = &mut self.engine {
            e.play(from, to, looping, len);
        } else if let Some(err) = self.engine_error.clone() {
            self.error(err);
        }
    }

    pub fn stop(&mut self) {
        let pos = self.engine.as_ref().map_or(0.0, |e| e.position());
        if let Some(e) = &mut self.engine {
            e.stop();
        }
        let t = if self.return_on_stop {
            self.play_started_at
        } else {
            pos
        };
        self.set_cursor(t);
    }

    pub fn toggle_play(&mut self) {
        if self.is_recording() {
            self.stop_recording();
        } else if self.is_playing() {
            self.stop();
        } else {
            self.play();
        }
    }

    /// Called every frame: follow the playhead, meters, jobs.
    pub fn tick(&mut self) {
        self.poll_jobs();
        self.sync_program();
        if let Some(e) = &mut self.engine {
            e.collect();
            let (l, r) = e.take_peaks();
            let rec = e.recording_level();
            let (l, r) = if e.is_recording() { (rec, rec) } else { (l, r) };
            let fall = 0.9;
            self.meter = (l.max(self.meter.0 * fall), r.max(self.meter.1 * fall));
            self.meter_hold = (
                self.meter.0.max(self.meter_hold.0 * 0.995),
                self.meter.1.max(self.meter_hold.1 * 0.995),
            );
        }
        if self.is_playing() {
            let pos = self.engine.as_ref().map_or(0.0, |e| e.position());
            match self.mode {
                Mode::Waveform => {
                    if let Some(d) = self.doc_mut() {
                        d.cursor = pos;
                        if pos > d.view.start + d.view.len * 0.95 || pos < d.view.start {
                            d.view.start = (pos - d.view.len * 0.05).max(0.0);
                        }
                    }
                }
                Mode::Multitrack => {
                    self.session_cursor = pos;
                    let v = &mut self.session_view;
                    if pos > v.start + v.len * 0.95 || pos < v.start {
                        v.start = (pos - v.len * 0.05).max(0.0);
                    }
                }
            }
        }
        self.toasts
            .retain(|t| t.at.elapsed().as_secs_f32() < if t.error { 8.0 } else { 4.0 });
        self.autosave();
    }

    pub fn position(&self) -> f64 {
        if self.is_playing() {
            self.engine.as_ref().map_or(0.0, |e| e.position())
        } else if self.is_recording() {
            self.cursor() + self.engine.as_ref().map_or(0.0, |e| e.recording_len_secs())
        } else {
            self.cursor()
        }
    }

    // -------------------------------------------------------- recording

    pub fn is_recording(&self) -> bool {
        self.engine.as_ref().is_some_and(|e| e.is_recording())
    }

    pub fn start_recording(&mut self) {
        let target = match self.mode {
            Mode::Waveform => RecordTarget::NewFile,
            Mode::Multitrack => {
                let armed = self.session.tracks.iter().find(|t| t.armed).map(|t| t.id);
                match armed.or(self.selected_track) {
                    Some(t) => RecordTarget::Track(t, self.session_cursor),
                    None => {
                        self.error("Arm a track (R) to record onto it.");
                        return;
                    }
                }
            }
        };
        let Some(e) = &mut self.engine else { return };
        match e.start_recording() {
            Ok(()) => {
                self.recording_target = Some(target);
                if matches!(target, RecordTarget::Track(..)) {
                    self.play();
                }
            }
            Err(err) => self.error(err),
        }
    }

    pub fn stop_recording(&mut self) {
        let Some(e) = &mut self.engine else { return };
        let Some(buf) = e.stop_recording() else {
            return;
        };
        e.stop();
        let n = self.docs.len() + 1;
        let dir = data_dir().join("recordings");
        let _ = std::fs::create_dir_all(&dir);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = dir.join(format!("Recording {stamp}.wav"));
        let saved = af27_io::write_wav(&path, &buf, af27_io::WavFormat::Float32).is_ok();
        let id = self.add_doc(format!("Recording {n}"), saved.then_some(path), buf);
        match self.recording_target.take() {
            Some(RecordTarget::Track(t, at)) => self.place_doc(id, t, at),
            _ => {
                self.current = Some(id);
                self.mode = Mode::Waveform;
            }
        }
    }

    // ------------------------------------------------- waveform editing

    pub fn effect_values(&self, e: Effect) -> Vec<f32> {
        self.effect_values
            .get(&e)
            .cloned()
            .unwrap_or_else(|| e.defaults())
    }

    pub fn apply_effect(&mut self, e: Effect) {
        let vals = self.effect_values(e);
        let Some(doc) = self.doc_mut() else {
            self.error("Open a file in the Waveform editor first.");
            return;
        };
        let r = doc.target();
        if r.is_empty() {
            return;
        }
        let mut b = (*doc.buf).clone();
        let seg = b.slice(r);
        let out = e.process(&seg, &vals);
        let new_len = out.frames();
        b.replace(r, &out);
        doc.set_buffer(e.name(), b);
        if doc.sel.is_some() {
            let rate = doc.buf.rate as f64;
            doc.sel = Some((r.start as f64 / rate, (r.start + new_len) as f64 / rate));
        }
    }

    pub fn edit_copy(&mut self) {
        if let Some(d) = self.doc() {
            if d.sel.is_some() {
                let c = d.buf.slice(d.target());
                self.clipboard = Some(c);
            }
        }
    }

    pub fn edit_delete(&mut self, label: &str) {
        if self.mode == Mode::Multitrack {
            self.delete_clip();
            return;
        }
        if let Some(d) = self.doc_mut() {
            if d.sel.is_none() {
                return;
            }
            let r = d.target();
            let mut b = (*d.buf).clone();
            b.cut(r);
            d.set_buffer(label, b);
            d.cursor = r.start as f64 / d.buf.rate as f64;
            d.sel = None;
        }
    }

    pub fn edit_cut(&mut self) {
        self.edit_copy();
        self.edit_delete("Cut");
    }

    pub fn edit_paste(&mut self) {
        let Some(clip) = self.clipboard.clone() else {
            return;
        };
        let Some(d) = self.doc_mut() else { return };
        let mut c = af27_dsp::resample::resample(&clip, d.buf.rate);
        c = af27_dsp::resample::remix(&c, d.buf.num_channels());
        let mut b = (*d.buf).clone();
        let at = match d.sel {
            Some(_) => {
                let r = d.target();
                b.cut(r);
                r.start
            }
            None => (d.cursor * d.buf.rate as f64) as usize,
        };
        b.insert(at, &c);
        let rate = b.rate as f64;
        d.set_buffer("Paste", b);
        d.sel = Some((at as f64 / rate, (at + c.frames()) as f64 / rate));
    }

    pub fn edit_crop(&mut self) {
        if let Some(d) = self.doc_mut() {
            if d.sel.is_some() {
                let b = d.buf.slice(d.target());
                d.set_buffer("Crop", b);
                d.sel = None;
                d.cursor = 0.0;
                d.view = View::fit(d.duration());
            }
        }
    }

    pub fn insert_buffer(&mut self, label: &str, buf: AudioBuffer) {
        if self.doc().is_none() {
            let n = self.docs.len() + 1;
            let id = self.add_doc(
                format!("Untitled {n}"),
                None,
                AudioBuffer::new(buf.rate, buf.num_channels(), 0),
            );
            self.current = Some(id);
        }
        let d = self.doc_mut().unwrap();
        let c = af27_dsp::resample::remix(
            &af27_dsp::resample::resample(&buf, d.buf.rate),
            d.buf.num_channels(),
        );
        let mut b = (*d.buf).clone();
        let at = (d.cursor * d.buf.rate as f64) as usize;
        b.insert(at, &c);
        let rate = b.rate as f64;
        d.set_buffer(label, b);
        d.sel = Some((at as f64 / rate, (at + c.frames()) as f64 / rate));
        d.view = View::fit(d.duration());
    }

    pub fn new_file(&mut self, rate: u32, channels: usize) {
        let n = self.docs.len() + 1;
        let id = self.add_doc(
            format!("Untitled {n}"),
            None,
            AudioBuffer::new(rate, channels, 0),
        );
        self.current = Some(id);
        self.mode = Mode::Waveform;
    }

    pub fn undo(&mut self) {
        match self.mode {
            Mode::Waveform => {
                if let Some(d) = self.doc_mut() {
                    d.undo();
                }
            }
            Mode::Multitrack => {
                if let Some(s) = self.session_history.undo(self.session.clone()) {
                    self.session = s;
                    self.session_version += 1;
                    self.session_dirty = true;
                }
            }
        }
    }

    pub fn redo(&mut self) {
        match self.mode {
            Mode::Waveform => {
                if let Some(d) = self.doc_mut() {
                    d.redo();
                }
            }
            Mode::Multitrack => {
                if let Some(s) = self.session_history.redo(self.session.clone()) {
                    self.session = s;
                    self.session_version += 1;
                    self.session_dirty = true;
                }
            }
        }
    }

    pub fn select_all(&mut self) {
        match self.mode {
            Mode::Waveform => {
                if let Some(d) = self.doc_mut() {
                    d.sel = Some((0.0, d.duration()));
                }
            }
            Mode::Multitrack => self.session_sel = Some((0.0, self.session.duration())),
        }
    }

    pub fn add_marker(&mut self) {
        let t = self.position();
        match self.mode {
            Mode::Waveform => {
                if let Some(d) = self.doc_mut() {
                    let n = d.markers.len() + 1;
                    d.markers.push(Marker {
                        name: format!("Marker {n:02}"),
                        time: t,
                        length: 0.0,
                    });
                    d.markers.sort_by(|a, b| a.time.total_cmp(&b.time));
                }
            }
            Mode::Multitrack => {
                let before = self.session.clone();
                let n = self.session.markers.len() + 1;
                self.session.markers.push(Marker {
                    name: format!("Marker {n:02}"),
                    time: t,
                    length: 0.0,
                });
                self.session
                    .markers
                    .sort_by(|a, b| a.time.total_cmp(&b.time));
                self.commit_session("Add marker", before);
            }
        }
    }

    // ----------------------------------------------- multitrack editing

    pub fn split_at_playhead(&mut self) {
        let t = self.session_cursor;
        let before = self.session.clone();
        let targets: Vec<_> = match self.selected_clip {
            Some(c) => vec![c],
            None => self
                .session
                .tracks
                .iter()
                .flat_map(|tr| tr.clips.iter())
                .filter(|c| c.start < t && c.end() > t)
                .map(|c| c.id)
                .collect(),
        };
        let mut any = false;
        for c in targets {
            any |= self.session.split_clip(c, t).is_some();
        }
        if any {
            self.commit_session("Split", before);
        }
    }

    pub fn delete_clip(&mut self) {
        if let Some(c) = self.selected_clip.take() {
            let before = self.session.clone();
            if self.session.remove_clip(c).is_some() {
                self.commit_session("Delete clip", before);
            }
        }
    }

    pub fn add_track(&mut self) {
        let before = self.session.clone();
        let id = self.session.add_track();
        let n = self.session.tracks.len() - 1;
        if let Some(t) = self.session.track_mut(id) {
            t.color = self.theme.track_colors[n % self.theme.track_colors.len().max(1)];
        }
        self.commit_session("Add track", before);
    }

    pub fn export_mixdown(&mut self, path: PathBuf, fmt: af27_io::WavFormat) {
        let program = af27_engine::mix_program(&self.session, &|m| self.media_buffer(m));
        let (from, to) = self.session_sel.unwrap_or((0.0, self.session.duration()));
        let buf = af27_engine::render(&program, self.session.rate, from, to);
        match af27_io::write_wav(&path, &buf, fmt) {
            Ok(()) => self.toast(format!("Exported {}", path.display())),
            Err(e) => self.error(e),
        }
    }

    /// Opens the separated stems and puts each on a new track.
    pub fn stems_to_tracks(&mut self) {
        let outs = self.separation.last_outputs.clone();
        for (name, path) in outs {
            let before = self.session.clone();
            let id = self.session.add_track();
            let n = self.session.tracks.len() - 1;
            if let Some(t) = self.session.track_mut(id) {
                t.name = name;
                t.color = self.theme.track_colors[n % self.theme.track_colors.len().max(1)];
            }
            self.commit_session("Add stem track", before);
            self.load_file(path, Some((id, 0.0)));
        }
        self.mode = Mode::Multitrack;
    }
}
