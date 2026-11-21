// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The built-in synth's full engine, used once any of the newer controls
//! is set (the untouched instrument keeps the original engine in
//! `synth.rs`, bit for bit).
//!
//! Voices: each note (or the one voice in Mono and Legato) has its own
//! oscillators (two, each stacked up to five times for unison, with
//! sync, FM, a sub and noise), its own filter (the original 2-pole
//! state-variable filter or a driven 4-pole ladder, low-, high- or
//! band-pass) and its envelopes. One LFO and the pitch bend and mod
//! wheel are shared, as on most classic polysynths. A chorus follows.
//!
//! Envelopes are worked out from each voice's own times, so a jump
//! starts the notes sounding there at the right level. Filters are tuned
//! every 16 samples. Everything is allocated up front: nothing allocates
//! while playing.

use af27_core::midi::{Instrument, Wave};
use af27_core::synth_model::{
    FilterKind, FilterType, LfoShape, SynthControls, VoiceMode, MAX_UNISON, MAX_VOICES,
};
use std::f32::consts::{PI, TAU};

use crate::synth::{envelope, Note};

const MAXV: usize = MAX_VOICES as usize;
const MAXU: usize = MAX_UNISON as usize;
/// Samples between filter tunings and voice checks.
const CTRL: usize = 16;
/// Longest chorus delay, seconds.
const CHORUS_MAX: f32 = 0.03;

/// A held key in Mono and Legato (for going back to it when a later key
/// comes up).
#[derive(Debug, Clone, Copy)]
struct Held {
    id: u32,
    note: Note,
}

#[derive(Debug, Clone, Copy, Default)]
struct FState {
    s: [f32; 4],
}

#[derive(Debug, Clone, Copy, Default)]
struct Coefs {
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    /// Ladder: one stage's gain g/(1+g), feedback and make-up gain.
    lg: f32,
    lk: f32,
    comp: f32,
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    on: bool,
    id: u32,
    note: Note,
    /// When the envelopes started, and when the key came up.
    start: f64,
    end: f64,
    /// Pitch now (log2 Hz) and where it glides to.
    log_f: f32,
    target: f32,
    accent: bool,
    seq: u64,
    ph1: [f32; MAXU],
    ph2: [f32; MAXU],
    phm: [f32; MAXU],
    sub: f32,
    noise: u32,
    flt: [FState; 2],
    co: Coefs,
}

impl Voice {
    fn new() -> Voice {
        Voice {
            on: false,
            id: 0,
            note: crate::synth::note(0.0, 0.0, 60, 100, 0.0),
            start: 0.0,
            end: 0.0,
            log_f: 8.0,
            target: 8.0,
            accent: false,
            seq: 0,
            ph1: [0.0; MAXU],
            ph2: [0.0; MAXU],
            phm: [0.0; MAXU],
            sub: 0.0,
            noise: 1,
            flt: [FState::default(); 2],
            co: Coefs::default(),
        }
    }

    /// Fresh oscillators and filter (a new note on a free voice).
    fn clear(&mut self, n: usize) {
        for u in 0..MAXU {
            // Unison copies start spread round the cycle so they do not
            // all add up on the first wave; a single one starts at zero.
            let p = if n > 1 {
                (u as f32 * 0.381_966).fract()
            } else {
                0.0
            };
            self.ph1[u] = p;
            self.ph2[u] = p;
            self.phm[u] = 0.0;
        }
        self.sub = 0.0;
        self.flt = [FState::default(); 2];
    }
}

/// Values worked out from the instrument when it is set.
#[derive(Debug, Clone, Copy, Default)]
struct Derived {
    r1: f32,
    r2: f32,
    uni: [f32; MAXU],
    pan_l: [f32; MAXU],
    pan_r: [f32; MAXU],
    uni_n: usize,
    uni_gain: f32,
    stereo: bool,
    glide_k: f32,
    release: f64,
    voices: usize,
    osc2_same: bool,
    wave2: Wave,
}

pub(crate) struct Engine {
    inst: Instrument,
    x: SynthControls,
    d: Derived,
    rate: f32,
    dt: f32,
    voices: [Voice; MAXV],
    held: [Option<Held>; MAXV],
    seq: u64,
    /// The pitch the last note played (log2 Hz), to glide from.
    last: Option<f32>,
    chorus: Chorus,
}

impl Engine {
    pub(crate) fn new(inst: Instrument, rate: f32) -> Engine {
        let mut e = Engine {
            inst,
            x: inst.ext,
            d: Derived::default(),
            rate,
            dt: 1.0 / rate,
            voices: [Voice::new(); MAXV],
            held: [None; MAXV],
            seq: 0,
            last: None,
            chorus: Chorus::new(rate),
        };
        e.set(inst);
        e
    }

    pub(crate) fn set(&mut self, inst: Instrument) {
        self.inst = inst;
        let x = inst.ext;
        self.x = x;
        let semis =
            |o: i8, s: i8, c: f32| 2f32.powf((o as f32 * 12.0 + s as f32 + c / 100.0) / 12.0);
        let n = x.unison.clamp(1, MAX_UNISON) as usize;
        let mut d = Derived {
            r1: semis(x.osc1_octave, x.osc1_semi, x.osc1_fine),
            r2: semis(x.osc2_octave, x.osc2_semi, inst.detune),
            uni_n: n,
            uni_gain: 1.0 / (n as f32).sqrt(),
            stereo: n > 1 && x.width > 0.0,
            glide_k: if x.glide > 0.0005 {
                1.0 - (-3.0 / (x.glide * self.rate)).exp()
            } else {
                1.0
            },
            release: inst.release.max(0.001) as f64,
            voices: match x.mode {
                VoiceMode::Poly => x.voices.clamp(1, MAX_VOICES) as usize,
                _ => 1,
            },
            osc2_same: false,
            wave2: x.osc2_wave.unwrap_or(inst.wave),
            ..Derived::default()
        };
        d.osc2_same = (d.r2 - d.r1).abs() < 1e-7 && d.wave2 == inst.wave && !x.sync;
        for u in 0..MAXU {
            let f = if n > 1 {
                u as f32 / (n - 1) as f32 - 0.5
            } else {
                0.0
            };
            d.uni[u] = 2f32.powf(x.spread * f / 1200.0);
            let p = (f * 2.0 * x.width.clamp(0.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
            d.pan_l[u] = p.cos() * std::f32::consts::SQRT_2;
            d.pan_r[u] = p.sin() * std::f32::consts::SQRT_2;
        }
        self.d = d;
        // Voices beyond a smaller count let go.
        for v in self.voices[d.voices..].iter_mut() {
            v.on = false;
        }
    }

    pub(crate) fn clear(&mut self) {
        for v in &mut self.voices {
            v.on = false;
        }
        self.held = [None; MAXV];
        self.last = None;
        self.chorus.clear();
    }

    /// Starts note `n` (its key `id`) at time `t`, its envelopes from
    /// `from` (earlier than `t` after a jump).
    pub(crate) fn note_on(&mut self, t: f64, from: f64, id: u32, n: Note) {
        let target = n.freq.max(1.0).log2();
        let accent = self.x.accent > 0.0 && n.vel >= 0.787;
        self.seq += 1;
        let seq = self.seq;
        let uni = self.d.uni_n;
        let glide = self.x.glide > 0.0005;
        match self.x.mode {
            VoiceMode::Poly => {
                let lim = self.d.voices;
                let pick = self.voices[..lim]
                    .iter()
                    .position(|v| !v.on)
                    .unwrap_or_else(|| {
                        // Steal: the oldest let go, else the oldest.
                        let mut best = 0;
                        let mut key = (true, u64::MAX);
                        for (i, v) in self.voices[..lim].iter().enumerate() {
                            let k = (t < v.end, v.seq);
                            if k < key {
                                key = k;
                                best = i;
                            }
                        }
                        best
                    });
                let from_f = match self.last {
                    Some(l) if glide => l,
                    _ => target,
                };
                let v = &mut self.voices[pick];
                v.clear(uni);
                v.on = true;
                v.id = id;
                v.note = n;
                v.start = from;
                v.end = n.end;
                v.log_f = from_f;
                v.target = target;
                v.accent = accent;
                v.seq = seq;
                v.noise = (seq as u32).wrapping_mul(0x9E37_79B9) | 1;
            }
            mode => {
                // Remember the key (the oldest goes when all are taken).
                if let Some(slot) = self.held.iter_mut().find(|h| h.is_none()) {
                    *slot = Some(Held { id, note: n });
                } else {
                    self.held.rotate_left(1);
                    self.held[MAXV - 1] = Some(Held { id, note: n });
                }
                let last = self.last;
                let v = &mut self.voices[0];
                let overlap = v.on && t < v.end;
                if !v.on {
                    v.clear(uni);
                    v.start = from;
                    v.log_f = match (mode, last) {
                        (VoiceMode::Mono, Some(l)) if glide => l,
                        _ => target,
                    };
                    v.noise = (seq as u32).wrapping_mul(0x9E37_79B9) | 1;
                } else if mode == VoiceMode::Mono || !overlap {
                    // Envelopes start again; the oscillators and filter
                    // run on.
                    v.start = from;
                    if mode == VoiceMode::Legato || !glide {
                        v.log_f = target;
                    }
                } else if !glide {
                    // Legato without glide: straight to the new pitch.
                    v.log_f = target;
                }
                v.on = true;
                v.id = id;
                v.note = n;
                v.end = n.end;
                v.target = target;
                v.accent = accent;
                v.seq = seq;
            }
        }
        self.last = Some(target);
    }

    /// Key `id` came up at `t` (live notes; a clip's notes know their ends).
    pub(crate) fn note_off(&mut self, t: f64, id: u32) {
        for v in self
            .voices
            .iter_mut()
            .filter(|v| v.on && v.id == id && v.end > t)
        {
            v.end = t.max(v.start);
        }
        for h in self.held.iter_mut().flatten() {
            if h.id == id && h.note.end > t {
                h.note.end = t;
            }
        }
    }

    pub(crate) fn all_off(&mut self, t: f64) {
        for v in self.voices.iter_mut().filter(|v| v.on && v.end > t) {
            v.end = t.max(v.start);
        }
        self.held = [None; MAXV];
    }

    /// Notes still held down (live), for handing over to the original
    /// engine: (key, note with its start where its envelope started).
    pub(crate) fn held_notes(&self, t: f64) -> impl Iterator<Item = (u32, Note)> + '_ {
        self.voices
            .iter()
            .filter(move |v| v.on && v.end > t)
            .map(|v| {
                let mut n = v.note;
                n.start = v.start;
                (v.id, n)
            })
    }

    /// Every 16 samples: Mono goes back to a key still held, voices that
    /// have died away free up, filters are tuned.
    fn control(&mut self, t: f64, lfo: f32) {
        if self.x.mode != VoiceMode::Poly {
            for h in self.held.iter_mut() {
                if h.is_some_and(|x| x.note.end <= t) {
                    *h = None;
                }
            }
            let v = &mut self.voices[0];
            if v.on && t >= v.end {
                if let Some(h) = self.held.iter().rev().flatten().next() {
                    v.id = h.id;
                    v.note = h.note;
                    v.end = h.note.end;
                    v.target = h.note.freq.max(1.0).log2();
                    if self.x.glide <= 0.0005 {
                        v.log_f = v.target;
                    }
                }
            }
        }
        let x = self.x;
        let inst = self.inst;
        let rate = self.rate;
        let rel = self.d.release;
        for v in self.voices.iter_mut().filter(|v| v.on) {
            let tl = t - v.start;
            if tl > (v.end - v.start).max(0.0) + rel + 0.002 {
                v.on = false;
                continue;
            }
            let held = (v.end - v.start).max(0.0) as f32;
            let fe = adsr(
                x.f_attack,
                x.f_decay,
                x.f_sustain,
                x.f_release,
                tl as f32,
                held,
            );
            let key = 12.0 * (v.log_f - 8.031_384); // semitones from middle C
            let mut oct = x.filter_env * fe
                + x.key_track * key / 12.0
                + x.lfo_filter * lfo
                + x.vel_filter * v.note.vel;
            if v.accent {
                oct += x.accent * (1.0 + 2.5 * fe);
            }
            let cut = (inst.cutoff * 2f32.powf(oct)).clamp(20.0, rate * 0.45);
            v.co = tune(cut, inst.resonance, x.filter_kind, rate);
        }
    }

    /// Adds `n` samples from `t0` into `l` and `r`: `pf` the shared pitch
    /// factor (bend, vibrato) and `lfo` the shared LFO (with its depth),
    /// per sample. Clip notes from `notes[*at]` on start as their times
    /// come (none live).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render(
        &mut self,
        t0: f64,
        dt: f64,
        l: &mut [f32],
        r: &mut [f32],
        pf: &[f32],
        lfo: &[f32],
        notes: &[Note],
        at: &mut usize,
    ) {
        let n = l.len();
        let mut i = 0;
        while i < n {
            let t = t0 + dt * i as f64;
            // Notes starting by this sample.
            while let Some(x) = notes.get(*at).copied().filter(|x| x.start < t + dt * 0.5) {
                self.note_on(t, x.start, *at as u32, x);
                *at += 1;
            }
            self.control(t, lfo[i]);
            let mut m = CTRL.min(n - i);
            // Stop short of the next note's start.
            if let Some(x) = notes.get(*at) {
                let k = ((x.start - t) / dt - 0.5).ceil().max(1.0);
                if k < m as f64 {
                    m = k as usize;
                }
            }
            for v in 0..MAXV {
                if self.voices[v].on {
                    self.voice(
                        v,
                        t,
                        dt,
                        &mut l[i..i + m],
                        &mut r[i..i + m],
                        &pf[i..i + m],
                        &lfo[i..i + m],
                    );
                }
            }
            i += m;
        }
        if self.x.chorus > 0.0 {
            self.chorus.run(l, r, &self.x, self.dt);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn voice(
        &mut self,
        k: usize,
        t: f64,
        dt: f64,
        l: &mut [f32],
        r: &mut [f32],
        pf: &[f32],
        lfo: &[f32],
    ) {
        let x = self.x;
        let inst = self.inst;
        let d = self.d;
        let dtf = dt as f32;
        let v = &mut self.voices[k];
        let held = (v.end - v.start).max(0.0) as f32;
        let vs = x.velocity.clamp(0.0, 1.0);
        let mut gain = ((1.0 - vs) + vs * v.note.vel * v.note.vel) * 0.5;
        if v.accent {
            gain *= 1.0 + 0.6 * x.accent;
        }
        let fm_on = x.fm > 0.0;
        let fm_vel = (1.0 - vs) + vs * v.note.vel;
        let sync_env = x.osc2_env != 0.0;
        let (w1, w2) = (inst.wave, d.wave2);
        let (l1, l2) = (x.osc1_level, x.osc2_level);
        for i in 0..l.len() {
            let tl = (t - v.start + dt * i as f64) as f32;
            let env = envelope(&inst, tl, held);
            if env <= 0.0 && tl > held {
                continue;
            }
            v.log_f += (v.target - v.log_f) * d.glide_k;
            let f = v.log_f.exp2() * pf[i];
            let pw = (x.pw + x.lfo_pwm * lfo[i]).clamp(0.05, 0.95);
            let r2 = if sync_env {
                let fe = adsr(x.f_attack, x.f_decay, x.f_sustain, x.f_release, tl, held);
                d.r2 * (x.osc2_env * fe / 12.0).exp2()
            } else {
                d.r2
            };
            let fm_i = if fm_on {
                x.fm * fm_vel * (-tl / x.fm_decay.max(0.01)).exp() / TAU
            } else {
                0.0
            };
            let (mut sl, mut sr) = (0.0f32, 0.0f32);
            for u in 0..d.uni_n {
                let fu = f * d.uni[u];
                let inc1 = (fu * d.r1 * dtf).min(0.49);
                let mut p1 = v.ph1[u];
                if fm_on {
                    let incm = (fu * d.r1 * x.fm_ratio * dtf).min(0.49);
                    let m = (TAU * v.phm[u]).sin() * fm_i;
                    p1 = (p1 + m).rem_euclid(1.0);
                    v.phm[u] = wrap(v.phm[u] + incm);
                }
                let o1 = wave(w1, p1, inc1, pw);
                let o2 = if d.osc2_same {
                    o1
                } else {
                    let inc2 = (fu * r2 * dtf).min(0.49);
                    let o = wave(w2, v.ph2[u], inc2, pw);
                    let mut p2 = v.ph2[u] + inc2;
                    if x.sync && v.ph1[u] + inc1 >= 1.0 {
                        // Restart with oscillator 1's new cycle.
                        let frac = (v.ph1[u] + inc1 - 1.0) / inc1.max(1e-9);
                        p2 = frac * inc2;
                    }
                    v.ph2[u] = wrap(p2);
                    o
                };
                v.ph1[u] = wrap(v.ph1[u] + inc1);
                let mut s = (l1 * o1 + l2 * o2) * 0.5;
                if u == 0 {
                    if x.sub > 0.0 {
                        let incs = (f * d.r1 * 0.5 * dtf).min(0.49);
                        s += x.sub * 0.5 * wave(Wave::Square, v.sub, incs, 0.5);
                        v.sub = wrap(v.sub + incs);
                    }
                    if x.noise > 0.0 {
                        v.noise ^= v.noise << 13;
                        v.noise ^= v.noise >> 17;
                        v.noise ^= v.noise << 5;
                        s += x.noise * 0.5 * (v.noise as f32 / u32::MAX as f32 * 2.0 - 1.0);
                    }
                }
                if d.stereo {
                    sl += s * d.pan_l[u];
                    sr += s * d.pan_r[u];
                } else {
                    sl += s;
                }
            }
            let g = env * gain * d.uni_gain;
            if d.stereo {
                let a = filt(
                    &v.co,
                    &mut v.flt[0],
                    sl,
                    x.filter_type,
                    x.filter_kind,
                    x.drive,
                );
                let b = filt(
                    &v.co,
                    &mut v.flt[1],
                    sr,
                    x.filter_type,
                    x.filter_kind,
                    x.drive,
                );
                l[i] += a * g * v.note.pl;
                r[i] += b * g * v.note.pr;
            } else {
                let a = filt(
                    &v.co,
                    &mut v.flt[0],
                    sl,
                    x.filter_type,
                    x.filter_kind,
                    x.drive,
                ) * g;
                l[i] += a * v.note.pl;
                r[i] += a * v.note.pr;
            }
        }
    }
}

#[inline]
fn wrap(p: f32) -> f32 {
    if p >= 1.0 {
        p - 1.0
    } else {
        p
    }
}

/// ADSR level `t` seconds into a note held `held` seconds (linear, the
/// release from wherever it was).
pub(crate) fn adsr(a: f32, d: f32, s: f32, r: f32, t: f32, held: f32) -> f32 {
    let lvl = |t: f32| -> f32 {
        let a = a.max(0.0005);
        let d = d.max(0.001);
        if t < a {
            t / a
        } else if t < a + d {
            1.0 - (1.0 - s) * (t - a) / d
        } else {
            s
        }
    };
    if t <= held {
        lvl(t)
    } else {
        let x = (t - held) / r.max(0.001);
        if x >= 1.0 {
            0.0
        } else {
            lvl(held) * (1.0 - x)
        }
    }
}

fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

/// One band-limited oscillator sample at phase `ph` (0..1).
#[inline]
pub(crate) fn wave(w: Wave, ph: f32, inc: f32, pw: f32) -> f32 {
    match w {
        Wave::Sine => (TAU * ph).sin(),
        Wave::Saw => 2.0 * ph - 1.0 - poly_blep(ph, inc),
        Wave::Square => {
            let mut v = if ph < 0.5 { 1.0 } else { -1.0 };
            v += poly_blep(ph, inc);
            v -= poly_blep((ph + 0.5).fract(), inc);
            v
        }
        Wave::Pulse => {
            let mut v = if ph < pw { 1.0 } else { -1.0 };
            v += poly_blep(ph, inc);
            v -= poly_blep((ph + 1.0 - pw).fract(), inc);
            // Without the offset a narrow pulse carries.
            v - (2.0 * pw - 1.0)
        }
        Wave::Triangle => 1.0 - 4.0 * (ph - 0.5).abs(),
    }
}

/// The LFO's value (-1..1) at phase `ph`; `sh` the held random level.
pub(crate) fn lfo_wave(s: LfoShape, ph: f32, sh: f32) -> f32 {
    match s {
        LfoShape::Sine => (TAU * ph).sin(),
        LfoShape::Triangle => 1.0 - 4.0 * (ph - 0.5).abs(),
        LfoShape::Square => {
            if ph < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        LfoShape::Saw => 1.0 - 2.0 * ph,
        LfoShape::SampleHold => sh,
    }
}

fn tune(cut: f32, res: f32, kind: FilterKind, rate: f32) -> Coefs {
    let g = (PI * cut / rate).tan();
    let res = res.clamp(0.0, 1.0);
    let mut c = Coefs::default();
    match kind {
        FilterKind::Classic => {
            c.k = 2.0 - 1.9 * res;
            c.a1 = 1.0 / (1.0 + g * (g + c.k));
            c.a2 = g * c.a1;
            c.a3 = g * c.a2;
        }
        FilterKind::Ladder => {
            c.lg = g / (1.0 + g);
            c.lk = 3.96 * res;
            c.comp = 1.0 + 0.6 * c.lk;
        }
    }
    c
}

/// A cheap tanh (exact enough for saturation, and bounded).
#[inline]
fn soft(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

#[inline]
fn filt(c: &Coefs, st: &mut FState, x: f32, ty: FilterType, kind: FilterKind, drive: f32) -> f32 {
    match kind {
        FilterKind::Classic => {
            let s = &mut st.s;
            let v3 = x - s[1];
            let v1 = c.a1 * s[0] + c.a2 * v3;
            let v2 = s[1] + c.a2 * s[0] + c.a3 * v3;
            s[0] = 2.0 * v1 - s[0];
            s[1] = 2.0 * v2 - s[1];
            match ty {
                FilterType::LowPass => v2,
                FilterType::BandPass => v1,
                FilterType::HighPass => x - c.k * v1 - v2,
            }
        }
        FilterKind::Ladder => {
            let x = if drive > 0.0 {
                let dg = 1.0 + 4.0 * drive;
                soft(x * dg) / soft(dg).max(0.5) * (1.0 + 0.5 * drive)
            } else {
                x
            };
            let g = c.lg;
            let b = 1.0 - g;
            let s = &mut st.s;
            let sig = g * g * g * b * s[0] + g * g * b * s[1] + g * b * s[2] + b * s[3];
            let g4 = g * g * g * g;
            let u = soft((x - c.lk * sig) / (1.0 + c.lk * g4));
            let mut y = [0.0f32; 4];
            let mut inp = u;
            for k in 0..4 {
                let v = (inp - s[k]) * g;
                y[k] = v + s[k];
                s[k] = y[k] + v;
                inp = y[k];
            }
            // Keep denormals out of a dying tail.
            for v in s.iter_mut() {
                if v.abs() < 1e-20 {
                    *v = 0.0;
                }
            }
            let out = match ty {
                FilterType::LowPass => y[3],
                FilterType::BandPass => 4.0 * (y[1] - 2.0 * y[2] + y[3]),
                FilterType::HighPass => u - 4.0 * y[0] + 6.0 * y[1] - 4.0 * y[2] + y[3],
            };
            out * c.comp
        }
    }
}

/// A stereo chorus: two short delays swept by a triangle in opposite
/// directions.
struct Chorus {
    buf: [Vec<f32>; 2],
    pos: usize,
    ph: f32,
    rate: f32,
}

impl Chorus {
    fn new(rate: f32) -> Chorus {
        let n = (rate * CHORUS_MAX) as usize + 4;
        Chorus {
            buf: [vec![0.0; n], vec![0.0; n]],
            pos: 0,
            ph: 0.0,
            rate,
        }
    }

    fn clear(&mut self) {
        for b in &mut self.buf {
            b.fill(0.0);
        }
    }

    fn run(&mut self, l: &mut [f32], r: &mut [f32], x: &SynthControls, dt: f32) {
        let len = self.buf[0].len();
        let mix = x.chorus.clamp(0.0, 1.0);
        let base = 0.0035 * self.rate;
        let swing = (0.0005 + 0.0045 * x.chorus_depth.clamp(0.0, 1.0)) * self.rate;
        let inc = x.chorus_rate.clamp(0.01, 10.0) * dt;
        for i in 0..l.len() {
            let tri = 1.0 - 4.0 * (self.ph - 0.5).abs();
            self.ph = wrap(self.ph + inc);
            let ins = [l[i], r[i]];
            self.buf[0][self.pos] = ins[0];
            self.buf[1][self.pos] = ins[1];
            let mut wet = [0.0f32; 2];
            for (c, w) in wet.iter_mut().enumerate() {
                let dly =
                    (base + swing * if c == 0 { tri } else { -tri }).clamp(1.0, len as f32 - 2.0);
                let rp = self.pos as f32 - dly + len as f32;
                let i0 = rp.floor();
                let fr = rp - i0;
                let a = self.buf[c][i0 as usize % len];
                let b = self.buf[c][(i0 as usize + 1) % len];
                *w = a + (b - a) * fr;
            }
            l[i] = ins[0] * (1.0 - 0.35 * mix) + wet[0] * 0.75 * mix;
            r[i] = ins[1] * (1.0 - 0.35 * mix) + wet[1] * 0.75 * mix;
            self.pos = (self.pos + 1) % len;
        }
    }
}

impl Engine {
    /// After a jump to `t0`: everything stops, and the clip notes
    /// sounding there start again at the level their envelopes have
    /// reached.
    pub(crate) fn seek(&mut self, t0: f64, notes: &[Note], at: &mut usize) {
        self.clear();
        *at = notes.partition_point(|n| n.start < t0);
        let rel = self.d.release;
        for (k, n) in notes[..*at].iter().enumerate() {
            if n.end + rel > t0 {
                self.note_on(t0, n.start, k as u32, *n);
            }
        }
    }
}
