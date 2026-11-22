// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The export dialog (File > Save Audio As, Export > Multitrack Mixdown,
//! Track Stems and the 3D speaker bed) and the background jobs that write
//! the files, plus Bounce Selected Track to New Track. Settings are kept
//! per format in `export.json` in the data folder.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};

use af2_core::{AudioBuffer, Session, TrackKind};
use af2_io::export::{
    self as ex, ChannelChoice, Container, LoudnessTarget, MarkerOut, Meta, Options, Report,
    SampleFormat, Tag,
};
use egui::RichText;
use serde::{Deserialize, Serialize};

use crate::state::{data_dir, DocId, Mode, State};

/// The settings one format remembers.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FormatPrefs {
    pub rate: Option<u32>,
    pub channels: ChannelChoice,
    pub format: SampleFormat,
    pub dither: bool,
    pub flac_level: u8,
}

impl Default for FormatPrefs {
    fn default() -> Self {
        FormatPrefs {
            rate: None,
            channels: ChannelChoice::Keep,
            format: SampleFormat::I24,
            dither: true,
            // The smallest files: every level is lossless.
            flac_level: 8,
        }
    }
}

/// What a mixdown takes from the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MixRange {
    #[default]
    Session,
    Selection,
    SelectedClips,
}

/// The loudness option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Loudness {
    #[default]
    Off,
    Preset(usize),
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportPrefs {
    pub container: Container,
    pub wav: FormatPrefs,
    pub flac: FormatPrefs,
    pub loudness: Loudness,
    pub custom: LoudnessTarget,
    pub range: MixRange,
    pub folder: Option<PathBuf>,
}

impl Default for ExportPrefs {
    fn default() -> Self {
        ExportPrefs {
            container: Container::Flac,
            wav: FormatPrefs::default(),
            flac: FormatPrefs {
                format: SampleFormat::I16,
                ..FormatPrefs::default()
            },
            loudness: Loudness::Off,
            custom: LoudnessTarget {
                lufs: -16.0,
                true_peak_db: -1.0,
            },
            range: MixRange::Session,
            folder: None,
        }
    }
}

impl ExportPrefs {
    fn path() -> PathBuf {
        data_dir().join("export.json")
    }

    pub fn load() -> ExportPrefs {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(data_dir());
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), s);
        }
    }

    pub fn fmt(&self, c: Container) -> &FormatPrefs {
        match c {
            Container::Wav => &self.wav,
            Container::Flac => &self.flac,
        }
    }

    pub fn fmt_mut(&mut self, c: Container) -> &mut FormatPrefs {
        match c {
            Container::Wav => &mut self.wav,
            Container::Flac => &mut self.flac,
        }
    }

    pub fn loudness_target(&self) -> Option<LoudnessTarget> {
        match self.loudness {
            Loudness::Off => None,
            Loudness::Preset(i) => ex::LOUDNESS_PRESETS.get(i).map(|p| p.1),
            Loudness::Custom => Some(self.custom),
        }
    }

    pub fn options(&self) -> Options {
        let f = self.fmt(self.container);
        Options {
            container: self.container,
            rate: f.rate,
            channels: f.channels,
            format: f.format,
            dither: f.dither,
            flac_level: f.flac_level,
            loudness: self.loudness_target(),
            channel_mask: None,
        }
    }
}

/// What the dialog is writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    SaveDoc(DocId),
    Mixdown,
    Bed,
    Stems,
}

/// What happens when a job is done.
enum Then {
    SaveDoc { doc: DocId, path: PathBuf },
    Files,
    /// A mixdown: when written, it opens in the Waveform editor.
    Mixdown(PathBuf),
    Bounce { name: String, at: f64 },
    /// A session archive; `note` says what went in.
    Archive(PathBuf),
}

struct Done {
    reports: Vec<Report>,
    /// What to tell the user when it's done, in place of the reports'.
    note: Option<String>,
    /// The file as written, when the editor should take it (Save As with
    /// a conversion; a bounce).
    reload: Option<AudioBuffer>,
}

/// A file (or files) being written in the background.
pub struct Job {
    label: String,
    rx: mpsc::Receiver<Result<Done, String>>,
    done: Arc<AtomicU64>,
    total: u64,
    cancel: Arc<AtomicBool>,
    then: Then,
    float: bool,
}

#[derive(Default)]
pub struct ExportUi {
    pub open: bool,
    purpose: Option<Purpose>,
    name: String,
    folder: PathBuf,
    prefs: Option<ExportPrefs>,
    /// Files that exist, waiting for "Replace".
    confirm: Option<Vec<PathBuf>>,
    pub job: Option<Job>,
    /// 3D Space: what the speaker bed export writes.
    pub bed: crate::space_export::BedOptions,
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn stem_of(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string())
}

fn with_ext(name: &str, c: Container) -> String {
    // Only an audio extension comes off: "Song v1.2 Mixdown" keeps its
    // whole name (its ".2 Mixdown" is not an extension).
    let stem = if Container::from_path(Path::new(name)).is_some() {
        stem_of(name)
    } else {
        name.to_string()
    };
    format!("{stem}.{}", c.extension())
}

// ------------------------------------------------------------ opening

fn open_with(st: &mut State, purpose: Purpose, name: String, folder: Option<PathBuf>) {
    let prefs = ExportPrefs::load();
    let folder = folder.or_else(|| prefs.folder.clone()).unwrap_or_else(home);
    let ui = &mut st.export;
    ui.name = with_ext(&ex::safe_file_name(&name), prefs.container);
    ui.folder = folder;
    ui.prefs = Some(prefs);
    ui.purpose = Some(purpose);
    ui.confirm = None;
    ui.open = true;
}

/// File > Export > Multitrack Session (.sesx): a copy of the session for
/// the other editor. The session itself is not changed and stays tied to
/// its own file.
pub fn export_sesx(st: &mut State) {
    let folder = st
        .session_path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(|| {
            st.sessions
                .source
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
        })
        .unwrap_or_else(home);
    let name = format!("{}.sesx", ex::safe_file_name(&st.session.name));
    let Some(path) = crate::file_dialog::FileDialog::new()
        .set_title("Export Multitrack Session")
        .set_directory(&folder)
        .set_file_name(name)
        .add_filter("Multitrack session (.sesx)", &["sesx"])
        .save_file()
    else {
        return;
    };
    let path = if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("sesx"))
    {
        path
    } else {
        let mut p = path.into_os_string();
        p.push(".sesx");
        PathBuf::from(p)
    };
    crate::plugins::capture_all(st);
    match af2_interop::sesx_out::export(&st.session, &path) {
        Ok(report) => {
            st.toast(format!(
                "Exported {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
            if let Some(n) = report.notice() {
                st.toast(n);
            }
        }
        Err(e) => st.error(e),
    }
}

/// File > Save Audio As.
pub fn open_save_as(st: &mut State) {
    let Some(d) = st.doc() else {
        st.error("Open a file in the Waveform editor first.");
        return;
    };
    let (id, name, folder) = (
        d.id,
        stem_of(&d.name),
        d.path
            .as_ref()
            .and_then(|p| p.parent().map(Path::to_path_buf)),
    );
    open_with(st, Purpose::SaveDoc(id), name, folder);
    // A file already in FLAC or WAV keeps its format; one never saved
    // starts as FLAC (lossless and compressed) at the depth it was made.
    let (ext, new_bits) = st.doc().map_or((None, None), |d| {
        (d.path.as_deref().and_then(Container::from_path), d.new_bits)
    });
    let unsaved = st.doc().is_some_and(|d| d.path.is_none());
    let c = ext.or(unsaved.then_some(Container::Flac));
    if let (Some(c), Some(p)) = (c, st.export.prefs.as_mut()) {
        p.container = c;
        if c == Container::Flac {
            if let Some(b) = new_bits {
                p.flac.format = if b == 16 {
                    SampleFormat::I16
                } else {
                    SampleFormat::I24
                };
            }
        }
        st.export.name = with_ext(&st.export.name, c);
    }
}

pub fn open_mixdown(st: &mut State) {
    let name = format!("{} Mixdown", st.session.name);
    let folder = st
        .session_path
        .as_ref()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    open_with(st, Purpose::Mixdown, name, folder);
}

pub fn open_bed(st: &mut State) {
    let name = format!("{} Bed", st.session.name);
    let folder = st
        .session_path
        .as_ref()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    open_with(st, Purpose::Bed, name, folder);
}

pub fn open_stems(st: &mut State) {
    if stem_tracks(&st.session).is_empty() {
        st.error("No unmuted track has clips to export.");
        return;
    }
    let folder = st
        .session_path
        .as_ref()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let name = st.session.name.clone();
    open_with(st, Purpose::Stems, name, folder);
}

/// File > Save in the Waveform editor: a WAV or FLAC saves in place with
/// that format's remembered depth (rate and channels kept); anything else
/// goes through Save Audio As.
pub fn save_in_place(st: &mut State) {
    let Some(d) = st.doc() else { return };
    let Some((path, c)) = d
        .path
        .clone()
        .and_then(|p| Container::from_path(&p).map(|c| (p, c)))
    else {
        open_save_as(st);
        return;
    };
    let prefs = ExportPrefs::load();
    let f = prefs.fmt(c);
    let opts = Options {
        container: c,
        format: f.format,
        dither: f.dither,
        flac_level: f.flac_level,
        ..Options::default()
    };
    let id = d.id;
    start_save_doc(st, id, path, opts);
}

// ------------------------------------------------------------ tracks

fn has_clips(t: &af2_core::Track) -> bool {
    !t.clips.is_empty() || !t.midi.is_empty()
}

/// Tracks that become stems: unmuted, not buses, with clips.
pub fn stem_tracks(s: &Session) -> Vec<usize> {
    s.tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| !t.mute && t.kind != TrackKind::Bus && has_clips(t))
        .map(|(i, _)| i)
        .collect()
}

/// The session with only track `ti` playing (solo ignored); buses stay
/// as they are so a track routed through one still reaches the master.
pub fn only_track(s: &Session, ti: usize) -> Session {
    let mut s = s.clone();
    for (i, t) in s.tracks.iter_mut().enumerate() {
        t.solo = false;
        if i == ti {
            t.mute = false;
        } else if t.kind != TrackKind::Bus {
            t.mute = true;
        }
    }
    s
}

/// The session with only the selected clip(s) left, and their span.
fn only_clips(s: &Session, keep: &[af2_core::ClipId]) -> (Session, f64, f64) {
    let mut s = s.clone();
    let (mut a, mut b) = (f64::MAX, 0.0f64);
    for t in &mut s.tracks {
        t.clips.retain(|c| keep.contains(&c.id));
        t.midi.clear();
        for c in &t.clips {
            a = a.min(c.start);
            b = b.max(c.end());
        }
    }
    if a > b {
        a = 0.0;
        b = 0.0;
    }
    (s, a, b)
}

fn session_meta(s: &Session, from: f64, to: f64, title: &str) -> Meta {
    Meta {
        markers: s
            .markers
            .iter()
            .filter(|m| m.time >= from && m.time <= to)
            .map(|m| MarkerOut {
                name: m.name.clone(),
                time: m.time - from,
                length: m.length.min(to - m.time),
                chapter: m.kind == af2_core::session::MarkerKind::Chapter,
            })
            .collect(),
        tags: tags(title),
    }
}

fn tags(title: &str) -> Vec<(Tag, String)> {
    vec![
        (Tag::Title, title.to_string()),
        (Tag::Software, "AudiForm 2".to_string()),
    ]
}

/// The mix range a mixdown covers, and the session to render.
fn mix_source(st: &State, range: MixRange) -> (Session, f64, f64) {
    match range {
        MixRange::Selection => {
            if let Some((a, b)) = st.session_sel {
                return (st.session.clone(), a.min(b), a.max(b));
            }
        }
        MixRange::SelectedClips => {
            if let Some(c) = st.selected_clip {
                return only_clips(&st.session, &[c]);
            }
        }
        MixRange::Session => {}
    }
    (st.session.clone(), 0.0, st.session.duration())
}

// ------------------------------------------------------------ jobs

/// Renders `program` from `from` to `to` into a file. Written as it
/// renders unless the options need the whole mix first.
#[allow(clippy::too_many_arguments)]
fn render_to_file(
    program: af2_engine::Program,
    rate: u32,
    from: f64,
    to: f64,
    bed: bool,
    path: &Path,
    opts: &Options,
    meta: &Meta,
    done: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<Report, String> {
    let ch = af2_engine::render_channels(&program, bed);
    let stopped = || "Cancelled.".to_string();
    if opts.needs_whole(rate) || opts.channels.count(ch) != ch {
        let mut inter = Vec::new();
        af2_engine::render_each(program, rate, from, to, bed, |b| {
            inter.extend_from_slice(b);
            done.fetch_add((b.len() / ch) as u64 / 2, Ordering::Relaxed);
            !cancel.load(Ordering::Relaxed)
        });
        if cancel.load(Ordering::Relaxed) {
            return Err(stopped());
        }
        let buf = AudioBuffer::from_interleaved(rate, ch, &inter);
        drop(inter);
        let r = ex::export_buffer(path, &buf, opts, meta);
        done.fetch_add(buf.frames() as u64 / 2, Ordering::Relaxed);
        return r;
    }
    let mut w = ex::Exporter::create(
        path,
        opts.container,
        opts.effective_format(),
        rate,
        ch,
        opts,
        meta,
    )?;
    let mut err = None;
    af2_engine::render_each(program, rate, from, to, bed, |b| {
        if let Err(e) = w.push(b) {
            err = Some(e);
            return false;
        }
        done.fetch_add((b.len() / ch) as u64, Ordering::Relaxed);
        !cancel.load(Ordering::Relaxed)
    });
    if let Some(e) = err {
        return Err(e);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(stopped());
    }
    let rep = w.finish()?;
    ex::readback_check(path, rep.frames, rep.channels)?;
    Ok(rep)
}

fn spawn(
    st: &mut State,
    label: String,
    total: u64,
    then: Then,
    float: bool,
    work: impl FnOnce(&AtomicU64, &AtomicBool) -> Result<Done, String> + Send + 'static,
    cleanup: Vec<PathBuf>,
) {
    let (tx, rx) = mpsc::channel();
    let done = Arc::new(AtomicU64::new(0));
    let cancel = Arc::new(AtomicBool::new(false));
    let (d2, c2) = (done.clone(), cancel.clone());
    std::thread::spawn(move || {
        let r = work(&d2, &c2);
        if r.is_err() {
            // A cancelled or failed export leaves no partial file.
            for p in &cleanup {
                let _ = std::fs::remove_file(p);
            }
        }
        let _ = tx.send(r);
    });
    st.export.job = Some(Job {
        label,
        rx,
        done,
        total: total.max(1),
        cancel,
        then,
        float,
    });
}

/// Runs an archive build as the export job (one at a time, with the same
/// progress window and Cancel).
pub fn spawn_archive(
    st: &mut State,
    total: u64,
    out: PathBuf,
    work: impl FnOnce(&AtomicU64, &AtomicBool) -> Result<crate::archive::Summary, String> + Send + 'static,
) {
    let name = prefs_name(&out);
    spawn(
        st,
        format!("Archiving {name}"),
        total,
        Then::Archive(out.clone()),
        false,
        move |d, c| {
            let s = work(d, c)?;
            let mut note = format!("Archived to {} ({}", out.display(), ex::human_bytes(s.bytes));
            if s.source_bytes > 0 {
                note.push_str(&format!(", from {} of audio", ex::human_bytes(s.source_bytes)));
            }
            note.push(')');
            if s.committed > 0 {
                note.push_str(&format!("; {} tracks rendered", s.committed));
            }
            if s.bounced > 0 {
                note.push_str(&format!("; {} buses bounced", s.bounced));
            }
            let _ = s.files;
            Ok(Done { reports: Vec::new(), reload: None, note: Some(note) })
        },
        Vec::new(),
    );
}

fn start_save_doc(st: &mut State, id: DocId, path: PathBuf, mut opts: Options) {
    if st.export.job.is_some() {
        st.error("An export is still running.");
        return;
    }
    let Some(d) = st.doc_by_id(id) else { return };
    let buf = d.buf.clone();
    // The file's own speaker layout (5.1.2 and 7.1 both have eight
    // channels), while its channels are kept.
    if opts.channels.count(buf.num_channels()) == buf.num_channels() {
        opts.channel_mask = opts.channel_mask.or(d.speaker_mask());
    }
    let meta = Meta {
        markers: d
            .markers
            .iter()
            .map(|m| MarkerOut {
                name: m.name.clone(),
                time: m.time,
                length: m.length,
                chapter: m.kind == af2_core::session::MarkerKind::Chapter,
            })
            .collect(),
        tags: tags(&stem_of(
            &path.file_name().unwrap_or_default().to_string_lossy(),
        )),
    };
    let changes = opts.loudness.is_some()
        || opts.rate.is_some_and(|r| r != buf.rate)
        || opts.channels.count(buf.num_channels()) != buf.num_channels();
    let total = buf.frames() as u64;
    let p2 = path.clone();
    let float = opts.effective_format() == SampleFormat::F32;
    spawn(
        st,
        format!("Saving {}", path.display()),
        total,
        Then::SaveDoc {
            doc: id,
            path: path.clone(),
        },
        float,
        move |done, _| {
            let rep = ex::export_buffer(&p2, &buf, &opts, &meta)?;
            done.store(total, Ordering::Relaxed);
            let reload = if changes {
                Some(af2_io::decode_file(&p2)?)
            } else {
                None
            };
            Ok(Done {
                note: None,
                reports: vec![rep],
                reload,
            })
        },
        vec![path],
    );
}

/// Multitrack > Bounce Selected Track to New Track: the selected track
/// alone (its effects, fader and pan, the master) over the time
/// selection or the whole session, onto a new track.
pub fn bounce_track(st: &mut State) {
    if st.export.job.is_some() {
        st.error("An export is still running.");
        return;
    }
    let Some(ti) = st
        .selected_track
        .and_then(|id| st.session.tracks.iter().position(|t| t.id == id))
    else {
        st.error("Select a track to bounce.");
        return;
    };
    if !has_clips(&st.session.tracks[ti]) {
        st.error("The selected track has no clips.");
        return;
    }
    let (from, to) = st
        .session_sel
        .map(|(a, b)| (a.min(b), a.max(b)))
        .unwrap_or((0.0, st.session.duration()));
    if to - from <= 0.0 {
        return;
    }
    let s = only_track(&st.session, ti);
    let rate = st.session.rate;
    let program = af2_engine::mix_program(&s, &|m| st.media_buffer(m), rate);
    let name = format!("{} Bounce", st.session.tracks[ti].name);
    let dir = data_dir().join("bounces");
    let _ = std::fs::create_dir_all(&dir);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("{} {stamp}.wav", ex::safe_file_name(&name)));
    let total = ((to - from) * rate as f64) as u64;
    let p2 = path.clone();
    spawn(
        st,
        format!("Bouncing {name}"),
        total,
        Then::Bounce { name, at: from },
        true,
        move |done, cancel| {
            let opts = Options {
                format: SampleFormat::F32,
                ..Options::default()
            };
            let rep = render_to_file(
                program,
                rate,
                from,
                to,
                false,
                &p2,
                &opts,
                &Meta::default(),
                done,
                cancel,
            )?;
            let buf = af2_io::decode_file(&p2)?;
            Ok(Done {
                note: None,
                reports: vec![rep],
                reload: Some(buf),
            })
        },
        vec![path],
    );
}

/// Starts the dialog's export.
fn start(st: &mut State, paths: Vec<PathBuf>) {
    let Some(purpose) = st.export.purpose else {
        return;
    };
    let prefs = st.export.prefs.clone().unwrap_or_default();
    prefs.save();
    let opts = prefs.options();
    let float = opts.effective_format() == SampleFormat::F32;
    let rate = st.session.rate;
    match purpose {
        Purpose::SaveDoc(id) => {
            start_save_doc(st, id, paths[0].clone(), opts);
        }
        Purpose::Mixdown | Purpose::Bed => {
            let bed = purpose == Purpose::Bed;
            let (mut s, from, to) =
                mix_source(st, if bed { MixRange::Session } else { prefs.range });
            let (from, to) = if bed {
                st.session_sel.unwrap_or((from, to))
            } else {
                (from, to)
            };
            if to - from <= 0.0 {
                st.error("There is nothing to mix in that range.");
                return;
            }
            let mut opts = opts;
            // 3D Space: the bed in the layout chosen for this export (or a
            // binaural file); a stereo mixdown is the plain fold, whatever
            // the session is listened to on.
            let (bed, order) = if bed {
                if let Some(why) = st.export.bed.refuse(s.space, opts.container) {
                    st.error(why);
                    return;
                }
                let (bed, mask, order) = st.export.bed.prepare(&mut s);
                if bed && opts.channels == ChannelChoice::Keep {
                    opts.channel_mask = mask;
                }
                (bed, order)
            } else {
                if s.space.is_some() {
                    s.monitor = af2_core::space::Monitor::StereoFold;
                }
                (false, None)
            };
            let mut program = af2_engine::mix_program(&s, &|m| st.media_buffer(m), rate);
            if let af2_engine::Program::Mix(m) = &mut program {
                m.set_bed_order(order);
            }
            let path = paths[0].clone();
            let meta = session_meta(&s, from, to, &stem_of(&prefs_name(&path)));
            let total = ((to - from) * rate as f64) as u64;
            let p2 = path.clone();
            // Exported over a file the editor has open (a second mixdown
            // to the same name): read it back here, as opening it again
            // would only bring the old copy to the front.
            let read_back = st.doc_for_path(&path).is_some();
            spawn(
                st,
                format!("Exporting {}", path.display()),
                total,
                Then::Mixdown(path.clone()),
                float,
                move |done, cancel| {
                    let r = render_to_file(
                        program, rate, from, to, bed, &p2, &opts, &meta, done, cancel,
                    )?;
                    let reload = if read_back {
                        Some(af2_io::open_file(&p2)?.buf)
                    } else {
                        None
                    };
                    Ok(Done {
                        note: None,
                        reports: vec![r],
                        reload,
                    })
                },
                vec![path],
            );
        }
        Purpose::Stems => {
            let tracks = stem_tracks(&st.session);
            let to = st.session.duration();
            let mut jobs = Vec::new();
            for (ti, path) in tracks.iter().zip(&paths) {
                let s = only_track(&st.session, *ti);
                let program = af2_engine::mix_program(&s, &|m| st.media_buffer(m), rate);
                let meta = session_meta(&s, 0.0, to, &st.session.tracks[*ti].name);
                jobs.push((program, path.clone(), meta));
            }
            let total = (to * rate as f64) as u64 * jobs.len() as u64;
            spawn(
                st,
                format!("Exporting {} stems", jobs.len()),
                total,
                Then::Files,
                float,
                move |done, cancel| {
                    let mut reports = Vec::new();
                    for (program, path, meta) in jobs {
                        reports.push(render_to_file(
                            program, rate, 0.0, to, false, &path, &opts, &meta, done, cancel,
                        )?);
                    }
                    Ok(Done {
                        reports,
                        reload: None,
                        note: None,
                    })
                },
                paths,
            );
        }
    }
}

fn prefs_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Picks up a finished job.
pub fn poll(st: &mut State, ctx: &egui::Context) {
    let Some(job) = &st.export.job else { return };
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    let r = match job.rx.try_recv() {
        Ok(r) => r,
        Err(mpsc::TryRecvError::Empty) => return,
        Err(mpsc::TryRecvError::Disconnected) => Err("The export stopped unexpectedly.".into()),
    };
    let job = st.export.job.take().unwrap();
    let done = match r {
        Ok(d) => d,
        Err(e) => {
            if e == "Cancelled." {
                st.toast("Export cancelled");
            } else {
                st.error(e);
            }
            return;
        }
    };
    let mut notes = Vec::new();
    for rep in &done.reports {
        if let Some(n) = rep.clip_note(job.float) {
            notes.push(format!("{}: {n}", prefs_name(&rep.path)));
        }
        if let Some(l) = rep.loudness {
            let mut s = match l.result_lufs {
                Some(v) => format!(
                    "{}: {v:.1} LUFS, true peak {:.1} dBTP",
                    prefs_name(&rep.path),
                    l.true_peak_db
                ),
                None => format!("{}: too quiet to measure loudness", prefs_name(&rep.path)),
            };
            if l.peak_limited {
                s.push_str(" (held back by the true-peak ceiling)");
            }
            notes.push(s);
        }
    }
    match job.then {
        Then::SaveDoc { doc, path } => {
            if let Some(d) = st.docs.iter_mut().find(|d| d.id == doc) {
                if let Some(b) = done.reload {
                    d.set_buffer("Save As conversion", b);
                }
                d.path = Some(path.clone());
                d.name = prefs_name(&path);
                d.dirty = false;
            }
            st.toast(format!("Saved {}", prefs_name(&path)));
        }
        Then::Files => {
            let n = done.reports.len();
            let bytes: u64 = done.reports.iter().map(|r| r.bytes).sum();
            let text = if n == 1 {
                format!(
                    "Exported {} ({})",
                    done.reports[0].path.display(),
                    ex::human_bytes(bytes)
                )
            } else {
                format!("Exported {n} files ({})", ex::human_bytes(bytes))
            };
            match done.reports.first() {
                Some(r) => st.toast_file(text, r.path.clone()),
                None => st.toast(text),
            }
        }
        Then::Mixdown(path) => {
            let bytes: u64 = done.reports.iter().map(|r| r.bytes).sum();
            st.toast_file(format!("Exported {} ({})", path.display(), ex::human_bytes(bytes)), path.clone());
            // The mixdown comes up in the Waveform editor to listen to.
            match (st.doc_for_path(&path), done.reload) {
                (Some(id), Some(buf)) => {
                    if let Some(d) = st.docs.iter_mut().find(|d| d.id == id) {
                        d.set_buffer("Mixdown exported again", buf);
                        d.dirty = false;
                    }
                    st.current = Some(id);
                }
                (Some(id), None) => st.current = Some(id),
                (None, _) => st.load_file(path, None),
            }
            st.mode = Mode::Waveform;
        }
        Then::Archive(path) => {
            let text = done.note.unwrap_or_else(|| format!("Archived to {}", path.display()));
            st.toast_file(text, path);
        }
        Then::Bounce { name, at } => {
            if let (Some(buf), Some(rep)) = (done.reload, done.reports.first()) {
                let id = st.add_doc(name.clone(), Some(rep.path.clone()), buf);
                let t = st.add_track_kind(TrackKind::Audio);
                if let Some(tr) = st.session.track_mut(t) {
                    tr.name = name.clone();
                }
                st.place_doc(id, t, at);
                st.mode = Mode::Multitrack;
                st.toast(format!("Bounced to new track {name}"));
            }
        }
    }
    for n in notes {
        if n.contains("Clipped") {
            st.error(n);
        } else {
            st.toast(n);
        }
    }
}

// ------------------------------------------------------------ dialog

fn title(p: Purpose) -> &'static str {
    match p {
        Purpose::SaveDoc(_) => "Save Audio As",
        Purpose::Mixdown => "Export Multitrack Mixdown",
        Purpose::Bed => "Export 3D Speaker Bed",
        Purpose::Stems => "Export Track Stems",
    }
}

/// The paths the dialog would write.
fn targets(st: &State, purpose: Purpose, c: Container) -> Vec<PathBuf> {
    let ui = &st.export;
    match purpose {
        Purpose::Stems => stem_tracks(&st.session)
            .iter()
            .enumerate()
            .map(|(n, &ti)| {
                ui.folder.join(ex::stem_file_name(
                    &st.session.name,
                    n + 1,
                    &st.session.tracks[ti].name,
                    c,
                ))
            })
            .collect(),
        _ => vec![ui.folder.join(with_ext(&ex::safe_file_name(&ui.name), c))],
    }
}

/// Source length (seconds), channels and rate, for the summary.
fn source_shape(st: &State, purpose: Purpose, range: MixRange) -> (f64, usize, u32, usize) {
    match purpose {
        Purpose::SaveDoc(id) => st.doc_by_id(id).map_or((0.0, 2, 48_000, 1), |d| {
            (d.duration(), d.buf.num_channels(), d.buf.rate, 1)
        }),
        Purpose::Mixdown => {
            let (_, a, b) = mix_source(st, range);
            (b - a, 2, st.session.rate, 1)
        }
        Purpose::Bed => {
            let ch = st.export.bed.channels(st.session.space);
            let (a, b) = st.session_sel.unwrap_or((0.0, st.session.duration()));
            (b - a, ch, st.session.rate, 1)
        }
        Purpose::Stems => (
            st.session.duration(),
            2,
            st.session.rate,
            stem_tracks(&st.session).len(),
        ),
    }
}

fn rate_label(r: Option<u32>) -> String {
    match r {
        None => "Keep".into(),
        Some(r) if r % 1000 == 0 => format!("{} kHz", r / 1000),
        Some(r) => format!("{:.1} kHz", r as f64 / 1000.0),
    }
}

/// Draws the dialog, or the progress of a running job.
pub fn show(ctx: &egui::Context, st: &mut State) {
    if let Some(job) = &st.export.job {
        let frac = job.done.load(Ordering::Relaxed) as f32 / job.total as f32;
        let mut cancel = false;
        crate::ui::popup(
            ctx,
            egui::Id::new(("af2-popup", "export.rs", 0)),
            "Exporting",
        )
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(&job.label);
            ui.add(egui::ProgressBar::new(frac.min(1.0)).show_percentage());
            cancel = ui.button("Cancel").clicked();
        });
        if cancel {
            job.cancel.store(true, Ordering::Relaxed);
        }
        return;
    }
    if !st.export.open {
        return;
    }
    let Some(purpose) = st.export.purpose else {
        st.export.open = false;
        return;
    };
    let mut open = true;
    let mut go = false;
    let mut close = false;
    let mut replace = false;
    let has_sel = st.session_sel.is_some_and(|(a, b)| (b - a).abs() > 1e-6);
    let has_clip = st.selected_clip.is_some();
    let mut prefs = st.export.prefs.clone().unwrap_or_default();
    let (secs, src_ch, src_rate, files) = source_shape(st, purpose, prefs.range);
    let confirm = st.export.confirm.clone();
    let session_name = st.session.name.clone();
    let session_layout = st.session.space;
    crate::ui::popup(ctx, egui::Id::new(("af2-popup", "export.rs", 1)), title(purpose))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            let ex_ui = &mut st.export;
            egui::Grid::new("export-grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    if purpose == Purpose::Stems {
                        ui.label("Names");
                        ui.label(ex::stem_file_name(&session_name, 1, "Track", prefs.container));
                        ui.end_row();
                    } else {
                        ui.label("File name");
                        let r = ui.add(egui::TextEdit::singleline(&mut ex_ui.name).desired_width(260.0));
                        if r.changed() {
                            // Typing ".flac" or ".wav" picks the format.
                            if let Some(c) = Container::from_path(Path::new(&ex_ui.name)) {
                                prefs.container = c;
                            }
                        }
                        ui.end_row();
                    }
                    ui.label("Location");
                    ui.horizontal(|ui| {
                        ui.label(ex_ui.folder.display().to_string());
                        if ui.button("Browse...").clicked() {
                            if purpose == Purpose::Stems {
                                if let Some(d) = crate::file_dialog::FileDialog::new()
                                    .set_directory(&ex_ui.folder)
                                    .pick_folder()
                                {
                                    ex_ui.folder = d;
                                }
                            } else if let Some(p) = crate::file_dialog::FileDialog::new()
                                .set_directory(&ex_ui.folder)
                                .set_file_name(&ex_ui.name)
                                .add_filter("WAV", &["wav"])
                                .add_filter("FLAC", &["flac"])
                                .save_file()
                            {
                                if let Some(c) = Container::from_path(&p) {
                                    prefs.container = c;
                                }
                                if let Some(d) = p.parent() {
                                    ex_ui.folder = d.to_path_buf();
                                }
                                ex_ui.name = prefs_name(&p);
                            }
                        }
                    });
                    ui.end_row();

                    ui.label("Format");
                    let before = prefs.container;
                    egui::ComboBox::from_id_salt("export-format")
                        .selected_text(prefs.container.label())
                        .show_ui(ui, |ui| {
                            for c in Container::ALL {
                                ui.selectable_value(&mut prefs.container, c, c.label());
                            }
                            ui.add_enabled(false, egui::SelectableLabel::new(false, "MP3 (needs a licence decision)"))
                                .on_disabled_hover_text(
                                    "MP3 needs an encoder whose licence fits AudiForm 2's. The usual one (LAME) is LGPL, so MP3 waits for a decision.",
                                );
                        });
                    if prefs.container != before {
                        ex_ui.name = with_ext(&ex_ui.name, prefs.container);
                    }
                    ui.end_row();
                    if purpose == Purpose::Bed {
                        ex_ui.bed.ui(ui, session_layout);
                    }

                    if purpose == Purpose::Mixdown {
                        ui.label("Mix");
                        ui.vertical(|ui| {
                            ui.radio_value(&mut prefs.range, MixRange::Session, "Whole session");
                            ui.add_enabled_ui(has_sel, |ui| {
                                ui.radio_value(&mut prefs.range, MixRange::Selection, "Time selection")
                                    .on_disabled_hover_text("Make a time selection first");
                            });
                            ui.add_enabled_ui(has_clip, |ui| {
                                ui.radio_value(&mut prefs.range, MixRange::SelectedClips, "Selected clips")
                                    .on_disabled_hover_text("Select a clip first");
                            });
                        });
                        ui.end_row();
                    }

                    let container = prefs.container;
                    let f = prefs.fmt_mut(container);
                    ui.label("Sample rate");
                    egui::ComboBox::from_id_salt("export-rate")
                        .selected_text(rate_label(f.rate))
                        .show_ui(ui, |ui| {
                            for r in ex::RATES {
                                ui.selectable_value(&mut f.rate, r, rate_label(r));
                            }
                        });
                    ui.end_row();
                    ui.label("Channels");
                    egui::ComboBox::from_id_salt("export-ch")
                        .selected_text(f.channels.label())
                        .show_ui(ui, |ui| {
                            for c in ChannelChoice::ALL {
                                ui.selectable_value(&mut f.channels, c, c.label());
                            }
                        });
                    ui.end_row();
                    if !container.formats().contains(&f.format) {
                        f.format = SampleFormat::I16;
                    }
                    ui.label("Bit depth");
                    egui::ComboBox::from_id_salt("export-bits")
                        .selected_text(f.format.label())
                        .show_ui(ui, |ui| {
                            for b in container.formats() {
                                ui.selectable_value(&mut f.format, *b, b.label());
                            }
                        });
                    ui.end_row();
                    ui.label("Dither");
                    ui.add_enabled(f.format.is_int(), egui::Checkbox::new(&mut f.dither, "Triangular (TPDF)"))
                        .on_disabled_hover_text("Float files need no dither");
                    ui.end_row();
                    if container == Container::Flac {
                        ui.label("Compression");
                        ui.add(crate::widgets::Slider::new(&mut f.flac_level, 0..=8))
                            .on_hover_text("0 is fastest, 8 smallest; every level is lossless");
                        ui.end_row();
                    }

                    ui.label("Loudness");
                    ui.vertical(|ui| {
                        let label = |l: Loudness| match l {
                            Loudness::Off => "Off".to_string(),
                            Loudness::Preset(i) => ex::LOUDNESS_PRESETS
                                .get(i)
                                .map_or("?".into(), |p| p.0.to_string()),
                            Loudness::Custom => "Custom".to_string(),
                        };
                        egui::ComboBox::from_id_salt("export-loud")
                            .selected_text(label(prefs.loudness))
                            .width(240.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut prefs.loudness, Loudness::Off, "Off");
                                for i in 0..ex::LOUDNESS_PRESETS.len() {
                                    let l = Loudness::Preset(i);
                                    ui.selectable_value(&mut prefs.loudness, l, label(l));
                                }
                                ui.selectable_value(&mut prefs.loudness, Loudness::Custom, "Custom");
                            });
                        if prefs.loudness == Loudness::Custom {
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::DragValue::new(&mut prefs.custom.lufs)
                                        .range(-40.0..=-5.0)
                                        .speed(0.1)
                                        .suffix(" LUFS"),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut prefs.custom.true_peak_db)
                                        .range(-9.0..=0.0)
                                        .speed(0.1)
                                        .suffix(" dBTP"),
                                );
                            });
                        }
                        if prefs.loudness != Loudness::Off {
                            ui.small("Gain only: the level moves to the target unless the true peak would pass the ceiling.");
                        }
                    });
                    ui.end_row();
                });

            // Summary.
            let opts = prefs.options();
            let out_rate = opts.rate.unwrap_or(src_rate);
            let out_ch = opts.channels.count(src_ch);
            let frames = (secs.max(0.0) * out_rate as f64).round() as u64;
            let bytes = ex::estimate_bytes(&opts, frames, src_ch) * files as u64;
            let ch_name = match out_ch {
                1 => "mono".to_string(),
                2 => "stereo".to_string(),
                n => format!("{n} channels"),
            };
            ui.separator();
            ui.label(format!(
                "{} Hz, {ch_name}, {} {}, {}{}; about {}",
                out_rate,
                opts.effective_format().label(),
                opts.container.label(),
                af2_core::time::format_time(secs.max(0.0)),
                if files > 1 {
                    format!(", {files} files")
                } else {
                    String::new()
                },
                ex::human_bytes(bytes)
            ));
            if purpose == Purpose::Bed {
                if let Some(why) = ex_ui.bed.refuse(session_layout, opts.container) {
                    ui.colored_label(egui::Color32::from_rgb(230, 120, 90), why);
                }
            }
            if opts.container == Container::Flac && out_ch > ex::flac::MAX_CHANNELS {
                ui.colored_label(egui::Color32::from_rgb(230, 120, 90), "FLAC holds at most 8 channels: choose WAV.");
            }

            ui.add_space(4.0);
            if let Some(list) = &confirm {
                let what = if list.len() == 1 {
                    format!("{} already exists. Replace it?", prefs_name(&list[0]))
                } else {
                    format!("{} of these files already exist. Replace them?", list.len())
                };
                ui.label(RichText::new(what).strong());
                ui.horizontal(|ui| {
                    replace = ui.button("Replace").clicked();
                    if ui.button("Cancel").clicked() {
                        ex_ui.confirm = None;
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    let label = if matches!(purpose, Purpose::SaveDoc(_)) { "Save" } else { "Export" };
                    go = ui.button(RichText::new(label).strong()).clicked();
                    close = ui.button("Cancel").clicked();
                });
            }
        });
    st.export.prefs = Some(prefs.clone());
    if !open || close {
        st.export.open = false;
        st.export.confirm = None;
        return;
    }
    if go || replace {
        let paths = targets(st, purpose, prefs.container);
        if go {
            let existing: Vec<PathBuf> = paths.iter().filter(|p| p.exists()).cloned().collect();
            if !existing.is_empty() {
                st.export.confirm = Some(existing);
                return;
            }
        }
        if let Some(p) = st.export.prefs.as_mut() {
            p.folder = Some(st.export.folder.clone());
        }
        st.export.open = false;
        st.export.confirm = None;
        start(st, paths);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_take_unmuted_tracks_with_clips_and_ignore_solo() {
        let mut s = Session::default();
        let m = s.alloc_id();
        s.media.push(af2_core::MediaItem {
            id: m,
            name: "a".into(),
            path: None,
            duration: 1.0,
            rate: 48_000,
            channels: 2,
            ext_id: None,
        });
        let ids: Vec<_> = s.tracks.iter().map(|t| t.id).collect();
        s.add_clip(ids[0], m, 0.0);
        s.add_clip(ids[1], m, 1.0);
        s.add_clip(ids[2], m, 2.0);
        s.tracks[1].mute = true;
        s.tracks[2].solo = true;
        assert_eq!(stem_tracks(&s), vec![0, 2]);
        let only = only_track(&s, 0);
        assert!(!only.tracks[0].mute);
        assert!(only.tracks[2].mute);
        assert!(only.tracks.iter().all(|t| !t.solo));
        assert!(only.audible(&only.tracks[0]));
        assert!(!only.audible(&only.tracks[2]));
    }

    #[test]
    fn prefs_round_trip_per_format() {
        let mut p = ExportPrefs {
            container: Container::Flac,
            ..Default::default()
        };
        p.flac.flac_level = 8;
        p.wav.format = SampleFormat::F32;
        p.loudness = Loudness::Preset(2);
        let back: ExportPrefs = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
        let o = back.options();
        assert_eq!(o.container, Container::Flac);
        assert_eq!(o.flac_level, 8);
        assert_eq!(o.loudness.unwrap().lufs, -16.0);
        // Old or partial files still load.
        let partial: ExportPrefs = serde_json::from_str(r#"{"container":"Wav"}"#).unwrap();
        assert_eq!(partial.flac.flac_level, 8);
    }

    #[test]
    fn typed_extension_names() {
        assert_eq!(with_ext("Take 1.wav", Container::Flac), "Take 1.flac");
        assert_eq!(with_ext("Take", Container::Wav), "Take.wav");
        assert_eq!(with_ext("Song v1.2 Mixdown", Container::Wav), "Song v1.2 Mixdown.wav");
        assert_eq!(with_ext("Song v1.2 Mixdown.wav", Container::Flac), "Song v1.2 Mixdown.flac");
    }

    /// Runs a mixdown job to its end the way the app does each frame.
    fn finish_mixdown(st: &mut State, path: &Path, secs: usize) {
        let ctx = egui::Context::default();
        let p2 = path.to_path_buf();
        let read_back = st.doc_for_path(path).is_some();
        spawn(
            st,
            "Exporting".into(),
            1,
            Then::Mixdown(path.to_path_buf()),
            false,
            move |_, _| {
                let buf = AudioBuffer::new(48_000, 2, 48_000 * secs);
                let r = ex::export_buffer(&p2, &buf, &Options::default(), &Meta::default())?;
                let reload = read_back.then(|| af2_io::open_file(&p2).unwrap().buf);
                Ok(Done { reports: vec![r], reload, note: None })
            },
            vec![],
        );
        for _ in 0..500 {
            poll(st, &ctx);
            st.poll_jobs();
            if st.export.job.is_none() && st.jobs.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn a_finished_mixdown_opens_in_the_waveform_editor_even_when_exported_again() {
        let dir = std::env::temp_dir().join(format!("af2-mixdown-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Song v1.2 Mixdown.wav");
        let mut st = State::new();
        st.mode = Mode::Multitrack;
        finish_mixdown(&mut st, &path, 1);
        assert_eq!(st.mode, Mode::Waveform);
        let d = st.doc().expect("the mixdown is the file in front");
        assert_eq!(d.path.as_deref(), Some(path.as_path()));
        assert!((d.duration() - 1.0).abs() < 1e-3);
        // Exported again over the same file (the session changed): the
        // editor shows the new file, not the one it had open.
        st.mode = Mode::Multitrack;
        finish_mixdown(&mut st, &path, 2);
        assert_eq!(st.mode, Mode::Waveform);
        let d = st.doc().unwrap();
        assert_eq!(d.path.as_deref(), Some(path.as_path()));
        assert!((d.duration() - 2.0).abs() < 1e-3, "{}", d.duration());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
