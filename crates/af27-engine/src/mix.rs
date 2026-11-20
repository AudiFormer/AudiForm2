// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The mix graph, run in blocks of up to [`BLOCK`] frames on the audio
//! thread itself, so every fader, pan, send and mute reacts within one
//! device buffer (there is no render-ahead and no fader lag).
//!
//! Each track or bus is a strip. Per block: every audio and MIDI track
//! renders its clips (or notes) and runs its effect chain (inside its
//! rack: input gain, the effects, dry/wet mix, output gain); buses run
//! their chain on what was fed to them. Then the strip's sound, folded to
//! mono if asked (after the chain, before the fader), goes out on its
//! routes: the output (a bus or the master) at fader times balance pan,
//! and each send at its own level and pan (post-fader sends also follow
//! the fader, pre-fader sends do not; every send follows mute and solo).
//! Gains move in a straight line across a block, so faders never click.
//!
//! Order and loops: outputs and sends (on or off) and sidechain keys are
//! edges; the strips run in an order where every feeder comes first. An
//! edge that would close a loop (only in a damaged file: edits refuse
//! them) is dropped: such an output goes to the master instead, such a
//! send plays nowhere, such a key leaves its effect hearing its own
//! input.
//!
//! Delay compensation: each strip's input time `in_t` (the latest of its
//! feeders' and key sources' output times) and output time `out_t`
//! (`in_t` plus its chain's latency) are worked out once; every route,
//! key and late track input is held back so it lands in step, and the
//! whole mix is late by one `lead`. Sources are read `lead` ahead, so the
//! output still lines up with the timeline exactly; starting playback
//! primes the graph with `lead` frames.
//!
//! Sidechain keys are the source's sound after its chain and before its
//! fader (muting or turning the source down does not change the duck),
//! held back by the latency of the stages before the keyed effect.
//!
//! A 3D Space session spreads each track over the speaker bed instead of
//! panning; its master chain then runs on the bed's speakers (every
//! channel, see `af27_dsp::wide`), before the bed is heard binaurally or
//! folded to stereo, or exported as it is. Bus tracks stay stereo. In a
//! stereo session the master chain runs on the stereo mix. The master
//! fader comes last, ramped. The metronome is not part of the mix: the
//! live engine adds it after the meters and the scope (see
//! `Mix::click_into`), so it never reaches an export.
//!
//! Everything a block needs is allocated when the program is built, on
//! the UI thread. The audio thread only reads and writes those buffers.

use std::sync::Arc;

use af27_core::automation::{LanePoint, LaneTarget, SILENT_DB};
use af27_core::session::{balance, fader_gain};
use af27_core::time::db_to_gain;
use af27_core::{AudioBuffer, Session, TrackKind};
use af27_dsp::live::{build_chain, Chain, Delay};
use af27_dsp::wide::{build_wide_chain, WideChain};

use crate::synth::{self, Synth};

mod space;
pub use space::{bed_names, plays_to_speakers};

pub const BLOCK: usize = 2048;
/// Speaker channels at most (7.1.4 is 12).
pub const MAX_SPEAKERS: usize = af27_space::MAX_SPEAKERS;
/// Effect lanes move their effect's settings this often (frames).
pub const FX_STEP: usize = 256;

pub struct MixClip {
    /// The session clip's id (for its rack's live settings).
    pub id: af27_core::ClipId,
    pub buf: Arc<AudioBuffer>,
    pub start: f64,
    pub offset: f64,
    pub length: f64,
    /// Clip gain, negative when the clip's polarity is inverted.
    pub gain: f32,
    pub fade_in: f64,
    pub fade_out: f64,
    pub fade_in_skew: f32,
    pub fade_out_skew: f32,
    /// Source seconds per timeline second (1 / the stretch ratio).
    pub speed: f64,
    /// Plays its source span backwards.
    pub reversed: bool,
    /// Stretched (and pitch-shifted, and turned round) audio of the whole
    /// clip, once the background work has it; until then the clip reads
    /// its source at `speed`.
    pub prepared: Option<Arc<crate::stretch::Prepared>>,
    /// The clip's volume lane (dB, clip time) and pan lane (-1..1), each
    /// empty when it has none playing.
    pub vol: Vec<LanePoint>,
    pub pan: Vec<LanePoint>,
    /// Source Channels: per file channel, its (left, right) amounts.
    /// Empty plays the file as usual; channels past the end are silent.
    pub map: Vec<[f32; 2]>,
    /// The clip's own effects, when it has any.
    pub fx: Option<Box<ClipFx>>,
}

/// A clip's own effect rack in the mix: its chain runs on the clip's
/// sound (read ahead by the chain's latency, so it comes out in step),
/// then the result goes into the track before the track's chain.
pub struct ClipFx {
    pub chain: Chain,
    /// The rack's meters, shared with the UI.
    pub meter: Arc<RackMeter>,
    /// Effect lanes, in clip time.
    lanes: Vec<FxLane>,
    l: Vec<f32>,
    r: Vec<f32>,
    /// The chain's latency in frames: the clip is read this far ahead.
    lat: usize,
    /// Ran last block (a fresh start clears the chain first).
    running: bool,
}

impl ClipFx {
    /// The rack of `c` at `rate`, None when the clip has no effects.
    /// Allocates; on the UI thread.
    pub fn new(c: &af27_core::Clip, rate: u32) -> Option<Box<ClipFx>> {
        let slots = &c.props.chain;
        if slots.is_empty() {
            return None;
        }
        let mut chain = build_chain(slots, rate, BLOCK);
        chain.set_rack(&c.props.rack);
        let lanes = c
            .props
            .lanes
            .iter()
            .filter(|l| l.plays())
            .filter_map(|lane| {
                let LaneTarget::Effect { slot, param, .. } = &lane.target else {
                    return None;
                };
                let si = slots
                    .iter()
                    .position(|s| s.ext_id.as_deref() == Some(slot.as_str()))?;
                (*param < slots[si].values.len()).then(|| FxLane {
                    slot: si,
                    param: *param,
                    points: lane.points.clone(),
                    vals: slots[si].values.clone(),
                    live: false,
                })
            })
            .collect();
        Some(Box::new(ClipFx {
            lat: chain.latency(),
            chain,
            meter: Default::default(),
            lanes,
            l: vec![0.0; BLOCK],
            r: vec![0.0; BLOCK],
            running: false,
        }))
    }

    /// New settings for one slot (effect lanes on it keep moving their
    /// own value). No allocation.
    pub fn set(&mut self, slot: usize, vals: &[f32]) {
        self.chain.set(slot, vals);
        for f in self.lanes.iter_mut().filter(|f| f.slot == slot) {
            for (k, (d, v)) in f.vals.iter_mut().zip(vals).enumerate() {
                if k != f.param {
                    *d = *v;
                }
            }
        }
    }
}

impl MixClip {
    /// The mix clip for `c` playing `buf`. Stretched clips get their
    /// prepared buffer from the cache (queued if new).
    pub fn new(c: &af27_core::Clip, buf: Arc<AudioBuffer>) -> MixClip {
        let p = &c.props;
        let prepared = crate::stretch::needs(c).then(|| crate::stretch::prepare(&buf, c));
        let g = db_to_gain(c.gain_db);
        let lane = |target: LaneTarget| {
            c.playing_lane(&target)
                .map(|l| l.points.clone())
                .unwrap_or_default()
        };
        MixClip {
            id: c.id,
            vol: lane(LaneTarget::Volume),
            pan: lane(LaneTarget::Pan),
            map: p
                .channel_map
                .as_ref()
                .map(|m| m.iter().map(|s| [s.left, s.right]).collect())
                .unwrap_or_default(),
            fx: None,
            buf,
            start: c.start,
            offset: c.offset,
            length: c.length,
            gain: if p.inverted { -g } else { g },
            fade_in: c.fade_in,
            fade_out: c.fade_out,
            fade_in_skew: p.fade_in_skew,
            fade_out_skew: p.fade_out_skew,
            speed: 1.0 / p.stretch.applied_ratio(),
            reversed: p.reversed,
            prepared,
        }
    }
}

/// A send from a strip to a bus.
pub struct MixSend {
    pub bus: usize,
    pub pre: bool,
    pub on: bool,
    /// Linear level and the send's own pan (-1..1).
    pub level: f32,
    pub pan: f32,
    /// False when the send would close a loop (a damaged file): it plays
    /// nowhere.
    pub kept: bool,
    cur: [f32; 2],
    delay: Delay,
}

/// What an automation lane on a strip moves, for [`crate::Engine`]'s live
/// lane switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneKind {
    Volume,
    Pan,
    Effect { slot: u8, param: u8 },
}

struct FxLane {
    slot: usize,
    param: usize,
    points: Vec<LanePoint>,
    /// The slot's settings, with the automated one moved.
    vals: Vec<f32>,
    live: bool,
}

/// A strip's playing lanes. A lane switched to live (being written)
/// steps aside, and its control is heard as it is set.
#[derive(Default)]
struct StripAuto {
    vol: Vec<LanePoint>,
    pan: Vec<LanePoint>,
    vol_live: bool,
    pan_live: bool,
    fx: Vec<FxLane>,
}

impl StripAuto {
    fn plays_vol(&self) -> bool {
        !self.vol.is_empty() && !self.vol_live
    }
    fn plays_pan(&self) -> bool {
        !self.pan.is_empty() && !self.pan_live
    }
    fn plays_fx(&self) -> bool {
        self.fx.iter().any(|f| !f.live)
    }
}

/// A volume lane's value as a gain.
#[inline]
fn lane_gain(points: &[LanePoint], t: f64) -> f32 {
    match af27_core::automation::value_at(points, t) {
        Some(db) if db > SILENT_DB => db_to_gain(db as f32),
        _ => 0.0,
    }
}

/// A sidechain key one chain slot hears: the source's sound, held back.
struct KeyTap {
    src: usize,
    delay: Delay,
    l: Vec<f32>,
    r: Vec<f32>,
}

impl KeyTap {
    fn new(src: usize, frames: usize) -> KeyTap {
        KeyTap {
            src,
            delay: Delay::new(frames),
            l: vec![0.0; BLOCK],
            r: vec![0.0; BLOCK],
        }
    }

    /// Takes this block of the source's sound (`sl`, `sr`) and delays it.
    fn fill(&mut self, sl: &[f32], sr: &[f32], n: usize) {
        self.l[..n].copy_from_slice(&sl[..n]);
        self.r[..n].copy_from_slice(&sr[..n]);
        self.delay.run(&mut self.l[..n], &mut self.r[..n]);
    }
}

/// A rack's input (after its input gain) and output peaks since the UI
/// last read them, as f32 bits.
#[derive(Debug, Default)]
pub struct RackMeter {
    pub input: std::sync::atomic::AtomicU32,
    pub output: std::sync::atomic::AtomicU32,
}

impl RackMeter {
    /// Takes both peaks (linear) and starts again.
    pub fn take(&self) -> (f32, f32) {
        use std::sync::atomic::Ordering::Relaxed;
        (
            f32::from_bits(self.input.swap(0, Relaxed)),
            f32::from_bits(self.output.swap(0, Relaxed)),
        )
    }
}

fn peak_into(a: &std::sync::atomic::AtomicU32, l: &[f32], r: &[f32], gain: f32) {
    use std::sync::atomic::Ordering::Relaxed;
    let mut m = 0.0f32;
    for (x, y) in l.iter().zip(r) {
        m = m.max(x.abs()).max(y.abs());
    }
    m *= gain;
    if m > f32::from_bits(a.load(Relaxed)) {
        a.store(m.to_bits(), Relaxed);
    }
}

pub struct MixTrack {
    /// The rack's meters, shared with the UI.
    pub meter: std::sync::Arc<RackMeter>,
    pub is_bus: bool,
    /// Heard, given mute and solo (see `Session::heard`).
    pub audible: bool,
    /// Fader (linear) and pan (-1..1; ignored in 3D sessions).
    pub fader: f32,
    pub pan: f32,
    /// -1 when the track's polarity is flipped.
    pub sign: f32,
    /// Folds left and right together after the chain.
    pub mono: bool,
    pub clips: Vec<MixClip>,
    pub synth: Option<Synth>,
    /// A SoundFont in place of the built-in synth.
    pub font: Option<Box<crate::soundfont::SfPlayer>>,
    pub chain: Chain,
    /// Index of the bus track it feeds, or None for the master (also
    /// when the output would close a loop).
    pub output: Option<usize>,
    pub sends: Vec<MixSend>,
    /// 3D sessions: its place in the room, 3D Bus bed, multichannel clips.
    pub sp: Option<Box<space::TrackSpaceMix>>,
    /// Per chain slot: the track whose sound keys it (a sidechain), once
    /// the effect takes a key and the source can run first; None hears
    /// its own input.
    pub keys: Vec<Option<usize>>,
    /// When the strip's input and output happen, in frames after the
    /// sources are read (delay compensation).
    pub in_t: usize,
    pub out_t: usize,
    taps: Vec<Option<KeyTap>>,
    /// Holds a track's own sound back to its input time.
    in_delay: Delay,
    /// Holds the output route back to its destination's input time.
    out_delay: Delay,
    main_cur: [f32; 2],
    auto: Option<Box<StripAuto>>,
    /// Another strip's effect listens to this one.
    keyed: bool,
    /// The strip's sound this block: after the chain and the mono fold,
    /// before the fader.
    l: Vec<f32>,
    r: Vec<f32>,
}

impl MixTrack {
    /// Whether another track's effect listens to this one.
    pub fn is_key(&self) -> bool {
        self.keyed
    }

    /// The output route's gains as set now (before automation).
    fn main_target(&self, space: bool) -> [f32; 2] {
        if !self.audible {
            return [0.0; 2];
        }
        let auto = self.auto.as_deref();
        let fader = if auto.is_some_and(StripAuto::plays_vol) {
            1.0
        } else {
            self.fader
        };
        let g = self.sign * fader;
        if space || auto.is_some_and(StripAuto::plays_pan) {
            [g, g]
        } else {
            let [l, r] = balance(self.pan);
            [g * l, g * r]
        }
    }

    fn send_target(&self, s: &MixSend) -> [f32; 2] {
        if !self.audible || !s.on || !s.kept {
            return [0.0; 2];
        }
        let mut g = self.sign * s.level;
        if !s.pre {
            let lane = self.auto.as_deref().is_some_and(StripAuto::plays_vol);
            g *= if lane { 1.0 } else { self.fader };
        }
        let [l, r] = balance(s.pan);
        [g * l, g * r]
    }

    /// Has anything to do this block (heard, keying another strip, or
    /// still ramping down).
    fn active(&self) -> bool {
        self.audible
            || self.keyed
            || self.main_cur != [0.0; 2]
            || self.sends.iter().any(|s| s.cur != [0.0; 2])
    }
}

pub struct SpaceMix {
    pub channels: usize,
    pub lfe_channel: Option<usize>,
    pub monitor: af27_space::Monitoring,
    /// The master chain, on the speakers.
    pub chain: WideChain,
    bed: Vec<f32>,
    lfe_filter: af27_space::lfe::LfeFilter,
    /// Each speaker's peak since last read (after the master fader).
    peaks: [f32; MAX_SPEAKERS],
    /// Listening on speakers: each on its own device output.
    speakers: bool,
    /// Export: the bed channel each output channel takes (None: as is).
    bed_order: Option<Vec<usize>>,
}

pub struct Mix {
    pub tracks: Vec<MixTrack>,
    /// The master chain of a stereo session (empty in a 3D session, whose
    /// master chain is `space.chain`).
    pub master_chain: Chain,
    /// The master fader (linear), ramped to across each block.
    pub master: f32,
    master_cur: f32,
    master_lane: Vec<LanePoint>,
    master_lane_live: bool,
    master_taps: Vec<Option<KeyTap>>,
    pub duration: f64,
    pub space: Option<SpaceMix>,
    /// The click is on.
    pub metronome: bool,
    /// The tempo map's stretches, for the click.
    pub click: Vec<af27_core::midi::Segment>,
    /// How late the whole mix is (frames); sources are read this far
    /// ahead.
    pub lead: usize,
    /// When the master's input happens (frames after the sources).
    pub master_in: usize,
    /// 3D: frames placed sound is held back by for Place Elements (on
    /// top of `lead`), 0 when nothing steers; see `space`.
    steer_lead: usize,
    primed: bool,
    prime_out: Vec<f32>,
    ml: Vec<f32>,
    mr: Vec<f32>,
    /// Scratch for one route.
    sl: Vec<f32>,
    sr: Vec<f32>,
    /// Per-frame lane factors for the strip being routed.
    av: Vec<f32>,
    apl: Vec<f32>,
    apr: Vec<f32>,
    /// Every track grouped so none in a group feeds or keys another in
    /// it: each group's own work (clips, notes, chains) runs in parallel,
    /// groups in order. Without sidechains the audio and MIDI tracks form
    /// the first group and the buses follow.
    levels: Vec<Vec<usize>>,
    /// Spread track work over the engine's threads (false: all on the caller).
    pub parallel: bool,
    /// The graph.s kept edges, run order and master keys, for aligning
    /// again (an export: plug-ins are late by other amounts there).
    kept: Vec<(usize, usize)>,
    order: Vec<usize>,
    master_keys: Vec<Option<usize>>,
    rate: f64,
}

/// Stereo at fractional index `x`, folding wide files (see `stereo_frame`).
#[inline]
pub fn sample(c: &[f32], x: f64) -> f32 {
    if x < 0.0 {
        return 0.0;
    }
    let i = x as usize;
    let f = (x - i as f64) as f32;
    match (c.get(i), c.get(i + 1)) {
        (Some(a), Some(b)) => a + (b - a) * f,
        (Some(a), None) => *a,
        _ => 0.0,
    }
}

/// One frame as stereo at fractional index `x`. Mono plays on both sides;
/// more than two channels fold down in WAV order (L R C LFE, then surround
/// pairs) with centre and surrounds at -3 dB, LFE left out, scaled so a
/// full-scale bed does not clip.
#[inline]
pub fn stereo_frame(buf: &AudioBuffer, x: f64) -> (f32, f32) {
    let ch = &buf.channels;
    match ch.len() {
        0 => (0.0, 0.0),
        1 => {
            let m = sample(&ch[0], x);
            (m, m)
        }
        2 => (sample(&ch[0], x), sample(&ch[1], x)),
        n => {
            const H: f32 = std::f32::consts::FRAC_1_SQRT_2;
            let mut l = sample(&ch[0], x);
            let mut r = sample(&ch[1], x);
            let c = sample(&ch[2], x) * H;
            l += c;
            r += c;
            let mut pairs = 0.0f32;
            let mut k = 4;
            while k < n {
                l += sample(&ch[k], x) * H;
                if k + 1 < n {
                    r += sample(&ch[k + 1], x) * H;
                }
                pairs += 1.0;
                k += 2;
            }
            let norm = 1.0 / (1.0 + H + H * pairs);
            (l * norm, r * norm)
        }
    }
}

/// Whether `to` can be reached from `from` along `edges`.
fn reaches(edges: &[(usize, usize)], n: usize, from: usize, to: usize) -> bool {
    let mut seen = vec![false; n];
    let mut stack = vec![from];
    while let Some(x) = stack.pop() {
        if x == to {
            return true;
        }
        if std::mem::replace(&mut seen[x], true) {
            continue;
        }
        stack.extend(edges.iter().filter(|e| e.0 == x && !seen[e.1]).map(|e| e.1));
    }
    false
}

/// The graph's shape, worked out on the UI thread: which edges are kept
/// and the levels strips run in.
struct Plan {
    /// Kept edges (from, to): routes and keys.
    kept: Vec<(usize, usize)>,
    /// Per track and slot: the key kept.
    keys: Vec<Vec<Option<usize>>>,
    levels: Vec<Vec<usize>>,
}

/// Works out the order strips run in. Routes (`routes[i]`: every bus a
/// strip outputs or sends to, on or off) come first, then keys (`wanted
/// [i][slot]`, already limited to effects that take a key), each in
/// session order. An edge to itself, to nothing, or one that would close
/// a loop is dropped. Each strip sits one level above everything it
/// needs, so the strips within a level are independent of one another.
fn plan(n: usize, routes: &[Vec<usize>], wanted: &[Vec<Option<usize>>]) -> Plan {
    let mut kept: Vec<(usize, usize)> = Vec::new();
    let try_edge = |kept: &mut Vec<(usize, usize)>, a: usize, b: usize| {
        let ok = a != b && a < n && b < n && !reaches(kept, n, b, a);
        if ok && !kept.contains(&(a, b)) {
            kept.push((a, b));
        }
        ok
    };
    for (a, to) in routes.iter().enumerate() {
        for &b in to {
            try_edge(&mut kept, a, b);
        }
    }
    let mut keys = Vec::with_capacity(n);
    for (i, slots) in wanted.iter().enumerate() {
        keys.push(
            slots
                .iter()
                .map(|k| k.filter(|&s| try_edge(&mut kept, s, i)))
                .collect(),
        );
    }
    // Longest path from the strips that need nothing (Kahn's order).
    let mut waiting = vec![0usize; n];
    for &(_, b) in &kept {
        waiting[b] += 1;
    }
    let mut level = vec![0usize; n];
    let mut ready: Vec<usize> = (0..n).filter(|&i| waiting[i] == 0).collect();
    while let Some(x) = ready.pop() {
        for &(a, b) in &kept {
            if a == x {
                level[b] = level[b].max(level[x] + 1);
                waiting[b] -= 1;
                if waiting[b] == 0 {
                    ready.push(b);
                }
            }
        }
    }
    let top = level.iter().copied().max().unwrap_or(0);
    let mut levels = vec![Vec::new(); top + 1];
    for (i, l) in level.iter().enumerate() {
        levels[*l].push(i);
    }
    levels.retain(|l| !l.is_empty());
    Plan { kept, keys, levels }
}

/// A track list shared with the engine's threads for one block.
#[derive(Clone, Copy)]
struct TrackPtr(*mut MixTrack);
// Each thread takes a different track (see `Mix::render`).
unsafe impl Sync for TrackPtr {}
unsafe impl Send for TrackPtr {}

impl TrackPtr {
    /// The `i`th track. Callers keep indices distinct across threads.
    #[inline]
    fn get(self, i: usize) -> *mut MixTrack {
        unsafe { self.0.add(i) }
    }
}

/// Runs a strip's chain over its block, each keyed slot hearing its
/// source's sound (taken into the slot's tap and held back), then folds
/// to mono if asked. `tin` is the timeline time of the chain's input at
/// the block's first frame, for effect lanes. Reads only the key
/// sources, which have finished this block (they sit at an earlier level)
/// and are not written meanwhile.
fn run_chain(t: &mut MixTrack, tracks: TrackPtr, n: usize, tin: f64, dt: f64) {
    let MixTrack {
        chain,
        taps,
        l,
        r,
        auto,
        mono,
        meter,
        ..
    } = &mut *t;
    let in_gain = if chain.bypass { 1.0 } else { chain.input };
    peak_into(&meter.input, &l[..n], &r[..n], in_gain);
    for tap in taps.iter_mut().flatten() {
        // A different strip (keys to itself are dropped), only read here.
        let s = unsafe { &*tracks.get(tap.src) };
        tap.fill(&s.l, &s.r, n);
    }
    let fx = auto.as_deref_mut().filter(|a| a.plays_fx());
    let step = if fx.is_some() { FX_STEP } else { n.max(1) };
    let mut fx = fx;
    let mut a = 0;
    while a < n {
        let b = (a + step).min(n);
        if let Some(au) = fx.as_deref_mut() {
            let at = tin + dt * a as f64;
            for f in au.fx.iter_mut().filter(|f| !f.live) {
                if let (Some(v), Some(slot)) = (
                    af27_core::automation::value_at(&f.points, at),
                    f.vals.get_mut(f.param),
                ) {
                    *slot = v as f32;
                    chain.set(f.slot, &f.vals);
                }
            }
        }
        chain.process_keyed(&mut l[a..b], &mut r[a..b], |slot| {
            let tap = taps.get(slot)?.as_ref()?;
            Some((&tap.l[a..b], &tap.r[a..b]))
        });
        a = b;
    }
    peak_into(&meter.output, &l[..n], &r[..n], 1.0);
    if *mono {
        for (x, y) in l[..n].iter_mut().zip(r[..n].iter_mut()) {
            let m = (*x + *y) * 0.5;
            *x = m;
            *y = m;
        }
    }
}

/// A track's own sound for one block: silence, then its clips or notes
/// (read at source time `ts`), held back to its input time, then its
/// effect chain. Writes nothing but the track itself.
fn render_source(t: &mut MixTrack, tracks: TrackPtr, ts: f64, dt: f64, n: usize) {
    t.l[..n].fill(0.0);
    t.r[..n].fill(0.0);
    let t1 = ts + dt * n as f64;
    let MixTrack { clips, l, r, .. } = &mut *t;
    for c in clips.iter_mut() {
        if let Some(mut fx) = c.fx.take() {
            clip_through_fx(c, &mut fx, ts, dt, &mut l[..n], &mut r[..n]);
            c.fx = Some(fx);
            continue;
        }
        if c.start >= t1 || c.start + c.length <= ts {
            continue;
        }
        read_clip(c, ts, dt, &mut l[..n], &mut r[..n]);
    }
    if let Some(s) = &mut t.synth {
        s.render(ts, dt, &mut t.l[..n], &mut t.r[..n]);
    }
    if let Some(f) = &mut t.font {
        f.render(ts, dt, &mut t.l[..n], &mut t.r[..n]);
    }
    t.in_delay.run(&mut t.l[..n], &mut t.r[..n]);
    let tin = ts - dt * t.in_t as f64;
    run_chain(t, tracks, n, tin, dt);
}

/// Adds one block of a clip with its own effects into `l` and `r`: the
/// clip is read `fx.lat` frames ahead into the rack's buffers, run
/// through its chain (effect lanes moving every [`FX_STEP`] frames) and
/// what lands inside the clip is added. The chain is cleared when the
/// clip starts sounding again. No allocation.
fn clip_through_fx(c: &MixClip, fx: &mut ClipFx, ts: f64, dt: f64, l: &mut [f32], r: &mut [f32]) {
    let n = l.len().min(fx.l.len());
    let lead = dt * fx.lat as f64;
    let t1 = ts + dt * n as f64;
    // Output for ts..t1 comes from the input at ts + lead..t1 + lead.
    if c.start >= t1 + lead || c.start + c.length <= ts {
        fx.running = false;
        return;
    }
    let ClipFx {
        chain,
        meter,
        lanes,
        l: fl,
        r: fr,
        running,
        ..
    } = fx;
    if !*running {
        chain.reset();
        *running = true;
    }
    let (fl, fr) = (&mut fl[..n], &mut fr[..n]);
    fl.fill(0.0);
    fr.fill(0.0);
    read_clip(c, ts + lead, dt, fl, fr);
    let in_gain = if chain.bypass { 1.0 } else { chain.input };
    peak_into(&meter.input, fl, fr, in_gain);
    let step = if lanes.is_empty() { n.max(1) } else { FX_STEP };
    let mut a = 0;
    while a < n {
        let b = (a + step).min(n);
        let local = ts + lead + dt * a as f64 - c.start;
        for f in lanes.iter_mut() {
            if let (Some(v), Some(slot)) = (
                af27_core::automation::value_at(&f.points, local),
                f.vals.get_mut(f.param),
            ) {
                *slot = v as f32;
                chain.set(f.slot, &f.vals);
            }
        }
        chain.process(&mut fl[a..b], &mut fr[a..b]);
        a = b;
    }
    peak_into(&meter.output, fl, fr, 1.0);
    // Only what falls inside the clip (tails stop at its end).
    let first = if c.start > ts {
        (((c.start - ts) / dt).ceil() as usize).min(n)
    } else {
        0
    };
    let last = (((c.start + c.length - ts) / dt).ceil().max(0.0) as usize).min(n);
    if first < last {
        add_into(
            &mut l[first..last],
            &mut r[first..last],
            &fl[first..last],
            &fr[first..last],
        );
    }
}

/// Adds `src * gain` into `dst`, the gain moving in a straight line from
/// `from` to `to` across the block, times per-frame factors (`v` for
/// both sides, `pl`/`pr` per side) when given.
#[allow(clippy::too_many_arguments)]
#[inline]
fn gain_into(
    sl: &mut [f32],
    sr: &mut [f32],
    src_l: &[f32],
    src_r: &[f32],
    from: [f32; 2],
    to: [f32; 2],
    v: Option<&[f32]>,
    pan: Option<(&[f32], &[f32])>,
) {
    let n = sl.len();
    let k = 1.0 / n.max(1) as f32;
    let dl = (to[0] - from[0]) * k;
    let dr = (to[1] - from[1]) * k;
    for i in 0..n {
        let x = (i + 1) as f32;
        let mut gl = from[0] + dl * x;
        let mut gr = from[1] + dr * x;
        if let Some(v) = v {
            gl *= v[i];
            gr *= v[i];
        }
        if let Some((pl, pr)) = pan {
            gl *= pl[i];
            gr *= pr[i];
        }
        sl[i] = src_l[i] * gl;
        sr[i] = src_r[i] * gr;
    }
}

#[inline]
fn add_into(dl: &mut [f32], dr: &mut [f32], sl: &[f32], sr: &[f32]) {
    for ((a, b), (x, y)) in dl.iter_mut().zip(dr.iter_mut()).zip(sl.iter().zip(sr)) {
        *a += x;
        *b += y;
    }
}

/// The clip's gain at `local` seconds into it: clip gain and fades (with
/// their skew; see `af27_core::session::fade_curve`).
#[inline]
fn clip_gain(c: &MixClip, local: f64) -> f32 {
    use af27_core::session::fade_curve;
    let mut g = c.gain as f64;
    if c.fade_in > 0.0 && local < c.fade_in {
        g *= fade_curve(local / c.fade_in, c.fade_in_skew);
    }
    if c.fade_out > 0.0 && c.length - local < c.fade_out {
        g *= fade_curve((c.length - local) / c.fade_out, c.fade_out_skew);
    }
    if !c.vol.is_empty() {
        g *= lane_gain(&c.vol, local) as f64;
    }
    g as f32
}

/// One frame through a clip's Source Channels: each file channel at its
/// left and right amounts.
#[inline]
fn mapped_frame(buf: &AudioBuffer, map: &[[f32; 2]], x: f64) -> (f32, f32) {
    let (mut l, mut r) = (0.0f32, 0.0f32);
    for (src, m) in buf.channels.iter().zip(map) {
        if m[0] == 0.0 && m[1] == 0.0 {
            continue;
        }
        let v = sample(src, x);
        l += v * m[0];
        r += v * m[1];
    }
    (l, r)
}

/// Adds one clip's sound for the block starting at `t0` (frames `dt`
/// apart) into `l` and `r`: from its prepared (stretched) audio when that
/// is ready, otherwise from its source at its speed and direction.
fn read_clip(c: &MixClip, t0: f64, dt: f64, l: &mut [f32], r: &mut [f32]) {
    match c.prepared.as_deref().and_then(|p| p.ready()) {
        Some(buf) => read_span(c, buf, 0.0, 1.0, false, t0, dt, l, r),
        None => read_span(c, &c.buf, c.offset, c.speed, c.reversed, t0, dt, l, r),
    }
}

/// [`read_clip`] from `buf`, whose audio for the clip starts at `offset`
/// seconds and runs `speed` source seconds per timeline second (backwards
/// from the span's end when `reversed`). Only the frames inside the clip
/// are visited; mono and stereo files read their channels directly, and a
/// file at the output rate is read without interpolation.
#[allow(clippy::too_many_arguments)]
fn read_span(
    c: &MixClip,
    buf: &AudioBuffer,
    offset: f64,
    speed: f64,
    reversed: bool,
    t0: f64,
    dt: f64,
    l: &mut [f32],
    r: &mut [f32],
) {
    let n = l.len();
    let first = if c.start > t0 {
        (((c.start - t0) / dt).ceil() as usize).min(n)
    } else {
        0
    };
    let last = (((c.start + c.length - t0) / dt).ceil().max(0.0) as usize).min(n);
    if first >= last {
        return;
    }
    let rate = buf.rate as f64;
    let local0 = t0 + dt * first as f64 - c.start;
    let (x0, step) = if reversed {
        // The last frame of the span first (matching a reversed copy).
        let end = (c.length * rate).round() - 1.0;
        (
            offset * rate + (end - local0 * rate) * speed,
            -dt * rate * speed,
        )
    } else {
        ((offset + local0 * speed) * rate, dt * rate * speed)
    };
    let ch = &buf.channels;
    // The fades change the gain; in the plain middle of a clip it is fixed.
    let in_fade = |local: f64| {
        (c.fade_in > 0.0 && local < c.fade_in)
            || (c.fade_out > 0.0 && c.length - local < c.fade_out)
    };
    let local_first = t0 + dt * first as f64 - c.start;
    let local_last = t0 + dt * (last - 1) as f64 - c.start;
    let flat = !in_fade(local_first) && !in_fade(local_last) && c.vol.is_empty();
    let whole = (step - 1.0).abs() < 1e-9 && (x0 - x0.round()).abs() < 1e-6 && x0 >= -0.5;
    // Source Channels and a pan lane go frame by frame.
    let plain = c.map.is_empty() && c.pan.is_empty();
    match ch.len() {
        2 if whole && plain => {
            let start = x0.round() as usize;
            let (a, b) = (&ch[0], &ch[1]);
            let avail = a.len().min(b.len());
            if start >= avail {
                return;
            }
            let count = (last - first).min(avail - start);
            let (sa, sb) = (&a[start..start + count], &b[start..start + count]);
            let (dl, dr) = (&mut l[first..first + count], &mut r[first..first + count]);
            if flat {
                let g = c.gain;
                for (((o, p), x), y) in dl.iter_mut().zip(dr.iter_mut()).zip(sa).zip(sb) {
                    *o += x * g;
                    *p += y * g;
                }
            } else {
                for (k, (((o, p), x), y)) in
                    dl.iter_mut().zip(dr.iter_mut()).zip(sa).zip(sb).enumerate()
                {
                    let g = clip_gain(c, local_first + dt * k as f64);
                    *o += x * g;
                    *p += y * g;
                }
            }
        }
        _ => {
            for i in first..last {
                let k = (i - first) as f64;
                let local = local_first + dt * k;
                if local < 0.0 || local >= c.length {
                    continue;
                }
                let g = if flat { c.gain } else { clip_gain(c, local) };
                let x = x0 + step * k;
                let (mut a, mut b) = if c.map.is_empty() {
                    stereo_frame(buf, x)
                } else {
                    mapped_frame(buf, &c.map, x)
                };
                if !c.pan.is_empty() {
                    let p = af27_core::automation::value_at(&c.pan, local).unwrap_or(0.0);
                    let [pl, pr] = balance(p as f32);
                    a *= pl;
                    b *= pr;
                }
                l[i] += a * g;
                r[i] += b * g;
            }
        }
    }
}

/// Fader and pan gains for a track (fader only in 3D sessions).
pub fn track_gains(t: &af27_core::Track, space: bool) -> (f32, f32) {
    if space {
        let g = fader_gain(t.volume_db);
        (g, g)
    } else {
        t.gains()
    }
}

/// A note's 0..1 velocity as MIDI's 1..127.
fn velocity_u8(v: f32) -> u8 {
    (v * 127.0).round().clamp(1.0, 127.0) as u8
}

/// A MIDI track's instrument with its notes: its SoundFont, or else the
/// built-in synth. A font that is missing or cannot play leaves the track
/// silent.
fn instruments(
    t: &af27_core::Track,
    tempo: &af27_core::midi::Tempo,
    rate: u32,
) -> (Option<Synth>, Option<Box<crate::soundfont::SfPlayer>>) {
    let notes = af27_core::midi::plan_notes(&t.midi, tempo);
    if let Some(sf) = &t.soundfont {
        let list = notes
            .iter()
            .map(|n| crate::soundfont::SfNote {
                start: n.start,
                end: n.end,
                key: n.pitch,
                velocity: velocity_u8(n.velocity),
                pan: n.pan,
                detune: n.detune,
            })
            .collect();
        let font = crate::soundfont::SfPlayer::new(sf, list, rate)
            .ok()
            .map(Box::new);
        return (None, font);
    }
    let list = notes
        .iter()
        .map(|n| {
            synth::note(n.start, n.end, n.pitch, velocity_u8(n.velocity), n.pan).detuned(n.detune)
        })
        .collect();
    (Some(Synth::new(t.instrument, list, rate)), None)
}

/// A strip's playing lanes, as the engine plays them (None with none).
fn strip_auto(t: &af27_core::Track) -> Option<Box<StripAuto>> {
    let mut a = StripAuto::default();
    for lane in t.lanes.iter().filter(|l| l.plays()) {
        match &lane.target {
            LaneTarget::Volume if a.vol.is_empty() => a.vol = lane.points.clone(),
            LaneTarget::Pan if a.pan.is_empty() => a.pan = lane.points.clone(),
            LaneTarget::Effect { slot, param, .. } => {
                let Some(si) = t
                    .chain
                    .iter()
                    .position(|s| s.ext_id.as_deref() == Some(slot.as_str()))
                else {
                    continue;
                };
                if *param < t.chain[si].values.len() {
                    a.fx.push(FxLane {
                        slot: si,
                        param: *param,
                        points: lane.points.clone(),
                        vals: t.chain[si].values.clone(),
                        live: false,
                    });
                }
            }
            _ => {}
        }
    }
    (!a.vol.is_empty() || !a.pan.is_empty() || !a.fx.is_empty()).then(|| Box::new(a))
}

/// Builds the mix for `session` at `rate`. `media` gives each media item's
/// current audio (with any unsaved Waveform edits).
pub fn build(
    session: &Session,
    media: &dyn Fn(af27_core::MediaId) -> Option<Arc<AudioBuffer>>,
    rate: u32,
) -> Mix {
    let space = session.space;
    let heard = session.heard();
    let index_of = |id: af27_core::TrackId| session.tracks.iter().position(|t| t.id == id);
    let bus_index =
        |id: af27_core::TrackId| index_of(id).filter(|&i| session.tracks[i].kind == TrackKind::Bus);
    // 3D: the Place Elements frame, when anything steers.
    let steer_lead = if space::session_steers(session) {
        af27_space::steer::STEER_FRAME
    } else {
        0
    };
    let mut tracks: Vec<MixTrack> = session
        .tracks
        .iter()
        .zip(&heard)
        .enumerate()
        .map(|(ti, (t, &audible))| {
            let mut clips: Vec<MixClip> = Vec::new();
            // 3D: clips of more than two channels play from their speakers.
            let mut wide: Vec<MixClip> = Vec::new();
            for c in t.clips.iter().filter(|c| !c.muted) {
                let Some(buf) = media(c.media) else { continue };
                let mut mc = MixClip::new(c, buf);
                if space.is_some() && plays_to_speakers(c, mc.buf.num_channels()) {
                    wide.push(mc);
                } else {
                    mc.fx = ClipFx::new(c, rate);
                    clips.push(mc);
                }
            }
            let (synth, font) = if t.kind == TrackKind::Midi {
                instruments(t, &session.tempo, rate)
            } else {
                (None, None)
            };
            let stereo =
                t.kind != TrackKind::Audio || clips.iter().any(|c| c.buf.num_channels() >= 2);
            let sp =
                space.map(|layout| space::for_track(layout, t, wide, stereo, rate, ti, steer_lead));
            MixTrack {
                meter: Default::default(),
                is_bus: t.kind == TrackKind::Bus,
                audible,
                fader: fader_gain(t.volume_db),
                pan: t.pan,
                sign: if t.polarity { -1.0 } else { 1.0 },
                mono: t.mono,
                clips,
                synth,
                font,
                chain: {
                    let mut c = build_chain(&t.chain, rate, BLOCK);
                    c.set_rack(&t.rack);
                    c
                },
                output: t.output.and_then(bus_index),
                sends: t
                    .sends
                    .iter()
                    .filter_map(|s| {
                        Some(MixSend {
                            bus: bus_index(s.bus)?,
                            pre: s.pre_fader,
                            on: s.enabled,
                            level: db_to_gain(s.level_db.clamp(-96.0, 12.0)),
                            pan: s.pan,
                            kept: true,
                            cur: [0.0; 2],
                            delay: Delay::default(),
                        })
                    })
                    .collect(),
                sp,
                keys: Vec::new(),
                in_t: 0,
                out_t: 0,
                taps: Vec::new(),
                in_delay: Delay::default(),
                out_delay: Delay::default(),
                main_cur: [0.0; 2],
                auto: strip_auto(t),
                keyed: false,
                l: vec![0.0; BLOCK],
                r: vec![0.0; BLOCK],
            }
        })
        .collect();
    let n = tracks.len();
    // Sidechains: which track keys each slot, for effects that take a key.
    let wanted: Vec<Vec<Option<usize>>> = session
        .tracks
        .iter()
        .zip(&tracks)
        .map(|(st, mt)| {
            st.chain
                .iter()
                .zip(&mt.chain.slots)
                .map(|(slot, (p, _))| slot.sidechain.and_then(index_of).filter(|_| p.takes_key()))
                .collect()
        })
        .collect();
    let routes: Vec<Vec<usize>> = tracks
        .iter()
        .map(|t| {
            t.output
                .into_iter()
                .chain(t.sends.iter().map(|s| s.bus))
                .collect()
        })
        .collect();
    let plan = plan(n, &routes, &wanted);
    for (i, t) in tracks.iter_mut().enumerate() {
        if let Some(b) = t.output {
            if !plan.kept.contains(&(i, b)) {
                t.output = None;
            }
        }
        for s in &mut t.sends {
            s.kept = plan.kept.contains(&(i, s.bus));
        }
    }
    for (t, k) in tracks.iter_mut().zip(plan.keys) {
        t.keys = k;
    }
    // Keys on the master chain: any strip, no loops possible (the master
    // runs last).
    let master_slots: &[af27_core::EffectSlot] = if space.is_some() {
        &[]
    } else {
        &session.master_chain
    };
    let mut master_chain = build_chain(master_slots, rate, BLOCK);
    let master_keys: Vec<Option<usize>> = master_slots
        .iter()
        .zip(&master_chain.slots)
        .map(|(slot, (p, _))| slot.sidechain.and_then(index_of).filter(|_| p.takes_key()))
        .collect();
    for i in 0..n {
        let used =
            tracks.iter().any(|t| t.keys.contains(&Some(i))) || master_keys.contains(&Some(i));
        tracks[i].keyed = used;
    }

    // Delay compensation, in run order.
    let order: Vec<usize> = plan.levels.iter().flatten().copied().collect();
    let (lead, master_in, master_taps) =
        align(&mut tracks, &plan.kept, &order, &master_keys, &master_chain);

    let space_mix = space.map(|layout| {
        let speakers = af27_space::speakers(layout);
        let channels = speakers.len();
        let names: Vec<String> = speakers.iter().map(|s| s.name.to_string()).collect();
        SpaceMix {
            channels,
            lfe_channel: af27_space::lfe_channel(layout),
            monitor: af27_space::Monitoring::new(layout, session.monitor, rate),
            chain: {
                let mut c = build_wide_chain(&session.master_chain, rate, &names, BLOCK);
                c.set_rack(&session.master_rack);
                c
            },
            bed: vec![0.0; BLOCK * channels],
            lfe_filter: af27_space::lfe::LfeFilter::new(rate),
            peaks: [0.0; MAX_SPEAKERS],
            speakers: session.monitor == af27_core::space::Monitor::Speakers,
            bed_order: None,
        }
    });
    // Start the engine's threads here, on the UI thread, never on the
    // audio thread.
    let _ = crate::pool::pool();
    if space.is_none() {
        master_chain.set_rack(&session.master_rack);
    }
    let master_lane = session
        .master_lanes
        .iter()
        .find(|l| l.target == LaneTarget::Volume && l.plays())
        .map(|l| l.points.clone())
        .unwrap_or_default();
    let master = fader_gain(session.master_db);
    let mut mix = Mix {
        levels: plan.levels,
        kept: plan.kept,
        order,
        master_keys,
        parallel: true,
        tracks,
        master_chain,
        master,
        master_cur: master,
        master_lane,
        master_lane_live: false,
        master_taps,
        duration: session.duration(),
        space: space_mix,
        metronome: session.tempo.metronome,
        click: session.tempo.segments().collect(),
        lead,
        master_in,
        steer_lead,
        primed: false,
        prime_out: if steer_lead > 0 {
            vec![0.0; BLOCK * MAX_SPEAKERS]
        } else {
            Vec::new()
        },
        ml: vec![0.0; BLOCK],
        mr: vec![0.0; BLOCK],
        sl: vec![0.0; BLOCK],
        sr: vec![0.0; BLOCK],
        av: vec![1.0; BLOCK],
        apl: vec![1.0; BLOCK],
        apr: vec![1.0; BLOCK],
        rate: rate as f64,
    };
    mix.settle();
    mix
}

/// Delay compensation: when each strip's input and output happen (from
/// the latency every effect reports), and the delays on every route,
/// key and late track input that line them up. Returns the lead (how
/// late the whole mix is), the master's input time and the master
/// chain's key taps. Allocates; on the UI thread (or before an export).
fn align(
    tracks: &mut [MixTrack],
    kept: &[(usize, usize)],
    order: &[usize],
    master_keys: &[Option<usize>],
    master_chain: &Chain,
) -> (usize, usize, Vec<Option<KeyTap>>) {
    let n = tracks.len();
    let mut master_in = 0usize;
    for &i in order {
        let mut t_in = 0usize;
        for &(a, b) in kept {
            if b == i {
                t_in = t_in.max(tracks[a].out_t);
            }
        }
        let t = &mut tracks[i];
        t.in_t = t_in;
        t.out_t = t_in + t.chain.latency();
        if t.output.is_none() {
            master_in = master_in.max(t.out_t);
        }
    }
    for k in master_keys.iter().flatten() {
        master_in = master_in.max(tracks[*k].out_t);
    }
    let lead = master_in + master_chain.latency();
    for i in 0..n {
        let dest_in = |b: Option<usize>| b.map_or(master_in, |b| tracks[b].in_t);
        let out_t = tracks[i].out_t;
        let in_t = tracks[i].in_t;
        let main = dest_in(tracks[i].output).saturating_sub(out_t);
        let sends: Vec<usize> = tracks[i]
            .sends
            .iter()
            .map(|s| tracks[s.bus].in_t.saturating_sub(out_t))
            .collect();
        let taps: Vec<Option<KeyTap>> = tracks[i]
            .keys
            .iter()
            .enumerate()
            .map(|(slot, k)| {
                k.map(|src| {
                    let d = in_t.saturating_sub(tracks[src].out_t)
                        + tracks[i].chain.latency_before(slot);
                    KeyTap::new(src, d)
                })
            })
            .collect();
        let t = &mut tracks[i];
        t.out_delay = Delay::new(main);
        for (s, d) in t.sends.iter_mut().zip(sends) {
            s.delay = Delay::new(d);
        }
        t.in_delay = Delay::new(if t.is_bus { 0 } else { in_t });
        t.taps = taps;
    }
    let master_taps = master_keys
        .iter()
        .enumerate()
        .map(|(slot, k)| {
            k.map(|src| {
                KeyTap::new(
                    src,
                    master_in.saturating_sub(tracks[src].out_t) + master_chain.latency_before(slot),
                )
            })
        })
        .collect();
    (lead, master_in, master_taps)
}

impl Mix {
    /// The order tracks run in: groups of track indices, each group's
    /// tracks independent of one another (see `plan`).
    pub fn levels(&self) -> &[Vec<usize>] {
        &self.levels
    }

    /// Every gain jumps to where it is set (no ramp), as at a start.
    fn settle(&mut self) {
        let space = self.space.is_some();
        for t in &mut self.tracks {
            t.main_cur = t.main_target(space);
            let targets: Vec<[f32; 2]> = t.sends.iter().map(|s| t.send_target(s)).collect();
            for (s, g) in t.sends.iter_mut().zip(targets) {
                s.cur = g;
            }
        }
        self.master_cur = self.master;
    }

    /// Works out every stretched clip's audio now (waiting for any the
    /// background is on), so an offline render hears them all.
    pub fn prepare_now(&self) {
        for t in &self.tracks {
            for c in &t.clips {
                if let Some(p) = &c.prepared {
                    p.ensure();
                }
            }
        }
    }

    pub fn channels_out(&self) -> usize {
        self.space.as_ref().map_or(2, |s| s.channels)
    }

    /// Clears effect tails and delay lines, after a jump.
    pub fn reset(&mut self) {
        for t in &mut self.tracks {
            for fx in t.clips.iter_mut().filter_map(|c| c.fx.as_deref_mut()) {
                fx.chain.reset();
                fx.running = false;
            }
            t.chain.reset();
            t.in_delay.clear();
            t.out_delay.clear();
            for s in &mut t.sends {
                s.delay.clear();
            }
            for tap in t.taps.iter_mut().flatten() {
                tap.delay.clear();
            }
            if let Some(s) = &mut t.synth {
                s.reset();
            }
            if let Some(f) = &mut t.font {
                f.reset();
            }
        }
        for tap in self.master_taps.iter_mut().flatten() {
            tap.delay.clear();
        }
        self.master_chain.reset();
        if let Some(sp) = &mut self.space {
            sp.chain.reset();
        }
        self.space_reset();
        self.settle();
    }

    /// This mix renders offline (an export): effects that run in another
    /// process (VST3 plug-ins) start copies of their own and wait for
    /// every block.
    pub fn set_offline(&mut self) {
        for t in &mut self.tracks {
            for fx in t.clips.iter_mut().filter_map(|c| c.fx.as_deref_mut()) {
                fx.chain.set_offline(true);
                fx.lat = fx.chain.latency();
            }
            t.chain.set_offline(true);
        }
        self.master_chain.set_offline(true);
        let (lead, master_in, taps) = align(
            &mut self.tracks,
            &self.kept,
            &self.order,
            &self.master_keys,
            &self.master_chain,
        );
        self.lead = lead;
        self.master_in = master_in;
        self.master_taps = taps;
    }

    /// Starts playing from `t` (seconds): clears everything, then primes
    /// the delay lines with `lead` frames so the first frame out is the
    /// sound at `t`. No allocation.
    pub fn start(&mut self, t: f64) {
        self.reset();
        self.prime(t);
    }

    /// Renders the frames the mix is late by (delay compensation and the
    /// Place Elements frame) before `t` into nothing, so what comes out
    /// from `t` on is in step. Used at a start and when a new program
    /// arrives while playing. No allocation.
    pub fn prime(&mut self, t: f64) {
        let dt = 1.0 / self.rate;
        self.primed = true;
        let mut left = self.lead + self.steer_lead;
        let mut scratch = [0.0f32; BLOCK * 2];
        while left > 0 {
            let n = left.min(BLOCK);
            let at = t - dt * left as f64;
            // A bed renders its stereo view here: only the state matters.
            self.render(at, dt, n, &mut scratch[..n * 2], false);
            left -= n;
        }
    }

    /// The rack of the clip `id`, if it has one in the mix.
    pub fn clip_fx(&mut self, id: af27_core::ClipId) -> Option<&mut ClipFx> {
        self.tracks
            .iter_mut()
            .flat_map(|t| t.clips.iter_mut())
            .find(|c| c.id == id)
            .and_then(|c| c.fx.as_deref_mut())
    }

    /// Every clip rack's meters, by clip id (for the UI).
    pub fn clip_meters(&self) -> Vec<(af27_core::ClipId, Arc<RackMeter>)> {
        self.tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter_map(|c| Some((c.id, c.fx.as_ref()?.meter.clone())))
            .collect()
    }

    /// New fader (linear) and pan for a strip, ramped to over the next
    /// block. No allocation.
    pub fn set_strip(&mut self, track: usize, fader: f32, pan: f32) {
        if let Some(t) = self.tracks.get_mut(track) {
            t.fader = fader;
            t.pan = pan;
        }
    }

    /// New settings for one send of a strip. No allocation.
    pub fn set_send(&mut self, track: usize, send: usize, level: f32, pan: f32, on: bool) {
        if let Some(s) = self
            .tracks
            .get_mut(track)
            .and_then(|t| t.sends.get_mut(send))
        {
            s.level = level;
            s.pan = pan;
            s.on = on;
        }
    }

    /// Switches a lane to its live control (true: being written) or back
    /// to its points. `track` None is the master.
    pub fn set_lane_live(&mut self, track: Option<usize>, kind: LaneKind, live: bool) {
        let Some(ti) = track else {
            if kind == LaneKind::Volume {
                self.master_lane_live = live;
            }
            return;
        };
        let Some(a) = self.tracks.get_mut(ti).and_then(|t| t.auto.as_deref_mut()) else {
            return;
        };
        match kind {
            LaneKind::Volume => a.vol_live = live,
            LaneKind::Pan => a.pan_live = live,
            LaneKind::Effect { slot, param } => {
                for f in &mut a.fx {
                    if f.slot == slot as usize && f.param == param as usize {
                        f.live = live;
                    }
                }
            }
        }
    }

    /// New settings for a master chain slot (on the speakers in 3D).
    pub fn set_master_param(&mut self, slot: usize, vals: &[f32]) {
        match &mut self.space {
            Some(sp) => sp.chain.set(slot, vals),
            None => self.master_chain.set(slot, vals),
        }
    }

    pub fn set_master_bypass(&mut self, slot: usize, on: bool) {
        match &mut self.space {
            Some(sp) => sp.chain.set_bypass(slot, on),
            None => self.master_chain.set_bypass(slot, on),
        }
    }

    /// New rack settings (bypass, input and output gain, mix) for a track's
    /// chain. No allocation.
    pub fn set_rack(&mut self, track: usize, rack: &af27_core::Rack) {
        if let Some(t) = self.tracks.get_mut(track) {
            t.chain.set_rack(rack);
        }
    }

    /// New rack settings for the master chain (on the speakers in 3D).
    pub fn set_master_rack(&mut self, rack: &af27_core::Rack) {
        match &mut self.space {
            Some(sp) => sp.chain.set_rack(rack),
            None => self.master_chain.set_rack(rack),
        }
    }

    /// The metronome click for `n` frames from `t0` into `out` (one value
    /// per frame), or silence when the click is off. Kept apart from
    /// [`Mix::render`] so it never reaches the meters, the scope or an
    /// export: the live engine adds it to what it hears last.
    pub fn click_into(
        &self,
        t0: f64,
        dt: f64,
        out: &mut [f32],
        settings: &af27_core::midi::MetronomeSettings,
    ) {
        if !self.metronome || settings.muted {
            out.fill(0.0);
            return;
        }
        for (i, o) in out.iter_mut().enumerate() {
            *o = synth::click_with(t0 + dt * i as f64, &self.click, settings);
        }
    }

    pub fn set_master_mix(&mut self, mix: f32) {
        match &mut self.space {
            Some(sp) => sp.chain.mix = mix,
            None => self.master_chain.mix = mix,
        }
    }

    /// Sends strip `ti`'s sound out on its output and sends. `tp` is the
    /// timeline time of its sound's first frame this block.
    fn route(&mut self, ti: usize, n: usize, tp: f64, dt: f64) {
        let space = self.space.is_some();
        // Per-frame lane factors.
        let (has_v, has_p) = {
            let t = &self.tracks[ti];
            match t.auto.as_deref() {
                Some(a) => {
                    let (v, p) = (a.plays_vol(), a.plays_pan() && !space);
                    for i in 0..n {
                        let at = tp + dt * i as f64;
                        if v {
                            self.av[i] = lane_gain(&a.vol, at);
                        }
                        if p {
                            let pan = af27_core::automation::value_at(&a.pan, at).unwrap_or(0.0);
                            let [l, r] = balance(pan as f32);
                            self.apl[i] = l;
                            self.apr[i] = r;
                        }
                    }
                    (v, p)
                }
                None => (false, false),
            }
        };
        let v = has_v.then_some(&self.av[..n]);
        let pan = has_p.then_some((&self.apl[..n], &self.apr[..n]));
        // Sends first, each into its bus.
        for k in 0..self.tracks[ti].sends.len() {
            let (bus, target, cur, pre) = {
                let t = &self.tracks[ti];
                let s = &t.sends[k];
                (s.bus, t.send_target(s), s.cur, s.pre)
            };
            if target == [0.0; 2] && cur == [0.0; 2] {
                continue;
            }
            {
                let t = &self.tracks[ti];
                gain_into(
                    &mut self.sl[..n],
                    &mut self.sr[..n],
                    &t.l[..n],
                    &t.r[..n],
                    cur,
                    target,
                    if pre { None } else { v },
                    None,
                );
            }
            let t = &mut self.tracks[ti];
            t.sends[k].cur = target;
            t.sends[k].delay.run(&mut self.sl[..n], &mut self.sr[..n]);
            if bus != ti && space && space::is_room(&self.tracks, bus) {
                // A 3D Bus: placed by the sender's spot.
                if let Some(sp) = self.space.as_ref() {
                    space::place_send(&mut self.tracks, sp, ti, bus, &self.sl, &self.sr, n);
                }
            } else if bus != ti {
                let b = &mut self.tracks[bus];
                add_into(&mut b.l[..n], &mut b.r[..n], &self.sl[..n], &self.sr[..n]);
            }
        }
        // Then the output.
        let (target, cur, out) = {
            let t = &self.tracks[ti];
            (t.main_target(space), t.main_cur, t.output)
        };
        if target == [0.0; 2] && cur == [0.0; 2] {
            return;
        }
        {
            let t = &self.tracks[ti];
            gain_into(
                &mut self.sl[..n],
                &mut self.sr[..n],
                &t.l[..n],
                &t.r[..n],
                cur,
                target,
                v,
                pan,
            );
        }
        {
            let t = &mut self.tracks[ti];
            t.main_cur = target;
            t.out_delay.run(&mut self.sl[..n], &mut self.sr[..n]);
        }
        // 3D: onto the bed by the spot (or a 3D Bus's bed, or its own
        // multichannel bed); a stereo bus takes it as in a stereo session.
        if space && self.route_space(ti, n, target[0]) {
            return;
        }
        match out {
            Some(bus) if bus != ti => {
                let b = &mut self.tracks[bus];
                add_into(&mut b.l[..n], &mut b.r[..n], &self.sl[..n], &self.sr[..n]);
            }
            _ => add_into(
                &mut self.ml[..n],
                &mut self.mr[..n],
                &self.sl[..n],
                &self.sr[..n],
            ),
        }
    }

    /// The master fader's gain per frame from `from` to `to`, times its
    /// volume lane when one plays (at output time `t0`).
    #[inline]
    fn master_gain(&self, i: usize, n: usize, t0: f64, dt: f64) -> f32 {
        let x = (i + 1) as f32 / n.max(1) as f32;
        let g = self.master_cur + (self.master - self.master_cur) * x;
        if !self.master_lane.is_empty() && !self.master_lane_live {
            lane_gain(&self.master_lane, t0 + dt * i as f64)
        } else {
            g
        }
    }

    /// Renders `n` frames (n <= BLOCK) from `t0` into `out`, interleaved
    /// with `out_channels` channels: 2 for listening, or the bed's own
    /// channel count for a multichannel export (`bed_out`).
    pub fn render(
        &mut self,
        t0: f64,
        dt: f64,
        n: usize,
        out: &mut [f32],
        bed_out: bool,
    ) -> (f32, f32) {
        let n = n.min(BLOCK);
        self.space_prime(t0, dt);
        // The sources are read ahead of what comes out by the delay
        // compensation and the Place Elements frame.
        let ts = t0 + dt * (self.lead + self.steer_lead) as f64;
        self.ml[..n].fill(0.0);
        self.mr[..n].fill(0.0);
        if let Some(sp) = &mut self.space {
            sp.bed[..n * sp.channels].fill(0.0);
        }
        self.space_begin(t0 + dt * n as f64 * 0.5, n);
        for t in &mut self.tracks {
            if t.is_bus {
                t.l[..n].fill(0.0);
                t.r[..n].fill(0.0);
            }
        }
        // Every strip's own work (its clips or notes and its effect chain)
        // is independent of the others in its level, so it is spread over
        // the engine's threads. Sends and outputs, which add into shared
        // buses, follow in order on this thread.
        let levels = std::mem::take(&mut self.levels);
        for level in &levels {
            {
                let tracks = TrackPtr(self.tracks.as_mut_ptr());
                let job = |k: usize| {
                    // Each index in a level is a different track, and the
                    // pool runs each `k` exactly once, so these borrows never
                    // overlap; key sources sit at earlier levels.
                    let t = unsafe { &mut *tracks.get(level[k]) };
                    // A 3D Bus runs its effects on its own bed instead.
                    let room = t.sp.as_ref().is_some_and(|s| s.room.is_some());
                    if !t.active() {
                        t.l[..n].fill(0.0);
                        t.r[..n].fill(0.0);
                    } else if !t.is_bus {
                        render_source(t, tracks, ts, dt, n);
                    } else if !room {
                        let tin = ts - dt * t.in_t as f64;
                        run_chain(t, tracks, n, tin, dt);
                    }
                    space::render_track(t, ts, dt, n);
                };
                if self.parallel && level.len() >= 2 {
                    crate::pool::pool().run(level.len(), &job);
                } else {
                    (0..level.len()).for_each(job);
                }
            }
            for &ti in level {
                let tp = ts - dt * self.tracks[ti].out_t as f64;
                self.route(ti, n, tp, dt);
                self.space_end_track(ti);
            }
        }
        self.levels = levels;
        // The LFE band and the master chain on the speakers, before
        // listening or export.
        self.space_master(n);
        if bed_out && self.space.is_some() {
            let peak = self.write_bed(n, out, t0, dt);
            self.master_cur = self.master;
            return peak;
        }
        if let Some(sp) = &mut self.space {
            let mut st = [0.0f32; BLOCK * 2];
            sp.monitor
                .process(&sp.bed[..n * sp.channels], &mut st[..n * 2]);
            for i in 0..n {
                self.ml[i] += st[i * 2];
                self.mr[i] += st[i * 2 + 1];
            }
        }
        {
            let tracks = &self.tracks;
            for tap in self.master_taps.iter_mut().flatten() {
                let s = &tracks[tap.src];
                tap.fill(&s.l, &s.r, n);
            }
            let taps = &self.master_taps;
            let (l, r) = (&mut self.ml[..n], &mut self.mr[..n]);
            self.master_chain.process_keyed(l, r, |slot| {
                let tap = taps.get(slot)?.as_ref()?;
                Some((&tap.l[..n], &tap.r[..n]))
            });
        }
        let (mut pl, mut pr) = (0.0f32, 0.0f32);
        for i in 0..n {
            let g = self.master_gain(i, n, t0, dt);
            let l = self.ml[i] * g;
            let r = self.mr[i] * g;
            pl = pl.max(l.abs());
            pr = pr.max(r.abs());
            out[i * 2] = l;
            out[i * 2 + 1] = r;
        }
        self.master_cur = self.master;
        (pl, pr)
    }
}
