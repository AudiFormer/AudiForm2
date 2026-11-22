// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Recording what the computer plays: the whole desktop, everything but
//! AudiForm 2 itself, or chosen applications.
//!
//! On Windows the capture runs on its own thread against WASAPI: device
//! loopback for a whole output device, and process loopback (Windows 10
//! build 20348 / Windows 11 and later) for one application's process tree
//! or for everything except our own. Frames are converted to stereo
//! `f32` and handed to the UI thread over an `rtrb` ring, the same way the
//! microphone path does; the capture loop never allocates.
//!
//! Everything here that is not an OS call is pure and tested: sample
//! format parsing and conversion, the stereo fold-down, keeping the take
//! in step with the wall clock while nothing plays, and turning the raw
//! audio-session list into a short, deduplicated list of applications.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, Mutex};

#[cfg(windows)]
mod wasapi;

/// Where a recording comes from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RecordSource {
    /// The default recording device (microphone / line in), all its
    /// channels.
    Input,
    /// One channel or a pair of an input device (or of the ASIO driver).
    Channels(crate::routing::InputPick),
    /// Everything the computer plays except AudiForm 2, so monitoring
    /// doesn't feed back into the take. On systems without process
    /// loopback this records the whole default output device instead.
    SystemExceptSelf,
    /// Everything an output device plays (the default one when `None`).
    System(Option<OutputDevice>),
    /// One application and its child processes.
    App(AudioApp),
}

impl RecordSource {
    /// A short name for menus and take names.
    pub fn label(&self) -> String {
        match self {
            RecordSource::Input => "Microphone / line in".into(),
            RecordSource::Channels(p) => p.label(),
            RecordSource::SystemExceptSelf => "System audio (except AudiForm 2)".into(),
            RecordSource::System(None) => "System audio (everything)".into(),
            RecordSource::System(Some(d)) => format!("System audio: {}", d.name),
            RecordSource::App(a) => a.name.clone(),
        }
    }

    /// True for the sources that record what the computer plays.
    pub fn is_system(&self) -> bool {
        !matches!(self, RecordSource::Input | RecordSource::Channels(_))
    }
}

/// An output (render) device that can be recorded as loopback.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OutputDevice {
    /// The endpoint ID the OS uses.
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// An application that has an audio session.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AudioApp {
    /// The top process of the application's tree; capture includes its
    /// children (Discord, browsers and games often play from a child).
    pub pid: u32,
    pub name: String,
    /// The executable's file name, e.g. `Discord.exe`.
    pub exe: String,
    /// Some process of the app is playing right now.
    pub active: bool,
}

/// What the OS supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureSupport {
    /// Whole-device loopback (any Windows since Vista).
    pub loopback: bool,
    /// Per-application capture (Windows 10 build 20348+, Windows 11).
    pub per_app: bool,
}

/// The lowest Windows build with process loopback capture.
pub const PROCESS_LOOPBACK_MIN_BUILD: u32 = 20348;

/// What this computer can capture.
pub fn support() -> CaptureSupport {
    #[cfg(windows)]
    {
        CaptureSupport {
            loopback: true,
            per_app: wasapi::os_build().is_some_and(|b| b >= PROCESS_LOOPBACK_MIN_BUILD),
        }
    }
    #[cfg(not(windows))]
    {
        CaptureSupport {
            loopback: false,
            per_app: false,
        }
    }
}

/// The applications that currently have audio sessions, playing ones first.
/// AudiForm 2 itself and the system sounds session are left out.
pub fn list_audio_apps() -> Result<Vec<AudioApp>, String> {
    #[cfg(windows)]
    {
        wasapi::list_audio_apps()
    }
    #[cfg(not(windows))]
    {
        Err(UNSUPPORTED.into())
    }
}

/// The active output devices.
pub fn list_output_devices() -> Result<Vec<OutputDevice>, String> {
    #[cfg(windows)]
    {
        wasapi::list_output_devices()
    }
    #[cfg(not(windows))]
    {
        Err(UNSUPPORTED.into())
    }
}

#[cfg(not(windows))]
const UNSUPPORTED: &str = "Recording system audio is only available on Windows.";

/// A running system-audio capture. Dropping it stops the thread.
pub struct LoopbackCapture {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    error: Arc<Mutex<Option<String>>>,
    /// Frames per second of the stereo `f32` stream it produces.
    pub rate: u32,
    /// A note for the user, e.g. that per-app capture fell back to the
    /// whole desktop.
    pub note: Option<String>,
}

impl LoopbackCapture {
    /// Opens `source` (not [`RecordSource::Input`]) and starts it. Stereo
    /// frames go to `tx`; the loudest sample since the last read goes to
    /// `peak`.
    pub fn open(
        source: &RecordSource,
        tx: rtrb::Producer<f32>,
        peak: Arc<AtomicU32>,
    ) -> Result<LoopbackCapture, String> {
        #[cfg(windows)]
        {
            wasapi::open(source, tx, peak)
        }
        #[cfg(not(windows))]
        {
            let _ = (source, tx, peak);
            Err(UNSUPPORTED.into())
        }
    }

    /// An error the capture thread hit after it started (device unplugged
    /// and the like). Taken once.
    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|mut e| e.take())
    }

    /// Stops the thread and waits for it.
    pub fn stop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for LoopbackCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

// ------------------------------------------------------------ formats

/// Sample layouts a WASAPI mix format can have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFmt {
    F32,
    I16,
    /// Packed 3-byte samples.
    I24,
    /// 4-byte integer containers (24 valid bits or 32).
    I32,
}

impl SampleFmt {
    pub fn bytes(self) -> usize {
        match self {
            SampleFmt::I16 => 2,
            SampleFmt::I24 => 3,
            SampleFmt::F32 | SampleFmt::I32 => 4,
        }
    }
}

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// Reads a `WAVEFORMATEX`'s sample layout. `sub_tag` is the first two
/// bytes of `WAVEFORMATEXTENSIBLE::SubFormat` (the KSDATAFORMAT subtypes
/// for PCM and float are the old format tags in that slot).
pub fn sample_fmt(tag: u16, bits: u16, sub_tag: Option<u16>) -> Option<SampleFmt> {
    let tag = if tag == WAVE_FORMAT_EXTENSIBLE {
        sub_tag?
    } else {
        tag
    };
    match (tag, bits) {
        (WAVE_FORMAT_IEEE_FLOAT, 32) => Some(SampleFmt::F32),
        (WAVE_FORMAT_PCM, 16) => Some(SampleFmt::I16),
        (WAVE_FORMAT_PCM, 24) => Some(SampleFmt::I24),
        (WAVE_FORMAT_PCM, 32) => Some(SampleFmt::I32),
        _ => None,
    }
}

/// One sample at the start of `b`, as `f32` in -1..1.
#[inline]
fn sample_at(b: &[u8], fmt: SampleFmt) -> f32 {
    match fmt {
        SampleFmt::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        SampleFmt::I16 => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
        SampleFmt::I24 => {
            let v = i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8;
            v as f32 / 8_388_608.0
        }
        SampleFmt::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
    }
}

const FOLD: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Folds one frame of `ch` channels (WAVE order: FL FR FC LFE BL BR SL SR)
/// to stereo. Mono goes to both sides; the LFE is left out.
#[inline]
pub fn fold_to_stereo(frame: &[f32]) -> (f32, f32) {
    match frame.len() {
        0 => (0.0, 0.0),
        1 => (frame[0], frame[0]),
        2 => (frame[0], frame[1]),
        n => {
            let mut l = frame[0];
            let mut r = frame[1];
            if n > 2 {
                l += FOLD * frame[2];
                r += FOLD * frame[2];
            }
            // Pairs after the LFE: back, side, and so on, left then right.
            let mut i = 4;
            while i + 1 < n {
                l += FOLD * frame[i];
                r += FOLD * frame[i + 1];
                i += 2;
            }
            if i < n {
                l += FOLD * frame[i];
                r += FOLD * frame[i];
            }
            (l, r)
        }
    }
}

/// The most channels a frame is read with; more are ignored.
pub const MAX_CHANNELS: usize = 16;

/// Converts interleaved `bytes` of `channels`-channel `fmt` audio to
/// stereo frames, calling `out(l, r)` for each. Returns the loudest
/// absolute sample out. Trailing partial frames are ignored. No allocation.
pub fn convert(
    bytes: &[u8],
    fmt: SampleFmt,
    channels: usize,
    mut out: impl FnMut(f32, f32),
) -> f32 {
    let channels = channels.max(1);
    let sb = fmt.bytes();
    let block = sb * channels;
    let used = channels.min(MAX_CHANNELS);
    let mut frame = [0.0f32; MAX_CHANNELS];
    let mut peak = 0.0f32;
    for f in bytes.chunks_exact(block) {
        for (c, slot) in frame.iter_mut().enumerate().take(used) {
            *slot = sample_at(&f[c * sb..], fmt);
        }
        let (l, r) = fold_to_stereo(&frame[..used]);
        peak = peak.max(l.abs()).max(r.abs());
        out(l, r);
    }
    peak
}

/// Pushes one stereo frame, both samples or neither, so a full ring never
/// leaves the channels out of step. Returns false when it was dropped.
#[inline]
pub fn push_frame(tx: &mut rtrb::Producer<f32>, l: f32, r: f32) -> bool {
    if tx.slots() < 2 {
        return false;
    }
    let _ = tx.push(l);
    let _ = tx.push(r);
    true
}

/// Loopback delivers nothing while nothing plays. To keep a take in step
/// with the timeline, silence is added once the stream lags the wall clock
/// by more than 60 ms, leaving 20 ms of slack for late packets. Returns
/// the frames of silence to add now.
pub fn silence_to_insert(expected: u64, delivered: u64, rate: u32) -> u64 {
    let lag = expected.saturating_sub(delivered);
    let threshold = rate as u64 * 60 / 1000;
    let slack = rate as u64 * 20 / 1000;
    if lag > threshold {
        lag - slack
    } else {
        0
    }
}

// ------------------------------------------------------- applications

/// One audio session as the OS reports it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawSession {
    pub pid: u32,
    pub display_name: String,
    pub active: bool,
    pub system_sounds: bool,
}

/// What we know about a process.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcInfo {
    pub parent: u32,
    /// Executable file name, e.g. `chrome.exe`.
    pub exe: String,
    /// Full image path when we could read it.
    pub path: Option<String>,
    /// The executable's FileDescription, e.g. "Google Chrome".
    pub description: Option<String>,
}

/// The top process of `pid`'s run of same-named ancestors (a browser's
/// audio helper becomes the browser).
pub fn app_root(pid: u32, procs: &HashMap<u32, ProcInfo>) -> u32 {
    let Some(me) = procs.get(&pid) else {
        return pid;
    };
    let mut cur = pid;
    for _ in 0..64 {
        let Some(info) = procs.get(&cur) else { break };
        let parent = info.parent;
        if parent == 0 || parent == cur {
            break;
        }
        match procs.get(&parent) {
            Some(p) if p.exe.eq_ignore_ascii_case(&me.exe) => cur = parent,
            _ => break,
        }
    }
    cur
}

/// True when `pid` is `ancestor` or one of its descendants.
pub fn in_tree(pid: u32, ancestor: u32, procs: &HashMap<u32, ProcInfo>) -> bool {
    let mut cur = pid;
    for _ in 0..64 {
        if cur == ancestor {
            return true;
        }
        match procs.get(&cur) {
            Some(i) if i.parent != 0 && i.parent != cur => cur = i.parent,
            _ => return false,
        }
    }
    false
}

/// A friendly name: the session's own name, the executable's
/// description, or the file name without `.exe`; games installed by Steam
/// read "Steam game: <folder>".
pub fn app_label(display_name: &str, info: Option<&ProcInfo>) -> String {
    if let Some(path) = info.and_then(|i| i.path.as_deref()) {
        let lower = path.to_ascii_lowercase().replace('/', "\\");
        if let Some(at) = lower.find("\\steamapps\\common\\") {
            let rest = &path[at + "\\steamapps\\common\\".len()..];
            let folder = rest.split(['\\', '/']).next().unwrap_or("");
            if !folder.is_empty() {
                return format!("Steam game: {folder}");
            }
        }
    }
    let d = display_name.trim();
    if !d.is_empty() && !d.starts_with('@') {
        return d.to_string();
    }
    if let Some(desc) = info.and_then(|i| i.description.as_deref()) {
        let desc = desc.trim();
        if !desc.is_empty() {
            return desc.to_string();
        }
    }
    match info {
        Some(i) if !i.exe.is_empty() => {
            let e = i.exe.as_str();
            match e.len().checked_sub(4) {
                Some(n) if e.is_char_boundary(n) && e[n..].eq_ignore_ascii_case(".exe") => {
                    e[..n].to_string()
                }
                _ => e.to_string(),
            }
        }
        _ => "Unknown app".into(),
    }
}

/// Turns sessions into one entry per application tree: our own process
/// tree, the system sounds session and PID 0 are dropped; sessions of the
/// same app merge under its top process. Playing apps come first, then by
/// name.
pub fn build_app_list(
    sessions: &[RawSession],
    procs: &HashMap<u32, ProcInfo>,
    own_pid: u32,
) -> Vec<AudioApp> {
    let mut out: Vec<AudioApp> = Vec::new();
    for s in sessions {
        if s.system_sounds || s.pid == 0 || in_tree(s.pid, own_pid, procs) {
            continue;
        }
        let root = app_root(s.pid, procs);
        if let Some(a) = out.iter_mut().find(|a| a.pid == root) {
            a.active |= s.active;
            continue;
        }
        let info = procs.get(&root).or_else(|| procs.get(&s.pid));
        // A child's session name beats the parent's lack of one.
        let name = app_label(&s.display_name, info);
        out.push(AudioApp {
            pid: root,
            name,
            exe: info.map(|i| i.exe.clone()).unwrap_or_default(),
            active: s.active,
        });
    }
    out.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_read_from_plain_and_extensible_headers() {
        assert_eq!(sample_fmt(3, 32, None), Some(SampleFmt::F32));
        assert_eq!(sample_fmt(1, 16, None), Some(SampleFmt::I16));
        assert_eq!(sample_fmt(0xFFFE, 32, Some(3)), Some(SampleFmt::F32));
        assert_eq!(sample_fmt(0xFFFE, 24, Some(1)), Some(SampleFmt::I24));
        assert_eq!(sample_fmt(0xFFFE, 32, Some(1)), Some(SampleFmt::I32));
        assert_eq!(sample_fmt(0xFFFE, 32, None), None);
        assert_eq!(sample_fmt(1, 8, None), None);
        assert_eq!(sample_fmt(2, 16, None), None);
    }

    fn collect(bytes: &[u8], fmt: SampleFmt, ch: usize) -> (Vec<(f32, f32)>, f32) {
        let mut v = Vec::new();
        let p = convert(bytes, fmt, ch, |l, r| v.push((l, r)));
        (v, p)
    }

    #[test]
    fn integer_and_float_samples_convert_to_unit_range() {
        let mut b = Vec::new();
        for s in [16384i16, -32768] {
            b.extend_from_slice(&s.to_le_bytes());
        }
        let (v, p) = collect(&b, SampleFmt::I16, 2);
        assert_eq!(v, vec![(0.5, -1.0)]);
        assert_eq!(p, 1.0);

        // 24-bit packed: 0x400000 is +0.5, 0xC00000 is -0.5.
        let b = [0x00, 0x00, 0x40, 0x00, 0x00, 0xC0];
        let (v, _) = collect(&b, SampleFmt::I24, 2);
        assert_eq!(v, vec![(0.5, -0.5)]);

        let mut b = Vec::new();
        b.extend_from_slice(&(1i32 << 30).to_le_bytes());
        b.extend_from_slice(&0.25f32.to_le_bytes());
        let (v, _) = collect(&b[..4], SampleFmt::I32, 1);
        assert_eq!(v, vec![(0.5, 0.5)]);
        let (v, _) = collect(&b[4..], SampleFmt::F32, 1);
        assert_eq!(v, vec![(0.25, 0.25)]);
    }

    #[test]
    fn partial_frames_are_ignored() {
        let b = [0u8; 4 * 2 * 3 + 5];
        let (v, p) = collect(&b, SampleFmt::F32, 2);
        assert_eq!(v.len(), 3);
        assert_eq!(p, 0.0);
    }

    #[test]
    fn surround_folds_to_stereo_without_the_lfe() {
        // 5.1: FL FR FC LFE BL BR
        let (l, r) = fold_to_stereo(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!((l, r), (1.0, 0.0));
        let (l, r) = fold_to_stereo(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        assert!((l - FOLD).abs() < 1e-6 && (r - FOLD).abs() < 1e-6);
        let (l, r) = fold_to_stereo(&[0.0, 0.0, 0.0, 0.0, 1.0, 0.5]);
        assert!((l - FOLD).abs() < 1e-6 && (r - 0.5 * FOLD).abs() < 1e-6);
        // 7.1 side pair lands too.
        let (l, r) = fold_to_stereo(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
        assert!(l.abs() < 1e-6 && (r - FOLD).abs() < 1e-6);
        assert_eq!(fold_to_stereo(&[0.3]), (0.3, 0.3));
    }

    #[test]
    fn ring_hand_off_keeps_frames_whole_and_in_order() {
        let (mut tx, mut rx) = rtrb::RingBuffer::<f32>::new(5);
        assert!(push_frame(&mut tx, 1.0, 2.0));
        assert!(push_frame(&mut tx, 3.0, 4.0));
        // One slot left: the frame is dropped whole, not split.
        assert!(!push_frame(&mut tx, 5.0, 6.0));
        let got: Vec<f32> = std::iter::from_fn(|| rx.pop().ok()).collect();
        assert_eq!(got, vec![1.0, 2.0, 3.0, 4.0]);
        assert!(push_frame(&mut tx, 7.0, 8.0));
        assert_eq!(rx.pop(), Ok(7.0));
        assert_eq!(rx.pop(), Ok(8.0));
    }

    #[test]
    fn converted_frames_cross_the_ring_from_another_thread() {
        let (mut tx, mut rx) = rtrb::RingBuffer::<f32>::new(4096);
        let mut bytes = Vec::new();
        for i in 0..1000i16 {
            bytes.extend_from_slice(&(i * 16).to_le_bytes());
        }
        let t = std::thread::spawn(move || {
            convert(&bytes, SampleFmt::I16, 1, |l, r| {
                assert!(push_frame(&mut tx, l, r));
            })
        });
        let peak = t.join().unwrap();
        let got: Vec<f32> = std::iter::from_fn(|| rx.pop().ok()).collect();
        assert_eq!(got.len(), 2000);
        assert_eq!(got[2 * 999], 999.0 * 16.0 / 32768.0);
        assert_eq!(got[2 * 999], got[2 * 999 + 1]);
        assert_eq!(peak, 999.0 * 16.0 / 32768.0);
    }

    #[test]
    fn silence_fills_only_real_gaps() {
        let rate = 48_000;
        // Within jitter: nothing.
        assert_eq!(silence_to_insert(48_000, 48_000 - 2_000, rate), 0);
        assert_eq!(silence_to_insert(100, 200, rate), 0);
        // A second of nothing: fill all but 20 ms.
        assert_eq!(silence_to_insert(48_000, 0, rate), 48_000 - 960);
    }

    fn procs() -> HashMap<u32, ProcInfo> {
        let p = |parent: u32, exe: &str, path: Option<&str>, desc: Option<&str>| ProcInfo {
            parent,
            exe: exe.into(),
            path: path.map(Into::into),
            description: desc.map(Into::into),
        };
        HashMap::from([
            (4, p(0, "System", None, None)),
            (100, p(4, "explorer.exe", None, None)),
            (
                200,
                p(
                    100,
                    "Discord.exe",
                    Some("C:\\Discord\\Discord.exe"),
                    Some("Discord"),
                ),
            ),
            (201, p(200, "Discord.exe", None, None)),
            (202, p(201, "Discord.exe", None, None)),
            (300, p(100, "chrome.exe", None, Some("Google Chrome"))),
            (301, p(300, "chrome.exe", None, None)),
            (
                400,
                p(
                    100,
                    "Hades.exe",
                    Some("D:\\SteamLibrary\\steamapps\\common\\Hades\\x64\\Hades.exe"),
                    Some("Hades"),
                ),
            ),
            (500, p(100, "audiform2.exe", None, None)),
            (501, p(500, "helper.exe", None, None)),
            (600, p(100, "Spotify.exe", None, None)),
            (700, p(9999, "orphan.exe", None, None)),
        ])
    }

    #[test]
    fn helpers_resolve_to_their_app_and_trees_are_checked() {
        let p = procs();
        assert_eq!(app_root(202, &p), 200);
        assert_eq!(app_root(301, &p), 300);
        assert_eq!(app_root(400, &p), 400);
        assert_eq!(app_root(700, &p), 700);
        assert_eq!(app_root(12345, &p), 12345);
        assert!(in_tree(501, 500, &p));
        assert!(in_tree(500, 500, &p));
        assert!(!in_tree(202, 500, &p));
        assert!(!in_tree(700, 500, &p));
    }

    #[test]
    fn labels_prefer_session_names_then_descriptions_then_files() {
        let p = procs();
        assert_eq!(app_label("", p.get(&200)), "Discord");
        assert_eq!(app_label("Meeting", p.get(&200)), "Meeting");
        assert_eq!(
            app_label("@%SystemRoot%\\x.dll,-202", p.get(&600)),
            "Spotify"
        );
        assert_eq!(app_label("", p.get(&400)), "Steam game: Hades");
        assert_eq!(app_label("", None), "Unknown app");
    }

    #[test]
    fn app_list_merges_trees_and_drops_ourselves() {
        let p = procs();
        let sessions = vec![
            RawSession {
                pid: 202,
                active: false,
                ..Default::default()
            },
            RawSession {
                pid: 201,
                active: true,
                ..Default::default()
            },
            RawSession {
                pid: 301,
                active: false,
                ..Default::default()
            },
            RawSession {
                pid: 501,
                active: true,
                ..Default::default()
            },
            RawSession {
                pid: 0,
                system_sounds: true,
                active: true,
                ..Default::default()
            },
            RawSession {
                pid: 400,
                active: true,
                ..Default::default()
            },
        ];
        let apps = build_app_list(&sessions, &p, 500);
        let names: Vec<(&str, u32, bool)> = apps
            .iter()
            .map(|a| (a.name.as_str(), a.pid, a.active))
            .collect();
        assert_eq!(
            names,
            vec![
                ("Discord", 200, true),
                ("Steam game: Hades", 400, true),
                ("Google Chrome", 300, false),
            ]
        );
        assert_eq!(apps[0].exe, "Discord.exe");
    }

    #[test]
    fn source_labels() {
        assert_eq!(RecordSource::Input.label(), "Microphone / line in");
        assert!(!RecordSource::Input.is_system());
        assert!(RecordSource::SystemExceptSelf.is_system());
        let d = OutputDevice {
            id: "x".into(),
            name: "Speakers".into(),
            is_default: true,
        };
        assert_eq!(
            RecordSource::System(Some(d)).label(),
            "System audio: Speakers"
        );
    }

    /// Opens the default output device as loopback for two seconds,
    /// writes the take to a WAV in the temp folder and reports the peak.
    /// Run with `cargo test -p af2-engine capture -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs a real audio device"]
    fn capture_two_seconds_of_desktop_audio() {
        let s = support();
        println!("support: {s:?}");
        let mut sources = vec![RecordSource::System(None)];
        if s.per_app {
            sources.push(RecordSource::SystemExceptSelf);
        }
        match list_output_devices() {
            Ok(d) => println!("outputs: {d:?}"),
            Err(e) => println!("outputs error: {e}"),
        }
        match list_audio_apps() {
            Ok(a) => {
                println!("apps: {a:?}");
                if s.per_app {
                    if let Some(app) = a.into_iter().next() {
                        sources.push(RecordSource::App(app));
                    }
                }
            }
            Err(e) => println!("apps error: {e}"),
        }
        for (n, src) in sources.iter().enumerate() {
            let (tx, mut rx) = rtrb::RingBuffer::<f32>::new(48_000 * 2 * 10);
            let peak = Arc::new(AtomicU32::new(0));
            let mut cap = LoopbackCapture::open(src, tx, peak.clone())
                .unwrap_or_else(|e| panic!("{}: {e}", src.label()));
            std::thread::sleep(std::time::Duration::from_secs(2));
            cap.stop();
            assert_eq!(cap.take_error(), None);
            let data: Vec<f32> = std::iter::from_fn(|| rx.pop().ok()).collect();
            let frames = data.len() / 2;
            let pk = data.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let path = std::env::temp_dir().join(format!("af2-capture-{n}.wav"));
            write_wav_f32(&path, cap.rate, &data);
            println!(
                "{}: {} Hz, {frames} frames ({:.2} s), peak {pk:.4}, note {:?} -> {}",
                src.label(),
                cap.rate,
                frames as f64 / cap.rate as f64,
                cap.note,
                path.display()
            );
            assert!(frames as f64 > cap.rate as f64 * 1.5, "too few frames");
        }
    }

    /// Plays a very quiet tone (-46 dBFS) from this process and records
    /// it two ways: the whole desktop must hear it, "except AudiForm 2"
    /// (here: except this test process) must not.
    #[test]
    #[ignore = "needs a real audio device; plays a near-silent tone"]
    fn excluding_ourselves_hides_our_own_playback() {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        if !support().per_app {
            println!("no process loopback here; skipped");
            return;
        }
        let dev = cpal::default_host()
            .default_output_device()
            .expect("output");
        let cfg = dev.default_output_config().expect("config");
        let rate = cfg.sample_rate().0 as f32;
        let ch = cfg.channels() as usize;
        let mut ph = 0.0f32;
        let stream = dev
            .build_output_stream(
                &cfg.into(),
                move |d: &mut [f32], _| {
                    for f in d.chunks_mut(ch) {
                        ph = (ph + 440.0 / rate) % 1.0;
                        let v = 0.005 * (ph * std::f32::consts::TAU).sin();
                        f.iter_mut().for_each(|s| *s = v);
                    }
                },
                |e| eprintln!("{e}"),
                None,
            )
            .expect("stream");
        stream.play().expect("play");
        let mut peaks = Vec::new();
        for src in [RecordSource::System(None), RecordSource::SystemExceptSelf] {
            let (tx, mut rx) = rtrb::RingBuffer::<f32>::new(48_000 * 2 * 10);
            let mut cap = LoopbackCapture::open(&src, tx, Arc::new(AtomicU32::new(0))).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1500));
            cap.stop();
            let pk = std::iter::from_fn(|| rx.pop().ok()).fold(0.0f32, |m, s| m.max(s.abs()));
            println!("{}: peak {pk:.5}", src.label());
            peaks.push(pk);
        }
        drop(stream);
        assert!(
            peaks[0] > 0.0005,
            "the desktop capture should hear the tone"
        );
        assert!(peaks[1] < peaks[0], "except-self should not hear our tone");
    }

    fn write_wav_f32(path: &std::path::Path, rate: u32, data: &[f32]) {
        let mut b = Vec::with_capacity(44 + data.len() * 4);
        let len = (data.len() * 4) as u32;
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&3u16.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 8).to_le_bytes());
        b.extend_from_slice(&8u16.to_le_bytes());
        b.extend_from_slice(&32u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&len.to_le_bytes());
        for s in data {
            b.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }
}
