// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Audio that plays from disk (pre-rendered tracks), so it need not sit
//! in memory.
//!
//! A [`DiskStream`] keeps the first [`HEAD_SECS`] in memory (playback
//! from the start is instant) and has a reader thread that loads the
//! chunks around each place being played, a few seconds ahead, and around
//! a hint the interface gives (the playhead while stopped, the loop
//! start), so a jump is usually ready before Play.
//!
//! A mix clip plays through its own [`StreamReader`]. On the audio thread
//! it never waits, locks only with `try_lock`, never allocates and never
//! frees: chunks it lets go of go back to the reader thread through a
//! ring. A chunk that is not loaded yet plays as silence (counted in
//! [`DiskStream::misses`]). Offline renders read with `blocking` and wait
//! for each chunk instead.
//!
//! The stream does not know the file format: the app gives it a loader
//! (chunk index to audio, one Vec per channel).

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use af2_core::AudioBuffer;

/// Seconds kept in memory from the start.
pub const HEAD_SECS: f64 = 1.5;
/// Chunks loaded ahead of each place being played.
const AHEAD: usize = 6;
/// Chunks a reader holds for itself.
const LOCAL: usize = 4;
/// Frames a reader hands the mix at once.
pub const SCRATCH: usize = 8192;
/// No one wants this place.
const NONE: usize = usize::MAX;

/// Loads chunk `k`: one Vec per channel (the last chunk may be short).
pub type Loader = Box<dyn FnMut(usize) -> Option<Vec<Vec<f32>>> + Send>;

pub struct Chunk(Vec<Vec<f32>>);

/// Where one reader (or the hint) is: a chunk index, or [`NONE`].
struct Want(AtomicUsize);

pub struct DiskStream {
    pub rate: u32,
    pub channels: usize,
    pub frames: usize,
    pub chunk: usize,
    head: Vec<Vec<f32>>,
    head_frames: usize,
    map: Mutex<HashMap<usize, Arc<Chunk>>>,
    wants: Mutex<Vec<Arc<Want>>>,
    hint: Arc<Want>,
    returns: Mutex<Vec<rtrb::Consumer<Arc<Chunk>>>>,
    wake: (Mutex<bool>, Condvar),
    /// Blocks played as silence because their chunk was not loaded.
    pub misses: AtomicU64,
    dead: AtomicBool,
}

impl DiskStream {
    /// Opens a stream of `frames` frames in chunks of `chunk`, loading the
    /// head now (on the calling thread) and starting the reader thread.
    pub fn open(
        rate: u32,
        channels: usize,
        frames: usize,
        chunk: usize,
        mut load: Loader,
    ) -> Result<Arc<DiskStream>, String> {
        let chunk = chunk.max(1);
        let head_chunks = ((HEAD_SECS * rate as f64) as usize).div_ceil(chunk);
        let mut head: Vec<Vec<f32>> = vec![Vec::new(); channels];
        let mut head_frames = 0;
        for k in 0..head_chunks.min(frames.div_ceil(chunk)) {
            let data = load(k).ok_or("the render could not be read")?;
            for (h, d) in head.iter_mut().zip(&data) {
                h.extend_from_slice(d);
            }
            head_frames += data.first().map_or(0, Vec::len);
        }
        head.iter_mut().for_each(|h| h.shrink_to_fit());
        let s = Arc::new(DiskStream {
            rate,
            channels,
            frames,
            chunk,
            head,
            head_frames,
            map: Mutex::new(HashMap::new()),
            wants: Mutex::new(Vec::new()),
            hint: Arc::new(Want(AtomicUsize::new(NONE))),
            returns: Mutex::new(Vec::new()),
            wake: (Mutex::new(false), Condvar::new()),
            misses: AtomicU64::new(0),
            dead: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&s);
        std::thread::Builder::new()
            .name("af2-disk".into())
            .spawn(move || reader_thread(weak, load))
            .map_err(|e| e.to_string())?;
        Ok(s)
    }

    /// The place the interface expects to play from next (the playhead
    /// while stopped, a loop's start), in frames; None for nowhere.
    pub fn hint(&self, frame: Option<usize>) {
        let k = frame.map_or(NONE, |f| f / self.chunk);
        if self.hint.0.swap(k, Ordering::Relaxed) != k {
            self.poke();
        }
    }

    fn poke(&self) {
        if let Ok(mut w) = self.wake.0.try_lock() {
            *w = true;
            self.wake.1.notify_one();
        }
    }

    /// A reader for one mix clip.
    pub fn reader(self: &Arc<Self>) -> StreamReader {
        let want = Arc::new(Want(AtomicUsize::new(NONE)));
        self.wants.lock().unwrap_or_else(|e| e.into_inner()).push(want.clone());
        let (tx, rx) = rtrb::RingBuffer::new(64);
        self.returns.lock().unwrap_or_else(|e| e.into_inner()).push(rx);
        StreamReader {
            s: self.clone(),
            want,
            local: Default::default(),
            ret: tx,
            scratch: AudioBuffer {
                rate: self.rate,
                channels: (0..self.channels)
                    .map(|_| Vec::with_capacity(SCRATCH + 2))
                    .collect(),
            },
            blocking: false,
        }
    }

    /// Bytes in memory now (head and loaded chunks).
    pub fn bytes(&self) -> usize {
        let chunks: usize = self
            .map
            .lock()
            .map(|m| m.values().map(|c| c.0.iter().map(Vec::len).sum::<usize>()).sum())
            .unwrap_or(0);
        (self.head_frames * self.channels + chunks) * 4
    }
}

impl Drop for DiskStream {
    fn drop(&mut self) {
        self.dead.store(true, Ordering::Relaxed);
    }
}

fn reader_thread(weak: Weak<DiskStream>, mut load: Loader) {
    loop {
        let Some(s) = weak.upgrade() else { return };
        if s.dead.load(Ordering::Relaxed) {
            return;
        }
        // Chunks readers let go of are freed here.
        {
            let mut rets = s.returns.lock().unwrap_or_else(|e| e.into_inner());
            for r in rets.iter_mut() {
                while r.pop().is_ok() {}
            }
            rets.retain(|r| !(r.is_abandoned() && r.is_empty()));
        }
        let count = s.frames.div_ceil(s.chunk);
        let first_streamed = s.head_frames.div_ceil(s.chunk);
        let places: Vec<usize> = {
            let mut w = s.wants.lock().unwrap_or_else(|e| e.into_inner());
            w.retain(|x| Arc::strong_count(x) > 1);
            w.iter()
                .chain(std::iter::once(&s.hint))
                .map(|x| x.0.load(Ordering::Relaxed))
                .filter(|&k| k != NONE)
                .collect()
        };
        // The chunks wanted, nearest first for each place.
        let mut order: Vec<usize> = Vec::new();
        for d in 0..=AHEAD {
            for &p in &places {
                let k = p + d;
                if k < count && k >= first_streamed && !order.contains(&k) {
                    order.push(k);
                }
            }
        }
        for &p in &places {
            if p > 0 && p - 1 >= first_streamed && p - 1 < count && !order.contains(&(p - 1)) {
                order.push(p - 1);
            }
        }
        let keep: BTreeSet<usize> = order.iter().copied().collect();
        let missing = {
            let mut m = s.map.lock().unwrap_or_else(|e| e.into_inner());
            m.retain(|k, _| keep.contains(k));
            order.iter().copied().find(|k| !m.contains_key(k))
        };
        if let Some(k) = missing {
            match load(k) {
                Some(data) => {
                    s.map
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(k, Arc::new(Chunk(data)));
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
            continue;
        }
        let (lock, cv) = &s.wake;
        let mut w = lock.lock().unwrap_or_else(|e| e.into_inner());
        if !*w {
            w = cv
                .wait_timeout(w, Duration::from_millis(8))
                .map(|r| r.0)
                .unwrap_or_else(|e| e.into_inner().0);
        }
        *w = false;
    }
}

/// One mix clip's view of a [`DiskStream`].
pub struct StreamReader {
    s: Arc<DiskStream>,
    want: Arc<Want>,
    local: [Option<(usize, Arc<Chunk>)>; LOCAL],
    ret: rtrb::Producer<Arc<Chunk>>,
    scratch: AudioBuffer,
    /// Wait for chunks instead of playing silence (offline renders only;
    /// never on the audio thread).
    pub blocking: bool,
}

impl StreamReader {
    pub fn stream(&self) -> &Arc<DiskStream> {
        &self.s
    }

    /// Chunk `k`, from this reader's own few or the stream's (taken if
    /// the reader thread is not holding them right now).
    fn chunk(&mut self, k: usize) -> Option<usize> {
        if let Some(i) = self.local.iter().position(|e| e.as_ref().is_some_and(|e| e.0 == k)) {
            return Some(i);
        }
        let got = match self.s.map.try_lock() {
            Ok(m) => m.get(&k).cloned(),
            Err(_) => None,
        }?;
        // The slot whose chunk is farthest from this one.
        let i = (0..LOCAL)
            .max_by_key(|&i| self.local[i].as_ref().map_or(usize::MAX, |e| e.0.abs_diff(k)))
            .unwrap_or(0);
        if let Some(old) = self.local[i].replace((k, got)) {
            if let Err(rtrb::PushError::Full(old)) = self.ret.push(old.1) {
                // Never free here (this may be the audio thread).
                std::mem::forget(old);
            }
        }
        Some(i)
    }

    /// Frames `start..start + n` (n at most [`SCRATCH`] + 2), silent past
    /// the end and where a chunk is not loaded yet.
    pub fn fill(&mut self, start: usize, n: usize) -> &AudioBuffer {
        let n = n.min(SCRATCH + 2);
        let ch = self.s.channels;
        let end = start + n;
        let k0 = start / self.s.chunk;
        if self.want.0.swap(k0, Ordering::Relaxed) != k0 {
            self.s.poke();
        }
        for c in &mut self.scratch.channels {
            c.clear();
            c.resize(n, 0.0);
        }
        let mut f = start;
        let mut missed = false;
        while f < end && f < self.s.frames {
            if f < self.s.head_frames {
                let to = end.min(self.s.head_frames);
                for c in 0..ch {
                    self.scratch.channels[c][f - start..to - start]
                        .copy_from_slice(&self.s.head[c][f..to]);
                }
                f = to;
                continue;
            }
            let k = f / self.s.chunk;
            let base = k * self.s.chunk;
            let to = end.min(base + self.s.chunk).min(self.s.frames);
            let mut slot = self.chunk(k);
            if slot.is_none() && self.blocking {
                for _ in 0..5000 {
                    self.s.poke();
                    std::thread::sleep(Duration::from_millis(1));
                    slot = self.chunk(k);
                    if slot.is_some() {
                        break;
                    }
                }
            }
            match slot {
                Some(i) => {
                    let data = &self.local[i].as_ref().unwrap().1 .0;
                    for c in 0..ch {
                        let src = &data[c];
                        let a = (f - base).min(src.len());
                        let b = (to - base).min(src.len());
                        self.scratch.channels[c][f - start..f - start + (b - a)]
                            .copy_from_slice(&src[a..b]);
                    }
                }
                None => missed = true,
            }
            f = to;
        }
        if missed {
            self.s.misses.fetch_add(1, Ordering::Relaxed);
        }
        &self.scratch
    }
}

/// Streams by the buffer that stands for them in a mix: the app gives
/// the mix an empty buffer per stream (same rate and channels) and the
/// mix asks here for the stream behind it.
fn registry() -> &'static Mutex<Vec<(Weak<AudioBuffer>, Arc<DiskStream>)>> {
    static R: std::sync::OnceLock<Mutex<Vec<(Weak<AudioBuffer>, Arc<DiskStream>)>>> =
        std::sync::OnceLock::new();
    R.get_or_init(|| Mutex::new(Vec::new()))
}

/// An empty buffer that stands for `s` in a mix (keep it while the
/// stream should play; the stream is let go with the last copy).
pub fn stand_in(s: &Arc<DiskStream>) -> Arc<AudioBuffer> {
    let buf = Arc::new(AudioBuffer {
        rate: s.rate,
        channels: vec![Vec::new(); s.channels],
    });
    let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
    r.retain(|(w, _)| w.strong_count() > 0);
    r.push((Arc::downgrade(&buf), s.clone()));
    buf
}

/// The stream `buf` stands for, if any.
pub fn stream_for(buf: &Arc<AudioBuffer>) -> Option<Arc<DiskStream>> {
    if buf.frames() > 0 {
        return None;
    }
    let r = registry().lock().unwrap_or_else(|e| e.into_inner());
    r.iter()
        .find(|(w, _)| w.upgrade().is_some_and(|b| Arc::ptr_eq(&b, buf)))
        .map(|(_, s)| s.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(frames: usize, chunk: usize) -> (Arc<DiskStream>, Arc<AtomicUsize>) {
        let loads = Arc::new(AtomicUsize::new(0));
        let l2 = loads.clone();
        let s = DiskStream::open(
            1000,
            2,
            frames,
            chunk,
            Box::new(move |k| {
                l2.fetch_add(1, Ordering::Relaxed);
                let a = k * chunk;
                let b = (a + chunk).min(frames);
                Some(vec![
                    (a..b).map(|f| f as f32).collect(),
                    (a..b).map(|f| -(f as f32)).collect(),
                ])
            }),
        )
        .unwrap();
        (s, loads)
    }

    #[test]
    fn the_head_plays_at_once_and_later_chunks_arrive() {
        let (s, _) = ramp(20_000, 500);
        assert_eq!(s.head_frames, 1500);
        let mut r = s.reader();
        let b = r.fill(1490, 20);
        assert_eq!(b.channels[0][0], 1490.0);
        // Past the head: silence until the reader thread has it.
        let mut got = false;
        for _ in 0..500 {
            let b = r.fill(9_000, 10);
            if b.channels[0][9] == 9_009.0 {
                assert_eq!(b.channels[1][9], -9_009.0);
                got = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(got, "the chunk came");
    }

    #[test]
    fn a_blocking_reader_waits_and_crosses_chunks() {
        let (s, _) = ramp(20_000, 500);
        let mut r = s.reader();
        r.blocking = true;
        let b = r.fill(7_990, 30);
        let want: Vec<f32> = (7_990..8_020).map(|f| f as f32).collect();
        assert_eq!(b.channels[0], want);
        let b = r.fill(19_990, 30);
        assert_eq!(b.channels[0][9], 19_999.0);
        assert_eq!(b.channels[0][10], 0.0, "silent past the end");
    }

    #[test]
    fn the_hint_loads_ahead_and_far_chunks_are_let_go() {
        let (s, _) = ramp(100_000, 500);
        s.hint(Some(50_000));
        for _ in 0..500 {
            if s.map.lock().unwrap().contains_key(&(50_000 / 500 + 3)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(s.map.lock().unwrap().contains_key(&100));
        s.hint(Some(90_000));
        for _ in 0..500 {
            if !s.map.lock().unwrap().contains_key(&100) {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!s.map.lock().unwrap().contains_key(&100));
        assert!(s.bytes() < 100_000 * 2 * 4 / 4);
    }

    #[test]
    fn a_stand_in_finds_its_stream() {
        let (s, _) = ramp(5_000, 500);
        let b = stand_in(&s);
        assert!(stream_for(&b).is_some_and(|x| Arc::ptr_eq(&x, &s)));
        let other = Arc::new(AudioBuffer::new(1000, 2, 0));
        assert!(stream_for(&other).is_none());
    }
}
