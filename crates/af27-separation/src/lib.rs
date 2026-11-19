// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! AI stem separation.
//!
//! The neural network runs in the separation engine
//! (`audiform-separate`), a separate program shared with Audiform. This
//! crate speaks its documented interface:
//!
//! * `audiform-separate --version` prints a version like `0.2.0`.
//! * `audiform-separate <job.json>` runs a job. The job names the model
//!   checkpoint, its architecture spec, an input WAV at the model's rate,
//!   the output WAVs (each a list of stem indices, optionally `rest`: the
//!   input minus those stems) and `device` (`gpu` or `cpu`).
//! * It prints `progress <0..1>`, then `done`, or `error <message>`
//!   (`error gpu <message>` when the graphics card could not be used).
//!
//! Where things live: Audiform27 keeps its own folder, and also finds the
//! engine and models an existing Audiform install already downloaded, so
//! nothing is fetched twice. `AUDIFORM_SEPARATE` points at a local engine
//! build for development.
//!
//! The model list grouping and score text, the engine checksum check and
//! update check, and removal are ported from Audiform's Audio Separation
//! (`src-tauri/src/separation/`, `src/app/separation.ts`; the project
//! lead's own files, see docs/clean-room.md).

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use af27_core::AudioBuffer;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const MIN_ENGINE: (u32, u32, u32) = (0, 2, 0);
pub const ENGINE_TAG: &str = "engine-v0.2.0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Stem {
    pub name: String,
    pub sdr: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub category: String,
    pub sdr: Option<f32>,
    #[serde(default)]
    pub sdr_note: String,
    pub target_name: String,
    pub rest_name: String,
    #[serde(default)]
    pub stems: Vec<Stem>,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub page: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub licence: String,
    pub spec: Value,
}

impl Model {
    /// Names of the results, in output order.
    pub fn output_names(&self) -> Vec<String> {
        if self.stems.is_empty() {
            vec![self.target_name.clone(), self.rest_name.clone()]
        } else {
            self.stems.iter().map(|s| s.name.clone()).collect()
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.spec["sample_rate"].as_u64().unwrap_or(44_100) as u32
    }
}

/// The built-in model list plus any user additions in
/// `<data>/separation/models.json` (same format; entries with an
/// existing id replace the built-in one). That file is how models an
/// older or newer Audiform downloaded can be described to Audiform27.
pub fn catalog(dirs: &Dirs) -> Vec<Model> {
    let mut list: Vec<Model> =
        serde_json::from_str(include_str!("models.json")).expect("built-in catalog parses");
    for extra in dirs.search_roots().iter().map(|r| r.join("models.json")) {
        if let Ok(text) = std::fs::read_to_string(&extra) {
            if let Ok(more) = serde_json::from_str::<Vec<Model>>(&text) {
                for m in more {
                    match list.iter_mut().find(|x| x.id == m.id) {
                        Some(slot) => *slot = m,
                        None => list.push(m),
                    }
                }
            }
        }
    }
    list
}

/// Folders the separation files may live in.
#[derive(Debug, Clone)]
pub struct Dirs {
    /// Audiform27's own: `<data>/Audiform27/separation`.
    pub own: PathBuf,
    /// Audiform's, when installed: `<data>/io.github.nwh212.audiform/separation`.
    pub legacy: Option<PathBuf>,
}

/// The per-user application data folder for this platform.
pub fn data_root() -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    }
    .unwrap_or_else(std::env::temp_dir)
}

impl Dirs {
    pub fn detect() -> Dirs {
        let root = data_root();
        let legacy = root.join("io.github.nwh212.audiform").join("separation");
        Dirs {
            own: root.join("Audiform27").join("separation"),
            legacy: legacy.is_dir().then_some(legacy),
        }
    }

    pub fn search_roots(&self) -> Vec<PathBuf> {
        let mut v = vec![self.own.clone()];
        v.extend(self.legacy.clone());
        v
    }

    fn engine_file() -> &'static str {
        if cfg!(windows) {
            "audiform-separate.exe"
        } else {
            "audiform-separate"
        }
    }

    /// The engine to run: the env override, ours, then Audiform's.
    pub fn engine(&self) -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("AUDIFORM_SEPARATE") {
            return Some(PathBuf::from(p));
        }
        self.search_roots()
            .into_iter()
            .map(|r| r.join("engine").join(Self::engine_file()))
            .find(|p| p.is_file())
    }

    pub fn own_engine_path(&self) -> PathBuf {
        self.own.join("engine").join(Self::engine_file())
    }

    /// A downloaded checkpoint for `m`, wherever it is.
    pub fn model(&self, m: &Model) -> Option<PathBuf> {
        self.search_roots()
            .into_iter()
            .map(|r| r.join("models").join(format!("{}.ckpt", m.id)))
            .find(|p| p.is_file())
    }

    pub fn own_model_path(&self, m: &Model) -> PathBuf {
        self.own.join("models").join(format!("{}.ckpt", m.id))
    }
}

/// Parses `0.2.0`-style versions and checks them against [`MIN_ENGINE`].
pub fn engine_is_new_enough(version: &str) -> bool {
    let mut parts = version.trim().split('.').map(|p| p.trim().parse::<u32>());
    match (parts.next(), parts.next()) {
        (Some(Ok(a)), Some(Ok(b))) => {
            let c = parts.next().and_then(Result::ok).unwrap_or(0);
            (a, b, c) >= MIN_ENGINE
        }
        _ => false,
    }
}

pub fn engine_version(engine: &Path) -> Result<String, String> {
    let mut cmd = Command::new(engine);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    hide_window(&mut cmd);
    let out = cmd
        .output()
        .map_err(|e| format!("Could not start the separation engine: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn engine_asset() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("audiform-separate-windows-x64.zip")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("audiform-separate-linux-x64.zip")
    } else {
        None
    }
}

pub fn engine_url(asset: &str) -> String {
    format!("https://github.com/nwh212/Audiform/releases/download/{ENGINE_TAG}/{asset}")
}

/// The job file for one run. `outputs` are the raw result files in
/// [`Model::output_names`] order.
pub fn job(
    m: &Model,
    checkpoint: &Path,
    input: &Path,
    outputs: &[PathBuf],
    gpu: bool,
    overlap: Option<u32>,
) -> Value {
    let mut spec = m.spec.clone();
    if let Some(o) = overlap {
        spec["num_overlap"] = json!(o.clamp(1, 8));
    }
    let files: Vec<Value> = if m.stems.is_empty() {
        vec![
            json!({ "path": outputs[0], "stems": [0] }),
            json!({ "path": outputs[1], "stems": [0], "rest": true }),
        ]
    } else {
        outputs
            .iter()
            .enumerate()
            .map(|(k, p)| json!({ "path": p, "stems": [k] }))
            .collect()
    };
    json!({
        "spec": spec,
        "checkpoint": checkpoint,
        "input": input,
        "outputs": files,
        "device": if gpu { "gpu" } else { "cpu" },
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Progress(f32),
    Done,
    Error { gpu: bool, message: String },
    Other(String),
}

pub fn parse_line(line: &str) -> Line {
    let line = line.trim();
    if let Some(p) = line.strip_prefix("progress ") {
        if let Ok(v) = p.trim().parse() {
            return Line::Progress(v);
        }
    }
    if line == "done" {
        return Line::Done;
    }
    if let Some(rest) = line.strip_prefix("error ") {
        return match rest.strip_prefix("gpu ") {
            Some(m) => Line::Error {
                gpu: true,
                message: m.into(),
            },
            None => Line::Error {
                gpu: false,
                message: rest.into(),
            },
        };
    }
    Line::Other(line.into())
}

/// Live state of a job, shared with the UI.
#[derive(Debug, Default)]
pub struct Progress {
    pub fraction: f32,
    /// The size is not known yet (the engine download): show a busy bar.
    pub indeterminate: bool,
    pub status: String,
    pub done: bool,
    pub error: Option<String>,
    /// The engine said `error gpu`: the graphics card could not be used,
    /// so the caller can offer the processor instead.
    pub gpu_error: bool,
    /// A yes or no answer from a check (an engine update is published, a
    /// model file no longer matches its pinned checksum).
    pub flag: bool,
    /// (stem name, file) once finished.
    pub outputs: Vec<(String, PathBuf)>,
}

pub type Shared = Arc<Mutex<Progress>>;

fn set(p: &Shared, f: impl FnOnce(&mut Progress)) {
    if let Ok(mut g) = p.lock() {
        f(&mut g);
    }
}

fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let _ = cmd;
}

/// Runs a whole separation on a background thread: resample `source` to
/// the model's rate, run the engine, bring every result back to the
/// source's own rate and exact length, and write them beside
/// `out_dir/<name> (<stem>).wav`.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    dirs: Dirs,
    model: Model,
    source: Arc<AudioBuffer>,
    source_name: String,
    out_dir: PathBuf,
    gpu: bool,
    overlap: Option<u32>,
    cancel: Arc<AtomicBool>,
) -> Shared {
    let progress: Shared = Arc::new(Mutex::new(Progress {
        status: "Preparing".into(),
        ..Default::default()
    }));
    let p = progress.clone();
    std::thread::spawn(move || {
        let r = run(
            &dirs,
            &model,
            &source,
            &source_name,
            &out_dir,
            gpu,
            overlap,
            &cancel,
            &p,
        );
        set(&p, |g| match r {
            Ok(outs) => {
                g.outputs = outs;
                g.fraction = 1.0;
                g.status = "Finished".into();
                g.done = true;
            }
            Err(e) => {
                g.error = Some(e);
                g.done = true;
            }
        });
    });
    progress
}

#[allow(clippy::too_many_arguments)]
fn run(
    dirs: &Dirs,
    m: &Model,
    source: &AudioBuffer,
    name: &str,
    out_dir: &Path,
    gpu: bool,
    overlap: Option<u32>,
    cancel: &AtomicBool,
    p: &Shared,
) -> Result<Vec<(String, PathBuf)>, String> {
    let engine = dirs
        .engine()
        .ok_or("The separation engine is not installed. Use Download Engine first.")?;
    let ver = engine_version(&engine)?;
    if !engine_is_new_enough(&ver) {
        return Err(format!(
            "The installed engine ({ver}) is too old; version {}.{}.{} or newer is needed.",
            MIN_ENGINE.0, MIN_ENGINE.1, MIN_ENGINE.2
        ));
    }
    let ckpt = dirs
        .model(m)
        .ok_or_else(|| format!("{} is not downloaded yet.", m.name))?;
    let work = std::env::temp_dir().join(format!("af27-separate-{}", std::process::id()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    // Stereo models take mono as is; wider files fold to stereo.
    let channels = source.num_channels().clamp(1, 2);
    let model_in = af27_dsp::resample::resample(
        &af27_dsp::resample::remix(source, channels),
        m.sample_rate(),
    );
    let input = work.join("input.wav");
    af27_io::write_wav(&input, &model_in, af27_io::WavFormat::Float32)?;
    drop(model_in);
    let names = m.output_names();
    let raw: Vec<PathBuf> = (0..names.len())
        .map(|k| work.join(format!("out{k}.wav")))
        .collect();
    let job_path = work.join("job.json");
    std::fs::write(
        &job_path,
        serde_json::to_vec(&job(m, &ckpt, &input, &raw, gpu, overlap)).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    set(p, |g| g.status = "Separating".into());

    let mut cmd = Command::new(&engine);
    cmd.arg(&job_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_window(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Could not start the separation engine: {e}"))?;
    let stdout = child.stdout.take().ok_or("The engine gave no output.")?;
    let stderr_text = Arc::new(Mutex::new(String::new()));
    if let Some(mut err) = child.stderr.take() {
        let t = stderr_text.clone();
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.read_to_string(&mut s);
            if let Ok(mut g) = t.lock() {
                *g = s;
            }
        });
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut finished = false;
    let mut failure = None;
    // Once the engine has said it is done (or failed), it gets a few
    // seconds to exit on its own, then it is stopped: it never lingers.
    let mut ended_at: Option<std::time::Instant> = None;
    loop {
        if ended_at.is_some_and(|t| t.elapsed() > std::time::Duration::from_secs(5)) {
            let _ = child.kill();
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&work);
            return Err("Separation cancelled.".into());
        }
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(line) => match parse_line(&line) {
                Line::Progress(f) => set(p, |g| g.fraction = f.clamp(0.0, 1.0) * 0.95),
                Line::Done => {
                    finished = true;
                    ended_at.get_or_insert_with(std::time::Instant::now);
                }
                Line::Error { gpu: true, message } => {
                    set(p, |g| g.gpu_error = true);
                    failure = Some(format!("The graphics card could not be used ({message})."));
                    ended_at.get_or_insert_with(std::time::Instant::now);
                }
                Line::Error { message, .. } => {
                    failure = Some(message);
                    ended_at.get_or_insert_with(std::time::Instant::now);
                }
                Line::Other(_) => {}
            },
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if let Some(f) = failure {
        let _ = std::fs::remove_dir_all(&work);
        return Err(f);
    }
    // A clean exit after `done`, or one stopped by the grace period above.
    if !finished || (!status.success() && ended_at.is_none()) {
        let _ = std::fs::remove_dir_all(&work);
        let said = stderr_text
            .lock()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let tail: String = said.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
        return Err(if tail.is_empty() {
            "The separation engine stopped without finishing.".into()
        } else {
            format!("The separation engine stopped: {tail}")
        });
    }
    set(p, |g| g.status = "Writing results".into());
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let mut outs = Vec::new();
    for (stem, file) in names.iter().zip(&raw) {
        let buf = af27_io::decode_file(file)?;
        let mut back = af27_dsp::resample::resample(&buf, source.rate);
        for c in &mut back.channels {
            c.resize(source.frames(), 0.0);
        }
        let dest = unique(out_dir, &format!("{name} ({stem})"));
        af27_io::write_wav(&dest, &back, af27_io::WavFormat::Float32)?;
        outs.push((stem.clone(), dest));
    }
    let _ = std::fs::remove_dir_all(&work);
    Ok(outs)
}

fn unique(dir: &Path, stem: &str) -> PathBuf {
    let clean: String = stem
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect();
    let mut p = dir.join(format!("{clean}.wav"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{clean} {n}.wav"));
        n += 1;
    }
    p
}

/// Downloads `url` to `dest` with the system `curl` (HTTPS only, from
/// github.com or huggingface.co), checks the SHA-256 when one is given,
/// and moves the file into place only when it is complete and correct.
pub fn download(
    url: &str,
    dest: &Path,
    sha256: &str,
    cancel: &AtomicBool,
    p: &Shared,
    total: u64,
) -> Result<(), String> {
    if !allowed(url) {
        return Err("That download is not from an allowed site.".into());
    }
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let part = dest.with_extension("part");
    let _ = std::fs::remove_file(&part);
    let curl = if cfg!(windows) {
        std::env::var_os("SystemRoot")
            .map(|r| PathBuf::from(r).join("System32").join("curl.exe"))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from("curl"))
    } else {
        PathBuf::from("curl")
    };
    let mut cmd = Command::new(curl);
    cmd.args([
        "--location",
        "--fail",
        "--silent",
        "--show-error",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--output",
    ])
    .arg(&part)
    .arg(url)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::piped());
    hide_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| {
        format!("Could not start the download ({e}). The system curl program is needed.")
    })?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&part);
            return Err("Download cancelled.".into());
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            if !status.success() {
                let mut err = String::new();
                if let Some(mut s) = child.stderr.take() {
                    let _ = s.read_to_string(&mut err);
                }
                let _ = std::fs::remove_file(&part);
                return Err(if err.contains("404") {
                    "The file is no longer at its download address (404).".into()
                } else {
                    format!("Download failed: {}", err.trim())
                });
            }
            break;
        }
        let got = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        set(p, |g| {
            g.indeterminate = total == 0;
            g.fraction = if total > 0 {
                (got as f32 / total as f32).min(1.0)
            } else {
                0.0
            };
            g.status = if total > 0 {
                format!("{} of {}", size_text(got), size_text(total))
            } else {
                size_text(got)
            };
        });
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    if !sha256.is_empty() {
        set(p, |g| g.status = "Checking the download".into());
        let actual = sha256_file(&part)?;
        if !actual.eq_ignore_ascii_case(sha256) {
            let _ = std::fs::remove_file(&part);
            return Err(
                "The download did not match its expected checksum, so it was discarded.".into(),
            );
        }
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Where downloads may come from: Audiform's own releases (the engine)
/// and the model authors' own pages.
pub fn allowed(url: &str) -> bool {
    const HOSTS: [&str; 2] = ["https://github.com/", "https://huggingface.co/"];
    HOSTS.iter().any(|h| url.starts_with(h))
}

/// The checksum in a published `.sha256` file (`<hex>` or `<hex>  name`),
/// lowercased. None unless it is 64 hex digits.
pub fn parse_sha256(text: &str) -> Option<String> {
    let first = text.split_whitespace().next()?;
    (first.len() == 64 && first.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| first.to_ascii_lowercase())
}

/// Whether the published engine differs from the installed one, from the
/// installed record (`engine.sha256`, written at install) and the
/// published checksum file. An engine with no record counts as out of
/// date, as in Audiform.
pub fn engine_update_available(installed_record: &str, published: &str) -> bool {
    match parse_sha256(published) {
        Some(latest) => parse_sha256(installed_record).as_deref() != Some(latest.as_str()),
        None => false,
    }
}

/// Human file sizes: "102 MB", "1.2 GB".
pub fn size_text(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1000.0)
    } else if mb >= 1.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{} KB", (bytes / 1000).max(1))
    }
}

/// The tabs of the model list, in order. Every catalog category is one.
pub const TABS: [&str; 3] = ["Vocals", "Instruments", "Denoise"];

/// What a model gives, for the headings in the list.
pub fn mode_label(m: &Model) -> &'static str {
    match m.category.as_str() {
        "Denoise" => "Clean and removed noise",
        _ if m.stems.len() > 2 => "Six stems: vocals, drums, bass, guitar, piano, other",
        "Instruments" => "Single instrument and the rest",
        _ => "Vocals and instrumental",
    }
}

/// Models grouped by mode in first-seen order, best published score first
/// within each, unscored last (as Audiform's list).
pub fn by_mode<'a>(
    models: impl IntoIterator<Item = &'a Model>,
) -> Vec<(&'static str, Vec<&'a Model>)> {
    let mut groups: Vec<(&'static str, Vec<&'a Model>)> = Vec::new();
    for m in models {
        let label = mode_label(m);
        match groups.iter_mut().find(|(l, _)| *l == label) {
            Some((_, v)) => v.push(m),
            None => groups.push((label, vec![m])),
        }
    }
    for (_, v) in &mut groups {
        v.sort_by(|a, b| {
            b.sdr
                .unwrap_or(-1.0)
                .partial_cmp(&a.sdr.unwrap_or(-1.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    groups
}

/// "SDR 11.00 dB", or "SDR not published". A several-stem model lists
/// each stem's score.
pub fn sdr_text(m: &Model) -> String {
    if !m.stems.is_empty() {
        let parts: Vec<String> = m
            .stems
            .iter()
            .map(|s| format!("{} {:.1}", s.name.to_lowercase(), s.sdr))
            .collect();
        return format!("SDR {} dB", parts.join(", "));
    }
    match m.sdr {
        Some(v) => format!("SDR {v:.2} dB"),
        None => "SDR not published".into(),
    }
}

impl Dirs {
    /// Every downloaded copy of `m` (Audiform27's own and Audiform's).
    pub fn model_copies(&self, m: &Model) -> Vec<PathBuf> {
        self.search_roots()
            .into_iter()
            .map(|r| r.join("models").join(format!("{}.ckpt", m.id)))
            .filter(|p| p.is_file())
            .collect()
    }

    /// Whether `path` is in Audiform's folder (so removing it affects
    /// Audiform too).
    pub fn is_shared(&self, path: &Path) -> bool {
        self.legacy.as_ref().is_some_and(|l| path.starts_with(l))
    }

    /// The installed record beside an engine.
    pub fn engine_record(engine: &Path) -> PathBuf {
        engine.with_file_name("engine.sha256")
    }
}

/// Removes every downloaded copy of a model.
pub fn remove_model(dirs: &Dirs, m: &Model) -> Result<(), String> {
    for p in dirs.model_copies(m) {
        std::fs::remove_file(&p).map_err(|e| format!("Could not remove {}: {e}", p.display()))?;
    }
    Ok(())
}

/// Removes the installed engine (every copy found, not a development
/// build named by `AUDIFORM_SEPARATE`) and its install records.
pub fn remove_engine(dirs: &Dirs) -> Result<(), String> {
    for r in dirs.search_roots() {
        let exe = r.join("engine").join(Dirs::engine_file());
        if exe.is_file() {
            std::fs::remove_file(&exe)
                .map_err(|e| format!("Could not remove {}: {e}", exe.display()))?;
        }
        let _ = std::fs::remove_file(Dirs::engine_record(&exe));
    }
    Ok(())
}

/// Asks whether a newer engine is published: fetches only the small
/// checksum file. Sets `flag` on the progress when one is.
pub fn check_engine_update(dirs: &Dirs, cancel: &AtomicBool, p: &Shared) -> Result<bool, String> {
    let asset = engine_asset().ok_or("Audio Separation is not available for this computer yet.")?;
    let engine = dirs
        .engine()
        .ok_or("The separation engine is not installed.")?;
    let tmp = dirs.own.join("engine").join("latest.sha256");
    download(
        &format!("{}.sha256", engine_url(asset)),
        &tmp,
        "",
        cancel,
        p,
        0,
    )?;
    let latest = std::fs::read_to_string(&tmp).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&tmp);
    if parse_sha256(&latest).is_none() {
        return Err("The published engine checksum could not be read.".into());
    }
    let mine = std::fs::read_to_string(Dirs::engine_record(&engine)).unwrap_or_default();
    let newer = engine_update_available(&mine, &latest);
    set(p, |g| g.flag = newer);
    Ok(newer)
}

/// Checks a downloaded model against its pinned checksum. Sets `flag` when
/// it no longer matches (the catalog moved to a new file, or it was
/// damaged), so the panel offers Update.
pub fn check_model(dirs: &Dirs, m: &Model, p: &Shared) -> Result<bool, String> {
    let path = dirs
        .model(m)
        .ok_or_else(|| format!("{} is not downloaded.", m.name))?;
    if m.sha256.is_empty() {
        return Ok(false);
    }
    set(p, |g| {
        g.indeterminate = true;
        g.status = "Checking the file".into();
    });
    let stale = !sha256_file(&path)?.eq_ignore_ascii_case(&m.sha256);
    set(p, |g| g.flag = stale);
    Ok(stale)
}

/// Downloads and unpacks the engine into Audiform27's own folder, using
/// the system `tar` (bsdtar ships with Windows 10+ and reads zip). The zip
/// is checked against the `.sha256` file published beside it, and that
/// checksum is kept as `engine.sha256` so Check for Update can compare.
pub fn install_engine(dirs: &Dirs, cancel: &AtomicBool, p: &Shared) -> Result<(), String> {
    let asset = engine_asset().ok_or("Audio Separation is not available for this computer yet.")?;
    let dir = dirs.own.join("engine");
    let zip = dirs.own.join(asset);
    let sum_path = dir.join(format!("{asset}.sha256"));
    download(
        &format!("{}.sha256", engine_url(asset)),
        &sum_path,
        "",
        cancel,
        p,
        0,
    )?;
    let sum_text = std::fs::read_to_string(&sum_path).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&sum_path);
    let expected =
        parse_sha256(&sum_text).ok_or("The published engine checksum could not be read.")?;
    download(&engine_url(asset), &zip, &expected, cancel, p, 0)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    set(p, |g| g.status = "Unpacking the engine".into());
    let mut cmd = Command::new("tar");
    cmd.arg("-xf")
        .arg(&zip)
        .arg("-C")
        .arg(&dir)
        .stdin(Stdio::null());
    hide_window(&mut cmd);
    let ok = cmd
        .status()
        .map_err(|e| format!("Could not unpack the engine: {e}"))?;
    let _ = std::fs::remove_file(&zip);
    if !ok.success() || !dirs.own_engine_path().is_file() {
        return Err("The engine download could not be unpacked.".into());
    }
    let _ = std::fs::write(Dirs::engine_record(&dirs.own_engine_path()), &expected);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parses_and_ids_are_unique() {
        let dirs = Dirs {
            own: std::env::temp_dir().join("af27-none"),
            legacy: None,
        };
        let c = catalog(&dirs);
        assert!(c.len() >= 10);
        let mut ids: Vec<_> = c.iter().map(|m| m.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), c.len());
        let six = c.iter().find(|m| m.id == "bs-roformer-sw").unwrap();
        assert_eq!(six.output_names().len(), 6);
        assert_eq!(six.sample_rate(), 44_100);
    }

    #[test]
    fn job_shape_matches_the_engine_interface() {
        let dirs = Dirs {
            own: std::env::temp_dir().join("af27-none"),
            legacy: None,
        };
        let c = catalog(&dirs);
        let kim = c.iter().find(|m| m.id == "mel-roformer-kim").unwrap();
        let outs = vec![PathBuf::from("a.wav"), PathBuf::from("b.wav")];
        let j = job(
            kim,
            Path::new("k.ckpt"),
            Path::new("in.wav"),
            &outs,
            true,
            Some(12),
        );
        assert_eq!(j["device"], "gpu");
        assert_eq!(j["spec"]["num_overlap"], 8);
        assert_eq!(j["outputs"][1]["rest"], true);
        assert_eq!(j["outputs"][0]["stems"][0], 0);
    }

    #[test]
    fn engine_lines_and_versions() {
        assert_eq!(parse_line("progress 0.5"), Line::Progress(0.5));
        assert_eq!(parse_line("done\r\n"), Line::Done);
        assert_eq!(
            parse_line("error gpu no CUDA"),
            Line::Error {
                gpu: true,
                message: "no CUDA".into()
            }
        );
        assert!(engine_is_new_enough("0.2.0\n"));
        assert!(engine_is_new_enough("1.0"));
        assert!(!engine_is_new_enough("0.1.9"));
        assert!(!engine_is_new_enough("junk"));
    }

    fn test_catalog() -> Vec<Model> {
        catalog(&Dirs {
            own: std::env::temp_dir().join("af27-none"),
            legacy: None,
        })
    }

    #[test]
    fn every_model_has_a_pinned_download_on_an_allowed_host() {
        for m in test_catalog() {
            assert!(allowed(&m.url), "{}", m.id);
            assert!(parse_sha256(&m.sha256).is_some(), "{}", m.id);
            assert!(m.bytes > 0 && m.spec["arch"].is_string(), "{}", m.id);
            assert!(TABS.contains(&m.category.as_str()), "{}", m.id);
        }
        assert!(allowed("https://huggingface.co/a/b/resolve/main/m.ckpt"));
        assert!(!allowed("http://github.com/x"));
        assert!(!allowed("https://github.com.evil.example/x"));
    }

    #[test]
    fn tabs_hold_the_three_modes() {
        let c = test_catalog();
        let tab = |t: &str| by_mode(c.iter().filter(|m| m.category == t));
        let vocals = tab("Vocals");
        assert_eq!(vocals.len(), 1);
        assert_eq!(vocals[0].0, "Vocals and instrumental");
        // Best score first.
        assert_eq!(vocals[0].1[0].id, "mel-roformer-deux");
        let inst = tab("Instruments");
        let labels: Vec<_> = inst.iter().map(|g| g.0).collect();
        assert!(labels[0].starts_with("Six stems"));
        assert!(labels[1].starts_with("Single instrument"));
        let singles: Vec<_> = inst[1].1.iter().map(|m| m.target_name.as_str()).collect();
        for want in ["Bass", "Drums", "Guitar", "Piano and keys"] {
            assert!(singles.contains(&want), "{want}");
        }
        let six = &inst[0].1[0];
        let mut names = six.output_names();
        names.sort();
        assert_eq!(
            names,
            ["Bass", "Drums", "Guitar", "Other", "Piano", "Vocals"]
        );
    }

    #[test]
    fn scores_read_as_published() {
        let c = test_catalog();
        let get = |id: &str| c.iter().find(|m| m.id == id).unwrap();
        assert_eq!(sdr_text(get("bs-polarformer")), "SDR 11.00 dB");
        assert_eq!(sdr_text(get("xlance-bass")), "SDR not published");
        assert!(sdr_text(get("bs-roformer-sw")).starts_with("SDR bass 14.6, drums 14.1"));
        assert_eq!(size_text(102_410_301), "102 MB");
        assert_eq!(size_text(913_106_900), "913 MB");
        assert_eq!(size_text(1_500_000_000), "1.5 GB");
    }

    #[test]
    fn checksum_files_and_engine_updates() {
        let h = "668559afdb7760fa72298cc5052e73923c7df753f900112cfc76b5524348364f";
        assert_eq!(
            parse_sha256(&format!("{}  x.zip\n", h.to_uppercase())),
            Some(h.into())
        );
        assert_eq!(parse_sha256("nope"), None);
        assert_eq!(parse_sha256(""), None);
        assert!(!engine_update_available(h, h));
        assert!(engine_update_available(h, &"0".repeat(64)));
        // No record: out of date. Unreadable published file: no claim.
        assert!(engine_update_available("", h));
        assert!(!engine_update_available(h, "<html>"));
    }

    #[test]
    fn six_stem_job_writes_one_file_per_stem_in_model_order() {
        let c = test_catalog();
        let m = c.iter().find(|m| m.id == "bs-roformer-sw").unwrap();
        let outs: Vec<PathBuf> = (0..6).map(|k| PathBuf::from(format!("o{k}.wav"))).collect();
        let j = job(
            m,
            Path::new("m.ckpt"),
            Path::new("in.wav"),
            &outs,
            false,
            None,
        );
        let files = j["outputs"].as_array().unwrap();
        assert_eq!(files.len(), 6);
        assert_eq!(files[3]["stems"], json!([3]));
        assert!(files[3].get("rest").is_none());
        assert_eq!(j["device"], "cpu");
        assert_eq!(
            m.output_names(),
            ["Bass", "Drums", "Other", "Vocals", "Guitar", "Piano"]
        );
        assert_eq!(
            parse_line("error bad file"),
            Line::Error {
                gpu: false,
                message: "bad file".into()
            }
        );
        assert_eq!(parse_line("progress x"), Line::Other("progress x".into()));
    }

    #[test]
    fn remove_finds_every_copy() {
        let root = std::env::temp_dir().join(format!("af27-sep-test-{}", std::process::id()));
        let dirs = Dirs {
            own: root.join("own"),
            legacy: Some(root.join("legacy")),
        };
        let c = test_catalog();
        let m = &c[0];
        for r in dirs.search_roots() {
            std::fs::create_dir_all(r.join("models")).unwrap();
            std::fs::write(r.join("models").join(format!("{}.ckpt", m.id)), b"x").unwrap();
        }
        let copies = dirs.model_copies(m);
        assert_eq!(copies.len(), 2);
        assert!(dirs.is_shared(&copies[1]) && !dirs.is_shared(&copies[0]));
        remove_model(&dirs, m).unwrap();
        assert!(dirs.model(m).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Runs a real separation of a few seconds of tones with the installed
    /// engine and the first catalog model already downloaded, on the
    /// processor. Needs an installed engine and model; downloads nothing:
    /// `cargo test -p af27-separation -- --ignored live`.
    ///
    /// As of engine 0.2.0 (2026-11-19) this fails on purpose: the engine
    /// answers "This engine works only with Audiform: it is not compatible
    /// with the program that started it." Audiform27 surfaces that message
    /// as is; an engine build that accepts Audiform27 is the project
    /// lead's call.
    #[test]
    #[ignore]
    fn live_separation_with_the_installed_engine() {
        let dirs = Dirs::detect();
        let c = catalog(&dirs);
        let m = c
            .iter()
            .find(|m| dirs.model(m).is_some())
            .expect("a downloaded model")
            .clone();
        let rate = 44_100;
        let frames = rate as usize * 4;
        let mut buf = AudioBuffer::new(rate, 2, frames);
        for (i, s) in buf.channels[0].iter_mut().enumerate() {
            *s = 0.2 * (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin();
        }
        buf.channels[1] = buf.channels[0].clone();
        let out = std::env::temp_dir().join(format!("af27-live-{}", std::process::id()));
        let p = spawn(
            dirs,
            m.clone(),
            Arc::new(buf),
            "tone".into(),
            out.clone(),
            false,
            Some(1),
            Arc::new(AtomicBool::new(false)),
        );
        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if p.lock().unwrap().done {
                break;
            }
        }
        let g = p.lock().unwrap();
        assert!(g.error.is_none(), "{:?}", g.error);
        assert_eq!(g.outputs.len(), m.output_names().len());
        for (_, f) in &g.outputs {
            let b = af27_io::decode_file(f).unwrap();
            assert_eq!(b.frames(), frames);
            assert_eq!(b.rate, rate);
        }
        eprintln!("separated with {}: {:?}", m.id, g.outputs);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn finds_an_existing_audiform_install() {
        let d = Dirs::detect();
        // Only meaningful on a machine with Audiform installed; never fails.
        if let Some(legacy) = &d.legacy {
            assert!(legacy.ends_with("separation"));
        }
    }
}
