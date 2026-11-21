// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.
// Ported from Audiform's Phase Wheel ring (engine/scope.rs), written by
// its author (nwh212) for Audiform and relicensed here by them.

//! The last moments of what is playing, for the Phase Wheel.
//!
//! The audio thread writes each output frame (left and right, after the
//! master fader) into a fixed ring of atomics: two relaxed stores a frame,
//! no allocation, no locks. The interface asks for the newest frames now
//! and then; a read may tear against a write in progress. For a picture
//! that is fine.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// Frames kept: enough for the largest analysis window.
pub const CAPACITY: usize = 1 << 15;

static RING: [AtomicU32; CAPACITY * 2] = [const { AtomicU32::new(0) }; CAPACITY * 2];
/// Frames written so far (wrapping), so the newest frame is `WRITTEN - 1`.
static WRITTEN: AtomicUsize = AtomicUsize::new(0);
static RATE: AtomicU32 = AtomicU32::new(48_000);

pub fn set_rate(rate: u32) {
    RATE.store(rate.max(1), Ordering::Relaxed);
}

pub fn rate() -> u32 {
    RATE.load(Ordering::Relaxed)
}

/// Records one output frame. Audio thread: two stores, nothing else.
#[inline]
pub fn push(l: f32, r: f32) {
    let w = WRITTEN.load(Ordering::Relaxed);
    let at = (w % CAPACITY) * 2;
    RING[at].store(l.to_bits(), Ordering::Relaxed);
    RING[at + 1].store(r.to_bits(), Ordering::Relaxed);
    WRITTEN.store(w.wrapping_add(1), Ordering::Release);
}

/// The newest `n` frames (n at most CAPACITY), oldest first.
/// The frames written since `pos` (a value this returned before, or 0),
/// at most the ring's size, and the new position.
pub fn since(pos: usize) -> (Vec<[f32; 2]>, usize) {
    let w = WRITTEN.load(Ordering::Acquire);
    let n = w.wrapping_sub(pos).min(CAPACITY);
    let start = w.wrapping_sub(n);
    let out = (0..n)
        .map(|i| {
            let at = (start.wrapping_add(i) % CAPACITY) * 2;
            [
                f32::from_bits(RING[at].load(Ordering::Relaxed)),
                f32::from_bits(RING[at + 1].load(Ordering::Relaxed)),
            ]
        })
        .collect();
    (out, w)
}

/// Frames written so far (wrapping).
pub fn written() -> usize {
    WRITTEN.load(Ordering::Acquire)
}

pub fn latest(n: usize) -> Vec<[f32; 2]> {
    let n = n.min(CAPACITY);
    let end = WRITTEN.load(Ordering::Acquire);
    (0..n)
        .map(|k| {
            let at = (end.wrapping_sub(n - k) % CAPACITY) * 2;
            [
                f32::from_bits(RING[at].load(Ordering::Relaxed)),
                f32::from_bits(RING[at + 1].load(Ordering::Relaxed)),
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_returns_the_newest_frames_in_order() {
        for i in 0..(CAPACITY + 10) {
            push(i as f32, -(i as f32));
        }
        let got = latest(3);
        let last = (CAPACITY + 9) as f32;
        assert_eq!(
            got,
            vec![
                [last - 2.0, 2.0 - last],
                [last - 1.0, 1.0 - last],
                [last, -last]
            ]
        );
    }
}
