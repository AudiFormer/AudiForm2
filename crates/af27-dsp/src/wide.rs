// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.
// The speaker grouping, the per-pair copies, the linked key and the
// effect lists are ported from Audiform (Audiform/src-tauri/src/dsp/
// effects/wide.rs), written by its author for Audiform and relicensed
// here by them. See docs/clean-room.md.

//! Effects on more than two channels: a 3D Space master (its speaker
//! bed) or a surround file in the Waveform editor, from 3.0 up to 7.1.4.
//!
//! - The Reverb, the Convolution Reverb and Modulation (`af.*`) run
//!   natively on the bed through a registered [`WideFactory`].
//! - Amplify and Invert work on every channel as they are.
//! - Every other effect runs as a set of stereo copies with the same
//!   settings, one per speaker pair (L and R, Ls and Rs, the height
//!   pairs...), and a copy per lone speaker (C) fed the same sound on
//!   both sides and averaged back. Each copy is set apart with
//!   [`Live::vary`], so sweeps and drifts do not move in lockstep.
//! - The Compressor, Limiter and Noise Gate listen to the loudest
//!   channel of the whole bed, so every copy moves its gain together and
//!   the image holds still.
//! - Delays, pitch and tone shapers leave the low-frequency channel alone
//!   (it only carries the deep end), held back by the effect's latency so
//!   it stays in time; filters and repair work on it too.
//! - Effects whose controls name left and right (Stereo Width, Channel
//!   Mixer, Center Channel Extractor) keep working on L and R only.
//!
//! Everything is allocated when a chain is built; processing a block
//! never allocates or locks.

use crate::effects::Effect;
use crate::live::{build_slot, Live};
use af27_core::time::db_to_gain;
use af27_core::EffectSlot;

/// Effects that only work on the front left and right (their controls
/// name left and right).
pub const STEREO_ONLY: [&str; 4] = [
    "stereo-width",
    "af.stereo_width",
    "af.channel_mixer",
    "af.center_extractor",
];

/// Effects whose copies move their gain together, from the loudest channel.
pub const LINKED: [&str; 3] = ["compressor", "limiter", "noise-gate"];

/// Effects that leave the low-frequency channel alone.
pub const SKIP_LFE: [&str; 8] = [
    "echo",
    "reverb",
    "af.phase_curve",
    "af.modulation",
    "af.amp_model",
    "af.tube_preamp",
    "af.opto_leveler",
    "af.vox_strip",
];

/// Effects that work on every channel as they are (a plain gain).
pub const ALL_CHANNELS: [&str; 2] = ["amplify", "invert"];

/// An effect running on interleaved frames of a fixed number of channels.
pub trait Wide: Send {
    /// Processes interleaved frames in place.
    fn process(&mut self, buf: &mut [f32]);
    /// New settings. Must not allocate.
    fn set(&mut self, vals: &[f32]);
    /// Clears tails and envelopes (on seek or stop).
    fn reset(&mut self) {}
    /// How many frames late the output is.
    fn latency(&self) -> usize {
        0
    }
}

/// Builds a native bed form for a slot another crate knows (the `af.*`
/// Reverb, Convolution Reverb and Modulation): slot, rate, channel names,
/// most frames per block.
pub type WideFactory = fn(&EffectSlot, u32, &[String], usize) -> Option<Box<dyn Wide>>;

static WIDE_FACTORY: std::sync::OnceLock<WideFactory> = std::sync::OnceLock::new();

pub fn register_wide_factory(f: WideFactory) {
    let _ = WIDE_FACTORY.set(f);
}

/// The channel names for a file of `n` channels: M for mono, L and R for
/// stereo, the speakers of the usual layouts (WAV order), numbers for
/// anything else.
pub fn labels(n: usize) -> Vec<String> {
    let named: &[&str] = match n {
        1 => &["M"],
        2 => &["L", "R"],
        3 => &["L", "R", "C"],
        4 => &["L", "R", "Ls", "Rs"],
        5 => &["L", "R", "C", "Ls", "Rs"],
        6 => &["L", "R", "C", "LFE", "Ls", "Rs"],
        7 => &["L", "R", "C", "LFE", "Cs", "Ls", "Rs"],
        8 => &["L", "R", "C", "LFE", "Lrs", "Rrs", "Lss", "Rss"],
        10 => &["L", "R", "C", "LFE", "Ls", "Rs", "Ltf", "Rtf", "Ltr", "Rtr"],
        12 => &[
            "L", "R", "C", "LFE", "Lrs", "Rrs", "Lss", "Rss", "Ltf", "Rtf", "Ltr", "Rtr",
        ],
        _ => &[],
    };
    if named.len() == n {
        named.iter().map(|s| (*s).to_string()).collect()
    } else {
        (1..=n).map(|k| k.to_string()).collect()
    }
}

/// The speaker groups an effect runs on: pairs by name (L with R, Lss
/// with Rss, Ltf with Rtf...), then the rest in pairs by order (files
/// with unnamed channels) or alone (C). The low-frequency channel is
/// left out when `skip_lfe`.
pub fn groups(names: &[String], skip_lfe: bool) -> Vec<Vec<usize>> {
    let mut used = vec![false; names.len()];
    let mut out = Vec::new();
    for (i, n) in names.iter().enumerate() {
        if used[i] || n == "LFE" {
            continue;
        }
        let Some(rest) = n.strip_prefix('L') else {
            continue;
        };
        let twin = format!("R{rest}");
        if let Some(j) = names.iter().position(|m| *m == twin).filter(|j| !used[*j]) {
            used[i] = true;
            used[j] = true;
            out.push(vec![i, j]);
        }
    }
    let mut numbered: Vec<usize> = Vec::new();
    for (i, n) in names.iter().enumerate() {
        if used[i] || (skip_lfe && n == "LFE") {
            continue;
        }
        if n.parse::<u32>().is_ok() {
            numbered.push(i);
        } else {
            out.push(vec![i]);
        }
    }
    out.extend(numbered.chunks(2).map(<[usize]>::to_vec));
    out.sort_by_key(|g| g[0]);
    out
}

/// Where a named speaker sits: (x, y, height), x to the right, y ahead.
fn spot(name: &str) -> Option<[f32; 3]> {
    Some(match name {
        "L" => [-1.0, 1.0, 0.0],
        "R" => [1.0, 1.0, 0.0],
        "C" => [0.0, 1.0, 0.0],
        "Ls" | "Lrs" => [-1.0, -1.0, 0.0],
        "Rs" | "Rrs" => [1.0, -1.0, 0.0],
        "Cs" => [0.0, -1.0, 0.0],
        "Lss" => [-1.0, 0.0, 0.0],
        "Rss" => [1.0, 0.0, 0.0],
        "Ltm" => [-1.0, 0.0, 1.0],
        "Rtm" => [1.0, 0.0, 1.0],
        "Ltf" => [-1.0, 1.0, 1.0],
        "Rtf" => [1.0, 1.0, 1.0],
        "Ltr" => [-1.0, -1.0, 1.0],
        "Rtr" => [1.0, -1.0, 1.0],
        _ => return None,
    })
}

/// The speakers an effect that spreads across the room works on, in ring
/// order: the floor clockwise from the front, then the heights clockwise.
/// The low-frequency channel is left out. Channels with no known place
/// (numbered ones) follow in their own order.
pub fn ring(names: &[String]) -> Vec<usize> {
    let mut placed: Vec<(bool, f32, usize)> = Vec::new();
    let mut rest = Vec::new();
    for (i, n) in names.iter().enumerate() {
        if n == "LFE" {
            continue;
        }
        match spot(n) {
            Some([x, y, z]) => {
                // Clockwise from straight ahead, 0 to a full turn.
                let a = x.atan2(y).rem_euclid(std::f32::consts::TAU);
                placed.push((z > 0.0, a, i));
            }
            None => rest.push(i),
        }
    }
    placed.sort_by(|a, b| {
        (a.0, a.1)
            .partial_cmp(&(b.0, b.1))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    placed.into_iter().map(|p| p.2).chain(rest).collect()
}

/// A gain, the same on every channel.
struct AllChannels {
    g: f32,
}

impl Wide for AllChannels {
    fn process(&mut self, buf: &mut [f32]) {
        for s in buf {
            *s *= self.g;
        }
    }
    fn set(&mut self, vals: &[f32]) {
        if self.g >= 0.0 {
            self.g = db_to_gain(vals.first().copied().unwrap_or(0.0));
        }
    }
}

/// Passes the sound through (a slot whose effect cannot run live).
struct Pass;

impl Wide for Pass {
    fn process(&mut self, _buf: &mut [f32]) {}
    fn set(&mut self, _vals: &[f32]) {}
}

/// One stereo copy of the effect and the speakers it works on (a pair,
/// or one fed to both sides).
struct Unit {
    a: usize,
    b: Option<usize>,
    proc: Box<dyn Live>,
    l: Vec<f32>,
    r: Vec<f32>,
}

/// The longest look-ahead the passed channels can be held back by.
const MAX_PASS: usize = 8192;

struct Pairs {
    ch: usize,
    block: usize,
    linked: bool,
    units: Vec<Unit>,
    /// The loudest channel of each frame (absolute), for linked effects.
    key: Vec<f32>,
    /// Channels no copy works on, held back by the effect's look-ahead so
    /// they stay in time: a ring of `cap` frames of them.
    passed: Vec<usize>,
    ring: Vec<f32>,
    cap: usize,
    at: usize,
}

impl Pairs {
    fn run(&mut self, buf: &mut [f32]) {
        let ch = self.ch;
        let frames = buf.len() / ch;
        if self.linked {
            for (k, f) in self.key[..frames].iter_mut().zip(buf.chunks_exact(ch)) {
                *k = f.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            }
        }
        for u in &mut self.units {
            let (l, r) = (&mut u.l[..frames], &mut u.r[..frames]);
            for (i, f) in buf.chunks_exact(ch).enumerate() {
                l[i] = f[u.a];
                r[i] = f[u.b.unwrap_or(u.a)];
            }
            if self.linked {
                u.proc.process_keyed(l, r, &self.key[..frames]);
            } else {
                u.proc.process(l, r);
            }
            for (i, f) in buf.chunks_exact_mut(ch).enumerate() {
                match u.b {
                    Some(b) => {
                        f[u.a] = l[i];
                        f[b] = r[i];
                    }
                    None => f[u.a] = (l[i] + r[i]) * 0.5,
                }
            }
        }
        let lat = self.latency().min(self.cap - 1);
        let np = self.passed.len();
        if np == 0 || lat == 0 {
            return;
        }
        for f in buf.chunks_exact_mut(ch) {
            let w = self.at * np;
            let rd = ((self.at + self.cap - lat) % self.cap) * np;
            for (j, c) in self.passed.iter().enumerate() {
                self.ring[w + j] = f[*c];
                f[*c] = self.ring[rd + j];
            }
            self.at = (self.at + 1) % self.cap;
        }
    }
}

impl Wide for Pairs {
    fn process(&mut self, buf: &mut [f32]) {
        let step = self.block * self.ch;
        for part in buf.chunks_mut(step) {
            self.run(part);
        }
    }

    fn set(&mut self, vals: &[f32]) {
        for u in &mut self.units {
            u.proc.set(vals);
        }
    }

    fn reset(&mut self) {
        for u in &mut self.units {
            u.proc.reset();
        }
        self.ring.fill(0.0);
    }

    fn latency(&self) -> usize {
        self.units.first().map_or(0, |u| u.proc.latency())
    }
}

/// The processor for one slot over channels named `names`, processing
/// blocks of up to `block` frames. None when the effect cannot run live.
pub fn build_wide(
    slot: &EffectSlot,
    rate: u32,
    names: &[String],
    block: usize,
) -> Option<Box<dyn Wide>> {
    let id = slot.effect.as_str();
    let ch = names.len().max(1);
    let block = block.max(1);
    if names.len() > 2 {
        if let Some(w) = WIDE_FACTORY.get().and_then(|f| f(slot, rate, names, block)) {
            return Some(w);
        }
    }
    if ALL_CHANNELS.contains(&id) {
        let e = Effect::from_id(id)?;
        let mut g = AllChannels {
            g: if e == Effect::Invert { -1.0 } else { 1.0 },
        };
        let d = e.defaults();
        let vals: Vec<f32> = (0..d.len())
            .map(|i| slot.values.get(i).copied().unwrap_or(d[i]))
            .collect();
        g.set(&vals);
        return Some(Box::new(g));
    }
    let skip_lfe = SKIP_LFE.contains(&id);
    let all = groups(names, skip_lfe);
    let chosen: Vec<Vec<usize>> = if STEREO_ONLY.contains(&id) {
        // The front pair by name, or the first group (a mono file).
        let front = (
            names.iter().position(|n| n == "L"),
            names.iter().position(|n| n == "R"),
        );
        match front {
            (Some(l), Some(r)) => vec![vec![l, r]],
            _ => all.into_iter().take(1).collect(),
        }
    } else {
        all
    };
    let mut units = Vec::with_capacity(chosen.len());
    for (k, g) in chosen.iter().enumerate() {
        let mut proc = build_slot(slot, rate)?;
        // Each pair its own: sweeps and drifts start at their own point.
        proc.vary(k);
        units.push(Unit {
            a: g[0],
            b: g.get(1).copied(),
            proc,
            l: vec![0.0; block],
            r: vec![0.0; block],
        });
    }
    let passed: Vec<usize> = (0..ch)
        .filter(|c| !chosen.iter().any(|g| g.contains(c)))
        .collect();
    let lat = units.first().map_or(0, |u| u.proc.latency());
    let cap = lat.max(MAX_PASS) + 1;
    Some(Box::new(Pairs {
        ch,
        block,
        linked: LINKED.contains(&id),
        units,
        key: vec![0.0; block],
        ring: vec![0.0; cap * passed.len()],
        passed,
        cap,
        at: 0,
    }))
}

/// A chain of effects over interleaved frames of a fixed number of
/// channels (a speaker bed), with its own mix, like [`crate::live::Chain`].
pub struct WideChain {
    pub slots: Vec<(Box<dyn Wide>, bool)>,
    channels: usize,
    dry: Vec<f32>,
    pub mix: f32,
    /// The rack's input and output gains (linear) and power, as in
    /// [`crate::live::Chain`].
    pub input: f32,
    pub output: f32,
    pub bypass: bool,
}

impl WideChain {
    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|(_, bypass)| *bypass)
    }

    /// Takes a rack's settings (no allocation).
    pub fn set_rack(&mut self, rack: &af27_core::Rack) {
        self.bypass = rack.bypass;
        self.input = db_to_gain(rack.input_db);
        self.output = db_to_gain(rack.output_db);
        self.mix = rack.mix.clamp(0.0, 1.0);
    }

    /// Processes interleaved frames (at most the block it was built for).
    pub fn process(&mut self, buf: &mut [f32]) {
        if self.bypass {
            return;
        }
        let n = buf.len().min(self.dry.len());
        let n = n - n % self.channels;
        if self.input != 1.0 {
            buf[..n].iter_mut().for_each(|s| *s *= self.input);
        }
        if !self.is_empty() {
            let wet_only = self.mix >= 0.999;
            if !wet_only {
                self.dry[..n].copy_from_slice(&buf[..n]);
            }
            for (p, bypass) in &mut self.slots {
                if !*bypass {
                    p.process(&mut buf[..n]);
                }
            }
            if !wet_only {
                let m = self.mix.max(0.0);
                for (o, d) in buf[..n].iter_mut().zip(&self.dry[..n]) {
                    *o = *o * m + d * (1.0 - m);
                }
            }
        }
        if self.output != 1.0 {
            buf[..n].iter_mut().for_each(|s| *s *= self.output);
        }
    }

    pub fn set(&mut self, slot: usize, vals: &[f32]) {
        if let Some((p, _)) = self.slots.get_mut(slot) {
            p.set(vals);
        }
    }

    pub fn set_bypass(&mut self, slot: usize, bypass: bool) {
        if let Some((_, b)) = self.slots.get_mut(slot) {
            *b = bypass;
        }
    }

    pub fn reset(&mut self) {
        for (p, _) in &mut self.slots {
            p.reset();
        }
    }
}

/// Builds a chain over channels named `names` from stored slots, for
/// blocks of up to `block` frames. A slot whose effect cannot run live
/// passes the sound through.
pub fn build_wide_chain(
    slots: &[EffectSlot],
    rate: u32,
    names: &[String],
    block: usize,
) -> WideChain {
    let channels = names.len().max(1);
    let built = slots
        .iter()
        .map(|s| {
            let p = build_wide(s, rate, names, block).unwrap_or_else(|| Box::new(Pass));
            (p, s.bypass)
        })
        .collect();
    WideChain {
        slots: built,
        channels,
        dry: vec![0.0; block.max(1) * channels],
        mix: 1.0,
        input: 1.0,
        output: 1.0,
        bypass: false,
    }
}

/// Runs one slot over a whole buffer of any channel count (Waveform
/// editor Apply), with the file's channel names; takes away the effect's
/// latency so the result lines up with the input. None when the effect
/// cannot run live.
pub fn process_buffer(
    slot: &EffectSlot,
    input: &af27_core::AudioBuffer,
    block: usize,
) -> Option<af27_core::AudioBuffer> {
    let ch = input.num_channels();
    if ch == 0 {
        return Some(input.clone());
    }
    let names = labels(ch);
    let mut w = build_wide(slot, input.rate, &names, block)?;
    let frames = input.frames();
    let lat = w.latency();
    let mut inter = vec![0.0f32; (frames + lat) * ch];
    for (c, data) in input.channels.iter().enumerate() {
        for (i, s) in data.iter().enumerate() {
            inter[i * ch + c] = *s;
        }
    }
    for part in inter.chunks_mut(block.max(1) * ch) {
        w.process(part);
    }
    let mut out = input.clone();
    for (c, data) in out.channels.iter_mut().enumerate() {
        for (i, s) in data.iter_mut().enumerate() {
            *s = inter[(i + lat) * ch + c];
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    fn slot(id: &str, vals: &[(usize, f32)]) -> EffectSlot {
        let e = Effect::from_id(id).unwrap();
        let mut values = e.defaults();
        for (i, v) in vals {
            values[*i] = *v;
        }
        EffectSlot {
            sidechain: None,
            effect: id.into(),
            values,
            bypass: false,
            data: None,
            ext_id: None,
        }
    }

    fn bed51() -> Vec<String> {
        s(&["L", "R", "C", "LFE", "Ls", "Rs"])
    }

    fn run(p: &mut dyn Wide, frames: &[[f32; 6]]) -> Vec<[f32; 6]> {
        let mut b: Vec<f32> = frames.iter().flatten().copied().collect();
        for c in b.chunks_mut(256 * 6) {
            p.process(c);
        }
        b.chunks_exact(6)
            .map(|f| [f[0], f[1], f[2], f[3], f[4], f[5]])
            .collect()
    }

    #[test]
    fn speakers_pair_by_name() {
        let bed = s(&[
            "L", "R", "C", "LFE", "Lrs", "Rrs", "Lss", "Rss", "Ltf", "Rtf", "Ltr", "Rtr",
        ]);
        assert_eq!(
            groups(&bed, true),
            vec![
                vec![0, 1],
                vec![2],
                vec![4, 5],
                vec![6, 7],
                vec![8, 9],
                vec![10, 11]
            ]
        );
        assert!(groups(&bed, false).contains(&vec![3]));
        assert_eq!(
            groups(&s(&["1", "2", "3"]), true),
            vec![vec![0, 1], vec![2]]
        );
        assert_eq!(labels(12), bed);
        assert_eq!(labels(9), s(&["1", "2", "3", "4", "5", "6", "7", "8", "9"]));
    }

    #[test]
    fn the_ring_goes_round_the_floor_then_the_heights() {
        let bed = labels(12);
        let r = ring(&bed);
        // C, R, Rss, Rrs, Lrs, Lss, L, then Rtf, Rtr, Ltr, Ltf; no LFE.
        assert_eq!(r, vec![2, 1, 7, 5, 4, 6, 0, 9, 11, 10, 8]);
    }

    #[test]
    fn gain_reaches_every_speaker() {
        let mut g = build_wide(&slot("amplify", &[(0, -6.0206)]), 48_000, &bed51(), 256).unwrap();
        let out = run(g.as_mut(), &[[1.0; 6]]);
        assert!(
            out[0].iter().all(|v| (v - 0.5).abs() < 1e-3),
            "{:?}",
            out[0]
        );
    }

    #[test]
    fn a_filter_works_on_every_channel() {
        let hp = slot("high-pass", &[(0, 1000.0)]);
        let mut eq = build_wide(&hp, 48_000, &bed51(), 256).unwrap();
        // A steady level (DC) is cut on every channel the filter reaches.
        let out = run(eq.as_mut(), &vec![[0.5; 6]; 48_000]);
        let last = out.last().unwrap();
        assert!(last.iter().all(|v| v.abs() < 0.05), "{last:?}");
    }

    #[test]
    fn the_limiter_turns_every_speaker_down_together() {
        let lim = slot("limiter", &[(0, -6.0), (1, 0.0), (2, 0.0)]);
        let mut l = build_wide(&lim, 48_000, &bed51(), 256).unwrap();
        // Only the left surround is loud; the fronts are quiet.
        let frame = [0.1, 0.1, 0.1, 0.1, 1.0, 0.1];
        let out = run(l.as_mut(), &vec![frame; 4800]);
        let f = out.last().unwrap();
        assert!(f[4] <= 0.502, "the ceiling holds: {}", f[4]);
        for c in [0, 1, 2, 3, 5] {
            let ratio = f[c] / f[4];
            assert!(
                (ratio - 0.1).abs() < 0.01,
                "speaker {c} comes down too: {f:?}"
            );
        }
    }

    #[test]
    fn the_compressor_and_gate_move_together() {
        let comp = slot("compressor", &[(0, -30.0), (1, 10.0), (2, 0.1)]);
        let mut c = build_wide(&comp, 48_000, &bed51(), 256).unwrap();
        let frame = [0.01, 0.01, 0.01, 0.01, 1.0, 0.01];
        let out = run(c.as_mut(), &vec![frame; 9600]);
        let f = out.last().unwrap();
        // The quiet speakers, alone well below the threshold, are turned
        // down by as much as the loud one.
        assert!((f[0] / f[4] - 0.01).abs() < 1e-3, "{f:?}");
        assert!(f[0] < 0.005, "{f:?}");
        let gate = slot("noise-gate", &[(0, -30.0)]);
        let mut g = build_wide(&gate, 48_000, &bed51(), 256).unwrap();
        let out = run(g.as_mut(), &vec![frame; 9600]);
        let f = out.last().unwrap();
        // The loud surround holds the gate open for every speaker.
        assert!((f[0] - 0.01).abs() < 1e-3, "{f:?}");
    }

    #[test]
    fn echo_leaves_the_low_channel_alone_and_in_time() {
        let mut e = build_wide(&slot("echo", &[]), 48_000, &bed51(), 256).unwrap();
        assert_eq!(e.latency(), 0);
        let mut frames = vec![[0.0f32; 6]; 48_000];
        frames[0][3] = 1.0;
        frames[0][4] = 1.0;
        let out = run(e.as_mut(), &frames);
        assert_eq!(out[0][3], 1.0);
        assert!(out[1..].iter().all(|f| f[3] == 0.0));
        // The surround it reaches does echo.
        assert!(out[1..].iter().any(|f| f[4].abs() > 0.05));
    }

    /// A stand-in effect: the sound held back by a fixed look-ahead.
    struct Late {
        buf: [Vec<f32>; 2],
        at: usize,
    }

    impl Live for Late {
        fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
            let n = self.buf[0].len();
            for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                *a = std::mem::replace(&mut self.buf[0][self.at], *a);
                *b = std::mem::replace(&mut self.buf[1][self.at], *b);
                self.at = (self.at + 1) % n;
            }
        }
        fn set(&mut self, _vals: &[f32]) {}
        fn latency(&self) -> usize {
            self.buf[0].len()
        }
    }

    #[test]
    fn a_skipped_low_channel_stays_in_time_behind_a_look_ahead() {
        let names = bed51();
        let chosen = groups(&names, true);
        let units = chosen
            .iter()
            .map(|g| Unit {
                a: g[0],
                b: g.get(1).copied(),
                proc: Box::new(Late {
                    buf: [vec![0.0; 100], vec![0.0; 100]],
                    at: 0,
                }),
                l: vec![0.0; 256],
                r: vec![0.0; 256],
            })
            .collect();
        let mut p = Pairs {
            ch: 6,
            block: 256,
            linked: false,
            units,
            key: vec![0.0; 256],
            passed: vec![3],
            ring: vec![0.0; MAX_PASS + 1],
            cap: MAX_PASS + 1,
            at: 0,
        };
        let mut frames = vec![[0.0f32; 6]; 1000];
        frames[10] = [1.0; 6];
        let out = run(&mut p, &frames);
        // Every channel, the LFE too, arrives 100 frames late, together.
        for c in 0..6 {
            assert_eq!(out[110][c], 1.0, "channel {c}");
            assert_eq!(out[10][c], 0.0, "channel {c}");
        }
    }

    #[test]
    fn stereo_only_effects_stay_on_the_front_pair() {
        let mut p = build_wide(&slot("stereo-width", &[(0, 2.0)]), 48_000, &bed51(), 256).unwrap();
        let out = run(p.as_mut(), &[[1.0, 0.0, 1.0, 1.0, 1.0, 0.0]]);
        assert_eq!(&out[0][2..], &[1.0, 1.0, 1.0, 0.0]);
        assert!((out[0][0] - 1.5).abs() < 1e-6, "{:?}", out[0]);
    }

    #[test]
    fn the_reverb_fills_the_room_but_not_the_low_channel() {
        let mut v = build_wide(&slot("reverb", &[(2, 1.0)]), 48_000, &bed51(), 256).unwrap();
        let mut frames = vec![[0.0f32; 6]; 48_000];
        for c in [0, 1, 2, 4, 5] {
            frames[0][c] = 1.0;
        }
        let out = run(v.as_mut(), &frames);
        let energy = |c: usize| out[1..].iter().map(|f| f[c] * f[c]).sum::<f32>();
        for c in [0, 1, 2, 4, 5] {
            assert!(energy(c) > 1e-4, "speaker {c} gets the room");
        }
        assert_eq!(energy(3), 0.0);
        // The surrounds' copy is not the fronts' copy.
        let corr = |a: usize, b: usize| {
            let ab: f32 = out.iter().map(|f| f[a] * f[b]).sum();
            ab / (energy(a) * energy(b)).sqrt()
        };
        assert!(corr(0, 4).abs() < 0.5, "{}", corr(0, 4));
        assert!(out.iter().flatten().all(|v| v.is_finite()));
    }

    #[test]
    fn every_built_in_effect_runs_on_a_7_1_4_bed() {
        let names = labels(12);
        let mut seed = 1u32;
        let frames = 4800;
        let input: Vec<f32> = (0..frames * 12)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as f32 / u32::MAX as f32 - 0.5) * 0.5
            })
            .collect();
        let mut wide = 0;
        for e in Effect::all().filter(|e| e.realtime()) {
            let s = EffectSlot {
                sidechain: None,
                effect: e.id().into(),
                values: e.defaults(),
                bypass: false,
                data: None,
                ext_id: None,
            };
            let mut p = build_wide(&s, 48_000, &names, 256)
                .unwrap_or_else(|| panic!("{} has a multichannel form", e.id()));
            wide += 1;
            let mut b = input.clone();
            for c in b.chunks_mut(256 * 12) {
                p.process(c);
            }
            assert!(b.iter().all(|v| v.is_finite()), "{} stays finite", e.id());
            // (Amplify and the EQ leave the sound as it is at their defaults.)
            let neutral = matches!(e, Effect::Amplify | Effect::ParametricEq);
            if !STEREO_ONLY.contains(&e.id()) && !neutral {
                // Every speaker but perhaps the LFE was worked on.
                let changed = (0..12)
                    .filter(|c| (0..frames).any(|i| b[i * 12 + c] != input[i * 12 + c]))
                    .count();
                assert!(changed >= 11, "{}: {changed} channels", e.id());
            }
        }
        assert!(wide >= 14, "{wide}");
    }

    #[test]
    fn a_wide_chain_takes_settings_bypass_and_mix() {
        let slots = vec![slot("amplify", &[(0, -6.0206)])];
        let mut c = build_wide_chain(&slots, 48_000, &bed51(), 256);
        let mut b = vec![1.0f32; 6 * 16];
        c.process(&mut b);
        assert!(b.iter().all(|v| (v - 0.5).abs() < 1e-3));
        c.set(0, &[0.0]);
        let mut b = vec![1.0f32; 6 * 16];
        c.process(&mut b);
        assert!(b.iter().all(|v| (v - 1.0).abs() < 1e-6));
        c.set(0, &[-6.0206]);
        c.mix = 0.5;
        let mut b = vec![1.0f32; 6 * 16];
        c.process(&mut b);
        assert!(b.iter().all(|v| (v - 0.75).abs() < 1e-3));
        c.set_bypass(0, true);
        let mut b = vec![1.0f32; 6 * 16];
        c.process(&mut b);
        assert!(b.iter().all(|v| *v == 1.0));
    }

    #[test]
    fn a_surround_file_is_processed_on_every_channel_and_mono_still_works() {
        let mut buf = af27_core::AudioBuffer::new(48_000, 6, 4800);
        for c in &mut buf.channels {
            c.fill(0.5);
        }
        let out = process_buffer(&slot("amplify", &[(0, 6.0206)]), &buf, 256).unwrap();
        assert!(out
            .channels
            .iter()
            .flatten()
            .all(|v| (v - 1.0).abs() < 1e-3));
        // A look-ahead limiter lines up with the input on every channel.
        let mut buf = af27_core::AudioBuffer::new(48_000, 6, 4800);
        for c in &mut buf.channels {
            c[1000] = 0.25;
        }
        let out = process_buffer(&slot("limiter", &[]), &buf, 256).unwrap();
        for c in &out.channels {
            assert!((c[1000] - 0.25).abs() < 1e-3, "{}", c[1000]);
        }
        let mono = af27_core::AudioBuffer {
            rate: 48_000,
            channels: vec![vec![0.5; 100]],
        };
        let out = process_buffer(&slot("amplify", &[(0, 6.0206)]), &mono, 256).unwrap();
        assert!((out.channels[0][50] - 1.0).abs() < 1e-3);
    }
}
