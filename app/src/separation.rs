// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The AI Stems panel (Audio Separation): the engine and the models to
//! pick from, in Vocals, Instruments and Denoise tabs, each model with its
//! published quality score (SDR), size and whether it is downloaded;
//! Download, Remove and Update for the engine and every model; and
//! Separate, for the file in front, the selected clip or the file picked
//! in the Files panel. Separation runs on this computer.
//!
//! Behaviour ported from Audiform's Audio Separation panel
//! (`SeparationPanel.svelte`, `separation-tools.svelte.ts` and the Audio
//! Separation settings page; the project lead's own files, see
//! docs/clean-room.md), rewritten for egui.
//!
//! Results always go into the Files panel, written to a `Separated`
//! folder beside the saved session (for clips) or beside the source file.
//! For clips, the panel then offers new tracks under each clip's track,
//! copying that track's effects and settings, with copies of the clips
//! playing the results. One undo removes them.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use af27_core::{AudioBuffer, ClipId, MediaId, Session};
use af27_separation::{self as sep, Model, Shared};
use egui::{Align2, RichText, Stroke};
use serde::{Deserialize, Serialize};

use crate::state::{data_dir, Job, Mode, State};
use crate::theme::c;

/// The wording shown wherever something is downloaded.
pub const CONSENT: &str = "It only pulls the necessary model for demixing; audio separation is handled locally on user hardware and speeds / results depend on that hardware.";

/// What Separate works on.
#[derive(Clone)]
pub struct Source {
    /// "song.wav", or "\"Clip 3\"".
    pub label: String,
    /// The base of the result file names.
    pub name: String,
    pub buf: Arc<AudioBuffer>,
    pub out_dir: PathBuf,
    /// The clips it came from (their whole file is separated), and the
    /// media they play.
    pub clips: Vec<ClipId>,
    pub media: Option<MediaId>,
}

#[derive(Clone)]
pub struct Request {
    pub model: Model,
    pub source: Source,
    pub gpu: bool,
}

/// One background job.
#[derive(Clone)]
enum Step {
    InstallEngine,
    CheckEngine,
    DownloadModel(Model),
    CheckModel(Model),
    Separate(Request),
}

/// A question waiting for the user.
enum Ask {
    /// Download what is missing, then separate.
    DownloadFirst {
        req: Request,
        engine: bool,
        model: bool,
    },
    /// A Download button in the list.
    Download(Step),
    /// The graphics card failed: try the processor.
    UseCpu(Request),
    AddTracks {
        clips: Vec<ClipId>,
        media: MediaId,
        outputs: Vec<(String, PathBuf)>,
    },
    RemoveModel(Model),
    RemoveEngine,
}

#[derive(Clone, Copy, PartialEq)]
enum EngineUpdate {
    Unknown,
    Current,
    Available,
}

/// Saved between runs, in `<data>/Audiform27/separation.json`.
#[derive(Serialize, Deserialize)]
struct Prefs {
    tab: String,
    model: Option<String>,
    gpu: bool,
    overlap: u32,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            tab: sep::TABS[0].into(),
            model: None,
            gpu: true,
            overlap: 2,
        }
    }
}

pub struct SeparationUi {
    tab: String,
    model: Option<String>,
    gpu: bool,
    overlap: u32,
    /// The running download, check or separation (the window keeps
    /// repainting while it is set).
    pub job: Option<Shared>,
    job_title: String,
    step: Option<Step>,
    /// What runs after the current job succeeds.
    queue: Vec<Step>,
    cancel: Arc<AtomicBool>,
    engine_version: Option<String>,
    engine_update: EngineUpdate,
    /// Models whose file no longer matches the pinned checksum.
    stale: HashSet<String>,
    ask: Option<Ask>,
    catalog: Option<Vec<Model>>,
    /// Model id to its downloaded copies, refreshed after every job.
    installed: Option<HashMap<String, Vec<PathBuf>>>,
}

fn prefs_path() -> PathBuf {
    data_dir().join("separation.json")
}

impl SeparationUi {
    pub fn load() -> SeparationUi {
        let p: Prefs = std::fs::read_to_string(prefs_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        SeparationUi {
            tab: if sep::TABS.contains(&p.tab.as_str()) {
                p.tab
            } else {
                sep::TABS[0].into()
            },
            model: p.model,
            gpu: p.gpu,
            overlap: p.overlap.clamp(1, 8),
            job: None,
            job_title: String::new(),
            step: None,
            queue: Vec::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            engine_version: None,
            engine_update: EngineUpdate::Unknown,
            stale: HashSet::new(),
            ask: None,
            catalog: None,
            installed: None,
        }
    }

    fn save(&self) {
        let p = Prefs {
            tab: self.tab.clone(),
            model: self.model.clone(),
            gpu: self.gpu,
            overlap: self.overlap,
        };
        let _ = std::fs::create_dir_all(data_dir());
        if let Ok(t) = serde_json::to_string_pretty(&p) {
            let _ = std::fs::write(prefs_path(), t);
        }
    }

    fn refresh(&mut self) {
        self.installed = None;
        self.engine_version = None;
    }
}

// ------------------------------------------------------------ the source

fn base_name(name: &str) -> String {
    name.rsplit_once('.').map_or(name, |(a, _)| a).to_string()
}

/// What Separate would work on: the file in the Waveform editor; in the
/// Multitrack editor the selected clip; else the file picked in Files.
pub fn source(st: &State) -> Option<Source> {
    if st.mode == Mode::Multitrack {
        if let Some((ti, ci)) = st.selected_clip.and_then(|id| st.session.find_clip(id)) {
            let clip = &st.session.tracks[ti].clips[ci];
            let media = st.session.media(clip.media)?;
            let buf = st.media_buffer(clip.media)?;
            let out_dir = st
                .session_path
                .as_ref()
                .and_then(|p| p.parent())
                .or_else(|| media.path.as_ref().and_then(|p| p.parent()))
                .map(|d| d.join("Separated"))
                .unwrap_or_else(|| data_dir().join("Separated"));
            return Some(Source {
                label: format!("\"{}\" (its whole file)", clip.name),
                name: base_name(&media.name),
                buf,
                out_dir,
                clips: vec![clip.id],
                media: Some(clip.media),
            });
        }
    }
    let d = st.doc()?;
    Some(Source {
        label: d.name.clone(),
        name: base_name(&d.name),
        buf: d.buf.clone(),
        out_dir: d
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.join("Separated"))
            .unwrap_or_else(|| data_dir().join("Separated")),
        clips: Vec::new(),
        media: None,
    })
}

// ------------------------------------------------------- separated tracks

/// Adds, under each track holding one of `clips`, one new track per result
/// in `outputs`: a copy of that track (effects, levels, routing, sends)
/// whose clips are copies of the selected ones playing the result file.
/// New media items are made for the results (they are at the source's
/// rate and exact length). Returns the media made, with their files.
pub fn add_separated_tracks(
    s: &mut Session,
    clips: &[ClipId],
    source_media: MediaId,
    outputs: &[(String, PathBuf)],
) -> Vec<(MediaId, PathBuf)> {
    let Some(src) = s.media(source_media).cloned() else {
        return Vec::new();
    };
    let media: Vec<(MediaId, PathBuf)> = outputs
        .iter()
        .map(|(_, path)| {
            let id = s.alloc_id();
            s.media.push(af27_core::session::MediaItem {
                id,
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                path: Some(path.clone()),
                duration: src.duration,
                rate: src.rate,
                channels: src.channels.clamp(1, 2),
                ext_id: None,
            });
            (id, path.clone())
        })
        .collect();
    // Track indexes holding the clips, bottom first so the ones above
    // keep their places.
    let mut tracks: Vec<usize> = clips
        .iter()
        .filter_map(|&id| s.find_clip(id).map(|(ti, _)| ti))
        .collect();
    tracks.sort_unstable();
    tracks.dedup();
    for &ti in tracks.iter().rev() {
        let original = s.tracks[ti].clone();
        let picked: Vec<_> = original
            .clips
            .iter()
            .filter(|c| clips.contains(&c.id) && c.media == source_media)
            .cloned()
            .collect();
        if picked.is_empty() {
            continue;
        }
        for (k, ((stem, _), (mid, _))) in outputs.iter().zip(&media).enumerate() {
            let mut t = original.clone();
            t.id = s.alloc_id();
            t.name = format!("{} {}", original.name, stem);
            t.solo = false;
            t.armed = false;
            t.ext_id = None;
            t.midi.clear();
            for send in &mut t.sends {
                send.ext_id = None;
            }
            for slot in &mut t.chain {
                slot.ext_id = None;
            }
            t.clips = picked
                .iter()
                .map(|c| {
                    let mut c = c.clone();
                    c.id = s.alloc_id();
                    c.media = *mid;
                    c.ext_id = None;
                    c
                })
                .collect();
            s.tracks.insert(ti + 1 + k, t);
        }
    }
    media
}

// ------------------------------------------------------------- jobs

fn background(shared: &Shared, f: impl FnOnce() -> Result<(), String> + Send + 'static) {
    let p = shared.clone();
    std::thread::spawn(move || {
        let r = f();
        if let Ok(mut g) = p.lock() {
            g.done = true;
            if let Err(e) = r {
                g.error = Some(e);
            }
        }
    });
}

fn start(st: &mut State, step: Step) {
    let dirs = st.sep_dirs.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let shared: Shared = Arc::new(Mutex::new(sep::Progress {
        indeterminate: true,
        status: "Starting".into(),
        ..Default::default()
    }));
    let ui = &mut st.separation;
    ui.cancel = cancel.clone();
    let shared = match &step {
        Step::InstallEngine => {
            ui.job_title = "Downloading the separation engine".into();
            let p = shared.clone();
            background(&shared, move || sep::install_engine(&dirs, &cancel, &p));
            shared
        }
        Step::CheckEngine => {
            ui.job_title = "Checking for an engine update".into();
            let p = shared.clone();
            background(&shared, move || {
                sep::check_engine_update(&dirs, &cancel, &p).map(|_| ())
            });
            shared
        }
        Step::DownloadModel(m) => {
            ui.job_title = format!("Downloading {}", m.name);
            let (p, m) = (shared.clone(), m.clone());
            background(&shared, move || {
                let dest = dirs.own_model_path(&m);
                sep::download(&m.url, &dest, &m.sha256, &cancel, &p, m.bytes)
            });
            shared
        }
        Step::CheckModel(m) => {
            ui.job_title = format!("Checking {}", m.name);
            let (p, m) = (shared.clone(), m.clone());
            background(&shared, move || sep::check_model(&dirs, &m, &p).map(|_| ()));
            shared
        }
        Step::Separate(req) => {
            ui.job_title = format!(
                "Separating {} with {}{}",
                req.source.label,
                req.model.name,
                if req.gpu { "" } else { " (processor)" }
            );
            sep::spawn(
                dirs,
                req.model.clone(),
                req.source.buf.clone(),
                req.source.name.clone(),
                req.source.out_dir.clone(),
                req.gpu,
                Some(ui.overlap),
                cancel,
            )
        }
    };
    ui.job = Some(shared);
    ui.step = Some(step);
}

/// Runs the next queued step, if nothing is running.
fn next(st: &mut State) {
    if st.separation.job.is_none() && !st.separation.queue.is_empty() {
        let step = st.separation.queue.remove(0);
        start(st, step);
    }
}

/// Picks up a finished job. Called every frame (from `State::tick`), so a
/// job finishes even while the panel is behind another tab.
pub fn poll(st: &mut State) {
    let Some(job) = st.separation.job.clone() else {
        return;
    };
    let (error, gpu_error, flag, outputs) = {
        let Ok(g) = job.lock() else { return };
        if !g.done {
            return;
        }
        (g.error.clone(), g.gpu_error, g.flag, g.outputs.clone())
    };
    st.separation.job = None;
    let step = st.separation.step.take();
    st.separation.refresh();
    match (step, error) {
        (Some(Step::Separate(req)), Some(_)) if gpu_error && req.gpu => {
            st.separation.queue.clear();
            st.separation.ask = Some(Ask::UseCpu(req));
        }
        (_, Some(e)) => {
            st.separation.queue.clear();
            if e.contains("cancelled") {
                st.toast(e);
            } else {
                st.error(e);
            }
        }
        (Some(Step::InstallEngine), None) => {
            st.separation.engine_update = EngineUpdate::Current;
            st.toast("The separation engine is installed.");
        }
        (Some(Step::CheckEngine), None) => {
            if flag {
                st.separation.engine_update = EngineUpdate::Available;
                st.toast("A newer separation engine is published.");
            } else {
                st.separation.engine_update = EngineUpdate::Current;
                st.toast("The separation engine is up to date.");
            }
        }
        (Some(Step::DownloadModel(m)), None) => {
            st.separation.stale.remove(&m.id);
            st.toast(format!("{} is downloaded.", m.name));
        }
        (Some(Step::CheckModel(m)), None) => {
            if flag {
                st.separation.stale.insert(m.id.clone());
                st.toast(format!(
                    "{} does not match its published file. Update downloads it again.",
                    m.name
                ));
            } else {
                st.separation.stale.remove(&m.id);
                st.toast(format!("{} matches its published file.", m.name));
            }
        }
        (Some(Step::Separate(req)), None) => finished(st, req, outputs),
        (None, None) => {}
    }
    next(st);
}

fn finished(st: &mut State, req: Request, outputs: Vec<(String, PathBuf)>) {
    if outputs.is_empty() {
        return;
    }
    for (_, p) in &outputs {
        st.load_file(p.clone(), None);
    }
    let names: Vec<&str> = outputs.iter().map(|(n, _)| n.as_str()).collect();
    st.toast(format!(
        "Separated {} into {}; the results are in the Files panel.",
        req.source.label,
        names.join(", ")
    ));
    if let (false, Some(media)) = (req.source.clips.is_empty(), req.source.media) {
        st.separation.ask = Some(Ask::AddTracks {
            clips: req.source.clips,
            media,
            outputs,
        });
    }
}

fn add_tracks(st: &mut State, clips: &[ClipId], media: MediaId, outputs: &[(String, PathBuf)]) {
    let before = st.session.clone();
    let made = add_separated_tracks(&mut st.session, clips, media, outputs);
    if made.is_empty() {
        return;
    }
    for (m, path) in made {
        let doc = st
            .docs
            .iter()
            .find(|d| d.path.as_deref() == Some(path.as_path()))
            .map(|d| d.id);
        match doc {
            Some(d) => st.link_media(m, d),
            None => {
                let pending = st
                    .jobs
                    .iter()
                    .any(|j| matches!(j, Job::Load { path: p, .. } if *p == path));
                if !pending {
                    st.load_file(path, None);
                }
            }
        }
    }
    st.commit_session("Add Separated Tracks", before);
    st.mode = Mode::Multitrack;
}

/// Separate pressed: asks to download what is missing first.
fn separate(st: &mut State, model: Model) {
    let Some(source) = source(st) else { return };
    let req = Request {
        model: model.clone(),
        source,
        gpu: st.separation.gpu,
    };
    let engine = st.sep_dirs.engine().is_none();
    let missing_model = st.sep_dirs.model(&model).is_none();
    if engine && sep::engine_asset().is_none() {
        st.error("Audio Separation is not available for this computer yet.");
        return;
    }
    if engine || missing_model {
        st.separation.ask = Some(Ask::DownloadFirst {
            req,
            engine,
            model: missing_model,
        });
    } else {
        start(st, Step::Separate(req));
    }
}

// ------------------------------------------------------------- drawing

/// Something a button asks for, run once drawing is done.
type Action = Box<dyn FnOnce(&mut State)>;

pub fn show(ui: &mut egui::Ui, st: &mut State) {
    poll(st);
    let th = st.theme.clone();
    if st.separation.catalog.is_none() {
        st.separation.catalog = Some(sep::catalog(&st.sep_dirs));
    }
    let catalog = st.separation.catalog.clone().unwrap_or_default();
    if st.separation.installed.is_none() {
        st.separation.installed = Some(
            catalog
                .iter()
                .map(|m| (m.id.clone(), st.sep_dirs.model_copies(m)))
                .collect(),
        );
    }
    let installed = st.separation.installed.clone().unwrap_or_default();
    let busy = st.separation.job.is_some();
    let dirs = st.sep_dirs.clone();
    let engine = dirs.engine();
    if let (Some(p), None) = (&engine, &st.separation.engine_version) {
        st.separation.engine_version = Some(sep::engine_version(p).unwrap_or_else(|e| e));
    }
    let dim = |t: &str| RichText::new(t).size(11.0).color(c(th.text_dim));
    let tab = st.separation.tab.clone();
    let model = catalog
        .iter()
        .filter(|m| m.category == tab)
        .find(|m| st.separation.model.as_deref() == Some(m.id.as_str()))
        .or_else(|| {
            sep::by_mode(catalog.iter().filter(|m| m.category == tab))
                .first()
                .and_then(|g| g.1.first().copied())
        })
        .cloned();
    let mut action: Option<Action> = None;

    // ---- Footer: the job, or Separate.
    egui::TopBottomPanel::bottom("separation_footer")
        .frame(
            egui::Frame::none()
                .fill(c(th.panel_raised))
                .stroke(Stroke::new(1.0_f32, c(th.border)))
                .inner_margin(8.0),
        )
        .show_inside(ui, |ui| {
            if let Some(job) = st.separation.job.clone() {
                let (f, ind, status) = job
                    .lock()
                    .map(|g| (g.fraction, g.indeterminate, g.status.clone()))
                    .unwrap_or_default();
                ui.label(RichText::new(&st.separation.job_title).color(c(th.text)));
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        st.separation.cancel.store(true, Ordering::Relaxed);
                        st.separation.queue.clear();
                    }
                    let bar = egui::ProgressBar::new(if ind { 0.0 } else { f })
                        .fill(c(th.accent))
                        .animate(true);
                    ui.add(if ind { bar } else { bar.show_percentage() });
                });
                ui.label(dim(&status));
            } else {
                let src = source(st);
                ui.label(match &src {
                    Some(s) => format!("Separates {}.", s.label),
                    None => "Open a file in the Waveform editor, or select a clip or a file, to separate it."
                        .into(),
                });
                ui.horizontal_wrapped(|ui| {
                    let b = egui::Button::new(
                        RichText::new("Separate").strong().color(c(th.background)),
                    )
                    .fill(c(th.accent));
                    if ui
                        .add_enabled(src.is_some() && model.is_some(), b)
                        .clicked()
                    {
                        if let Some(m) = model.clone() {
                            action = Some(Box::new(move |st| separate(st, m)));
                        }
                    }
                    ui.label(dim(&format!(
                        "On the {}. Runs on this computer; speed and results depend on your hardware.",
                        if st.separation.gpu {
                            "graphics card when it can be used"
                        } else {
                            "processor"
                        }
                    )));
                });
            }
        });

    // ---- The list.
    egui::CentralPanel::default()
        .frame(egui::Frame::none().inner_margin(egui::Margin::symmetric(6.0, 4.0)))
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.label(RichText::new("Separation engine").strong().color(c(th.text_dim)));
                card(ui, &th, false, |ui| {
                    ui.label(RichText::new("Engine (runs the models on this computer)").strong());
                    ui.horizontal_wrapped(|ui| match &engine {
                        Some(p) => {
                            let v = st.separation.engine_version.clone().unwrap_or_default();
                            let ok = sep::engine_is_new_enough(&v);
                            if let Ok(meta) = std::fs::metadata(p) {
                                ui.label(dim(&sep::size_text(meta.len())));
                            }
                            ui.label(RichText::new(format!("Downloaded, version {v}")).color(c(
                                if ok { th.accent } else { th.accent_alt },
                            )));
                            if dirs.is_shared(p) {
                                ui.label(dim("shared with Audiform"));
                            }
                            let up = st.separation.engine_update;
                            if !ok || up == EngineUpdate::Available {
                                ui.label(RichText::new(if ok {
                                    "Update available"
                                } else {
                                    "Too old for this version"
                                })
                                .color(c(th.accent_alt)));
                                if ui.add_enabled(!busy, egui::Button::new("Update")).clicked() {
                                    action = Some(Box::new(|st| {
                                        st.separation.ask =
                                            Some(Ask::Download(Step::InstallEngine))
                                    }));
                                }
                            } else {
                                let label = if up == EngineUpdate::Current {
                                    "Up to date. Check again"
                                } else {
                                    "Check for Update"
                                };
                                if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                                    action = Some(Box::new(|st| start(st, Step::CheckEngine)));
                                }
                            }
                            if std::env::var_os("AUDIFORM_SEPARATE").is_none()
                                && ui.add_enabled(!busy, egui::Button::new("Remove")).clicked()
                            {
                                action = Some(Box::new(|st| {
                                    st.separation.ask = Some(Ask::RemoveEngine)
                                }));
                            }
                        }
                        None => {
                            ui.label(dim("About 10 MB"));
                            let can = !busy && sep::engine_asset().is_some();
                            if ui.add_enabled(can, egui::Button::new("Download")).clicked() {
                                action = Some(Box::new(|st| {
                                    st.separation.ask = Some(Ask::Download(Step::InstallEngine))
                                }));
                            }
                        }
                    });
                });
                ui.add_space(6.0);

                // Tabs.
                ui.horizontal(|ui| {
                    for t in sep::TABS {
                        let on = tab == t;
                        let r = ui.add(
                            egui::SelectableLabel::new(
                                on,
                                RichText::new(t).color(c(if on { th.text } else { th.text_dim })),
                            ),
                        );
                        if r.clicked() && !on {
                            st.separation.tab = t.into();
                            st.separation.save();
                        }
                    }
                });
                ui.separator();

                for (mode, models) in sep::by_mode(catalog.iter().filter(|m| m.category == tab)) {
                    ui.label(RichText::new(mode).size(11.0).strong().color(c(th.text_dim)));
                    for m in models {
                        let copies = installed.get(&m.id).cloned().unwrap_or_default();
                        let on = model.as_ref().is_some_and(|x| x.id == m.id);
                        card(ui, &th, on, |ui| {
                            ui.horizontal(|ui| {
                                let r = ui.radio(on, RichText::new(&m.name).strong());
                                if r.clicked() {
                                    st.separation.model = Some(m.id.clone());
                                    st.separation.save();
                                }
                                r.on_hover_text(hover(m));
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new(sep::sdr_text(m))
                                        .monospace()
                                        .size(10.0)
                                        .color(c(th.accent)),
                                );
                                ui.label(dim(&sep::size_text(m.bytes)));
                                if copies.is_empty() {
                                    let can = !busy && sep::engine_asset().is_some();
                                    if ui
                                        .add_enabled(can, egui::Button::new("Download").small())
                                        .clicked()
                                    {
                                        let m = m.clone();
                                        action = Some(Box::new(move |st| {
                                            st.separation.ask =
                                                Some(Ask::Download(Step::DownloadModel(m)))
                                        }));
                                    }
                                } else {
                                    ui.label(RichText::new("Downloaded").size(11.0).color(c(th.accent)));
                                    if copies.iter().any(|p| dirs.is_shared(p)) {
                                        ui.label(dim("shared with Audiform"));
                                    }
                                    if st.separation.stale.contains(&m.id) {
                                        if ui
                                            .add_enabled(!busy, egui::Button::new("Update").small())
                                            .clicked()
                                        {
                                            let m = m.clone();
                                            action = Some(Box::new(move |st| {
                                                st.separation.ask =
                                                    Some(Ask::Download(Step::DownloadModel(m)))
                                            }));
                                        }
                                    } else if ui
                                        .add_enabled(!busy, egui::Button::new("Check").small())
                                        .on_hover_text("Check the file against its published checksum; offers Update when it no longer matches")
                                        .clicked()
                                    {
                                        let m = m.clone();
                                        action = Some(Box::new(move |st| start(st, Step::CheckModel(m))));
                                    }
                                    if ui
                                        .add_enabled(!busy, egui::Button::new("Remove").small())
                                        .clicked()
                                    {
                                        let m = m.clone();
                                        action = Some(Box::new(move |st| {
                                            st.separation.ask = Some(Ask::RemoveModel(m))
                                        }));
                                    }
                                }
                            });
                        });
                    }
                    ui.add_space(4.0);
                }
                if sep::engine_asset().is_none() {
                    ui.label(dim("Audio Separation is not available for this computer yet."));
                }

                ui.add_space(4.0);
                egui::CollapsingHeader::new("Settings")
                    .id_salt("separation_settings")
                    .show(ui, |ui| {
                        ui.label(dim("Run separation on"));
                        let mut gpu = st.separation.gpu;
                        ui.radio_value(&mut gpu, true, "The graphics card (GPU) when one can be used");
                        ui.radio_value(&mut gpu, false, "The processor (CPU) only");
                        let mut overlap = st.separation.overlap;
                        egui::ComboBox::from_label("Overlap")
                            .selected_text(overlap_text(overlap))
                            .show_ui(ui, |ui| {
                                for o in [1, 2, 4, 8] {
                                    ui.selectable_value(&mut overlap, o, overlap_text(o));
                                }
                            });
                        if gpu != st.separation.gpu || overlap != st.separation.overlap {
                            st.separation.gpu = gpu;
                            st.separation.overlap = overlap;
                            st.separation.save();
                        }
                        ui.label(dim("Each moment is separated this many times in overlapping pieces and blended, which hides the seams between pieces. 2 is what the models were measured with."));
                        ui.label(dim("A graphics card is usually many times faster. If it cannot be used, Audiform27 offers the processor instead."));
                    });
                ui.add_space(4.0);
                ui.label(dim(CONSENT));
            });
        });

    if let Some(f) = action {
        f(st);
    }
    dialogs(ui.ctx(), st);
}

fn overlap_text(o: u32) -> &'static str {
    match o {
        1 => "1: fastest",
        2 => "2: standard",
        4 => "4: smoother, twice as long",
        _ => "8: smoothest, four times as long",
    }
}

fn hover(m: &Model) -> String {
    format!(
        "{}\n{}\n{}\nBy {}. {}\nScore: {}. Higher is better.\n{}",
        m.name,
        if m.stems.is_empty() {
            format!("{} and {}", m.target_name, m.rest_name)
        } else {
            m.output_names().join(", ")
        },
        sep::size_text(m.bytes),
        m.author,
        m.licence,
        m.sdr_note,
        m.page
    )
}

fn card(ui: &mut egui::Ui, th: &crate::theme::Theme, on: bool, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(c(th.panel))
        .stroke(Stroke::new(
            1.0_f32,
            c(if on { th.accent } else { th.border }),
        ))
        .rounding(th.rounding)
        .inner_margin(egui::Margin::symmetric(6.0, 4.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui)
        });
    ui.add_space(3.0);
}

fn dialogs(ctx: &egui::Context, st: &mut State) {
    let Some(ask) = st.separation.ask.take() else {
        return;
    };
    let size = |b: u64| sep::size_text(b);
    let (title, text, yes, no) = match &ask {
        Ask::DownloadFirst { req, engine, model } => {
            let mut parts = Vec::new();
            if *model {
                parts.push(format!("{} ({})", req.model.name, size(req.model.bytes)));
            }
            if *engine {
                parts.push("the separation engine (about 10 MB, once)".into());
            }
            (
                "Download first?".to_string(),
                format!(
                    "To separate with {}, Audiform27 needs to download {} into its own files, where it is kept for next time.\n\n{CONSENT}",
                    req.model.name,
                    parts.join(" and ")
                ),
                "Download",
                "Cancel",
            )
        }
        Ask::Download(step) => {
            let what = match step {
                Step::DownloadModel(m) => format!("{} ({})", m.name, size(m.bytes)),
                _ => "the separation engine (about 10 MB)".into(),
            };
            (
                "Download?".to_string(),
                format!("Audiform27 downloads {what} into its own files, where it is kept for next time.\n\n{CONSENT}"),
                "Download",
                "Cancel",
            )
        }
        Ask::UseCpu(_) => (
            "Use the processor?".to_string(),
            "The graphics card could not be used on this computer. Separate on the processor instead? It is slower. (Settings in this panel can turn the graphics card off.)".to_string(),
            "Use the Processor",
            "Cancel",
        ),
        Ask::AddTracks { outputs, .. } => {
            let names: Vec<String> = outputs.iter().map(|(n, _)| n.to_lowercase()).collect();
            let what = if names.len() == 2 {
                format!("the {} and {} on two", names[0], names[1])
            } else {
                format!("the {} results on {}", names.len(), names.len())
            };
            (
                "Add as new tracks?".to_string(),
                format!("Put {what} new tracks below the clip's track? They copy that track's effects and settings."),
                "Add Tracks",
                "Files Only",
            )
        }
        Ask::RemoveModel(m) => {
            let shared = st.sep_dirs.model_copies(m).iter().any(|p| st.sep_dirs.is_shared(p));
            (
                format!("Remove {}?", m.name),
                format!(
                    "This deletes the downloaded model ({}). You can download it again any time.{}",
                    size(m.bytes),
                    if shared {
                        " It is in Audiform's folder, so Audiform loses it too."
                    } else {
                        ""
                    }
                ),
                "Remove",
                "Cancel",
            )
        }
        Ask::RemoveEngine => {
            let shared = st.sep_dirs.engine().is_some_and(|p| st.sep_dirs.is_shared(&p));
            (
                "Remove the separation engine?".to_string(),
                format!(
                    "This deletes the downloaded separation engine. Your models stay. It is downloaded again when you next separate.{}",
                    if shared {
                        " It is in Audiform's folder, so Audiform loses it too."
                    } else {
                        ""
                    }
                ),
                "Remove",
                "Cancel",
            )
        }
    };
    let mut answer = None;
    egui::Window::new(title)
        .id(egui::Id::new("separation_ask"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_max_width(380.0);
            ui.label(text);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new(yes).strong()).clicked() {
                    answer = Some(true);
                }
                if ui.button(no).clicked() {
                    answer = Some(false);
                }
            });
        });
    match answer {
        None => st.separation.ask = Some(ask),
        Some(false) => {}
        Some(true) => match ask {
            Ask::DownloadFirst { req, engine, model } => {
                let q = &mut st.separation.queue;
                q.clear();
                if engine {
                    q.push(Step::InstallEngine);
                }
                if model {
                    q.push(Step::DownloadModel(req.model.clone()));
                }
                q.push(Step::Separate(req));
                next(st);
            }
            Ask::Download(step) => start(st, step),
            Ask::UseCpu(mut req) => {
                req.gpu = false;
                start(st, Step::Separate(req));
            }
            Ask::AddTracks {
                clips,
                media,
                outputs,
            } => add_tracks(st, &clips, media, &outputs),
            Ask::RemoveModel(m) => {
                match sep::remove_model(&st.sep_dirs, &m) {
                    Ok(()) => st.toast(format!("Removed {}.", m.name)),
                    Err(e) => st.error(e),
                }
                st.separation.refresh();
            }
            Ask::RemoveEngine => {
                match sep::remove_engine(&st.sep_dirs) {
                    Ok(()) => st.toast("Removed the separation engine."),
                    Err(e) => st.error(e),
                }
                st.separation.engine_update = EngineUpdate::Unknown;
                st.separation.refresh();
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separated_tracks_copy_the_track_under_it() {
        let mut s = Session::new("t", 48_000, 2);
        let mid = s.alloc_id();
        s.media.push(af27_core::session::MediaItem {
            id: mid,
            name: "song.wav".into(),
            path: Some(PathBuf::from("song.wav")),
            duration: 10.0,
            rate: 44_100,
            channels: 2,
            ext_id: None,
        });
        let t0 = s.tracks[0].id;
        let clip = s.add_clip(t0, mid, 2.0).unwrap();
        s.tracks[0].volume_db = -3.0;
        s.tracks[0].solo = true;
        s.tracks[0].chain.push(af27_core::EffectSlot {
            effect: "gain".into(),
            values: vec![1.0],
            bypass: false,
            data: None,
            ext_id: None,
        });
        let outs = vec![
            ("Vocals".to_string(), PathBuf::from("song (Vocals).wav")),
            (
                "Instrumental".to_string(),
                PathBuf::from("song (Instrumental).wav"),
            ),
        ];
        let made = add_separated_tracks(&mut s, &[clip], mid, &outs);
        assert_eq!(made.len(), 2);
        assert_eq!(s.tracks.len(), 4);
        assert_eq!(s.tracks[1].name, "Track 1 Vocals");
        assert_eq!(s.tracks[2].name, "Track 1 Instrumental");
        assert_eq!(s.tracks[3].name, "Track 2");
        let v = &s.tracks[1];
        assert_eq!(v.volume_db, -3.0);
        assert!(!v.solo);
        assert_eq!(v.chain.len(), 1);
        assert_eq!(v.clips.len(), 1);
        assert_eq!(v.clips[0].start, 2.0);
        assert_eq!(v.clips[0].media, made[0].0);
        assert_ne!(v.clips[0].id, clip);
        assert_eq!(s.media(made[1].0).unwrap().rate, 44_100);
        // The original is untouched.
        assert_eq!(s.tracks[0].clips[0].media, mid);
    }

    #[test]
    fn nothing_is_added_for_unknown_media() {
        let mut s = Session::new("t", 48_000, 1);
        assert!(add_separated_tracks(&mut s, &[1], 99, &[]).is_empty());
        assert_eq!(s.tracks.len(), 1);
    }
}
