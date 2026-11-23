// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Video: the picture to work to.
//!
//! A video file put in a session (dropped or imported like audio) becomes
//! the session's video track, a row of thumbnails along the top of the
//! Multitrack, while its sound goes on an audio track as usual. The Video
//! panel (Window > Video; at first behind History) shows the frame under
//! the playhead, following playback, scrubbing and the Waveform editor
//! when a video file is open there. It decodes at full, half or quarter
//! size, so a big video plays smoothly on a modest computer.
//!
//! Frames come from the system's decoders (Windows: Media Foundation),
//! else from an installed ffmpeg. Nothing is shipped or downloaded.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use af2_osdecode::video::{Frame, VideoInfo};
use egui::{pos2, vec2, Color32, Rect, Rounding, Sense, Stroke, TextureHandle};

use crate::state::{Mode, State};
use crate::theme::{c, ca};

/// Files that may carry a picture.
pub const EXTS: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "asf", "mpg", "mpeg", "vob", "ts", "m2ts", "mts",
    "flv", "3gp", "3g2", "ogv",
];

pub fn is_video_path(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// The row's height in the Multitrack.
pub const ROW_H: f32 = 30.0;

// ---------------------------------------------------------------- reading

/// Frames from an installed ffmpeg: a process per seek, writing raw RGBA.
struct Ff {
    exe: PathBuf,
    path: PathBuf,
    w: u32,
    h: u32,
    fps: f64,
    child: Option<Child>,
    next_time: f64,
}

impl Ff {
    fn info(exe: &Path, path: &Path) -> Result<VideoInfo, String> {
        let probe = exe.with_file_name(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" });
        let mut cmd = Command::new(&probe);
        cmd.args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height,r_frame_rate:format=duration", "-of", "default=nw=1"])
            .arg(path)
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        hide_console(&mut cmd);
        let out = cmd.output().map_err(|e| format!("ffprobe did not run: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).map(str::trim);
        let w: u32 = get("width").and_then(|v| v.parse().ok()).ok_or("It has no video.")?;
        let h: u32 = get("height").and_then(|v| v.parse().ok()).ok_or("It has no video.")?;
        let fps = get("r_frame_rate")
            .and_then(|r| r.split_once('/'))
            .and_then(|(n, d)| Some(n.parse::<f64>().ok()? / d.parse::<f64>().ok().filter(|d| *d > 0.0)?))
            .filter(|f| *f > 0.0 && *f < 1000.0)
            .unwrap_or(30.0);
        let duration = get("duration").and_then(|v| v.parse().ok()).unwrap_or(0.0);
        Ok(VideoInfo { width: w, height: h, fps, duration })
    }

    fn open(path: &Path, width: Option<u32>) -> Result<(Ff, VideoInfo), String> {
        let exe = af2_io::fallback::ffmpeg().ok_or("No decoder for this video here (installing ffmpeg would add one).")?;
        let info = Ff::info(&exe, path)?;
        let (w, h) = af2_osdecode::video::scaled(info.width, info.height, width);
        let mut f = Ff { exe, path: path.to_path_buf(), w, h, fps: info.fps, child: None, next_time: 0.0 };
        f.seek(0.0)?;
        Ok((f, info))
    }

    fn seek(&mut self, t: f64) -> Result<(), String> {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let mut cmd = Command::new(&self.exe);
        cmd.args(["-v", "error", "-nostdin", "-ss", &format!("{:.3}", t.max(0.0)), "-i"])
            .arg(&self.path)
            .args(["-an", "-sn", "-vf", &format!("scale={}:{}", self.w, self.h), "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        hide_console(&mut cmd);
        self.child = Some(cmd.spawn().map_err(|e| format!("ffmpeg did not run: {e}"))?);
        self.next_time = t.max(0.0);
        Ok(())
    }

    fn next(&mut self) -> Result<Option<Frame>, String> {
        let Some(out) = self.child.as_mut().and_then(|c| c.stdout.as_mut()) else { return Ok(None) };
        let mut rgba = vec![0u8; (self.w * self.h * 4) as usize];
        if out.read_exact(&mut rgba).is_err() {
            return Ok(None);
        }
        let time = self.next_time;
        self.next_time += 1.0 / self.fps;
        Ok(Some(Frame { time, width: self.w, height: self.h, rgba }))
    }
}

impl Drop for Ff {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

enum Source {
    Os(af2_osdecode::video::VideoReader),
    Ff(Ff),
}

impl Source {
    /// The system's decoders first, then ffmpeg.
    fn open(path: &Path, width: Option<u32>) -> Result<(Source, VideoInfo), String> {
        match af2_osdecode::video::open(path, width) {
            Ok((r, i)) => Ok((Source::Os(r), i)),
            Err(os) => match Ff::open(path, width) {
                Ok((f, i)) => Ok((Source::Ff(f), i)),
                Err(ff) if cfg!(windows) => Err(format!("{os}. {ff}")),
                Err(ff) => Err(ff),
            },
        }
    }

    fn seek(&mut self, t: f64) -> Result<(), String> {
        match self {
            Source::Os(r) => r.seek(t),
            Source::Ff(f) => f.seek(t),
        }
    }

    fn next(&mut self) -> Result<Option<Frame>, String> {
        match self {
            Source::Os(r) => r.next_frame(),
            Source::Ff(f) => f.next(),
        }
    }
}

// ---------------------------------------------------------------- player

#[derive(Clone, PartialEq)]
struct Want {
    path: PathBuf,
    width: Option<u32>,
}

#[derive(Default)]
struct Shared {
    want: Option<Want>,
    time: f64,
    playing: bool,
    frame: Option<Arc<Frame>>,
    info: Option<Result<VideoInfo, String>>,
    /// Bumped with each new frame, so the panel knows to upload it.
    version: u64,
    quit: bool,
}

/// Decodes the frame under the playhead on its own thread.
struct Player {
    shared: Arc<(Mutex<Shared>, Condvar)>,
}

impl Player {
    fn start(ctx: egui::Context) -> Player {
        let shared = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let s2 = shared.clone();
        let _ = std::thread::Builder::new().name("af2-video".into()).spawn(move || run(s2, ctx));
        Player { shared }
    }

    fn set(&self, want: Option<Want>, time: f64, playing: bool) {
        let (m, cv) = &*self.shared;
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        let changed = g.want != want || (g.time - time).abs() > 1e-4 || g.playing != playing;
        if g.want != want {
            g.frame = None;
            g.info = None;
        }
        g.want = want;
        g.time = time;
        g.playing = playing;
        if changed {
            cv.notify_all();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let (m, cv) = &*self.shared;
        m.lock().unwrap_or_else(|e| e.into_inner()).quit = true;
        cv.notify_all();
    }
}

fn run(shared: Arc<(Mutex<Shared>, Condvar)>, ctx: egui::Context) {
    let (m, cv) = &*shared;
    let mut open: Option<(Want, Source, VideoInfo)> = None;
    // The last frame shown, and one read past the playhead.
    let mut shown: Option<f64> = None;
    let mut ahead: Option<Frame> = None;
    loop {
        let (want, t, playing) = {
            let g = m.lock().unwrap_or_else(|e| e.into_inner());
            if g.quit {
                return;
            }
            (g.want.clone(), g.time, g.playing)
        };
        let Some(want) = want else {
            open = None;
            let g = m.lock().unwrap_or_else(|e| e.into_inner());
            let _ = cv.wait_timeout(g, Duration::from_millis(200));
            continue;
        };
        if open.as_ref().map(|(w, ..)| w) != Some(&want) {
            open = None;
            shown = None;
            ahead = None;
            let r = Source::open(&want.path, want.width);
            let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
            match r {
                Ok((s, info)) => {
                    g.info = Some(Ok(info));
                    open = Some((want.clone(), s, info));
                }
                Err(e) => {
                    g.info = Some(Err(e));
                    drop(g);
                    // Not again until something else is asked for.
                    loop {
                        let g = m.lock().unwrap_or_else(|e| e.into_inner());
                        if g.quit || g.want.as_ref() != Some(&want) {
                            break;
                        }
                        let _ = cv.wait_timeout(g, Duration::from_millis(300));
                    }
                    continue;
                }
            }
            ctx.request_repaint();
        }
        let (w, src, info) = open.as_mut().unwrap();
        let half = 0.5 / info.fps.max(1.0);
        let t = t.clamp(0.0, (info.duration - half).max(0.0));
        let need_seek = match shown {
            None => true,
            Some(s) => t < s - half - 1e-3 || t > s + 2.0,
        };
        let mut best: Option<Frame> = None;
        if need_seek {
            ahead = None;
            let mut ok = src.seek(t).is_ok();
            let mut n = 0;
            loop {
                let f = match src.next() {
                    Ok(Some(f)) => f,
                    _ => {
                        // Some streams can't seek: from the start again.
                        if ok && best.is_none() {
                            if let Ok((s, _)) = Source::open(&w.path, w.width) {
                                *src = s;
                            }
                            ok = false;
                            continue;
                        }
                        break;
                    }
                };
                n += 1;
                if f.time <= t + half || best.is_none() && f.time > t + half && n == 1 && f.time < t + 1.0 {
                    let past = f.time > t + half;
                    best = Some(f);
                    if past {
                        break;
                    }
                } else {
                    ahead = Some(f);
                    break;
                }
                if n > 20_000 {
                    break;
                }
            }
        } else {
            // On from where it was: frames up to the playhead.
            loop {
                let f = match ahead.take() {
                    Some(f) => f,
                    None => match src.next() {
                        Ok(Some(f)) => f,
                        _ => break,
                    },
                };
                if f.time <= t + half {
                    best = Some(f);
                } else {
                    ahead = Some(f);
                    break;
                }
            }
        }
        if let Some(f) = best {
            shown = Some(f.time);
            let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
            g.frame = Some(Arc::new(f));
            g.version += 1;
            drop(g);
            ctx.request_repaint();
        } else if shown.is_none() {
            shown = Some(t);
        }
        let g = m.lock().unwrap_or_else(|e| e.into_inner());
        let wait = if playing { Duration::from_millis(4) } else { Duration::from_millis(100) };
        let _ = cv.wait_timeout(g, wait);
    }
}

// ---------------------------------------------------------------- thumbnails

/// Small pictures along a video, filled in as they're decoded.
#[derive(Default)]
struct Thumbs {
    step: f64,
    pics: Vec<(f64, Arc<Frame>)>,
    done: bool,
    error: Option<String>,
}

const THUMB_W: u32 = 96;

fn thumbnails(path: PathBuf, out: Arc<Mutex<Thumbs>>, ctx: egui::Context) {
    let _ = std::thread::Builder::new().name("af2-video-thumbs".into()).spawn(move || {
        let (mut src, info) = match Source::open(&path, Some(THUMB_W)) {
            Ok(x) => x,
            Err(e) => {
                let mut g = out.lock().unwrap_or_else(|e| e.into_inner());
                g.error = Some(e);
                g.done = true;
                return;
            }
        };
        // About 150 along the whole video, at least half a second apart.
        let step = (info.duration / 150.0).max(0.5);
        out.lock().unwrap_or_else(|e| e.into_inner()).step = step;
        let mut next = 0.0;
        let mut n = 0;
        while let Ok(Some(f)) = src.next() {
            if f.time + 1e-6 >= next {
                next = (f.time / step).floor() * step + step;
                out.lock().unwrap_or_else(|e| e.into_inner()).pics.push((f.time, Arc::new(f)));
                n += 1;
                if n % 8 == 0 {
                    ctx.request_repaint();
                }
            }
            if Arc::strong_count(&out) == 1 {
                return;
            }
        }
        out.lock().unwrap_or_else(|e| e.into_inner()).done = true;
        ctx.request_repaint();
    });
}

// ---------------------------------------------------------------- state

#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Scale {
    Full,
    Half,
    Quarter,
}

impl Scale {
    fn factor(self) -> f64 {
        match self {
            Scale::Full => 1.0,
            Scale::Half => 0.5,
            Scale::Quarter => 0.25,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Scale::Full => "1x",
            Scale::Half => "0.5x",
            Scale::Quarter => "0.25x",
        }
    }
}

pub struct VideoUi {
    player: Option<Player>,
    scale: Scale,
    texture: Option<(u64, TextureHandle)>,
    thumbs: HashMap<PathBuf, Arc<Mutex<Thumbs>>>,
    thumb_tex: HashMap<(PathBuf, usize), TextureHandle>,
    /// The full-size picture's size of what's showing, once known.
    shown: Option<(PathBuf, VideoInfo)>,
}

fn scale_path() -> PathBuf {
    crate::state::data_dir().join("video_scale.txt")
}

impl Default for VideoUi {
    fn default() -> Self {
        let scale = match std::fs::read_to_string(scale_path()).unwrap_or_default().trim() {
            "1x" => Scale::Full,
            "0.25x" => Scale::Quarter,
            _ => Scale::Half,
        };
        VideoUi { player: None, scale, texture: None, thumbs: HashMap::new(), thumb_tex: HashMap::new(), shown: None }
    }
}

/// What the panel shows now: the file and the time in it.
fn current(st: &State) -> Option<(PathBuf, f64)> {
    match st.mode {
        Mode::Multitrack => st.session.video.as_ref().map(|v| (v.path.clone(), st.position() - v.start)),
        Mode::Waveform => {
            let d = st.doc()?;
            let p = d.path.clone().filter(|p| is_video_path(p))?;
            Some((p, st.position()))
        }
    }
}

/// Each frame: tells the decoder where the playhead is.
pub fn frame(ctx: &egui::Context, st: &mut State) {
    let cur = current(st);
    let playing = st.is_playing();
    let info = st
        .video
        .shown
        .as_ref()
        .filter(|(p, _)| cur.as_ref().is_some_and(|(c, _)| c == p))
        .map(|(_, i)| *i);
    let scale = st.video.scale;
    if cur.is_none() && st.video.player.is_none() {
        return;
    }
    let player = st.video.player.get_or_insert_with(|| Player::start(ctx.clone()));
    let want = cur.as_ref().map(|(p, _)| Want {
        path: p.clone(),
        width: match (scale, info) {
            (Scale::Full, _) => None,
            (s, Some(i)) => Some((i.width as f64 * s.factor()).round() as u32),
            // Not known yet: a guess the first frames settle.
            (s, None) => Some((1920.0 * s.factor()) as u32),
        },
    });
    player.set(want, cur.as_ref().map_or(0.0, |(_, t)| *t), playing);
    if let Some((p, _)) = &cur {
        let g = player.shared.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(Ok(i)) = &g.info {
            let i = *i;
            drop(g);
            if st.video.shown.as_ref().map(|(sp, _)| sp) != Some(p) {
                st.video.shown = Some((p.clone(), i));
            }
        }
    }
}

/// The session's video, from a file just placed on a track (as Audition
/// does): its picture goes on the video track at the clip's start.
/// Whether the session's video row shows in the Multitrack.
pub fn row_shown(st: &State) -> bool {
    st.session.video.as_ref().is_some_and(|v| v.shown)
}

pub fn take_from_placed(st: &mut State, path: &Path, at: f64) {
    if st.session.video.is_none() && is_video_path(path) {
        st.session.video = Some(af2_core::session::VideoTrack { path: path.to_path_buf(), start: at, shown: true });
    }
}

fn texture_of(ctx: &egui::Context, name: String, f: &Frame) -> TextureHandle {
    let img = egui::ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.rgba);
    ctx.load_texture(name, img, egui::TextureOptions::LINEAR)
}

/// The Video panel.
pub fn panel(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    let cur = current(st);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Decode at").color(c(th.text_dim)));
        for s in [Scale::Full, Scale::Half, Scale::Quarter] {
            if ui
                .selectable_label(st.video.scale == s, s.label())
                .on_hover_text(match s {
                    Scale::Full => "Every pixel: the sharpest picture, the most work",
                    Scale::Half => "Half the width and height: a quarter of the work",
                    Scale::Quarter => "A quarter of the width and height: for a slow computer or a big video",
                })
                .clicked()
            {
                st.video.scale = s;
                let _ = std::fs::write(scale_path(), s.label());
            }
        }
        if let Some((_, i)) = &st.video.shown {
            ui.label(
                egui::RichText::new(format!("{}x{} · {:.3} fps", i.width, i.height, i.fps))
                    .small()
                    .color(c(th.text_dim)),
            );
        }
    });
    let area = ui.available_rect_before_wrap();
    let (rect, _) = ui.allocate_exact_size(area.size(), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, Rounding::same(3.0), Color32::BLACK);
    let note = |text: &str| {
        p.text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(12.0), c(th.text_dim));
    };
    let Some((path, t)) = cur else {
        note("Put a video file in a session, or open one in the Waveform editor");
        return;
    };
    let Some(player) = &st.video.player else { return };
    let (frame, version, err) = {
        let g = player.shared.0.lock().unwrap_or_else(|e| e.into_inner());
        (g.frame.clone(), g.version, g.info.as_ref().and_then(|i| i.as_ref().err()).cloned())
    };
    if let Some(e) = err {
        note(&e);
        return;
    }
    let Some(frame) = frame else {
        note("Loading...");
        return;
    };
    if st.video.texture.as_ref().map(|(v, _)| *v) != Some(version) {
        let tex = texture_of(ui.ctx(), "af2-video-frame".into(), &frame);
        st.video.texture = Some((version, tex));
    }
    let Some((_, tex)) = &st.video.texture else { return };
    // Fitted, with the picture's shape (the full-size one when known).
    let (fw, fh) = st
        .video
        .shown
        .as_ref()
        .filter(|(sp, _)| *sp == path)
        .map_or((frame.width as f32, frame.height as f32), |(_, i)| (i.width as f32, i.height as f32));
    let k = (rect.width() / fw).min(rect.height() / fh);
    let pic = Rect::from_center_size(rect.center(), vec2(fw * k, fh * k));
    p.image(tex.id(), pic, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    if t < 0.0 {
        p.rect_filled(pic, Rounding::ZERO, Color32::from_black_alpha(200));
        note("Before the video starts");
    }
}

/// The video row along the top of the Multitrack: its header and its
/// thumbnails on the timeline. Dragging the row moves the video.
pub fn row(
    ui: &mut egui::Ui,
    st: &mut State,
    head: Rect,
    lane: Rect,
    time_to_x: &dyn Fn(f64) -> f32,
    x_to_time: &dyn Fn(f32) -> f64,
) {
    let Some(v) = st.session.video.clone() else { return };
    let th = st.theme.clone();
    let p = ui.painter_at(head.union(lane));
    p.rect_filled(head, Rounding::ZERO, c(th.panel));
    p.rect_filled(Rect::from_min_size(head.min, vec2(4.0, head.height())), Rounding::ZERO, ca(th.accent_alt, 255));
    p.line_segment([head.left_bottom(), lane.right_bottom()], Stroke::new(1.0_f32, c(th.border)));
    let name = v.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mid = head.center().y;
    p.text(pos2(head.left() + 12.0, mid), egui::Align2::LEFT_CENTER, "VIDEO", egui::FontId::proportional(10.0), c(th.text_dim));
    let name_x = head.left() + 52.0;
    let name_clip = Rect::from_min_max(pos2(name_x, head.top()), pos2(head.right() - 26.0, head.bottom()));
    ui.painter_at(name_clip).text(pos2(name_x, mid), egui::Align2::LEFT_CENTER, &name, egui::FontId::proportional(12.0), c(th.text));
    // Remove.
    let x = Rect::from_center_size(pos2(head.right() - 15.0, mid), vec2(18.0, 18.0));
    let xr = ui.interact(x, ui.id().with("af2-video-remove"), Sense::click()).on_hover_text("Take the video out of the session (its sound stays)");
    p.text(x.center(), egui::Align2::CENTER_CENTER, "×", egui::FontId::proportional(14.0), if xr.hovered() { c(th.text) } else { c(th.text_dim) });
    if xr.clicked() {
        let before = st.session.clone();
        st.session.video = None;
        st.commit_session("Remove Video", before);
        return;
    }
    // Thumbnails.
    let thumbs = st
        .video
        .thumbs
        .entry(v.path.clone())
        .or_insert_with(|| {
            let t = Arc::new(Mutex::new(Thumbs::default()));
            thumbnails(v.path.clone(), t.clone(), ui.ctx().clone());
            t
        })
        .clone();
    let lp = ui.painter_at(lane);
    lp.rect_filled(lane, Rounding::ZERO, c(th.wave_background));
    let g = thumbs.lock().unwrap_or_else(|e| e.into_inner());
    let pic_h = lane.height() - 4.0;
    let mut drawn_to = f32::MIN;
    for (i, (t, f)) in g.pics.iter().enumerate() {
        let x0 = time_to_x(v.start + t);
        let w = pic_h * f.width as f32 / f.height.max(1) as f32;
        if x0 + w < lane.left() || x0 > lane.right() {
            continue;
        }
        // No overlaps when zoomed out: the next picture that fits.
        if x0 < drawn_to {
            continue;
        }
        let key = (v.path.clone(), i);
        let tex = st
            .video
            .thumb_tex
            .entry(key)
            .or_insert_with(|| texture_of(ui.ctx(), format!("af2-thumb-{i}"), f))
            .clone();
        let r = Rect::from_min_size(pos2(x0, lane.top() + 2.0), vec2(w, pic_h));
        lp.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        drawn_to = x0 + w + 1.0;
    }
    if let Some(e) = &g.error {
        lp.text(lane.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, e, egui::FontId::proportional(11.0), c(th.text_dim));
    }
    let end = g.pics.last().map(|(t, _)| *t + g.step).unwrap_or(0.0);
    drop(g);
    // Its span.
    let (x0, x1) = (time_to_x(v.start), time_to_x(v.start + end.max(0.1)));
    lp.rect_stroke(Rect::from_min_max(pos2(x0, lane.top() + 1.0), pos2(x1, lane.bottom() - 1.0)), Rounding::same(2.0), Stroke::new(1.0_f32, ca(th.accent_alt, 200)));
    // Drag to move it along the timeline.
    let r = ui.interact(lane, ui.id().with("af2-video-lane"), Sense::drag());
    let key = ui.id().with("af2-video-drag");
    if r.drag_started() {
        ui.ctx().data_mut(|d| d.insert_temp(key, (st.session.clone(), v.start)));
    }
    if r.dragged() {
        if let Some((_, start)) = ui.ctx().data(|d| d.get_temp::<(af2_core::Session, f64)>(key)) {
            let dt = x_to_time(lane.left() + r.drag_delta().x) - x_to_time(lane.left());
            let _ = start;
            if let Some(vt) = &mut st.session.video {
                vt.start = (vt.start + dt).max(0.0);
            }
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    if r.drag_stopped() {
        if let Some((before, _)) = ui.ctx().data_mut(|d| d.remove_temp::<(af2_core::Session, f64)>(key)) {
            st.commit_session("Move Video", before);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `AF2_VIDEO=<file> cargo test -p audiform2 -- --ignored player_follows --nocapture`
    #[test]
    #[ignore]
    fn player_follows_the_playhead() {
        let path = PathBuf::from(std::env::var("AF2_VIDEO").expect("AF2_VIDEO"));
        let p = Player::start(egui::Context::default());
        let shown = |p: &Player| p.shared.0.lock().unwrap().frame.as_ref().map(|f| f.time);
        let settle = |p: &Player, want: f64| {
            for _ in 0..400 {
                if shown(p).is_some_and(|t| (t - want).abs() < 0.05) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        };
        let want = Some(Want { path: path.clone(), width: Some(320) });
        // A jump, then playing on, then back.
        for (t, playing) in [(10.0, false), (10.5, true), (11.0, true), (12.2, true), (3.0, false)] {
            p.set(want.clone(), t, playing);
            let ok = settle(&p, t);
            eprintln!("asked {t:.2}: shows {:?}", shown(&p));
            assert!(ok, "at {t}");
        }
    }

    #[test]
    fn video_files_are_told_from_audio() {
        assert!(is_video_path(Path::new("a/b.MP4")));
        assert!(is_video_path(Path::new("clip.mkv")));
        assert!(!is_video_path(Path::new("song.wav")));
        assert!(!is_video_path(Path::new("song.m4a")));
    }

    #[test]
    fn a_placed_video_becomes_the_session_video_once() {
        let mut st = State::new();
        take_from_placed(&mut st, Path::new("C:/v/first.mp4"), 2.0);
        take_from_placed(&mut st, Path::new("C:/v/second.mov"), 5.0);
        take_from_placed(&mut st, Path::new("C:/v/song.wav"), 0.0);
        let v = st.session.video.as_ref().unwrap();
        assert_eq!((v.path.as_path(), v.start), (Path::new("C:/v/first.mp4"), 2.0));
    }
}
