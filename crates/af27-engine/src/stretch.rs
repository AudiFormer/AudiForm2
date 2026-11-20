// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Stretched and pitch-shifted clip audio, worked out in the background.
//!
//! A clip whose stretch is on plays from a [`Prepared`] buffer: its whole
//! timeline span, already stretched, shifted and (if the clip is
//! reversed) turned round, at the source's rate. Building a mix asks
//! [`prepare`] for each such clip; that finds the buffer in a cache keyed
//! by the source audio, the span and every setting that changes the
//! sound, or makes a new entry and queues it for the worker threads.
//!
//! The audio thread only ever looks at [`Prepared::ready`], a lock-free
//! read: until the buffer is there the clip plays straight from its
//! source at the stretched speed (tape style, right timing), and from the
//! next block on it plays the finished audio. Live and Rendered share the
//! same entries, so they sound the same. Offline renders call
//! [`Prepared::ensure`] and wait for (or do) the work themselves.
//!
//! The UI can poll [`version`] (bumped whenever a buffer finishes) to
//! redraw waveforms, and [`pending`] to show that work is going on.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use af27_core::session::{Clip, StretchKind};
use af27_core::AudioBuffer;
use af27_dsp::stretch::{render, Params};

/// Everything that changes a prepared buffer's sound.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Key {
    /// The source buffer (held alive by the entry, so never reused).
    src: usize,
    offset: u64,
    length: u64,
    ratio: u64,
    pitch: u32,
    kind: u8,
    window: u32,
    transients: u32,
    formants: bool,
    precise: bool,
    reversed: bool,
}

/// One clip's stretched audio, ready or on its way.
pub struct Prepared {
    key: Key,
    src: Arc<AudioBuffer>,
    params: Params,
    /// Source frame where the span starts (fractional).
    in_start: f64,
    frames: usize,
    reversed: bool,
    out: OnceLock<Arc<AudioBuffer>>,
    /// Held while the work runs, so it never runs twice at once.
    work: Mutex<()>,
    queued: AtomicBool,
    used: AtomicU64,
}

impl Prepared {
    /// The finished audio, if it is there (lock-free; fine on the audio
    /// thread).
    #[inline]
    pub fn ready(&self) -> Option<&Arc<AudioBuffer>> {
        self.out.get()
    }

    /// The finished audio, worked out here and now if no one has yet
    /// (waits for a worker already on it). Not for the audio thread.
    pub fn ensure(&self) -> Arc<AudioBuffer> {
        self.run(&|| false).expect("never cancelled")
    }

    fn run(&self, cancel: &dyn Fn() -> bool) -> Option<Arc<AudioBuffer>> {
        if let Some(b) = self.out.get() {
            return Some(b.clone());
        }
        let _guard = self.work.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(b) = self.out.get() {
            return Some(b.clone());
        }
        let chans: Vec<&[f32]> = self.src.channels.iter().map(|c| c.as_slice()).collect();
        let mut out = render(
            &chans,
            self.src.rate,
            &self.params,
            self.in_start,
            self.frames,
            cancel,
        )?;
        if self.reversed {
            out.iter_mut().for_each(|c| c.reverse());
        }
        let buf = Arc::new(AudioBuffer {
            rate: self.src.rate,
            channels: out,
        });
        let _ = self.out.set(buf.clone());
        VERSION.fetch_add(1, Ordering::Release);
        Some(buf)
    }

    fn bytes(&self) -> usize {
        self.frames * self.src.num_channels() * 4
    }
}

struct Cache {
    map: HashMap<Key, Arc<Prepared>>,
    tick: u64,
}

struct Queue {
    jobs: Mutex<VecDeque<Arc<Prepared>>>,
    wake: Condvar,
}

static VERSION: AtomicU64 = AtomicU64::new(0);
static PENDING: AtomicUsize = AtomicUsize::new(0);
/// Finished buffers no mix uses are kept up to this many bytes (undo and
/// toggling a setting back then cost nothing).
const KEEP_BYTES: usize = 768 << 20;

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| {
        Mutex::new(Cache {
            map: HashMap::new(),
            tick: 0,
        })
    })
}

fn queue() -> &'static Arc<Queue> {
    static Q: OnceLock<Arc<Queue>> = OnceLock::new();
    Q.get_or_init(|| {
        let q = Arc::new(Queue {
            jobs: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
        });
        let threads = std::thread::available_parallelism()
            .map_or(2, |n| n.get() / 2)
            .clamp(1, 4);
        for i in 0..threads {
            let q = q.clone();
            let _ = std::thread::Builder::new()
                .name(format!("af27-stretch-{i}"))
                .spawn(move || worker(&q));
        }
        q
    })
}

fn worker(q: &Queue) {
    loop {
        let job = {
            let mut jobs = q.jobs.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(j) = jobs.pop_front() {
                    break j;
                }
                jobs = q.wake.wait(jobs).unwrap_or_else(|e| e.into_inner());
            }
        };
        job.queued.store(false, Ordering::Release);
        // Only the cache and this job hold it: no mix wants it any more
        // (a setting moved on), so the work is skipped, and stopped if it
        // becomes so midway. A later mix that wants it queues it again.
        let unwanted = || Arc::strong_count(&job) <= 2;
        if !unwanted() {
            let _ = job.run(&unwanted);
        }
        PENDING.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Bumped each time a stretched clip's audio is finished.
pub fn version() -> u64 {
    VERSION.load(Ordering::Acquire)
}

/// How many clips are queued or being worked out.
pub fn pending() -> usize {
    PENDING.load(Ordering::Acquire)
}

/// Whether `clip` plays through a prepared buffer.
pub fn needs(clip: &Clip) -> bool {
    clip.props.stretch.is_on()
}

fn key_of(src: &Arc<AudioBuffer>, clip: &Clip) -> Key {
    let s = &clip.props.stretch;
    let p = Params::from_clip(s);
    // Settings the algorithm ignores do not split the cache.
    let vari = s.kind == StretchKind::Varispeed;
    Key {
        src: Arc::as_ptr(src) as usize,
        offset: clip.offset.to_bits(),
        length: clip.length.to_bits(),
        ratio: p.ratio.to_bits(),
        pitch: if vari { 0 } else { (p.pitch as f32).to_bits() },
        kind: s.kind as u8,
        window: if vari { 0 } else { s.window_ms.to_bits() },
        transients: if vari { 0 } else { s.transients.to_bits() },
        formants: !vari && s.preserve_formants && p.pitch != 0.0,
        precise: !vari && s.high_precision,
        reversed: clip.props.reversed,
    }
}

/// The prepared buffer for `clip` playing `src`, from the cache or newly
/// queued. Call on the UI thread (or any thread but the audio one).
pub fn prepare(src: &Arc<AudioBuffer>, clip: &Clip) -> Arc<Prepared> {
    let key = key_of(src, clip);
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    c.tick += 1;
    let tick = c.tick;
    let p = match c.map.get(&key) {
        Some(p) => p.clone(),
        None => {
            let rate = src.rate as f64;
            let p = Arc::new(Prepared {
                key,
                src: src.clone(),
                params: Params::from_clip(&clip.props.stretch),
                in_start: clip.offset * rate,
                frames: (clip.length * rate).round().max(0.0) as usize,
                reversed: clip.props.reversed,
                out: OnceLock::new(),
                work: Mutex::new(()),
                queued: AtomicBool::new(false),
                used: AtomicU64::new(tick),
            });
            c.map.insert(key, p.clone());
            p
        }
    };
    p.used.store(tick, Ordering::Relaxed);
    trim(&mut c);
    drop(c);
    if p.ready().is_none() && !p.queued.swap(true, Ordering::AcqRel) {
        PENDING.fetch_add(1, Ordering::AcqRel);
        let q = queue();
        q.jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_front(p.clone());
        q.wake.notify_one();
    }
    p
}

/// The finished stretched audio of `clip`, if it is in the cache (for
/// drawing its waveform). Does not start any work.
pub fn cached(src: &Arc<AudioBuffer>, clip: &Clip) -> Option<Arc<AudioBuffer>> {
    let key = key_of(src, clip);
    let c = cache().lock().unwrap_or_else(|e| e.into_inner());
    c.map.get(&key).and_then(|p| p.ready().cloned())
}

/// Drops entries no one uses: unfinished ones at once, finished ones
/// oldest first once they pass [`KEEP_BYTES`].
fn trim(c: &mut Cache) {
    c.map.retain(|_, p| {
        Arc::strong_count(p) > 1 || p.ready().is_some() || p.queued.load(Ordering::Acquire)
    });
    let mut idle: Vec<(u64, usize, Key)> = c
        .map
        .values()
        .filter(|p| Arc::strong_count(p) == 1)
        .map(|p| (p.used.load(Ordering::Relaxed), p.bytes(), p.key))
        .collect();
    let mut total: usize = idle.iter().map(|x| x.1).sum();
    if total <= KEEP_BYTES {
        return;
    }
    idle.sort_by_key(|x| x.0);
    for (_, bytes, key) in idle {
        if total <= KEEP_BYTES {
            break;
        }
        c.map.remove(&key);
        total -= bytes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use af27_core::session::{ClipStretch, StretchMode};

    fn clip(ratio: f64) -> Clip {
        Clip {
            id: 1,
            media: 1,
            name: String::new(),
            start: 0.0,
            offset: 0.0,
            length: 0.5 * ratio,
            gain_db: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
            props: af27_core::session::ClipProps {
                stretch: ClipStretch {
                    mode: StretchMode::Live,
                    ratio,
                    ..Default::default()
                },
                ..Default::default()
            },
            ext_id: None,
        }
    }

    #[test]
    fn same_settings_share_an_entry_and_finish() {
        let src = Arc::new(AudioBuffer::new(48_000, 1, 24_000));
        let a = prepare(&src, &clip(2.0));
        let b = prepare(&src, &clip(2.0));
        assert!(Arc::ptr_eq(&a, &b));
        let c = prepare(&src, &clip(1.5));
        assert!(!Arc::ptr_eq(&a, &c));
        assert_eq!(a.ensure().frames(), 48_000);
        assert_eq!(c.ensure().frames(), 36_000);
        assert!(cached(&src, &clip(2.0)).is_some());
    }
}
