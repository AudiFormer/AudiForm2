// SPDX-License-Identifier: MIT OR Apache-2.0
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
    pub status: String,
    pub done: bool,
    pub error: Option<String>,
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
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&work);
            return Err("Separation cancelled.".into());
        }
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(line) => match parse_line(&line) {
                Line::Progress(f) => set(p, |g| g.fraction = f.clamp(0.0, 1.0) * 0.95),
                Line::Done => finished = true,
                Line::Error { gpu: true, message } => {
                    failure = Some(format!(
                        "The graphics card could not be used ({message}). Turn off Use GPU and try again."
                    ))
                }
                Line::Error { message, .. } => failure = Some(message),
                Line::Other(_) => {}
            },
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if let Some(f) = failure {
        return Err(f);
    }
    if !finished || !status.success() {
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
    const HOSTS: [&str; 2] = ["https://github.com/", "https://huggingface.co/"];
    if !HOSTS.iter().any(|h| url.starts_with(h)) {
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
            g.fraction = if total > 0 {
                (got as f32 / total as f32).min(1.0)
            } else {
                0.0
            };
            g.status = format!("Downloading: {} of {} MB", got >> 20, total >> 20);
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

/// Downloads and unpacks the engine into Audiform27's own folder, using
/// the system `tar` (bsdtar ships with Windows 10+ and reads zip).
pub fn install_engine(dirs: &Dirs, cancel: &AtomicBool, p: &Shared) -> Result<(), String> {
    let asset = engine_asset().ok_or("No separation engine is published for this computer yet.")?;
    let dir = dirs.own.join("engine");
    let zip = dirs.own.join(asset);
    download(&engine_url(asset), &zip, "", cancel, p, 0)?;
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

    #[test]
    fn finds_an_existing_audiform_install() {
        let d = Dirs::detect();
        // Only meaningful on a machine with Audiform installed; never fails.
        if let Some(legacy) = &d.legacy {
            assert!(legacy.ends_with("separation"));
        }
    }
}
