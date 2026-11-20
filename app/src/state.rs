// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Everything the app knows that is not drawing: open files, the
//! multitrack session, undo histories, the transport and background jobs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
            len: dur.max(0.0005),
        }
    }
    /// Keeps the view inside the file: never wider than it, never
    /// scrolled past either end, as in the leading editor.
    pub fn clamp_to(&mut self, total: f64) {
        let total = total.max(1e-7);
        if !self.len.is_finite() || self.len <= 0.0 {
            self.len = total;
        }
        self.len = self.len.min(total);
        self.start = self.start.clamp(0.0, total - self.len);
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
    /// Display-only amplitude zoom of the waveform (1 to 16).
    pub amp: f32,
    /// The spectral selection (time x frequency shapes).
    pub spec_sel: crate::spectral_select::SpecSel,
    /// Tags and the markers/chapters found in the file when it was opened.
    pub info: af27_io::meta::FileInfo,
    /// Made with New Audio File at this bit depth (Save As starts there).
    pub new_bits: Option<u16>,
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
            new_bits: None,
            dirty: false,
            view: View::fit(dur),
            sel: None,
            cursor: 0.0,
            markers: Vec::new(),
            version: 1,
            spectral: true,
            amp: 1.0,
            spec_sel: Default::default(),
            info: Default::default(),
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
    /// A file decoding in the background (copied into the session folder
    /// first when it is going into a saved session). The result carries
    /// the path it was opened from.
    Load {
        path: PathBuf,
        rx: Receiver<(PathBuf, Result<af27_io::Opened, String>)>,
        /// Put it on this track at this time when done.
        place: Option<(af27_core::TrackId, f64)>,
        /// The import batch and slot it belongs to.
        batch: Option<(u64, usize)>,
    },
}

impl Job {
    pub fn path(&self) -> &Path {
        match self {
            Job::Load { path, .. } => path,
        }
    }
}

pub struct Toast {
    pub text: String,
    pub at: Instant,
    pub error: bool,
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
    /// Waits between attempts to open the audio device again after it
    /// went away or would not open.
    pub audio_retry: af27_engine::health::Backoff,
    program_key: (Mode, DocId, u64, u64),
    /// Where playback began, shown on the ruler while playing or paused;
    /// Stop folds it into the playhead.
    pub start_marker: Option<f64>,
    /// Paused: the playhead and the start marker stay; Play continues.
    pub paused: bool,
    /// Playback this side started and has not yet seen stop (the audio
    /// thread may stop on its own at the end).
    transport_live: bool,
    /// Automation writing and lane display (see `mixing`).
    pub mixing: crate::mixing::Mixing,
    /// MIDI input for armed MIDI tracks (see `midi_in`).
    pub midi: crate::midi_in::MidiIn,
    /// A new selection taking over the loop while playing.
    loop_watch: crate::transport::Settle,
    /// Seconds of held Left/Right since playback last skipped along.
    scrub_clock: f64,
    /// When the app started (the clock for settling a new loop).
    born: Instant,
    /// `born` seconds at the last tick.
    last_tick: f64,
    pub return_on_stop: bool,
    pub meter: (f32, f32),
    /// Motion and music state for the HUD chrome.
    pub fx: crate::hud::Fx,
    /// The workspace on screen, and one the toolbar asked to switch to.
    pub workspace: crate::workspace::Workspace,
    pub want_workspace: Option<crate::workspace::Workspace>,
    /// What is being typed into the time display, while it is.
    pub time_edit: Option<String>,
    pub reset_workspace: bool,
    /// Visual effects: 0 Auto (cut back when frames are slow), 1 Full, 2 Lite.
    pub visuals_mode: u8,
    /// Auto mode decided this device needs the lighter look.
    pub visuals_lite: bool,
    pub meter_hold: (f32, f32),

    pub clipboard: Option<AudioBuffer>,
    /// The export dialog and any export running.
    pub export: crate::export::ExportUi,
    /// The last effect applied and its settings, for Repeat Last Effect.
    pub last_effect: Option<(Effect, Vec<f32>)>,
    pub effect: Effect,
    pub effect_values: HashMap<Effect, Vec<f32>>,
    pub separation: crate::separation::SeparationUi,
    pub sep_dirs: af27_separation::Dirs,
    pub generate: GenerateUi,

    pub theme: Theme,
    /// The chosen theme as made, before the user's own hues.
    pub theme_base: Theme,
    /// The user's hues for the current theme, if they changed any.
    pub hues: Option<crate::theme::Hues>,
    pub themes: Vec<Theme>,
    pub theme_changed: bool,
    pub toasts: Vec<Toast>,
    pub stats: Option<(String, af27_dsp::loudness::Stats)>,
    pub tool: Tool,
    pub show_about: bool,
    pub last_autosave: Instant,
    pub recording_target: Option<RecordTarget>,
    /// Where recordings come from, per track.
    pub rec_inputs: crate::record_input::RecordInputs,
    /// The MIDI clip open in the Piano Roll.
    pub piano_clip: Option<af27_core::ClipId>,
    pub phase: crate::phasewheel::PhasePrefs,
    /// The Visualizer panel: its choices and its picture.
    pub viz: crate::visualizer::Visualizer,
    /// The last analysis window taken while stopped, and what it was taken at.
    pub scope_cache: Option<((u64, u64, i64), Vec<[f32; 2]>)>,
    /// Effect slot open for editing in the Inspector: (track or MASTER, slot).
    pub open_slot: Option<(u16, usize)>,
    /// Effect windows open: (rack, slot).
    pub fx_windows: Vec<(u16, usize)>,
    /// The Waveform editor's rack (applied to the file).
    pub file_chain: Vec<af27_core::EffectSlot>,
    pub file_rack: af27_core::Rack,
    /// Track or master rack in the Effects panel.
    pub rack_tab: crate::rack::RackTab,
    /// A rack preset being named.
    pub rack_preset_name: Option<String>,
    /// Bring the Effects panel to the front.
    pub want_effects: bool,
    /// The Effects menu's apply window is open.
    pub effect_dialog: bool,
    /// File > New Audio File / New Multitrack Session dialogs.
    pub new_file_ui: Option<crate::new_dialogs::NewFileDraft>,
    pub new_session_ui: Option<crate::new_dialogs::NewSessionDraft>,
    /// The rack row last clicked: (rack, row).
    pub rack_row: Option<(u16, usize)>,
    /// Each track's rack meters (from the mix the engine plays), and the
    /// levels shown (falling back slowly).
    pub rack_meters: Vec<std::sync::Arc<af27_engine::mix::RackMeter>>,
    pub rack_shown: HashMap<u16, (f32, f32)>,
    /// The Import from Audiform 1 window.
    pub af1_prompt: Option<crate::af1_import::Prompt>,
    /// The clip whose Properties show, and a floating window for them.
    pub want_properties: bool,
    /// A live-edit drag is under way (one undo step per drag).
    pub gesture: bool,
    /// Bring the Piano Roll to the front.
    pub want_piano: bool,
    /// The Audiform document the session was opened from (or last saved
    /// as), so saving keeps everything Audiform27 does not use.
    pub session_base: Option<serde_json::Value>,
    /// The Audiform effect picked in the Effects panel, and its settings.
    pub fx_pick: Option<String>,
    pub fx_values: HashMap<String, Vec<f32>>,
    /// Edit > Preferences: the settings saved in `prefs.json`.
    pub prefs: crate::settings::Prefs,
    /// Zoom preferences (saved as zoom.json).
    pub zoom: crate::zoom::ZoomPrefs,
    /// Follow Playhead, and whether a hand-made move let go of it.
    pub follow: crate::zoom::Follow,
    /// The spectral display's frequency view, for the run of the app.
    pub spec: crate::zoom::SpecView,
    /// Widths in pixels of the Waveform editor's and the Multitrack's
    /// time areas last frame (for the 32 px per sample zoom limit).
    pub wave_px: f32,
    pub session_px: f32,
    /// Height in pixels of the Multitrack's lanes last frame (for fitting
    /// the tracks).
    pub session_lanes_h: f32,
    /// Display-only amplitude zoom of the Multitrack's waveforms (1 to 16).
    pub session_amp: f32,
    /// Brush Select and Spot Repair brush diameter, in pixels.
    pub brush_px: f32,
    /// The waveform's share of the Waveform editor's height when the
    /// spectrum shows (the divider between them).
    pub spec_split: f32,
    /// The spectrogram's two colours (quiet, loud), blended around the
    /// colour wheel; None uses the theme's gradient.
    pub spec_colors: Option<[[u8; 3]; 2]>,
    /// A spectral edit running in the background.
    pub spectral_job: Option<crate::spectral_select::SpecJob>,
    /// The tempo window and a `.mid` import waiting for an answer.
    pub tempo_ui: crate::tempo::TempoUi,
    /// The pointer is over the Piano Roll: Cut, Copy, Paste, Delete and
    /// Select All go to its notes.
    pub piano_hover: bool,
    pub piano_cmd: Option<crate::pianoroll::PianoCmd>,
    /// What the engine's audition plays through (a track and its settings).
    pub audition_key: Option<String>,
    /// The session before a drag on a transport field began.
    pub gesture_before: Option<Session>,
    /// A wheel gesture on track heights, kept as one undo step once the
    /// wheel rests.
    pub height_commit: crate::zoom::Debounced<Session>,
    /// Sessions set aside, the recent list, imports on their way in,
    /// recovery copies.
    pub sessions: crate::sessions::Sessions,
    /// The Files panel's selection, sort and filter.
    pub files_ui: crate::files_panel::FilesUi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Move,
    Razor,
    TimeSelect,
    /// Spectral selection tools (Waveform editor, spectrum).
    BoxSelect,
    LassoSelect,
    BrushSelect,
    SpotRepair,
}

impl Tool {
    /// Works in the spectral display.
    pub fn is_spectral(self) -> bool {
        matches!(
            self,
            Tool::BoxSelect | Tool::LassoSelect | Tool::BrushSelect | Tool::SpotRepair
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub enum RecordTarget {
    NewFile,
    Track(af27_core::TrackId, f64),
}

fn hues_path(theme: &str) -> std::path::PathBuf {
    let safe: String = theme
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    data_dir().join("hues").join(format!("{safe}.json"))
}

/// The user's saved hues for theme `name`.
pub fn load_hues(name: &str) -> Option<crate::theme::Hues> {
    serde_json::from_str(&std::fs::read_to_string(hues_path(name)).ok()?).ok()
}

impl State {
    /// Shows theme `t` with the user's saved hues for it.
    pub fn set_theme(&mut self, t: Theme) {
        self.hues = load_hues(&t.name);
        self.theme = match &self.hues {
            Some(h) => t.with_hues(h),
            None => t.clone(),
        };
        self.theme_base = t;
        self.theme_changed = true;
    }

    /// New hues for the current theme (None: back to the theme's own),
    /// shown at once and saved.
    pub fn set_hues(&mut self, h: Option<crate::theme::Hues>) {
        self.hues = h;
        self.theme = match &h {
            Some(h) => self.theme_base.with_hues(h),
            None => self.theme_base.clone(),
        };
        self.theme_changed = true;
        let p = hues_path(&self.theme_base.name);
        match h {
            Some(h) => {
                let _ = std::fs::create_dir_all(p.parent().unwrap_or(&p));
                let _ = std::fs::write(&p, serde_json::to_string(&h).unwrap_or_default());
            }
            None => {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

pub fn data_dir() -> PathBuf {
    af27_separation::data_root().join("Audiform27")
}

impl State {
    pub fn new() -> State {
        af27_engine::health::set_log_path(data_dir().join("audio.log"));
        af27_engine::health::install_panic_log();
        let audio = crate::settings::Prefs::load().audio.choice();
        let (engine, engine_error) = match Engine::open(&audio) {
            Ok(e) => (Some(e), None),
            Err(e) => (None, Some(e)),
        };
        let themes = crate::theme::load_all(&crate::theme::user_dir());
        let saved_theme = std::fs::read_to_string(data_dir().join("theme.txt")).unwrap_or_default();
        let theme_base = themes
            .iter()
            .find(|t| t.name == saved_theme.trim())
            .cloned()
            .unwrap_or_default();
        let hues = load_hues(&theme_base.name);
        let theme = match &hues {
            Some(h) => theme_base.with_hues(h),
            None => theme_base.clone(),
        };
        let prefs = crate::settings::Prefs::load();
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
            audio_retry: Default::default(),
            program_key: (Mode::Waveform, 0, 0, 0),
            start_marker: None,
            paused: false,
            transport_live: false,
            mixing: Default::default(),
            midi: Default::default(),
            loop_watch: Default::default(),
            scrub_clock: 0.0,
            born: Instant::now(),
            last_tick: 0.0,
            return_on_stop: prefs.space_returns,
            meter: (0.0, 0.0),
            fx: crate::hud::Fx::default(),
            workspace: crate::workspace::Workspace::from_key(
                &std::fs::read_to_string(data_dir().join("workspace.txt")).unwrap_or_default(),
            ),
            want_workspace: None,
            time_edit: None,
            reset_workspace: false,
            visuals_mode: std::fs::read_to_string(data_dir().join("visuals.txt"))
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .filter(|m: &u8| *m <= 2)
                .unwrap_or(0),
            visuals_lite: false,
            meter_hold: (0.0, 0.0),
            clipboard: None,
            export: Default::default(),
            last_effect: None,
            effect: Effect::Amplify,
            effect_values: HashMap::new(),
            separation: crate::separation::SeparationUi::load(),
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
            theme_base,
            hues,
            themes,
            theme_changed: true,
            toasts: Vec::new(),
            stats: None,
            tool: Tool::Move,
            show_about: false,
            last_autosave: Instant::now(),
            recording_target: None,
            rec_inputs: Default::default(),
            piano_clip: None,
            phase: crate::phasewheel::PhasePrefs::load(),
            viz: crate::visualizer::Visualizer::load(),
            scope_cache: None,
            open_slot: None,
            fx_windows: Vec::new(),
            file_chain: Vec::new(),
            file_rack: Default::default(),
            rack_tab: Default::default(),
            rack_preset_name: None,
            want_effects: false,
            effect_dialog: false,
            new_file_ui: None,
            new_session_ui: None,
            af1_prompt: None,
            rack_row: None,
            rack_meters: Vec::new(),
            rack_shown: HashMap::new(),
            want_properties: false,
            gesture: false,
            want_piano: false,
            session_base: None,
            fx_pick: None,
            fx_values: HashMap::new(),
            prefs,
            zoom: crate::zoom::ZoomPrefs::load(),
            follow: crate::zoom::Follow::default(),
            spec: crate::zoom::SpecView::default(),
            wave_px: 1000.0,
            session_px: 1000.0,
            session_lanes_h: 400.0,
            session_amp: 1.0,
            tempo_ui: Default::default(),
            piano_hover: false,
            piano_cmd: None,
            audition_key: None,
            gesture_before: None,
            height_commit: Default::default(),
            brush_px: 24.0,
            spec_split: 0.5,
            spec_colors: None,
            spectral_job: None,
            sessions: crate::sessions::Sessions::load(),
            files_ui: Default::default(),
        }
    }

    /// Picks a tool; a spectral tool turns the spectrum on.
    pub fn choose_tool(&mut self, tool: Tool) {
        self.tool = tool;
        if tool.is_spectral() {
            self.mode = Mode::Waveform;
            if let Some(d) = self.doc_mut() {
                d.spectral = true;
            }
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
            if crate::sessions::is_session_ext(&ext) {
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
            let _ = tx.send((p.clone(), af27_io::open_file(&p)));
        });
        self.jobs.push(Job::Load {
            path,
            rx,
            place,
            batch: None,
        });
    }

    /// Decodes a file for an import batch, copying it into the session
    /// folder first when `copy` says so (on the loading thread).
    pub fn load_into(
        &mut self,
        path: PathBuf,
        batch: Option<(u64, usize)>,
        copy: Option<crate::sessions::CopyTo>,
    ) {
        let (tx, rx) = channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let (from, note) = match copy.map(|c| c.place(&p)) {
                Some(Ok(q)) => (q, None),
                Some(Err(e)) => (p.clone(), Some(e)),
                None => (p.clone(), None),
            };
            let r = af27_io::open_file(&from);
            // A failed copy still opens the original; say so.
            let r = match (r, note) {
                (Ok(o), Some(e)) => {
                    eprintln!("{e}");
                    Ok(o)
                }
                (r, _) => r,
            };
            let _ = tx.send((from, r));
        });
        self.jobs.push(Job::Load {
            path,
            rx,
            place: None,
            batch,
        });
    }

    /// Picks up finished background loads.
    pub fn poll_jobs(&mut self) {
        let mut done = Vec::new();
        self.jobs.retain(|j| match j {
            Job::Load {
                rx, place, batch, ..
            } => match rx.try_recv() {
                Ok((from, r)) => {
                    done.push((from, r, *place, *batch));
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true,
                Err(_) => false,
            },
        });
        let any = !done.is_empty();
        for (path, r, place, batch) in done {
            match r {
                Ok(opened) => {
                    // Opened before (a copy found again, or a second drop).
                    if let Some(id) = self.doc_for_path(&path) {
                        if let Some(b) = batch {
                            self.batch_loaded(b, Some(id));
                        } else if let Some((track, at)) = place {
                            self.place_doc(id, track, at);
                        }
                        continue;
                    }
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Audio")
                        .to_string();
                    if let Some(n) = opened.notice() {
                        self.toast(format!("{name}: {n}."));
                    }
                    let af27_io::Opened { buf, info, .. } = opened;
                    let id = self.add_doc(name, Some(path.clone()), buf);
                    if let Some(d) = self.docs.iter_mut().find(|d| d.id == id) {
                        d.markers = file_markers(&info, d.duration());
                        d.info = info;
                    }
                    // Session media waiting on this file, in every open
                    // session.
                    let same = |m: &MediaItem| {
                        m.path
                            .as_deref()
                            .is_some_and(|p| crate::sessions::same_path(p, &path))
                    };
                    let waiting: Vec<MediaId> = self
                        .session
                        .media
                        .iter()
                        .filter(|m| same(m))
                        .map(|m| m.id)
                        .collect();
                    for m in waiting {
                        self.link_media(m, id);
                    }
                    let (dur, rate, ch) = self
                        .doc_by_id(id)
                        .map(|d| (d.duration(), d.buf.rate, d.buf.num_channels() as u16))
                        .unwrap_or_default();
                    for p in &mut self.sessions.parked {
                        for m in p.session.media.iter_mut().filter(|m| same(m)) {
                            m.duration = dur;
                            m.rate = rate;
                            m.channels = ch;
                            p.media_docs.insert(m.id, id);
                        }
                    }
                    if let Some(b) = batch {
                        self.batch_loaded(b, Some(id));
                    } else if let Some((track, at)) = place {
                        self.place_doc(id, track, at);
                    } else if self.current.is_none() || self.mode == Mode::Waveform {
                        self.current = Some(id);
                    }
                }
                Err(e) => {
                    if let Some(b) = batch {
                        self.batch_loaded(b, None);
                    }
                    self.error(e)
                }
            }
        }
        if any {
            self.finish_batches();
        }
    }

    pub fn loading(&self) -> Vec<String> {
        self.jobs
            .iter()
            .map(|j| {
                j.path()
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            })
            .collect()
    }

    pub fn link_media(&mut self, media: MediaId, doc: DocId) {
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
            ext_id: None,
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

    /// Closes a file. A file a clip plays, in any open session, stays.
    /// Its waveform leaves the editor (and stops playing) with it.
    pub fn close_doc(&mut self, id: DocId) {
        if self.doc_used(id) {
            self.error("That file is used in an open session. Remove its clips first.");
            return;
        }
        if self.current == Some(id) && self.mode == Mode::Waveform && self.is_playing() {
            self.stop();
        }
        self.docs.retain(|d| d.id != id);
        self.media_docs.retain(|_, d| *d != id);
        for p in &mut self.sessions.parked {
            p.media_docs.retain(|_, d| *d != id);
        }
        self.files_ui.forget(id);
        if self.current == Some(id) {
            self.current = self.docs.last().map(|d| d.id);
        }
    }

    // ---------------------------------------------------------- session

    /// Records `before` as an undo step for the session.
    pub fn commit_session(&mut self, label: &str, before: Session) {
        // A track height wheel gesture still resting becomes its own step
        // first.
        if let Some(h) = self.height_commit.take() {
            self.session_history.push("Track Height", h);
        }
        self.session_history.push(label, before);
        self.session_dirty = true;
        self.session_version += 1;
    }

    /// Opens a session. One already open comes to the front instead; the
    /// session in front is set aside, not closed.
    pub fn open_session(&mut self, path: &Path) {
        match self.find_open(path) {
            Some(None) => {
                self.mode = Mode::Multitrack;
                return;
            }
            Some(Some(i)) => {
                self.switch_session(i);
                return;
            }
            None => {}
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let result = if ext == "af27" {
            std::fs::read_to_string(path)
                .map_err(|e| e.to_string())
                .and_then(|t| Session::from_json(&t))
                .map(|s| (s, Vec::new(), None))
        } else {
            crate::sessions::import_session(path).map(|i| (i.session, i.notices, i.base))
        };
        match result {
            Ok((session, notices, base)) => {
                self.park_front();
                self.session = session;
                self.session_base = if ext == "af27" || ext == "sesx" {
                    None
                } else {
                    base
                };
                self.session_history.clear();
                // Audiform sessions save back to themselves; a .sesx import
                // asks for a name on first save.
                self.session_path =
                    matches!(ext.as_str(), "af27" | "audfrm" | "altaudio").then(|| {
                        if ext == "altaudio" {
                            path.with_extension("audfrm")
                        } else {
                            path.to_path_buf()
                        }
                    });
                self.sessions.source = Some(path.to_path_buf());
                self.session_dirty = false;
                self.session_version += 1;
                self.media_docs.clear();
                self.selected_clip = None;
                self.session_view = View::fit(self.session.duration().max(30.0));
                self.session_cursor = self.session.playhead.max(0.0);
                self.mode = Mode::Multitrack;
                let missing = self.link_session_media();
                self.remember_recent(path);
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

    /// Saves the session. `.audfrm` (the default) is Audiform's own format,
    /// written losslessly over what was opened; `.af27` is a plain dump of
    /// Audiform27's model.
    ///
    /// Before writing: unsaved Waveform edits to files the session uses are
    /// kept, files that came in this run are copied into the session
    /// folder (Session Media), media no clip uses that was never saved is
    /// left out, and the file as it was goes into Backups.
    pub fn save_session_to(&mut self, path: PathBuf) {
        self.keep_playhead();
        crate::plugins::capture_all(self);
        let is_af27 = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("af27"));
        // Save As, or the first save, names the session after its file.
        let new_file = !self
            .session_path
            .as_deref()
            .is_some_and(|p| crate::sessions::same_path(p, &path));
        if new_file {
            if let Some(stem) = path.file_stem() {
                self.session.name = stem.to_string_lossy().to_string();
            }
        }
        self.keep_waveform_edits(&path);
        self.copy_new_media_on_save(&path);
        self.prune_unsaved_media();
        if self.prefs.backups {
            if let Err(e) = crate::sessions::back_up(&path, self.prefs.backups_keep as usize) {
                self.error(format!("Could not back up the session first: {e}"));
            }
        }
        let result = if is_af27 {
            std::fs::write(&path, self.session.to_json()).map_err(|e| e.to_string())
        } else {
            af27_interop::audfrm::save(&mut self.session, self.session_base.as_ref(), &path).map(
                |doc| {
                    self.session_base = Some(doc);
                },
            )
        };
        match result {
            Ok(()) => {
                self.session_path = Some(path.clone());
                self.sessions.source = Some(path.clone());
                self.session_dirty = false;
                crate::sessions::delete_autosave(self.sessions.key);
                self.remember_recent(&path);
                self.toast(format!("Saved {}", path.display()));
            }
            Err(e) => self.error(format!("Could not save the session: {e}")),
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
        self.follow.resume();
        self.put_cursor(t);
        if !self.transport_live && !self.paused {
            self.keep_playhead();
        }
    }

    /// Moves the playhead with nothing else (no follow, nothing saved).
    fn put_cursor(&mut self, t: f64) {
        match self.mode {
            Mode::Waveform => {
                if let Some(d) = self.doc_mut() {
                    d.cursor = t.clamp(0.0, d.duration());
                }
            }
            Mode::Multitrack => self.session_cursor = t.max(0.0),
        }
    }

    /// The stopped playhead goes into the session (saved with it, but
    /// never marking it changed).
    pub fn keep_playhead(&mut self) {
        if !self.transport_live && !self.paused {
            self.session.playhead = self.session_cursor;
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
                // No file open: nothing plays (not the file just closed).
                None => {
                    af27_engine::buffer_program(Arc::new(AudioBuffer::new(self.session.rate, 2, 0)))
                }
            },
            Mode::Multitrack => {
                let rate = self.engine.as_ref().map_or(self.session.rate, |e| e.rate);
                let p = af27_engine::mix_program(&self.session, &|m| self.media_buffer(m), rate);
                if let af27_engine::Program::Mix(m) = &p {
                    self.rack_meters = m.tracks.iter().map(|t| t.meter.clone()).collect();
                }
                p
            }
        };
        if let Some(e) = &mut self.engine {
            e.set_program(program);
        }
    }

    pub fn is_playing(&self) -> bool {
        self.engine.as_ref().is_some_and(|e| e.is_playing())
    }

    /// The time display format with this editor's rate and tempo.
    pub fn time_ctx(&self) -> crate::transport::TimeCtx {
        let rate = match self.mode {
            Mode::Waveform => self.doc().map_or(self.session.rate, |d| d.buf.rate),
            Mode::Multitrack => self.session.rate,
        };
        crate::transport::TimeCtx {
            format: self.prefs.time_format,
            rate,
            tempo: self.session.tempo.clone(),
        }
    }

    pub fn play_mode(&self) -> crate::transport::PlayMode {
        self.prefs.play_mode
    }

    /// Normal / Loop / Skip (saved). While playing, playback carries on
    /// from the same spot under the new mode.
    pub fn set_play_mode(&mut self, m: crate::transport::PlayMode) {
        if self.prefs.play_mode == m {
            return;
        }
        self.prefs.play_mode = m;
        self.prefs.save();
        if self.transport_live && self.is_playing() && !self.is_recording() {
            let pos = self.position();
            self.jump_to(pos);
            self.loop_watch.start(self.selection());
        }
    }

    pub fn set_return_on_stop(&mut self, on: bool) {
        self.return_on_stop = on;
        self.prefs.space_returns = on;
        self.prefs.save();
    }

    /// Plays from the playhead (or continues from Pause).
    pub fn play(&mut self) {
        self.sync_program();
        let len = self.program_len();
        let rec = self.is_recording();
        let cursor = if self.paused || rec {
            self.cursor()
        } else {
            crate::transport::fresh_start(self.cursor(), len)
        };
        let sel = self.selection();
        let (from, plan) = crate::transport::plan_for(self.play_mode(), sel, cursor, len, rec);
        // Continuing from Pause keeps the marker of where playback began.
        if !(self.paused && self.start_marker.is_some()) {
            self.start_marker = Some(from);
        }
        self.paused = false;
        self.follow.resume();
        self.loop_watch.start(sel);
        self.scrub_clock = 0.0;
        if let Some(e) = &mut self.engine {
            e.play_plan(from, plan);
            self.transport_live = true;
            self.put_cursor(from);
            if self.mode == Mode::Multitrack {
                self.auto_start(from);
            }
        } else if let Some(err) = self.engine_error.clone() {
            self.start_marker = None;
            self.error(err);
        }
    }

    /// Stops: the start marker folds into the playhead, which stays where
    /// playback reached (or goes back to the marker with Return to Start
    /// on Stop).
    pub fn stop(&mut self) {
        let reached = if self.transport_live {
            self.engine.as_ref().map_or(0.0, |e| e.position())
        } else {
            self.cursor()
        };
        if let Some(e) = &mut self.engine {
            e.stop();
        }
        self.finish(reached);
    }

    /// Playback is over (stopped, or ran to its end) at `reached`.
    fn finish(&mut self, reached: f64) {
        self.auto_stop(reached);
        let t = crate::transport::stop_position(reached, self.start_marker, self.return_on_stop);
        self.start_marker = None;
        self.paused = false;
        self.transport_live = false;
        self.set_cursor(t);
    }

    /// Pause: the playhead stays and so does the start marker; Pause or
    /// Play again continues.
    pub fn toggle_pause(&mut self) {
        if self.is_recording() {
            return;
        }
        if self.paused {
            self.play();
        } else if self.transport_live && self.is_playing() {
            let pos = self.engine.as_ref().map_or(0.0, |e| e.position());
            if let Some(e) = &mut self.engine {
                e.stop();
            }
            self.transport_live = false;
            self.paused = true;
            self.put_cursor(pos);
        }
    }

    /// Playing (and not paused), as the transport sees it.
    pub fn transport_playing(&self) -> bool {
        self.transport_live && self.is_playing()
    }

    /// Space: plays, or stops when playing or recording.
    pub fn toggle_play(&mut self) {
        if self.is_recording() {
            self.stop_recording();
        } else if self.transport_playing() {
            self.stop();
        } else {
            self.play();
        }
    }

    /// Moves the playhead; while playing, playback carries on from there
    /// (without a gap or an effects reset). Ignored while recording.
    pub fn jump_to(&mut self, t: f64) {
        if self.is_recording() {
            return;
        }
        let t = t.max(0.0);
        if self.transport_playing() {
            let len = self.program_len();
            let (from, plan) =
                crate::transport::plan_for(self.play_mode(), self.selection(), t, len, false);
            if let Some(e) = &mut self.engine {
                e.retarget(Some(from), plan);
            }
            self.follow.resume();
            self.put_cursor(from);
        } else {
            self.set_cursor(t);
        }
    }

    /// Held Left (`dir` -1) or Right (1) for `dt` seconds: stopped, the
    /// playhead glides silently at 4x; playing, playback skips along.
    pub fn scrub_arrows(&mut self, dir: f64, dt: f64) {
        use crate::transport::{scrub_jump, SCRUB_SPEED, SCRUB_STEP};
        if self.is_recording() {
            return;
        }
        if self.transport_playing() {
            self.scrub_clock += dt;
            if self.scrub_clock >= SCRUB_STEP {
                self.scrub_clock -= SCRUB_STEP;
                let pos = self.position();
                self.jump_to(pos + scrub_jump(dir));
            }
        } else {
            let now = self.cursor();
            self.jump_to(now + dir * dt * SCRUB_SPEED);
        }
    }

    /// While looping: a new selection becomes the loop once it settles.
    fn follow_new_loop(&mut self) {
        use crate::transport::{plan_for, takeover, PlayMode};
        if self.play_mode() != PlayMode::Loop || self.is_recording() {
            return;
        }
        let now = self.born.elapsed().as_secs_f64();
        let sel = self.selection();
        let Some(new) = self.loop_watch.update(sel, now) else {
            return;
        };
        let len = self.program_len();
        let (a, b) = new.unwrap_or((0.0, len));
        let pos = self.position();
        let jump = takeover(pos, a, b);
        let (_, plan) = plan_for(PlayMode::Loop, new, jump.unwrap_or(pos), len, false);
        if let Some(e) = &mut self.engine {
            e.retarget(jump, plan);
        }
    }

    /// Called every frame: follow the playhead, meters, jobs.
    /// Opens the output again with the devices in Settings (the system
    /// default where the chosen one is missing). Stops playback.
    pub fn reopen_audio(&mut self) {
        if self.is_playing() || self.is_recording() {
            self.stop();
        }
        // The old stream closes (on silence) before the new one opens: an
        // ASIO driver serves one set of buffers.
        self.engine = None;
        match af27_engine::Engine::open(&self.prefs.audio.choice()) {
            Ok(e) => {
                if let Some(n) = &e.note {
                    let n = n.clone();
                    self.error(n);
                }
                self.engine = Some(e);
                self.engine_error = None;
                self.audio_retry.reset();
            }
            Err(e) => self.engine_error = Some(e),
        }
        // Hand the new stream everything again.
        self.program_key = (Mode::Waveform, u64::MAX as DocId, u64::MAX, u64::MAX);
        self.sync_program();
        self.live_metronome();
    }

    /// Keeps the output healthy, called every frame: a device that went
    /// away, failed or stopped calling back is opened again (the system
    /// default where the chosen one is gone), and with no output at all
    /// it keeps trying, waiting longer each time. Also hands the engine
    /// the 3D layout (for speakers to their own outputs) and the track
    /// input menus the ASIO driver's inputs.
    fn watch_audio(&mut self, now: Instant) {
        let why = self.engine.as_mut().and_then(|e| e.check(now));
        if let Some(why) = why {
            self.reopen_audio();
            let text = match &self.engine {
                Some(e) => format!("{why} Playing through {}.", e.device),
                None => format!("{why} Trying again…"),
            };
            self.error(text);
        } else if self.engine.is_none() && self.audio_retry.due(now) && self.audio_retry.tries() > 1
        {
            // (The first `due` only starts the clock: the failure was just
            // reported.)
            self.reopen_audio();
            if let Some(e) = &self.engine {
                let text = format!("Audio is back: playing through {}.", e.device);
                self.toast(text);
            }
        }
        let layout = match self.mode {
            Mode::Multitrack => self.session.space,
            Mode::Waveform => None,
        };
        if let Some(e) = &mut self.engine {
            e.set_layout(layout);
        }
        let asio = self.engine.as_ref().and_then(|e| e.asio_inputs.as_ref());
        self.rec_inputs.follow_driver(asio);
    }

    pub fn tick(&mut self) {
        self.watch_audio(Instant::now());
        self.poll_jobs();
        crate::separation::poll(self);
        crate::spectral_select::poll(self);
        self.sync_program();
        if let Some(e) = &mut self.engine {
            e.collect();
            for text in e.take_recording_errors() {
                let at = Instant::now();
                self.toasts.push(Toast {
                    text,
                    at,
                    error: true,
                });
            }
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
        if self.transport_live && !self.is_playing() {
            // The audio thread reached the end (or the selection's end).
            let reached = self.engine.as_ref().map_or(0.0, |e| e.position());
            self.finish(reached);
        }
        if self.transport_live && self.is_playing() {
            self.follow_new_loop();
            self.auto_tick();
        }
        if self.mode == Mode::Multitrack {
            self.midi_tick();
        }
        let now = self.born.elapsed().as_secs_f64();
        let dt = (now - self.last_tick).clamp(0.0, 1.0);
        self.last_tick = now;
        if self.is_playing() {
            let pos = self.engine.as_ref().map_or(0.0, |e| e.position());
            // Following picks up again once the view is left alone with
            // the playhead in it.
            let v = match self.mode {
                Mode::Waveform => self.doc().map(|d| d.view),
                Mode::Multitrack => Some(self.session_view),
            };
            let in_view = v.is_some_and(|v| pos >= v.start && pos <= v.start + v.len);
            // The Multitrack picks following up again on its own; the
            // Waveform editor waits for playback to restart or the
            // playhead to move.
            if self.mode == Mode::Multitrack {
                self.follow.idle(dt, in_view);
            }
            // The setting, and not let go of by a hand-made move.
            let follow = self.prefs.follow_playhead && self.follow.active();
            match self.mode {
                Mode::Waveform => {
                    let px = self.wave_px as f64;
                    if let Some(d) = self.doc_mut() {
                        d.cursor = pos;
                        if follow {
                            if let Some(s) = crate::zoom::follow_page(&d.view, pos, px, false) {
                                d.view.start = s;
                                d.view.clamp_to(d.duration());
                            }
                        }
                    }
                }
                Mode::Multitrack => {
                    self.session_cursor = pos;
                    if follow {
                        let lim = self.session_limits();
                        let v = &mut self.session_view;
                        let px = self.session_px as f64;
                        if let Some(s) = crate::zoom::follow_page(v, pos, px, true) {
                            v.start = s;
                            v.limit(&lim);
                        }
                    }
                }
            }
        }
        self.settle_track_heights(false);
        self.toasts
            .retain(|t| t.at.elapsed().as_secs_f32() < if t.error { 8.0 } else { 4.0 });
        self.autosave();
    }

    /// The view was scrolled or zoomed by hand: during playback that
    /// stops Follow Playhead until playback restarts or the playhead moves.
    pub fn hand_moved_view(&mut self) {
        let playing = self.is_playing();
        self.follow.hand_moved(playing);
    }

    /// How far the Multitrack's view zooms and scrolls: 32 pixels per
    /// sample in, four hours (or the content plus a tenth) out, from 0 on.
    pub fn session_limits(&self) -> crate::zoom::TimeLimits {
        let min = crate::zoom::min_len(self.session_px, self.session.rate.max(1) as f64);
        crate::zoom::TimeLimits::session(self.session.duration(), min)
    }

    /// How far the Waveform editor's view of the open file zooms and
    /// scrolls: 32 pixels per sample in, the whole file out.
    pub fn doc_limits(&self) -> crate::zoom::TimeLimits {
        match self.doc() {
            Some(d) => crate::zoom::TimeLimits::file(
                d.duration(),
                crate::zoom::min_len(self.wave_px, d.buf.rate as f64),
            ),
            None => crate::zoom::TimeLimits::file(1.0, 0.0005),
        }
    }

    /// Ends a wheel gesture on the track heights once the wheel rests,
    /// as one undo step.
    pub fn settle_track_heights(&mut self, force: bool) {
        let now = self.born.elapsed().as_secs_f64();
        let before = if force {
            self.height_commit.take()
        } else {
            self.height_commit
                .due(now, crate::zoom::HEIGHT_COMMIT_QUIET)
        };
        if let Some(before) = before {
            let changed = before.tracks.iter().map(|t| t.height).ne(self
                .session
                .tracks
                .iter()
                .map(|t| t.height));
            if changed {
                self.commit_session("Track Height", before);
            }
        }
    }

    /// A wheel movement changed track heights: the first of a gesture
    /// keeps the session from before it for the undo step.
    pub fn touch_track_heights(&mut self) {
        let now = self.born.elapsed().as_secs_f64();
        let session = &self.session;
        self.height_commit.touch(now, || session.clone());
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
        self.engine.as_ref().is_some_and(|e| e.is_recording()) || self.midi.recording
    }

    pub fn start_recording(&mut self) {
        // Only MIDI tracks armed: record what they hear while playing.
        let audio_armed = self
            .session
            .tracks
            .iter()
            .any(|t| t.armed && t.kind == af27_core::TrackKind::Audio);
        if self.mode == Mode::Multitrack && self.midi_armed() && !audio_armed {
            self.midi_record_start();
            self.play();
            return;
        }
        let target = match self.mode {
            Mode::Waveform => RecordTarget::NewFile,
            Mode::Multitrack => {
                let armed = self
                    .session
                    .tracks
                    .iter()
                    .find(|t| t.armed && t.kind != af27_core::TrackKind::Midi)
                    .map(|t| t.id);
                match armed.or(self.selected_track) {
                    Some(t) => RecordTarget::Track(t, self.session_cursor),
                    None => {
                        self.error("Arm a track (R) to record onto it.");
                        return;
                    }
                }
            }
        };
        let sources = crate::record_input::begin(self, target);
        let Some(e) = &mut self.engine else { return };
        match e.start_recording_from(&sources) {
            Ok(()) => {
                self.recording_target = Some(target);
                if matches!(target, RecordTarget::Track(..)) {
                    if self.midi_armed() {
                        self.midi_record_start();
                    }
                    self.play();
                }
            }
            Err(err) => self.error(err),
        }
    }

    pub fn stop_recording(&mut self) {
        if self.midi.recording {
            let reached = self.position();
            self.midi_record_stop(reached);
            let audio = self.engine.as_ref().is_some_and(|e| e.is_recording());
            if !audio {
                self.stop();
                return;
            }
        }
        let Some(e) = &mut self.engine else { return };
        let takes = e.stop_recording_all();
        if takes.is_empty() {
            return;
        }
        let reached = e.position();
        e.stop();
        if self.transport_live {
            self.finish(reached);
        }
        let target = self.recording_target.take();
        crate::record_input::finish(self, takes, target);
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
        if self.doc().is_some() {
            self.last_effect = Some((e, vals.clone()));
        }
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
                self.settle_track_heights(true);
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
        self.add_marker_at(t);
    }

    pub fn add_marker_at(&mut self, t: f64) {
        match self.mode {
            Mode::Waveform => {
                if let Some(d) = self.doc_mut() {
                    let n = d.markers.len() + 1;
                    d.markers.push(Marker {
                        name: format!("Marker {n:02}"),
                        time: t,
                        length: 0.0,
                        ext_id: None,
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
                    ext_id: None,
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
            } else if let Some((ti, ci)) = self.session.find_midi(c) {
                self.session.tracks[ti].midi.remove(ci);
                if self.piano_clip == Some(c) {
                    self.piano_clip = None;
                }
                self.commit_session("Delete MIDI clip", before);
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

    // ------------------------------------------------------- live edits

    /// Records an undo step for an edit the audio thread already has
    /// (fader, pan, effect settings, 3D position, instrument): the mix is
    /// not rebuilt, so effect tails and the playhead carry on.
    pub fn commit_live(&mut self, label: &str, before: Session) {
        // While automation is written, the moves belong to its one step.
        if !self.mixing.pass_open() {
            self.session_history.push(label, before);
        }
        self.session_dirty = true;
    }

    /// Seconds since the app started (a wall clock for UI timing).
    pub fn clock(&self) -> f64 {
        self.born.elapsed().as_secs_f64()
    }

    /// Sends a track's fader, pan and mute or solo state to the mix.
    pub fn live_track(&mut self, ti: usize) {
        let audible = self.session.heard();
        let Some(t) = self.session.tracks.get(ti) else {
            return;
        };
        let fader = af27_core::session::fader_gain(t.volume_db);
        let pan = t.pan;
        let sends: Vec<(f32, f32, bool)> = t
            .sends
            .iter()
            .filter(|s| self.session.track_index(s.bus).is_some())
            .map(|s| {
                let level = af27_core::time::db_to_gain(s.level_db.clamp(-96.0, 12.0));
                (level, s.pan, s.enabled)
            })
            .collect();
        if let Some(e) = &mut self.engine {
            e.set_strip(ti, fader, pan);
            for (k, (level, pan, on)) in sends.into_iter().enumerate() {
                e.set_send(ti, k, level, pan, on);
            }
            // Solo on one track changes what every track does.
            for (i, a) in audible.into_iter().enumerate() {
                e.set_audible(i, a);
            }
        }
    }

    pub fn live_master(&mut self) {
        let g = af27_core::time::db_to_gain(self.session.master_db);
        if let Some(e) = &mut self.engine {
            e.set_master(g);
        }
    }

    /// The effect chain for a track index, or the master chain for MASTER.
    pub fn chain_mut(&mut self, track: u16) -> Option<&mut Vec<af27_core::EffectSlot>> {
        if track == af27_engine::MASTER {
            Some(&mut self.session.master_chain)
        } else {
            self.session
                .tracks
                .get_mut(track as usize)
                .map(|t| &mut t.chain)
        }
    }

    /// A rack's input and output levels to show (linear), falling back
    /// slowly between peaks. The master's output is the main meter.
    pub fn rack_levels(&mut self, target: u16) -> (f32, f32) {
        let fresh = if target == af27_engine::MASTER {
            (
                self.meter.0.max(self.meter.1),
                self.meter.0.max(self.meter.1),
            )
        } else {
            self.rack_meters
                .get(target as usize)
                .map_or((0.0, 0.0), |m| m.take())
        };
        let e = self.rack_shown.entry(target).or_insert((0.0, 0.0));
        e.0 = fresh.0.max(e.0 * 0.88);
        e.1 = fresh.1.max(e.1 * 0.88);
        *e
    }

    /// A rack's power, levels or mix changed.
    pub fn live_rack(&mut self, track: u16) {
        let rack = if track == af27_engine::MASTER {
            Some(self.session.master_rack)
        } else {
            self.session.tracks.get(track as usize).map(|t| t.rack)
        };
        if let (Some(r), Some(e)) = (rack, &mut self.engine) {
            e.set_rack(track, r);
        }
    }

    pub fn live_slot(&mut self, track: u16, slot: usize) {
        let Some(s) = self.chain_mut(track).and_then(|c| c.get(slot)).cloned() else {
            return;
        };
        if let Some(e) = &mut self.engine {
            e.set_param(track, slot, &s.values);
            e.set_bypass(track, slot, s.bypass);
        }
    }

    pub fn live_space(&mut self, ti: usize) {
        if self.session.space.is_none() {
            return;
        }
        let Some(pos) = self.session.tracks.get(ti).map(|t| t.pos) else {
            return;
        };
        if let Some(e) = &mut self.engine {
            e.set_space(ti, pos);
        }
    }

    pub fn live_instrument(&mut self, ti: usize) {
        let Some(t) = self.session.tracks.get(ti) else {
            return;
        };
        let inst = t.instrument;
        let font_db = t.soundfont.as_ref().map(|f| f.gain_db);
        if let Some(e) = &mut self.engine {
            e.set_instrument(ti, inst);
            if let Some(db) = font_db {
                e.set_font_gain(ti, db);
            }
        }
    }

    pub fn live_metronome(&mut self) {
        let on = self.session.tempo.metronome;
        let m = self.prefs.metronome;
        if let Some(e) = &mut self.engine {
            e.set_metronome(on);
            e.set_metronome_settings(m);
        }
    }

    /// Points the engine's audition at track `ti`'s instrument, building
    /// it only when the track or its settings changed.
    pub fn audition_for(&mut self, ti: usize) -> bool {
        let Some(t) = self.session.tracks.get(ti) else {
            return false;
        };
        let key = format!(
            "{}|{:?}|{:?}",
            t.id,
            t.instrument,
            t.soundfont
                .as_ref()
                .map(|f| (&f.path, f.bank, f.program, f.gain_db))
        );
        if self.audition_key.as_deref() == Some(key.as_str()) {
            return self.engine.is_some();
        }
        let Some(e) = &mut self.engine else {
            return false;
        };
        let a = match &t.soundfont {
            Some(sf) => match af27_engine::soundfont::Audition::font(sf, e.rate) {
                Ok(a) => a,
                Err(_) => return false,
            },
            None => af27_engine::soundfont::Audition::synth(t.instrument, e.rate),
        };
        e.set_audition(a);
        self.audition_key = Some(key);
        true
    }

    /// Starts a note on track `ti`'s instrument (the piano roll).
    pub fn audition_on(&mut self, ti: usize, pitch: u8, velocity: u8, pan: f32) {
        if self.audition_for(ti) {
            if let Some(e) = &mut self.engine {
                e.note_on(pitch, velocity, pan);
            }
        }
    }

    pub fn audition_off(&mut self, pitch: u8) {
        if let Some(e) = &mut self.engine {
            e.note_off(pitch);
        }
    }

    pub fn audition_all_off(&mut self) {
        if let Some(e) = &mut self.engine {
            e.all_notes_off();
        }
    }

    pub fn add_track_kind(&mut self, kind: af27_core::TrackKind) -> af27_core::TrackId {
        let before = self.session.clone();
        let id = self.session.add_track_of(kind);
        let n = self.session.tracks.len() - 1;
        if let Some(t) = self.session.track_mut(id) {
            t.color = self.theme.track_colors[n % self.theme.track_colors.len().max(1)];
        }
        self.commit_session("Add track", before);
        self.selected_track = Some(id);
        id
    }

    /// Renders the session offline at its own rate: stereo as heard, or
    /// (3D sessions, `bed`) one channel per speaker.
    pub fn render_mix(&self, from: f64, to: f64, bed: bool) -> AudioBuffer {
        let program =
            af27_engine::mix_program(&self.session, &|m| self.media_buffer(m), self.session.rate);
        if bed {
            af27_engine::render_bed(program, self.session.rate, from, to)
        } else {
            af27_engine::render(program, self.session.rate, from, to)
        }
    }

    /// The newest stereo frames heard: the output ring while playing;
    /// while stopped, the audio just before the playhead (cached).
    pub fn scope_frames(&mut self, n: usize) -> (Vec<[f32; 2]>, u32) {
        if self.is_playing() {
            return (af27_engine::scope::latest(n), af27_engine::scope::rate());
        }
        let t = self.cursor();
        let key = (
            self.session_version
                + self.docs.iter().map(|d| d.version).sum::<u64>()
                + self.mode as u64 * 1_000_000,
            n as u64,
            (t * 1000.0) as i64,
        );
        let rate = match self.mode {
            Mode::Waveform => self.doc().map_or(48_000, |d| d.buf.rate),
            Mode::Multitrack => self.session.rate,
        };
        if let Some((k, frames)) = &self.scope_cache {
            if *k == key {
                return (frames.clone(), rate);
            }
        }
        let frames: Vec<[f32; 2]> = match self.mode {
            Mode::Waveform => match self.doc() {
                Some(d) => {
                    let end = (t * d.buf.rate as f64) as usize;
                    let start = end.saturating_sub(n);
                    (start..start + n)
                        .map(|i| {
                            let (l, r) = d.buf.stereo_at(i);
                            [l, r]
                        })
                        .collect()
                }
                None => vec![[0.0; 2]; n],
            },
            Mode::Multitrack => {
                let span = n as f64 / rate as f64;
                let b = self.render_mix((t - span).max(0.0), (t - span).max(0.0) + span, false);
                (0..b.frames())
                    .map(|i| [b.channels[0][i], b.channels[1][i]])
                    .collect()
            }
        };
        self.scope_cache = Some((key, frames.clone()));
        (frames, rate)
    }

    /// Levels in dBFS of `out.len()` bands, 20 Hz to 20 kHz evenly in
    /// pitch, from the newest frames heard (while stopped, the audio just
    /// before the playhead), as the Visualizer reads them.
    pub fn scope_spectrum(&mut self, sp: &mut crate::visualizer::Spectrum, out: &mut [f32]) {
        let (frames, rate) = self.scope_frames(sp.size());
        sp.bands(&frames, rate, out);
    }
}

/// The markers and chapters read from a file, as document markers
/// (ranges keep their length, clipped to the file). The marker model has
/// no kinds yet, so cue, range and chapter are told apart only by length;
/// the original kinds stay in `Doc::info`.
fn file_markers(info: &af27_io::meta::FileInfo, dur: f64) -> Vec<Marker> {
    let mut out: Vec<Marker> = info
        .markers
        .iter()
        .filter(|m| m.time.is_finite() && m.time <= dur + 1e-6)
        .map(|m| Marker {
            name: m.name.clone(),
            time: m.time,
            length: m.length.min((dur - m.time).max(0.0)),
            ext_id: None,
        })
        .collect();
    out.sort_by(|a, b| a.time.total_cmp(&b.time));
    out
}

#[cfg(test)]
mod view_tests {
    use super::View;

    #[test]
    fn the_view_never_runs_past_the_file() {
        let mut v = View::fit(4.12);
        assert_eq!((v.start, v.len), (0.0, 4.12));
        let lim = crate::zoom::TimeLimits::file(4.12, 0.0005);
        v.zoom_at(3.0, 2.0, &lim);
        assert!(v.start >= 0.0 && v.start + v.len <= 4.12 + 1e-9);
        v.zoom_at(0.25, 4.0, &lim);
        v.start = 99.0;
        v.clamp_to(4.12);
        assert!((v.start + v.len - 4.12).abs() < 1e-9, "{v:?}");
    }
}
