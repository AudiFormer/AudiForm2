// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Restoration effects written for AudiForm 2: Click Repair, Clip Repair,
//! Noise Reducer, Hiss Reducer and Room Reducer. Slot ids and control
//! keys are the ones Audiform sessions save, so their voice chains carry
//! over; the processing is our own.
//!
//! * **Click Repair** and **Clip Repair** run the finders in
//!   `af2_dsp::repair` over a sliding window (a block with some sound
//!   either side, so a fault at a block edge is still seen whole), the
//!   same code the Diagnostics panel scans with.
//! * The three **reducers** share one streaming spectral engine: short
//!   overlapping frames, a gain per frequency, overlap-add back.
//!   - Noise and Hiss track the steady background per frequency as the
//!     lowest smoothed level seen over the last few seconds (the
//!     "learn time"), ignoring digital silence, and turn down whatever
//!     sits near it. Hiss leaves everything under its frequency alone.
//!   - Room estimates the reverberant tail in each frequency from what
//!     was there a moment ago, decayed at the room's rate, and turns that
//!     part down.

use super::{num, EffectInfo, Params, Processor};
use af2_dsp::repair::{self, Scratch, Span};
use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

pub const CLICK: &str = "af.click_repair";
pub const CLIP: &str = "af.clip_repair";
pub const NOISE: &str = "af.noise_reducer";
pub const HISS: &str = "af.hiss_reducer";
pub const ROOM: &str = "af.room_reducer";

pub fn click_info() -> EffectInfo {
    EffectInfo {
        id: CLICK,
        sidechain: false,
        name: "Click Repair",
        category: "Restoration",
        description: "Smooths over clicks, pops and lip smacks: finds tiny jumps in the waveform and redraws them from the sound around them.",
        params: vec![
            num("sensitivity", "Sensitivity", "%", 0.0, 100.0, 50.0, 1.0),
            num("width", "Longest click", "ms", 0.1, 3.0, 1.0, 0.1),
        ],
    }
}

pub fn clip_info() -> EffectInfo {
    EffectInfo {
        id: CLIP,
        sidechain: false,
        name: "Clip Repair",
        category: "Restoration",
        description: "Redraws peaks a too-hot recording flattened, then brings the level down so the rebuilt peaks fit.",
        params: vec![
            num("level", "Clipped above", "dB", -12.0, 0.0, -0.5, 0.1),
            num("longest", "Longest flat run", "ms", 0.1, 10.0, 3.0, 0.1),
            num("output", "Output level", "dB", -12.0, 0.0, -3.0, 0.1),
        ],
    }
}

pub fn noise_info() -> EffectInfo {
    EffectInfo {
        id: NOISE,
        sidechain: false,
        name: "Noise Reducer",
        category: "Restoration",
        description: "Turns down steady background sound (computer fans, air conditioning, preamp noise) under the voice, learning it as it plays.",
        params: vec![
            num("reduction", "Reduction", "dB", 0.0, 40.0, 12.0, 0.5),
            num("strength", "Strength", "", 1.0, 4.0, 2.0, 0.1),
            num("learn", "Learn time", "s", 0.5, 10.0, 2.0, 0.1).log(),
            num("smoothing", "Smoothing", "%", 0.0, 100.0, 50.0, 1.0),
        ],
    }
}

pub fn hiss_info() -> EffectInfo {
    EffectInfo {
        id: HISS,
        sidechain: false,
        name: "Hiss Reducer",
        category: "Restoration",
        description: "Turns down hiss in the top end only, leaving the body of the voice untouched.",
        params: vec![
            num("reduction", "Reduction", "dB", 0.0, 40.0, 10.0, 0.5),
            num("strength", "Strength", "", 1.0, 4.0, 1.8, 0.1),
            num("freq", "Above", "Hz", 1000.0, 12_000.0, 3000.0, 10.0).log(),
            num("learn", "Learn time", "s", 0.5, 10.0, 2.0, 0.1).log(),
        ],
    }
}

pub fn room_info() -> EffectInfo {
    EffectInfo {
        id: ROOM,
        sidechain: false,
        name: "Room Reducer",
        category: "Restoration",
        description: "Pulls back the ring of a bare room after each word, so a voice recorded at a distance sounds closer.",
        params: vec![
            num("amount", "Amount", "%", 0.0, 100.0, 50.0, 1.0),
            num("decay", "Room decay", "s", 0.1, 3.0, 0.6, 0.01).log(),
            num("reduction", "Most reduction", "dB", 0.0, 30.0, 12.0, 0.5),
        ],
    }
}

// ------------------------------------------------------ block repair

/// Runs a fix over a sliding window of each channel: `CTX` samples of
/// context, a `BLOCK` to put out, `CTX` samples of look-ahead.
const BLOCK: usize = 2048;
const CTX: usize = 256;
const WIN: usize = CTX + BLOCK + CTX;

enum Fix {
    Click { sensitivity: f32, longest: usize },
    Clip { level: f32, longest: usize, out_gain: f32 },
}

pub struct BlockRepair {
    rate: u32,
    fix: Fix,
    hist: [Vec<f32>; 2],
    /// How many new samples have come in since the last window ran.
    filled: usize,
    /// The finished block being played out, and where we are in it.
    out: [Vec<f32>; 2],
    pos: usize,
    scr: Scratch,
}

impl BlockRepair {
    fn new(fix: Fix, rate: u32) -> Self {
        Self {
            rate,
            fix,
            hist: [vec![0.0; WIN], vec![0.0; WIN]],
            filled: 0,
            out: [vec![0.0; BLOCK], vec![0.0; BLOCK]],
            pos: 0,
            scr: Scratch::with_capacity(WIN),
        }
    }

    pub fn click(p: &Params, rate: u32) -> Self {
        let mut s = Self::new(
            Fix::Click {
                sensitivity: 0.0,
                longest: 1,
            },
            rate,
        );
        s.set(p);
        s
    }

    pub fn clip(p: &Params, rate: u32) -> Self {
        let mut s = Self::new(
            Fix::Clip {
                level: 1.0,
                longest: 1,
                out_gain: 1.0,
            },
            rate,
        );
        s.set(p);
        s
    }

    fn samples(&self, ms: f64) -> usize {
        ((ms / 1000.0) * self.rate as f64).ceil().max(1.0) as usize
    }

    fn set(&mut self, p: &Params) {
        self.fix = match self.fix {
            Fix::Click { .. } => Fix::Click {
                sensitivity: p.get("sensitivity") as f32,
                longest: self.samples(p.get("width")).min(CTX - 8),
            },
            Fix::Clip { .. } => Fix::Clip {
                level: 10f32.powf(p.get("level") as f32 / 20.0),
                longest: self.samples(p.get("longest")).min(CTX - 8),
                out_gain: 10f32.powf(p.get("output") as f32 / 20.0),
            },
        };
    }

    fn run_window(&mut self) {
        for c in 0..2 {
            let h = &mut self.hist[c];
            match self.fix {
                Fix::Click {
                    sensitivity,
                    longest,
                } => repair::find_clicks_into(h, sensitivity, longest, &mut self.scr),
                Fix::Clip { level, longest, .. } => {
                    repair::find_clipped_into(h, level, longest, &mut self.scr.spans)
                }
            }
            for i in 0..self.scr.spans.len() {
                let s: Span = self.scr.spans[i];
                // Faults reaching into the played context are left as
                // they went out.
                if s.start >= 2 {
                    repair::fill(h, s);
                }
            }
            let g = match self.fix {
                Fix::Clip { out_gain, .. } => out_gain,
                Fix::Click { .. } => 1.0,
            };
            for (o, v) in self.out[c].iter_mut().zip(&h[CTX..CTX + BLOCK]) {
                *o = v * g;
            }
            h.copy_within(BLOCK.., 0);
        }
        self.pos = 0;
    }
}

impl Processor for BlockRepair {
    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.as_chunks_mut::<2>().0 {
            let at = 2 * CTX + self.filled;
            self.hist[0][at] = f[0];
            self.hist[1][at] = f[1];
            f[0] = self.out[0][self.pos];
            f[1] = self.out[1][self.pos];
            self.pos += 1;
            self.filled += 1;
            if self.filled == BLOCK {
                self.filled = 0;
                self.run_window();
            }
        }
    }

    fn latency(&self) -> usize {
        BLOCK + CTX
    }

    fn reset(&mut self) {
        for c in 0..2 {
            self.hist[c].fill(0.0);
            self.out[c].fill(0.0);
        }
        self.filled = 0;
        self.pos = 0;
    }

    fn update(&mut self, p: &Params) -> bool {
        self.set(p);
        true
    }
}

// --------------------------------------------------- spectral engine

enum Mode {
    /// Steady noise, optionally only above a frequency (in bins).
    Noise {
        reduction: f32,
        strength: f32,
        above_bin: usize,
        /// Frames per minimum-tracking sub-window.
        sub_len: usize,
        smoothing: f32,
    },
    Room {
        amount: f32,
        /// Power decay over the look-back distance.
        decay: f32,
        floor: f32,
    },
}

/// Sub-windows the noise minimum is tracked over (the learn time is
/// split into this many, so the estimate can rise again after at most
/// one learn time).
const SUBS: usize = 6;
/// How far the average background sits above the lowest smoothed level
/// seen (the level of a noisy bin swings a lot from frame to frame).
const NOISE_BIAS: f32 = 3.5;
/// Frames back the room tail is predicted from.
const ROOM_BACK: usize = 4;

struct Chan {
    inp: Vec<f32>,
    acc: Vec<f32>,
    /// Smoothed power per bin.
    pow: Vec<f32>,
    /// Running minimum this sub-window, and the minima of the last ones.
    cur_min: Vec<f32>,
    mins: Vec<Vec<f32>>,
    noise: Vec<f32>,
    gain: Vec<f32>,
    /// Recent power spectra (room look-back), a ring.
    past: Vec<Vec<f32>>,
}

pub struct Spectral {
    rate: u32,
    size: usize,
    hop: usize,
    mode: Mode,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    win: Vec<f32>,
    ch: [Chan; 2],
    frame: Vec<f32>,
    spec: Vec<Complex32>,
    scratch_f: Vec<Complex32>,
    scratch_i: Vec<Complex32>,
    tmp_gain: Vec<f32>,
    /// Samples since the last frame.
    count: usize,
    frames: usize,
    sub_frames: usize,
    sub_index: usize,
    past_at: usize,
}

impl Spectral {
    fn new(mode: Mode, rate: u32) -> Self {
        let size = if rate > 64_000 { 4096 } else { 2048 };
        let hop = size / 4;
        let bins = size / 2 + 1;
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(size);
        let inv = planner.plan_fft_inverse(size);
        // sqrt-Hann on both sides: together a Hann, which sums to 2 at 75% overlap.
        let win: Vec<f32> = (0..size)
            .map(|i| {
                (0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / size as f32).cos()).sqrt()
            })
            .collect();
        let chan = || Chan {
            inp: vec![0.0; size],
            acc: vec![0.0; size],
            pow: vec![0.0; bins],
            cur_min: vec![f32::MAX; bins],
            mins: vec![vec![f32::MAX; bins]; SUBS],
            noise: vec![0.0; bins],
            gain: vec![1.0; bins],
            past: vec![vec![0.0; bins]; ROOM_BACK],
        };
        Self {
            rate,
            size,
            hop,
            mode,
            scratch_f: fwd.make_scratch_vec(),
            scratch_i: inv.make_scratch_vec(),
            fwd,
            inv,
            win,
            ch: [chan(), chan()],
            frame: vec![0.0; size],
            spec: vec![Complex32::new(0.0, 0.0); bins],
            tmp_gain: vec![1.0; bins],
            count: 0,
            frames: 0,
            sub_frames: 0,
            sub_index: 0,
            past_at: 0,
        }
    }

    pub fn noise(p: &Params, rate: u32) -> Self {
        let mut s = Self::new(
            Mode::Noise {
                reduction: 0.0,
                strength: 1.0,
                above_bin: 0,
                sub_len: 1,
                smoothing: 0.0,
            },
            rate,
        );
        s.set_noise(p, None);
        s
    }

    pub fn hiss(p: &Params, rate: u32) -> Self {
        let mut s = Self::noise(p, rate);
        s.set_noise(p, Some(p.get("freq")));
        s
    }

    pub fn room(p: &Params, rate: u32) -> Self {
        let mut s = Self::new(
            Mode::Room {
                amount: 0.0,
                decay: 0.0,
                floor: 1.0,
            },
            rate,
        );
        s.set_room(p);
        s
    }

    fn set_noise(&mut self, p: &Params, above_hz: Option<f64>) {
        let frames_per_s = self.rate as f64 / self.hop as f64;
        let learn = p.get("learn");
        let above_bin = above_hz
            .map(|f| (f / self.rate as f64 * self.size as f64) as usize)
            .unwrap_or(0);
        let smoothing = if above_hz.is_some() {
            0.5
        } else {
            (p.get("smoothing") / 100.0) as f32
        };
        self.mode = Mode::Noise {
            reduction: 10f32.powf(-(p.get("reduction") as f32) / 20.0),
            strength: p.get("strength") as f32,
            above_bin,
            sub_len: ((learn * frames_per_s) / SUBS as f64).ceil().max(1.0) as usize,
            smoothing: smoothing * 0.9,
        };
    }

    fn set_room(&mut self, p: &Params) {
        // The tail falls 60 dB over the decay time; over the look-back
        // distance its power falls by this much.
        let back_s = (ROOM_BACK * self.hop) as f64 / self.rate as f64;
        let decay = 10f64.powf(-6.0 * back_s / p.get("decay").max(0.05)) as f32;
        self.mode = Mode::Room {
            amount: (p.get("amount") / 100.0) as f32,
            decay,
            floor: 10f32.powf(-(p.get("reduction") as f32) / 20.0),
        };
    }

    fn run_frame(&mut self) {
        let size = self.size;
        let bins = size / 2 + 1;
        // Shared by both channels: whether this frame starts a new
        // sub-window of the noise minimum.
        let mut roll = false;
        if let Mode::Noise { sub_len, .. } = self.mode {
            self.sub_frames += 1;
            if self.sub_frames >= sub_len {
                self.sub_frames = 0;
                roll = true;
            }
        }
        for c in 0..2 {
            for i in 0..size {
                self.frame[i] = self.ch[c].inp[i] * self.win[i];
            }
            if self
                .fwd
                .process_with_scratch(&mut self.frame, &mut self.spec, &mut self.scratch_f)
                .is_err()
            {
                return;
            }
            let ch = &mut self.ch[c];
            let mut total = 0.0f32;
            for b in 0..bins {
                let pw = self.spec[b].norm_sqr();
                total += pw;
                ch.pow[b] = pw + (ch.pow[b] - pw) * 0.6;
            }
            match self.mode {
                Mode::Noise {
                    reduction,
                    strength,
                    above_bin,
                    smoothing,
                    ..
                } => {
                    // Digital silence teaches nothing about the noise.
                    if total / bins as f32 > 1e-14 {
                        for b in 0..bins {
                            ch.cur_min[b] = ch.cur_min[b].min(ch.pow[b]);
                        }
                    }
                    if roll {
                        let slot = self.sub_index % SUBS;
                        ch.mins[slot].copy_from_slice(&ch.cur_min);
                        ch.cur_min.fill(f32::MAX);
                    }
                    for b in 0..bins {
                        let m = ch
                            .mins
                            .iter()
                            .map(|v| v[b])
                            .fold(ch.cur_min[b], f32::min);
                        // The average noise sits above its minimum.
                        ch.noise[b] = if m == f32::MAX { 0.0 } else { m * NOISE_BIAS };
                    }
                    for b in 0..bins {
                        let g = if b < above_bin {
                            1.0
                        } else {
                            let p = ch.pow[b].max(1e-20);
                            (1.0 - strength * ch.noise[b] / p).max(reduction)
                        };
                        self.tmp_gain[b] = g;
                    }
                    // A little smoothing across frequency, then in time
                    // (fast to open, slower to close).
                    for b in 0..bins {
                        let lo = b.saturating_sub(1);
                        let hi = (b + 2).min(bins);
                        let g = self.tmp_gain[lo..hi].iter().sum::<f32>() / (hi - lo) as f32;
                        let prev = ch.gain[b];
                        ch.gain[b] = if g > prev {
                            g
                        } else {
                            g + (prev - g) * smoothing
                        };
                    }
                }
                Mode::Room {
                    amount,
                    decay,
                    floor,
                } => {
                    let back = &ch.past[self.past_at % ROOM_BACK];
                    for b in 0..bins {
                        let tail = amount * decay * back[b];
                        let p = ch.pow[b].max(1e-20);
                        let g = (1.0 - tail / p).max(floor);
                        ch.gain[b] = g + (ch.gain[b] - g) * 0.3;
                    }
                    ch.past[self.past_at % ROOM_BACK].copy_from_slice(&ch.pow);
                }
            }
            for b in 0..bins {
                self.spec[b] *= ch.gain[b];
            }
            self.spec[0].im = 0.0;
            self.spec[bins - 1].im = 0.0;
            if self
                .inv
                .process_with_scratch(&mut self.spec, &mut self.frame, &mut self.scratch_i)
                .is_err()
            {
                return;
            }
            let norm = 1.0 / (size as f32 * 2.0);
            for i in 0..size {
                ch.acc[i] += self.frame[i] * self.win[i] * norm;
            }
        }
        if roll {
            self.sub_index += 1;
        }
        self.past_at += 1;
        self.frames += 1;
    }
}

impl Processor for Spectral {
    fn process(&mut self, buf: &mut [f32]) {
        let (size, hop) = (self.size, self.hop);
        for f in buf.as_chunks_mut::<2>().0 {
            let at = size - hop + self.count;
            for c in 0..2 {
                self.ch[c].inp[at] = f[c];
                f[c] = self.ch[c].acc[self.count];
            }
            self.count += 1;
            if self.count == hop {
                self.count = 0;
                for c in 0..2 {
                    let ch = &mut self.ch[c];
                    ch.acc.copy_within(hop.., 0);
                    ch.acc[size - hop..].fill(0.0);
                }
                self.run_frame();
                for c in 0..2 {
                    self.ch[c].inp.copy_within(hop.., 0);
                }
            }
        }
    }

    fn latency(&self) -> usize {
        self.size
    }

    fn reset(&mut self) {
        for ch in &mut self.ch {
            ch.inp.fill(0.0);
            ch.acc.fill(0.0);
            ch.gain.fill(1.0);
            for p in &mut ch.past {
                p.fill(0.0);
            }
            // The learned noise is kept: a seek does not change the room.
        }
        self.count = 0;
    }

    fn update(&mut self, p: &Params) -> bool {
        match self.mode {
            Mode::Room { .. } => self.set_room(p),
            Mode::Noise { above_bin, .. } => {
                let hz = (above_bin > 0).then(|| p.get("freq"));
                self.set_noise(p, hz);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build, params_from};

    fn rng(seed: u32) -> impl FnMut() -> f32 {
        let mut s = seed;
        move || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s as f32 / u32::MAX as f32) * 2.0 - 1.0
        }
    }

    fn fx(id: &str, set: &[(&str, f64)]) -> Box<dyn Processor> {
        let pairs: Vec<(String, f64)> = set.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        build(id, 48_000, &params_from(id, &pairs).unwrap()).unwrap()
    }

    fn run(p: &mut Box<dyn Processor>, buf: &mut [f32]) {
        for b in buf.chunks_mut(512) {
            p.process(b);
        }
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len().max(1) as f32).sqrt()
    }

    fn sine(frames: usize, hz: f32, amp: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = (2.0 * std::f32::consts::PI * hz * i as f32 / 48_000.0).sin() * amp;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn click_repair_removes_a_click_and_keeps_the_timing() {
        let mut p = fx(CLICK, &[]);
        let lat = p.latency();
        let clean = sine(48_000, 220.0, 0.3);
        let mut b = clean.clone();
        for k in 0..4 {
            b[2 * (20_000 + k)] += 0.7 * if k % 2 == 0 { 1.0 } else { -1.0 };
            b[2 * (20_000 + k) + 1] += 0.7 * if k % 2 == 0 { 1.0 } else { -1.0 };
        }
        run(&mut p, &mut b);
        let err = (19_900..20_100)
            .map(|i| (b[2 * (i + lat)] - clean[2 * i]).abs())
            .fold(0.0, f32::max);
        assert!(err < 0.03, "{err}");
    }

    #[test]
    fn clip_repair_rebuilds_peaks_and_lowers_the_level() {
        let mut p = fx(CLIP, &[("output", -6.0)]);
        let mut b: Vec<f32> = sine(48_000, 100.0, 1.3)
            .into_iter()
            .map(|s| s.clamp(-1.0, 1.0))
            .collect();
        run(&mut p, &mut b);
        let peak = b[20_000..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        // Rebuilt above 1.0, then lowered 6 dB.
        assert!(peak > 0.55 && peak < 0.75, "{peak}");
    }

    #[test]
    fn noise_reducer_turns_down_steady_noise_but_keeps_the_voice() {
        let mut r = rng(7);
        let frames = 48_000 * 5;
        let noise: Vec<f32> = (0..frames).flat_map(|_| {
            let v = r() * 0.01;
            [v, v]
        }).collect();
        // Noise alone: quieter after the learn time.
        let mut p = fx(NOISE, &[("reduction", 20.0)]);
        let mut b = noise.clone();
        run(&mut p, &mut b);
        let tail = 2 * 48_000 * 3;
        assert!(rms(&b[tail..]) < rms(&noise[tail..]) * 0.4, "{} vs {}", rms(&b[tail..]), rms(&noise[tail..]));
        // Speech-like bursts (a tone in syllables) over the noise come
        // through nearly whole. A steady tone would count as background.
        let tone: Vec<f32> = sine(frames, 440.0, 0.3)
            .chunks(2)
            .enumerate()
            .flat_map(|(i, f)| {
                let on = (i % 16_800) < 9_600;
                if on { [f[0], f[1]] } else { [0.0, 0.0] }
            })
            .collect();
        let mut p = fx(NOISE, &[("reduction", 20.0)]);
        let mut mixed: Vec<f32> = tone.iter().zip(&noise).map(|(a, b)| a + b).collect();
        run(&mut p, &mut mixed);
        let ratio = rms(&mixed[tail..]) / rms(&tone[tail..]);
        assert!((ratio - 1.0).abs() < 0.1, "{ratio}");
    }

    #[test]
    fn hiss_reducer_leaves_the_lows_alone() {
        let frames = 48_000 * 4;
        let mut r = rng(3);
        let low = sine(frames, 200.0, 0.2);
        let mut p = fx(HISS, &[("freq", 3000.0), ("reduction", 30.0)]);
        let mut b: Vec<f32> = low.iter().map(|s| s + r() * 0.003).collect();
        run(&mut p, &mut b);
        let tail = 2 * 48_000 * 3;
        let ratio = rms(&b[tail..]) / rms(&low[tail..]);
        assert!((ratio - 1.0).abs() < 0.05, "{ratio}");
    }

    #[test]
    fn room_reducer_shortens_a_tail() {
        // A burst, then a decaying tone (the room) after it.
        let frames = 48_000;
        let mut b = vec![0.0f32; frames * 2];
        for i in 0..frames {
            let t = i as f32 / 48_000.0;
            let env = if i < 9600 { 1.0 } else { (-(t - 0.2) * 8.0).exp() };
            let v = (2.0 * std::f32::consts::PI * 500.0 * t).sin() * 0.4 * env;
            b[2 * i] = v;
            b[2 * i + 1] = v;
        }
        let dry = b.clone();
        let mut p = fx(ROOM, &[("amount", 100.0), ("decay", 0.8)]);
        let lat = p.latency();
        run(&mut p, &mut b);
        let tail = |v: &[f32], off: usize| rms(&v[2 * (14_400 + off)..2 * (24_000 + off)]);
        assert!(tail(&b, lat) < tail(&dry, 0) * 0.8, "{} vs {}", tail(&b, lat), tail(&dry, 0));
    }
}
