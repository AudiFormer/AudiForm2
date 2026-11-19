// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The mix graph, run in blocks of up to [`BLOCK`] frames.
//!
//! Per block: buses are cleared; every audio and MIDI track renders its
//! clips (or notes), runs its effect chain, feeds pre-fader sends, takes
//! its fader and pan, feeds post-fader sends, then goes to its output (a
//! bus or the master). Buses then run in session order. A 3D Space
//! session spreads each track over the speaker bed instead of panning;
//! its master chain then runs on the bed's speakers (every channel, see
//! `af27_dsp::wide`), before the bed is heard binaurally or folded to
//! stereo, or exported as it is. Bus tracks stay stereo. In a stereo
//! session the master chain runs on the stereo mix. The master fader
//! comes last, plus the metronome.
//!
//! Everything a block needs is allocated when the program is built, on
//! the UI thread. The audio thread only reads and writes those buffers.

use std::sync::Arc;

use af27_core::time::db_to_gain;
use af27_core::{AudioBuffer, Session, TrackKind};
use af27_dsp::live::{build_chain, Chain};
use af27_dsp::wide::{build_wide_chain, WideChain};

use crate::synth::{self, Synth};

pub const BLOCK: usize = 256;
/// Speaker channels at most (7.1.4 is 12).
pub const MAX_SPEAKERS: usize = 16;

#[derive(Clone)]
pub struct MixClip {
    pub buf: Arc<AudioBuffer>,
    pub start: f64,
    pub offset: f64,
    pub length: f64,
    pub gain: f32,
    pub fade_in: f64,
    pub fade_out: f64,
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
    pub chain: Chain,
    /// Index of the bus track it feeds, or None for the master.
    pub output: Option<usize>,
    pub sends: Vec<MixSend>,
    /// 3D: gains per speaker for the left and right side, and the LFE send.
    pub space_l: [f32; MAX_SPEAKERS],
    pub space_r: [f32; MAX_SPEAKERS],
    pub lfe: f32,
    l: Vec<f32>,
    r: Vec<f32>,
}

pub struct SpaceMix {
    pub channels: usize,
    pub lfe_channel: Option<usize>,
    pub monitor: af27_space::Monitoring,
    /// The master chain, on the speakers.
    pub chain: WideChain,
    bed: Vec<f32>,
}

pub struct Mix {
    pub tracks: Vec<MixTrack>,
    /// The master chain of a stereo session (empty in a 3D session, whose
    /// master chain is `space.chain`).
    pub master_chain: Chain,
    pub master: f32,
    pub duration: f64,
    pub space: Option<SpaceMix>,
    pub metronome: Option<(f64, u32)>,
    ml: Vec<f32>,
    mr: Vec<f32>,
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

/// Per-speaker gains for a track's left and right sides in a 3D session.
pub fn space_gains(
    layout: af27_core::space::SpaceLayout,
    pos: &af27_core::space::SpacePos,
    stereo: bool,
) -> ([f32; MAX_SPEAKERS], [f32; MAX_SPEAKERS], f32) {
    let [gl, gr] = af27_space::source_gains(layout, pos, stereo);
    let mut l = [0.0; MAX_SPEAKERS];
    let mut r = [0.0; MAX_SPEAKERS];
    for (i, v) in gl.iter().take(MAX_SPEAKERS).enumerate() {
        l[i] = *v;
    }
    for (i, v) in gr.iter().take(MAX_SPEAKERS).enumerate() {
        r[i] = *v;
    }
    let lfe = if pos.lfe_db <= -60.0 {
        0.0
    } else {
        db_to_gain(pos.lfe_db as f32)
    };
    (l, r, lfe)
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
    let index_of = |id: af27_core::TrackId| session.tracks.iter().position(|t| t.id == id);
    let tracks: Vec<MixTrack> = session
        .tracks
        .iter()
        .map(|t| {
            let clips: Vec<MixClip> = t
                .clips
                .iter()
                .filter(|c| !c.muted)
                .filter_map(|c| {
                    Some(MixClip {
                        buf: media(c.media)?,
                        start: c.start,
                        offset: c.offset,
                        length: c.length,
                        gain: db_to_gain(c.gain_db),
                        fade_in: c.fade_in,
                        fade_out: c.fade_out,
                    })
                })
                .collect();
            let synth = (t.kind == TrackKind::Midi).then(|| {
                let notes = t
                    .midi
                    .iter()
                    .filter(|c| !c.muted)
                    .flat_map(|c| {
                        c.notes
                            .iter()
                            .filter(move |n| n.start < c.length)
                            .map(move |n| {
                                let s = session.tempo.secs(c.start + n.start);
                                let e = session
                                    .tempo
                                    .secs(c.start + (n.start + n.length).min(c.length));
                                synth::note(s, e, n.pitch, n.velocity, n.pan)
                            })
                    })
                    .collect();
                Synth::new(t.instrument, notes, rate)
            });
            let stereo =
                t.kind != TrackKind::Audio || clips.iter().any(|c| c.buf.num_channels() >= 2);
            let (space_l, space_r, lfe) = match space {
                Some(layout) => space_gains(layout, &t.pos, stereo),
                None => ([0.0; MAX_SPEAKERS], [0.0; MAX_SPEAKERS], 0.0),
            };
            let (gain_l, gain_r) = track_gains(t, space.is_some());
            MixTrack {
                is_bus: t.kind == TrackKind::Bus,
                audible: session.audible(t),
                gain_l,
                gain_r,
                clips,
                synth,
                chain: build_chain(&t.chain, rate, BLOCK),
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
                space_l,
                space_r,
                lfe,
                l: vec![0.0; BLOCK],
                r: vec![0.0; BLOCK],
            }
        })
        .collect();
    let space = space.map(|layout| {
        let speakers = af27_space::speakers(layout);
        let channels = speakers.len();
        let names: Vec<String> = speakers.iter().map(|s| s.name.to_string()).collect();
        SpaceMix {
            channels,
            lfe_channel: af27_space::lfe_channel(layout),
            monitor: af27_space::Monitoring::new(layout, session.monitor, rate),
            chain: build_wide_chain(&session.master_chain, rate, &names, BLOCK),
            bed: vec![0.0; BLOCK * channels],
        }
    });
    let master_slots: &[af27_core::EffectSlot] = if space.is_some() {
        &[]
    } else {
        &session.master_chain
    };
    Mix {
        tracks,
        master_chain: build_chain(master_slots, rate, BLOCK),
        master: db_to_gain(session.master_db),
        duration: session.duration(),
        space,
        metronome: session
            .tempo
            .metronome
            .then_some((session.tempo.bpm, session.tempo.beats_per_bar)),
        ml: vec![0.0; BLOCK],
        mr: vec![0.0; BLOCK],
    }
}

impl Mix {
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
        }
        self.master_chain.reset();
        if let Some(sp) = &mut self.space {
            sp.chain.reset();
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

    pub fn set_master_mix(&mut self, mix: f32) {
        match &mut self.space {
            Some(sp) => sp.chain.mix = mix,
            None => self.master_chain.mix = mix,
        }
    }

    fn route(&mut self, ti: usize, n: usize) {
        let (out, l, r) = {
            let t = &self.tracks[ti];
            let mut l = [0.0f32; BLOCK];
            let mut r = [0.0f32; BLOCK];
            l[..n].copy_from_slice(&t.l[..n]);
            r[..n].copy_from_slice(&t.r[..n]);
            (t.output, l, r)
        };
        match out {
            Some(bus) if bus > ti || !self.tracks[ti].is_bus => {
                let b = &mut self.tracks[bus];
                for i in 0..n {
                    b.l[i] += l[i];
                    b.r[i] += r[i];
                }
            }
            _ => match &mut self.space {
                Some(sp) => {
                    let t = &self.tracks[ti];
                    let ch = sp.channels;
                    for i in 0..n {
                        let frame = &mut sp.bed[i * ch..(i + 1) * ch];
                        for c in 0..ch.min(MAX_SPEAKERS) {
                            frame[c] += l[i] * t.space_l[c] + r[i] * t.space_r[c];
                        }
                        if let Some(lc) = sp.lfe_channel {
                            frame[lc] += (l[i] + r[i]) * 0.5 * t.lfe;
                        }
                    }
                }
                None => {
                    for i in 0..n {
                        self.ml[i] += l[i];
                        self.mr[i] += r[i];
                    }
                }
            },
        }
    }

    fn sends(&mut self, ti: usize, n: usize, pre: bool) {
        for s in 0..self.tracks[ti].sends.len() {
            let (bus, g, is_pre) = {
                let x = &self.tracks[ti].sends[s];
                (x.bus, x.gain, x.pre)
            };
            if is_pre != pre || bus == ti {
                continue;
            }
            let mut l = [0.0f32; BLOCK];
            let mut r = [0.0f32; BLOCK];
            l[..n].copy_from_slice(&self.tracks[ti].l[..n]);
            r[..n].copy_from_slice(&self.tracks[ti].r[..n]);
            let b = &mut self.tracks[bus];
            for i in 0..n {
                b.l[i] += l[i] * g;
                b.r[i] += r[i] * g;
            }
        }
    }

    fn finish_track(&mut self, ti: usize, n: usize) {
        {
            let t = &mut self.tracks[ti];
            let (l, r) = (&mut t.l[..n], &mut t.r[..n]);
            t.chain.process(l, r);
        }
        self.sends(ti, n, true);
        {
            let t = &mut self.tracks[ti];
            for i in 0..n {
                t.l[i] *= t.gain_l;
                t.r[i] *= t.gain_r;
            }
        }
        self.sends(ti, n, false);
        self.route(ti, n);
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
        self.ml[..n].fill(0.0);
        self.mr[..n].fill(0.0);
        if let Some(sp) = &mut self.space {
            sp.bed[..n * sp.channels].fill(0.0);
        }
        for t in &mut self.tracks {
            if t.is_bus {
                t.l[..n].fill(0.0);
                t.r[..n].fill(0.0);
            }
        }
        for ti in 0..self.tracks.len() {
            if self.tracks[ti].is_bus {
                continue;
            }
            let t = &mut self.tracks[ti];
            t.l[..n].fill(0.0);
            t.r[..n].fill(0.0);
            if !t.audible {
                continue;
            }
            let t1 = t0 + dt * n as f64;
            let (clips, tl, tr) = (&t.clips, &mut t.l, &mut t.r);
            for c in clips {
                if c.start >= t1 || c.start + c.length <= t0 {
                    continue;
                }
                let rate = c.buf.rate as f64;
                for i in 0..n {
                    let t = t0 + dt * i as f64;
                    let local = t - c.start;
                    if local < 0.0 || local >= c.length {
                        continue;
                    }
                    let mut g = c.gain as f64;
                    if c.fade_in > 0.0 && local < c.fade_in {
                        g *= local / c.fade_in;
                    }
                    if c.fade_out > 0.0 && c.length - local < c.fade_out {
                        g *= (c.length - local) / c.fade_out;
                    }
                    let (a, b) = stereo_frame(&c.buf, (c.offset + local) * rate);
                    tl[i] += a * g as f32;
                    tr[i] += b * g as f32;
                }
            }
            let t = &mut self.tracks[ti];
            if let Some(s) = &mut t.synth {
                let (l, r) = (&mut t.l[..n], &mut t.r[..n]);
                s.render(t0, dt, l, r);
            }
            self.finish_track(ti, n);
        }
        for ti in 0..self.tracks.len() {
            if self.tracks[ti].is_bus && self.tracks[ti].audible {
                self.finish_track(ti, n);
            }
        }
        if let Some(sp) = &mut self.space {
            // The master chain on the speakers, before listening or export.
            sp.chain.process(&mut sp.bed[..n * sp.channels]);
            if bed_out {
                let ch = sp.channels;
                let mut peak = (0.0f32, 0.0f32);
                for i in 0..n * ch {
                    out[i] = sp.bed[i] * self.master;
                    peak.0 = peak.0.max(out[i].abs());
                }
                peak.1 = peak.0;
                return peak;
            }
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
            let mut l = self.ml[i] * self.master;
            let mut r = self.mr[i] * self.master;
            if let Some((bpm, bpb)) = self.metronome {
                let c = synth::click(t0 + dt * i as f64, bpm, bpb);
                l += c;
                r += c;
            }
            pl = pl.max(l.abs());
            pr = pr.max(r.abs());
            out[i * 2] = l;
            out[i * 2 + 1] = r;
        }
        (pl, pr)
    }
}
