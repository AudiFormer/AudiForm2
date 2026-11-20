// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Keeping a device stream healthy: ending on silence, catching a panic
//! in the callback, noticing a device that stopped calling back, reopening
//! with a growing wait, and the audio log.
//!
//! The audio thread only touches atomics here. Everything that waits,
//! writes a file or decides runs on the UI thread, and the deciding parts
//! take the time as an argument so they are tested without hardware.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ------------------------------------------------------------ silence

/// Silent callbacks a closing stream plays before it is dropped.
pub const HUSH_CALLS: u32 = 3;
/// The longest a close waits for them (a stream that stopped calling back).
pub const HUSH_MAX: Duration = Duration::from_millis(400);

/// Ends a stream on silence. An ASIO driver keeps running while any other
/// stream of the process is open on it, and it may play again whatever a
/// closed stream last left in its buffers; a few silent buffers before the
/// close leave nothing to replay. Harmless on other hosts.
#[derive(Default)]
pub struct Hush {
    on: AtomicBool,
    calls: AtomicU32,
}

impl Hush {
    /// In the output callback: true once the stream is closing, with `out`
    /// already silenced, so the callback does nothing else.
    pub fn silenced<T: Copy>(&self, out: &mut [T], zero: T) -> bool {
        if !self.on.load(Ordering::Relaxed) {
            return false;
        }
        out.fill(zero);
        self.calls.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Starts the silence and waits until the device has taken
    /// [`HUSH_CALLS`] silent buffers or `max` has passed. Then the stream
    /// can be dropped. Returns the silent callbacks seen.
    pub fn settle_within(&self, max: Duration) -> u32 {
        self.on.store(true, Ordering::SeqCst);
        let t = Instant::now();
        while self.calls.load(Ordering::Relaxed) < HUSH_CALLS && t.elapsed() < max {
            std::thread::sleep(Duration::from_millis(2));
        }
        self.calls.load(Ordering::Relaxed)
    }

    pub fn settle(&self) -> u32 {
        self.settle_within(HUSH_MAX)
    }

    pub fn closing(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }
}

// ------------------------------------------------------------ faults

/// Runs a callback body, catching a panic: the buffer is left silent and
/// `fault` is set so the UI can say so. Only Rust panics are caught; a
/// crash inside a native driver still ends the process.
pub fn guarded<T: Copy>(out: &mut [T], zero: T, fault: &AtomicBool, body: impl FnOnce(&mut [T])) {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(&mut *out)));
    if r.is_err() {
        out.fill(zero);
        fault.store(true, Ordering::Relaxed);
    }
}

// ------------------------------------------------------------ stalls

/// What the callback reports, through atomics.
#[derive(Default)]
pub struct CallbackStats {
    /// Callbacks so far.
    pub calls: AtomicU64,
    /// Frames in the last callback: the device's real buffer size.
    pub frames: AtomicU32,
    /// Frames in the first callback (0 until it came).
    pub first_frames: AtomicU32,
}

impl CallbackStats {
    /// In the callback: one more, of `frames` frames.
    pub fn tick(&self, frames: usize) {
        let f = frames.min(u32::MAX as usize) as u32;
        if self.calls.fetch_add(1, Ordering::Relaxed) == 0 {
            self.first_frames.store(f, Ordering::Relaxed);
        }
        self.frames.store(f, Ordering::Relaxed);
    }

    /// The buffer size the device really uses, once it has called back.
    pub fn buffer(&self) -> Option<u32> {
        Some(self.frames.load(Ordering::Relaxed)).filter(|&n| n > 0)
    }
}

/// How long an open stream may go without a callback before it counts as
/// stalled. Generous: a busy machine or a long first buffer must not trip
/// it.
pub const STALL: Duration = Duration::from_millis(2_000);

/// Notices an output that stopped calling back (a driver that stopped
/// serving us, a device asleep) without an error.
#[derive(Clone, Copy, Debug)]
pub struct Watchdog {
    last_calls: u64,
    since: Instant,
    limit: Duration,
}

impl Watchdog {
    pub fn new(now: Instant, limit: Duration) -> Watchdog {
        Watchdog {
            last_calls: 0,
            since: now,
            limit,
        }
    }

    /// Given the callback count now, true once it has not moved for the
    /// limit. Starts counting again after each report.
    pub fn stalled(&mut self, calls: u64, now: Instant) -> bool {
        if calls != self.last_calls {
            self.last_calls = calls;
            self.since = now;
            return false;
        }
        if now.saturating_duration_since(self.since) >= self.limit {
            self.since = now;
            return true;
        }
        false
    }
}

// ------------------------------------------------------------ reopening

/// Waits between attempts to reopen a lost device: 0.5 s, 1 s, 2 s, 4 s,
/// then every 8 s, so an unplugged interface does not cost a busy loop.
#[derive(Clone, Copy, Debug, Default)]
pub struct Backoff {
    tries: u32,
    next: Option<Instant>,
}

impl Backoff {
    /// The wait before attempt `n` (0-based).
    pub fn wait(n: u32) -> Duration {
        Duration::from_millis(500u64 << n.min(4))
    }

    /// Whether to try now; when it says yes, the next try is scheduled.
    pub fn due(&mut self, now: Instant) -> bool {
        match self.next {
            Some(t) if now < t => false,
            _ => {
                self.next = Some(now + Backoff::wait(self.tries));
                self.tries += 1;
                true
            }
        }
    }

    pub fn tries(&self) -> u32 {
        self.tries
    }

    /// The device is back: start over.
    pub fn reset(&mut self) {
        *self = Backoff::default();
    }
}

// ------------------------------------------------------------ the log

/// The log starts over once it passes this.
pub const LOG_MAX: u64 = 1_000_000;

static LOG_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Where [`log`] writes (`audio.log` in the data folder).
pub fn set_log_path(path: PathBuf) {
    *LOG_PATH.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
}

pub fn log_path() -> Option<PathBuf> {
    LOG_PATH.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Appends a timestamped line to the audio log: each stream opened (host,
/// device, rate, channels, format, buffer), stream errors, reopens. Never
/// call it from an audio callback.
pub fn log(line: &str) {
    if let Some(p) = log_path() {
        let _ = append_capped(&p, line, LOG_MAX);
    }
}

/// Appends `"<unix seconds> <line>"` to `path`, starting the file over
/// first when it is already `cap` bytes or more.
pub fn append_capped(path: &Path, line: &str, cap: u64) -> std::io::Result<()> {
    if std::fs::metadata(path).is_ok_and(|m| m.len() >= cap) {
        std::fs::write(path, b"")?;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{secs} {line}")
}

/// Logs panics (with the thread's name) before the usual report.
pub fn install_panic_log() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let name = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_string();
        log(&format!("panic on thread \"{name}\": {info}"));
        prev(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn hush_silences_and_counts_once_closing() {
        let h = Hush::default();
        let mut out = [0.5f32; 4];
        assert!(!h.silenced(&mut out, 0.0));
        assert_eq!(out, [0.5; 4]);
        h.on.store(true, Ordering::SeqCst);
        assert!(h.silenced(&mut out, 0.0));
        assert_eq!(out, [0.0; 4]);
        assert_eq!(h.calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn settle_waits_for_three_silent_callbacks() {
        let h = Arc::new(Hush::default());
        let h2 = h.clone();
        // A fake device calling back every millisecond.
        let dev = std::thread::spawn(move || {
            let mut buf = [1i16; 8];
            for _ in 0..2000 {
                h2.silenced(&mut buf, 0);
                if h2.calls.load(Ordering::Relaxed) >= 5 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let seen = h.settle_within(Duration::from_secs(5));
        assert!(seen >= HUSH_CALLS);
        dev.join().unwrap();
    }

    #[test]
    fn settle_gives_up_on_a_stream_that_stopped() {
        let h = Hush::default();
        let t = Instant::now();
        assert_eq!(h.settle_within(Duration::from_millis(30)), 0);
        let took = t.elapsed();
        assert!(took >= Duration::from_millis(30) && took < Duration::from_secs(2));
        assert!(h.closing());
    }

    #[test]
    fn a_fault_in_a_callback_leaves_silence() {
        let fault = AtomicBool::new(false);
        let mut out = [0.0f32; 4];
        // (The panic's message shows in the test output; that is expected.)
        guarded(&mut out, 0.0, &fault, |o| {
            o.fill(0.9);
            panic!("bad driver data (expected by this test)");
        });
        assert_eq!(out, [0.0; 4]);
        assert!(fault.load(Ordering::Relaxed));
        let ok = AtomicBool::new(false);
        guarded(&mut out, 0.0, &ok, |o| o.fill(0.25));
        assert_eq!(out, [0.25; 4]);
        assert!(!ok.load(Ordering::Relaxed));
    }

    #[test]
    fn stats_keep_the_real_buffer_size() {
        let s = CallbackStats::default();
        assert_eq!(s.buffer(), None);
        s.tick(480);
        s.tick(512);
        assert_eq!(s.first_frames.load(Ordering::Relaxed), 480);
        assert_eq!(s.buffer(), Some(512));
        assert_eq!(s.calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn a_stream_that_stops_calling_back_is_noticed() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut w = Watchdog::new(t0, STALL);
        // Calling back: fine however long it runs.
        for i in 1..50u64 {
            assert!(!w.stalled(i, t0 + ms(100 * i)));
        }
        // Stops at 49 callbacks (t = 4.9 s).
        assert!(!w.stalled(49, t0 + ms(6_000)));
        assert!(w.stalled(49, t0 + ms(6_900)));
        // Reported once, then the count starts again.
        assert!(!w.stalled(49, t0 + ms(7_000)));
        assert!(w.stalled(49, t0 + ms(8_900)));
        // It comes back.
        assert!(!w.stalled(50, t0 + ms(20_000)));
    }

    #[test]
    fn a_stream_that_never_starts_is_noticed() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(t0, STALL);
        assert!(!w.stalled(0, t0 + Duration::from_millis(1_000)));
        assert!(w.stalled(0, t0 + STALL));
    }

    #[test]
    fn reopening_waits_longer_each_time() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut b = Backoff::default();
        assert!(b.due(t0));
        assert!(!b.due(t0 + ms(400)));
        assert!(b.due(t0 + ms(500)));
        // Next after a second more.
        assert!(!b.due(t0 + ms(1_400)));
        assert!(b.due(t0 + ms(1_500)));
        assert_eq!(b.tries(), 3);
        assert_eq!(Backoff::wait(10), ms(8_000));
        b.reset();
        assert!(b.due(t0 + ms(1_501)));
    }

    #[test]
    fn the_log_starts_over_at_its_cap() {
        let dir = std::env::temp_dir().join(format!("af2-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("audio.log");
        append_capped(&p, "play: WASAPI on \"Speakers\"", 100).unwrap();
        append_capped(&p, "second", 100).unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        assert_eq!(s.lines().count(), 2);
        assert!(s
            .lines()
            .next()
            .unwrap()
            .ends_with(" play: WASAPI on \"Speakers\""));
        // Past the cap: the next line starts a fresh file.
        std::fs::write(&p, vec![b'x'; 120]).unwrap();
        append_capped(&p, "fresh", 100).unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        assert_eq!(s.lines().count(), 1);
        assert!(s.ends_with(" fresh\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
