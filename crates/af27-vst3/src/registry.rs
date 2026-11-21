// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The plug-ins found on this computer (`plugins.json` in the settings
//! folder) and the scanner that finds them.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::protocol::{
    cid_other_order, parse_scan_output, ClassInfo, ScanLine, ScanResult, SCAN_PREFIX,
};

/// Bumped when older scans must be redone.
pub const SCHEMA_VERSION: u32 = 1;
/// Bundles scanned side by side.
pub const SCAN_THREADS: usize = 4;
/// One bundle's scan.
pub const SCAN_TIMEOUT: Duration = Duration::from_secs(60);
/// After a scan process ends, how long its pipes may stay open (helper
/// programs it started can hold them).
const PIPE_GRACE: Duration = Duration::from_secs(2);

/// A plug-in and the bundle it is in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub bundle: PathBuf,
    pub info: ClassInfo,
}

impl Found {
    /// The category for menus: the first sub-category after `Fx`, or
    /// "Other".
    pub fn category(&self) -> String {
        let parts: Vec<&str> = self
            .info
            .sub_categories
            .split('|')
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("fx"))
            .collect();
        match parts.first() {
            Some(c) if !c.eq_ignore_ascii_case("instrument") => (*c).to_string(),
            _ => "Other".to_string(),
        }
    }

    /// An instrument's kind from its sub-categories (`Instrument|Synth`
    /// is "Synth", `Instrument|Drum` "Drum"), or None when it says none.
    pub fn instrument_kind(&self) -> Option<String> {
        self.info
            .sub_categories
            .split('|')
            .map(str::trim)
            .find(|s| {
                !s.is_empty()
                    && !s.eq_ignore_ascii_case("instrument")
                    && !s.eq_ignore_ascii_case("fx")
                    && !s.eq_ignore_ascii_case("mono")
                    && !s.eq_ignore_ascii_case("stereo")
            })
            .map(str::to_string)
    }

    pub fn maker(&self) -> String {
        let v = self.info.vendor.trim();
        if v.is_empty() {
            "Unknown maker".into()
        } else {
            v.to_string()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Failed {
    pub bundle: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Registry {
    pub schema_version: u32,
    /// Folders the user added.
    pub folders: Vec<PathBuf>,
    /// Standard folders the user turned off.
    pub skipped: Vec<PathBuf>,
    pub plugins: Vec<Found>,
    pub failed: Vec<Failed>,
    /// Each scanned bundle's modified time (seconds), so a rescan skips
    /// bundles that did not change.
    pub stamps: BTreeMap<PathBuf, u64>,
    /// Makers whose plug-ins each run in a process of their own (lower
    /// case). Every other maker's plug-ins share one process.
    pub separate_makers: Vec<String>,
    /// Plug-ins (class ids) the user turned off: not offered in menus.
    pub disabled: Vec<String>,
    /// Set when the file was from an older scanner (a rescan is due).
    #[serde(skip)]
    pub stale: bool,
}

impl Default for Registry {
    fn default() -> Self {
        Registry {
            schema_version: SCHEMA_VERSION,
            folders: Vec::new(),
            skipped: Vec::new(),
            plugins: Vec::new(),
            failed: Vec::new(),
            stamps: BTreeMap::new(),
            separate_makers: Vec::new(),
            disabled: Vec::new(),
            stale: false,
        }
    }
}

/// The standard VST3 folders of this system.
pub fn standard_folders() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(windows) {
        let common = std::env::var_os("CommonProgramFiles")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\Common Files"));
        out.push(common.join("VST3"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            out.push(
                PathBuf::from(local)
                    .join("Programs")
                    .join("Common")
                    .join("VST3"),
            );
        }
    } else if cfg!(target_os = "macos") {
        out.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        if let Some(h) = std::env::var_os("HOME") {
            out.push(PathBuf::from(h).join("Library/Audio/Plug-Ins/VST3"));
        }
    } else {
        if let Some(h) = std::env::var_os("HOME") {
            out.push(PathBuf::from(h).join(".vst3"));
        }
        out.push(PathBuf::from("/usr/lib/vst3"));
        out.push(PathBuf::from("/usr/local/lib/vst3"));
    }
    out
}

/// Every `.vst3` bundle (folder or file) under `dir`, four levels deep,
/// sorted.
pub fn find_bundles(dir: &Path) -> Vec<PathBuf> {
    fn walk(d: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else {
            return;
        };
        let mut items: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        items.sort();
        for p in items {
            let is_vst3 = p
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("vst3"));
            if is_vst3 {
                out.push(p);
            } else if p.is_dir() && depth > 0 {
                walk(&p, depth - 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, 4, &mut out);
    out
}

fn stamp(p: &Path) -> u64 {
    // A bundle folder's own time rarely changes on update; its library
    // does.
    let lib = crate::host::module::library_path(p);
    std::fs::metadata(&lib)
        .or_else(|_| std::fs::metadata(p))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

impl Registry {
    pub fn load(path: &Path) -> Registry {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Registry::default();
        };
        let mut r: Registry = serde_json::from_str(&text).unwrap_or_default();
        if r.schema_version != SCHEMA_VERSION {
            r.stamps.clear();
            r.stale = true;
            r.schema_version = SCHEMA_VERSION;
        }
        r.dedupe();
        r
    }

    /// Saves atomically (a temporary file renamed over the old one).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    /// The folders a scan looks in.
    pub fn scan_folders(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = standard_folders()
            .into_iter()
            .filter(|f| !self.skipped.contains(f))
            .collect();
        for f in &self.folders {
            if !out.contains(f) {
                out.push(f.clone());
            }
        }
        out
    }

    /// One entry per class id (in either byte order), preferring a copy
    /// read in full. Two copies of one plug-in confused the menus.
    pub fn dedupe(&mut self) {
        let mut out: Vec<Found> = Vec::new();
        for f in std::mem::take(&mut self.plugins) {
            let id = f.info.cid.to_ascii_uppercase();
            match out
                .iter_mut()
                .find(|o| o.info.cid.eq_ignore_ascii_case(&id))
            {
                Some(o) => {
                    if o.info.note.is_some() && f.info.note.is_none() {
                        *o = f;
                    }
                }
                None => out.push(f),
            }
        }
        self.plugins = out;
    }

    /// A plug-in by class id (either byte order, any case).
    pub fn find(&self, cid: &str) -> Option<&Found> {
        let other = cid_other_order(cid).unwrap_or_default();
        self.plugins.iter().find(|f| {
            f.info.cid.eq_ignore_ascii_case(cid) || f.info.cid.eq_ignore_ascii_case(&other)
        })
    }

    pub fn is_disabled(&self, cid: &str) -> bool {
        self.disabled.iter().any(|d| d.eq_ignore_ascii_case(cid))
    }

    pub fn set_disabled(&mut self, cid: &str, off: bool) {
        self.disabled.retain(|d| !d.eq_ignore_ascii_case(cid));
        if off {
            self.disabled.push(cid.to_ascii_uppercase());
        }
    }

    /// Effects (not instruments) the user has not turned off.
    pub fn effects(&self) -> Vec<&Found> {
        let mut v: Vec<&Found> = self
            .plugins
            .iter()
            .filter(|f| !f.info.instrument && !self.is_disabled(&f.info.cid))
            .collect();
        v.sort_by_key(|f| {
            (
                f.category().to_lowercase(),
                f.maker().to_lowercase(),
                f.info.name.to_lowercase(),
            )
        });
        v
    }

    /// Instruments the user has not turned off, by maker, then name.
    pub fn instruments(&self) -> Vec<&Found> {
        let mut v: Vec<&Found> = self
            .plugins
            .iter()
            .filter(|f| f.info.instrument && !self.is_disabled(&f.info.cid))
            .collect();
        v.sort_by_key(|f| (f.maker().to_lowercase(), f.info.name.to_lowercase()));
        v
    }

    /// Every maker with plug-ins here, sorted.
    pub fn makers(&self) -> Vec<String> {
        let mut m: Vec<String> = self.plugins.iter().map(Found::maker).collect();
        m.sort_by_key(|s| s.to_lowercase());
        m.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        m
    }

    pub fn runs_separately(&self, maker: &str) -> bool {
        let m = maker.trim().to_lowercase();
        self.separate_makers.contains(&m)
    }

    pub fn set_separate(&mut self, maker: &str, separate: bool) {
        let m = maker.trim().to_lowercase();
        self.separate_makers.retain(|s| *s != m);
        if separate {
            self.separate_makers.push(m);
        }
    }

    /// Takes one bundle's scan into the registry.
    pub fn take_scan(&mut self, bundle: &Path, r: ScanResult) {
        self.plugins.retain(|f| f.bundle != bundle);
        self.failed.retain(|f| f.bundle != bundle);
        let complete = r.error.is_none() && r.classes.iter().all(|c| c.note.is_none());
        if let Some(error) = r.error {
            self.failed.push(Failed {
                bundle: bundle.to_path_buf(),
                error,
            });
        }
        for c in r.classes {
            self.plugins.push(Found {
                bundle: bundle.to_path_buf(),
                info: c,
            });
        }
        // Failed and partly read bundles are tried again next time.
        if complete {
            self.stamps.insert(bundle.to_path_buf(), stamp(bundle));
        } else {
            self.stamps.remove(bundle);
        }
        self.dedupe();
    }

    /// The bundles a scan should read: all, or only new and changed ones.
    pub fn due(&self, all: bool) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for folder in self.scan_folders() {
            for b in find_bundles(&folder) {
                if out.contains(&b) {
                    continue;
                }
                let fresh = self.stamps.get(&b).is_some_and(|s| *s == stamp(&b));
                if all || !fresh {
                    out.push(b);
                }
            }
        }
        out
    }

    /// Forgets plug-ins whose bundles are gone.
    pub fn forget_missing(&mut self) {
        self.plugins.retain(|f| f.bundle.exists());
        self.failed.retain(|f| f.bundle.exists());
        self.stamps.retain(|b, _| b.exists());
    }
}

/// Progress of a running scan, shared with the interface.
#[derive(Default)]
pub struct ScanProgress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
    pub current: Mutex<String>,
    pub stop: AtomicBool,
    pub finished: AtomicBool,
}

/// Reads a pipe on a thread while the process runs (a pipe holds only a
/// few KB; a scan of a plug-in with many controls prints far more).
fn drain<R: Read + Send + 'static>(
    r: R,
    done_flag: Option<Arc<AtomicBool>>,
) -> (Arc<Mutex<String>>, std::sync::mpsc::Receiver<()>) {
    let text = Arc::new(Mutex::new(String::new()));
    let (tx, rx) = std::sync::mpsc::channel();
    let t = text.clone();
    std::thread::spawn(move || {
        let mut br = BufReader::new(r);
        let mut line = Vec::new();
        while let Ok(n) = br.read_until(b'\n', &mut line) {
            if n == 0 {
                break;
            }
            let s = String::from_utf8_lossy(&line).into_owned();
            if let Some(f) = &done_flag {
                if s.trim_end().strip_prefix(SCAN_PREFIX).is_some_and(|j| {
                    matches!(serde_json::from_str::<ScanLine>(j), Ok(ScanLine::Done))
                }) {
                    f.store(true, Ordering::SeqCst);
                }
            }
            t.lock().unwrap().push_str(&s);
            line.clear();
        }
        let _ = tx.send(());
    });
    (text, rx)
}

/// Scans one bundle in a scan process (`exe --plugin-scan <bundle>`).
pub fn scan_bundle(exe: &Path, bundle: &Path, timeout: Duration, stop: &AtomicBool) -> ScanResult {
    let mut cmd = Command::new(exe);
    cmd.arg("--plugin-scan")
        .arg(bundle)
        .arg("--parent")
        .arg(std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => {
            crate::bind_to_app(&c);
            c
        }
        Err(e) => {
            return ScanResult {
                error: Some(format!("The scanner could not start: {e}")),
                ..Default::default()
            }
        }
    };
    let said_done = Arc::new(AtomicBool::new(false));
    let (out, out_end) = drain(child.stdout.take().unwrap(), Some(said_done.clone()));
    let (err, err_end) = drain(child.stderr.take().unwrap(), None);
    let start = Instant::now();
    let mut exit = None;
    let mut stopped = false;
    loop {
        match child.try_wait() {
            Ok(Some(s)) => {
                exit = s.code();
                break;
            }
            Ok(None) => {}
            Err(_) => break,
        }
        // A scan that said "done" is finished, even if it hangs on exit.
        if said_done.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
            exit = Some(0);
            break;
        }
        if start.elapsed() > timeout || stop.load(Ordering::SeqCst) {
            stopped = true;
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    // Helper programs may keep the pipes open: wait a little, no more.
    let _ = out_end.recv_timeout(PIPE_GRACE);
    let _ = err_end.recv_timeout(Duration::from_millis(200));
    let stdout = out.lock().unwrap().clone();
    let stderr = err.lock().unwrap().clone();
    let mut r = parse_scan_output(&stdout, &stderr, exit);
    if stopped && !r.done {
        let why = if stop.load(Ordering::SeqCst) {
            "The scan was stopped before it finished."
        } else {
            "It took too long to read (over a minute), so it was skipped."
        };
        if r.classes.is_empty() {
            r.error = Some(why.into());
        }
    }
    r
}

/// Scans `bundles` (four at a time) into `reg`, reporting to `progress`.
pub fn scan_into(
    reg: &Mutex<Registry>,
    exe: &Path,
    bundles: Vec<PathBuf>,
    progress: &ScanProgress,
) {
    progress.total.store(bundles.len(), Ordering::SeqCst);
    progress.done.store(0, Ordering::SeqCst);
    let queue = Mutex::new(bundles.into_iter());
    std::thread::scope(|s| {
        for _ in 0..SCAN_THREADS {
            s.spawn(|| loop {
                if progress.stop.load(Ordering::SeqCst) {
                    return;
                }
                let Some(b) = queue.lock().unwrap().next() else {
                    return;
                };
                *progress.current.lock().unwrap() = b
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let r = scan_bundle(exe, &b, SCAN_TIMEOUT, &progress.stop);
                // A stopped bundle stays unstamped, to be read next time.
                if !(progress.stop.load(Ordering::SeqCst) && !r.done) {
                    reg.lock().unwrap().take_scan(&b, r);
                }
                progress.done.fetch_add(1, Ordering::SeqCst);
            });
        }
    });
    progress.finished.store(true, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(cid: &str, note: Option<&str>) -> Found {
        Found {
            bundle: PathBuf::from(format!("{cid}.vst3")),
            info: ClassInfo {
                cid: cid.into(),
                name: cid.into(),
                sub_categories: "Fx|Dynamics".into(),
                note: note.map(str::to_string),
                ..Default::default()
            },
        }
    }

    #[test]
    fn duplicate_ids_are_listed_once_preferring_a_full_read() {
        let mut r = Registry::default();
        let id = "0123456789ABCDEF0123456789ABCDEF";
        r.plugins = vec![
            found(id, Some("partly")),
            found(&id.to_lowercase(), None),
            found("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF", None),
        ];
        r.dedupe();
        assert_eq!(r.plugins.len(), 2);
        assert!(r.plugins[0].info.note.is_none());
        // Found by either byte order.
        let other = cid_other_order(id).unwrap();
        assert!(r.find(&other).is_some());
        assert!(r.find(id).is_some());
    }

    #[test]
    fn categories_makers_and_switches() {
        let mut f = found("A", None);
        assert_eq!(f.category(), "Dynamics");
        f.info.sub_categories = "Fx".into();
        assert_eq!(f.category(), "Other");
        assert_eq!(f.maker(), "Unknown maker");
        let mut r = Registry::default();
        r.set_separate(" Waves ", true);
        assert!(r.runs_separately("WAVES"));
        r.set_separate("waves", false);
        assert!(!r.runs_separately("Waves"));
        r.plugins.push(found("B", None));
        r.set_disabled("b", true);
        assert!(r.effects().is_empty());
        r.set_disabled("B", false);
        assert_eq!(r.effects().len(), 1);
    }

    #[test]
    fn saves_atomically_and_old_files_ask_for_a_rescan() {
        let dir = std::env::temp_dir().join(format!("af2-reg-{}", std::process::id()));
        let p = dir.join("plugins.json");
        let mut r = Registry::default();
        r.plugins.push(found("A", None));
        r.stamps.insert("A.vst3".into(), 5);
        r.save(&p).unwrap();
        let back = Registry::load(&p);
        assert_eq!(back.plugins, r.plugins);
        assert!(!back.stale);
        std::fs::write(&p, r#"{"schema_version":0,"stamps":{"x":1}}"#).unwrap();
        let old = Registry::load(&p);
        assert!(old.stale && old.stamps.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
