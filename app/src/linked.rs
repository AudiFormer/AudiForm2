// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Session clips: another session's whole mix as one clip.
//!
//! For overdubbing over a heavy session without carrying its weight:
//! Multitrack > Insert Session as Clip (or a session file dropped on the
//! lanes) renders that session's mix, every plug-in included, to a float
//! WAV in a "Linked Sessions" folder beside this session, and places it
//! on the track at the playhead (a new track named after it when no track
//! is picked), in one undo step. It plays as one clip, however many
//! tracks and plug-ins are inside, and it is an ordinary clip: split,
//! trim, stretch, fade, copy and paste it as any other.
//!
//! The media remembers the session file and its stamp (size and modified
//! time, Audiform's `linked` on a media reference). When this session
//! comes to the front, or the window comes back into focus, a session
//! saved since is rendered again and swapped in as one undo step; every
//! piece of a split follows the new mix, pieces past a shorter mix's end
//! are trimmed or go, and a clip that ran to the old end runs to the new.
//! Right-click: Open Session, or Render Session Again (from the open
//! session as it is, unsaved edits included, when it's open). Double-click
//! opens the session.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use af2_core::session::LinkedSession;
use af2_core::{AudioBuffer, MediaId, Session, TrackId, TrackKind};

use crate::state::State;

const FOLDER: &str = "Linked Sessions";

/// A session file's stamp: `<bytes>-<modified, unix ms>` (as Audiform's).
pub fn stamp(path: &Path) -> Option<String> {
    let m = std::fs::metadata(path).ok()?;
    let t = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(format!("{}-{}", m.len(), t.as_millis()))
}

enum Then {
    Place { track: Option<TrackId>, at: f64 },
    Update(MediaId),
}

struct Rendered {
    buf: AudioBuffer,
    file: PathBuf,
    stamp: String,
    name: String,
    notes: Vec<String>,
}

struct Job {
    session: PathBuf,
    then: Then,
    rx: Receiver<Result<Rendered, String>>,
}

#[derive(Default)]
pub struct LinkedUi {
    jobs: Vec<Job>,
    /// Session files already reported missing, so it's said once.
    told_missing: HashSet<PathBuf>,
    last_focus: bool,
    last_key: u64,
}

fn is_session_file(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(crate::sessions::is_session_ext)
}

fn same(a: &Path, b: &Path) -> bool {
    af2_interop::paths::same(a, b)
}

/// Where this session's session clips are written.
fn folder(st: &State) -> PathBuf {
    match st.session_path.as_ref().and_then(|p| p.parent()) {
        Some(d) => d.join(FOLDER),
        None => crate::state::data_dir().join("linked"),
    }
}

/// The file a session's mix goes to: named after it, the same file each
/// time it's rendered again.
fn file_for(dir: &Path, session: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let stem = session.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Session".into());
    let mut h = std::collections::hash_map::DefaultHasher::new();
    af2_interop::paths::normalize(session).hash(&mut h);
    let name = af2_io::export::safe_file_name(&stem);
    dir.join(format!("{name} ({:06x}).wav", h.finish() & 0xff_ffff))
}

/// A session loaded from its file, with the audio it plays.
fn load(path: &Path) -> Result<(Session, HashMap<MediaId, Arc<AudioBuffer>>, Vec<String>), String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let session = if ext == "af2" || ext == "af27" {
        let t = std::fs::read_to_string(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        Session::from_json(&t)?
    } else {
        af2_interop::import_with(path, af2_interop::sesx::Surrounds::Behind)?.session
    };
    let mut media = HashMap::new();
    let mut missing = Vec::new();
    let used: HashSet<MediaId> = session.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.media)).collect();
    for m in session.media.iter().filter(|m| used.contains(&m.id)) {
        match m.path.as_ref().map(|p| af2_io::decode_file(p)) {
            Some(Ok(b)) => {
                media.insert(m.id, Arc::new(b));
            }
            _ => missing.push(m.name.clone()),
        }
    }
    let notes = if missing.is_empty() {
        Vec::new()
    } else {
        vec![format!("{} files of that session could not be found and are silent in its mix: {}", missing.len(), missing.join(", "))]
    };
    Ok((session, media, notes))
}

/// Renders a session's mix (its whole length) and writes it to `file`.
fn render(
    session: &Session,
    media: &HashMap<MediaId, Arc<AudioBuffer>>,
    file: &Path,
) -> Result<AudioBuffer, String> {
    let rate = session.rate;
    let program = af2_engine::mix_program(session, &|m| media.get(&m).cloned(), rate);
    let end = session.duration().max(0.1);
    let buf = af2_engine::render(program, rate, 0.0, end);
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("Could not make {}: {e}", d.display()))?;
    }
    af2_io::write_wav(file, &buf, af2_io::WavFormat::Float32)?;
    Ok(buf)
}

fn display_name(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Session".into())
}

/// Starts rendering `session` (from its file, or from `open`: a session
/// open here with its audio, as it is) for `then`.
fn spawn(st: &mut State, session: PathBuf, then: Then, open: Option<(Session, HashMap<MediaId, Arc<AudioBuffer>>)>) {
    if st.linked.jobs.iter().any(|j| same(&j.session, &session)) {
        return;
    }
    let file = file_for(&folder(st), &session);
    let (tx, rx) = channel();
    let path = session.clone();
    std::thread::spawn(move || {
        let r = (|| {
            let stamp = stamp(&path).unwrap_or_default();
            let (s, media, notes) = match open {
                Some((s, m)) => (s, m, Vec::new()),
                None => load(&path)?,
            };
            let buf = render(&s, &media, &file)?;
            Ok(Rendered { buf, file, stamp, name: display_name(&path), notes })
        })();
        let _ = tx.send(r);
    });
    st.linked.jobs.push(Job { session, then, rx });
    st.toast(format!("Rendering {}...", display_name(&st.linked.jobs.last().unwrap().session)));
}

/// Multitrack > Insert Session as Clip, or a session dropped on a track:
/// `path`'s mix goes on `track` (a new track when None) at `at`.
pub fn insert(st: &mut State, path: &Path, track: Option<TrackId>, at: f64) {
    if !path.is_file() {
        st.error(format!("{} could not be found.", path.display()));
        return;
    }
    if st.session_path.as_deref().is_some_and(|p| same(p, path)) {
        st.error("A session can't hold its own mix. Bring the other session to the front, then put this one in it.");
        return;
    }
    let track = track.filter(|t| st.session.tracks.iter().any(|x| x.id == *t && x.kind == TrackKind::Audio));
    spawn(st, path.to_path_buf(), Then::Place { track, at }, None);
}

/// Multitrack > Insert Session as Clip...
pub fn insert_dialog(st: &mut State) {
    let exts = ["audfrm", "af2", "sesx", "altaudio", "aup3"];
    let mut d = crate::file_dialog::FileDialog::new().add_filter("Sessions", &exts);
    if let Some(dir) = st.session_path.as_ref().and_then(|p| p.parent()) {
        d = d.set_directory(dir);
    }
    if let Some(p) = d.pick_file() {
        let track = st.selected_track;
        let at = st.session_cursor;
        insert(st, &p, track, at);
    }
}

/// The session a clip's media stands in for.
pub fn session_of(st: &State, media: MediaId) -> Option<PathBuf> {
    st.session.linked.get(&media).map(|l| l.path.clone())
}

/// Opens (brings up) the session a session clip stands in for.
pub fn open(st: &mut State, media: MediaId) {
    let Some(p) = session_of(st, media) else { return };
    if !p.is_file() {
        st.error(format!("{} could not be found; the clip keeps its last mix.", p.display()));
        return;
    }
    st.open_session(&p);
}

/// Renders a session clip's session again now: as it is when it's open
/// here (unsaved edits included), else from its file.
pub fn render_again(st: &mut State, media: MediaId) {
    let Some(p) = session_of(st, media) else { return };
    let open = match st.find_open(&p) {
        Some(Some(i)) => {
            let parked = &st.sessions.parked[i];
            let s = parked.session.clone();
            let bufs: HashMap<MediaId, Arc<AudioBuffer>> = parked
                .media_docs
                .iter()
                .filter_map(|(m, d)| st.doc_by_id(*d).map(|d| (*m, d.buf.clone())))
                .collect();
            Some((s, bufs))
        }
        _ => None,
    };
    if open.is_none() && !p.is_file() {
        st.error(format!("{} could not be found; the clip keeps its last mix.", p.display()));
        return;
    }
    spawn(st, p, Then::Update(media), open);
}

/// Session clips whose session was saved since: rendered again.
pub fn check(st: &mut State) {
    let stale: Vec<(MediaId, PathBuf)> = st
        .session
        .linked
        .iter()
        .filter_map(|(m, l)| match stamp(&l.path) {
            Some(now) => (now != l.stamp).then(|| (*m, l.path.clone())),
            None => None,
        })
        .collect();
    let missing: Vec<PathBuf> = st
        .session
        .linked
        .values()
        .filter(|l| !l.path.is_file())
        .map(|l| l.path.clone())
        .filter(|p| !st.linked.told_missing.contains(p))
        .collect();
    for p in missing {
        st.toast(format!("{} could not be found; its clip keeps the last mix.", p.display()));
        st.linked.told_missing.insert(p);
    }
    for (m, p) in stale {
        spawn(st, p, Then::Update(m), None);
    }
}

/// Each frame: checks when the session in front changes or the window
/// comes back into focus, and takes in finished renders.
pub fn frame(ctx: &egui::Context, st: &mut State) {
    let focused = ctx.input(|i| i.focused);
    let key = st.sessions.key;
    if (focused && !st.linked.last_focus) || key != st.linked.last_key {
        st.linked.last_key = key;
        if !st.session.linked.is_empty() {
            check(st);
        }
    }
    st.linked.last_focus = focused;
    if st.linked.jobs.is_empty() {
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(150));
    let mut i = 0;
    while i < st.linked.jobs.len() {
        let r = match st.linked.jobs[i].rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                i += 1;
                continue;
            }
            Err(_) => Err("The render stopped unexpectedly.".into()),
        };
        let job = st.linked.jobs.remove(i);
        match r {
            Ok(done) => finish(st, job, done),
            Err(e) => st.error(format!("Could not render {}: {e}", job.session.display())),
        }
    }
}

fn finish(st: &mut State, job: Job, r: Rendered) {
    for n in &r.notes {
        st.toast(n.clone());
    }
    let link = LinkedSession { path: job.session.clone(), stamp: r.stamp.clone() };
    match job.then {
        Then::Place { track, at } => {
            let before = st.session.clone();
            let doc = st.add_doc(r.name.clone(), Some(r.file.clone()), r.buf);
            let track = match track {
                Some(t) => t,
                None => {
                    let t = st.session.add_track_of(TrackKind::Audio);
                    if let Some(tr) = st.session.track_mut(t) {
                        tr.name = r.name.clone();
                    }
                    t
                }
            };
            let Some(m) = st.media_for_doc(doc) else { return };
            st.session.linked.insert(m, link);
            if let Some(c) = st.session.add_clip(track, m, at) {
                if let Some((ti, ci)) = st.session.find_clip(c) {
                    st.session.tracks[ti].clips[ci].name = r.name.clone();
                }
                st.selected_clip = Some(c);
            }
            st.commit_session("Insert Session as Clip", before);
            st.mode = crate::state::Mode::Multitrack;
            st.toast(format!("{} is in as a session clip", r.name));
        }
        Then::Update(m) => {
            if !st.session.linked.contains_key(&m) {
                return;
            }
            let before = st.session.clone();
            let new_len = r.buf.frames() as f64 / r.buf.rate.max(1) as f64;
            let old_len = st.session.media(m).map_or(new_len, |x| x.duration);
            if let Some(d) = st.media_docs.get(&m).copied() {
                if let Some(doc) = st.docs.iter_mut().find(|x| x.id == d) {
                    doc.set_buffer("Update Linked Session", r.buf);
                    doc.dirty = false;
                }
            }
            if let Some(item) = st.session.media.iter_mut().find(|x| x.id == m) {
                item.duration = new_len;
                item.rate = st.session.rate;
            }
            follow(&mut st.session, m, old_len, new_len);
            st.session.linked.insert(m, link);
            st.commit_session("Update Linked Session", before);
            st.toast(format!("{} updated", r.name));
        }
    }
}

/// Clips of `m` after its file went from `old` to `new` seconds: pieces
/// past the new end go, pieces running over it are trimmed, and a clip
/// that ran to the old end runs to the new one.
pub fn follow(s: &mut Session, m: MediaId, old: f64, new: f64) {
    for t in &mut s.tracks {
        t.clips.retain(|c| c.media != m || c.offset < new - 1e-6);
        for c in t.clips.iter_mut().filter(|c| c.media == m) {
            let ratio = c.props.stretch.applied_ratio();
            let src_end = c.offset + c.length / ratio;
            let to_end = (src_end - old).abs() < 1e-3;
            if to_end || src_end > new {
                c.length = ((new - c.offset) * ratio).max(0.0);
            }
        }
    }
}

/// Session files among dropped paths go in as session clips at `(track,
/// at)`; what's left is returned.
pub fn take_dropped(st: &mut State, paths: Vec<PathBuf>, track: Option<TrackId>, at: f64) -> Vec<PathBuf> {
    let (sessions, rest): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|p| p.is_file() && is_session_file(p));
    for (k, p) in sessions.iter().enumerate() {
        // Several at once: each on its own new track after the first.
        insert(st, p, if k == 0 { track } else { None }, at);
    }
    rest
}

#[cfg(test)]
mod tests {
    use super::*;
    use af2_core::MediaItem;

    #[test]
    fn clips_follow_a_longer_or_shorter_mix() {
        let mut s = Session::new("t", 48_000, 0);
        let m = s.alloc_id();
        s.media.push(MediaItem { id: m, name: "mix".into(), path: None, duration: 60.0, rate: 48_000, channels: 2, ext_id: None });
        let t = s.add_track_of(TrackKind::Audio);
        let whole = s.add_clip(t, m, 0.0).unwrap();
        let early = s.add_clip(t, m, 70.0).unwrap();
        let late = s.add_clip(t, m, 90.0).unwrap();
        let set = |s: &mut Session, c, off: f64, len: f64| {
            let (ti, ci) = s.find_clip(c).unwrap();
            s.tracks[ti].clips[ci].offset = off;
            s.tracks[ti].clips[ci].length = len;
        };
        set(&mut s, early, 10.0, 20.0);
        set(&mut s, late, 50.0, 10.0);
        let mut longer = s.clone();
        follow(&mut longer, m, 60.0, 75.0);
        let len = |s: &Session, c| s.find_clip(c).map(|(ti, ci)| s.tracks[ti].clips[ci].length);
        assert_eq!(len(&longer, whole), Some(75.0), "ran to the end: runs to the new end");
        assert_eq!(len(&longer, early), Some(20.0), "a piece inside stays");
        assert_eq!(len(&longer, late), Some(25.0));
        let mut shorter = s.clone();
        follow(&mut shorter, m, 60.0, 45.0);
        assert_eq!(len(&shorter, whole), Some(45.0));
        assert_eq!(len(&shorter, early), Some(20.0));
        assert_eq!(len(&shorter, late), None, "past the new end: gone");
    }

    #[test]
    fn a_session_mix_renders_to_a_file_with_a_stamp() {
        let dir = std::env::temp_dir().join(format!("af2-linked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rate = 48_000;
        let wav = dir.join("tone.wav");
        let mut b = AudioBuffer::new(rate, 2, rate as usize * 2);
        for c in 0..2 {
            for (i, x) in b.channels[c].iter_mut().enumerate() {
                *x = 0.25 * (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin();
            }
        }
        af2_io::write_wav(&wav, &b, af2_io::WavFormat::Float32).unwrap();
        let mut inner = Session::new("Inner", rate, 0);
        let m = inner.alloc_id();
        inner.media.push(MediaItem { id: m, name: "tone".into(), path: Some(wav), duration: 2.0, rate, channels: 2, ext_id: None });
        let t = inner.add_track_of(TrackKind::Audio);
        inner.add_clip(t, m, 1.0);
        inner.track_mut(t).unwrap().volume_db = -6.0;
        let file = dir.join("Inner.audfrm");
        af2_interop::audfrm::save(&mut inner, None, &file).unwrap();
        let st1 = stamp(&file).unwrap();
        assert!(st1.contains('-'));
        let (s, media, notes) = load(&file).unwrap();
        assert!(notes.is_empty());
        let out = file_for(&dir.join(FOLDER), &file);
        let mix = render(&s, &media, &out).unwrap();
        assert!(out.is_file());
        assert!((mix.frames() as f64 / rate as f64 - 3.0).abs() < 0.01, "the whole session: 1 s in, 2 s of tone");
        // Silent before the clip, the tone at -6 dB after.
        let peak = |a: usize, b: usize| mix.channels[0][a..b].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak(0, rate as usize - 100) < 1e-6);
        let p = peak(rate as usize + 1000, rate as usize * 2);
        assert!(p > 0.1 && p < 0.2, "{p}");
        // Saved with its link and read back.
        let mut host = Session::new("Host", rate, 0);
        let hm = host.alloc_id();
        host.media.push(MediaItem { id: hm, name: "Inner".into(), path: Some(out.clone()), duration: 3.0, rate, channels: 2, ext_id: None });
        host.linked.insert(hm, LinkedSession { path: file.clone(), stamp: st1.clone() });
        let ht = host.add_track_of(TrackKind::Audio);
        host.add_clip(ht, hm, 0.0);
        let hf = dir.join("Host.audfrm");
        af2_interop::audfrm::save(&mut host, None, &hf).unwrap();
        let back = af2_interop::import(&hf).unwrap().session;
        let l = back.linked.values().next().expect("link kept");
        assert!(same(&l.path, &file) && l.stamp == st1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
