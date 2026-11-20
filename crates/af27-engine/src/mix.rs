// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The mix graph, run in blocks of up to [`BLOCK`] frames.
//!
//! Per block: buses are cleared; every audio and MIDI track renders its
//! clips (or notes), runs its effect chain (inside its rack: input gain,
//! the effects, dry/wet mix, output gain), feeds pre-fader sends, takes
//! its fader and pan, feeds post-fader sends, then goes to its output (a
//! bus or the master). Buses then run in session order. An effect keyed
//! by another track or bus (a sidechain) hears that source's sound after
//! its chain and before its fader, so the source runs first; see `plan`.
//! A 3D Space
//! session spreads each track over the speaker bed instead of panning;
//! its master chain then runs on the bed's speakers (every channel, see
//! `af27_dsp::wide`), before the bed is heard binaurally or folded to
//! stereo, or exported as it is. Bus tracks stay stereo. In a stereo
//! session the master chain runs on the stereo mix. The master fader
//! comes last. The metronome is not part of the mix: the live engine adds
//! it after the meters and the scope (see `Mix::click_into`), so it never
//! reaches an export.
//!
//! Everything a block needs is allocated when the program is built, on
//! the UI thread. The audio thread only reads and writes those buffers.

use std::sync::Arc;

use af27_core::time::db_to_gain;
use af27_core::{AudioBuffer, Session, TrackKind};
use af27_dsp::live::{build_chain, Chain};
use af27_dsp::wide::{build_wide_chain, WideChain};

use crate::synth::{self, Synth};

mod space;
pub use space::{bed_names, plays_to_speakers};

pub const BLOCK: usize = 2048;
/// Speaker channels at most (7.1.4 is 12).
pub const MAX_SPEAKERS: usize = af27_space::MAX_SPEAKERS;

#[derive(Clone)]
pub struct MixClip {
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
}

impl MixClip {
    /// The mix clip for `c` playing `buf`. Stretched clips get their
    /// prepared buffer from the cache (queued if new).
    pub fn new(c: &af27_core::Clip, buf: Arc<AudioBuffer>) -> MixClip {
        let p = &c.props;
        let prepared = crate::stretch::needs(c).then(|| crate::stretch::prepare(&buf, c));
        let g = db_to_gain(c.gain_db);
        MixClip {
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

pub struct MixSend {
    pub bus: usize,
    pub gain: f32,
    pub pre: bool,
}

pub struct MixTrack {
    pub is_bus: bool,
    pub audible: bool,
    /// Fader and pan (pan is ignored in 3D sessions).
    pub gain_l: f32,
    pub gain_r: f32,
    pub clips: Vec<MixClip>,
    pub synth: Option<Synth>,
    /// A SoundFont in place of the built-in synth.
    pub font: Option<Box<crate::soundfont::SfPlayer>>,
    pub chain: Chain,
    /// Index of the bus track it feeds, or None for the master.
    pub output: Option<usize>,
    pub sends: Vec<MixSend>,
    /// Has a pre-fader send (so the fader waits for the sends).
    pub has_pre: bool,
    /// 3D sessions: its place in the room, 3D Bus bed, multichannel clips.
    pub sp: Option<Box<space::TrackSpaceMix>>,
    /// Per chain slot: the track whose sound keys it (a sidechain), once
    /// the effect takes a key and the source can run first; None hears
    /// its own input.
    pub keys: Vec<Option<usize>>,
    l: Vec<f32>,
    r: Vec<f32>,
    /// This track's sound after its chain and before its fader, kept for
    /// the effects it keys (empty when it keys nothing).
    kl: Vec<f32>,
    kr: Vec<f32>,
}

impl MixTrack {
    /// Whether another track's effect listens to this one.
    pub fn is_key(&self) -> bool {
        !self.kl.is_empty()
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
    pub master: f32,
    pub duration: f64,
    pub space: Option<SpaceMix>,
    /// The click is on.
    pub metronome: bool,
    /// The tempo map's stretches, for the click.
    pub click: Vec<af27_core::midi::Segment>,
    ml: Vec<f32>,
    mr: Vec<f32>,
    /// Every track grouped so none in a group feeds or keys another in
    /// it: each group's own work (clips, notes, chains) runs in parallel,
    /// groups in order. Without sidechains the audio and MIDI tracks form
    /// the first group and the buses follow.
    levels: Vec<Vec<usize>>,
    /// Spread track work over the engine's threads (false: all on the caller).
    pub parallel: bool,
    /// 3D: frames placed sound is held back by for Place Elements (sources
    /// read that far ahead), 0 when nothing steers; see `space::prime`.
    lead: usize,
    primed: bool,
    prime_out: Vec<f32>,
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

/// Whether `from` needs `to`'s sound first, directly or through others
/// (`deps[x]`: what `x` needs first; the graph has no loops).
fn needs(deps: &[Vec<usize>], from: usize, to: usize) -> bool {
    let mut seen = vec![false; deps.len()];
    let mut stack = vec![from];
    while let Some(x) = stack.pop() {
        if x == to {
            return true;
        }
        if std::mem::replace(&mut seen[x], true) {
            continue;
        }
        stack.extend(deps[x].iter().copied().filter(|&d| !seen[d]));
    }
    false
}

/// Works out the order tracks run in, on the UI thread.
///
/// A bus runs after everything that feeds it (by output or send); feeds
/// to an earlier bus arrive after it has run, as they always have, and are
/// ignored here. A track whose effect is keyed by another track or bus
/// (`wanted[i][slot]`, already limited to effects that take a key) runs
/// after that source. A key to itself, to a track that is not there, or
/// one that would close a loop (the source needs this track's sound
/// first) is dropped, and that effect hears its own input.
///
/// Returns the keys kept, per track and slot, and the levels: each track
/// sits one above everything it needs, so the tracks within a level are
/// independent of one another.
fn plan(
    tracks: &[MixTrack],
    wanted: &[Vec<Option<usize>>],
) -> (Vec<Vec<Option<usize>>>, Vec<Vec<usize>>) {
    let n = tracks.len();
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (a, t) in tracks.iter().enumerate() {
        for b in t.output.into_iter().chain(t.sends.iter().map(|s| s.bus)) {
            let counts = b != a && b < n && tracks[b].is_bus && (!t.is_bus || a < b);
            if counts && !deps[b].contains(&a) {
                deps[b].push(a);
            }
        }
    }
    let mut keys = Vec::with_capacity(n);
    for (i, slots) in wanted.iter().enumerate() {
        let mut kept = Vec::with_capacity(slots.len());
        for k in slots {
            let src = k.filter(|&s| s != i && s < n && !needs(&deps, s, i));
            if let Some(s) = src {
                if !deps[i].contains(&s) {
                    deps[i].push(s);
                }
            }
            kept.push(src);
        }
        keys.push(kept);
    }
    // Longest path from the tracks that need nothing (Kahn's order).
    let mut users: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut waiting: Vec<usize> = deps.iter().map(Vec::len).collect();
    for (i, d) in deps.iter().enumerate() {
        for &x in d {
            users[x].push(i);
        }
    }
    let mut level = vec![0usize; n];
    let mut ready: Vec<usize> = (0..n).filter(|&i| waiting[i] == 0).collect();
    while let Some(x) = ready.pop() {
        for &u in &users[x] {
            level[u] = level[u].max(level[x] + 1);
            waiting[u] -= 1;
            if waiting[u] == 0 {
                ready.push(u);
            }
        }
    }
    let top = level.iter().copied().max().unwrap_or(0);
    let mut out = vec![Vec::new(); top + 1];
    for (i, l) in level.iter().enumerate() {
        out[*l].push(i);
    }
    out.retain(|l| !l.is_empty());
    (keys, out)
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

/// Runs a track's chain over its block, each keyed slot hearing its
/// source's kept sound, then keeps this track's own sound if it keys
/// others. Reads only the key sources, which have finished this block
/// (they sit at an earlier level) and are not written meanwhile.
fn run_chain(t: &mut MixTrack, tracks: TrackPtr, n: usize) {
    {
        let MixTrack {
            chain, keys, l, r, ..
        } = &mut *t;
        chain.process_keyed(&mut l[..n], &mut r[..n], |slot| {
            let src = keys.get(slot).copied().flatten()?;
            // A different track from `t` (keys to itself are dropped),
            // only read here.
            let s = unsafe { &*tracks.get(src) };
            // A source that is not heard has no sound to key with: the
            // effect hears its own input rather than silence.
            (s.audible && s.is_key()).then(|| (&s.kl[..n], &s.kr[..n]))
        });
    }
    if t.is_key() {
        t.kl[..n].copy_from_slice(&t.l[..n]);
        t.kr[..n].copy_from_slice(&t.r[..n]);
    }
}

/// A track's own sound for one block: silence, then its clips or notes,
/// then its effect chain. Writes nothing but the track itself.
fn render_source(t: &mut MixTrack, tracks: TrackPtr, t0: f64, dt: f64, n: usize) {
    t.l[..n].fill(0.0);
    t.r[..n].fill(0.0);
    if !t.audible {
        return;
    }
    let t1 = t0 + dt * n as f64;
    for c in &t.clips {
        if c.start >= t1 || c.start + c.length <= t0 {
            continue;
        }
        read_clip(c, t0, dt, &mut t.l[..n], &mut t.r[..n]);
    }
    if let Some(s) = &mut t.synth {
        s.render(t0, dt, &mut t.l[..n], &mut t.r[..n]);
    }
    if let Some(f) = &mut t.font {
        f.render(t0, dt, &mut t.l[..n], &mut t.r[..n]);
    }
    run_chain(t, tracks, n);
    // With no pre-fader send, the fader can go on here, in parallel too.
    if !t.has_pre {
        apply_fader(t, n);
    }
}

#[inline]
fn apply_fader(t: &mut MixTrack, n: usize) {
    let (gl, gr) = (t.gain_l, t.gain_r);
    for (l, r) in t.l[..n].iter_mut().zip(&mut t.r[..n]) {
        *l *= gl;
        *r *= gr;
    }
}

/// Track `a` to read and track `b` to add into (`a != b`).
#[inline]
fn pair(tracks: &mut [MixTrack], a: usize, b: usize) -> (&MixTrack, &mut MixTrack) {
    if a < b {
        let (x, y) = tracks.split_at_mut(b);
        (&x[a], &mut y[0])
    } else {
        let (x, y) = tracks.split_at_mut(a);
        (&y[0], &mut x[b])
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
    g as f32
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
    let flat = !in_fade(local_first) && !in_fade(local_last);
    let whole = (step - 1.0).abs() < 1e-9 && (x0 - x0.round()).abs() < 1e-6 && x0 >= -0.5;
    match ch.len() {
        2 if whole => {
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
                let (a, b) = stereo_frame(buf, x0 + step * k);
                l[i] += a * g;
                r[i] += b * g;
            }
        }
    }
}

/// Fader and pan gains for a track (fader only in 3D sessions).
pub fn track_gains(t: &af27_core::Track, space: bool) -> (f32, f32) {
    if space {
        let g = db_to_gain(t.volume_db);
        (g, g)
    } else {
        t.gains()
    }
}

/// Builds the mix for `session` at `rate`. `media` gives each media item's
/// current audio (with any unsaved Waveform edits).
pub fn build(
    session: &Session,
    media: &dyn Fn(af27_core::MediaId) -> Option<Arc<AudioBuffer>>,
    rate: u32,
) -> Mix {
    let space = session.space;
    let lead = if space::session_steers(session) {
        af27_space::steer::STEER_FRAME
    } else {
        0
    };
    let index_of = |id: af27_core::TrackId| session.tracks.iter().position(|t| t.id == id);
    let tracks: Vec<MixTrack> = session
        .tracks
        .iter()
        .enumerate()
        .map(|(ti, t)| {
            let mut clips: Vec<MixClip> = Vec::new();
            // 3D: clips of more than two channels play from their speakers.
            let mut wide: Vec<MixClip> = Vec::new();
            for c in t.clips.iter().filter(|c| !c.muted) {
                let Some(buf) = media(c.media) else { continue };
                let mc = MixClip::new(c, buf);
                if space.is_some() && plays_to_speakers(c, mc.buf.num_channels()) {
                    wide.push(mc);
                } else {
                    clips.push(mc);
                }
            }
            // Notes in seconds, through the tempo map, cut at their clip's end.
            let notes = || {
                t.midi.iter().filter(|c| !c.muted).flat_map(|c| {
                    c.notes
                        .iter()
                        .filter(move |n| n.start < c.length)
                        .map(move |n| {
                            let s = session.tempo.secs(c.start + n.start);
                            let e = session
                                .tempo
                                .secs(c.start + (n.start + n.length).min(c.length));
                            (s, e, n)
                        })
                })
            };
            let midi = t.kind == TrackKind::Midi;
            let font = t.soundfont.as_ref().filter(|_| midi).and_then(|sf| {
                let list = notes()
                    .map(|(s, e, n)| crate::soundfont::SfNote {
                        start: s,
                        end: e,
                        key: n.pitch,
                        velocity: n.velocity.clamp(1, 127),
                        pan: n.pan,
                    })
                    .collect();
                // A font that is missing or cannot play leaves the track silent.
                crate::soundfont::SfPlayer::new(sf, list, rate)
                    .ok()
                    .map(Box::new)
            });
            let synth = (midi && t.soundfont.is_none()).then(|| {
                let list = notes()
                    .map(|(s, e, n)| synth::note(s, e, n.pitch, n.velocity, n.pan))
                    .collect();
                Synth::new(t.instrument, list, rate)
            });
            let stereo =
                t.kind != TrackKind::Audio || clips.iter().any(|c| c.buf.num_channels() >= 2);
            let sp = space.map(|layout| space::for_track(layout, t, wide, stereo, rate, ti, lead));
            let (gain_l, gain_r) = track_gains(t, space.is_some());
            MixTrack {
                is_bus: t.kind == TrackKind::Bus,
                audible: session.audible(t),
                gain_l,
                gain_r,
                clips,
                synth,
                font,
                chain: {
                    let mut c = build_chain(&t.chain, rate, BLOCK);
                    c.set_rack(&t.rack);
                    c
                },
                output: t
                    .output
                    .and_then(index_of)
                    .filter(|&i| session.tracks[i].kind == TrackKind::Bus),
                sends: t
                    .sends
                    .iter()
                    .filter_map(|s| {
                        let bus = index_of(s.bus)
                            .filter(|&i| session.tracks[i].kind == TrackKind::Bus)?;
                        Some(MixSend {
                            bus,
                            gain: db_to_gain(s.level_db),
                            pre: s.pre_fader,
                        })
                    })
                    .collect(),
                has_pre: t.sends.iter().any(|s| s.pre_fader),
                sp,
                keys: Vec::new(),
                l: vec![0.0; BLOCK],
                r: vec![0.0; BLOCK],
                kl: Vec::new(),
                kr: Vec::new(),
            }
        })
        .collect();
    let mut tracks = tracks;
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
    let (keys, levels) = plan(&tracks, &wanted);
    for (t, k) in tracks.iter_mut().zip(keys) {
        t.keys = k;
    }
    for i in 0..tracks.len() {
        let used = tracks.iter().any(|t| t.keys.contains(&Some(i)));
        if used {
            tracks[i].kl = vec![0.0; BLOCK];
            tracks[i].kr = vec![0.0; BLOCK];
        }
    }
    let space = space.map(|layout| {
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
    let master_slots: &[af27_core::EffectSlot] = if space.is_some() {
        &[]
    } else {
        &session.master_chain
    };
    // Start the engine's threads here, on the UI thread, never on the
    // audio thread.
    let _ = crate::pool::pool();
    let mut master_chain = build_chain(master_slots, rate, BLOCK);
    if space.is_none() {
        master_chain.set_rack(&session.master_rack);
    }
    Mix {
        levels,
        parallel: true,
        tracks,
        master_chain,
        master: db_to_gain(session.master_db),
        duration: session.duration(),
        space,
        metronome: session.tempo.metronome,
        click: session.tempo.segments().collect(),
        ml: vec![0.0; BLOCK],
        mr: vec![0.0; BLOCK],
        lead,
        primed: false,
        prime_out: if lead > 0 {
            vec![0.0; BLOCK * MAX_SPEAKERS]
        } else {
            Vec::new()
        },
    }
}

impl Mix {
    /// The order tracks run in: groups of track indices, each group's
    /// tracks independent of one another (see `plan`).
    pub fn levels(&self) -> &[Vec<usize>] {
        &self.levels
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

    /// Clears effect tails, after a jump.
    pub fn reset(&mut self) {
        for t in &mut self.tracks {
            t.chain.reset();
            if let Some(s) = &mut t.synth {
                s.reset();
            }
            if let Some(f) = &mut t.font {
                f.reset();
            }
        }
        self.master_chain.reset();
        if let Some(sp) = &mut self.space {
            sp.chain.reset();
        }
        self.space_reset();
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

    fn route(&mut self, ti: usize, n: usize) {
        if self.route_space(ti, n) {
            return;
        }
        let out = self.tracks[ti].output;
        match out {
            Some(bus) if bus != ti && (bus > ti || !self.tracks[ti].is_bus) => {
                let (t, b) = pair(&mut self.tracks, ti, bus);
                for ((bl, br), (l, r)) in b.l[..n]
                    .iter_mut()
                    .zip(&mut b.r[..n])
                    .zip(t.l[..n].iter().zip(&t.r[..n]))
                {
                    *bl += l;
                    *br += r;
                }
            }
            // (In a 3D session `route_space` placed it on the bed.)
            _ => {
                let t = &self.tracks[ti];
                for ((ml, mr), (l, r)) in self.ml[..n]
                    .iter_mut()
                    .zip(&mut self.mr[..n])
                    .zip(t.l[..n].iter().zip(&t.r[..n]))
                {
                    *ml += l;
                    *mr += r;
                }
            }
        }
    }

    fn sends(&mut self, ti: usize, n: usize, pre: bool) {
        for s in 0..self.tracks[ti].sends.len() {
            let (bus, g, is_pre) = {
                let x = &self.tracks[ti].sends[s];
                (x.bus, x.gain, x.pre)
            };
            if is_pre != pre || bus == ti || self.send_space(ti, bus, g, n) {
                continue;
            }
            let (t, b) = pair(&mut self.tracks, ti, bus);
            for ((bl, br), (l, r)) in b.l[..n]
                .iter_mut()
                .zip(&mut b.r[..n])
                .zip(t.l[..n].iter().zip(&t.r[..n]))
            {
                *bl += l * g;
                *br += r * g;
            }
        }
    }

    /// After the chain: pre-fader sends, fader (unless `faded`: already
    /// applied in the parallel stage), post-fader sends, output.
    fn mix_out(&mut self, ti: usize, n: usize, faded: bool) {
        if !faded {
            self.sends(ti, n, true);
            let t = &mut self.tracks[ti];
            apply_fader(t, n);
        }
        self.sends(ti, n, false);
        self.route(ti, n);
        self.space_end_track(ti);
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
        self.prime(t0, dt);
        // Sources are read ahead by the Place Elements delay (0 without it).
        let ts = t0 + self.lead as f64 * dt;
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
        // Every track's own work (its clips or notes and its effect chain)
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
                    let room = t.sp.as_ref().is_some_and(|s| s.room.is_some());
                    if !t.is_bus {
                        render_source(t, tracks, ts, dt, n);
                    } else if t.audible && !room {
                        run_chain(t, tracks, n);
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
                let t = &self.tracks[ti];
                if t.audible {
                    // A track with no pre-fader send took its fader above.
                    let faded = !t.is_bus && !t.has_pre;
                    self.mix_out(ti, n, faded);
                }
            }
        }
        self.levels = levels;
        // The LFE band and the master chain on the speakers, before
        // listening or export.
        self.space_master(n);
        if bed_out && self.space.is_some() {
            return self.write_bed(n, out);
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
            let (l, r) = (&mut self.ml[..n], &mut self.mr[..n]);
            self.master_chain.process(l, r);
        }
        let (mut pl, mut pr) = (0.0f32, 0.0f32);
        for i in 0..n {
            let l = self.ml[i] * self.master;
            let r = self.mr[i] * self.master;
            pl = pl.max(l.abs());
            pr = pr.max(r.abs());
            out[i * 2] = l;
            out[i * 2 + 1] = r;
        }
        (pl, pr)
    }
}
