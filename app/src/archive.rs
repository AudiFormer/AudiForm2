// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Session archives (`.audarc`): the session and every piece of audio it
//! uses in one small file that opens anywhere.
//!
//! An archive is a zip holding `session.audfrm`, the audio under `Audio/`,
//! any SoundFonts and impulse responses under `Files/`, and `archive.json`
//! (what made it and how). What goes in:
//! - only the parts of each file that clips play, plus a few seconds of
//!   handle either side; parts far apart become separate files, and media
//!   nothing plays is left out;
//! - audio as FLAC, bit for bit: 16- and 24-bit sources stay exactly as
//!   they were. Float sources become 24-bit FLAC (far below hearing),
//!   unless "keep float audio exact" is on, or they go over full scale;
//!   then they're float WAV, compressed in the zip;
//! - optionally, each track's effects rendered into its audio (MIDI tracks
//!   become audio), so the session sounds right where those effects or
//!   instruments aren't installed, and each bus bounced to a muted track
//!   after it: its inputs through its effects, ready to swap in.
//!
//! Opening an archive unpacks it into a folder beside it (named after
//! it) and opens the session there; opening it again opens that folder's
//! session, with any changes made since.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use af2_core::automation::LaneTarget;
use af2_core::{AudioBuffer, MediaId, MediaItem, Session, TrackId, TrackKind};
use serde::{Deserialize, Serialize};

use crate::state::State;

pub const EXT: &str = "audarc";
const FORMAT: &str = "audiform-archive";
const SESSION_FILE: &str = "session.audfrm";
const MANIFEST: &str = "archive.json";
/// Written into the unpacked folder: which archive it came from.
const MARK: &str = ".audarc-id";
/// Stands for the unpacked folder in paths that aren't media (SoundFonts,
/// impulse responses), which are read as they are written.
const HERE: &str = "${AUDARC}";
/// Rendered effects and bounces run on this long after the last sound.
const TAIL: f64 = 6.0;
/// Parts of a file closer than this are kept as one.
const JOIN_GAP: f64 = 10.0;
/// Quieter than this at the ends of a render counts as silence (-110 dB).
const SILENCE: f32 = 3.2e-6;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// Keep only the parts of files that clips play.
    pub trim: bool,
    /// Seconds kept either side of what plays, for later edits and fades.
    pub handles: f64,
    /// Parts of clips covered by a clip on top of them (which play
    /// nothing there) are cut away, and clips covered whole left out.
    pub drop_hidden: bool,
    /// Float sources stay float (bigger) instead of 24-bit FLAC.
    pub exact_float: bool,
    /// Each track's effects (and MIDI instruments) rendered into its audio.
    pub commit: bool,
    /// With `commit`: the tracks as they were, muted, after the rendered.
    pub keep_originals: bool,
    /// Each bus rendered to a muted track after it.
    pub bounce_buses: bool,
    /// SoundFonts and impulse responses the session reads.
    pub include_files: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            trim: true,
            handles: 2.0,
            drop_hidden: true,
            exact_float: false,
            commit: false,
            keep_originals: false,
            bounce_buses: true,
            include_files: true,
        }
    }
}

#[derive(Default)]
pub struct ArchiveUi {
    pub open: bool,
    opts: Option<Options>,
}

fn prefs_path() -> PathBuf {
    crate::state::data_dir().join("archive.json")
}

impl ArchiveUi {
    fn opts(&mut self) -> &mut Options {
        self.opts.get_or_insert_with(|| {
            std::fs::read_to_string(prefs_path())
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default()
        })
    }

    fn save(&self) {
        if let Some(o) = &self.opts {
            if let Ok(t) = serde_json::to_string_pretty(o) {
                let _ = std::fs::create_dir_all(crate::state::data_dir());
                let _ = std::fs::write(prefs_path(), t);
            }
        }
    }
}

// ------------------------------------------------------------ planning

/// Each media item's played source spans (seconds into the file).
pub fn used_spans(s: &Session) -> HashMap<MediaId, Vec<(f64, f64)>> {
    let mut out: HashMap<MediaId, Vec<(f64, f64)>> = HashMap::new();
    for t in &s.tracks {
        for c in &t.clips {
            let a = c.offset.max(0.0);
            out.entry(c.media).or_default().push((a, a + c.source_len().max(0.0)));
        }
    }
    out
}

/// The spans to keep of a file `dur` seconds long: each widened by
/// `handles`, joined when closer than `gap`, inside the file. With nothing
/// worth cutting (it keeps nearly all of it), the whole file.
pub fn keep_spans(mut spans: Vec<(f64, f64)>, handles: f64, gap: f64, dur: f64) -> Vec<(f64, f64)> {
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (a, b) in spans {
        let (a, b) = ((a - handles).max(0.0), (b + handles).min(dur));
        if b <= a {
            continue;
        }
        match out.last_mut() {
            Some(l) if a - l.1 < gap => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    let kept: f64 = out.iter().map(|(a, b)| b - a).sum();
    if out.is_empty() || kept > dur * 0.95 {
        return vec![(0.0, dur)];
    }
    out
}

/// Takes `hidden` out of `whole`: what's left, in order.
fn subtract(whole: (f64, f64), mut hidden: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    hidden.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = Vec::new();
    let mut at = whole.0;
    for (a, b) in hidden {
        let (a, b) = (a.max(whole.0), b.min(whole.1));
        if b <= a {
            continue;
        }
        if a > at {
            out.push((at, a));
        }
        at = at.max(b);
    }
    if at < whole.1 {
        out.push((at, whole.1));
    }
    out
}

/// Cuts away what clips on top hide, as playback does: a later clip on a
/// track covers the clips under it except under its own fades, and a
/// lower clip still plays through fades of its own; muted clips cover
/// nothing. A clip covered whole goes. A plain clip (no stretch, reverse,
/// clip effects or clip automation, which a cut would change) covered in
/// part keeps only what's heard, plus `handles` either side (still under
/// the cover, so it sounds the same). Returns how many clips went and how
/// many were cut.
pub fn drop_hidden(s: &mut Session, handles: f64) -> (usize, usize) {
    let (mut gone, mut cut) = (0, 0);
    let mut next = s.next_id;
    for t in &mut s.tracks {
        let clips = std::mem::take(&mut t.clips);
        let mut out = Vec::with_capacity(clips.len());
        for (i, c) in clips.iter().enumerate() {
            if c.muted {
                out.push(c.clone());
                continue;
            }
            let end = c.start + c.length;
            let covers: Vec<(f64, f64)> = clips[i + 1..]
                .iter()
                .filter(|u| !u.muted && u.start < end && u.start + u.length > c.start)
                .map(|u| (u.start + u.fade_in, u.start + u.length - u.fade_out))
                .filter(|(a, b)| b > a)
                .collect();
            // Its own fades always play.
            let own: Vec<(f64, f64)> = [(c.start, c.start + c.fade_in), (end - c.fade_out, end)]
                .into_iter()
                .filter(|(a, b)| b > a)
                .collect();
            let hidden: Vec<(f64, f64)> = covers.into_iter().flat_map(|h| subtract(h, own.clone())).collect();
            let heard = subtract((c.start, end), hidden.clone());
            if heard.is_empty() {
                gone += 1;
                continue;
            }
            let p = &c.props;
            let plain = p.stretch.mode == af2_core::session::StretchMode::Off
                && !p.reversed
                && p.chain.is_empty()
                && p.lanes.is_empty();
            if hidden.is_empty() || !plain {
                out.push(c.clone());
                continue;
            }
            let mut pieces: Vec<(f64, f64)> = Vec::new();
            for (a, b) in heard {
                let (a, b) = ((a - handles).max(c.start), (b + handles).min(end));
                match pieces.last_mut() {
                    Some(l) if a <= l.1 => l.1 = l.1.max(b),
                    _ => pieces.push((a, b)),
                }
            }
            if pieces.len() == 1 && pieces[0].0 <= c.start && pieces[0].1 >= end {
                out.push(c.clone());
                continue;
            }
            cut += 1;
            for (k, (a, b)) in pieces.iter().copied().enumerate() {
                let mut piece = c.clone();
                if k > 0 {
                    piece.id = next;
                    next += 1;
                    piece.ext_id = None;
                }
                piece.start = a;
                piece.offset = c.offset + (a - c.start);
                piece.length = b - a;
                piece.fade_in = if a <= c.start { c.fade_in } else { 0.0 };
                piece.fade_out = if b >= end { c.fade_out } else { 0.0 };
                out.push(piece);
            }
        }
        t.clips = out;
    }
    s.next_id = next;
    (gone, cut)
}

/// What a track's sound depends on besides its clips: the tracks keying
/// its effects' sidechains.
fn keys(t: &af2_core::session::Track) -> impl Iterator<Item = TrackId> + '_ {
    t.chain
        .iter()
        .chain(t.clips.iter().flat_map(|c| c.props.chain.iter()))
        .filter_map(|s| s.sidechain)
}

/// The session as `target` alone hears it, before its fader: what feeds
/// it (through outputs and sends that are on, as in the mix), its clips,
/// notes and effects with their automation, and the tracks that key its
/// sidechains (heard by the effects, not in the result). Everything else
/// is muted, and what plays only to feed the target goes nowhere else.
pub fn isolate(s: &Session, target: TrackId) -> Session {
    let mut out = s.clone();
    let idx: HashMap<TrackId, usize> = out.tracks.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
    let routes = |t: &af2_core::session::Track| -> Vec<TrackId> {
        t.output
            .into_iter()
            .chain(t.sends.iter().filter(|x| x.enabled).map(|x| x.bus))
            .collect()
    };
    // Feeds the target: walking back from it.
    let mut feeds: HashSet<TrackId> = HashSet::new();
    let mut stack = vec![target];
    while let Some(x) = stack.pop() {
        for t in &out.tracks {
            if t.id != target && routes(t).contains(&x) && feeds.insert(t.id) {
                stack.push(t.id);
            }
        }
    }
    // Needed: the target, what feeds it, sidechain keys and what feeds them.
    let mut needed: HashSet<TrackId> = feeds.clone();
    needed.insert(target);
    let mut stack: Vec<TrackId> = needed.iter().copied().collect();
    while let Some(x) = stack.pop() {
        let Some(&i) = idx.get(&x) else { continue };
        let mut more: Vec<TrackId> = keys(&out.tracks[i]).collect();
        if x != target && !feeds.contains(&x) {
            // A key's own inputs.
            more.extend(out.tracks.iter().filter(|t| routes(t).contains(&x)).map(|t| t.id));
        }
        for k in more {
            if needed.insert(k) {
                stack.push(k);
            }
        }
    }
    // Outputs that must go nowhere land on a muted bus of their own.
    let sink = out.alloc_id();
    for t in &mut out.tracks {
        t.solo = false;
        t.solo_safe = false;
        t.prerender = false;
        if !needed.contains(&t.id) {
            t.mute = true;
            continue;
        }
        if t.id == target {
            t.mute = false;
            t.volume_db = 0.0;
            t.pan = 0.0;
            t.output = None;
            t.sends.clear();
            t.lanes.retain(|l| matches!(l.target, LaneTarget::Effect { .. }));
            continue;
        }
        // Feeding tracks reach the target as in the mix; keys reach only
        // what they key (or the buses that make up a key).
        let to_target = feeds.contains(&t.id);
        let keep = |dest: TrackId| {
            if to_target {
                dest == target || feeds.contains(&dest)
            } else {
                needed.contains(&dest) && dest != target && !feeds.contains(&dest)
            }
        };
        if !t.output.is_some_and(keep) {
            t.output = Some(sink);
        }
        for x in &mut t.sends {
            if !keep(x.bus) {
                x.enabled = false;
            }
        }
    }
    let mut bus = out.tracks.iter().find(|t| t.id == target).cloned().expect("target");
    bus.id = sink;
    bus.name = "Archive sink".into();
    bus.kind = TrackKind::Bus;
    bus.mute = true;
    bus.clips.clear();
    bus.midi.clear();
    bus.chain.clear();
    bus.lanes.clear();
    bus.sends.clear();
    bus.output = None;
    bus.instrument_plugin = None;
    bus.soundfont = None;
    out.tracks.push(bus);
    out.master_db = 0.0;
    out.master_pan = 0.0;
    out.master_mono = false;
    out.master_invert = false;
    out.master_chain.clear();
    out.master_rack = Default::default();
    out.master_lanes.clear();
    out.space = None;
    out
}

/// Tracks whose effects or instrument shape their sound: what "render
/// effects" renders.
pub fn commit_targets(s: &Session) -> Vec<TrackId> {
    s.tracks
        .iter()
        .filter(|t| match t.kind {
            TrackKind::Bus => false,
            TrackKind::Midi => !t.midi.is_empty(),
            TrackKind::Audio => {
                !t.clips.is_empty()
                    && (t.chain.iter().any(|x| !x.bypass)
                        || t.clips.iter().any(|c| c.props.chain.iter().any(|x| !x.bypass)))
            }
        })
        .map(|t| t.id)
        .collect()
}

/// Buses something plays into.
pub fn bounce_targets(s: &Session) -> Vec<TrackId> {
    s.tracks
        .iter()
        .filter(|b| b.kind == TrackKind::Bus)
        .filter(|b| {
            s.tracks.iter().any(|t| {
                t.id != b.id
                    && (t.output == Some(b.id) || t.sends.iter().any(|x| x.enabled && x.bus == b.id))
            })
        })
        .map(|b| b.id)
        .collect()
}

/// Where a track's sound starts and ends on the timeline.
fn extent(t: &af2_core::session::Track) -> Option<(f64, f64)> {
    let spans = t
        .clips
        .iter()
        .map(|c| (c.start, c.start + c.length))
        .chain(t.midi.iter().map(|m| (m.start, m.start + m.length)));
    spans.fold(None, |acc, (a, b)| match acc {
        None => Some((a, b)),
        Some((x, y)) => Some((x.min(a), y.max(b))),
    })
}

/// The bit depth a buffer's samples were stored at, when every sample is
/// exactly an integer at 16 or 24 bits (as decoded from a PCM file).
pub fn int_depth(buf: &AudioBuffer) -> Option<u16> {
    let fits = |scale: f64| {
        buf.channels.iter().flatten().all(|&s| {
            let v = s as f64 * scale;
            v.fract() == 0.0 && v >= -scale && v < scale
        })
    };
    if fits(32768.0) {
        Some(16)
    } else if fits(8_388_608.0) {
        Some(24)
    } else {
        None
    }
}

/// How a piece of audio is stored, and under which file extension.
fn storage(buf: &AudioBuffer, exact_float: bool) -> (af2_io::export::Container, af2_io::export::SampleFormat, &'static str) {
    use af2_io::export::{Container, SampleFormat};
    let over = buf.channels.iter().flatten().any(|s| s.abs() > 1.0 || !s.is_finite());
    let flac_ok = buf.num_channels() <= 8;
    match int_depth(buf) {
        Some(16) if flac_ok => (Container::Flac, SampleFormat::I16, "flac"),
        Some(24) if flac_ok => (Container::Flac, SampleFormat::I24, "flac"),
        Some(16) => (Container::Wav, SampleFormat::I16, "wav"),
        Some(_) => (Container::Wav, SampleFormat::I24, "wav"),
        None if over || exact_float || !flac_ok => (Container::Wav, SampleFormat::F32, "wav"),
        None => (Container::Flac, SampleFormat::I24, "flac"),
    }
}

fn slice(buf: &AudioBuffer, a: f64, b: f64) -> AudioBuffer {
    let r = buf.rate as f64;
    let n = buf.frames();
    let (i, j) = (((a * r).round() as usize).min(n), ((b * r).round() as usize).min(n));
    AudioBuffer {
        rate: buf.rate,
        channels: buf.channels.iter().map(|c| c[i..j.max(i)].to_vec()).collect(),
    }
}

/// A render with its silent ends cut: the frames dropped from the start,
/// and what's left (None when it's all silence).
fn trim_silence(buf: AudioBuffer) -> Option<(usize, AudioBuffer)> {
    let n = buf.frames();
    let loud = |i: usize| buf.channels.iter().any(|c| c[i].abs() > SILENCE);
    let first = (0..n).find(|&i| loud(i))?;
    let last = (0..n).rev().find(|&i| loud(i))?;
    let channels = buf.channels.iter().map(|c| c[first..=last].to_vec()).collect();
    Some((first, AudioBuffer { rate: buf.rate, channels }))
}

fn file_name(name: &str, used: &mut HashSet<String>, ext: &str) -> String {
    let stem = Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());
    let stem: String = af2_io::export::safe_file_name(&stem);
    let stem = if stem.trim().is_empty() { "Audio".to_string() } else { stem };
    let mut n = 1;
    loop {
        let f = if n == 1 { format!("{stem}.{ext}") } else { format!("{stem} {n}.{ext}") };
        if used.insert(f.to_ascii_lowercase()) {
            return f;
        }
        n += 1;
    }
}

/// Clears every id a saved session matches its file's objects by, so a
/// copy saves as new objects instead of overwriting the original's.
fn fresh_ids(t: &mut af2_core::session::Track) {
    t.ext_id = None;
    for c in &mut t.clips {
        c.ext_id = None;
        for x in &mut c.props.chain {
            x.ext_id = None;
        }
    }
    for m in &mut t.midi {
        m.ext_id = None;
    }
    for x in &mut t.chain {
        x.ext_id = None;
    }
    if let Some(x) = &mut t.instrument_plugin {
        x.ext_id = None;
    }
    for x in &mut t.sends {
        x.ext_id = None;
    }
    for l in &mut t.lanes {
        l.ext_id = None;
    }
}

// ------------------------------------------------------------ building

enum Task {
    Commit(TrackId),
    Bounce(TrackId),
}

struct Render {
    task: Task,
    /// What feeds the track or bus, alone; its program is built on the
    /// worker that renders it (building one loads its effects and
    /// plug-ins, slow on a heavy session, so not on the UI thread).
    session: Session,
    from: f64,
    to: f64,
}

/// Everything the background work needs, gathered on the UI thread.
struct Plan {
    session: Session,
    base: Option<serde_json::Value>,
    media: HashMap<MediaId, Arc<AudioBuffer>>,
    renders: Vec<Render>,
    opts: Options,
    out: PathBuf,
    /// Bytes of the files the session used, for the size comparison.
    source_bytes: u64,
}

fn plan(st: &mut State, opts: Options, out: PathBuf) -> Result<Plan, String> {
    crate::plugins::capture_all(st);
    let session = st.session.clone();
    let base = st.session_base.clone();
    let st: &State = st;
    plan_with(session, base, &|m| st.media_buffer(m), opts, out)
}

fn plan_with(
    session: Session,
    base: Option<serde_json::Value>,
    get: &dyn Fn(MediaId) -> Option<Arc<AudioBuffer>>,
    opts: Options,
    out: PathBuf,
) -> Result<Plan, String> {
    let mut media = HashMap::new();
    let mut missing = Vec::new();
    for id in used_spans(&session).keys() {
        match get(*id) {
            Some(b) => {
                media.insert(*id, b);
            }
            None => missing.push(
                session.media(*id).map(|m| m.name.clone()).unwrap_or_else(|| format!("media {id}")),
            ),
        }
    }
    if !missing.is_empty() {
        missing.sort();
        return Err(format!(
            "These files aren't loaded or can't be found, so the archive can't include them: {}",
            missing.join(", ")
        ));
    }
    let source_bytes = session
        .media
        .iter()
        .filter(|m| media.contains_key(&m.id))
        .filter_map(|m| m.path.as_ref().and_then(|p| std::fs::metadata(p).ok()))
        .map(|m| m.len())
        .sum();
    let mut renders = Vec::new();
    if opts.commit {
        for id in commit_targets(&session) {
            let Some(t) = session.tracks.iter().find(|t| t.id == id) else { continue };
            let Some((a, b)) = extent(t) else { continue };
            renders.push(Render { task: Task::Commit(id), session: isolate(&session, id), from: a, to: b + TAIL });
        }
        if opts.bounce_buses {
            for id in bounce_targets(&session) {
                renders.push(Render {
                    task: Task::Bounce(id),
                    session: isolate(&session, id),
                    from: 0.0,
                    to: session.duration() + TAIL,
                });
            }
        }
    }
    Ok(Plan {
        session,
        base,
        media,
        renders,
        opts,
        out,
        source_bytes,
    })
}

/// Renders a track or bus to a stereo buffer; None when cancelled.
fn render(
    r: &Render,
    media: &HashMap<MediaId, Arc<AudioBuffer>>,
    rate: u32,
    cancel: &AtomicBool,
) -> Option<AudioBuffer> {
    // Tails decaying into silence: no slow subnormal arithmetic.
    af2_engine::denormal::flush_to_zero();
    let program = af2_engine::mix_program(&r.session, &|m| media.get(&m).cloned(), rate);
    let mut inter: Vec<f32> = Vec::with_capacity(((r.to - r.from).max(0.0) * rate as f64) as usize * 2);
    let ch = af2_engine::render_each(program, rate, r.from, r.to, false, |b| {
        inter.extend_from_slice(b);
        !cancel.load(Ordering::Relaxed)
    });
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let ch = ch.max(1);
    let frames = inter.len() / ch;
    let mut buf = AudioBuffer::new(rate, ch, frames);
    for (i, f) in inter.chunks_exact(ch).enumerate() {
        for (c, s) in f.iter().enumerate() {
            buf.channels[c][i] = *s;
        }
    }
    Some(buf)
}

pub struct Summary {
    pub bytes: u64,
    pub source_bytes: u64,
    pub files: usize,
    pub committed: usize,
    pub bounced: usize,
}

fn new_media(s: &mut Session, name: String, buf: &AudioBuffer) -> MediaId {
    let id = s.alloc_id();
    s.media.push(MediaItem {
        id,
        name,
        path: None,
        duration: buf.frames() as f64 / buf.rate as f64,
        rate: buf.rate,
        channels: buf.num_channels() as u16,
        ext_id: None,
    });
    id
}

fn build(p: Plan, done: &AtomicU64, cancel: &AtomicBool) -> Result<Summary, String> {
    let Plan { mut session, base, mut media, renders, opts, out, source_bytes } = p;
    let rate = session.rate;
    let original = session.clone();
    let step = |n: u64| {
        done.fetch_add(n, Ordering::Relaxed);
    };
    let cancelled = || cancel.load(Ordering::Relaxed);
    let trace = std::env::var_os("AF2_TRACE").is_some();
    let t_start = std::time::Instant::now();
    let lap = |what: &str| {
        if trace {
            eprintln!("archive: {what} at {:.1} s", t_start.elapsed().as_secs_f64());
        }
    };

    // 1. Renders, from the session as it was: several at once (each
    // track or bus is independent), then put in, in order.
    let mut committed = 0;
    let mut bounced = 0;
    let outs = parallel(renders.len(), render_workers(&renders, rate), |i| {
        if cancelled() {
            return None;
        }
        let b = render(&renders[i], &media, rate, cancel);
        step(100);
        b
    });
    for (r, buf) in renders.iter().zip(outs) {
        let buf = buf.ok_or("Cancelled.")?;
        let from = r.from;
        let task = match r.task {
            Task::Commit(t) => Task::Commit(t),
            Task::Bounce(t) => Task::Bounce(t),
        };
        let trimmed = trim_silence(buf);
        match task {
            Task::Commit(id) => {
                let Some(i) = session.tracks.iter().position(|t| t.id == id) else { continue };
                let old = original.tracks.iter().find(|t| t.id == id).cloned().expect("track");
                let name = format!("{} (rendered)", old.name);
                let clip_media = trimmed.map(|(lead, b)| {
                    let m = new_media(&mut session, name.clone(), &b);
                    media.insert(m, Arc::new(b));
                    (m, from + lead as f64 / rate as f64)
                });
                let t = &mut session.tracks[i];
                t.chain.clear();
                t.rack = Default::default();
                t.lanes.retain(|l| !matches!(l.target, LaneTarget::Effect { .. }));
                t.clips.clear();
                t.midi.clear();
                t.instrument_plugin = None;
                t.soundfont = None;
                t.instrument = Default::default();
                t.prerender = false;
                if t.kind == TrackKind::Midi {
                    t.kind = TrackKind::Audio;
                }
                if let Some((m, at)) = clip_media {
                    session.add_clip(id, m, at);
                }
                if opts.keep_originals {
                    let mut copy = old;
                    copy.id = session.alloc_id();
                    copy.name = format!("{} (original)", copy.name);
                    copy.mute = true;
                    copy.solo = false;
                    fresh_ids(&mut copy);
                    let at = session.tracks.iter().position(|t| t.id == id).map_or(session.tracks.len(), |i| i + 1);
                    session.tracks.insert(at, copy);
                }
                committed += 1;
            }
            Task::Bounce(id) => {
                let Some((lead, b)) = trimmed else { continue };
                let Some(bus) = original.tracks.iter().find(|t| t.id == id).cloned() else { continue };
                let name = format!("{} (bounce)", bus.name);
                let m = new_media(&mut session, name.clone(), &b);
                media.insert(m, Arc::new(b));
                let t = session.add_track_of(TrackKind::Audio);
                session.add_clip(t, m, from + lead as f64 / rate as f64);
                let mut tr = session.tracks.pop().expect("new track");
                tr.name = name;
                tr.mute = true;
                tr.volume_db = bus.volume_db;
                tr.pan = bus.pan;
                tr.color = bus.color.clone();
                tr.output = bus.output;
                tr.sends = bus.sends.clone();
                tr.lanes = bus
                    .lanes
                    .iter()
                    .filter(|l| matches!(l.target, LaneTarget::Volume | LaneTarget::Pan))
                    .cloned()
                    .collect();
                fresh_ids(&mut tr);
                let at = session.tracks.iter().position(|t| t.id == id).map_or(session.tracks.len(), |i| i + 1);
                session.tracks.insert(at, tr);
                bounced += 1;
            }
        }
    }

    lap("renders done");
    // 2. Media: what clips play, cut to size, each as its own file.
    if opts.trim && opts.drop_hidden {
        drop_hidden(&mut session, opts.handles.max(0.0));
    }
    let tmp = out.with_extension("audarc-parts");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("Audio")).map_err(|e| format!("Could not write next to {}: {e}", out.display()))?;
    let cleanup = scopeguard(tmp.clone());
    let spans = used_spans(&session);
    let mut names = HashSet::new();
    let mut entries: Vec<(String, PathBuf, bool)> = Vec::new();
    let mut new_items: Vec<MediaItem> = Vec::new();
    // Old media id and source span -> new media id and its start.
    let mut moves: HashMap<MediaId, Vec<(f64, f64, MediaId)>> = HashMap::new();
    let mut ids: Vec<MediaId> = spans.keys().copied().collect();
    ids.sort_unstable();
    // What each file keeps, named in order; then all encoded at once.
    struct Part {
        mid: MediaId,
        k: usize,
        a: f64,
        b: f64,
        label: String,
        file: String,
        whole: bool,
        last: bool,
    }
    let mut parts: Vec<Part> = Vec::new();
    for &mid in &ids {
        let Some(item) = session.media(mid) else { continue };
        let Some(buf) = media.get(&mid) else { continue };
        let dur = buf.frames() as f64 / buf.rate as f64;
        let keep = if opts.trim {
            keep_spans(spans[&mid].clone(), opts.handles.max(0.0), JOIN_GAP, dur)
        } else {
            vec![(0.0, dur)]
        };
        let n = keep.len();
        // FLAC or WAV is decided by the file's samples, the same for
        // every part of it.
        let (_, _, ext) = storage(buf, opts.exact_float);
        for (k, (a, b)) in keep.into_iter().enumerate() {
            let label = if n > 1 { format!("{} part {}", item.name, k + 1) } else { item.name.clone() };
            let file = file_name(&label, &mut names, ext);
            parts.push(Part { mid, k, a, b, label, file, whole: a <= 0.0 && b >= dur, last: k + 1 == n });
        }
    }
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 8);
    // Each part: (container, rate, channels, frames), or an error.
    type Wrote = Result<(af2_io::export::Container, u32, usize, usize), String>;
    let written: Vec<Option<Wrote>> = parallel(parts.len(), workers, |i| {
        if cancelled() {
            return None;
        }
        let pt = &parts[i];
        let buf = media.get(&pt.mid)?;
        let cut;
        let part: &AudioBuffer = if pt.whole {
            buf
        } else {
            cut = slice(buf, pt.a, pt.b);
            &cut
        };
        let (container, format, _) = storage(part, opts.exact_float);
        let eo = af2_io::export::Options {
            container,
            format,
            dither: false,
            // Within about 1% of the smallest, several times faster.
            flac_level: 5,
            ..Default::default()
        };
        let r = af2_io::export::export_buffer(&tmp.join("Audio").join(&pt.file), part, &eo, &Default::default())
            .map(|_| (container, part.rate, part.num_channels(), part.frames()));
        if pt.last {
            step(20);
        }
        Some(r)
    });
    if cancelled() {
        return Err("Cancelled.".into());
    }
    for (pt, w) in parts.iter().zip(written) {
        let (container, prate, pch, pframes) = w.ok_or("Cancelled.")??;
        let Some(item) = session.media(pt.mid).cloned() else { continue };
        entries.push((format!("Audio/{}", pt.file), tmp.join("Audio").join(&pt.file), container == af2_io::export::Container::Wav));
        let mut it = item;
        if pt.k > 0 {
            it.id = session.alloc_id();
            it.ext_id = None;
        }
        it.name = pt.label.clone();
        it.path = Some(PathBuf::from(format!("Audio/{}", pt.file)));
        it.rate = prate;
        it.channels = pch as u16;
        it.duration = pframes as f64 / prate.max(1) as f64;
        moves.entry(pt.mid).or_default().push((pt.a, pt.b, it.id));
        new_items.push(it);
    }
    for t in &mut session.tracks {
        for c in &mut t.clips {
            let Some(parts) = moves.get(&c.media) else { continue };
            let s = c.offset.max(0.0);
            let (a, _, id) = parts
                .iter()
                .copied()
                .find(|(a, b, _)| s >= *a - 1e-9 && s <= *b)
                .unwrap_or(parts[0]);
            c.media = id;
            c.offset -= a;
        }
    }
    session.media = new_items;
    lap("audio encoded");

    // 3. Other files the session reads.
    let mut files: HashMap<PathBuf, String> = HashMap::new();
    let mut take = |p: &Path, names: &mut HashSet<String>| -> Option<String> {
        if !p.is_file() {
            return None;
        }
        if let Some(n) = files.get(p) {
            return Some(n.clone());
        }
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("bin").to_string();
        let name = format!("Files/{}", file_name(&p.to_string_lossy(), names, &ext));
        files.insert(p.to_path_buf(), name.clone());
        Some(name)
    };
    if opts.include_files {
        for t in &mut session.tracks {
            if let Some(sf) = &mut t.soundfont {
                if let Some(n) = take(&sf.path, &mut names) {
                    sf.path = PathBuf::from(format!("{HERE}/{n}"));
                }
            }
            let slots = t
                .chain
                .iter_mut()
                .chain(t.clips.iter_mut().flat_map(|c| c.props.chain.iter_mut()));
            for slot in slots {
                let Some(data) = &slot.data else { continue };
                let Ok(mut v) = serde_json::from_str::<serde_json::Value>(data) else { continue };
                let Some(p) = v.get("path").and_then(|p| p.as_str()).map(PathBuf::from) else { continue };
                if let Some(n) = take(&p, &mut names) {
                    v["path"] = serde_json::json!(format!("{HERE}/{n}"));
                    slot.data = Some(v.to_string());
                }
            }
        }
    }

    // 4. The session, with paths inside the archive.
    let mut doc = af2_interop::audfrm::export(&mut session, base.as_ref());
    if let Some(list) = doc.get_mut("media").and_then(|m| m.as_array_mut()) {
        for m in list {
            let p = m["path"].clone();
            m["relative_path"] = p;
        }
    }
    let mut text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    text.push('\n');
    let id = archive_id();
    let manifest = serde_json::json!({
        "format": FORMAT,
        "version": 1,
        "id": id,
        "session": original.name,
        "made_by": format!("AudiForm 2 {}", env!("CARGO_PKG_VERSION")),
        "options": opts,
        "rendered_tracks": committed,
        "bus_bounces": bounced,
    });

    // 5. One file.
    let part = out.with_extension("audarc-part");
    let w = std::fs::File::create(&part).map_err(|e| format!("Could not write {}: {e}", part.display()))?;
    let mut z = zip::ZipWriter::new(std::io::BufWriter::new(w));
    let deflate = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    let stored = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);
    let zerr = |e: zip::result::ZipError| format!("Could not write the archive: {e}");
    let ioerr = |e: std::io::Error| format!("Could not write the archive: {e}");
    z.start_file(MANIFEST, deflate).map_err(zerr)?;
    z.write_all(serde_json::to_string_pretty(&manifest).unwrap_or_default().as_bytes()).map_err(ioerr)?;
    z.start_file(SESSION_FILE, deflate).map_err(zerr)?;
    z.write_all(text.as_bytes()).map_err(ioerr)?;
    let copy = |name: &str, src: &Path, squeeze: bool, z: &mut zip::ZipWriter<_>| -> Result<(), String> {
        z.start_file(name, if squeeze { deflate } else { stored }).map_err(zerr)?;
        let mut f = std::fs::File::open(src).map_err(ioerr)?;
        std::io::copy(&mut f, z).map_err(ioerr)?;
        Ok(())
    };
    for (name, src, squeeze) in &entries {
        if cancelled() {
            drop(z);
            let _ = std::fs::remove_file(&part);
            return Err("Cancelled.".into());
        }
        copy(name, src, *squeeze, &mut z)?;
    }
    for (src, name) in &files {
        copy(name, src, true, &mut z)?;
    }
    z.finish().map_err(zerr)?.flush().map_err(ioerr)?;
    std::fs::rename(&part, &out).map_err(|e| format!("Could not write {}: {e}", out.display()))?;
    lap("zip written");
    drop(cleanup);
    step(50);
    let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    Ok(Summary {
        bytes,
        source_bytes,
        files: entries.len() + files.len(),
        committed,
        bounced,
    })
}

/// Runs `f(0..n)` on up to `workers` threads, results in index order.
fn parallel<T: Send>(n: usize, workers: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out: Vec<std::sync::Mutex<Option<T>>> = (0..n).map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|sc| {
        for _ in 0..workers.clamp(1, n.max(1)) {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    break;
                }
                let v = f(i);
                *out[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
            });
        }
    });
    out.into_iter()
        .map(|m| m.into_inner().unwrap_or_else(|e| e.into_inner()).expect("every index ran"))
        .collect()
}

/// How many renders run at once: the processor's cores, up to four, and
/// fewer for long sessions so their buffers (stereo float, the length of
/// the render) stay under about 2 GB together.
fn render_workers(renders: &[Render], rate: u32) -> usize {
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    let longest = renders.iter().map(|r| (r.to - r.from).max(0.0)).fold(0.0, f64::max);
    // Interleaved while rendering, then split: about twice the stereo size.
    let bytes = longest * rate as f64 * 2.0 * 4.0 * 2.0;
    let by_memory = if bytes > 0.0 { (2.0e9 / bytes).floor() as usize } else { 4 };
    cores.min(4).min(by_memory).max(1)
}

/// Removes the working folder however the build ends.
struct Scope(PathBuf);
impl Drop for Scope {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn scopeguard(p: PathBuf) -> Scope {
    Scope(p)
}

fn archive_id() -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut h);
    std::process::id().hash(&mut h);
    let a = h.finish();
    a.hash(&mut h);
    format!("{a:016x}{:016x}", h.finish())
}

/// Starts writing an archive of the open session to `out`.
pub fn start(st: &mut State, opts: Options, out: PathBuf) {
    if st.export.job.is_some() {
        st.error("An export is still running.");
        return;
    }
    let p = match plan(st, opts, out.clone()) {
        Ok(p) => p,
        Err(e) => {
            st.error(e);
            return;
        }
    };
    let total = p.renders.len() as u64 * 100 + p.media.len() as u64 * 20 + 50;
    crate::export::spawn_archive(st, total, out, move |done, cancel| build(p, done, cancel));
}

// ------------------------------------------------------------ opening

/// Unpacks an archive beside it (or, when that folder can't be written,
/// in AudiForm 2's own folder) and gives the session file to open.
pub fn unpack(path: &Path) -> Result<PathBuf, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("{} isn't an AudiForm archive ({e}).", path.display()))?;
    let manifest: serde_json::Value = {
        let mut s = String::new();
        z.by_name(MANIFEST)
            .map_err(|_| format!("{} isn't an AudiForm archive.", path.display()))?
            .read_to_string(&mut s)
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&s).map_err(|e| e.to_string())?
    };
    if manifest["format"] != FORMAT {
        return Err(format!("{} isn't an AudiForm archive.", path.display()));
    }
    if manifest["version"].as_u64().unwrap_or(0) > 1 {
        return Err("This archive was made by a newer AudiForm 2: update to open it.".into());
    }
    let id = manifest["id"].as_str().unwrap_or("").to_string();
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Archive".into());
    let homes = [
        path.parent().map(Path::to_path_buf).unwrap_or_default(),
        crate::state::data_dir().join("archives"),
    ];
    for home in homes {
        for n in 1..100 {
            let dir = home.join(if n == 1 { stem.clone() } else { format!("{stem} ({n})") });
            let mark = dir.join(MARK);
            if dir.exists() {
                // Unpacked before: open that (with any changes since).
                if std::fs::read_to_string(&mark).is_ok_and(|m| m.trim() == id) && dir.join(SESSION_FILE).is_file() {
                    return Ok(dir.join(SESSION_FILE));
                }
                continue;
            }
            if std::fs::create_dir_all(&dir).is_err() {
                break;
            }
            extract(&mut z, &dir)?;
            let _ = std::fs::write(&mark, &id);
            return Ok(dir.join(SESSION_FILE));
        }
    }
    Err(format!("Could not find a folder to unpack {} into.", path.display()))
}

fn extract(z: &mut zip::ZipArchive<std::fs::File>, dir: &Path) -> Result<(), String> {
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| e.to_string())?;
        // Never outside the folder.
        let Some(rel) = e.enclosed_name() else { continue };
        let to = dir.join(rel);
        if e.is_dir() {
            let _ = std::fs::create_dir_all(&to);
            continue;
        }
        if let Some(p) = to.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        if e.name() == SESSION_FILE {
            let mut s = String::new();
            e.read_to_string(&mut s).map_err(|e| e.to_string())?;
            // Paths that aren't media name the folder itself.
            let here = serde_json::to_string(&dir.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            let s = s.replace(HERE, here.trim_matches('"'));
            std::fs::write(&to, s).map_err(|e| e.to_string())?;
        } else {
            let mut f = std::fs::File::create(&to).map_err(|e| e.to_string())?;
            std::io::copy(&mut e, &mut f).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// ------------------------------------------------------------ window

pub fn show(ctx: &egui::Context, st: &mut State) {
    if !st.archive.open {
        return;
    }
    let mut open = true;
    let mut go = false;
    let has_buses = st.session.tracks.iter().any(|t| t.kind == TrackKind::Bus);
    let renders = commit_targets(&st.session).len();
    let mut o = st.archive.opts().clone();
    crate::ui::popup(ctx, egui::Id::new("af2-archive"), "Archive Session")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.label("Puts the session and all the audio it uses into one small file that opens anywhere.");
            ui.add_space(6.0);
            ui.checkbox(&mut o.trim, "Leave out audio no clip plays")
                .on_hover_text("Only the parts of each file that clips play are kept, plus the handles below. Files nothing plays are left out.");
            ui.add_enabled_ui(o.trim, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    ui.label("Keep");
                    ui.add(egui::DragValue::new(&mut o.handles).range(0.0..=30.0).speed(0.1).suffix(" s"));
                    ui.label("either side, for later edits and fades");
                });
            });
            ui.add_enabled_ui(o.trim, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    ui.checkbox(&mut o.drop_hidden, "Leave out parts of clips hidden under other clips")
                        .on_hover_text("Where a clip lies on top of another, the one under it plays nothing: that part is cut away (keeping the handles above), and clips covered whole are left out.");
                });
            });
            ui.checkbox(&mut o.exact_float, "Keep 32-bit float audio exact (bigger)")
                .on_hover_text("Audio stays bit for bit as FLAC when it came from 16- or 24-bit files. Float audio is stored as 24-bit FLAC unless this is on; audio that goes over full scale always stays float.");
            ui.checkbox(&mut o.include_files, "Include SoundFonts and impulse responses");
            ui.separator();
            ui.checkbox(&mut o.commit, format!("Render track effects into the audio ({renders} tracks)"))
                .on_hover_text("Each track's effects, clip effects and MIDI instrument are rendered into its audio and taken off, so the session sounds the same where they aren't installed. Faders, pans, sends and buses stay live.");
            ui.add_enabled_ui(o.commit, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    ui.checkbox(&mut o.keep_originals, "Keep the original tracks too, muted");
                });
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    ui.add_enabled(has_buses, egui::Checkbox::new(&mut o.bounce_buses, "Bounce each bus to a muted track after it"))
                        .on_hover_text("What plays into each bus, through its effects, as a muted track with the bus's fader, pan and routing. Unmute it and mute the bus where the bus's effects aren't installed.");
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                go = ui.button(egui::RichText::new("Archive...").strong()).clicked();
                if ui.button("Cancel").clicked() {
                    st.archive.open = false;
                }
            });
        });
    if *st.archive.opts() != o {
        *st.archive.opts() = o.clone();
        st.archive.save();
    }
    if !open {
        st.archive.open = false;
    }
    if go {
        let name = st
            .session_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| st.session.name.clone());
        let mut d = crate::file_dialog::FileDialog::new()
            .add_filter("AudiForm archive", &[EXT])
            .set_file_name(format!("{name}.{EXT}"));
        if let Some(dir) = st.session_path.as_ref().and_then(|p| p.parent()) {
            d = d.set_directory(dir);
        }
        if let Some(mut out) = d.save_file() {
            if out.extension().is_none() {
                out.set_extension(EXT);
            }
            st.archive.open = false;
            start(st, o, out);
        }
    }
}

#[cfg(test)]
mod tests {
    /// Times archiving a real session: `AF2_ARCHIVE_SESSION=<file>
    /// AF2_TRACE=1 cargo test --release -p audiform2 -- --ignored
    /// archive_a_real_session --nocapture` (the archive goes to the temp
    /// folder; the session is only read).
    #[test]
    #[ignore]
    fn archive_a_real_session() {
        use std::time::Instant;
        crate::fx::register();
        let path = PathBuf::from(std::env::var("AF2_ARCHIVE_SESSION").expect("AF2_ARCHIVE_SESSION"));
        let t = Instant::now();
        let im = af2_interop::import_with(&path, af2_interop::sesx::Surrounds::Behind).expect("import");
        let s = im.session;
        eprintln!("imported in {:.1} s: {} tracks, {} clips, {} media, {} Hz, {:.0} s long",
            t.elapsed().as_secs_f64(), s.tracks.len(), s.tracks.iter().map(|t| t.clips.len()).sum::<usize>(),
            s.media.len(), s.rate, s.duration());
        let t = Instant::now();
        let used: Vec<MediaId> = used_spans(&s).keys().copied().collect();
        let bufs: HashMap<MediaId, Arc<AudioBuffer>> = parallel(used.len(), 8, |i| {
            let m = s.media(used[i])?;
            let b = af2_io::decode_file(m.path.as_ref()?).ok()?;
            Some((used[i], Arc::new(b)))
        })
        .into_iter()
        .flatten()
        .collect();
        let bytes: usize = bufs.values().map(|b| b.frames() * b.num_channels() * 4).sum();
        eprintln!("decoded {} of {} used files in {:.1} s ({:.0} MB in memory)", bufs.len(), used.len(), t.elapsed().as_secs_f64(), bytes as f64 / 1e6);
        let out = std::env::temp_dir().join("af2-real-archive.audarc");
        let opts = Options::default();
        let t = Instant::now();
        let p = plan_with(s, None, &|m| bufs.get(&m).cloned(), opts, out.clone()).expect("plan");
        eprintln!("planned in {:.1} s: {} renders", t.elapsed().as_secs_f64(), p.renders.len());
        let t = Instant::now();
        let r = build(p, &AtomicU64::new(0), &AtomicBool::new(false)).expect("build");
        eprintln!("built in {:.1} s: {:.0} MB from {:.0} MB, {} files, {} bounced",
            t.elapsed().as_secs_f64(), r.bytes as f64 / 1e6, r.source_bytes as f64 / 1e6, r.files, r.bounced);
        let _ = std::fs::remove_file(&out);
    }

    use super::*;
    use af2_core::session::Send;

    #[test]
    fn spans_get_handles_join_when_close_and_keep_whole_files_whole() {
        // Two takes far apart in a 100 s file: two parts with handles.
        assert_eq!(keep_spans(vec![(10.0, 20.0), (60.0, 65.0)], 2.0, 10.0, 100.0), vec![(8.0, 22.0), (58.0, 67.0)]);
        // Close together: one.
        assert_eq!(keep_spans(vec![(10.0, 20.0), (25.0, 30.0)], 1.0, 10.0, 100.0), vec![(9.0, 31.0)]);
        // Nearly all of it: the whole file.
        assert_eq!(keep_spans(vec![(0.5, 99.0)], 1.0, 10.0, 100.0), vec![(0.0, 100.0)]);
        // Handles stop at the file's ends.
        assert_eq!(keep_spans(vec![(0.0, 5.0)], 2.0, 10.0, 100.0), vec![(0.0, 7.0)]);
    }

    #[test]
    fn pcm_audio_is_recognised_so_it_stays_bit_exact() {
        let mut b = AudioBuffer::new(48_000, 1, 3);
        b.channels[0] = vec![0.5, -1.0, 12345.0 / 32768.0];
        assert_eq!(int_depth(&b), Some(16));
        b.channels[0][2] = 1234567.0 / 8_388_608.0;
        assert_eq!(int_depth(&b), Some(24));
        b.channels[0][2] = 0.1;
        assert_eq!(int_depth(&b), None);
        assert_eq!(storage(&b, false).2, "flac");
        assert_eq!(storage(&b, true).2, "wav");
        b.channels[0][2] = 1.5;
        assert_eq!(storage(&b, false).2, "wav", "overs stay float");
    }

    fn fx(e: &str) -> af2_core::session::EffectSlot {
        af2_core::session::EffectSlot {
            effect: e.into(),
            values: Vec::new(),
            bypass: false,
            position: None,
            sidechain: None,
            data: None,
            ext_id: None,
        }
    }

    fn session() -> (Session, TrackId, TrackId, TrackId, TrackId) {
        let mut s = Session::default();
        let a = s.add_track_of(TrackKind::Audio);
        let b = s.add_track_of(TrackKind::Audio);
        let other = s.add_track_of(TrackKind::Audio);
        let bus = s.add_track_of(TrackKind::Bus);
        let verb = s.add_track_of(TrackKind::Bus);
        // a and b play into the bus; a also sends to the reverb.
        s.track_mut(a).unwrap().output = Some(bus);
        s.track_mut(a).unwrap().sends.push(Send::new(verb));
        s.track_mut(b).unwrap().sends.push(Send::new(bus));
        s.track_mut(bus).unwrap().volume_db = -6.0;
        (s, a, b, other, bus)
    }

    #[test]
    fn a_bus_alone_hears_only_what_feeds_it() {
        let (s, a, b, other, bus) = session();
        let iso = isolate(&s, bus);
        let t = |id| iso.tracks.iter().find(|t| t.id == id).unwrap();
        assert!(t(other).mute, "unrelated tracks are muted");
        assert!(!t(a).mute && !t(b).mute);
        assert_eq!(t(a).output, Some(bus));
        assert!(!t(a).sends[0].enabled, "the reverb send is off");
        // b's own output goes nowhere; its send to the bus stays.
        let sink = t(b).output.expect("sink");
        assert!(t(sink).mute && t(sink).kind == TrackKind::Bus);
        assert!(t(b).sends[0].enabled);
        // The bus itself, before its fader, straight out.
        assert_eq!(t(bus).volume_db, 0.0);
        assert_eq!(t(bus).output, None);
        let verb = s.tracks.iter().find(|t| t.name.starts_with("Bus 2")).unwrap().id;
        assert!(t(verb).mute);
    }

    #[test]
    fn what_gets_rendered_and_bounced() {
        let (mut s, a, b, _, bus) = session();
        assert!(commit_targets(&s).is_empty(), "no clips, no effects: nothing to render");
        let m = {
            let id = s.alloc_id();
            s.media.push(MediaItem { id, name: "x".into(), path: None, duration: 10.0, rate: 48_000, channels: 2, ext_id: None });
            id
        };
        s.add_clip(a, m, 0.0);
        s.add_clip(b, m, 0.0);
        s.track_mut(a).unwrap().chain.push(fx("amplify"));
        assert_eq!(commit_targets(&s), vec![a]);
        let buses = bounce_targets(&s);
        assert_eq!(buses.len(), 2);
        assert!(buses.contains(&bus));
    }

    fn pcm16(secs: f64, freq: f64, rate: u32) -> AudioBuffer {
        let n = (secs * rate as f64) as usize;
        let mut b = AudioBuffer::new(rate, 2, n);
        for c in 0..2 {
            for i in 0..n {
                let t = i as f64 / rate as f64;
                let v = 0.4 * (t * freq * std::f64::consts::TAU + c as f64).sin() + 0.05 * ((i * 7919 + c * 13) % 97) as f64 / 97.0;
                b.channels[c][i] = ((v * 32768.0).round() / 32768.0) as f32;
            }
        }
        b
    }

    fn rms_db(a: &AudioBuffer) -> f64 {
        let n: usize = a.channels.iter().map(Vec::len).sum();
        let e: f64 = a.channels.iter().flatten().map(|s| (*s as f64).powi(2)).sum();
        10.0 * (e / n.max(1) as f64).max(1e-30).log10()
    }

    #[test]
    fn clips_under_others_keep_only_what_is_heard_and_sound_the_same() {
        let rate = 48_000;
        let mut s = Session::new("Cover test", rate, 0);
        let m = s.alloc_id();
        s.media.push(MediaItem { id: m, name: "take".into(), path: None, duration: 30.0, rate, channels: 2, ext_id: None });
        let buf = Arc::new(pcm16(30.0, 220.0, rate));
        let t = s.add_track_of(TrackKind::Audio);
        let set = |s: &mut Session, c, start: f64, off: f64, len: f64, fi: f64, fo: f64| {
            let (ti, ci) = s.find_clip(c).unwrap();
            let k = &mut s.tracks[ti].clips[ci];
            k.start = start;
            k.offset = off;
            k.length = len;
            k.fade_in = fi;
            k.fade_out = fo;
        };
        // A long take; a short clip buried under the punch-in; a
        // punch-in over the take's middle; a crossfaded tail over its end.
        let base = s.add_clip(t, m, 0.0).unwrap();
        set(&mut s, base, 0.0, 0.0, 20.0, 0.0, 0.5);
        let buried = s.add_clip(t, m, 0.0).unwrap();
        set(&mut s, buried, 8.0, 12.0, 2.0, 0.0, 0.0);
        let punch = s.add_clip(t, m, 0.0).unwrap();
        set(&mut s, punch, 5.0, 25.0, 8.0, 0.2, 0.2);
        let tail = s.add_clip(t, m, 0.0).unwrap();
        set(&mut s, tail, 19.5, 2.0, 5.0, 0.5, 0.0);
        let before = s.clone();
        let (gone, cut) = drop_hidden(&mut s, 0.5);
        assert_eq!((gone, cut), (1, 1));
        let clips = &s.tracks[0].clips;
        assert_eq!(clips.len(), 4, "the take in two pieces, the punch-in, the tail");
        let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // The take's heard parts: up to the punch-in's fade end (5.2) and
        // from its fade start (12.8), each with half a second more.
        assert!(near(clips[0].start, 0.0) && near(clips[0].start + clips[0].length, 5.7));
        assert!(near(clips[1].start, 12.3) && near(clips[1].offset, 12.3));
        assert!(near(clips[1].start + clips[1].length, 20.0));
        assert!(near(clips[1].fade_out, 0.5) && near(clips[0].fade_out, 0.0));
        let bufs: HashMap<MediaId, Arc<AudioBuffer>> = [(m, buf)].into_iter().collect();
        let mix = |s: &Session| {
            af2_engine::render(af2_engine::mix_program(s, &|id| bufs.get(&id).cloned(), rate), rate, 0.0, 25.0)
        };
        let (a, b) = (mix(&before), mix(&s));
        let mut diff = a.clone();
        for c in 0..2 {
            for (d, x) in diff.channels[c].iter_mut().zip(&b.channels[c]) {
                *d -= x;
            }
        }
        let (level, off) = (rms_db(&a), rms_db(&diff));
        eprintln!("cover: mix {level:.1} dB, difference {off:.1} dB");
        assert!(off < level - 100.0, "mix {level:.1} dB, difference {off:.1} dB");
    }

    #[test]
    fn a_session_archives_small_and_reopens_the_same() {
        let rate = 48_000;
        let dir = std::env::temp_dir().join(format!("af2-archive-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::new("Archive test", rate, 0);
        let add = |s: &mut Session, name: &str, freq: f64| {
            let path = dir.join(format!("{name}.wav"));
            af2_io::write_wav(&path, &pcm16(30.0, freq, rate), af2_io::WavFormat::Pcm16).unwrap();
            let id = s.alloc_id();
            s.media.push(MediaItem { id, name: name.into(), path: Some(path), duration: 30.0, rate, channels: 2, ext_id: None });
            id
        };
        let m1 = add(&mut s, "takes", 220.0);
        let m2 = add(&mut s, "keys", 330.0);
        let _unused = add(&mut s, "unused", 440.0);
        let a = s.add_track_of(TrackKind::Audio);
        let b = s.add_track_of(TrackKind::Audio);
        let bus = s.add_track_of(TrackKind::Bus);
        // Two takes far apart in one file; a clip of another through a
        // low-pass into the bus; the bus has an echo.
        let c1 = s.add_clip(a, m1, 0.0).unwrap();
        let c2 = s.add_clip(a, m1, 5.0).unwrap();
        for (c, off) in [(c1, 1.0), (c2, 20.0)] {
            let (ti, ci) = s.find_clip(c).unwrap();
            let clip = &mut s.tracks[ti].clips[ci];
            clip.offset = off;
            clip.length = 3.0;
        }
        let cb = s.add_clip(b, m2, 0.0).unwrap();
        let (ti, ci) = s.find_clip(cb).unwrap();
        s.tracks[ti].clips[ci].length = 6.0;
        let slot = fx;
        s.track_mut(b).unwrap().chain.push(slot("low-pass"));
        s.track_mut(b).unwrap().output = Some(bus);
        s.track_mut(bus).unwrap().chain.push(slot("echo"));
        s.track_mut(a).unwrap().sends.push(Send::new(bus));
        let file = dir.join("Archive test.audfrm");
        af2_interop::audfrm::save(&mut s, None, &file).unwrap();
        let im = af2_interop::import(&file).unwrap();
        let s = im.session;
        let bufs: HashMap<MediaId, Arc<AudioBuffer>> = s
            .media
            .iter()
            .map(|m| (m.id, Arc::new(af2_io::decode_file(m.path.as_ref().unwrap()).unwrap())))
            .collect();
        let get = |m: MediaId| bufs.get(&m).cloned();
        let out = dir.join("Archive test.audarc");
        let opts = Options { handles: 1.0, commit: true, bounce_buses: true, ..Default::default() };
        let p = plan_with(s.clone(), im.base, &get, opts, out.clone()).unwrap();
        let sum = build(p, &AtomicU64::new(0), &AtomicBool::new(false)).unwrap();
        assert_eq!((sum.committed, sum.bounced), (1, 1));
        let source: u64 = ["takes", "keys", "unused"].iter().map(|n| std::fs::metadata(dir.join(format!("{n}.wav"))).unwrap().len()).sum();
        assert!(sum.bytes * 3 < source, "{} bytes from {source}", sum.bytes);

        // Reopened from the archive.
        let session_file = unpack(&out).unwrap();
        assert_eq!(unpack(&out).unwrap(), session_file, "opening again reuses the folder");
        let s2 = af2_interop::import(&session_file).unwrap().session;
        assert_eq!(s2.media.len(), 4, "two parts of the takes, the render, the bounce: {:?}", s2.media.iter().map(|m| &m.name).collect::<Vec<_>>());
        for m in &s2.media {
            assert!(m.path.as_ref().unwrap().is_file(), "{:?}", m.path);
        }
        let bufs2: HashMap<MediaId, Arc<AudioBuffer>> = s2
            .media
            .iter()
            .map(|m| (m.id, Arc::new(af2_io::decode_file(m.path.as_ref().unwrap()).unwrap())))
            .collect();
        // The plain track's clips play exactly the same samples.
        let ta = s.tracks.iter().position(|t| t.id == a).unwrap();
        let t2 = &s2.tracks[ta];
        for (old, new) in s.tracks[ta].clips.iter().zip(&t2.clips) {
            let ob = &bufs[&old.media];
            let nb = &bufs2[&new.media];
            let (o0, n0) = ((old.offset * rate as f64).round() as usize, (new.offset * rate as f64).round() as usize);
            let len = (old.length * rate as f64) as usize;
            assert!(nb.frames() < ob.frames(), "trimmed");
            for c in 0..2 {
                assert_eq!(&ob.channels[c][o0..o0 + len], &nb.channels[c][n0..n0 + len], "bit for bit");
            }
        }
        // The effect track is rendered and its effect gone; the bus stays,
        // with a muted bounce after it.
        let tb2 = s2.tracks.iter().find(|t| t.name == s.tracks[ta + 1].name).unwrap();
        assert!(tb2.chain.is_empty() && tb2.clips.len() == 1);
        let bi = s2.tracks.iter().position(|t| t.kind == TrackKind::Bus).unwrap();
        assert_eq!(s2.tracks[bi].chain.len(), 1);
        assert!(s2.tracks[bi + 1].name.ends_with("(bounce)") && s2.tracks[bi + 1].mute);

        // And the mix sounds the same.
        let end = s.duration() + 2.0;
        let mix = |s: &Session, b: &HashMap<MediaId, Arc<AudioBuffer>>| {
            af2_engine::render(af2_engine::mix_program(s, &|m| b.get(&m).cloned(), rate), rate, 0.0, end)
        };
        let (m1, m2) = (mix(&s, &bufs), mix(&s2, &bufs2));
        let mut diff = m1.clone();
        for c in 0..2 {
            for (d, x) in diff.channels[c].iter_mut().zip(&m2.channels[c]) {
                *d -= x;
            }
        }
        let (level, off) = (rms_db(&m1), rms_db(&diff));
        eprintln!("archive {} bytes from {source}; mix {level:.1} dB, difference {off:.1} dB", sum.bytes);
        assert!(off < level - 60.0, "mix {level:.1} dB, difference {off:.1} dB");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
