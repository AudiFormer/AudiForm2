// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! A worker pool the audio thread can use without locking or allocating.
//!
//! The pool's threads are started once, on the UI thread, the first time
//! a mix is built. To run a job, the caller publishes it (a pointer to its
//! closure and an item count) and bumps a generation counter; the workers,
//! which spin briefly and then park between jobs, see the new generation
//! and claim items with an atomic counter. The caller claims items too, so
//! it is never idle, and returns once every item is done. Claiming item by
//! item means a heavy track does not hold up the rest: whoever is free
//! takes the next one.
//!
//! Only atomics are touched on the audio thread, plus `unpark` for workers
//! that went to sleep (a single system call that neither allocates nor
//! takes a lock). On a one-core machine, or with `AF2_THREADS=1`, there are
//! no workers and jobs simply run in a loop on the caller.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::Thread;
use std::time::{Duration, Instant};

/// A published job: the closure (type-erased) and how to call it.
#[derive(Clone, Copy)]
struct Job {
    data: *const (),
    call: unsafe fn(*const (), usize),
}

struct Shared {
    generation: AtomicU64,
    /// The job's generation (high 32 bits) and the next item (low 32), so
    /// a worker still finishing the last job can never claim from this one.
    ticket: AtomicU64,
    count: AtomicUsize,
    done: AtomicUsize,
    job: UnsafeCell<Option<Job>>,
    sleeping: Vec<AtomicBool>,
    threads: OnceLock<Vec<Thread>>,
}

// The job cell is written only by the caller while no worker can read it
// (see `run`), and read only between claiming an item and reporting it done.
unsafe impl Sync for Shared {}
unsafe impl Send for Shared {}

pub struct Pool {
    shared: Arc<Shared>,
    workers: usize,
    /// Only one caller at a time may publish a job.
    busy: AtomicBool,
}

/// How long a worker spins after a job before it parks. Audio blocks come
/// every few milliseconds, so a short spin catches the next one awake
/// without burning a core while nothing plays.
const SPIN: Duration = Duration::from_micros(1000);

impl Pool {
    fn new(workers: usize) -> Pool {
        let shared = Arc::new(Shared {
            generation: AtomicU64::new(0),
            ticket: AtomicU64::new(0),
            count: AtomicUsize::new(0),
            done: AtomicUsize::new(0),
            job: UnsafeCell::new(None),
            sleeping: (0..workers).map(|_| AtomicBool::new(false)).collect(),
            threads: OnceLock::new(),
        });
        let mut threads = Vec::new();
        for k in 0..workers {
            let s = shared.clone();
            let h = std::thread::Builder::new()
                .name(format!("af2-engine-{k}"))
                .spawn(move || worker(s, k))
                .expect("engine worker thread");
            threads.push(h.thread().clone());
        }
        let _ = shared.threads.set(threads);
        Pool {
            shared,
            workers,
            busy: AtomicBool::new(false),
        }
    }

    /// Threads that help the caller (0 on a one-core machine).
    pub fn workers(&self) -> usize {
        self.workers
    }

    /// Calls `f(i)` for every `i` in `0..n`, spread over the pool and the
    /// calling thread, and returns when all are done. Each `i` is run
    /// exactly once. If another job is already running (a second caller),
    /// this one simply runs its items itself.
    pub fn run<F: Fn(usize) + Sync>(&self, n: usize, f: &F) {
        if n == 0 {
            return;
        }
        if self.workers == 0 || n == 1 || self.busy.swap(true, Ordering::Acquire) {
            for i in 0..n {
                f(i);
            }
            return;
        }
        unsafe fn trampoline<F: Fn(usize)>(data: *const (), i: usize) {
            unsafe { (*(data as *const F))(i) }
        }
        let s = &*self.shared;
        let gen = (s.generation.load(Ordering::Relaxed) + 1) & 0xffff_ffff;
        // Close the ticket first: it now names the new generation but no
        // item can be claimed from it. A worker still holding a ticket it
        // read from the last job fails its compare-exchange from here on, so
        // it can never claim an item against the new count and closure
        // written below (which would run a new item early, and twice).
        s.ticket.store((gen << 32) | 0xffff_ffff, Ordering::SeqCst);
        // No worker reads the job cell now: the last job's items were all
        // reported done before the last `run` returned.
        unsafe {
            *s.job.get() = Some(Job {
                data: f as *const F as *const (),
                call: trampoline::<F>,
            });
        }
        s.count.store(n, Ordering::Relaxed);
        s.done.store(0, Ordering::Relaxed);
        s.ticket.store(gen << 32, Ordering::Release);
        s.generation.store(gen, Ordering::Release);
        if let Some(threads) = s.threads.get() {
            for (k, t) in threads.iter().enumerate().take(n - 1) {
                if s.sleeping[k].load(Ordering::Acquire) {
                    t.unpark();
                }
            }
        }
        claim(s, gen);
        while s.done.load(Ordering::Acquire) < n {
            std::hint::spin_loop();
        }
        self.busy.store(false, Ordering::Release);
    }
}

/// Claims and runs items of job `gen` until none are left (or a newer
/// job has replaced it).
fn claim(s: &Shared, gen: u64) {
    loop {
        let t = s.ticket.load(Ordering::Acquire);
        if t >> 32 != gen {
            return;
        }
        let i = (t & 0xffff_ffff) as usize;
        let n = s.count.load(Ordering::Acquire);
        if i >= n {
            return;
        }
        if s
            .ticket
            .compare_exchange_weak(t, t + 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            continue;
        }
        // Safe to read: the caller cannot publish a new job before this
        // item is reported done.
        if let Some(job) = unsafe { *s.job.get() } {
            unsafe { (job.call)(job.data, i) };
        }
        s.done.fetch_add(1, Ordering::AcqRel);
    }
}

fn worker(s: Arc<Shared>, k: usize) {
    raise_priority();
    // Generation 0 is "no job yet": a worker that starts late still sees
    // the first job as new.
    let mut seen = 0;
    loop {
        let start = Instant::now();
        let mut spins = 0u32;
        loop {
            let g = s.generation.load(Ordering::Acquire);
            if g != seen {
                seen = g;
                break;
            }
            spins += 1;
            if spins % 64 == 0 && start.elapsed() > SPIN {
                s.sleeping[k].store(true, Ordering::Release);
                // Check again after saying we sleep, so a job published in
                // between is not missed; park wakes on unpark or spuriously.
                if s.generation.load(Ordering::Acquire) == seen {
                    std::thread::park_timeout(Duration::from_millis(50));
                }
                s.sleeping[k].store(false, Ordering::Release);
            } else {
                std::hint::spin_loop();
            }
        }
        claim(&s, seen);
    }
}

/// Engine workers run at high priority, like the audio thread they serve,
/// so a busy desktop does not make them late.
fn raise_priority() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentThread() -> isize;
            fn SetThreadPriority(thread: isize, priority: i32) -> i32;
        }
        const THREAD_PRIORITY_HIGHEST: i32 = 2;
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        }
    }
}

/// How many helper threads to start: one fewer than the cores (the audio
/// thread works too), at most 15, or `AF2_THREADS - 1` when set.
fn workers_wanted() -> usize {
    let cores = std::env::var("AF2_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()));
    cores.saturating_sub(1).min(15)
}

static POOL: OnceLock<Pool> = OnceLock::new();

/// The engine's pool, started on first use. Call it once from the UI
/// thread (building a mix does) so the threads never start on the audio
/// thread.
pub fn pool() -> &'static Pool {
    POOL.get_or_init(|| Pool::new(workers_wanted()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn every_item_runs_exactly_once_many_times_over() {
        let p = Pool::new(3);
        for round in 0..2000 {
            let n = 1 + round % 37;
            let hits: Vec<AtomicU32> = (0..n).map(|_| AtomicU32::new(0)).collect();
            p.run(n, &|i| {
                hits[i].fetch_add(1, Ordering::Relaxed);
            });
            assert!(hits.iter().all(|h| h.load(Ordering::Relaxed) == 1), "round {round}");
        }
    }

    /// The race that once ran an item twice: many short jobs back to back,
    /// with more workers than items, so workers are often mid-claim on the
    /// last job while the next is published.
    #[test]
    fn back_to_back_jobs_never_run_an_item_twice() {
        let p = Pool::new(7);
        for round in 0..30_000 {
            let n = 2 + round % 5;
            let hits: Vec<AtomicU32> = (0..n).map(|_| AtomicU32::new(0)).collect();
            p.run(n, &|i| {
                hits[i].fetch_add(1, Ordering::Relaxed);
            });
            assert!(hits.iter().all(|h| h.load(Ordering::Relaxed) == 1), "round {round}");
        }
    }

    #[test]
    fn work_spreads_over_threads() {
        let p = Pool::new(3);
        let ids = std::sync::Mutex::new(std::collections::HashSet::new());
        p.run(64, &|_| {
            std::thread::sleep(Duration::from_millis(2));
            ids.lock().unwrap().insert(std::thread::current().id());
        });
        assert!(ids.lock().unwrap().len() >= 2);
    }

    #[test]
    fn no_workers_runs_on_the_caller() {
        let p = Pool::new(0);
        let sum = AtomicU32::new(0);
        p.run(10, &|i| {
            sum.fetch_add(i as u32, Ordering::Relaxed);
        });
        assert_eq!(sum.load(Ordering::Relaxed), 45);
    }

    #[test]
    fn a_second_caller_runs_its_own_items() {
        let p = Arc::new(Pool::new(2));
        let hs: Vec<_> = (0..4)
            .map(|_| {
                let p = p.clone();
                std::thread::spawn(move || {
                    for _ in 0..200 {
                        let c = AtomicU32::new(0);
                        p.run(9, &|_| {
                            c.fetch_add(1, Ordering::Relaxed);
                        });
                        assert_eq!(c.load(Ordering::Relaxed), 9);
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
    }
}
