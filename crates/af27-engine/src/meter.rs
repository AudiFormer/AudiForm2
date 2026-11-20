// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Level readings handed from the audio thread to the interface.
//!
//! The audio thread folds every block into a "since the last read"
//! accumulator; the interface takes (and so resets) it once a frame. That
//! way a frame's reading covers all the audio played since the previous
//! frame, never just the one block that happened to be last.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Raises `a` (a non-negative f32 kept as bits) to `v` if `v` is larger.
/// One atomic step, so a read that resets it in between never loses a peak.
#[inline]
pub(crate) fn store_max_f32(a: &AtomicU32, v: f32) {
    // Non-negative floats order the same as their bit patterns.
    if v > 0.0 {
        a.fetch_max(v.to_bits(), Ordering::Relaxed);
    }
}

/// A running sum of squares and how many samples went into it, per side.
/// Lock-free: the audio thread adds, the interface swaps to zero.
#[derive(Default)]
pub(crate) struct MeanSquare {
    l: AtomicU64,
    r: AtomicU64,
    n: AtomicU64,
}

fn add_f64(a: &AtomicU64, v: f64) {
    let mut cur = a.load(Ordering::Relaxed);
    loop {
        let next = (f64::from_bits(cur) + v).to_bits();
        match a.compare_exchange_weak(cur, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(now) => cur = now,
        }
    }
}

impl MeanSquare {
    /// Audio thread: adds a block's sums of squares over `n` frames.
    #[inline]
    pub(crate) fn add(&self, l: f64, r: f64, n: u64) {
        if n == 0 {
            return;
        }
        add_f64(&self.l, l);
        add_f64(&self.r, r);
        self.n.fetch_add(n, Ordering::Relaxed);
    }

    /// The mean squares since the last take (left, right), or None when no
    /// audio has come through since then.
    pub(crate) fn take(&self) -> Option<(f32, f32)> {
        let n = self.n.swap(0, Ordering::Relaxed);
        let l = f64::from_bits(self.l.swap(0, Ordering::Relaxed));
        let r = f64::from_bits(self.r.swap(0, Ordering::Relaxed));
        (n > 0).then(|| ((l / n as f64) as f32, (r / n as f64) as f32))
    }
}

/// Sums of squares of the first two channels of interleaved `frames`
/// (`width` wide), and the frame count.
pub(crate) fn sum_squares(frames: &[f32], width: usize) -> (f64, f64, u64) {
    let width = width.max(1);
    let (mut l, mut r, mut n) = (0.0f64, 0.0f64, 0u64);
    for f in frames.chunks_exact(width) {
        let a = f[0] as f64;
        let b = *f.get(1).unwrap_or(&f[0]) as f64;
        l += a * a;
        r += b * b;
        n += 1;
    }
    (l, r, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_largest_peak_since_the_last_read_wins() {
        let a = AtomicU32::new(0);
        store_max_f32(&a, 0.25);
        store_max_f32(&a, 0.75);
        store_max_f32(&a, 0.5);
        store_max_f32(&a, -1.0);
        store_max_f32(&a, f32::NAN);
        assert_eq!(f32::from_bits(a.swap(0, Ordering::Relaxed)), 0.75);
        assert_eq!(f32::from_bits(a.load(Ordering::Relaxed)), 0.0);
    }

    #[test]
    fn mean_square_covers_every_block_since_the_last_take() {
        let m = MeanSquare::default();
        assert_eq!(m.take(), None);
        // A loud block then a quiet one: the take averages both.
        let loud = [1.0f32, 0.5, 1.0, 0.5];
        let quiet = [0.0f32, 0.0, 0.0, 0.0];
        let (l, r, n) = sum_squares(&loud, 2);
        m.add(l, r, n);
        let (l, r, n) = sum_squares(&quiet, 2);
        m.add(l, r, n);
        let (l, r) = m.take().unwrap();
        assert!((l - 0.5).abs() < 1e-6, "{l}");
        assert!((r - 0.125).abs() < 1e-6, "{r}");
        assert_eq!(m.take(), None);
    }
}
