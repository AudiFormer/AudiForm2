// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

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
//! Ported from Audiform's pre-render (the project lead's own work there).

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use af27_core::automation::LaneTarget;
use af27_core::{AudioBuffer, Clip, ClipId, MediaId, MediaItem, Session, TrackId, TrackKind};

use crate::state::State;

/// Seconds rendered past the track's last sound, for effect tails.
const TAIL: f64 = 6.0;
/// How many renders the cache folder keeps, newest first.
const KEEP: usize = 24;

/// Where one pre-rendered track stands.
#[derive(Default)]
struct Entry {
    /// The fingerprint being rendered now, if any.
    rendering: Option<u64>,
    /// A finished render and its fingerprint.
    ready: Option<(u64, Arc<AudioBuffer>)>,
    /// The fingerprint of the track as it is now (None: it cannot be
    /// rendered alone, or its audio is still loading).
    now: Option<u64>,
}

/// Every pre-rendered track's state (kept in [`State`]).
pub struct Prerender {
    entries: HashMap<TrackId, Entry>,
    tx: Sender<(TrackId, u64, Result<Arc<AudioBuffer>, String>)>,
    rx: Receiver<(TrackId, u64, Result<Arc<AudioBuffer>, String>)>,
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
                let (key, buf) = e.ready.as_ref()?;
                (e.now == Some(*key)).then_some((t.id, buf.frames() as u64))
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
                .and_then(|e| e.ready.as_ref().map(|r| r.1.clone()));
        }
        self.media_buffer(m)
    }

    /// Picks up finished renders and starts one for each pre-rendered
    /// track whose sound changed. Cheap when nothing changed.
    pub fn prerender_poll(&mut self) {
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
            let path = cache_dir().join(format!("{key:016x}.wav"));
            if let Ok(buf) = af27_io::decode_file(&path) {
                if buf.rate == self.session.rate {
                    e.ready = Some((key, Arc::new(buf)));
                    moved = true;
                    continue;
                }
            }
            e.rendering = Some(key);
            let job = alone(&self.session, id);
            let media: HashMap<MediaId, Arc<AudioBuffer>> = job
                .media
                .iter()
                .filter_map(|m| self.media_buffer(m.id).map(|b| (m.id, b)))
                .collect();
            let tx = self.prerender.tx.clone();
            std::thread::spawn(move || {
                let done = render(&job, &media, &path);
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

/// Renders one track alone: stereo at the session rate from the start,
/// kept as a 32-bit float file in the cache folder for later.
fn render(
    job: &Session,
    media: &HashMap<MediaId, Arc<AudioBuffer>>,
    path: &Path,
) -> Result<Arc<AudioBuffer>, String> {
    let rate = job.rate;
    let program = af27_engine::mix_program(job, &|m| media.get(&m).cloned(), rate);
    let to = job.duration() + TAIL;
    let buf = af27_engine::render(program, rate, 0.0, to);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("part.wav");
    if af27_io::write_wav(&tmp, &buf, af27_io::WavFormat::Float32).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
    Ok(Arc::new(buf))
}

/// Removes all but the newest [`KEEP`] renders.
fn prune(dir: &Path) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = read
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "wav"))
        .filter(|e| !e.path().to_string_lossy().ends_with(".part.wav"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (_, path) in files.into_iter().skip(KEEP) {
        let _ = std::fs::remove_file(path);
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
        let buf = Arc::new(AudioBuffer::new(48_000, 2, 96_000));
        st.prerender.entries.insert(
            id,
            Entry {
                rendering: None,
                ready: Some((key, buf)),
                now: Some(key),
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
        assert_eq!(st.heard_media(m).map(|b| b.frames()), Some(96_000));
        assert_eq!(st.prerender_status(id), Some(Status::Ready));
        // A change to the sound: the live track plays again.
        drop(h);
        st.prerender.entries.get_mut(&id).unwrap().now = Some(key ^ 1);
        assert!(matches!(st.heard_session(), Cow::Borrowed(_)));
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
