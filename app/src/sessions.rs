// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Sessions and bringing files into them.
//!
//! * Several sessions open at once. The one in front is the one the
//!   Multitrack shows; the others are set aside with their unsaved
//!   changes, undo history and view, and listed in the Files panel.
//! * New (blank or from a template), Open (an open one is brought
//!   forward instead of opened twice), Open Recent (the last ten), Save,
//!   Save As, Save All, Close, Close All, Save as Session Template,
//!   Remove Unused Files, Collect Session Media, Session Properties.
//! * Autosave writes a recovery copy of every session with unsaved
//!   changes; saving or closing deletes it. A copy left by a run that did
//!   not end cleanly is offered at the next start. Each save first copies
//!   the previous file into a Backups folder beside it, keeping the newest
//!   few.
//! * Importing several files into the Multitrack: dropped on the lanes
//!   they go one per track, all from the drop time (the first on the
//!   track under the pointer, the rest on the audio tracks below it, then
//!   on new tracks named after the files). Holding Alt puts them one after
//!   another on one track. Insert Files as Tracks puts each on its own new
//!   track at the session start, named without a leading speaker number.
//! * Session Media: files imported into a saved session are copied into
//!   its media folder (or not, or after asking), so the folder holds the
//!   whole show.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use af27_core::{ClipId, History, MediaId, Session, TrackId, TrackKind};
use serde::{Deserialize, Serialize};

use crate::state::{data_dir, DocId, Mode, State, View};

/// How many sessions Open Recent lists.
pub const RECENT_MAX: usize = 10;
/// Folders dropped or imported are searched this deep.
pub const MAX_DEPTH: usize = 8;
/// At most this many files come in from one drop.
pub const MAX_FILES: usize = 5_000;
/// A recovery lock older than this belongs to a run that has ended.
pub const LOCK_STALE_SECS: i64 = 150;

/// What happens to audio imported into a saved session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MediaPolicy {
    /// Copy it into the session's media folder (the default).
    #[default]
    Copy,
    /// Ask each time.
    Ask,
    /// Leave it where it is.
    Leave,
}

// ------------------------------------------------------------------ names

/// A track name from a file name: no extension, and no leading speaker
/// number ("1-name.flac" becomes "name", "02 - Guest.wav" becomes
/// "Guest"). Only one to three digits count as a speaker number, so a
/// name that starts with a year ("2026-11-19 call") keeps it.
pub fn strip_speaker_number(file_name: &str) -> String {
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file_name);
    let digits = stem.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=3).contains(&digits) {
        let rest = &stem[digits..];
        let name = rest.trim_start_matches([' ', '-', '_', '.']);
        if name.len() < rest.len() && !name.trim().is_empty() {
            return name.to_string();
        }
    }
    stem.to_string()
}

/// Orders names the way people count: "2 guest" before "10 guest",
/// letters without regard to case.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    let (mut x, mut y) = (a.as_bytes(), b.as_bytes());
    loop {
        match (x.first(), y.first()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let n = x.iter().take_while(|c| c.is_ascii_digit()).count();
                let m = y.iter().take_while(|c| c.is_ascii_digit()).count();
                let (dx, dy) = (&x[..n], &y[..m]);
                let tx = dx.iter().skip_while(|&&c| c == b'0').count();
                let ty = dy.iter().skip_while(|&&c| c == b'0').count();
                let o = tx
                    .cmp(&ty)
                    .then_with(|| dx[n - tx..].cmp(&dy[m - ty..]))
                    .then_with(|| n.cmp(&m));
                if o != Ordering::Equal {
                    return o;
                }
                x = &x[n..];
                y = &y[m..];
            }
            (Some(p), Some(q)) => {
                if p != q {
                    return p.cmp(q);
                }
                x = &x[1..];
                y = &y[1..];
            }
        }
    }
}

/// Two paths name the same file: without regard to case on Windows and
/// macOS, exactly elsewhere.
pub fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// `path` is inside `dir` (at any depth).
pub fn inside(path: &Path, dir: &Path) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        let p = PathBuf::from(path.to_string_lossy().to_lowercase());
        let d = PathBuf::from(dir.to_string_lossy().to_lowercase());
        p.starts_with(d)
    } else {
        path.starts_with(dir)
    }
}

// ----------------------------------------------------------------- layout

/// Where an imported file goes: an existing track (by index) or the k-th
/// new track added below the last.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Lane {
    Track(usize),
    New(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub lane: Lane,
    pub start: f64,
}

/// Where files dropped on the Multitrack land.
///
/// `lengths` are the files' lengths in seconds, in order; `audio` says
/// for each track whether it takes audio clips; `target` is the track
/// under the pointer (None: below the last track); `at` is the drop time.
///
/// Without Alt: one per track, all from `at`. The first goes on the
/// target track, the rest on the audio tracks below it in order, then on
/// new tracks. With Alt: one after another on the target track from `at`.
/// A target that cannot take audio (a bus or MIDI track), or none, means
/// new tracks.
pub fn layout_drop(
    lengths: &[f64],
    audio: &[bool],
    target: Option<usize>,
    at: f64,
    alt: bool,
) -> Vec<Placement> {
    let at = at.max(0.0);
    let first = target.filter(|&i| audio.get(i).copied().unwrap_or(false));
    if alt {
        let lane = first.map_or(Lane::New(0), Lane::Track);
        let mut t = at;
        return lengths
            .iter()
            .map(|len| {
                let p = Placement { lane, start: t };
                t += len.max(0.0);
                p
            })
            .collect();
    }
    let mut lanes = Vec::new();
    if let Some(f) = first {
        lanes.push(Lane::Track(f));
        lanes.extend((f + 1..audio.len()).filter(|&i| audio[i]).map(Lane::Track));
    }
    let mut new = 0;
    (0..lengths.len())
        .map(|i| {
            let lane = lanes.get(i).copied().unwrap_or_else(|| {
                new += 1;
                Lane::New(new - 1)
            });
            Placement { lane, start: at }
        })
        .collect()
}

/// Insert Files as Tracks: each on its own new track at the start.
pub fn layout_as_tracks(n: usize) -> Vec<Placement> {
    (0..n)
        .map(|k| Placement {
            lane: Lane::New(k),
            start: 0.0,
        })
        .collect()
}

/// A drop close to the session start lands on it: within three times the
/// usual snap distance (`edge_px` pixels at `px_per_sec`).
pub fn snap_drop_start(t: f64, px_per_sec: f64, edge_px: f64) -> f64 {
    if t * px_per_sec <= 3.0 * edge_px {
        0.0
    } else {
        t
    }
}

/// The audio files among dropped or picked paths, folders searched (up to
/// [`MAX_DEPTH`] deep, at most [`MAX_FILES`]), each folder's files in
/// natural name order, duplicates left out. Sessions are returned apart.
pub fn expand_paths(paths: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut audio: Vec<PathBuf> = Vec::new();
    let mut sessions = Vec::new();
    fn walk(p: &Path, depth: usize, audio: &mut Vec<PathBuf>, sessions: &mut Vec<PathBuf>) {
        if audio.len() >= MAX_FILES {
            return;
        }
        if p.is_dir() {
            if depth >= MAX_DEPTH {
                return;
            }
            let mut kids: Vec<PathBuf> = std::fs::read_dir(p)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .collect();
            kids.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
            for k in kids {
                walk(&k, depth + 1, audio, sessions);
            }
            return;
        }
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if is_session_ext(&ext) {
            sessions.push(p.to_path_buf());
        } else if af27_io::EXTENSIONS.contains(&ext.as_str())
            && !audio.iter().any(|a| same_path(a, p))
        {
            audio.push(p.to_path_buf());
        }
    }
    // Loose files in natural order (a file manager hands them over in the
    // order they were picked); folders keep their place after them.
    let mut loose: Vec<&PathBuf> = paths.iter().filter(|p| !p.is_dir()).collect();
    loose.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
    for p in loose {
        walk(p, 0, &mut audio, &mut sessions);
    }
    for p in paths.iter().filter(|p| p.is_dir()) {
        walk(p, 0, &mut audio, &mut sessions);
    }
    audio.truncate(MAX_FILES);
    (audio, sessions)
}

pub fn is_session_ext(ext: &str) -> bool {
    matches!(ext, "af27" | "sesx" | "audfrm" | "altaudio")
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

// ----------------------------------------------------------------- recent

/// Puts `p` at the top of the recent list, once, keeping [`RECENT_MAX`].
pub fn push_recent(list: &mut Vec<PathBuf>, p: &Path) {
    list.retain(|x| !same_path(x, p));
    list.insert(0, p.to_path_buf());
    list.truncate(RECENT_MAX);
}

fn recent_file() -> PathBuf {
    data_dir().join("recent-sessions.json")
}

pub fn load_recent() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_to_string(recent_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    v.truncate(RECENT_MAX);
    v
}

pub fn save_recent(list: &[PathBuf]) {
    let _ = std::fs::create_dir_all(data_dir());
    if let Ok(s) = serde_json::to_string_pretty(list) {
        let _ = std::fs::write(recent_file(), s);
    }
}

// ---------------------------------------------------------------- backups

/// Seconds since the epoch as "YYYY-MM-DD HH-MM-SS" (UTC), a form that
/// sorts in time order and is allowed in file names everywhere.
pub fn stamp(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil from days (Howard Hinnant's public-domain algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}-{:02}-{:02}",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The backup file name for a session called `stem` saved at `secs`.
pub fn backup_name(stem: &str, secs: i64) -> String {
    format!("{stem} {}.audfrm", stamp(secs))
}

/// The time stamp of a backup of `stem`, if `name` is one of ours.
pub fn backup_stamp<'a>(name: &'a str, stem: &str) -> Option<&'a str> {
    let s = name.strip_prefix(stem)?.strip_prefix(' ')?;
    let s = s.strip_suffix(".audfrm")?;
    let b = s.as_bytes();
    let ok = b.len() == 19
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 | 13 | 16 => *c == b'-',
            10 => *c == b' ',
            _ => c.is_ascii_digit(),
        });
    ok.then_some(s)
}

/// Which of `names` (a Backups folder) to delete to keep the newest
/// `keep` backups of `stem`. Files that are not our own backups of this
/// session are never chosen.
pub fn backups_to_prune(names: &[String], stem: &str, keep: usize) -> Vec<String> {
    let mut ours: Vec<(&str, &String)> = names
        .iter()
        .filter_map(|n| backup_stamp(n, stem).map(|s| (s, n)))
        .collect();
    ours.sort();
    let extra = ours.len().saturating_sub(keep.max(1));
    ours[..extra].iter().map(|(_, n)| (*n).clone()).collect()
}

/// Copies the session file as it is now into `Backups` beside it, then
/// prunes old backups. Nothing to do when the file does not exist yet.
pub fn back_up(file: &Path, keep: usize) -> Result<(), String> {
    if !file.is_file() {
        return Ok(());
    }
    let dir = file.parent().unwrap_or(Path::new(".")).join("Backups");
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Session".into());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::copy(file, dir.join(backup_name(&stem, now_secs()))).map_err(|e| e.to_string())?;
    let names: Vec<String> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    for n in backups_to_prune(&names, &stem, keep) {
        let _ = std::fs::remove_file(dir.join(n));
    }
    Ok(())
}

// ------------------------------------------------------------ media copies

/// "voice.wav" numbered: "voice (2).wav".
pub fn numbered(name: &str, n: usize) -> String {
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    match p.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem} ({n}).{ext}"),
        None => format!("{stem} ({n})"),
    }
}

/// The same file, as far as a copy goes: the same size, and the same
/// first and last 64 KB.
pub fn same_contents(a: &Path, b: &Path) -> bool {
    const SPAN: u64 = 64 * 1024;
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    if ma.len() != mb.len() {
        return false;
    }
    let read = |p: &Path, from: u64| -> Option<Vec<u8>> {
        let mut f = std::fs::File::open(p).ok()?;
        f.seek(SeekFrom::Start(from)).ok()?;
        let mut v = Vec::new();
        f.take(SPAN).read_to_end(&mut v).ok()?;
        Some(v)
    };
    let tail = ma.len().saturating_sub(SPAN);
    read(a, 0).is_some() && read(a, 0) == read(b, 0) && read(a, tail) == read(b, tail)
}

/// Copies `src` into `folder`, or finds the copy made before. A different
/// file with the same name gets a numbered name. The copy is written under
/// a temporary name first, so one cut short never looks finished.
pub fn copy_in(src: &Path, folder: &Path) -> Result<PathBuf, String> {
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| format!("{} is not a file.", src.display()))?;
    std::fs::create_dir_all(folder)
        .map_err(|e| format!("Could not make {}: {e}", folder.display()))?;
    for n in 1..10_000 {
        let dest = folder.join(if n == 1 {
            name.clone()
        } else {
            numbered(&name, n)
        });
        if same_path(&dest, src) {
            return Ok(dest);
        }
        if dest.exists() {
            if same_contents(src, &dest) {
                return Ok(dest);
            }
            continue;
        }
        let part = folder.join(format!(
            "{}.part",
            dest.file_name().unwrap().to_string_lossy()
        ));
        std::fs::copy(src, &part).map_err(|e| {
            let _ = std::fs::remove_file(&part);
            format!("Could not copy {}: {e}", src.display())
        })?;
        std::fs::rename(&part, &dest)
            .map_err(|e| format!("Could not copy {}: {e}", src.display()))?;
        return Ok(dest);
    }
    Err(format!("Could not find a free name for {name}."))
}

/// Where imports into a saved session are copied: the session's folder
/// and its media subfolder.
#[derive(Debug, Clone)]
pub struct CopyTo {
    pub session_dir: PathBuf,
    pub media_dir: PathBuf,
}

impl CopyTo {
    /// The file to open: `src` itself when it is already in the session
    /// folder, else its copy in the media folder.
    pub fn place(&self, src: &Path) -> Result<PathBuf, String> {
        if inside(src, &self.session_dir) {
            Ok(src.to_path_buf())
        } else {
            copy_in(src, &self.media_dir)
        }
    }
}

// ---------------------------------------------------------- open sessions

/// A session set aside: everything the Multitrack keeps for the session
/// in front, so it comes back exactly as it was left.
pub struct Parked {
    pub session: Session,
    pub history: History<Session>,
    pub path: Option<PathBuf>,
    /// The file it was opened from (a .sesx keeps no path to save to).
    pub source: Option<PathBuf>,
    pub dirty: bool,
    pub view: View,
    pub sel: Option<(f64, f64)>,
    pub cursor: f64,
    pub selected_clip: Option<ClipId>,
    pub selected_track: Option<TrackId>,
    pub media_docs: HashMap<MediaId, DocId>,
    pub base: Option<serde_json::Value>,
    pub key: u64,
}

/// Files dropped from the file manager this frame, for the Multitrack to
/// take if they landed on its lanes.
pub struct OsDrop {
    pub paths: Vec<PathBuf>,
    /// The pointer when they were dropped (egui points), if known.
    pub pos: Option<egui::Pos2>,
    pub alt: bool,
}

/// How a batch of imported files is laid out once they have all loaded.
#[derive(Debug, Clone, Copy)]
pub enum Layout {
    Drop {
        track: Option<TrackId>,
        at: f64,
        alt: bool,
    },
    AsTracks,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Slot {
    Pending,
    Ready(DocId),
    Failed,
}

/// Files coming into the session together, placed in one step when the
/// last of them has loaded.
pub struct ImportBatch {
    pub id: u64,
    pub layout: Layout,
    pub slots: Vec<Slot>,
}

/// A recovery copy left by a run that did not end cleanly.
#[derive(Debug, Clone)]
pub struct Recoverable {
    pub file: PathBuf,
    pub note: Option<AutosaveNote>,
}

/// What sits beside each recovery copy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutosaveNote {
    pub key: u64,
    pub name: String,
    /// The session's own file, if it had one.
    pub path: Option<PathBuf>,
    pub pid: u32,
}

/// The Session Properties window's working copy.
#[derive(Debug, Clone)]
pub struct PropsDraft {
    pub name: String,
    pub rate: u32,
}

/// Everything about the open sessions beyond the one in front.
pub struct Sessions {
    pub parked: Vec<Parked>,
    /// The front session's recovery-copy key.
    pub key: u64,
    pub source: Option<PathBuf>,
    pub recent: Vec<PathBuf>,
    pub batches: Vec<ImportBatch>,
    pub next_batch: u64,
    pub os_drop: Option<OsDrop>,
    pub props: Option<PropsDraft>,
    pub recovery: Vec<Recoverable>,
    pub last_lock: Option<Instant>,
    /// The session in front was made with New Session (or brought back
    /// from the list), so it is listed in Files even while empty.
    pub made: bool,
}

impl Default for Sessions {
    fn default() -> Self {
        Sessions {
            parked: Vec::new(),
            key: new_key(),
            source: None,
            recent: Vec::new(),
            batches: Vec::new(),
            next_batch: 1,
            os_drop: None,
            props: None,
            recovery: Vec::new(),
            last_lock: None,
            made: false,
        }
    }
}

impl Sessions {
    pub fn load() -> Sessions {
        Sessions {
            recent: load_recent(),
            recovery: find_recoverable(),
            ..Default::default()
        }
    }
}

pub fn new_key() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut z = t
        ^ N.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed)
        ^ u64::from(std::process::id()) << 40;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

// --------------------------------------------------------------- recovery

fn autosave_dir() -> PathBuf {
    data_dir().join("autosave")
}

fn lock_file(pid: u32) -> PathBuf {
    autosave_dir().join(format!("running-{pid}.lock"))
}

/// A lock touched at `touched` is still held at `now`.
pub fn lock_fresh(touched: i64, now: i64) -> bool {
    now - touched < LOCK_STALE_SECS
}

/// Marks this run as alive, so another copy of the app does not offer to
/// recover what it is still working on.
pub fn touch_lock() {
    let _ = std::fs::create_dir_all(autosave_dir());
    let _ = std::fs::write(lock_file(std::process::id()), now_secs().to_string());
}

fn lock_held(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    std::fs::read_to_string(lock_file(pid))
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .is_some_and(|t| lock_fresh(t, now_secs()))
}

/// Recovery copies whose run is over.
pub fn find_recoverable() -> Vec<Recoverable> {
    let dir = autosave_dir();
    let mut out = Vec::new();
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("audfrm") {
            continue;
        }
        let note: Option<AutosaveNote> = std::fs::read_to_string(p.with_extension("json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());
        match &note {
            Some(n) if lock_held(n.pid) => continue,
            // The single copy earlier versions kept.
            None if p.file_stem().and_then(|s| s.to_str()) != Some("autosave") => continue,
            _ => {}
        }
        out.push(Recoverable { file: p, note });
    }
    out
}

fn autosave_file(key: u64) -> PathBuf {
    autosave_dir().join(format!("{key:016x}.audfrm"))
}

pub fn delete_autosave(key: u64) {
    let f = autosave_file(key);
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_file(f.with_extension("json"));
}

fn write_autosave(
    key: u64,
    session: &mut Session,
    base: Option<&serde_json::Value>,
    path: Option<&Path>,
) {
    let f = autosave_file(key);
    let _ = std::fs::create_dir_all(autosave_dir());
    if af27_interop::audfrm::save(session, base, &f).is_ok() {
        let note = AutosaveNote {
            key,
            name: session.name.clone(),
            path: path.map(Path::to_path_buf),
            pid: std::process::id(),
        };
        if let Ok(s) = serde_json::to_string_pretty(&note) {
            let _ = std::fs::write(f.with_extension("json"), s);
        }
    }
}

/// Removes this run's recovery copies and lock (a clean exit).
pub fn clean_exit() {
    let pid = std::process::id();
    for e in std::fs::read_dir(autosave_dir())
        .into_iter()
        .flatten()
        .flatten()
    {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let mine = std::fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str::<AutosaveNote>(&s).ok())
            .is_some_and(|n| n.pid == pid);
        if mine {
            let _ = std::fs::remove_file(p.with_extension("audfrm"));
            let _ = std::fs::remove_file(&p);
        }
    }
    let _ = std::fs::remove_file(lock_file(pid));
}

// ---------------------------------------------------------------- dialogs

pub enum Answer {
    Save,
    Discard,
    Cancel,
}

pub fn ask_save(what: &str) -> Answer {
    let r = rfd::MessageDialog::new()
        .set_title("AudiForm 2")
        .set_level(rfd::MessageLevel::Warning)
        .set_description(format!("Save changes to {what}?"))
        .set_buttons(rfd::MessageButtons::YesNoCancel)
        .show();
    match r {
        rfd::MessageDialogResult::Yes => Answer::Save,
        rfd::MessageDialogResult::No => Answer::Discard,
        _ => Answer::Cancel,
    }
}

fn ask_yes_no(text: &str) -> bool {
    rfd::MessageDialog::new()
        .set_title("AudiForm 2")
        .set_description(text)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes
}

pub fn templates_dir() -> PathBuf {
    data_dir().join("templates")
}

pub fn list_templates() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(templates_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("audfrm"))
        .collect();
    v.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
    v
}

/// Shows a file in the system file manager.
/// Opens a folder in the file manager.
pub fn reveal_dir(dir: &Path) {
    #[cfg(windows)]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(dir).spawn();
}

pub fn reveal(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = path.parent().unwrap_or(path);
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

// ---------------------------------------------------------- the app's side

impl State {
    /// A new blank session in the theme's track colours.
    pub fn blank_session(&self) -> Session {
        let mut s = Session::default();
        let n = self.theme.track_colors.len().max(1);
        for (i, t) in s.tracks.iter_mut().enumerate() {
            if let Some(c) = self.theme.track_colors.get(i % n) {
                t.color = *c;
            }
        }
        s
    }

    /// No session is open: the one in front is the blank stand-in and
    /// none is set aside. The Multitrack editor shows an empty state.
    pub fn no_session(&self) -> bool {
        self.front_is_blank() && self.sessions.parked.is_empty()
    }

    /// The session in front was never saved and nobody has touched it.
    pub fn front_is_blank(&self) -> bool {
        self.session_path.is_none()
            && !self.sessions.made
            && self.sessions.source.is_none()
            && !self.session_dirty
            && self.session_history.undo_label().is_none()
            && self.session.markers.is_empty()
            && self
                .session
                .tracks
                .iter()
                .all(|t| t.clips.is_empty() && t.midi.is_empty())
    }

    /// Takes the session in front out, leaving a blank one.
    fn take_front(&mut self) -> Parked {
        let blank = self.blank_session();
        let p = Parked {
            session: std::mem::replace(&mut self.session, blank),
            history: std::mem::replace(&mut self.session_history, History::new(200)),
            path: self.session_path.take(),
            source: self.sessions.source.take(),
            dirty: std::mem::take(&mut self.session_dirty),
            view: std::mem::replace(&mut self.session_view, View::fit(60.0)),
            sel: self.session_sel.take(),
            cursor: std::mem::take(&mut self.session_cursor),
            selected_clip: self.selected_clip.take(),
            selected_track: self.selected_track.take(),
            media_docs: std::mem::take(&mut self.media_docs),
            base: self.session_base.take(),
            key: std::mem::replace(&mut self.sessions.key, new_key()),
        };
        self.sessions.made = false;
        self.piano_clip = None;
        self.start_marker = None;
        self.gesture_before = None;
        self.session_version += 1;
        p
    }

    fn put_front(&mut self, p: Parked) {
        self.session = p.session;
        self.session_history = p.history;
        self.session_path = p.path;
        self.sessions.source = p.source;
        self.session_dirty = p.dirty;
        self.session_view = p.view;
        self.session_sel = p.sel;
        self.session_cursor = p.cursor;
        self.selected_clip = p.selected_clip;
        self.selected_track = p.selected_track;
        self.media_docs = p.media_docs;
        self.session_base = p.base;
        self.sessions.key = p.key;
        self.sessions.made = true;
        self.piano_clip = None;
        self.start_marker = None;
        self.session_version += 1;
    }

    /// Sets the session in front aside (a blank one is dropped), leaving
    /// a blank session in front.
    pub fn park_front(&mut self) {
        if self.is_playing() || self.is_recording() {
            self.stop();
        }
        if self.front_is_blank() {
            self.take_front();
            return;
        }
        let mut p = self.take_front();
        if p.dirty {
            let path = p.path.clone();
            write_autosave(p.key, &mut p.session, p.base.as_ref(), path.as_deref());
        }
        self.sessions.parked.push(p);
    }

    /// Brings set-aside session `i` to the front.
    pub fn switch_session(&mut self, i: usize) {
        if i >= self.sessions.parked.len() {
            return;
        }
        let p = self.sessions.parked.remove(i);
        self.park_front();
        self.put_front(p);
        self.mode = Mode::Multitrack;
    }

    /// Runs `f` with set-aside session `i` in front, then puts everything
    /// back where it was.
    fn with_parked(&mut self, i: usize, f: impl FnOnce(&mut State)) {
        if i >= self.sessions.parked.len() {
            return;
        }
        let p = self.sessions.parked.remove(i);
        let front = self.take_front();
        self.put_front(p);
        f(self);
        let p = self.take_front();
        self.put_front(front);
        self.sessions.parked.insert(i, p);
    }

    /// The open session (front: None, set aside: Some(i)) saved at or
    /// opened from `path`.
    pub fn find_open(&self, path: &Path) -> Option<Option<usize>> {
        let hit = |p: &Option<PathBuf>| p.as_deref().is_some_and(|p| same_path(p, path));
        let alt = path.with_extension("audfrm");
        let hit_alt = |p: &Option<PathBuf>| p.as_deref().is_some_and(|p| same_path(p, &alt));
        let altaudio = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("altaudio"));
        if !self.front_is_blank()
            && (hit(&self.session_path)
                || hit(&self.sessions.source)
                || (altaudio && hit_alt(&self.session_path)))
        {
            return Some(None);
        }
        self.sessions
            .parked
            .iter()
            .position(|p| hit(&p.path) || hit(&p.source) || (altaudio && hit_alt(&p.path)))
            .map(Some)
    }

    pub fn remember_recent(&mut self, p: &Path) {
        push_recent(&mut self.sessions.recent, p);
        save_recent(&self.sessions.recent);
    }

    /// File > New Multitrack Session, blank or from a template.
    pub fn new_session(&mut self, template: Option<&Path>) {
        let made = match template {
            None => None,
            Some(t) => match af27_interop::import(t) {
                Ok(i) => Some(i),
                Err(e) => {
                    self.error(e);
                    return;
                }
            },
        };
        self.park_front();
        self.sessions.made = true;
        // Each new session gets its own name in the list.
        let taken: Vec<String> = self
            .sessions
            .parked
            .iter()
            .map(|p| p.session.name.clone())
            .collect();
        self.session.name = untitled_name(&taken);
        if let Some(i) = made {
            let name = std::mem::take(&mut self.session.name);
            self.session = i.session;
            self.session.name = name;
            // A session of its own: a new id, and where it came from.
            self.session_base = i.base.map(|mut b| {
                if let Some(o) = b.as_object_mut() {
                    let from = o.remove("id").unwrap_or(serde_json::Value::Null);
                    o.remove("created");
                    o.insert("template_id".into(), from);
                }
                b
            });
            self.session_version += 1;
            let missing = self.link_session_media();
            if missing > 0 {
                self.error(format!(
                    "{missing} media file(s) in the template could not be found."
                ));
            }
            self.session_view = View::fit(self.session.duration().max(30.0));
        }
        self.mode = Mode::Multitrack;
    }

    /// Asks to save the session in front if it has changes. False: the
    /// user cancelled (or the save did not happen).
    fn settle_front(&mut self) -> bool {
        if !self.session_dirty || self.front_is_blank() {
            return true;
        }
        match ask_save(&format!("\"{}\"", self.session.name)) {
            Answer::Save => self.save_front(false),
            Answer::Discard => true,
            Answer::Cancel => false,
        }
    }

    /// Closes the session in front (asking to save first); the last one
    /// set aside comes forward. False when cancelled.
    pub fn close_front(&mut self) -> bool {
        if !self.settle_front() {
            return false;
        }
        if self.is_playing() || self.is_recording() {
            self.stop();
        }
        delete_autosave(self.sessions.key);
        self.take_front();
        if let Some(p) = self.sessions.parked.pop() {
            self.put_front(p);
        }
        true
    }

    pub fn close_parked(&mut self, i: usize) -> bool {
        if i >= self.sessions.parked.len() {
            return false;
        }
        self.switch_session(i);
        self.close_front()
    }

    /// Closes every session, asking for each. False when cancelled.
    pub fn close_all_sessions(&mut self) -> bool {
        loop {
            if self.front_is_blank() && self.sessions.parked.is_empty() {
                return true;
            }
            if !self.close_front() {
                return false;
            }
        }
    }

    /// Saves the session in front: to its file, or asking where (always
    /// with `save_as`). True when it was saved.
    pub fn save_front(&mut self, save_as: bool) -> bool {
        let path = match (&self.session_path, save_as) {
            (Some(p), false) => Some(p.clone()),
            _ => {
                let mut d = rfd::FileDialog::new()
                    .set_file_name(format!("{}.audfrm", self.session.name))
                    .add_filter("Audiform session", &["audfrm"])
                    .add_filter("AudiForm 2 model dump", &["af27"]);
                if let Some(dir) = self.session_path.as_ref().and_then(|p| p.parent()) {
                    d = d.set_directory(dir);
                }
                d.save_file()
            }
        };
        let Some(path) = path else { return false };
        self.save_session_to(path);
        !self.session_dirty
    }

    /// File > Save All: every open session with changes, and any never
    /// saved that has something in it.
    pub fn save_all(&mut self) {
        if !self.front_is_blank() && (self.session_dirty || self.session_path.is_none()) {
            self.save_front(false);
        }
        for i in 0..self.sessions.parked.len() {
            let p = &self.sessions.parked[i];
            if p.dirty || p.path.is_none() {
                self.with_parked(i, |st| {
                    st.save_front(false);
                });
            }
        }
    }

    /// Sessions with unsaved changes, front first.
    pub fn unsaved_sessions(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.session_dirty && !self.front_is_blank() {
            v.push(self.session.name.clone());
        }
        v.extend(
            self.sessions
                .parked
                .iter()
                .filter(|p| p.dirty)
                .map(|p| p.session.name.clone()),
        );
        v
    }

    /// The window is closing. False: keep it open (the user cancelled or
    /// a save did not happen).
    pub fn may_quit(&mut self) -> bool {
        let unsaved = self.unsaved_sessions();
        if !unsaved.is_empty() {
            let what = if unsaved.len() == 1 {
                format!("\"{}\"", unsaved[0])
            } else {
                format!("{} open sessions", unsaved.len())
            };
            match ask_save(&format!("{what} before quitting")) {
                Answer::Save => {
                    self.save_all();
                    if !self.unsaved_sessions().is_empty() {
                        return false;
                    }
                }
                Answer::Discard => {}
                Answer::Cancel => return false,
            }
        }
        clean_exit();
        true
    }

    /// File > Save as Session Template: the session as it is (tracks,
    /// levels, clips, markers) as a template for new sessions.
    pub fn save_template(&mut self) {
        let dir = templates_dir();
        let _ = std::fs::create_dir_all(&dir);
        let Some(path) = rfd::FileDialog::new()
            .set_directory(&dir)
            .set_file_name(format!("{}.audfrm", self.session.name))
            .add_filter("Session template", &["audfrm"])
            .save_file()
        else {
            return;
        };
        let mut s = self.session.clone();
        match af27_interop::audfrm::save(&mut s, self.session_base.as_ref(), &path) {
            Ok(_) => self.toast(format!(
                "Saved the template {}",
                path.file_stem().unwrap_or_default().to_string_lossy()
            )),
            Err(e) => self.error(e),
        }
    }

    /// Asks for session media files and loads them, linking any already
    /// open. Returns how many could not be found.
    pub fn link_session_media(&mut self) -> usize {
        let media: Vec<_> = self.session.media.clone();
        let mut missing = 0;
        for m in media {
            match &m.path {
                Some(p) if p.exists() => {
                    if let Some(d) = self
                        .docs
                        .iter()
                        .find(|d| d.path.as_deref().is_some_and(|q| same_path(q, p)))
                    {
                        let id = d.id;
                        self.link_media(m.id, id);
                    } else if !self.jobs.iter().any(|j| j.path() == p) {
                        self.load_file(p.clone(), None);
                    }
                }
                _ => missing += 1,
            }
        }
        missing
    }

    // ------------------------------------------------------------- media

    /// The session's media folder, when it has been saved.
    pub fn copy_target(&self) -> Option<CopyTo> {
        let dir = self.session_path.as_ref()?.parent()?.to_path_buf();
        let folder = self.prefs.media_folder.trim();
        let folder = if folder.is_empty() { "Audio" } else { folder };
        Some(CopyTo {
            media_dir: dir.join(folder),
            session_dir: dir,
        })
    }

    /// Where `n` files coming into the session are copied, by the Session
    /// Media setting (asking when it says so).
    fn import_copy_target(&self, n: usize) -> Option<CopyTo> {
        let to = self.copy_target()?;
        match self.prefs.media_policy {
            MediaPolicy::Leave => None,
            MediaPolicy::Copy => Some(to),
            MediaPolicy::Ask => {
                let what = if n == 1 {
                    "the file".to_string()
                } else {
                    format!("the {n} files")
                };
                ask_yes_no(&format!(
                    "Copy {what} into the session's {} folder?",
                    to.media_dir
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                ))
                .then_some(to)
            }
        }
    }

    /// The document open for `path`, if any.
    pub fn doc_for_path(&self, path: &Path) -> Option<DocId> {
        self.docs
            .iter()
            .find(|d| d.path.as_deref().is_some_and(|q| same_path(q, path)))
            .map(|d| d.id)
    }

    /// Brings files (and folders) into the session, laid out by `layout`
    /// once they have all loaded. Sessions among them are opened.
    pub fn import_paths(&mut self, paths: Vec<PathBuf>, layout: Layout) {
        let (audio, sessions) = expand_paths(&paths);
        for s in sessions {
            self.open_session(&s);
        }
        if audio.is_empty() {
            return;
        }
        let copy = self.import_copy_target(audio.len());
        let id = self.sessions.next_batch;
        self.sessions.next_batch += 1;
        let mut slots = Vec::with_capacity(audio.len());
        for (i, p) in audio.into_iter().enumerate() {
            match self.doc_for_path(&p) {
                Some(d) => slots.push(Slot::Ready(d)),
                None => {
                    slots.push(Slot::Pending);
                    self.load_into(p, Some((id, i)), copy.clone());
                }
            }
        }
        self.sessions
            .batches
            .push(ImportBatch { id, layout, slots });
        self.finish_batches();
    }

    /// Puts open documents into the session in one step.
    pub fn import_docs(&mut self, docs: Vec<DocId>, layout: Layout) {
        if docs.is_empty() {
            return;
        }
        self.place_batch(&docs, layout);
    }

    /// A finished load for a batch slot.
    pub fn batch_loaded(&mut self, batch: (u64, usize), doc: Option<DocId>) {
        if let Some(b) = self.sessions.batches.iter_mut().find(|b| b.id == batch.0) {
            if let Some(s) = b.slots.get_mut(batch.1) {
                *s = doc.map_or(Slot::Failed, Slot::Ready);
            }
        }
    }

    /// Places every batch whose files have all loaded.
    pub fn finish_batches(&mut self) {
        let mut i = 0;
        while i < self.sessions.batches.len() {
            if self.sessions.batches[i].slots.contains(&Slot::Pending) {
                i += 1;
                continue;
            }
            let b = self.sessions.batches.remove(i);
            let docs: Vec<DocId> = b
                .slots
                .iter()
                .filter_map(|s| match s {
                    Slot::Ready(d) => Some(*d),
                    _ => None,
                })
                .collect();
            let failed = b.slots.len() - docs.len();
            if failed > 0 {
                self.error(format!(
                    "{failed} file(s) could not be read and were left out."
                ));
            }
            if !docs.is_empty() {
                self.place_batch(&docs, b.layout);
            }
        }
    }

    /// Lays `docs` out in the session as one undoable step.
    fn place_batch(&mut self, docs: &[DocId], layout: Layout) {
        let before = self.session.clone();
        let was_empty = self
            .session
            .tracks
            .iter()
            .all(|t| t.clips.is_empty() && t.midi.is_empty());
        let lengths: Vec<f64> = docs
            .iter()
            .map(|d| self.doc_by_id(*d).map_or(0.0, |d| d.duration()))
            .collect();
        let audio: Vec<bool> = self
            .session
            .tracks
            .iter()
            .map(|t| t.kind == TrackKind::Audio)
            .collect();
        let placements = match layout {
            Layout::Drop { track, at, alt } => {
                let target = track.and_then(|id| self.session.track_index(id));
                layout_drop(&lengths, &audio, target, at, alt)
            }
            Layout::AsTracks => layout_as_tracks(docs.len()),
        };
        let tracks: Vec<TrackId> = self.session.tracks.iter().map(|t| t.id).collect();
        let mut new_tracks: Vec<TrackId> = Vec::new();
        let mut last = None;
        for (&doc, pl) in docs.iter().zip(placements) {
            let track = match pl.lane {
                Lane::Track(i) => tracks[i],
                Lane::New(k) => {
                    while new_tracks.len() <= k {
                        let name = self
                            .doc_by_id(doc)
                            .map(|d| strip_speaker_number(&d.name))
                            .unwrap_or_default();
                        let id = self.session.add_track_of(TrackKind::Audio);
                        let n = self.session.tracks.len() - 1;
                        let colors = &self.theme.track_colors;
                        let color = colors.get(n % colors.len().max(1)).copied();
                        if let Some(t) = self.session.track_mut(id) {
                            if !name.is_empty() {
                                t.name = name;
                            }
                            if let Some(c) = color {
                                t.color = c;
                            }
                        }
                        new_tracks.push(id);
                    }
                    new_tracks[k]
                }
            };
            if let Some(m) = self.media_for_doc(doc) {
                last = self.session.add_clip(track, m, pl.start).or(last);
            }
        }
        if last.is_none() && new_tracks.is_empty() {
            return;
        }
        self.selected_clip = last;
        let label = if docs.len() == 1 {
            "Add Clip".to_string()
        } else {
            format!("Add {} Clips", docs.len())
        };
        self.commit_session(&label, before);
        self.mode = Mode::Multitrack;
        if was_empty {
            self.session_view = View::fit(self.session.duration().max(10.0) * 1.05);
        }
    }

    /// Media items no clip uses that were never saved with the session
    /// (they came in and went out again in this run). A saved session
    /// only lists files that are in it.
    pub fn prune_unsaved_media(&mut self) {
        let used: std::collections::HashSet<MediaId> = self
            .session
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.media))
            .collect();
        let saved: std::collections::HashSet<String> = self
            .session_base
            .as_ref()
            .and_then(|b| b["media"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|m| m["id"].as_str().map(str::to_string))
            .collect();
        let gone: Vec<MediaId> = self
            .session
            .media
            .iter()
            .filter(|m| {
                !used.contains(&m.id) && !m.ext_id.as_ref().is_some_and(|e| saved.contains(e))
            })
            .map(|m| m.id)
            .collect();
        if gone.is_empty() {
            return;
        }
        self.session.media.retain(|m| !gone.contains(&m.id));
        self.media_docs.retain(|m, _| !gone.contains(m));
    }

    /// On save: files that came into the session in this run and still
    /// sit outside its folder are copied in (Session Media setting).
    /// Files the session already had are left where its file says.
    pub fn copy_new_media_on_save(&mut self, session_file: &Path) {
        if !self.prefs.copy_on_save || self.prefs.media_policy == MediaPolicy::Leave {
            return;
        }
        let Some(dir) = session_file.parent() else {
            return;
        };
        let folder = self.prefs.media_folder.trim();
        let to = CopyTo {
            session_dir: dir.to_path_buf(),
            media_dir: dir.join(if folder.is_empty() { "Audio" } else { folder }),
        };
        let saved: std::collections::HashSet<String> = self
            .session_base
            .as_ref()
            .and_then(|b| b["media"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|m| m["id"].as_str().map(str::to_string))
            .collect();
        let mut moves = Vec::new();
        for m in &self.session.media {
            if m.ext_id.as_ref().is_some_and(|e| saved.contains(e)) {
                continue;
            }
            if let Some(p) = m.path.as_ref().filter(|p| p.is_file()) {
                if !inside(p, dir) {
                    moves.push((m.id, p.clone()));
                }
            }
        }
        let mut failed = 0;
        for (id, from) in moves {
            match to.place(&from) {
                Ok(dest) => self.repath(id, &from, dest),
                Err(_) => failed += 1,
            }
        }
        if failed > 0 {
            self.error(format!(
                "{failed} file(s) could not be copied into the session folder."
            ));
        }
    }

    /// Points media `id`, and the Files panel's document for `from`, at
    /// `dest`.
    fn repath(&mut self, id: MediaId, from: &Path, dest: PathBuf) {
        if let Some(m) = self.session.media.iter_mut().find(|m| m.id == id) {
            m.path = Some(dest.clone());
        }
        for d in &mut self.docs {
            if d.path.as_deref().is_some_and(|p| same_path(p, from)) {
                d.path = Some(dest.clone());
            }
        }
    }

    /// Session save keeps the Waveform editor's unsaved edits to files the
    /// session uses: written into the file when it is a WAV in the session
    /// folder, otherwise as "<name> (edited).wav" in the media folder, the
    /// session and the Files panel moving to it. The original is untouched.
    pub fn keep_waveform_edits(&mut self, session_file: &Path) {
        let Some(dir) = session_file.parent().map(Path::to_path_buf) else {
            return;
        };
        let used: std::collections::HashSet<MediaId> = self
            .session
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.media))
            .collect();
        let edited: Vec<(MediaId, DocId)> = self
            .media_docs
            .iter()
            .filter(|(m, d)| used.contains(m) && self.doc_by_id(**d).is_some_and(|d| d.dirty))
            .map(|(m, d)| (*m, *d))
            .collect();
        let folder = self.prefs.media_folder.trim();
        let media_dir = dir.join(if folder.is_empty() { "Audio" } else { folder });
        for (m, d) in edited {
            let Some(doc) = self.doc_by_id(d) else {
                continue;
            };
            let buf = doc.buf.clone();
            let src = doc.path.clone();
            // FLAC, or float WAV for audio past full scale.
            let ext = crate::new_dialogs::lossless_ext(&buf);
            let wav_inside = src.as_deref().is_some_and(|p| {
                inside(p, &dir) && p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
            });
            let dest = if wav_inside {
                src.clone().unwrap()
            } else {
                let stem = Path::new(&doc.name)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Audio".into());
                let name = format!("{stem} (edited).{ext}");
                let _ = std::fs::create_dir_all(&media_dir);
                let mut dest = media_dir.join(&name);
                let mut n = 2;
                while dest.exists() {
                    dest = media_dir.join(numbered(&name, n));
                    n += 1;
                }
                dest
            };
            let part = dest.with_extension(format!("{ext}.part"));
            let written = crate::new_dialogs::write_lossless(&part, &buf, self.session.bit_depth)
                .and_then(|_| std::fs::rename(&part, &dest).map_err(|e| e.to_string()));
            match written {
                Ok(()) => {
                    let dur = buf.duration_secs();
                    if let Some(item) = self.session.media.iter_mut().find(|i| i.id == m) {
                        item.path = Some(dest.clone());
                        item.duration = dur;
                        if !wav_inside {
                            item.name = file_name(&dest);
                        }
                    }
                    if let Some(doc) = self.docs.iter_mut().find(|x| x.id == d) {
                        doc.path = Some(dest.clone());
                        doc.name = file_name(&dest);
                        doc.dirty = false;
                    }
                    if !wav_inside {
                        self.toast(format!("Kept the Waveform edits as {}", file_name(&dest)));
                    }
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&part);
                    self.error(format!("Could not keep the Waveform edits: {e}"));
                }
            }
        }
    }

    /// File > Collect Session Media: copies every file the session uses
    /// into its media folder, as one undoable step (Undo points the
    /// session back at the originals; the copies stay).
    pub fn collect_media(&mut self) {
        let Some(to) = self.copy_target() else {
            self.error("Save the session first: its media folder sits beside the session file.");
            return;
        };
        let before = self.session.clone();
        let wanted: Vec<(MediaId, PathBuf)> = self
            .session
            .media
            .iter()
            .filter_map(|m| m.path.clone().map(|p| (m.id, p)))
            .filter(|(_, p)| p.is_file() && !inside(p, &to.session_dir))
            .collect();
        let (mut n, mut failed) = (0, 0);
        for (id, from) in wanted {
            match copy_in(&from, &to.media_dir) {
                Ok(dest) => {
                    self.repath(id, &from, dest);
                    n += 1;
                }
                Err(_) => failed += 1,
            }
        }
        if n > 0 {
            self.commit_session("Collect Session Media", before);
        }
        let mut msg = match n {
            0 => "Every file is already in the session folder.".to_string(),
            1 => "Copied 1 file into the session folder.".to_string(),
            _ => format!("Copied {n} files into the session folder."),
        };
        if failed > 0 {
            msg.push_str(&format!(" {failed} could not be copied."));
        }
        self.toast(msg);
    }

    /// The document is played by a clip in any open session.
    pub fn doc_used(&self, id: DocId) -> bool {
        let used_in = |s: &Session, md: &HashMap<MediaId, DocId>| {
            s.tracks
                .iter()
                .flat_map(|t| &t.clips)
                .any(|c| md.get(&c.media) == Some(&id))
        };
        used_in(&self.session, &self.media_docs)
            || self
                .sessions
                .parked
                .iter()
                .any(|p| used_in(&p.session, &p.media_docs))
    }

    /// Remove Unused Files: closes every file no clip in any open session
    /// plays. Files with unsaved Waveform edits are closed only after
    /// asking.
    pub fn remove_unused_files(&mut self) {
        let unused: Vec<DocId> = self
            .docs
            .iter()
            .map(|d| d.id)
            .filter(|&d| !self.doc_used(d))
            .collect();
        let edited = unused
            .iter()
            .filter(|&&d| self.doc_by_id(d).is_some_and(|d| d.dirty))
            .count();
        let drop_edited = edited == 0
            || ask_yes_no(&format!(
                "{edited} unused file(s) have unsaved edits. Close them and discard the edits?"
            ));
        let mut n = 0;
        for d in unused {
            if !drop_edited && self.doc_by_id(d).is_some_and(|d| d.dirty) {
                continue;
            }
            self.close_doc(d);
            n += 1;
        }
        self.toast(match n {
            0 => "Every file is in use.".to_string(),
            1 => "Removed 1 unused file.".to_string(),
            _ => format!("Removed {n} unused files."),
        });
    }

    // ---------------------------------------------------------- autosave

    /// Writes recovery copies of every session with unsaved changes, every
    /// few minutes, and keeps this run's lock fresh.
    pub fn autosave(&mut self) {
        if self
            .sessions
            .last_lock
            .is_none_or(|t| t.elapsed().as_secs() >= 30)
        {
            touch_lock();
            self.sessions.last_lock = Some(Instant::now());
        }
        let every = self.prefs.autosave_minutes.max(1) as u64 * 60;
        if !self.prefs.autosave || self.last_autosave.elapsed().as_secs() < every {
            return;
        }
        self.last_autosave = Instant::now();
        if self.session_dirty && !self.front_is_blank() {
            let base = self.session_base.clone();
            let path = self.session_path.clone();
            write_autosave(
                self.sessions.key,
                &mut self.session,
                base.as_ref(),
                path.as_deref(),
            );
        }
        for p in &mut self.sessions.parked {
            if p.dirty {
                let path = p.path.clone();
                write_autosave(p.key, &mut p.session, p.base.as_ref(), path.as_deref());
            }
        }
    }

    /// Opens a recovery copy as unsaved work that saves to its own file.
    pub fn recover(&mut self, r: &Recoverable) {
        let text = match std::fs::read_to_string(&r.file) {
            Ok(t) => t,
            Err(e) => {
                self.error(format!("Could not read the recovery copy: {e}"));
                return;
            }
        };
        let dir = r.file.parent().unwrap_or(Path::new("."));
        match af27_interop::audfrm::import(&text, dir) {
            Ok(i) => {
                self.park_front();
                self.session = i.session;
                self.session_base = i.base;
                self.session_path = r.note.as_ref().and_then(|n| n.path.clone());
                self.session_dirty = true;
                self.session_version += 1;
                self.session_view = View::fit(self.session.duration().max(30.0));
                self.session_cursor = self.session.playhead.max(0.0);
                self.mode = Mode::Multitrack;
                let missing = self.link_session_media();
                if missing > 0 {
                    self.error(format!("{missing} media file(s) could not be found."));
                }
                self.toast(format!("Recovered {}", self.session.name));
                discard_recovery(r);
            }
            Err(e) => self.error(e),
        }
    }

    /// Session Properties: name and sample rate, one undoable step.
    pub fn apply_props(&mut self, d: &PropsDraft) {
        if d.name == self.session.name && d.rate == self.session.rate {
            return;
        }
        let before = self.session.clone();
        if !d.name.trim().is_empty() {
            self.session.name = d.name.trim().to_string();
        }
        self.session.rate = d.rate.max(8_000);
        self.commit_session("Session Properties", before);
    }
}

pub fn discard_recovery(r: &Recoverable) {
    let _ = std::fs::remove_file(&r.file);
    let _ = std::fs::remove_file(r.file.with_extension("json"));
}

// ---------------------------------------------------------------- windows

/// The recovery offer at startup, and Session Properties.
pub fn windows(ctx: &egui::Context, st: &mut State) {
    if !st.sessions.recovery.is_empty() {
        let mut act: Option<(usize, bool)> = None;
        crate::ui::popup(
            ctx,
            egui::Id::new(("af2-popup", "sessions.rs", 0)),
            "Recover Unsaved Sessions",
        )
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(
                "AudiForm 2 did not close normally last time. These sessions had unsaved changes:",
            );
            ui.add_space(6.0);
            for (i, r) in st.sessions.recovery.iter().enumerate() {
                ui.horizontal(|ui| {
                    let name = r
                        .note
                        .as_ref()
                        .map_or("Autosave (earlier version)".to_string(), |n| n.name.clone());
                    let saved = std::fs::metadata(&r.file)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| stamp(d.as_secs() as i64))
                        .unwrap_or_default();
                    let where_ = r
                        .note
                        .as_ref()
                        .and_then(|n| n.path.as_ref())
                        .map_or("never saved".to_string(), |p| p.display().to_string());
                    ui.label(egui::RichText::new(name).strong())
                        .on_hover_text(format!("{where_}\nCopy written {saved} UTC"));
                    if ui.button("Recover").clicked() {
                        act = Some((i, true));
                    }
                    if ui.button("Discard").clicked() {
                        act = Some((i, false));
                    }
                });
            }
        });
        if let Some((i, keep)) = act {
            let r = st.sessions.recovery.remove(i);
            if keep {
                st.recover(&r);
            } else {
                discard_recovery(&r);
            }
        }
    }
    if let Some(mut d) = st.sessions.props.take() {
        let mut open = true;
        let mut done = false;
        let mut cancel = false;
        crate::ui::popup(
            ctx,
            egui::Id::new(("af2-popup", "sessions.rs", 1)),
            "Session Properties",
        )
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            let s = &st.session;
            egui::Grid::new("af2-session-props")
                .num_columns(2)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut d.name);
                    ui.end_row();
                    ui.label("Sample rate");
                    egui::ComboBox::from_id_salt("af2-props-rate")
                        .selected_text(format!("{} Hz", d.rate))
                        .show_ui(ui, |ui| {
                            for r in [
                                22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000,
                            ] {
                                ui.selectable_value(&mut d.rate, r, format!("{r} Hz"));
                            }
                        });
                    ui.end_row();
                    let depth = st
                        .session_base
                        .as_ref()
                        .and_then(|b| b["bit_depth"].as_u64())
                        .unwrap_or(32);
                    ui.label("Bit depth");
                    ui.label(format!("{depth}-bit"));
                    ui.end_row();
                    let clips: usize = s.tracks.iter().map(|t| t.clips.len() + t.midi.len()).sum();
                    let used: std::collections::HashSet<MediaId> = s
                        .tracks
                        .iter()
                        .flat_map(|t| t.clips.iter().map(|c| c.media))
                        .collect();
                    for (k, v) in [
                        ("Tracks", s.tracks.len().to_string()),
                        ("Clips", clips.to_string()),
                        ("Files used", used.len().to_string()),
                        ("Markers", s.markers.len().to_string()),
                        ("Length", af27_core::time::format_time(s.duration())),
                    ] {
                        ui.label(k);
                        ui.label(v);
                        ui.end_row();
                    }
                    ui.label("File");
                    ui.label(
                        st.session_path
                            .as_ref()
                            .map_or("Not saved yet".into(), |p| p.display().to_string()),
                    );
                    ui.end_row();
                    let saved = st
                        .session_path
                        .as_ref()
                        .and_then(|p| std::fs::metadata(p).ok())
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map_or("Never".into(), |t| {
                            format!("{} UTC", stamp(t.as_secs() as i64))
                        });
                    ui.label("Saved");
                    ui.label(if st.session_dirty {
                        format!("{saved} (changed since)")
                    } else {
                        saved
                    });
                    ui.end_row();
                });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() {
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if done {
            st.apply_props(&d);
        } else if open && !cancel {
            st.sessions.props = Some(d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lanes(p: &[Placement]) -> Vec<Lane> {
        p.iter().map(|p| p.lane).collect()
    }

    #[test]
    fn several_files_go_one_per_track_from_the_drop_time() {
        // Tracks: audio, audio, bus, audio. Drop three on track 0 at 5 s.
        let audio = [true, true, false, true];
        let p = layout_drop(&[10.0, 20.0, 30.0], &audio, Some(0), 5.0, false);
        assert_eq!(lanes(&p), [Lane::Track(0), Lane::Track(1), Lane::Track(3)]);
        assert!(p.iter().all(|p| p.start == 5.0));
        // More files than audio tracks below: new tracks after them.
        let p = layout_drop(&[1.0; 5], &audio, Some(1), 0.0, false);
        assert_eq!(
            lanes(&p),
            [
                Lane::Track(1),
                Lane::Track(3),
                Lane::New(0),
                Lane::New(1),
                Lane::New(2)
            ]
        );
        // Tracks above the drop are never used.
        let p = layout_drop(&[1.0, 1.0], &audio, Some(3), 2.0, false);
        assert_eq!(lanes(&p), [Lane::Track(3), Lane::New(0)]);
    }

    #[test]
    fn alt_puts_them_one_after_another_on_one_track() {
        let audio = [true, true];
        let p = layout_drop(&[10.0, 20.0, 30.0], &audio, Some(1), 5.0, true);
        assert_eq!(lanes(&p), [Lane::Track(1); 3]);
        let starts: Vec<f64> = p.iter().map(|p| p.start).collect();
        assert_eq!(starts, [5.0, 15.0, 35.0]);
        // Below the last track: one new track.
        let p = layout_drop(&[1.0, 2.0], &audio, None, 0.0, true);
        assert_eq!(lanes(&p), [Lane::New(0); 2]);
        assert_eq!(p[1].start, 1.0);
    }

    #[test]
    fn a_drop_below_the_tracks_or_on_a_bus_makes_new_tracks() {
        let audio = [true, false];
        let p = layout_drop(&[1.0, 1.0], &audio, None, 3.0, false);
        assert_eq!(lanes(&p), [Lane::New(0), Lane::New(1)]);
        let p = layout_drop(&[1.0, 1.0], &audio, Some(1), 3.0, false);
        assert_eq!(lanes(&p), [Lane::New(0), Lane::New(1)]);
        let p = layout_drop(&[1.0, 1.0], &audio, Some(1), 3.0, true);
        assert_eq!(lanes(&p), [Lane::New(0), Lane::New(0)]);
        // One file, either way, lands at the drop point.
        for alt in [false, true] {
            let p = layout_drop(&[4.0], &[true], Some(0), -2.0, alt);
            assert_eq!(
                p,
                [Placement {
                    lane: Lane::Track(0),
                    start: 0.0
                }]
            );
        }
        assert!(layout_drop(&[], &audio, Some(0), 0.0, false).is_empty());
    }

    #[test]
    fn insert_as_tracks_puts_each_on_a_new_track_at_the_start() {
        let p = layout_as_tracks(3);
        assert_eq!(lanes(&p), [Lane::New(0), Lane::New(1), Lane::New(2)]);
        assert!(p.iter().all(|p| p.start == 0.0));
    }

    #[test]
    fn drops_near_the_start_land_on_it() {
        // 100 px a second, 6 px snap: within 18 px (0.18 s) goes to 0.
        assert_eq!(snap_drop_start(0.15, 100.0, 6.0), 0.0);
        assert_eq!(snap_drop_start(0.5, 100.0, 6.0), 0.5);
    }

    #[test]
    fn speaker_numbers_come_off_track_names() {
        assert_eq!(strip_speaker_number("1-name.flac"), "name");
        assert_eq!(strip_speaker_number("02 - Guest Two.wav"), "Guest Two");
        assert_eq!(strip_speaker_number("3_host.mp3"), "host");
        assert_eq!(strip_speaker_number("12. Intro.wav"), "Intro");
        // Not a speaker number.
        assert_eq!(
            strip_speaker_number("2026-11-19 call.wav"),
            "2026-11-19 call"
        );
        assert_eq!(strip_speaker_number("1984.wav"), "1984");
        assert_eq!(strip_speaker_number("7.wav"), "7");
        assert_eq!(strip_speaker_number("3-.wav"), "3-");
        assert_eq!(strip_speaker_number("voice.wav"), "voice");
        assert_eq!(strip_speaker_number("4track"), "4track");
    }

    #[test]
    fn natural_order_counts_numbers() {
        let mut v = vec!["10-b.wav", "outro.wav", "2-a.wav", "1-c.wav", "Intro.wav"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            v,
            ["1-c.wav", "2-a.wav", "10-b.wav", "Intro.wav", "outro.wav"]
        );
        assert_eq!(
            natural_cmp("track 02", "track 2"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn recent_sessions_are_newest_first_once_and_ten_at_most() {
        let mut list = Vec::new();
        for i in 0..12 {
            push_recent(&mut list, Path::new(&format!("s{i}.audfrm")));
        }
        assert_eq!(list.len(), RECENT_MAX);
        assert_eq!(list[0], PathBuf::from("s11.audfrm"));
        assert_eq!(list[9], PathBuf::from("s2.audfrm"));
        // Opening one again moves it to the top without a duplicate.
        push_recent(&mut list, Path::new("s5.audfrm"));
        assert_eq!(list[0], PathBuf::from("s5.audfrm"));
        assert_eq!(list.len(), RECENT_MAX);
        assert_eq!(list.iter().filter(|p| p.ends_with("s5.audfrm")).count(), 1);
        assert_eq!(list[1], PathBuf::from("s11.audfrm"));
    }

    #[cfg(windows)]
    #[test]
    fn recent_paths_ignore_case_on_windows() {
        let mut list = vec![PathBuf::from(r"C:\Shows\Ep1.audfrm")];
        push_recent(&mut list, Path::new(r"c:\shows\EP1.audfrm"));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn stamps_are_utc_dates_that_sort() {
        assert_eq!(stamp(0), "1970-01-01 00-00-00");
        // 2026-11-19 14:03:22 UTC.
        assert_eq!(stamp(1_795_097_002), "2026-11-19 14-03-22");
        assert_eq!(stamp(951_782_400), "2000-02-29 00-00-00");
    }

    #[test]
    fn backups_keep_the_newest_and_touch_nothing_else() {
        let stem = "Show";
        let mut names: Vec<String> = (0..6)
            .map(|i| backup_name(stem, 1_795_097_002 + i * 60))
            .collect();
        names.push("Show notes.txt".into());
        names.push("Show.audfrm".into());
        names.push("Other 2026-11-19 14-03-22.audfrm".into());
        names.push("Show 2026-11-19 14-03-22 copy.audfrm".into());
        names.reverse();
        let gone = backups_to_prune(&names, stem, 4);
        assert_eq!(
            gone,
            [
                backup_name(stem, 1_795_097_002),
                backup_name(stem, 1_795_097_002 + 60)
            ]
        );
        assert!(backups_to_prune(&names, stem, 10).is_empty());
        // Keep at least one.
        assert_eq!(backups_to_prune(&names, stem, 0).len(), 5);
        assert_eq!(
            backup_stamp("Show 2026-11-19 14-03-22.audfrm", "Show"),
            Some("2026-11-19 14-03-22")
        );
        assert_eq!(
            backup_stamp("Show 2026-11-19 14-03-2x.audfrm", "Show"),
            None
        );
    }

    #[test]
    fn backing_up_rotates_on_disk() {
        let dir = std::env::temp_dir().join(format!("af2-backup-{}", new_key()));
        std::fs::create_dir_all(dir.join("Backups")).unwrap();
        let file = dir.join("Ep.audfrm");
        std::fs::write(&file, "{}").unwrap();
        for i in 0..5 {
            let old = dir.join("Backups").join(backup_name("Ep", 1_000_000 + i));
            std::fs::write(old, "old").unwrap();
        }
        std::fs::write(dir.join("Backups").join("keep me.txt"), "x").unwrap();
        back_up(&file, 3).unwrap();
        let mut left: Vec<String> = std::fs::read_dir(dir.join("Backups"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        assert_eq!(left.len(), 4, "{left:?}");
        assert!(left.contains(&"keep me.txt".to_string()));
        assert!(!left.contains(&backup_name("Ep", 1_000_000)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copies_are_found_again_or_numbered() {
        let dir = std::env::temp_dir().join(format!("af2-copy-{}", new_key()));
        let (a, b, media) = (dir.join("a"), dir.join("b"), dir.join("show").join("Audio"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("voice.wav"), b"first").unwrap();
        std::fs::write(b.join("voice.wav"), b"other").unwrap();
        let c1 = copy_in(&a.join("voice.wav"), &media).unwrap();
        assert_eq!(c1, media.join("voice.wav"));
        // The same file again: the copy made before.
        assert_eq!(copy_in(&a.join("voice.wav"), &media).unwrap(), c1);
        // A different file with the same name: a numbered copy.
        let c2 = copy_in(&b.join("voice.wav"), &media).unwrap();
        assert_eq!(c2, media.join("voice (2).wav"));
        assert_eq!(std::fs::read(&c2).unwrap(), b"other");
        // Already in the session folder: left where it is.
        let to = CopyTo {
            session_dir: dir.join("show"),
            media_dir: media.clone(),
        };
        assert_eq!(to.place(&c2).unwrap(), c2);
        assert!(!std::fs::read_dir(&media)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with(".part")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folders_expand_to_their_audio_in_order() {
        let dir = std::env::temp_dir().join(format!("af2-expand-{}", new_key()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for n in [
            "10-c.wav",
            "2-b.flac",
            "1-a.mp3",
            "notes.txt",
            "sub/x.wav",
            "show.audfrm",
        ] {
            std::fs::write(dir.join(n), b"").unwrap();
        }
        let (audio, sessions) = expand_paths(std::slice::from_ref(&dir));
        let names: Vec<String> = audio.iter().map(|p| file_name(p)).collect();
        assert_eq!(names, ["1-a.mp3", "2-b.flac", "10-c.wav", "x.wav"]);
        assert_eq!(sessions.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn locks_go_stale() {
        assert!(lock_fresh(1000, 1100));
        assert!(!lock_fresh(1000, 1000 + LOCK_STALE_SECS));
    }

    #[test]
    fn numbered_names() {
        assert_eq!(numbered("voice.wav", 2), "voice (2).wav");
        assert_eq!(numbered("README", 3), "README (3)");
    }
}

/// "Untitled Session", or "Untitled Session 2", 3… when that is taken.
pub fn untitled_name(taken: &[String]) -> String {
    let base = "Untitled Session";
    if !taken.iter().any(|t| t == base) {
        return base.into();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|n| !taken.contains(n))
        .unwrap()
}

#[cfg(test)]
mod untitled_tests {
    #[test]
    fn new_sessions_get_free_names() {
        assert_eq!(super::untitled_name(&[]), "Untitled Session");
        let t = vec![
            "Untitled Session".to_string(),
            "Untitled Session 2".to_string(),
        ];
        assert_eq!(super::untitled_name(&t), "Untitled Session 3");
    }
}
