// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The audio thread must not allocate: an allocation can take a lock in
//! the allocator and miss the deadline (a click). Every effect is run,
//! retuned (`update`, as a drag or automation does) and run again, with a
//! counting allocator watching this thread.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static WATCH: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        // `try_with`: the thread-locals may be gone while a thread ends.
        let _ = WATCH.try_with(|w| {
            if w.get() {
                let _ = COUNT.try_with(|c| c.set(c.get() + 1));
            }
        });
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let _ = WATCH.try_with(|w| {
            if w.get() {
                let _ = COUNT.try_with(|c| c.set(c.get() + 1));
            }
        });
        System.realloc(p, l, n)
    }
}

#[global_allocator]
static A: Counting = Counting;

/// Allocations made by `f` on this thread.
fn count(f: impl FnOnce()) -> usize {
    COUNT.with(|c| c.set(0));
    WATCH.with(|w| w.set(true));
    f();
    WATCH.with(|w| w.set(false));
    COUNT.with(|c| c.get())
}

fn signal(frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|i| {
            let v = ((i as f32) * 0.031).sin() * 0.3 + ((i as f32) * 0.0047).sin() * 0.2;
            [v, -v * 0.8]
        })
        .collect()
}

#[test]
fn no_effect_allocates_while_running_retuning_or_resetting() {
    const BLOCK: usize = 512;
    let mut bad = Vec::new();
    for info in af2_fx::registry() {
        let params = af2_fx::params_from(info.id, &[]).unwrap();
        let Some(mut fx) = af2_fx::build(info.id, 48_000, &params) else {
            continue;
        };
        let mut buf = signal(BLOCK * 8);
        // Warm up: the first blocks may size buffers.
        for b in buf.chunks_mut(BLOCK * 2) {
            fx.process(b);
        }
        // New settings: every continuous control moved a little (choices
        // and switches kept, as those may need a rebuild).
        let moved: Vec<(String, f64)> = info
            .params
            .iter()
            .filter(|p| p.choices.is_empty() && !p.toggle)
            .map(|p| (p.key.to_string(), p.default + (p.max - p.default) * 0.25))
            .collect();
        let next = af2_fx::params_from(info.id, &moved).unwrap();
        let mut buf = signal(BLOCK * 8);
        let n = count(|| {
            let _ = fx.update(&next);
            for b in buf.chunks_mut(BLOCK * 2) {
                fx.process(b);
            }
            let _ = fx.update(&params);
            for b in buf.chunks_mut(BLOCK * 2) {
                fx.process(b);
            }
            // A seek forgets the tails.
            fx.reset();
            for b in buf.chunks_mut(BLOCK * 2) {
                fx.process(b);
            }
        });
        if n > 0 {
            bad.push(format!("{} ({n})", info.id));
        }
    }
    assert!(bad.is_empty(), "allocated on the audio thread: {}", bad.join(", "));
}
