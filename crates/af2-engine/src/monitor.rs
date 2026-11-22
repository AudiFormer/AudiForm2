// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Input monitoring: hearing a track's input live, through the track's
//! own effects, fader and pan, the way a multitrack recorder does.
//!
//! An input stream (the recorder's own while recording, else one opened
//! just for monitoring) copies each buffer into a small lock-free ring
//! through a [`Tap`]. The output callback takes from the ring a block at
//! a time ([`Feed`]), splits it into each monitored track's left and
//! right ([`LiveBlock`]) and the mix adds that to the track's input,
//! after its clips and before its chain, so the takes stay dry while the
//! player hears the effects. Nothing here allocates on the audio thread.
//!
//! On ASIO the input and output buffers arrive together, so the ring
//! holds at most a buffer or two: the delay is the driver's in and out
//! buffers plus any plug-in delay the mix compensates for. With separate
//! system devices the ring keeps a small cushion against their clocks
//! drifting apart, and drops what piles up past it.

use std::sync::{Arc, Mutex};

use af2_core::TrackId;
use cpal::{FromSample, SizedSample};

use crate::mix::BLOCK;

/// Where an input stream hands its buffers to the monitor: a ring's
/// producer, swapped by the UI thread as monitoring changes. The input
/// callback only ever `try_lock`s it (no waiting): a buffer arriving
/// during a swap is skipped.
pub type Tap = Arc<Mutex<Option<rtrb::Producer<f32>>>>;

pub fn new_tap() -> Tap {
    Arc::new(Mutex::new(None))
}

/// The input callback's part: converts and pushes whole frames, as many
/// as fit. Never blocks, never allocates.
pub fn push<T: SizedSample>(tap: &Tap, d: &[T], channels: usize)
where
    f32: FromSample<T>,
{
    let Ok(mut g) = tap.try_lock() else { return };
    let Some(tx) = g.as_mut() else { return };
    let ch = channels.max(1);
    let fit = (tx.slots() / ch * ch).min(d.len() / ch * ch);
    for &s in &d[..fit] {
        let v: f32 = s.to_sample();
        let _ = tx.push(v);
    }
}

/// Seconds of input a monitor ring holds (far more than is ever kept).
const RING_SECS: f64 = 0.5;

/// A ring sized for `channels` at `rate`.
pub fn ring(rate: u32, channels: usize) -> (rtrb::Producer<f32>, rtrb::Consumer<f32>) {
    rtrb::RingBuffer::new(((rate as f64 * RING_SECS) as usize).max(BLOCK * 4) * channels.max(1))
}

/// One monitored track, as the audio thread sees it.
pub struct Lane {
    pub track: TrackId,
    /// Which device in the feed it listens to.
    dev: usize,
    /// The device channels heard on the left and right (the same one
    /// twice for a mono input, centred).
    pick: [usize; 2],
    l: Vec<f32>,
    r: Vec<f32>,
}

/// Every monitored track's input for the block being rendered. Shared
/// read-only with the mixer's worker threads.
#[derive(Default)]
pub struct LiveBlock {
    lanes: Vec<Lane>,
    n: usize,
}

impl LiveBlock {
    /// `track`'s input for this block (`n` frames), if it is monitored.
    pub fn get(&self, track: TrackId, n: usize) -> Option<(&[f32], &[f32])> {
        let n = n.min(self.n);
        self.lanes
            .iter()
            .find(|l| l.track == track)
            .map(|l| (&l.l[..n], &l.r[..n]))
    }

    pub fn is_empty(&self) -> bool {
        self.lanes.is_empty()
    }
}

/// One input device's ring on the audio thread.
struct Dev {
    rx: rtrb::Consumer<f32>,
    channels: usize,
    /// One block of interleaved frames.
    frames: Vec<f32>,
    /// Enough has arrived to start (again) without running dry at once.
    primed: bool,
}

/// What the output callback owns: the rings and the block they fill.
pub struct Feed {
    devs: Vec<Dev>,
    pub block: LiveBlock,
}

/// A track to monitor and where its input comes from, for [`Feed::new`].
pub struct FeedLane {
    pub track: TrackId,
    /// Index into the rings given to [`Feed::new`].
    pub dev: usize,
    /// The device channels (0-based): one for mono, two for stereo.
    pub pick: [usize; 2],
}

/// Frames kept waiting beyond one callback's worth before the excess is
/// dropped, at least.
const MIN_CUSHION: usize = 128;

impl Feed {
    /// UI thread: every buffer is allocated here.
    pub fn new(rings: Vec<(rtrb::Consumer<f32>, usize)>, lanes: Vec<FeedLane>) -> Feed {
        let devs = rings
            .into_iter()
            .map(|(rx, channels)| Dev {
                rx,
                channels: channels.max(1),
                frames: vec![0.0; BLOCK * channels.max(1)],
                primed: false,
            })
            .collect::<Vec<_>>();
        let lanes = lanes
            .into_iter()
            .filter(|l| l.dev < devs.len())
            .map(|l| Lane {
                track: l.track,
                dev: l.dev,
                pick: l.pick,
                l: vec![0.0; BLOCK],
                r: vec![0.0; BLOCK],
            })
            .collect();
        Feed {
            devs,
            block: LiveBlock { lanes, n: 0 },
        }
    }

    /// At the start of each output callback of `frames` frames: starts a
    /// ring once a callback's worth has arrived, and drops what piled up
    /// past a small cushion so the delay cannot creep up.
    pub fn begin(&mut self, frames: usize) {
        let cushion = frames.max(MIN_CUSHION);
        for d in &mut self.devs {
            let avail = d.rx.slots() / d.channels;
            if !d.primed && avail >= frames {
                d.primed = true;
            }
            if avail > frames + 2 * cushion {
                let drop = (avail - frames - cushion) * d.channels;
                if let Ok(c) = d.rx.read_chunk(drop) {
                    c.commit_all();
                }
            }
        }
    }

    /// Takes the next `n` frames (at most [`BLOCK`]) from every ring into
    /// the block. A ring that runs dry gives silence for the rest and
    /// waits to fill again.
    pub fn pull(&mut self, n: usize) {
        let n = n.min(BLOCK);
        for d in &mut self.devs {
            let ch = d.channels;
            let want = n * ch;
            let mut got = 0;
            if d.primed {
                let take = d.rx.slots().min(want) / ch * ch;
                if let Ok(c) = d.rx.read_chunk(take) {
                    let (a, b) = c.as_slices();
                    d.frames[..a.len()].copy_from_slice(a);
                    d.frames[a.len()..a.len() + b.len()].copy_from_slice(b);
                    c.commit_all();
                    got = take;
                }
                if got < want {
                    d.primed = false;
                }
            }
            d.frames[got..want].fill(0.0);
        }
        for lane in &mut self.block.lanes {
            let d = &self.devs[lane.dev];
            let ch = d.channels;
            let [a, b] = lane.pick;
            for i in 0..n {
                let f = &d.frames[i * ch..(i + 1) * ch];
                lane.l[i] = f.get(a).copied().unwrap_or(0.0);
                lane.r[i] = f.get(b).copied().unwrap_or(0.0);
            }
        }
        self.block.n = n;
    }

    /// No live input this block (the mix is not what plays).
    pub fn silence(&mut self) {
        self.block.n = 0;
    }
}

/// The left and right device channels a track hears from its input
/// pick: a mono input on both sides, a stereo one as it is. `None` (the
/// whole default device) is its first two channels, or its only one.
pub fn pick_of(channels: Option<&[u16]>, device_channels: usize) -> [usize; 2] {
    match channels {
        Some([a]) => [usize::from(*a); 2],
        Some([a, b, ..]) => [usize::from(*a), usize::from(*b)],
        _ if device_channels >= 2 => [0, 1],
        _ => [0, 0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tid(n: u64) -> TrackId {
        n
    }

    fn feed_of(channels: usize, lanes: Vec<FeedLane>) -> (rtrb::Producer<f32>, Feed) {
        let (tx, rx) = ring(48_000, channels);
        (tx, Feed::new(vec![(rx, channels)], lanes))
    }

    #[test]
    fn mono_and_stereo_picks() {
        assert_eq!(pick_of(Some(&[1]), 2), [1, 1]);
        assert_eq!(pick_of(Some(&[2, 3]), 4), [2, 3]);
        assert_eq!(pick_of(None, 2), [0, 1]);
        assert_eq!(pick_of(None, 1), [0, 0]);
    }

    #[test]
    fn the_tap_pushes_whole_frames_and_never_blocks() {
        let tap = new_tap();
        // No ring yet: nothing happens.
        push::<f32>(&tap, &[0.5, 0.5], 2);
        let (tx, mut rx) = rtrb::RingBuffer::<f32>::new(5);
        *tap.lock().unwrap() = Some(tx);
        push::<f32>(&tap, &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6], 2);
        // Room for 5 samples: two whole frames go in.
        assert_eq!(rx.slots(), 4);
        assert_eq!(rx.pop().unwrap(), 0.1);
        // Held by someone else: the buffer is skipped, not waited for.
        let g = tap.lock().unwrap();
        push::<f32>(&tap, &[0.9, 0.9], 2);
        drop(g);
        assert_eq!(rx.slots(), 3);
        // Integer formats are converted.
        let tap = new_tap();
        let (tx, mut rx) = rtrb::RingBuffer::<f32>::new(4);
        *tap.lock().unwrap() = Some(tx);
        push::<i16>(&tap, &[i16::MAX, 0], 2);
        assert!((rx.pop().unwrap() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn a_mono_input_is_heard_on_both_sides_and_a_stereo_one_as_is() {
        let lanes = vec![
            FeedLane { track: tid(1), dev: 0, pick: [1, 1] },
            FeedLane { track: tid(2), dev: 0, pick: [0, 1] },
        ];
        let (mut tx, mut f) = feed_of(2, lanes);
        for i in 0..64 {
            tx.push(i as f32).unwrap();
            tx.push(-(i as f32)).unwrap();
        }
        f.begin(32);
        f.pull(32);
        let (l, r) = f.block.get(tid(1), 32).unwrap();
        assert_eq!((l[3], r[3]), (-3.0, -3.0));
        let (l, r) = f.block.get(tid(2), 32).unwrap();
        assert_eq!((l[3], r[3]), (3.0, -3.0));
        assert!(f.block.get(tid(9), 32).is_none());
        // The next block carries on where this one stopped.
        f.pull(32);
        let (l, _) = f.block.get(tid(2), 32).unwrap();
        assert_eq!(l[0], 32.0);
    }

    #[test]
    fn it_waits_for_a_buffer_runs_dry_to_silence_and_starts_again() {
        let (mut tx, mut f) = feed_of(1, vec![FeedLane { track: tid(1), dev: 0, pick: [0, 0] }]);
        for _ in 0..16 {
            tx.push(1.0).unwrap();
        }
        // Less than a callback's worth: silence, nothing taken.
        f.begin(32);
        f.pull(32);
        assert!(f.block.get(tid(1), 32).unwrap().0.iter().all(|&v| v == 0.0));
        for _ in 0..16 {
            tx.push(1.0).unwrap();
        }
        f.begin(32);
        f.pull(32);
        assert!(f.block.get(tid(1), 32).unwrap().0.iter().all(|&v| v == 1.0));
        // Dry: what came, then silence.
        for _ in 0..8 {
            tx.push(0.5).unwrap();
        }
        f.begin(32);
        f.pull(32);
        let (l, _) = f.block.get(tid(1), 32).unwrap();
        assert_eq!(l[7], 0.5);
        assert_eq!(l[8], 0.0);
    }

    #[test]
    fn a_backlog_is_dropped_so_the_delay_stays_short() {
        let (mut tx, mut f) = feed_of(1, vec![FeedLane { track: tid(1), dev: 0, pick: [0, 0] }]);
        for i in 0..4000 {
            tx.push(i as f32).unwrap();
        }
        f.begin(64);
        // At most a callback plus the cushion is left waiting.
        let left = 4000 - 1;
        f.pull(64);
        let (l, _) = f.block.get(tid(1), 64).unwrap();
        // The newest audio is kept: what plays is from near the end.
        assert!(l[0] as i32 >= left - 64 - 128 - 64, "{}", l[0]);
    }

    #[test]
    fn silence_hides_the_block() {
        let (_tx, mut f) = feed_of(1, vec![FeedLane { track: tid(1), dev: 0, pick: [0, 0] }]);
        f.pull(16);
        f.silence();
        assert_eq!(f.block.get(tid(1), 16).unwrap().0.len(), 0);
    }
}
