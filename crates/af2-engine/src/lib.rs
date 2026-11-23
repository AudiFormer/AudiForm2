// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Real-time playback and recording.
//!
//! The UI builds a [`Program`] (a file to play, or a multitrack mix with
//! its effect chains, buses, synths and 3D bed) and hands it to the audio
//! thread over a lock-free queue. The audio thread never allocates, locks
//! or frees: programs it is done with go back over a second queue and are
//! dropped by [`Engine::collect`] on the UI thread. Fader, pan, effect
//! settings, bypass, 3D positions and instrument settings change live
//! through small fixed-size commands, so a tweak never restarts a tail.
//! Position, meters and the Phase Wheel's ring come back through atomics.

mod asio;
pub mod capture;
pub mod denormal;
pub mod devices;
pub mod health;
mod meter;
pub mod mix;
pub mod monitor;
pub mod plugin_inst;
pub mod pool;
pub mod disk;
mod record;
pub use record::LIVE_BIN as RECORD_LIVE_BIN;
pub mod routing;
pub mod scope;
pub mod soundfont;
mod stream;
pub mod stretch;
pub mod synth;
mod synth_voice;
pub mod transport;

#[cfg(test)]
mod tests_clipfx;
#[cfg(test)]
mod tests_keys;
#[cfg(test)]
mod tests_mix;
#[cfg(test)]
mod tests_monitor;
#[cfg(test)]
mod tests_space;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use af2_core::AudioBuffer;
use cpal::traits::{DeviceTrait, HostTrait};

pub use capture::{AudioApp, CaptureSupport, OutputDevice, RecordSource};
pub use devices::{DeviceInfo, DeviceList, HostKind, ASIO_BUILT, ASIO_CREDIT};
pub use mix::{stereo_frame, track_gains, Mix, BLOCK, MAX_SPEAKERS};
pub use routing::{InputPick, OutputMap};
pub use transport::{Plan, Region};

/// What the audio thread plays.
pub enum Program {
    /// One buffer (the Waveform editor). `channel_mask` bit 0 is left,
    /// bit 1 right, for listening to one side.
    Buffer {
        buf: Arc<AudioBuffer>,
        channel_mask: u8,
        /// A surround file heard on headphones: every channel placed
        /// around the head (None: folded to plain stereo).
        head: Option<Box<HeadRender>>,
    },
    Mix(Box<Mix>),
}

/// How a file of more than two channels is heard in the Waveform editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurroundPlay {
    /// Folded to plain stereo: no spatial processing.
    Fold,
    /// Every channel placed around the head (binaural).
    Headphones,
    /// The binaural picture for a pair of near-field speakers: crosstalk
    /// between the speakers and the far ear cancelled.
    Speakers,
    /// Each channel to its own device output (Settings > Channel
    /// Routing's speaker outputs).
    Direct,
}

/// A file of more than two channels for headphones, speakers or the
/// speaker outputs: each channel goes to its speaker of the nearest
/// surround layout; then the speakers are heard binaurally (af2-space's
/// head model, as 3D Space uses), through crosstalk cancellation for two
/// speakers, or straight out.
pub struct HeadRender {
    bin: af2_space::Binaural,
    /// Per file channel: gain on each speaker of the layout.
    map: Vec<af2_space::Gains>,
    speakers: usize,
    bed: Vec<f32>,
    mode: SurroundPlay,
    layout: af2_space::SpaceLayout,
    xtc: Xtc,
    /// Headphones and Speakers: brings the binaural picture to the plain
    /// fold's loudness, so switching modes doesn't jump in level.
    gain: f32,
}

/// Crosstalk cancellation for two speakers at about 30 degrees either
/// side: what one speaker sends to the far ear (a little later and duller)
/// is taken back out of the other speaker, round and round. Only the band
/// where it matters (about 250 Hz to 5 kHz) is cancelled, so the bass is
/// left alone.
struct Xtc {
    delay: usize,
    gain: f32,
    ring: [Vec<f32>; 2],
    at: usize,
    hp: [(f32, f32); 2],
    lp: [f32; 2],
    a_hp: f32,
    a_lp: f32,
}

impl Xtc {
    fn new(rate: u32) -> Xtc {
        let r = rate.max(1) as f32;
        // The far ear is about 0.26 ms later for speakers at 30 degrees.
        let delay = ((0.26e-3 * r).round() as usize).max(1);
        Xtc {
            delay,
            gain: 0.6,
            ring: [vec![0.0; delay + 1], vec![0.0; delay + 1]],
            at: 0,
            hp: [(0.0, 0.0); 2],
            lp: [0.0; 2],
            a_hp: (-std::f32::consts::TAU * 250.0 / r).exp(),
            a_lp: (-std::f32::consts::TAU * 5_000.0 / r).exp(),
        }
    }

    fn process(&mut self, out: &mut [f32]) {
        let len = self.delay + 1;
        for f in out.chunks_exact_mut(2) {
            let old = (self.at + 1) % len;
            let mut y = [0.0f32; 2];
            for side in 0..2 {
                // The other side's output, as the far ear hears it.
                let d = self.ring[1 - side][old];
                let (x1, y1) = self.hp[side];
                let h = self.a_hp * (y1 + d - x1);
                self.hp[side] = (d, h);
                self.lp[side] = (1.0 - self.a_lp) * h + self.a_lp * self.lp[side];
                y[side] = f[side] - self.gain * self.lp[side];
            }
            for side in 0..2 {
                self.ring[side][self.at] = y[side];
                f[side] = y[side];
            }
            self.at = old;
        }
    }
}

impl HeadRender {
    /// For a file of `n` channels (named from its WAV `mask` when it has
    /// one) played out at `rate` in `mode`. None for mono and stereo files
    /// and for the plain fold.
    pub fn new(n: usize, mask: Option<u32>, rate: u32, mode: SurroundPlay) -> Option<HeadRender> {
        use af2_space::channels::{names, targets, Target};
        if n <= 2 || mode == SurroundPlay::Fold {
            return None;
        }
        let layout = file_layout(n, mask);
        let names = names(n, mask);
        let map: Vec<af2_space::Gains> = targets(layout, &names).iter().map(Target::gains).collect();
        let bin = af2_space::Binaural::new(layout, rate);
        let speakers = bin.channels();
        let mut h = HeadRender {
            bin,
            map,
            speakers,
            bed: vec![0.0; BLOCK * speakers],
            mode,
            layout,
            xtc: Xtc::new(rate),
            gain: 1.0,
        };
        if mode != SurroundPlay::Direct {
            h.gain = h.match_fold(n, rate);
        }
        Some(h)
    }

    /// The gain that puts this mode at the plain fold's loudness: the same
    /// noise on every channel, measured both ways (then the head model and
    /// crosstalk canceller start afresh).
    fn match_fold(&mut self, n: usize, rate: u32) -> f32 {
        const LEN: usize = 8_192;
        const SKIP: usize = 1_024;
        let mut probe = AudioBuffer::new(rate.max(1), n, LEN);
        let mut x: u32 = 0x9E37_79B9;
        for c in probe.channels.iter_mut() {
            for v in c.iter_mut() {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                *v = (x as f32 / u32::MAX as f32 - 0.5) * 0.5;
            }
        }
        let fold: f64 = (SKIP..LEN)
            .map(|i| {
                let (l, r) = stereo_frame(&probe, i as f64);
                (l * l + r * r) as f64
            })
            .sum();
        let dt = 1.0 / rate.max(1) as f64;
        let mut out = vec![0.0f32; BLOCK * 2];
        let mut heard = 0.0f64;
        let mut at = 0usize;
        while at < LEN {
            let k = BLOCK.min(LEN - at);
            self.render(&probe, at as f64 * dt, dt, k, &mut out);
            for (j, f) in out[..k * 2].chunks_exact(2).enumerate() {
                if at + j >= SKIP {
                    heard += (f[0] * f[0] + f[1] * f[1]) as f64;
                }
            }
            at += k;
        }
        self.bin = af2_space::Binaural::new(self.layout, rate);
        self.xtc = Xtc::new(rate);
        if heard <= 0.0 || fold <= 0.0 {
            return 1.0;
        }
        ((fold / heard).sqrt() as f32).clamp(0.05, 2.0)
    }

    /// Straight to the speaker outputs.
    pub fn is_direct(&self) -> bool {
        self.mode == SurroundPlay::Direct
    }

    /// The bed's speakers (its width when it goes straight out).
    pub fn speakers(&self) -> usize {
        self.speakers
    }

    pub fn layout(&self) -> af2_space::SpaceLayout {
        self.layout
    }

    /// The file's channels on the layout's speakers: `n` frames into
    /// `self.bed`.
    fn fill_bed(&mut self, buf: &AudioBuffer, t0: f64, dt: f64, n: usize) {
        let s = self.speakers;
        let bed = &mut self.bed[..n * s];
        bed.fill(0.0);
        for i in 0..n {
            let x = (t0 + dt * i as f64) * buf.rate as f64;
            let frame = &mut bed[i * s..(i + 1) * s];
            for (ch, g) in buf.channels.iter().zip(&self.map) {
                let v = mix::sample(ch, x);
                if v != 0.0 {
                    for (o, gain) in frame.iter_mut().zip(g.iter()) {
                        *o += v * gain;
                    }
                }
            }
        }
    }

    /// Renders `n` frames (at most [`BLOCK`]) of `buf` from `t0` seconds,
    /// `dt` apart, into interleaved stereo `out` (binaural, and for
    /// speakers crosstalk-cancelled).
    fn render(&mut self, buf: &AudioBuffer, t0: f64, dt: f64, n: usize, out: &mut [f32]) {
        self.fill_bed(buf, t0, dt, n);
        let s = self.speakers;
        self.bin.process(&self.bed[..n * s], &mut out[..n * 2]);
        if self.mode == SurroundPlay::Speakers {
            self.xtc.process(&mut out[..n * 2]);
        }
        if self.gain != 1.0 {
            for v in &mut out[..n * 2] {
                *v *= self.gain;
            }
        }
    }

    /// Renders `n` frames as the speaker bed itself, `speakers()` wide,
    /// for the device's speaker outputs.
    fn render_bed(&mut self, buf: &AudioBuffer, t0: f64, dt: f64, n: usize, out: &mut [f32]) {
        self.fill_bed(buf, t0, dt, n);
        let s = self.speakers;
        out[..n * s].copy_from_slice(&self.bed[..n * s]);
    }
}

/// The surround layout a file of `n` channels (named from its WAV `mask`)
/// plays on: the smallest with a speaker for every channel; else the
/// biggest, where any others are placed at their usual spots.
pub fn file_layout(n: usize, mask: Option<u32>) -> af2_space::SpaceLayout {
    use af2_space::channels::{names, targets, Target};
    let names = names(n, mask);
    af2_space::SpaceLayout::ALL
        .into_iter()
        .filter(|l| *l != af2_space::SpaceLayout::Stereo20)
        .find(|l| targets(*l, &names).iter().all(|t| matches!(t, Target::Speaker(_))))
        .unwrap_or(af2_space::SpaceLayout::Surround714)
}

/// The master chain in [`Cmd::Param`] and friends.
pub const MASTER: u16 = u16::MAX;
/// Most values one effect takes.
pub const MAX_VALUES: usize = 24;

enum Cmd {
    Program(Box<Program>),
    /// Start afresh (effect tails cleared).
    Play {
        from: f64,
        plan: Plan,
    },
    /// New bounds while playing, and a new position if `to` is Some.
    /// Nothing is reset, so effect tails carry on across the move.
    Retarget {
        to: Option<f64>,
        plan: Plan,
    },
    Stop,
    Strip {
        track: u16,
        fader: f32,
        pan: f32,
    },
    Send {
        track: u16,
        send: u16,
        level: f32,
        pan: f32,
        on: bool,
    },
    LaneLive {
        track: u16,
        kind: mix::LaneKind,
        live: bool,
    },
    Audible {
        track: u16,
        on: bool,
    },
    Master(f32),
    /// The master's balance, mono sum and polarity flip.
    MasterOpts {
        pan: f32,
        mono: bool,
        invert: bool,
    },
    Param {
        track: u16,
        slot: u8,
        n: u8,
        vals: [f32; MAX_VALUES],
    },
    Bypass {
        track: u16,
        slot: u8,
        on: bool,
    },
    ChainMix {
        track: u16,
        mix: f32,
    },
    /// A track's new spot in the room.
    Space {
        track: u16,
        pos: af2_core::space::SpacePos,
    },
    Instrument {
        track: u16,
        inst: af2_core::midi::Instrument,
    },
    Metronome(bool),
    /// The click's sound, level, accent and subdivisions.
    MetronomeSettings(af2_core::midi::MetronomeSettings),
    /// Recording started or stopped (for a click only while recording).
    Recording(bool),
    /// A chain's rack: power, input and output gain, mix.
    Rack {
        track: u16,
        rack: af2_core::Rack,
    },
    /// New settings for a slot of a clip's own rack.
    ClipParam {
        clip: af2_core::ClipId,
        slot: u8,
        n: u8,
        vals: [f32; MAX_VALUES],
    },
    ClipBypass {
        clip: af2_core::ClipId,
        slot: u8,
        on: bool,
    },
    ClipRack {
        clip: af2_core::ClipId,
        rack: af2_core::Rack,
    },
    /// SoundFont level of a track.
    FontGain {
        track: u16,
        db: f32,
    },
    /// What hand-played notes go through.
    Audition(Box<soundfont::Audition>),
    NoteOn {
        pitch: u8,
        velocity: u8,
        pan: f32,
    },
    NoteOff(u8),
    AllNotesOff,
    /// New built-in synth settings for the audition (held notes carry on).
    AuditionInstrument(af2_core::midi::Instrument),
    /// Where each channel goes on the device (handled by the callback).
    Routes(routing::Routes),
    /// A MIDI keyboard plays a track's plug-in instrument through this
    /// (None: no longer). Handled by the callback.
    Keys(Option<Box<plugin_inst::KeysPlayer>>),
    KeysGain([f32; 2]),
    KeysEvent(af2_dsp::instrument::NoteEvent),
    KeysAllOff,
    /// Inputs heard live through their tracks (None: none). Handled by
    /// the callback.
    Monitor(Option<Box<monitor::Feed>>),
}

/// What the audio thread hands back to be freed on the UI thread.
#[allow(dead_code)]
enum Garbage {
    Program(Box<Program>),
    Audition(Box<soundfont::Audition>),
    Keys(Box<plugin_inst::KeysPlayer>),
    Monitor(Box<monitor::Feed>),
}

struct Shared {
    playing: AtomicBool,
    pos: AtomicU64,
    peak_l: AtomicU32,
    peak_r: AtomicU32,
    /// The output's mean square since the interface last read it.
    mean_square: meter::MeanSquare,
    /// 3D: each speaker's peak, and how many speakers there are.
    bed_peaks: [AtomicU32; MAX_SPEAKERS],
    bed_count: AtomicU32,
    /// A surround file playing in the Waveform editor: each of its
    /// channels' peak before any fold or binaural, and how many.
    src_peaks: [AtomicU32; MAX_SPEAKERS],
    src_count: AtomicU32,
}

pub(crate) use meter::store_max_f32;

struct Voice {
    program: Option<Box<Program>>,
    audition: Option<Box<soundfont::Audition>>,
    playing: bool,
    t: f64,
    plan: Plan,
    /// The click's settings (the app's, kept across programs).
    metro: af2_core::midi::MetronomeSettings,
    recording: bool,
    /// Live playback only: the click for the frames last rendered, one
    /// value per frame, added after the meters and the scope. Offline
    /// renders have none, so the click never reaches an export.
    clicks: Option<Vec<f32>>,
    /// Jumps while playing fade out, move, and fade back in.
    declick: Declick,
}

/// How long a jump's fade out (and its fade back in) lasts, in seconds:
/// short enough not to be heard as a gap, long enough not to click.
pub const DECLICK_SECS: f64 = 0.004;

/// A jump waiting for the sound to fade out, and the fade's gain.
#[derive(Debug, Clone, Copy)]
struct Declick {
    /// Gain applied to the output (0..1).
    gain: f64,
    /// Where to go (and the plan to go with) once the gain reaches 0.
    pending: Option<(f64, Plan)>,
}

impl Default for Declick {
    fn default() -> Self {
        Declick {
            gain: 1.0,
            pending: None,
        }
    }
}

/// Frames of click the live voice holds (longer callbacks go without
/// the click past this).
const CLICK_FRAMES: usize = 1 << 16;

impl Voice {
    fn apply(&mut self, cmd: Cmd, trash: &mut rtrb::Producer<Garbage>) {
        let mix = match self.program.as_deref_mut() {
            Some(Program::Mix(m)) => Some(m.as_mut()),
            _ => None,
        };
        match cmd {
            Cmd::Program(mut p) => {
                // Arriving while playing: prime it so it carries on in step.
                // Its effects carry on from the playing mix where they are
                // the same (no restart, no dip); otherwise it is primed.
                if let (true, Program::Mix(m)) = (self.playing, p.as_mut()) {
                    let kept = match self.program.as_deref_mut() {
                        Some(Program::Mix(old)) => m.adopt(old),
                        _ => false,
                    };
                    if !kept {
                        m.prime(self.t);
                    }
                }
                if let Some(old) = self.program.replace(p) {
                    if let Err(rtrb::PushError::Full(old)) = trash.push(Garbage::Program(old)) {
                        // Never free on the audio thread.
                        std::mem::forget(old);
                    }
                }
            }
            Cmd::Play { from, plan } => {
                self.declick = Declick::default();
                self.t = plan.start_at(from);
                self.plan = plan;
                self.playing = true;
                if let Some(Program::Mix(m)) = self.program.as_deref_mut() {
                    m.start(self.t);
                }
            }
            Cmd::Retarget { to, plan } => match to {
                // Playing: fade out first, then jump (see `render`).
                Some(t) if self.playing && self.program.is_some() => {
                    self.declick.pending = Some((t, plan));
                }
                Some(t) => {
                    self.t = plan.start_at(t);
                    self.plan = plan;
                }
                None => {
                    // A plan change on top of a jump still waiting keeps
                    // the jump's place.
                    match &mut self.declick.pending {
                        Some((_, p)) => *p = plan,
                        None => self.plan = plan,
                    }
                }
            },
            Cmd::Stop => self.playing = false,
            Cmd::Strip { track, fader, pan } => {
                if let Some(m) = mix {
                    m.set_strip(track as usize, fader, pan);
                }
            }
            Cmd::Send {
                track,
                send,
                level,
                pan,
                on,
            } => {
                if let Some(m) = mix {
                    m.set_send(track as usize, send as usize, level, pan, on);
                }
            }
            Cmd::LaneLive { track, kind, live } => {
                if let Some(m) = mix {
                    let t = (track != MASTER).then_some(track as usize);
                    m.set_lane_live(t, kind, live);
                }
            }
            Cmd::Audible { track, on } => {
                if let Some(t) = mix.and_then(|m| m.tracks.get_mut(track as usize)) {
                    t.audible = on;
                }
            }
            Cmd::Master(g) => {
                if let Some(m) = mix {
                    m.master = g;
                }
            }
            Cmd::MasterOpts { pan, mono, invert } => {
                if let Some(m) = mix {
                    m.master_pan = pan.clamp(-1.0, 1.0);
                    m.master_mono = mono;
                    m.master_invert = invert;
                }
            }
            Cmd::Param {
                track,
                slot,
                n,
                vals,
            } => {
                if let Some(m) = mix {
                    if track == MASTER {
                        m.set_master_param(slot as usize, &vals[..n as usize]);
                    } else if let Some(t) = m.tracks.get_mut(track as usize) {
                        t.chain.set(slot as usize, &vals[..n as usize]);
                        m.space_param(track as usize, slot as usize, &vals[..n as usize]);
                    }
                }
            }
            Cmd::Bypass { track, slot, on } => {
                if let Some(m) = mix {
                    if track == MASTER {
                        m.set_master_bypass(slot as usize, on);
                    } else if let Some(t) = m.tracks.get_mut(track as usize) {
                        t.chain.set_bypass(slot as usize, on);
                        m.space_bypass(track as usize, slot as usize, on);
                    }
                }
            }
            Cmd::ChainMix { track, mix: amount } => {
                if let Some(m) = mix {
                    if track == MASTER {
                        m.set_master_mix(amount);
                    } else if let Some(t) = m.tracks.get_mut(track as usize) {
                        t.chain.mix = amount;
                        m.space_chain_mix(track as usize, amount);
                    }
                }
            }
            Cmd::Space { track, pos } => {
                if let Some(m) = mix {
                    m.set_space(track as usize, pos);
                }
            }
            Cmd::Instrument { track, inst } => {
                if let Some(s) = mix
                    .and_then(|m| m.tracks.get_mut(track as usize))
                    .and_then(|t| t.synth.as_mut())
                {
                    s.set(inst);
                }
            }
            Cmd::Metronome(on) => {
                if let Some(m) = mix {
                    m.metronome = on;
                }
            }
            Cmd::MetronomeSettings(s) => self.metro = s,
            Cmd::Recording(on) => self.recording = on,
            Cmd::Rack { track, rack } => {
                if let Some(m) = mix {
                    if track == MASTER {
                        m.set_master_rack(&rack);
                    } else {
                        m.set_rack(track as usize, &rack);
                        m.space_rack(track as usize, &rack);
                    }
                }
            }
            Cmd::ClipParam {
                clip,
                slot,
                n,
                vals,
            } => {
                if let Some(fx) = mix.and_then(|m| m.clip_fx(clip)) {
                    fx.set(slot as usize, &vals[..n as usize]);
                }
            }
            Cmd::ClipBypass { clip, slot, on } => {
                if let Some(fx) = mix.and_then(|m| m.clip_fx(clip)) {
                    fx.chain.set_bypass(slot as usize, on);
                }
            }
            Cmd::ClipRack { clip, rack } => {
                if let Some(fx) = mix.and_then(|m| m.clip_fx(clip)) {
                    fx.chain.set_rack(&rack);
                }
            }
            Cmd::FontGain { track, db } => {
                if let Some(f) = mix
                    .and_then(|m| m.tracks.get_mut(track as usize))
                    .and_then(|t| t.font.as_mut())
                {
                    f.set_gain(db);
                }
            }
            Cmd::Audition(a) => {
                if let Some(old) = self.audition.replace(a) {
                    if let Err(rtrb::PushError::Full(old)) = trash.push(Garbage::Audition(old)) {
                        std::mem::forget(old);
                    }
                }
            }
            Cmd::NoteOn {
                pitch,
                velocity,
                pan,
            } => {
                if let Some(a) = &mut self.audition {
                    a.note_on(pitch, velocity, pan);
                }
            }
            Cmd::NoteOff(pitch) => {
                if let Some(a) = &mut self.audition {
                    a.note_off(pitch);
                }
            }
            Cmd::AuditionInstrument(inst) => {
                if let Some(a) = &mut self.audition {
                    a.set_instrument(inst);
                }
            }
            Cmd::AllNotesOff => {
                if let Some(a) = &mut self.audition {
                    a.all_off();
                }
            }
            // The device callback keeps the routes and the keys player.
            Cmd::Routes(_)
            | Cmd::Keys(_)
            | Cmd::KeysGain(_)
            | Cmd::KeysEvent(_)
            | Cmd::KeysAllOff
            | Cmd::Monitor(_) => {}
        }
    }

    /// Fills interleaved `out` (`channels` wide) and returns the peaks.
    fn render(&mut self, out: &mut [f32], channels: usize, rate: f64, bed_out: bool) -> (f32, f32) {
        self.render_live(out, channels, rate, bed_out, None)
    }

    /// [`Voice::render`] with monitored inputs ([`monitor`]): heard through
    /// their tracks while playing, and on their own while stopped.
    fn render_live(
        &mut self,
        out: &mut [f32],
        channels: usize,
        rate: f64,
        bed_out: bool,
        mut live: Option<&mut monitor::Feed>,
    ) -> (f32, f32) {
        out.fill(0.0);
        if let Some(c) = &mut self.clicks {
            let frames = (out.len() / channels.max(1)).min(c.len());
            c[..frames].fill(0.0);
        }
        let Some(program) = self.program.as_deref_mut() else {
            return (0.0, 0.0);
        };
        if !self.playing {
            return match (program, live) {
                (Program::Mix(m), Some(f)) if !f.block.is_empty() => {
                    monitor_stopped(m, f, self.t, out, channels, rate, bed_out)
                }
                _ => (0.0, 0.0),
            };
        }
        let dt = 1.0 / rate;
        let frames = out.len() / channels;
        let width = if bed_out {
            match program {
                Program::Mix(m) => m.channels_out(),
                Program::Buffer { head: Some(h), .. } if h.is_direct() => h.speakers(),
                _ => 2,
            }
        } else {
            2
        };
        // 3D, listening on speakers: each speaker on its own output (in
        // layout order) when the device has enough of them.
        let speakers = match &*program {
            Program::Mix(m) if !bed_out && m.speakers_out(channels) => Some(m.channels_out()),
            _ => None,
        };
        let mut done = 0;
        let mut peaks = (0.0f32, 0.0f32);
        let mut block = [0.0f32; BLOCK * MAX_SPEAKERS];
        while done < frames && self.playing {
            // Render up to the next seam (the stop point, a loop end or a
            // skipped range), then jump there sample-accurately. The mixer
            // is not reset at a jump: reverbs and delays ring on across it.
            let (seam, action) = self.plan.next_seam(self.t);
            // Within a thousandth of a frame counts as there.
            let eps = dt * 1e-3;
            if self.t >= seam - eps {
                if cross(&mut self.t, &mut self.playing, seam, action) {
                    continue;
                }
                break;
            }
            let until_seam = (((seam - self.t) / dt - 1e-6).ceil().min(1e12) as usize).max(1);
            let mut n = (frames - done).min(BLOCK).min(until_seam);
            // A jump waiting: render only up to where the fade out ends.
            let step = dt / DECLICK_SECS;
            if self.declick.pending.is_some() {
                let to_zero = (self.declick.gain / step - 1e-9).ceil().max(0.0) as usize;
                if to_zero == 0 {
                    let (t, plan) = self.declick.pending.take().unwrap();
                    self.t = plan.start_at(t);
                    self.plan = plan;
                    self.declick.gain = 0.0;
                    continue;
                }
                n = n.min(to_zero);
            }
            let p = match program {
                // Straight to the speaker outputs: the bed itself.
                Program::Buffer { buf, head: Some(h), .. } if bed_out && h.is_direct() => {
                    h.render_bed(buf, self.t, dt, n, &mut block[..n * width]);
                    let mut pk = (0.0f32, 0.0f32);
                    for i in 0..n {
                        let f = &block[i * width..(i + 1) * width];
                        pk = (pk.0.max(f[0].abs()), pk.1.max(f[1].abs()));
                    }
                    pk
                }
                // Direct with a device that cannot take it: the plain fold.
                Program::Buffer { buf, channel_mask, head: Some(h) } if h.is_direct() => {
                    let mut pk = (0.0f32, 0.0f32);
                    for i in 0..n {
                        let x = (self.t + dt * i as f64) * buf.rate as f64;
                        let (l, r) = stereo_frame(buf, x);
                        let l = if *channel_mask & 1 != 0 { l } else { 0.0 };
                        let r = if *channel_mask & 2 != 0 { r } else { 0.0 };
                        block[i * 2] = l;
                        block[i * 2 + 1] = r;
                        pk = (pk.0.max(l.abs()), pk.1.max(r.abs()));
                    }
                    pk
                }
                Program::Buffer { buf, channel_mask, head: Some(h) } => {
                    h.render(buf, self.t, dt, n, &mut block[..n * 2]);
                    let mut pk = (0.0f32, 0.0f32);
                    for i in 0..n {
                        if *channel_mask & 1 == 0 {
                            block[i * 2] = 0.0;
                        }
                        if *channel_mask & 2 == 0 {
                            block[i * 2 + 1] = 0.0;
                        }
                        pk = (pk.0.max(block[i * 2].abs()), pk.1.max(block[i * 2 + 1].abs()));
                    }
                    pk
                }
                Program::Buffer { buf, channel_mask, head: None } => {
                    let mut pk = (0.0f32, 0.0f32);
                    for i in 0..n {
                        let x = (self.t + dt * i as f64) * buf.rate as f64;
                        let (l, r) = stereo_frame(buf, x);
                        let l = if *channel_mask & 1 != 0 { l } else { 0.0 };
                        let r = if *channel_mask & 2 != 0 { r } else { 0.0 };
                        block[i * 2] = l;
                        block[i * 2 + 1] = r;
                        pk = (pk.0.max(l.abs()), pk.1.max(r.abs()));
                    }
                    pk
                }
                Program::Mix(m) => {
                    let click_on = !self.metro.recording_only || self.recording;
                    if let (Some(c), true) = (&mut self.clicks, click_on) {
                        let end = (done + n).min(c.len());
                        if done < end {
                            m.click_into(self.t, dt, &mut c[done..end], &self.metro);
                        }
                    }
                    let lv = live.as_deref_mut().map(|f| {
                        f.pull(n);
                        &f.block
                    });
                    m.render_live(self.t, dt, n, &mut block[..n * width], bed_out, lv, true)
                }
            };
            peaks = (peaks.0.max(p.0), peaks.1.max(p.1));
            for i in 0..n {
                let o = &mut out[(done + i) * channels..(done + i + 1) * channels];
                if bed_out {
                    o[..width].copy_from_slice(&block[i * width..(i + 1) * width]);
                } else if let (Some(w), Program::Mix(m)) = (speakers, &*program) {
                    m.bed_frame(i, &mut o[..w]);
                } else {
                    let (l, r) = (block[i * 2], block[i * 2 + 1]);
                    match channels {
                        1 => o[0] = (l + r) * 0.5,
                        _ => {
                            o[0] = l;
                            o[1] = r;
                        }
                    }
                }
                // Fading out before a jump, or back in after one.
                let d = &mut self.declick;
                if d.pending.is_some() {
                    d.gain = (d.gain - step).max(0.0);
                } else if d.gain < 1.0 {
                    d.gain = (d.gain + step).min(1.0);
                }
                if d.gain < 1.0 {
                    let g = d.gain as f32;
                    for s in o.iter_mut() {
                        *s *= g;
                    }
                }
            }
            self.t += dt * n as f64;
            done += n;
            if self.t >= seam - eps && !cross(&mut self.t, &mut self.playing, seam, action) {
                break;
            }
        }
        peaks
    }
}

/// Stopped, monitoring: the monitored tracks' inputs through the mix with
/// nothing read from the timeline, written like a playing block. The
/// time stands still, so lanes hold their values where the playhead is.
fn monitor_stopped(
    m: &mut Mix,
    f: &mut monitor::Feed,
    t: f64,
    out: &mut [f32],
    channels: usize,
    rate: f64,
    bed_out: bool,
) -> (f32, f32) {
    let dt = 1.0 / rate;
    let frames = out.len() / channels.max(1);
    let width = if bed_out { m.channels_out() } else { 2 };
    let speakers = (!bed_out && m.speakers_out(channels)).then(|| m.channels_out());
    let mut block = [0.0f32; BLOCK * MAX_SPEAKERS];
    let mut peaks = (0.0f32, 0.0f32);
    let mut done = 0;
    while done < frames {
        let n = (frames - done).min(BLOCK);
        f.pull(n);
        let p = m.render_live(t, dt, n, &mut block[..n * width], bed_out, Some(&f.block), false);
        peaks = (peaks.0.max(p.0), peaks.1.max(p.1));
        for i in 0..n {
            let o = &mut out[(done + i) * channels..(done + i + 1) * channels];
            if bed_out {
                o[..width].copy_from_slice(&block[i * width..(i + 1) * width]);
            } else if let Some(w) = speakers {
                m.bed_frame(i, &mut o[..w]);
            } else {
                let (l, r) = (block[i * 2], block[i * 2 + 1]);
                match channels {
                    1 => o[0] = (l + r) * 0.5,
                    _ => {
                        o[0] = l;
                        o[1] = r;
                    }
                }
            }
        }
        done += n;
    }
    peaks
}

/// Adds the click (one value per frame) to the first two channels of
/// interleaved `out` (the live engine maps the click with
/// [`routing::add_clicks_at`]; kept for the click tests).
#[cfg(test)]
fn add_clicks(out: &mut [f32], channels: usize, clicks: &[f32]) {
    let ch = channels.max(1);
    for (f, c) in out.chunks_exact_mut(ch).zip(clicks) {
        if *c != 0.0 {
            for s in f.iter_mut().take(2) {
                *s += c;
            }
        }
    }
}

/// Playback reached `seam`: jump (carrying any overshoot over) and return
/// true, or stop and return false.
fn cross(t: &mut f64, playing: &mut bool, seam: f64, action: transport::Action) -> bool {
    match action {
        transport::Action::Stop => {
            *playing = false;
            false
        }
        transport::Action::Jump(to) => {
            *t = to + (*t - seam).max(0.0);
            true
        }
    }
}

/// Which driver and devices to use. A device name that is None (or no
/// longer present) means the system default.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceChoice {
    /// The system's own driver, or ASIO.
    pub host: HostKind,
    /// The ASIO driver (None: the best guess among those installed).
    pub asio_driver: Option<String>,
    pub output: Option<String>,
    pub input: Option<String>,
    /// Frames per audio callback; None lets the driver choose.
    pub buffer: Option<u32>,
    /// The rate to run the device at; None keeps the device's own.
    pub rate: Option<u32>,
    /// Which outputs the stereo mix and 3D speakers play on.
    pub map: OutputMap,
}

/// The system driver's output devices present now, by name.
pub fn output_devices() -> Vec<String> {
    devices::list(HostKind::System, None, None)
        .outputs
        .into_iter()
        .map(|d| d.name)
        .collect()
}

/// The system driver's recording devices present now, by name.
pub fn input_devices() -> Vec<String> {
    devices::list(HostKind::System, None, None)
        .inputs
        .into_iter()
        .map(|d| d.name)
        .collect()
}

/// The system's default output and input device names.
pub fn default_devices() -> (Option<String>, Option<String>) {
    let h = cpal::default_host();
    (
        h.default_output_device().and_then(|d| d.name().ok()),
        h.default_input_device().and_then(|d| d.name().ok()),
    )
}

/// The system driver layer ("WASAPI" on Windows).
pub fn driver_name() -> String {
    devices::system_name()
}

static INPUT_CHOICE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The recording device new recordings use (None: the system default).
pub fn set_input_device(name: Option<String>) {
    *INPUT_CHOICE.lock().unwrap_or_else(|e| e.into_inner()) = name;
}

pub(crate) fn find_output(host: &cpal::Host, name: Option<&str>) -> Option<cpal::Device> {
    name.and_then(|n| {
        host.output_devices()
            .ok()?
            .find(|d| d.name().is_ok_and(|x| x == n))
    })
    .or_else(|| host.default_output_device())
}

pub(crate) fn find_input(host: &cpal::Host) -> Option<cpal::Device> {
    let want = INPUT_CHOICE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    want.and_then(|n| {
        host.input_devices()
            .ok()?
            .find(|d| d.name().is_ok_and(|x| x == n))
    })
    .or_else(|| host.default_input_device())
}

pub struct Engine {
    /// Dropped last, after the device has taken a few silent buffers.
    stream: Option<cpal::Stream>,
    /// Set when the output device went away (unplugged) or failed.
    lost: Arc<AtomicBool>,
    /// Set when the callback caught a panic.
    fault: Arc<AtomicBool>,
    hush: Arc<health::Hush>,
    stats: Arc<health::CallbackStats>,
    watchdog: health::Watchdog,
    /// The frames per callback asked for, if any.
    pub buffer: Option<u32>,
    tx: rtrb::Producer<Cmd>,
    trash: rtrb::Consumer<Garbage>,
    shared: Arc<Shared>,
    pub rate: u32,
    pub device: String,
    /// The driver layer in use (after any fallback).
    pub host: HostKind,
    /// "WASAPI", or "ASIO (SSL ASIO Driver)".
    pub driver: String,
    /// The device's output channels as opened.
    pub channels: usize,
    pub format: cpal::SampleFormat,
    /// What the user should know about how the device opened: a fallback
    /// to the system driver, a rate or buffer size it could not use.
    pub note: Option<String>,
    map: OutputMap,
    layout: Option<af2_core::space::SpaceLayout>,
    /// The ASIO driver's device, which recording must share.
    asio_device: Option<cpal::Device>,
    /// On ASIO: the driver as an input device, for the track input menus.
    pub asio_inputs: Option<DeviceInfo>,
    recorders: Vec<record::Recorder>,
    /// Input monitoring: what is asked for and the inputs opened for it.
    monitor: MonitorState,
    /// Notes played live (MIDI input), until a sender is taken.
    notes: Option<rtrb::Producer<LiveNote>>,
    /// Events for a plug-in instrument played live (MIDI input), until a
    /// sender is taken.
    keys: Option<rtrb::Producer<af2_dsp::instrument::NoteEvent>>,
}

/// A note played live, straight to the audition instrument from another
/// thread (the MIDI input), without waiting for the UI.
#[derive(Debug, Clone, Copy)]
pub enum LiveNote {
    On { pitch: u8, velocity: u8, pan: f32 },
    Off(u8),
    AllOff,
    /// A controller (64 sustain, 66 sostenuto, 1 mod wheel, 129 pitch
    /// bend...), value 0..1.
    Control { controller: u8, value: f32 },
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Inputs first (on ASIO they share the driver), then end the
        // output on silence before it closes.
        self.monitor.streams.clear();
        self.recorders.clear();
        if let Some(s) = self.stream.take() {
            self.hush.settle();
            drop(s);
        }
    }
}

impl Engine {
    /// The sender for live notes (once per opened engine): the MIDI
    /// input's thread plays the audition instrument through it, heard
    /// within one device buffer.
    pub fn take_note_sender(&mut self) -> Option<rtrb::Producer<LiveNote>> {
        self.notes.take()
    }

    /// Opens the default output device.
    pub fn new() -> Result<Engine, String> {
        Engine::open(&DeviceChoice::default())
    }

    /// Opens the chosen output device, or the system default when it is
    /// not there (and the system driver when an ASIO driver will not open).
    pub fn open(choice: &DeviceChoice) -> Result<Engine, String> {
        stream::open(choice)
    }

    /// The output device went away or failed: reopen (the default).
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }

    /// Call every frame: why the output should be opened again (gone,
    /// failed, stopped calling back, or a fault on the audio thread).
    pub fn check(&mut self, now: std::time::Instant) -> Option<String> {
        if self.is_lost() {
            health::log(&format!("play: \"{}\" went away or failed", self.device));
            return Some("The audio device went away or failed.".into());
        }
        if self.fault.swap(false, Ordering::Relaxed) {
            health::log("play: a fault on the audio thread (see the panic above)");
            return Some("Playback stopped after a fault on the audio thread.".into());
        }
        let calls = self.stats.calls.load(Ordering::Relaxed);
        if self.watchdog.stalled(calls, now) {
            health::log(&format!(
                "play: \"{}\" stopped calling back after {calls} buffers",
                self.device
            ));
            return Some(format!("{} stopped asking for sound.", self.device));
        }
        None
    }

    /// The frames the device really asks for each callback, once it has.
    pub fn real_buffer(&self) -> Option<u32> {
        self.stats.buffer()
    }

    /// The channel map this output was opened with.
    pub fn map(&self) -> &OutputMap {
        &self.map
    }

    /// The 3D speaker layout of what plays (None: a stereo session), so
    /// its speakers can go to their own outputs. Live.
    pub fn set_layout(&mut self, layout: Option<af2_core::space::SpaceLayout>) {
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        let names: Option<Vec<&str>> =
            layout.map(|l| af2_space::speakers(l).iter().map(|s| s.name).collect());
        let r = self.map.routes(names.as_deref());
        self.send(Cmd::Routes(r));
    }

    /// Whether every speaker of `layout` has its own output here:
    /// Channel Routing's speaker outputs are on and the device is wide
    /// enough for them.
    pub fn speakers_fit(&self, layout: af2_core::space::SpaceLayout) -> bool {
        if !self.map.speakers_out {
            return false;
        }
        let names: Vec<&str> = af2_space::speakers(layout).iter().map(|s| s.name).collect();
        self.map.width_needed(Some(&names)) <= self.channels
    }

    /// Speakers (or "Left"/"Right") this device has no output for.
    pub fn unplayable(&self) -> Vec<String> {
        let names: Option<Vec<&str>> = self
            .layout
            .map(|l| af2_space::speakers(l).iter().map(|s| s.name).collect());
        self.map.unplayable(names.as_deref(), self.channels)
    }

    fn send(&mut self, c: Cmd) {
        let _ = self.tx.push(c);
    }

    pub fn set_program(&mut self, p: Program) {
        self.send(Cmd::Program(Box::new(p)));
    }

    /// Plays from `from` up to `to` (None = the end), looping `from..to`
    /// when asked.
    pub fn play(&mut self, from: f64, to: Option<f64>, looping: bool, program_len: f64) {
        let to = to.unwrap_or(program_len).max(from);
        let plan = if looping {
            Plan::new(
                f64::INFINITY,
                Region::Loop {
                    start: from,
                    end: to,
                },
            )
        } else {
            Plan::new(to, Region::None)
        };
        self.play_plan(from, plan);
    }

    /// Plays from `from` (or from the end of a skipped range it is in),
    /// following `plan`. Effect tails start clean.
    pub fn play_plan(&mut self, from: f64, plan: Plan) {
        self.send(Cmd::Play { from, plan });
        self.shared.playing.store(true, Ordering::Relaxed);
        self.shared
            .pos
            .store(plan.start_at(from).to_bits(), Ordering::Relaxed);
    }

    /// While playing: new bounds, and a new position when `to` is Some,
    /// without a gap or a reset (effect tails continue).
    pub fn retarget(&mut self, to: Option<f64>, plan: Plan) {
        self.send(Cmd::Retarget { to, plan });
        if let Some(t) = to {
            self.shared
                .pos
                .store(plan.start_at(t).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn stop(&mut self) {
        self.send(Cmd::Stop);
    }

    /// A strip's fader (linear) and pan (-1..1), ramped in over a block.
    pub fn set_strip(&mut self, track: usize, fader: f32, pan: f32) {
        self.send(Cmd::Strip {
            track: track as u16,
            fader,
            pan,
        });
    }

    /// One send's level (linear), pan and on/off.
    pub fn set_send(&mut self, track: usize, send: usize, level: f32, pan: f32, on: bool) {
        self.send(Cmd::Send {
            track: track as u16,
            send: send as u16,
            level,
            pan,
            on,
        });
    }

    /// Switches a lane to its live control while it is written (`track`
    /// None: the master).
    pub fn set_lane_live(&mut self, track: Option<usize>, kind: mix::LaneKind, live: bool) {
        self.send(Cmd::LaneLive {
            track: track.map_or(MASTER, |t| t as u16),
            kind,
            live,
        });
    }

    pub fn set_audible(&mut self, track: usize, on: bool) {
        self.send(Cmd::Audible {
            track: track as u16,
            on,
        });
    }

    pub fn set_master(&mut self, gain: f32) {
        self.send(Cmd::Master(gain));
    }

    /// The master's balance (-1..=1), mono sum and polarity flip.
    pub fn set_master_opts(&mut self, pan: f32, mono: bool, invert: bool) {
        self.send(Cmd::MasterOpts { pan, mono, invert });
    }

    /// New effect settings for `slot` of a track's chain (or [`MASTER`]).
    pub fn set_param(&mut self, track: u16, slot: usize, values: &[f32]) {
        let mut vals = [0.0; MAX_VALUES];
        let n = values.len().min(MAX_VALUES);
        vals[..n].copy_from_slice(&values[..n]);
        self.send(Cmd::Param {
            track,
            slot: slot as u8,
            n: n as u8,
            vals,
        });
    }

    pub fn set_bypass(&mut self, track: u16, slot: usize, on: bool) {
        self.send(Cmd::Bypass {
            track,
            slot: slot as u8,
            on,
        });
    }

    /// New settings for `slot` of a clip's own rack.
    pub fn set_clip_param(&mut self, clip: af2_core::ClipId, slot: usize, values: &[f32]) {
        let mut vals = [0.0; MAX_VALUES];
        let n = values.len().min(MAX_VALUES);
        vals[..n].copy_from_slice(&values[..n]);
        self.send(Cmd::ClipParam {
            clip,
            slot: slot as u8,
            n: n as u8,
            vals,
        });
    }

    pub fn set_clip_bypass(&mut self, clip: af2_core::ClipId, slot: usize, on: bool) {
        self.send(Cmd::ClipBypass {
            clip,
            slot: slot as u8,
            on,
        });
    }

    /// A clip rack's power, levels and mix.
    pub fn set_clip_rack(&mut self, clip: af2_core::ClipId, rack: af2_core::Rack) {
        self.send(Cmd::ClipRack { clip, rack });
    }

    pub fn set_chain_mix(&mut self, track: u16, mix: f32) {
        self.send(Cmd::ChainMix { track, mix });
    }

    /// A track's new spot in the room (heard at once, gliding there).
    pub fn set_space(&mut self, track: usize, pos: af2_core::space::SpacePos) {
        self.send(Cmd::Space {
            track: track as u16,
            pos,
        });
    }

    pub fn set_instrument(&mut self, track: usize, inst: af2_core::midi::Instrument) {
        self.send(Cmd::Instrument {
            track: track as u16,
            inst,
        });
    }

    /// The click on or off (the session's `tempo.metronome`).
    pub fn set_metronome(&mut self, on: bool) {
        self.send(Cmd::Metronome(on));
    }

    /// The click's sound, level, accent, subdivisions, mute and whether it
    /// plays only while recording. Kept across programs.
    pub fn set_metronome_settings(&mut self, settings: af2_core::midi::MetronomeSettings) {
        self.send(Cmd::MetronomeSettings(settings));
    }

    /// New rack settings (power, input and output gain, dry/wet mix) for a
    /// track's chain, by track index, or [`MASTER`] for the master chain.
    /// Live, without rebuilding the mix.
    pub fn set_rack(&mut self, track: u16, rack: af2_core::Rack) {
        self.send(Cmd::Rack { track, rack });
    }

    pub fn set_font_gain(&mut self, track: usize, db: f32) {
        self.send(Cmd::FontGain {
            track: track as u16,
            db,
        });
    }

    /// The sender for a MIDI keyboard playing a track's plug-in instrument
    /// (once per opened engine), from the MIDI input's thread.
    pub fn take_keys_sender(&mut self) -> Option<rtrb::Producer<af2_dsp::instrument::NoteEvent>> {
        self.keys.take()
    }

    /// What a MIDI keyboard plays a plug-in instrument through (built on
    /// this thread), or None to stop.
    pub fn set_keys_player(&mut self, p: Option<plugin_inst::KeysPlayer>) {
        self.send(Cmd::Keys(p.map(Box::new)));
    }

    /// The keys player's left and right gain (the track's fader and pan).
    pub fn set_keys_gain(&mut self, gain: [f32; 2]) {
        self.send(Cmd::KeysGain(gain));
    }

    /// One event for the keys player (the piano roll's notes).
    pub fn keys_event(&mut self, e: af2_dsp::instrument::NoteEvent) {
        self.send(Cmd::KeysEvent(e));
    }

    pub fn keys_all_off(&mut self) {
        self.send(Cmd::KeysAllOff);
    }

    /// Sets what hand-played notes go through (built on this thread).
    pub fn set_audition(&mut self, a: soundfont::Audition) {
        self.send(Cmd::Audition(Box::new(a)));
    }

    pub fn note_on(&mut self, pitch: u8, velocity: u8, pan: f32) {
        self.send(Cmd::NoteOn {
            pitch,
            velocity,
            pan,
        });
    }

    pub fn note_off(&mut self, pitch: u8) {
        self.send(Cmd::NoteOff(pitch));
    }

    pub fn all_notes_off(&mut self) {
        self.send(Cmd::AllNotesOff);
    }

    /// New settings for the built-in synth the audition plays, live.
    pub fn set_audition_instrument(&mut self, inst: af2_core::midi::Instrument) {
        self.send(Cmd::AuditionInstrument(inst));
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    pub fn position(&self) -> f64 {
        f64::from_bits(self.shared.pos.load(Ordering::Relaxed))
    }

    /// Peak levels since the last call (left, right), linear.
    pub fn take_peaks(&self) -> (f32, f32) {
        let l = f32::from_bits(self.shared.peak_l.swap(0, Ordering::Relaxed));
        let r = f32::from_bits(self.shared.peak_r.swap(0, Ordering::Relaxed));
        (l, r)
    }

    /// The output's mean square (left, right, linear power) since the
    /// last call, for RMS meters; None when no audio came through since.
    pub fn take_mean_squares(&self) -> Option<(f32, f32)> {
        self.shared.mean_square.take()
    }

    /// 3D sessions: each speaker's peak since the last call, linear, in
    /// layout order (after the master chain and fader); empty otherwise.
    pub fn take_bed_peaks(&self) -> Vec<f32> {
        let n = (self.shared.bed_count.load(Ordering::Relaxed) as usize).min(MAX_SPEAKERS);
        self.shared.bed_peaks[..n]
            .iter()
            .map(|a| f32::from_bits(a.swap(0, Ordering::Relaxed)))
            .collect()
    }

    /// A file of more than two channels playing alone (the Waveform
    /// editor): each channel's peak since the last call, linear, before
    /// it is folded or placed around the head; empty otherwise.
    pub fn take_source_peaks(&self) -> Vec<f32> {
        let n = (self.shared.src_count.load(Ordering::Relaxed) as usize).min(MAX_SPEAKERS);
        self.shared.src_peaks[..n]
            .iter()
            .map(|a| f32::from_bits(a.swap(0, Ordering::Relaxed)))
            .collect()
    }

    /// Frees programs the audio thread has finished with. Call every frame.
    pub fn collect(&mut self) {
        while self.trash.pop().is_ok() {}
        for r in &mut self.recorders {
            r.drain();
        }
        // A monitoring input that reset or went away: opened again after
        // a moment.
        let now = std::time::Instant::now();
        let before = self.monitor.streams.len();
        self.monitor
            .streams
            .retain(|s| !s.reset.load(Ordering::Relaxed));
        if self.monitor.streams.len() != before {
            self.monitor.retry = Some(now + std::time::Duration::from_secs(1));
        }
        if self.monitor.retry.is_some_and(|t| now >= t) {
            self.monitor.retry = None;
            self.rebuild_monitor();
        }
    }

    /// Hears each lane's input live through its track (see [`monitor`]):
    /// call every frame with the tracks to monitor; inputs open and close
    /// only when the list changes. Works stopped, playing and recording
    /// (the takes stay dry).
    pub fn set_monitor(&mut self, lanes: Vec<MonitorLane>) {
        if lanes == self.monitor.want {
            return;
        }
        self.monitor.want = lanes;
        self.rebuild_monitor();
    }

    /// Whether any input is heard live now.
    pub fn is_monitoring(&self) -> bool {
        self.monitor.active
    }

    /// Why a monitored input can't be heard, once per new reason.
    pub fn take_monitor_note(&mut self) -> Option<String> {
        self.monitor.note.take()
    }

    /// Opens (or reuses) an input for every monitored device, gives each a
    /// fresh ring and hands the audio thread a new feed. A device a take
    /// is recording from lends its stream (on ASIO there is only one).
    fn rebuild_monitor(&mut self) {
        let mut notes: Vec<String> = Vec::new();
        let mut lanes: Vec<(af2_core::TrackId, record::Resolved)> = Vec::new();
        for l in &self.monitor.want {
            if l.source.is_system() {
                notes.push(format!(
                    "{} can't be heard live (it would play itself back).",
                    l.source.label()
                ));
                continue;
            }
            match record::resolve(&l.source, self.host, self.asio_device.as_ref()) {
                Ok(r) => lanes.push((l.track, r)),
                Err(e) => notes.push(e),
            }
        }
        let mut names: Vec<(String, cpal::Device)> = Vec::new();
        for (_, r) in &lanes {
            if !names.iter().any(|(n, _)| *n == r.name) {
                names.push((r.name.clone(), r.device.clone()));
            }
        }
        let recording: Vec<String> = self
            .recorders
            .iter()
            .filter(|r| r.tap.is_some())
            .map(|r| r.device_name.clone())
            .collect();
        self.monitor.streams.retain(|s| {
            names.iter().any(|(n, _)| *n == s.name) && !recording.contains(&s.name)
        });
        // Let go of every old ring first.
        let taps = self
            .recorders
            .iter()
            .filter_map(|r| r.tap.as_ref())
            .chain(self.monitor.streams.iter().map(|s| &s.tap));
        for t in taps {
            if let Ok(mut g) = t.lock() {
                *g = None;
            }
        }
        let mut rings = Vec::new();
        let mut feed = Vec::new();
        for (name, device) in &names {
            let from_rec = self
                .recorders
                .iter()
                .find(|r| r.device_name == *name)
                .and_then(|r| r.tap.clone().map(|t| (t, r.rate, r.channels)));
            let from_own = self
                .monitor
                .streams
                .iter()
                .find(|s| s.name == *name)
                .map(|s| (s.tap.clone(), s.rate, s.channels));
            let found = match from_rec.or(from_own) {
                Some(f) => f,
                None => match record::monitor_stream(device, name, self.rate) {
                    Ok(s) => {
                        let f = (s.tap.clone(), s.rate, s.channels);
                        self.monitor.streams.push(s);
                        f
                    }
                    Err(e) => {
                        notes.push(e);
                        continue;
                    }
                },
            };
            let (tap, rate, channels) = found;
            if rate != self.rate {
                notes.push(format!(
                    "{name} records at {rate} Hz and the output plays at {} Hz, so it can't \
                     be heard live while recording.",
                    self.rate
                ));
                continue;
            }
            let (tx, rx) = monitor::ring(rate, channels);
            if let Ok(mut g) = tap.lock() {
                *g = Some(tx);
            }
            let dev = rings.len();
            rings.push((rx, channels));
            for (track, r) in lanes.iter().filter(|(_, r)| r.name == *name) {
                feed.push(monitor::FeedLane {
                    track: *track,
                    dev,
                    pick: monitor::pick_of(r.channels.as_deref(), channels),
                });
            }
        }
        let feed = (!feed.is_empty()).then(|| Box::new(monitor::Feed::new(rings, feed)));
        self.monitor.active = feed.is_some();
        self.send(Cmd::Monitor(feed));
        let note = (!notes.is_empty()).then(|| notes.join(" "));
        if note.is_some() && note != self.monitor.last_note {
            self.monitor.note = note.clone();
        }
        self.monitor.last_note = note;
    }

    /// Records from the default input device.
    pub fn start_recording(&mut self) -> Result<(), String> {
        self.start_recording_from(&[RecordSource::Input])
    }

    /// Records each source at once (a game on one track, a voice chat on
    /// another, channels 1 and 2 of an interface on two more). Sources on
    /// one input device share one stream. All or nothing: if one fails to
    /// open, none run.
    pub fn start_recording_from(&mut self, sources: &[RecordSource]) -> Result<(), String> {
        // Monitoring's own inputs close first (an ASIO driver has one
        // input stream); the recorders' streams feed it from here on.
        let monitoring = !self.monitor.streams.is_empty();
        self.monitor.streams.clear();
        let recs = match record::open_all(sources, self.host, self.asio_device.as_ref()) {
            Ok(r) => r,
            Err(e) => {
                if monitoring {
                    self.rebuild_monitor();
                }
                return Err(e);
            }
        };
        self.recorders = recs;
        self.send(Cmd::Recording(true));
        if !self.monitor.want.is_empty() {
            self.rebuild_monitor();
        }
        Ok(())
    }

    pub fn is_recording(&self) -> bool {
        !self.recorders.is_empty()
    }

    pub fn recording_len_secs(&self) -> f64 {
        self.recorders
            .iter()
            .map(|r| r.len_secs())
            .fold(0.0, f64::max)
    }

    /// The takes so far, for drawing while recording: calls `f` with each
    /// source's index (as given to [`Engine::start_recording_from`]), its
    /// rate, and min/max pairs of [`record::LIVE_BIN`] frames each.
    pub fn recording_live(&self, mut f: impl FnMut(usize, u32, &[[f32; 2]])) {
        for r in &self.recorders {
            for (i, peaks) in r.live() {
                f(i, r.rate, peaks);
            }
        }
    }

    pub fn recording_level(&self) -> f32 {
        self.recorders
            .iter()
            .map(|r| f32::from_bits(r.peak.swap(0, Ordering::Relaxed)))
            .fold(0.0, f32::max)
    }

    /// The loudest input's mean square (linear power) over the audio taken
    /// in since the last call, for RMS meters; None when none came in.
    pub fn recording_mean_square(&mut self) -> Option<f32> {
        self.recorders
            .iter_mut()
            .filter_map(|r| r.take_mean_square())
            .reduce(f32::max)
    }

    /// Problems a running capture hit (a device went away, restarted or
    /// changed its rate), each taken once.
    pub fn take_recording_errors(&mut self) -> Vec<String> {
        self.recorders
            .iter_mut()
            .filter_map(|r| r.take_error())
            .collect()
    }

    /// Stops recording and returns the first source's take.
    pub fn stop_recording(&mut self) -> Option<AudioBuffer> {
        self.stop_recording_all().into_iter().next().map(|t| t.buf)
    }

    /// Stops recording and returns one take per source, in the order
    /// they were given.
    pub fn stop_recording_all(&mut self) -> Vec<Take> {
        self.send(Cmd::Recording(false));
        let takes = record::finish_all(std::mem::take(&mut self.recorders));
        // The recorders' inputs are closed: monitoring opens its own again.
        if !self.monitor.want.is_empty() {
            self.rebuild_monitor();
        }
        takes
    }
}

/// A track heard live through its effects, and the input it listens to
/// (see [`monitor`] and [`Engine::set_monitor`]).
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorLane {
    pub track: af2_core::TrackId,
    pub source: RecordSource,
}

/// The engine's input monitoring, on the UI thread.
#[derive(Default)]
struct MonitorState {
    /// What the app asks for.
    want: Vec<MonitorLane>,
    /// Inputs opened only to be heard (devices no take records from).
    streams: Vec<record::MonitorStream>,
    /// A feed with at least one track went to the audio thread.
    active: bool,
    note: Option<String>,
    last_note: Option<String>,
    /// When to open a monitoring input that reset again.
    retry: Option<std::time::Instant>,
}

/// One recorded source.
pub struct Take {
    pub source: RecordSource,
    pub buf: AudioBuffer,
    /// What went wrong while recording, or a note such as a fallback.
    pub note: Option<String>,
}

/// Program for a buffer playing both sides.
pub fn buffer_program(buf: Arc<AudioBuffer>) -> Program {
    Program::Buffer {
        buf,
        channel_mask: 0b11,
        head: None,
    }
}

/// Program for a buffer whose surround channels (more than two, named
/// from its WAV `mask`) are heard as `mode` says, played out at `rate`.
pub fn buffer_program_for(buf: Arc<AudioBuffer>, mask: Option<u32>, mode: SurroundPlay, rate: u32) -> Program {
    let head = HeadRender::new(buf.num_channels(), mask, rate, mode).map(Box::new);
    Program::Buffer {
        buf,
        channel_mask: 0b11,
        head,
    }
}

/// Builds a mix program from session data and loaded media, at `rate`.
pub fn mix_program(
    session: &af2_core::Session,
    media: &dyn Fn(af2_core::MediaId) -> Option<Arc<AudioBuffer>>,
    rate: u32,
) -> Program {
    Program::Mix(Box::new(mix::build(session, media, rate)))
}

fn render_with(program: Program, rate: u32, from: f64, to: f64, bed_out: bool) -> AudioBuffer {
    crate::denormal::flush_to_zero();
    let channels = match (&program, bed_out) {
        (Program::Mix(m), true) => m.channels_out(),
        (Program::Buffer { head: Some(h), .. }, true) if h.is_direct() => h.speakers(),
        _ => 2,
    };
    let mut v = Voice {
        program: Some(Box::new(program)),
        audition: None,
        playing: true,
        t: from,
        plan: Plan::new(to, Region::None),
        metro: Default::default(),
        recording: false,
        clicks: None,
        declick: Default::default(),
    };
    if let Some(Program::Mix(m)) = v.program.as_deref_mut() {
        m.set_offline();
        // Offline: every stretched clip as it will finally sound.
        m.prepare_now();
        m.start(from);
    }
    let frames = ((to - from).max(0.0) * rate as f64).round() as usize;
    let mut inter = vec![0.0f32; frames * channels];
    for chunk in inter.chunks_mut(BLOCK * channels) {
        v.render(chunk, channels, rate as f64, bed_out);
    }
    AudioBuffer::from_interleaved(rate, channels, &inter)
}

/// Offline render handed out block by block (interleaved), so a long
/// mixdown can go straight to a file without sitting in memory. `each`
/// returns false to stop early. Returns the channel count.
pub fn render_each(
    program: Program,
    rate: u32,
    from: f64,
    to: f64,
    bed_out: bool,
    mut each: impl FnMut(&[f32]) -> bool,
) -> usize {
    let channels = match (&program, bed_out) {
        (Program::Mix(m), true) => m.channels_out(),
        _ => 2,
    };
    let mut v = Voice {
        program: Some(Box::new(program)),
        playing: true,
        t: from,
        plan: Plan::new(to, Region::None),
        audition: None,
        metro: Default::default(),
        recording: false,
        clicks: None,
        declick: Default::default(),
    };
    if let Some(Program::Mix(m)) = v.program.as_deref_mut() {
        m.set_offline();
        // Offline: every stretched clip as it will finally sound.
        m.prepare_now();
        m.start(from);
    }
    let mut left = ((to - from).max(0.0) * rate as f64).round() as usize;
    let mut block = vec![0.0f32; BLOCK * channels];
    while left > 0 {
        let n = left.min(BLOCK);
        let chunk = &mut block[..n * channels];
        chunk.fill(0.0);
        v.render(chunk, channels, rate as f64, bed_out);
        if !each(chunk) {
            break;
        }
        left -= n;
    }
    channels
}

/// The channels [`render_each`] hands out for a program.
pub fn render_channels(program: &Program, bed_out: bool) -> usize {
    match (program, bed_out) {
        (Program::Mix(m), true) => m.channels_out(),
        (Program::Buffer { head: Some(h), .. }, true) if h.is_direct() => h.speakers(),
        _ => 2,
    }
}

#[cfg(test)]
mod render_each_tests {
    use super::*;

    #[test]
    fn blocks_match_the_whole_render() {
        let mut b = AudioBuffer::new(48_000, 2, 30_000);
        for (i, s) in b.channels[0].iter_mut().enumerate() {
            *s = (i as f32 * 0.01).sin() * 0.5;
        }
        let buf = Arc::new(b);
        let prog = || Program::Buffer {
            buf: buf.clone(),
            channel_mask: 0b11,
            head: None,
        };
        let whole = render(prog(), 48_000, 0.1, 0.5);
        let mut got = Vec::new();
        let ch = render_each(prog(), 48_000, 0.1, 0.5, false, |blk| {
            got.extend_from_slice(blk);
            true
        });
        assert_eq!(ch, 2);
        assert_eq!(got.len(), whole.frames() * 2);
        assert_eq!(AudioBuffer::from_interleaved(48_000, 2, &got), whole);
    }
}

/// Offline stereo mixdown (3D sessions as heard: binaural or folded).
pub fn render(program: Program, rate: u32, from: f64, to: f64) -> AudioBuffer {
    render_with(program, rate, from, to, false)
}

/// Offline mixdown of a 3D session's speaker bed, one channel per speaker.
pub fn render_bed(program: Program, rate: u32, from: f64, to: f64) -> AudioBuffer {
    render_with(program, rate, from, to, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use af2_core::session::{Clip, MediaItem, Send};
    use af2_core::{Session, TrackKind};

    fn session_with_tone() -> (Session, Arc<AudioBuffer>) {
        let mut s = Session::new("t", 100, 1);
        let m = s.alloc_id();
        s.media.push(MediaItem {
            id: m,
            name: "x".into(),
            path: None,
            duration: 2.0,
            rate: 100,
            channels: 1,
            ext_id: None,
        });
        let buf = Arc::new(AudioBuffer {
            rate: 100,
            channels: vec![vec![1.0; 200]],
        });
        s.tracks[0].clips.push(Clip {
            props: Default::default(),
            id: 99,
            media: m,
            name: "x".into(),
            start: 1.0,
            offset: 0.0,
            length: 1.0,
            gain_db: 0.0,
            fade_in: 0.0,
            fade_out: 0.5,
            muted: false,
            ext_id: None,
        });
        (s, buf)
    }

    /// A clip played from disk sounds exactly as the same audio in memory,
    /// offline (waiting for chunks) and live once its chunks are loaded.
    #[test]
    fn a_streamed_clip_matches_the_same_audio_in_memory() {
        let (mut s, _) = session_with_tone();
        let frames = 20_000usize;
        let src: Vec<Vec<f32>> = vec![
            (0..frames).map(|i| ((i as f32) * 0.037).sin()).collect(),
            (0..frames).map(|i| ((i as f32) * 0.011).cos() * 0.5).collect(),
        ];
        let mem = Arc::new(AudioBuffer { rate: 100, channels: src.clone() });
        s.media[0].duration = 200.0;
        s.media[0].channels = 2;
        let c = &mut s.tracks[0].clips[0];
        c.start = 0.3;
        c.offset = 1.7;
        c.length = 150.0;
        c.fade_out = 0.0;
        let chunk = 700;
        let data = src.clone();
        let stream = disk::DiskStream::open(
            100,
            2,
            frames,
            chunk,
            Box::new(move |k| {
                let a = k * chunk;
                let b = (a + chunk).min(frames);
                Some(data.iter().map(|ch| ch[a..b].to_vec()).collect())
            }),
        )
        .unwrap();
        let stand_in = disk::stand_in(&stream);
        let m2 = mem.clone();
        let want = render(mix_program(&s, &move |_| Some(m2.clone()), 100), 100, 0.0, 160.0);
        let si = stand_in.clone();
        let got = render(mix_program(&s, &move |_| Some(si.clone()), 100), 100, 0.0, 160.0);
        assert_eq!(want.channels, got.channels, "offline");
        // Live: a mix reading without waiting, the chunks already there.
        let si = stand_in.clone();
        let Program::Mix(mut live) = mix_program(&s, &move |_| Some(si.clone()), 100) else {
            panic!()
        };
        let at = 90.0;
        stream.hint(Some((at * 100.0) as usize));
        std::thread::sleep(std::time::Duration::from_millis(300));
        live.start(at);
        let mut out = vec![0.0f32; 64 * 2];
        live.render(at, 0.01, 64, &mut out, false);
        let w = (at * 100.0) as usize;
        for i in 0..64 {
            assert_eq!(out[i * 2], want.channels[0][w + i], "live frame {i}");
        }
    }

    /// A clip on top hides the one under it, which comes through only
    /// under the top clip's fades; a lower clip fading itself (a
    /// crossfade) plays its own fade.
    #[test]
    fn a_clip_on_top_covers_the_one_under_it() {
        let (mut s, buf) = session_with_tone();
        s.media[0].duration = 10.0;
        let ones = Arc::new(AudioBuffer { rate: 100, channels: vec![vec![1.0; 1000]] });
        let base = s.tracks[0].clips[0].clone();
        let mut under = base.clone();
        under.id = 1;
        under.start = 0.0;
        under.length = 8.0;
        under.fade_out = 0.0;
        let mut top = base.clone();
        top.id = 2;
        top.start = 2.0;
        top.length = 4.0;
        top.fade_in = 1.0;
        top.fade_out = 0.0;
        s.tracks[0].clips = vec![under.clone(), top.clone()];
        s.tracks[0].pan = 0.0;
        let b = ones.clone();
        let out = render(mix_program(&s, &move |_| Some(b.clone()), 100), 100, 0.0, 8.0);
        let l = &out.channels[0];
        let alone = l[100];
        assert!(alone > 0.1);
        // Inside the top clip's body only the top clip sounds.
        assert!((l[450] - alone).abs() < 1e-4, "{} vs {alone}", l[450]);
        // Under its fade in the two add to one clip's level (a linear fade).
        assert!((l[250] - alone).abs() < 1e-3, "{} vs {alone}", l[250]);
        // After it, the lower clip again.
        assert!((l[700] - alone).abs() < 1e-4);
        // The other way round, the lower one would be on top: no doubling.
        s.tracks[0].clips = vec![top.clone(), under.clone()];
        let b = ones.clone();
        let out = render(mix_program(&s, &move |_| Some(b.clone()), 100), 100, 0.0, 8.0);
        assert!((out.channels[0][450] - alone).abs() < 1e-4);
        // A crossfade: the lower clip fades out over the top one's fade in,
        // and both follow their own fades.
        let mut a = under.clone();
        a.length = 3.0;
        a.fade_out = 1.0;
        s.tracks[0].clips = vec![a, top];
        let b = ones.clone();
        let out = render(mix_program(&s, &move |_| Some(b.clone()), 100), 100, 0.0, 8.0);
        assert!((out.channels[0][250] - alone).abs() < 1e-3, "{}", out.channels[0][250]);
        drop(buf);
    }

    #[test]
    fn offline_mix_places_clips_and_fades() {
        let (mut s, buf) = session_with_tone();
        s.tracks[0].pan = 0.5;
        let b2 = buf.clone();
        let out = render(
            mix_program(&s, &move |_| Some(b2.clone()), 100),
            100,
            0.0,
            3.0,
        );
        assert_eq!(out.frames(), 300);
        assert_eq!(out.channels[0][50], 0.0);
        assert!(out.channels[1][120] > out.channels[0][120]);
        assert!((out.channels[0][175] / out.channels[0][120] - 0.5).abs() < 0.03);
        assert_eq!(out.channels[0][250], 0.0);
    }

    #[test]
    fn sends_and_buses_and_chains() {
        let (mut s, buf) = session_with_tone();
        let bus = s.add_track_of(TrackKind::Bus);
        // Track to the bus only; bus at -6 dB with an Amplify of +6 dB: unity.
        s.tracks[0].output = Some(bus);
        let bi = s.track_index(bus).unwrap();
        s.tracks[bi].volume_db = -6.0206;
        s.tracks[bi].chain.push(af2_core::EffectSlot {
            position: None,
            sidechain: None,
            effect: "amplify".into(),
            values: vec![6.0206],
            bypass: false,
            data: None,
            ext_id: None,
        });
        let b2 = buf.clone();
        let out = render(
            mix_program(&s, &move |_| Some(b2.clone()), 100),
            100,
            0.0,
            2.0,
        );
        assert!(
            (out.channels[0][120] - 1.0).abs() < 1e-3,
            "{}",
            out.channels[0][120]
        );
        // A pre-fader send with the track itself muted by fader still reaches the bus.
        s.tracks[0].output = None;
        s.tracks[0].volume_db = -120.0;
        s.tracks[0].sends.push(Send {
            bus,
            level_db: 0.0,
            pre_fader: true,
            pan: 0.0,
            enabled: true,
            ext_id: None,
        });
        let b2 = buf.clone();
        let out = render(
            mix_program(&s, &move |_| Some(b2.clone()), 100),
            100,
            0.0,
            2.0,
        );
        assert!((out.channels[0][120] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn a_3d_session_exports_every_speaker() {
        let (mut s, buf) = session_with_tone();
        s.space = Some(af2_core::space::SpaceLayout::Surround51);
        // Straight ahead at the centre speaker.
        s.tracks[0].pos.x = 0.0;
        s.tracks[0].pos.y = 1.0;
        let b2 = buf.clone();
        let bed = render_bed(
            mix_program(&s, &move |_| Some(b2.clone()), 100),
            100,
            0.0,
            2.0,
        );
        assert_eq!(bed.num_channels(), 6);
        // Mono source: half from each side, both at the centre speaker.
        assert!(
            (bed.channels[2][120] - 1.0).abs() < 1e-3,
            "{:?}",
            bed.channels.iter().map(|c| c[120]).collect::<Vec<_>>()
        );
        assert!(bed.channels[0][120].abs() < 1e-6);
        let b2 = buf.clone();
        let st = render(
            mix_program(&s, &move |_| Some(b2.clone()), 100),
            100,
            0.0,
            2.0,
        );
        assert_eq!(st.num_channels(), 2);
        assert!(st.channels[0][150].abs() > 0.1);
    }

    fn amp(db: f32) -> af2_core::EffectSlot {
        af2_core::EffectSlot {
            position: None,
            sidechain: None,
            effect: "amplify".into(),
            values: vec![db],
            bypass: false,
            data: None,
            ext_id: None,
        }
    }

    #[test]
    fn a_3d_master_chain_runs_on_every_speaker_before_listening_and_export() {
        let (mut s, buf) = session_with_tone();
        s.space = Some(af2_core::space::SpaceLayout::Surround714);
        // Spread wide so every speaker but the LFE gets some.
        s.tracks[0].pos.x = 0.3;
        s.tracks[0].pos.y = 0.2;
        s.tracks[0].pos.size = 1.0;
        let b2 = buf.clone();
        let plain = render_bed(
            mix_program(&s, &move |_| Some(b2.clone()), 100),
            100,
            0.0,
            2.0,
        );
        let b2 = buf.clone();
        let plain_st = render(
            mix_program(&s, &move |_| Some(b2.clone()), 100),
            100,
            0.0,
            2.0,
        );
        s.master_chain.push(amp(-6.0206));
        let b2 = buf.clone();
        let bed = render_bed(
            mix_program(&s, &move |_| Some(b2.clone()), 100),
            100,
            0.0,
            2.0,
        );
        assert_eq!(bed.num_channels(), 12);
        for c in 0..12 {
            let (a, b) = (plain.channels[c][150], bed.channels[c][150]);
            assert!((b - a * 0.5).abs() < 1e-4, "speaker {c}: {a} -> {b}");
        }
        assert!(
            (0..12)
                .filter(|c| plain.channels[*c][150].abs() > 1e-3)
                .count()
                >= 3,
            "the source reaches the room"
        );
        // Heard (folded or binaural) from the processed bed, not run again.
        let b2 = buf.clone();
        let st = render(
            mix_program(&s, &move |_| Some(b2.clone()), 100),
            100,
            0.0,
            2.0,
        );
        for c in 0..2 {
            let (a, b) = (plain_st.channels[c][150], st.channels[c][150]);
            assert!((b - a * 0.5).abs() < 1e-3, "side {c}: {a} -> {b}");
        }
    }

    #[test]
    fn a_3d_master_chain_takes_live_settings_bypass_and_mix() {
        let (mut s, buf) = session_with_tone();
        s.space = Some(af2_core::space::SpaceLayout::Surround51);
        s.tracks[0].pos.x = 0.0;
        s.tracks[0].pos.y = 1.0;
        s.master_chain.push(amp(-6.0206));
        let b2 = buf.clone();
        let program = mix_program(&s, &move |_| Some(b2.clone()), 100);
        let (mut trash, _rx) = rtrb::RingBuffer::<Garbage>::new(4);
        let mut v = Voice {
            program: Some(Box::new(program)),
            audition: None,
            playing: false,
            t: 0.0,
            plan: Plan::default(),
            metro: Default::default(),
            recording: false,
            clicks: None,
            declick: Default::default(),
        };
        let mut centre = |v: &mut Voice| {
            v.apply(
                Cmd::Play {
                    from: 1.2,
                    plan: Plan::new(1.3, Region::None),
                },
                &mut trash,
            );
            let mut out = vec![0.0f32; 6 * 4];
            v.render(&mut out, 6, 100.0, true);
            out[2]
        };
        assert!((centre(&mut v) - 0.5).abs() < 1e-3);
        let (mut trash, _rx) = rtrb::RingBuffer::<Garbage>::new(4);
        let mut vals = [0.0; MAX_VALUES];
        vals[0] = -12.0412;
        v.apply(
            Cmd::Param {
                track: MASTER,
                slot: 0,
                n: 1,
                vals,
            },
            &mut trash,
        );
        assert!((centre(&mut v) - 0.25).abs() < 1e-3);
        v.apply(
            Cmd::ChainMix {
                track: MASTER,
                mix: 0.5,
            },
            &mut trash,
        );
        assert!((centre(&mut v) - 0.625).abs() < 1e-3);
        v.apply(
            Cmd::Bypass {
                track: MASTER,
                slot: 0,
                on: true,
            },
            &mut trash,
        );
        assert!((centre(&mut v) - 1.0).abs() < 1e-3);
    }

    /// A voice playing a one-second ramp at 1000 frames a second, so the
    /// position in frames reads straight off the samples too.
    fn ramp_voice() -> Voice {
        let buf = Arc::new(AudioBuffer {
            rate: 1000,
            channels: vec![(0..1000).map(|i| i as f32 / 1000.0).collect()],
        });
        Voice {
            program: Some(Box::new(buffer_program(buf))),
            playing: false,
            t: 0.0,
            plan: Plan::default(),
            audition: None,
            metro: Default::default(),
            recording: false,
            clicks: None,
            declick: Default::default(),
        }
    }

    /// Starts at frame `from` under `region`, renders `frames` and returns
    /// the frame reached plus the last sample heard (in frames, x1000).
    fn play_frames(region: Region, from: f64, frames: usize) -> (f64, f32) {
        let mut v = ramp_voice();
        let (mut trash, _rx) = rtrb::RingBuffer::<Garbage>::new(4);
        v.apply(
            Cmd::Play {
                from: from / 1000.0,
                plan: Plan::new(f64::INFINITY, region),
            },
            &mut trash,
        );
        let mut out = vec![0.0f32; frames * 2];
        // In uneven callback sizes, as a sound card asks.
        for chunk in out.chunks_mut(2 * 37) {
            v.render(chunk, 2, 1000.0, false);
        }
        (v.t * 1000.0, out[out.len() - 2] * 1000.0)
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn jumps_while_playing_fade_instead_of_clicking() {
        let rate = 48_000.0;
        // A full-scale 440 Hz sine, two seconds long.
        let buf = Arc::new(AudioBuffer {
            rate: rate as u32,
            channels: vec![(0..96_000)
                .map(|i| (i as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin())
                .collect()],
        });
        let mut v = Voice {
            program: Some(Box::new(buffer_program(buf))),
            playing: false,
            t: 0.0,
            plan: Plan::default(),
            audition: None,
            metro: Default::default(),
            recording: false,
            clicks: None,
            declick: Default::default(),
        };
        let (mut trash, _rx) = rtrb::RingBuffer::<Garbage>::new(4);
        let plan = || Plan::new(f64::INFINITY, Region::None);
        v.apply(Cmd::Play { from: 1.0, plan: plan() }, &mut trash);
        let mut out = vec![0.0f32; 2 * 4800];
        v.render(&mut out, 2, rate, false);
        // Backward by half a second, as reverse scrubbing does, at an odd
        // spot in the wave.
        v.apply(Cmd::Retarget { to: Some(0.6123), plan: plan() }, &mut trash);
        let mut out = vec![0.0f32; 2 * 4800];
        v.render(&mut out, 2, rate, false);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        // A 440 Hz sine moves at most 2 pi 440 / 48000 (0.058) a sample;
        // a hard jump could step by up to 2.
        let worst = left.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(worst < 0.07, "step of {worst}");
        // The dip lasts the fade out and back in, about 4 ms each.
        let fade = (DECLICK_SECS * rate) as usize;
        let quiet = left.iter().position(|x| x.abs() < 1e-6 && true).unwrap();
        assert!(quiet.abs_diff(fade) <= 2, "silent at {quiet}");
        let loud_after = left[quiet + fade + 2..].iter().take(200).fold(0.0f32, |a, x| a.max(x.abs()));
        assert!(loud_after > 0.95, "back to full level: {loud_after}");
        // It went where it was sent (plus what has played since).
        let since = (4800 - quiet) as f64 / rate;
        assert!((v.t - (0.6123 + since)).abs() < 2.0 / rate, "{}", v.t);
    }

    #[test]
    fn loop_positions_are_exact() {
        let lp = Region::Loop {
            start: 0.1,
            end: 0.2,
        };
        for (from, frames, want) in [(50.0, 40, 90.0), (50.0, 150, 100.0), (50.0, 275, 125.0)] {
            let (at, _) = play_frames(lp, from, frames);
            assert!(
                near(at, want),
                "start {from}, {frames} frames: at {at}, want {want}"
            );
        }
        // Starting after the loop rolls over into its start.
        let (at, _) = play_frames(lp, 300.0, 10);
        assert!(near(at, 110.0), "{at}");
        // The frame before the seam is the loop's last; the next is its first.
        let (_, last) = play_frames(lp, 50.0, 150);
        assert!((last - 199.0).abs() < 1e-3, "{last}");
        let (_, first) = play_frames(lp, 50.0, 151);
        assert!((first - 100.0).abs() < 1e-3, "{first}");
    }

    #[test]
    fn skip_positions_are_exact() {
        let sk = Region::Skip {
            start: 0.1,
            end: 0.2,
        };
        let (at, heard) = play_frames(sk, 50.0, 60);
        assert!(near(at, 210.0), "{at}");
        assert!((heard - 209.0).abs() < 1e-3, "{heard}");
        // Starting inside the skipped range starts at its end.
        let (at, _) = play_frames(sk, 150.0, 5);
        assert!(near(at, 205.0), "{at}");
    }

    #[test]
    fn retargeting_keeps_playing_from_where_it_was() {
        let mut v = ramp_voice();
        let (mut trash, _rx) = rtrb::RingBuffer::<Garbage>::new(4);
        v.apply(
            Cmd::Play {
                from: 0.05,
                plan: Plan::default(),
            },
            &mut trash,
        );
        let mut out = vec![0.0f32; 2 * 10];
        v.render(&mut out, 2, 1000.0, false);
        // A new loop the playhead is inside: continue with the new bounds.
        v.apply(
            Cmd::Retarget {
                to: None,
                plan: Plan::new(
                    f64::INFINITY,
                    Region::Loop {
                        start: 0.0,
                        end: 0.07,
                    },
                ),
            },
            &mut trash,
        );
        let mut out = vec![0.0f32; 2 * 15];
        v.render(&mut out, 2, 1000.0, false);
        assert!(near(v.t * 1000.0, 5.0), "{}", v.t * 1000.0);
        assert!(v.playing);
    }

    #[test]
    fn midi_tracks_sound() {
        let mut s = Session::new("m", 48_000, 0);
        let t = s.add_track_of(TrackKind::Midi);
        let ti = s.track_index(t).unwrap();
        s.tracks[ti].midi.push(af2_core::midi::MidiClip {
            id: 1,
            name: "c".into(),
            start: 0.0,
            length: 4.0,
            notes: vec![af2_core::midi::MidiNote {
                pitch: 60,
                start: 0.0,
                length: 1.0,
                velocity: 100,
                channel: 0,
                pan: 0.0,
            }],
            timing: Default::default(),
            stretch: 1.0,
            pitch: 0.0,
            muted: false,
            gain_db: 0.0,
            controls: Vec::new(),
            ext_id: None,
        });
        let out = render(mix_program(&s, &|_| None, 48_000), 48_000, 0.0, 1.0);
        assert!(out.peak() > 0.01);
    }

    #[test]
    fn a_stretched_and_shifted_midi_clip_renders_later_and_higher() {
        use af2_core::midi::{MidiClip, MidiNote, Wave};
        // A note at beat 1 of a clip at 0, 120 BPM: 0.5 s; at 200%, 1 s.
        let mut s = Session::new("m", 8_000, 0);
        let t = s.add_track_of(TrackKind::Midi);
        let ti = s.track_index(t).unwrap();
        s.tracks[ti].instrument.wave = Wave::Sine;
        s.tracks[ti].instrument.detune = 0.0;
        s.tracks[ti].instrument.attack = 0.001;
        s.tracks[ti].instrument.release = 0.01;
        let mut c = MidiClip::new(1, "c", 0.0, 4.0);
        c.notes.push(MidiNote {
            pitch: 57,
            start: 1.0,
            length: 0.5,
            velocity: 127,
            channel: 0,
            pan: 0.0,
        });
        c.stretch = 2.0;
        c.pitch = 12.0;
        s.tracks[ti].midi.push(c);
        let rate = 8_000;
        let out = render(mix_program(&s, &|_| None, rate), rate, 0.0, 2.5);
        let ch = &out.channels[0];
        let first = ch.iter().position(|v| v.abs() > 1e-3).unwrap();
        let last = ch.iter().rposition(|v| v.abs() > 1e-3).unwrap();
        assert!((first as f64 / rate as f64 - 1.0).abs() < 0.002, "{first}");
        // Half a beat at 200% is a beat: 0.5 s.
        assert!((last as f64 / rate as f64 - 1.5).abs() < 0.03, "{last}");
        // An octave up from A3: about 440 Hz, by zero crossings.
        let seg = &ch[(1.1 * rate as f64) as usize..(1.4 * rate as f64) as usize];
        let crossings = seg
            .windows(2)
            .filter(|w| (w[0] <= 0.0) != (w[1] <= 0.0))
            .count() as f64;
        let hz = crossings / 2.0 / 0.3;
        assert!((hz - 440.0).abs() < 15.0, "{hz} Hz");
        assert!((s.duration() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn notes_follow_the_tempo_map() {
        use af2_core::midi::{MidiClip, MidiNote, TempoChange};
        // One bar of 4/4 at 120 (2 s), then 60 BPM: beat 5 is at 3 s.
        let mut s = Session::new("m", 48_000, 0);
        s.tempo.changes.push(TempoChange::new(1, 60.0, 4, 4));
        let t = s.add_track_of(TrackKind::Midi);
        let ti = s.track_index(t).unwrap();
        s.tracks[ti].instrument.attack = 0.001;
        s.tracks[ti].instrument.release = 0.01;
        s.tracks[ti].midi.push(MidiClip {
            id: 1,
            name: "c".into(),
            start: 4.0,
            length: 4.0,
            notes: vec![MidiNote {
                pitch: 69,
                start: 1.0,
                length: 0.5,
                velocity: 127,
                channel: 0,
                pan: 0.0,
            }],
            timing: Default::default(),
            stretch: 1.0,
            pitch: 0.0,
            muted: false,
            gain_db: 0.0,
            controls: Vec::new(),
            ext_id: None,
        });
        let rate = 8_000;
        let out = render(mix_program(&s, &|_| None, rate), rate, 0.0, 4.0);
        let ch = &out.channels[0];
        let first = ch.iter().position(|v| v.abs() > 1e-3).unwrap();
        let last = ch.iter().rposition(|v| v.abs() > 1e-3).unwrap();
        // Starts at 3 s, held half a beat (0.5 s at 60 BPM), then a short release.
        assert!((first as f64 / rate as f64 - 3.0).abs() < 0.002, "{first}");
        assert!((last as f64 / rate as f64 - 3.5).abs() < 0.03, "{last}");
        assert!((s.duration() - 6.0).abs() < 1e-9);
    }
}

/// `cargo test --release -p af2-engine bench -- --ignored --nocapture`
#[cfg(test)]
mod bench {
    use super::*;
    use af2_core::session::{Clip, MediaItem, Send};
    use af2_core::{EffectSlot, Session, TrackKind};

    fn slot(id: &str) -> EffectSlot {
        let e = af2_dsp::Effect::from_id(id).unwrap();
        EffectSlot {
            position: None,
            sidechain: None,
            effect: id.into(),
            values: e.defaults(),
            bypass: false,
            data: None,
            ext_id: None,
        }
    }

    #[test]
    #[ignore]
    fn mix_32_tracks_with_effects() {
        run_bench(None);
    }

    /// The same session in 3D Space (7.1.4): the master chain (EQ,
    /// compressor, reverb, limiter) runs on all twelve speakers, then the
    /// bed is folded to stereo.
    #[test]
    #[ignore]
    fn mix_32_tracks_on_a_7_1_4_bed() {
        run_bench(Some(af2_core::space::SpaceLayout::Surround714));
    }

    fn run_bench(space: Option<af2_core::space::SpaceLayout>) {
        let rate = 48_000u32;
        let secs = 60.0;
        let n = (rate as f64 * secs) as usize;
        let buf = Arc::new(AudioBuffer {
            rate,
            channels: vec![(0..n).map(|i| ((i as f32) * 0.01).sin() * 0.1).collect(); 2],
        });
        let mut s = Session::new("bench", rate, 0);
        let m = s.alloc_id();
        s.media.push(MediaItem {
            id: m,
            name: "x".into(),
            path: None,
            duration: secs,
            rate,
            channels: 2,
            ext_id: None,
        });
        let verb_a = s.add_track_of(TrackKind::Bus);
        let verb_b = s.add_track_of(TrackKind::Bus);
        for b in [verb_a, verb_b] {
            let i = s.track_index(b).unwrap();
            s.tracks[i].chain.push(slot("reverb"));
        }
        for k in 0..32 {
            let t = s.add_track();
            let i = s.track_index(t).unwrap();
            s.tracks[i].clips.push(Clip {
                props: Default::default(),
                id: 1000 + k,
                media: m,
                name: "c".into(),
                start: 0.0,
                offset: 0.0,
                length: secs,
                gain_db: 0.0,
                fade_in: 0.1,
                fade_out: 0.1,
                muted: false,
                ext_id: None,
            });
            s.tracks[i].chain.push(slot("parametric-eq"));
            s.tracks[i].chain.push(slot("compressor"));
            s.tracks[i].sends.push(Send {
                bus: if k % 2 == 0 { verb_a } else { verb_b },
                level_db: -12.0,
                pre_fader: false,
                pan: 0.0,
                enabled: true,
                ext_id: None,
            });
        }
        if space.is_some() {
            s.space = space;
            s.master_chain.insert(0, slot("reverb"));
            s.master_chain.insert(0, slot("compressor"));
            s.master_chain.insert(0, slot("parametric-eq"));
        }
        s.master_chain.push(slot("limiter"));
        let b2 = buf.clone();
        let t0 = std::time::Instant::now();
        let out = render(
            mix_program(&s, &move |_| Some(b2.clone()), rate),
            rate,
            0.0,
            secs,
        );
        let took = t0.elapsed().as_secs_f64();
        let what = if space.is_some() {
            "3D 7.1.4: 32 tracks x (EQ + compressor) + 2 reverb buses + master EQ, compressor, reverb, limiter on 12 speakers"
        } else {
            "32 tracks x (EQ + compressor) + 2 reverb buses + master limiter"
        };
        eprintln!(
            "{what}: {secs} s of audio in {took:.2} s = {:.1}x real time on one core",
            secs / took
        );
        assert_eq!(out.frames(), n);
    }
}

#[cfg(test)]
mod headphone_tests {
    use super::*;

    /// A 5.1 file (L R C LFE Ls Rs) with a tone on `live` only, heard on
    /// headphones: (left energy, right energy, left ear signal).
    fn tone_file(live: usize) -> Arc<AudioBuffer> {
        let mut b = AudioBuffer::new(48_000, 6, 24_000);
        for (i, s) in b.channels[live].iter_mut().enumerate() {
            *s = (i as f32 * 2.0 * std::f32::consts::PI * 5_000.0 / 48_000.0).sin() * 0.5;
        }
        Arc::new(b)
    }

    fn heard_in(live: usize, mode: SurroundPlay) -> (f32, f32, Vec<f32>) {
        let out = render(buffer_program_for(tone_file(live), None, mode, 48_000), 48_000, 0.0, 0.5);
        let e = |c: &[f32]| c[4_800..].iter().map(|v| v * v).sum::<f32>();
        (e(&out.channels[0]), e(&out.channels[1]), out.channels[0].clone())
    }

    fn heard(live: usize, headphones: bool) -> (f32, f32, Vec<f32>) {
        let mode = if headphones { SurroundPlay::Headphones } else { SurroundPlay::Fold };
        let out = render(buffer_program_for(tone_file(live), None, mode, 48_000), 48_000, 0.0, 0.5);
        let e = |c: &[f32]| c[4_800..].iter().map(|v| v * v).sum::<f32>();
        (e(&out.channels[0]), e(&out.channels[1]), out.channels[0].clone())
    }

    #[test]
    fn surround_channels_land_around_the_head() {
        let (l, r, _) = heard(0, true);
        assert!(l > 1.5 * r && l > 1.0, "front left on the left: {l} {r}");
        let (l, r, _) = heard(4, true);
        assert!(l > 1.5 * r && l > 1.0, "left surround on the left: {l} {r}");
        let (l, r, _) = heard(5, true);
        assert!(r > 1.5 * l && r > 1.0, "right surround on the right: {l} {r}");
        let (l, r, _) = heard(2, true);
        assert!((l / r - 1.0).abs() < 0.05 && l > 1.0, "centre in the middle: {l} {r}");
        // The surround is not just the front again: its ear signal differs.
        let (_, _, front) = heard(0, true);
        let (_, _, back) = heard(4, true);
        let diff: f32 = front.iter().zip(&back).map(|(a, b)| (a - b).powi(2)).sum();
        assert!(diff > 1.0, "surround sounds different from front: {diff}");
    }

    #[test]
    fn stereo_files_and_speakers_keep_the_plain_fold() {
        assert!(HeadRender::new(2, None, 48_000, SurroundPlay::Headphones).is_none());
        assert!(HeadRender::new(1, None, 48_000, SurroundPlay::Headphones).is_none());
        assert!(HeadRender::new(6, None, 48_000, SurroundPlay::Fold).is_none());
        let (l, r, _) = heard(4, false);
        assert!(l > 1.0 && r == 0.0, "speakers: left surround folded to the left: {l} {r}");
    }

    #[test]
    fn layouts_follow_the_channel_count() {
        for n in [6usize, 8, 10, 12] {
            let h = HeadRender::new(n, None, 48_000, SurroundPlay::Headphones).unwrap();
            assert_eq!(h.map.len(), n);
            assert!(h.speakers >= n.min(12) - 0, "{n} channels on {} speakers", h.speakers);
        }
    }

    #[test]
    fn speakers_mode_cancels_crosstalk_and_stays_finite() {
        let (hl, hr, head) = heard_in(0, SurroundPlay::Headphones);
        let (sl, sr, spk) = heard_in(0, SurroundPlay::Speakers);
        assert!(spk.iter().all(|v| v.is_finite()), "finite");
        let diff: f32 = head.iter().zip(&spk).map(|(a, b)| (a - b).powi(2)).sum();
        assert!(diff > 0.1, "speakers differ from headphones: {diff}");
        // Front left still on the left; the far speaker carries the
        // anti-phase cancelling signal, but nothing runs away.
        assert!(sl > sr && sl > 1.0, "front left on the left: {sl} {sr}");
        assert!(sl + sr < 4.0 * (hl + hr), "bounded: {} vs {}", sl + sr, hl + hr);
    }

    #[test]
    fn direct_mode_plays_the_bed_or_folds() {
        let p = buffer_program_for(tone_file(4), None, SurroundPlay::Direct, 48_000);
        assert_eq!(render_channels(&p, true), 6);
        assert_eq!(render_channels(&p, false), 2);
        let bed = render_bed(p, 48_000, 0.0, 0.25);
        assert_eq!(bed.num_channels(), 6);
        let e: Vec<f32> = bed.channels.iter().map(|c| c.iter().map(|v| v * v).sum()).collect();
        let loud = e.iter().cloned().fold(0.0, f32::max);
        assert!(loud > 1.0 && e.iter().filter(|v| **v > 0.01).count() == 1, "one speaker: {e:?}");
        // Without speaker outputs: the plain fold (left surround on the left).
        let (l, r, _) = heard_in(4, SurroundPlay::Direct);
        assert!(l > 1.0 && r == 0.0, "folded: {l} {r}");
    }

    #[test]
    fn six_channels_are_five_one() {
        assert_eq!(file_layout(6, None), af2_space::SpaceLayout::Surround51);
    }
}

#[cfg(test)]
mod surround_level_tests {
    use super::*;

    #[test]
    fn every_mode_plays_at_the_folds_loudness() {
        // Decorrelated noise on every channel of a 5.1 file.
        let mut b = AudioBuffer::new(48_000, 6, 96_000);
        let mut x: u32 = 12345;
        for c in b.channels.iter_mut() {
            for s in c.iter_mut() {
                x ^= x << 13; x ^= x >> 17; x ^= x << 5;
                *s = (x as f32 / u32::MAX as f32 - 0.5) * 0.4;
            }
        }
        let b = Arc::new(b);
        let rms = |c: &[f32]| (c[9_600..].iter().map(|v| v * v).sum::<f32>() / (c.len() - 9_600) as f32).sqrt();
        let level = |mode| {
            let out = render(buffer_program_for(b.clone(), None, mode, 48_000), 48_000, 0.0, 2.0);
            (rms(&out.channels[0]) + rms(&out.channels[1])) / 2.0
        };
        let fold = level(SurroundPlay::Fold);
        for mode in [SurroundPlay::Headphones, SurroundPlay::Speakers] {
            let db = 20.0 * (level(mode) / fold).log10();
            assert!(db.abs() < 1.0, "{mode:?} is {db:.1} dB from the fold");
        }
    }
}
