// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Pre-render: a track's clips through its effects, rendered ahead.
//!
//! With Pre-render on, a track's clips (or its MIDI notes through its
//! instrument) and its effect rack are rendered in the background. Once the
//! render is ready, playback plays it in place of the clips and effects, so
//! heavy plug-ins rest. What comes after the effects stays live: the fader,
//! pan, mono, polarity, sends, output and the 3D Space spot.
//!
//! Each render is named after a fingerprint of everything that shapes the
//! track's sound before its fader (clips, notes, instrument, effects and
//! their automation, the audio used, the tempo, the rate). Any change gives
//! a new fingerprint: the live track plays again while the new render is
//! made and takes over when it is ready. Going back to an earlier state
//! reuses its render from the cache folder.
//!
//! Renders play from disk: each is kept in the cache folder as 24-bit
//! FLAC in chunks (`af2_io::chunked`, stored 24 dB down so peaks over
//! full scale survive) and streamed by `af2_engine::disk`, with only its
//! first moments and the few seconds around the playhead in memory.
//!
//! Ported from Audiform's pre-render (the project lead's own work there).

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use af2_core::automation::LaneTarget;
use af2_core::{AudioBuffer, Clip, ClipId, MediaId, MediaItem, Session, TrackId, TrackKind};

use crate::state::State;

/// Seconds rendered past the track's last sound, for effect tails.
const TAIL: f64 = 6.0;
/// How many renders the cache folder keeps at least, newest first...
const KEEP: usize = 24;
/// ...and beyond that, as many more as fit in this many bytes (a session
/// with many pre-rendered tracks keeps every track's render).
const KEEP_BYTES: u64 = 3_000_000_000;
/// Frames per chunk in a render's cache file.
const CHUNK: usize = 65_536;

/// A finished render: played from disk through `stream`, standing in the
/// mix as `stand_in` (an empty buffer the engine knows it by).
#[derive(Clone)]
pub struct Ready {
    pub frames: usize,
    pub stand_in: Arc<AudioBuffer>,
    pub stream: Arc<af2_engine::disk::DiskStream>,
}

/// Opens a render's cache file for streaming.
fn open_render(path: &Path) -> Result<Ready, String> {
    let c = af2_io::chunked::Chunked::open(path)?;
    let (rate, channels, frames, chunk) = (c.rate, c.channels, c.frames, c.chunk);
    let mut file: Option<std::fs::File> = None;
    let load = Box::new(move |k: usize| {
        if file.is_none() {
            file = std::fs::File::open(&c.path).ok();
        }
        c.read(file.as_mut()?, k).ok()
    });
    let stream = af2_engine::disk::DiskStream::open(rate, channels, frames, chunk, load)?;
    Ok(Ready {
        frames,
        stand_in: af2_engine::disk::stand_in(&stream),
        stream,
    })
}

/// Where one pre-rendered track stands.
#[derive(Default)]
struct Entry {
    /// The fingerprint being rendered now, if any.
    rendering: Option<u64>,
    /// How far that render is, in thousandths.
    progress: Arc<AtomicU32>,
    /// A finished render and its fingerprint.
    ready: Option<(u64, Ready)>,
    /// The fingerprint of the track as it is now (None: it cannot be
    /// rendered alone, or its audio is still loading).
    now: Option<u64>,
}

/// Every pre-rendered track's state (kept in [`State`]).
pub struct Prerender {
    entries: HashMap<TrackId, Entry>,
    tx: Sender<(TrackId, u64, Result<Ready, String>)>,
    rx: Receiver<(TrackId, u64, Result<Ready, String>)>,
    /// What the fingerprints were worked out from (session and files).
    checked: Option<(u64, u64)>,
    /// Bumped whenever what plays changes (a render came or went).
    pub version: u64,
}

impl Default for Prerender {
    fn default() -> Self {
        let (tx, rx) = channel();
        Prerender {
            entries: HashMap::new(),
            tx,
            rx,
            checked: None,
            version: 0,
        }
    }
}

/// Where a pre-rendered track stands, for the interface.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// Its render plays.
    Ready,
    /// Being rendered; the live track plays meanwhile.
    Rendering,
    /// On, but nothing to play yet (audio loading, or it cannot be
    /// rendered alone).
    Waiting,
}

/// The colour a pre-render state shows in: green rendered, amber
/// rendering, grey waiting. None when pre-render is off.
pub fn status_rgb(s: Option<Status>) -> Option<[u8; 3]> {
    s.map(|s| match s {
        Status::Ready => [90, 210, 120],
        Status::Rendering => [240, 175, 55],
        Status::Waiting => [170, 170, 170],
    })
}

/// What a pre-render state is called on a label: "Rendered",
/// "Rendering…", "Pre-render waiting".
pub fn status_label(s: Status) -> &'static str {
    match s {
        Status::Ready => "Rendered",
        Status::Rendering => "Rendering\u{2026}",
        Status::Waiting => "Pre-render waiting",
    }
}

impl Status {
    pub fn words(self) -> &'static str {
        match self {
            Status::Ready => "Pre-rendered: playback uses the rendered track, so its effects rest.",
            Status::Rendering => {
                "Rendering the track's clips and effects. The live track plays until it is ready."
            }
            Status::Waiting => "Pre-render is on and waiting to render.",
        }
    }
}

fn cache_dir() -> PathBuf {
    crate::state::data_dir().join("prerender")
}

/// The id a track's render goes by as a media item (never a real one's).
fn render_media(id: TrackId) -> MediaId {
    (1 << 62) | id
}

/// The track alone as its render hears it: clips, notes, instrument and
/// effects (with their automation), through nothing after the effects.
fn alone(s: &Session, id: TrackId) -> Session {
    let mut out = s.clone();
    out.tracks.retain(|t| t.id == id);
    for t in &mut out.tracks {
        t.mute = false;
        t.solo = false;
        t.volume_db = 0.0;
        t.pan = 0.0;
        t.mono = false;
        t.polarity = false;
        t.prerender = false;
        t.output = None;
        t.sends.clear();
        t.lanes
            .retain(|l| matches!(l.target, LaneTarget::Effect { .. }));
    }
    out.master_db = 0.0;
    out.master_pan = 0.0;
    out.master_mono = false;
    out.master_invert = false;
    out.master_chain.clear();
    out.master_rack = Default::default();
    out.master_lanes.clear();
    // Rendered as plain stereo; the room spot is applied live after it.
    out.space = None;
    out
}

/// The fingerprint of what shapes the track's sound before its fader, or
/// None when it cannot be rendered alone (a bus, an effect keyed by
/// another track) or `file` has no audio for one of its clips yet.
fn fingerprint(
    s: &Session,
    id: TrackId,
    file: &dyn Fn(MediaId) -> Option<(u64, usize)>,
) -> Option<u64> {
    let t = s.tracks.iter().find(|t| t.id == id)?;
    if t.kind == TrackKind::Bus || t.chain.iter().any(|c| c.sidechain.is_some()) {
        return None;
    }
    let mut used: Vec<(&MediaItem, (u64, usize))> = Vec::new();
    for m in s
        .media
        .iter()
        .filter(|m| t.clips.iter().any(|c| c.media == m.id))
    {
        used.push((m, file(m.id)?));
    }
    let lanes: Vec<_> = t
        .lanes
        .iter()
        .filter(|l| matches!(l.target, LaneTarget::Effect { .. }))
        .collect();
    let text = serde_json::to_string(&(
        &t.kind,
        &t.clips,
        &t.midi,
        &t.instrument,
        &t.soundfont,
        &t.chain,
        &t.rack,
        &lanes,
        &t.automation,
        used.iter()
            .map(|(m, f)| (&m.path, m.duration, m.rate, m.channels, f))
            .collect::<Vec<_>>(),
        s.rate,
        &s.tempo,
    ))
    .ok()?;
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    Some(h.finish())
}

impl State {
    /// Each pre-rendered track's state, by track.
    pub fn prerender_status(&self, id: TrackId) -> Option<Status> {
        let t = self.session.tracks.iter().find(|t| t.id == id)?;
        if !t.prerender {
            return None;
        }
        let e = self.prerender.entries.get(&id);
        Some(match e {
            Some(e) if e.now.is_some() && e.ready.as_ref().map(|r| r.0) == e.now => Status::Ready,
            Some(e) if e.rendering.is_some() => Status::Rendering,
            _ => Status::Waiting,
        })
    }

    /// The session as playback hears it: every pre-rendered track whose
    /// render is current plays that render in place of its clips and
    /// effects.
    pub fn heard_session(&self) -> Cow<'_, Session> {
        let s = &self.session;
        let ready: Vec<(TrackId, u64)> = s
            .tracks
            .iter()
            .filter(|t| t.prerender)
            .filter_map(|t| {
                let e = self.prerender.entries.get(&t.id)?;
                let (key, r) = e.ready.as_ref()?;
                (e.now == Some(*key)).then_some((t.id, r.frames as u64))
            })
            .collect();
        if ready.is_empty() {
            return Cow::Borrowed(s);
        }
        let mut out = s.clone();
        for (id, frames) in ready {
            let media = render_media(id);
            let len = frames as f64 / s.rate as f64;
            let Some(t) = out.tracks.iter_mut().find(|t| t.id == id) else {
                continue;
            };
            t.clips = vec![Clip {
                id: (1 << 62) | id as ClipId,
                media,
                name: format!("{} (pre-rendered)", t.name),
                start: 0.0,
                offset: 0.0,
                length: len,
                gain_db: 0.0,
                fade_in: 0.0,
                fade_out: 0.0,
                muted: false,
                props: Default::default(),
                ext_id: None,
            }];
            t.midi.clear();
            t.kind = TrackKind::Audio;
            t.chain.clear();
            t.rack = Default::default();
            t.lanes
                .retain(|l| !matches!(l.target, LaneTarget::Effect { .. }));
            let name = format!("{} (pre-rendered)", t.name);
            out.media.push(MediaItem {
                id: media,
                name,
                path: None,
                duration: len,
                rate: s.rate,
                channels: 2,
                ext_id: None,
            });
        }
        Cow::Owned(out)
    }

    /// The audio playback uses for media `m`: a track's render, or the file.
    pub fn heard_media(&self, m: MediaId) -> Option<Arc<AudioBuffer>> {
        if m >> 62 == 1 {
            let id = m & !(1 << 62);
            return self
                .prerender
                .entries
                .get(&id)
                .and_then(|e| e.ready.as_ref().map(|r| r.1.stand_in.clone()));
        }
        self.media_buffer(m)
    }

    /// How far a pre-rendered track's render is (0 to 1), while it is
    /// being rendered.
    pub fn prerender_progress(&self, id: TrackId) -> Option<f32> {
        let e = self.prerender.entries.get(&id)?;
        e.rendering?;
        Some(e.progress.load(Ordering::Relaxed) as f32 / 1000.0)
    }

    /// Picks up finished renders and starts one for each pre-rendered
    /// track whose sound changed. Cheap when nothing changed.
    pub fn prerender_poll(&mut self) {
        self.prerender_hint();
        let mut moved = false;
        while let Ok((id, key, done)) = self.prerender.rx.try_recv() {
            let Some(e) = self.prerender.entries.get_mut(&id) else {
                continue;
            };
            if e.rendering == Some(key) {
                e.rendering = None;
            }
            match done {
                Ok(buf) => {
                    e.ready = Some((key, buf));
                    moved = true;
                }
                Err(err) => eprintln!("pre-render failed: {err}"),
            }
        }
        let any = self.session.tracks.iter().any(|t| t.prerender);
        if !any && self.prerender.entries.is_empty() {
            if moved {
                self.prerender.version += 1;
            }
            return;
        }
        let files = {
            let mut h = DefaultHasher::new();
            for d in &self.docs {
                (d.id, d.version).hash(&mut h);
            }
            let mut m: Vec<_> = self.media_docs.iter().collect();
            m.sort();
            m.hash(&mut h);
            h.finish()
        };
        let check = (self.session_version, files);
        if self.prerender.checked == Some(check) {
            if moved {
                self.prerender.version += 1;
            }
            return;
        }
        self.prerender.checked = Some(check);
        let before: Vec<(TrackId, Option<u64>)> = {
            let mut v: Vec<_> = self
                .prerender
                .entries
                .iter()
                .map(|(id, e)| (*id, e.now))
                .collect();
            v.sort();
            v
        };
        let on: Vec<TrackId> = self
            .session
            .tracks
            .iter()
            .filter(|t| t.prerender)
            .map(|t| t.id)
            .collect();
        self.prerender.entries.retain(|id, _| on.contains(id));
        for id in on {
            let file = |m: MediaId| {
                let d = self.media_docs.get(&m)?;
                let doc = self.doc_by_id(*d)?;
                Some((doc.version, doc.buf.frames()))
            };
            let key = fingerprint(&self.session, id, &file);
            let e = self.prerender.entries.entry(id).or_default();
            e.now = key;
            let Some(key) = key else { continue };
            if e.ready.as_ref().is_some_and(|r| r.0 == key) || e.rendering == Some(key) {
                continue;
            }
            let path = cache_dir().join(format!("{key:016x}.afr"));
            if let Ok(r) = open_render(&path) {
                if r.stream.rate == self.session.rate {
                    // Used again: it counts as new, so tidying keeps it.
                    touch(&path);
                    e.ready = Some((key, r));
                    moved = true;
                    continue;
                }
            }
            e.rendering = Some(key);
            e.progress = Arc::new(AtomicU32::new(0));
            let progress = e.progress.clone();
            let job = alone(&self.session, id);
            let media: HashMap<MediaId, Arc<AudioBuffer>> = job
                .media
                .iter()
                .filter_map(|m| self.media_buffer(m.id).map(|b| (m.id, b)))
                .collect();
            let tx = self.prerender.tx.clone();
            std::thread::spawn(move || {
                // A few at a time: a session opening with every track
                // pre-rendered would otherwise render them all at once
                // (every core busy, and each render's buffer in memory).
                af2_engine::pool::background_priority();
                let _turn = render_turn();
                af2_engine::denormal::flush_to_zero();
                let done = render(&job, &media, &path, &progress);
                prune(&cache_dir());
                let _ = tx.send((id, key, done));
            });
        }
        let after: Vec<(TrackId, Option<u64>)> = {
            let mut v: Vec<_> = self
                .prerender
                .entries
                .iter()
                .map(|(id, e)| (*id, e.now))
                .collect();
            v.sort();
            v
        };
        if moved || before != after {
            self.prerender.version += 1;
        }
    }
}

impl State {
    /// Tells each playing render where playback will go next, so its
    /// audio is loaded before then: the playhead while stopped, the loop
    /// start while looping.
    fn prerender_hint(&self) {
        if self.prerender.entries.is_empty() {
            return;
        }
        let rate = self.session.rate as f64;
        let at = if !self.is_playing() {
            Some(self.session_cursor)
        } else if self.play_mode() == crate::transport::PlayMode::Loop {
            self.session_sel.map(|s| s.0)
        } else {
            None
        };
        let frame = at.map(|t| (t.max(0.0) * rate) as usize);
        for e in self.prerender.entries.values() {
            if let Some((_, r)) = &e.ready {
                r.stream.hint(frame);
            }
        }
    }

    /// Bytes the renders hold in memory now.
    pub fn prerender_bytes(&self) -> usize {
        self.prerender
            .entries
            .values()
            .filter_map(|e| e.ready.as_ref())
            .map(|r| r.1.stream.bytes())
            .sum()
    }
}

/// Renders one track alone (stereo at the session rate from the start),
/// saves it to the cache folder and opens it to stream from there.
fn render(
    job: &Session,
    media: &HashMap<MediaId, Arc<AudioBuffer>>,
    path: &Path,
    progress: &AtomicU32,
) -> Result<Ready, String> {
    let rate = job.rate;
    let program = af2_engine::mix_program(job, &|m| media.get(&m).cloned(), rate);
    let to = job.duration() + TAIL;
    let total = ((to * rate as f64).round() as usize).max(1);
    let mut inter = Vec::with_capacity(total * 2);
    af2_engine::render_each(program, rate, 0.0, to, false, |b| {
        inter.extend_from_slice(b);
        let done = (inter.len() / 2) as u64 * 1000 / total as u64;
        progress.store(done.min(999) as u32, Ordering::Relaxed);
        true
    });
    let buf = AudioBuffer::from_interleaved(rate, 2, &inter);
    drop(inter);
    af2_io::chunked::write(path, &buf, CHUNK)?;
    drop(buf);
    open_render(path)
}

/// Marks a render file as just used.
fn touch(path: &Path) {
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

/// A turn to render: at most half the processor's cores (one to four)
/// render at once; the rest wait.
fn render_turn() -> impl Drop {
    use std::sync::{Condvar, Mutex, OnceLock};
    static GATE: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();
    let gate = GATE.get_or_init(|| (Mutex::new(0), Condvar::new()));
    let most = std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(1, 4);
    let mut busy = gate.0.lock().unwrap_or_else(|e| e.into_inner());
    while *busy >= most {
        busy = gate.1.wait(busy).unwrap_or_else(|e| e.into_inner());
    }
    *busy += 1;
    struct Turn(&'static (Mutex<usize>, Condvar));
    impl Drop for Turn {
        fn drop(&mut self) {
            let mut busy = self.0 .0.lock().unwrap_or_else(|e| e.into_inner());
            *busy = busy.saturating_sub(1);
            self.0 .1.notify_one();
        }
    }
    Turn(gate)
}

/// Removes old renders: the newest [`KEEP`] stay, and more while they fit
/// in [`KEEP_BYTES`].
fn prune(dir: &Path) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf, u64)> = Vec::new();
    for e in read.flatten() {
        let p = e.path();
        match p.extension().and_then(|x| x.to_str()) {
            Some("afr") => {
                if let Some(m) = e.metadata().ok() {
                    if let Ok(t) = m.modified() {
                        files.push((t, p, m.len()));
                    }
                }
            }
            // Renders from before they streamed (32-bit float WAV).
            Some("wav") => {
                let _ = std::fs::remove_file(p);
            }
            _ => {}
        }
    }
    files.sort_by_key(|a| std::cmp::Reverse(a.0));
    let mut total = 0u64;
    for (k, (_, path, len)) in files.into_iter().enumerate() {
        total += len;
        if k >= KEEP && total > KEEP_BYTES {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> (Session, TrackId) {
        let mut s = Session::new("s", 48_000, 0);
        let id = s.add_track_of(TrackKind::Audio);
        let t = s.track_mut(id).unwrap();
        t.prerender = true;
        t.volume_db = -6.0;
        t.pan = 0.5;
        (s, id)
    }

    #[test]
    fn the_fingerprint_follows_the_sound_not_the_fader() {
        let (mut s, id) = session();
        let none = |_: MediaId| None;
        let a = fingerprint(&s, id, &none).unwrap();
        let t = s.track_mut(id).unwrap();
        t.volume_db = 3.0;
        t.pan = -1.0;
        t.mute = true;
        t.height = 200.0;
        assert_eq!(fingerprint(&s, id, &none), Some(a));
        s.track_mut(id)
            .unwrap()
            .chain
            .push(crate::rack::new_slot("amplify"));
        assert_ne!(fingerprint(&s, id, &none), Some(a));
    }

    #[test]
    fn a_ready_render_stands_in_for_the_clips_and_effects() {
        let (s, id) = session();
        let mut st = State::new();
        st.session = s;
        st.session
            .track_mut(id)
            .unwrap()
            .chain
            .push(crate::rack::new_slot("amplify"));
        let key = fingerprint(&st.session, id, &|_| None).unwrap();
        let dir = std::env::temp_dir().join(format!("af2-pr-test-{}", std::process::id()));
        let path = dir.join("r.afr");
        af2_io::chunked::write(&path, &AudioBuffer::new(48_000, 2, 96_000), CHUNK).unwrap();
        let ready = open_render(&path).unwrap();
        st.prerender.entries.insert(
            id,
            Entry {
                rendering: None,
                ready: Some((key, ready)),
                now: Some(key),
                ..Default::default()
            },
        );
        let h = st.heard_session();
        let t = h.tracks.iter().find(|t| t.id == id).unwrap();
        assert_eq!(t.clips.len(), 1);
        assert!((t.clips[0].length - 2.0).abs() < 1e-9);
        assert!(t.chain.is_empty());
        // What comes after the effects stays.
        assert_eq!((t.volume_db, t.pan), (-6.0, 0.5));
        let m = t.clips[0].media;
        assert!(h.media.iter().any(|x| x.id == m));
        // The mix gets a stand-in for the stream that plays it from disk.
        let stand_in = st.heard_media(m).unwrap();
        let stream = af2_engine::disk::stream_for(&stand_in).expect("streamed");
        assert_eq!(stream.frames, 96_000);
        assert_eq!(st.prerender_status(id), Some(Status::Ready));
        // A change to the sound: the live track plays again.
        drop(h);
        st.prerender.entries.get_mut(&id).unwrap().now = Some(key ^ 1);
        assert!(matches!(st.heard_session(), Cow::Borrowed(_)));
    }

    #[test]
    fn a_render_shows_progress_then_rendered_until_the_track_changes() {
        let mut st = State::new();
        let mut b = AudioBuffer::new(48_000, 1, 24_000);
        for (i, x) in b.channels[0].iter_mut().enumerate() {
            *x = (i as f32 * 0.05).sin() * 0.3;
        }
        let doc = st.add_doc("pr.wav".into(), None, b);
        let m = st.media_for_doc(doc).unwrap();
        let id = st.session.tracks[0].id;
        // A start of its own each run, so no earlier run's render is in
        // the cache folder to be found ready at once.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        st.session.add_clip(id, m, 0.25 + nanos as f64 * 1e-10).unwrap();
        st.session.track_mut(id).unwrap().prerender = true;
        st.session_version += 1;
        let wait = |st: &mut State| {
            for _ in 0..1000 {
                st.prerender_poll();
                if st.prerender_status(id) == Some(Status::Ready) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("never rendered");
        };
        st.prerender_poll();
        let s = st.prerender_status(id);
        assert!(matches!(s, Some(Status::Rendering) | Some(Status::Ready)), "{s:?}");
        if s == Some(Status::Rendering) {
            let f = st.prerender_progress(id).expect("progress while rendering");
            assert!((0.0..=1.0).contains(&f));
        }
        wait(&mut st);
        assert_eq!(st.prerender_progress(id), None, "no progress once rendered");
        // A change to what the track sounds like takes "rendered" away.
        st.session
            .track_mut(id)
            .unwrap()
            .chain
            .push(crate::rack::new_slot("amplify"));
        st.session_version += 1;
        st.prerender_poll();
        assert_ne!(st.prerender_status(id), Some(Status::Ready));
        wait(&mut st);
        // Off: no state at all.
        st.session.track_mut(id).unwrap().prerender = false;
        st.session_version += 1;
        st.prerender_poll();
        assert_eq!(st.prerender_status(id), None);
        assert_eq!(st.prerender_progress(id), None);
    }

    #[test]
    fn a_keyed_effect_or_a_bus_is_not_rendered_alone() {
        let (mut s, id) = session();
        let mut slot = crate::rack::new_slot("compressor");
        slot.sidechain = Some(id);
        s.track_mut(id).unwrap().chain.push(slot);
        assert_eq!(fingerprint(&s, id, &|_| None), None);
    }
}
