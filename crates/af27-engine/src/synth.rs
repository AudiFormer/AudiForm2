// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The built-in MIDI instrument and the metronome click.
//!
//! The untouched instrument plays through the original engine, kept
//! exactly as it was: notes are known ahead (they come from MIDI clips),
//! so each voice is worked out from its note's own time: a band-limited
//! (PolyBLEP) oscillator plus a detuned second one, an ADSR envelope with
//! its release after the note ends, velocity and pan. Only the track's
//! resonant low-pass filter keeps state. Pitch bend and mod-wheel vibrato
//! bend every note's time by one shared warp, so with neither moved the
//! sound is the same bit for bit.
//!
//! Once any of the newer controls is set the full engine in
//! `synth_voice.rs` plays instead (voices, unison, filters, LFO, glide,
//! chorus). Nothing allocates while playing.

use af27_core::midi::{
    note_freq, ClickSound, Instrument, MetronomeSettings, PlannedControl, Segment, Subdivide, Wave,
    MOD_WHEEL, PITCH_BEND,
};
use af27_core::synth_model::LfoShape;
use af27_core::time::db_to_gain;
use std::f32::consts::{PI, TAU};

use crate::synth_voice::{lfo_wave, Engine};

/// Most samples worked out at once (the size of the shared buffers).
const CHUNK: usize = 512;
/// Live keys' ids in the full engine (a clip note's id is its index).
const LIVE: u32 = 0x8000_0000;

/// One note in seconds, ready to play.
#[derive(Debug, Clone, Copy)]
pub struct Note {
    pub start: f64,
    pub end: f64,
    pub freq: f32,
    pub gain: f32,
    /// Constant-power pan gains.
    pub pl: f32,
    pub pr: f32,
    /// Velocity 0..1.
    pub vel: f32,
    pub pitch: u8,
    /// The shared pitch warp where this note started (original engine).
    w0: f64,
    w_set: bool,
}

pub struct Synth {
    pub inst: Instrument,
    /// Sorted by start.
    pub notes: Vec<Note>,
    gain: f32,
    svf: [Svf; 2],
    rate: f32,
    /// The original engine plays (no newer control set).
    classic: bool,
    eng: Box<Engine>,
    /// Clip mod-wheel and pitch-bend moves, in time order.
    ctl: Vec<PlannedControl>,
    ctl_at: usize,
    /// The next clip note the full engine starts.
    note_at: usize,
    /// Pitch bend (-1..1) and mod wheel (0..1): where each is going and
    /// where it is (eased, so moves do not click).
    bend_to: f32,
    wheel_to: f32,
    bend: f32,
    wheel: f32,
    ease: f32,
    lfo_ph: f32,
    lfo_sh: f32,
    lfo_n: u32,
    /// The original engine's pitch warp: the running sum of (pitch
    /// factor - 1) over time, on once bend or vibrato has moved.
    w: f64,
    w_on: bool,
    wbuf: Vec<f64>,
    pf: Vec<f32>,
    lfo: Vec<f32>,
    sl: Vec<f32>,
    sr: Vec<f32>,
    /// Played by hand (no clip notes, never jumps).
    live: bool,
    next_t: f64,
    seek: bool,
}

impl Synth {
    pub fn new(inst: Instrument, mut notes: Vec<Note>, rate: u32) -> Synth {
        notes.sort_by(|a, b| a.start.total_cmp(&b.start));
        let mut s = Synth {
            inst,
            notes,
            gain: 1.0,
            svf: [Svf::default(); 2],
            rate: rate as f32,
            classic: inst.is_classic(),
            eng: Box::new(Engine::new(inst, rate as f32)),
            ctl: Vec::new(),
            ctl_at: 0,
            note_at: 0,
            bend_to: 0.0,
            wheel_to: 0.0,
            bend: 0.0,
            wheel: 0.0,
            ease: 1.0 - (-1.0 / (0.004 * rate as f32)).exp(),
            lfo_ph: 0.0,
            lfo_sh: 0.0,
            lfo_n: 1,
            w: 0.0,
            w_on: false,
            wbuf: vec![0.0; CHUNK],
            pf: vec![1.0; CHUNK],
            lfo: vec![0.0; CHUNK],
            sl: vec![0.0; CHUNK],
            sr: vec![0.0; CHUNK],
            live: false,
            next_t: f64::NAN,
            seek: true,
        };
        s.set(inst);
        s
    }

    /// For notes played by hand (`key_on`, `key_off`).
    pub fn live(inst: Instrument, rate: u32) -> Synth {
        let mut s = Synth::new(inst, Vec::new(), rate);
        s.live = true;
        s
    }

    /// With a clip's mod-wheel and pitch-bend moves (in time order).
    pub fn with_controls(mut self, ctl: Vec<PlannedControl>) -> Synth {
        self.ctl = ctl;
        self.seek = true;
        self
    }

    pub fn set(&mut self, inst: Instrument) {
        let was = self.classic;
        self.inst = inst;
        self.gain = db_to_gain(inst.gain_db);
        for f in &mut self.svf {
            f.tune(inst.cutoff, inst.resonance, self.rate);
        }
        self.eng.set(inst);
        self.classic = inst.is_classic();
        if was != self.classic {
            if self.live {
                self.hand_over();
            } else {
                self.seek = true;
            }
        }
    }

    /// The engine changed under held keys: they carry on in the new one.
    fn hand_over(&mut self) {
        let t = if self.next_t.is_nan() {
            0.0
        } else {
            self.next_t
        };
        if self.classic {
            let cap = self.notes.capacity();
            for (_, n) in self.eng.held_notes(t) {
                if self.notes.len() < cap {
                    self.notes.push(n);
                }
            }
            self.eng.clear();
        } else {
            for n in self.notes.iter().filter(|n| n.end > t) {
                self.eng.note_on(t, n.start, LIVE | n.pitch as u32, *n);
            }
            self.notes.clear();
        }
    }

    /// The longest a note sounds after its key comes up.
    pub fn release(&self) -> f64 {
        self.inst.release.max(0.001) as f64
    }

    /// A key played by hand at `t`.
    pub fn key_on(&mut self, t: f64, pitch: u8, velocity: u8, pan: f32) {
        let n = note(t, t + 3600.0, pitch, velocity, pan);
        if self.classic {
            if self.notes.len() < self.notes.capacity() {
                self.notes.push(n);
            }
        } else {
            self.eng.note_on(t, t, LIVE | pitch as u32, n);
        }
    }

    pub fn key_off(&mut self, t: f64, pitch: u8) {
        let f = note_freq(pitch as f32);
        for n in self.notes.iter_mut().filter(|n| n.freq == f && n.end > t) {
            n.end = t;
        }
        self.eng.note_off(t, LIVE | pitch as u32);
    }

    pub fn keys_off(&mut self, t: f64) {
        for n in self.notes.iter_mut().filter(|n| n.end > t) {
            n.end = t;
        }
        self.eng.all_off(t);
    }

    /// Pitch bend by hand, 0..1 (0.5 is the middle).
    pub fn set_bend(&mut self, v: f32) {
        self.bend_to = bend_amount(v);
    }

    /// The mod wheel by hand, 0..1.
    pub fn set_wheel(&mut self, v: f32) {
        self.wheel_to = v.clamp(0.0, 1.0);
    }

    /// Adds the notes sounding over `n` frames from `t0` into `l` and `r`.
    pub fn render(&mut self, t0: f64, dt: f64, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        let mut done = 0;
        while done < n {
            let m = (n - done).min(CHUNK);
            let t = t0 + dt * done as f64;
            self.render_part(t, dt, &mut l[done..done + m], &mut r[done..done + m]);
            done += m;
        }
    }

    fn render_part(&mut self, t0: f64, dt: f64, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        if !self.live && (self.seek || self.next_t.is_nan() || (t0 - self.next_t).abs() > dt * 0.5)
        {
            self.jump(t0);
        }
        self.seek = false;
        self.mods(t0, dt, n);
        if self.classic {
            self.render_classic(t0, dt, l, r);
        } else {
            let (sl, sr) = (&mut self.sl[..n], &mut self.sr[..n]);
            sl.fill(0.0);
            sr.fill(0.0);
            self.eng.render(
                t0,
                dt,
                sl,
                sr,
                &self.pf[..n],
                &self.lfo[..n],
                &self.notes,
                &mut self.note_at,
            );
            for i in 0..n {
                l[i] += sl[i] * self.gain;
                r[i] += sr[i] * self.gain;
            }
        }
        self.next_t = t0 + dt * n as f64;
    }

    /// After a jump: controllers as they stand at `t0`, the warp afresh,
    /// and the full engine's notes sounding there.
    fn jump(&mut self, t0: f64) {
        self.ctl_at = self.ctl.partition_point(|e| e.t < t0);
        if !self.ctl.is_empty() {
            let before = &self.ctl[..self.ctl_at];
            self.bend_to = before
                .iter()
                .rev()
                .find(|e| e.controller == PITCH_BEND)
                .map_or(0.0, |e| bend_amount(e.value));
            self.wheel_to = before
                .iter()
                .rev()
                .find(|e| e.controller == MOD_WHEEL)
                .map_or(0.0, |e| e.value);
            self.bend = self.bend_to;
            self.wheel = self.wheel_to;
        }
        self.w = 0.0;
        self.w_on = false;
        for n in &mut self.notes {
            n.w_set = false;
        }
        if !self.classic {
            self.eng.seek(t0, &self.notes, &mut self.note_at);
        }
    }

    /// The shared modulation for `n` samples from `t0`: controller moves,
    /// the LFO, the pitch factor and (original engine) the warp.
    fn mods(&mut self, t0: f64, dt: f64, n: usize) {
        let x = self.inst.ext;
        let inc = x.lfo_rate.max(0.0) * dt as f32;
        for i in 0..n {
            let t = t0 + dt * i as f64;
            while let Some(e) = self.ctl.get(self.ctl_at).filter(|e| e.t <= t) {
                match e.controller {
                    MOD_WHEEL => self.wheel_to = e.value.clamp(0.0, 1.0),
                    PITCH_BEND => self.bend_to = bend_amount(e.value),
                    _ => {}
                }
                self.ctl_at += 1;
            }
            self.bend += (self.bend_to - self.bend) * self.ease;
            if (self.bend - self.bend_to).abs() < 1e-6 {
                self.bend = self.bend_to;
            }
            self.wheel += (self.wheel_to - self.wheel) * self.ease;
            if (self.wheel - self.wheel_to).abs() < 1e-6 {
                self.wheel = self.wheel_to;
            }
            let depth = (x.lfo_depth + self.wheel * x.wheel_depth).clamp(0.0, 1.0);
            let lv = if depth > 0.0 {
                lfo_wave(x.lfo_shape, self.lfo_ph, self.lfo_sh) * depth
            } else {
                0.0
            };
            self.lfo_ph += inc;
            if self.lfo_ph >= 1.0 {
                self.lfo_ph -= self.lfo_ph.floor();
                if x.lfo_shape == LfoShape::SampleHold {
                    self.lfo_n = self.lfo_n.wrapping_add(1);
                    self.lfo_sh = noise(self.lfo_n);
                }
            }
            let semis = self.bend * x.bend_range + lv * x.lfo_pitch;
            let pf = if semis == 0.0 {
                1.0
            } else {
                (semis / 12.0).exp2()
            };
            self.pf[i] = pf;
            self.lfo[i] = lv;
            if self.classic {
                if pf != 1.0 {
                    self.w_on = true;
                }
                self.wbuf[i] = self.w;
                self.w += (pf as f64 - 1.0) * dt;
            }
        }
    }

    /// The original engine.
    fn render_classic(&mut self, t0: f64, dt: f64, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        let t1 = t0 + dt * n as f64;
        let rel = self.inst.release.max(0.001) as f64;
        let w_on = self.w_on;
        let Synth {
            notes, inst, wbuf, ..
        } = self;
        // Notes are sorted by start; stop once they start after this block.
        for note in notes.iter_mut().take_while(|x| x.start < t1) {
            if note.end + rel <= t0 {
                continue;
            }
            for i in 0..n {
                let t = t0 + dt * i as f64;
                let local = t - note.start;
                if local < 0.0 {
                    continue;
                }
                let env = envelope(inst, local as f32, (note.end - note.start) as f32);
                if env <= 0.0 {
                    continue;
                }
                // Bent time: the note's own time plus the warp since it
                // started (only once anything has bent the pitch).
                let at = if w_on {
                    if !note.w_set {
                        note.w_set = true;
                        note.w0 = wbuf[i];
                    }
                    local + (wbuf[i] - note.w0)
                } else {
                    local
                };
                let s = voice(inst, note.freq, at, dt as f32) * env * note.gain;
                l[i] += s * note.pl;
                r[i] += s * note.pr;
            }
        }
        for i in 0..n {
            l[i] = self.svf[0].run(l[i]) * self.gain;
            r[i] = self.svf[1].run(r[i]) * self.gain;
        }
    }

    pub fn reset(&mut self) {
        self.svf = [Svf::default(); 2];
        self.eng.clear();
        self.seek = true;
        let inst = self.inst;
        self.set(inst);
    }
}

/// A pitch bend value (0..1, 0.5 the middle) as -1..1.
fn bend_amount(v: f32) -> f32 {
    if v.is_nan() {
        return 0.0;
    }
    ((v.clamp(0.0, 1.0) * 16383.0 - 8192.0) / 8192.0).clamp(-1.0, 1.0)
}

/// ADSR level at `t` seconds into a note held for `held` seconds.
pub fn envelope(i: &Instrument, t: f32, held: f32) -> f32 {
    let held_level = |t: f32| -> f32 {
        let a = i.attack.max(0.0005);
        let d = i.decay.max(0.001);
        if t < a {
            t / a
        } else if t < a + d {
            1.0 - (1.0 - i.sustain) * (t - a) / d
        } else {
            i.sustain
        }
    };
    if t <= held {
        held_level(t)
    } else {
        let r = i.release.max(0.001);
        let from = held_level(held);
        let x = (t - held) / r;
        if x >= 1.0 {
            0.0
        } else {
            from * (1.0 - x)
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

fn osc(wave: Wave, freq: f32, local: f64, dt: f32) -> f32 {
    let ph = ((freq as f64 * local).fract()) as f32;
    let inc = (freq * dt).min(0.5);
    match wave {
        Wave::Sine => (TAU * ph).sin(),
        Wave::Saw => 2.0 * ph - 1.0 - poly_blep(ph, inc),
        // The original engine's pulse width is always a half: a square.
        Wave::Square | Wave::Pulse => {
            let mut v = if ph < 0.5 { 1.0 } else { -1.0 };
            v += poly_blep(ph, inc);
            v -= poly_blep((ph + 0.5).fract(), inc);
            v
        }
        Wave::Triangle => 1.0 - 4.0 * (ph - 0.5).abs(),
    }
}

fn voice(i: &Instrument, freq: f32, local: f64, dt: f32) -> f32 {
    let a = osc(i.wave, freq, local, dt);
    if i.detune.abs() < 0.01 {
        return a;
    }
    let f2 = freq * 2f32.powf(i.detune / 1200.0);
    (a + osc(i.wave, f2, local, dt)) * 0.5
}

impl Note {
    /// The note `semitones` off its key (a MIDI clip's fractional pitch
    /// shift).
    pub fn detuned(mut self, semitones: f32) -> Note {
        if semitones != 0.0 {
            self.freq *= 2f32.powf(semitones / 12.0);
        }
        self
    }
}

/// A note's frequency, gain and pan from MIDI values.
pub fn note(start: f64, end: f64, pitch: u8, velocity: u8, pan: f32) -> Note {
    let p = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    let v = velocity.clamp(1, 127) as f32 / 127.0;
    Note {
        start,
        end,
        freq: note_freq(pitch as f32),
        gain: v * v * 0.5,
        pl: p.cos() * std::f32::consts::SQRT_2,
        pr: p.sin() * std::f32::consts::SQRT_2,
        vel: v,
        pitch,
        w0: 0.0,
        w_set: false,
    }
}

/// A state-variable low-pass (trapezoidal integration, stable at any setting).
#[derive(Debug, Clone, Copy, Default)]
struct Svf {
    g: f32,
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    ic1: f32,
    ic2: f32,
}

impl Svf {
    fn tune(&mut self, cutoff: f32, resonance: f32, rate: f32) {
        let f = cutoff.clamp(20.0, rate * 0.45);
        self.g = (PI * f / rate).tan();
        self.k = 2.0 - 1.9 * resonance.clamp(0.0, 1.0);
        self.a1 = 1.0 / (1.0 + self.g * (self.g + self.k));
        self.a2 = self.g * self.a1;
        self.a3 = self.g * self.a2;
    }

    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        v2
    }
}

/// How long one click lasts (seconds).
pub const CLICK_LEN: f64 = 0.08;

/// The metronome click at time `t` on a tempo map's stretches (in
/// order), with the default settings.
#[inline]
pub fn click(t: f64, segs: &[Segment]) -> f32 {
    click_with(t, segs, &MetronomeSettings::default())
}

/// White noise in -1..1 for step `n` (a hash: the same every time, no
/// state, no allocation).
#[inline]
fn noise(n: u32) -> f32 {
    let mut x = n.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    (x >> 8) as f32 / (1u32 << 23) as f32 - 1.0
}

/// One click of `sound`, `local` seconds after it starts, its pitch
/// times `pitch`; peaks near 1.
#[inline]
pub fn click_sound(sound: ClickSound, local: f32, pitch: f32) -> f32 {
    if !(0.0..CLICK_LEN as f32).contains(&local) {
        return 0.0;
    }
    match sound {
        ClickSound::Tick => (TAU * 1600.0 * pitch * local).sin() * (-local / 0.006).exp(),
        ClickSound::Wood => {
            let f = 820.0 * pitch;
            let tone = 0.7 * (TAU * f * local).sin() + 0.3 * (TAU * f * 2.63 * local).sin();
            tone * (-local / 0.018).exp()
        }
        ClickSound::Beep => {
            let env = if local < 0.003 {
                local / 0.003
            } else if local < 0.05 {
                1.0
            } else {
                (1.0 - (local - 0.05) / 0.01).max(0.0)
            };
            (TAU * 880.0 * pitch * local).sin() * env * 0.8
        }
        // Noise steps at 48 kHz (faster for a higher click), whatever
        // the output rate.
        ClickSound::Shaker => {
            noise((local * 48_000.0 * pitch) as u32) * (-local / 0.012).exp() * 0.7
        }
    }
}

/// Clicks per beat for a subdivision.
pub fn per_beat(s: Subdivide) -> u32 {
    match s {
        Subdivide::None => 1,
        Subdivide::Halves => 2,
        Subdivide::Triplets => 3,
        Subdivide::Quarters => 4,
    }
}

/// The metronome at time `t` on a tempo map's stretches (in order): a
/// click on every beat of the time signature (eighths in 6/8), and on
/// the subdivisions between them. The first beat of each bar is higher
/// (pitch times 1.5) and louder (1.0) when `accent` is on; other beats
/// are at 0.75 and subdivisions at 0.4, at the beat's pitch. Scaled by
/// `level_db`; silent when muted. It depends on `t` alone, so it follows
/// loops, jumps and tempo changes by itself.
#[inline]
pub fn click_with(t: f64, segs: &[Segment], s: &MetronomeSettings) -> f32 {
    if s.muted || t < 0.0 {
        return 0.0;
    }
    let Some(first) = segs.first() else {
        return 0.0;
    };
    let mut seg = first;
    for x in &segs[1..] {
        if x.secs > t {
            break;
        }
        seg = x;
    }
    let per = per_beat(s.subdivide) as i64;
    let (k, local) = if seg.is_ramp() {
        // In a glide the clicks are evenly spaced in beats, not seconds:
        // find the last click in beats, then how long ago it was.
        let step = seg.unit_beats() / per as f64;
        let into = (seg.beats_at(t) - seg.beat).max(0.0);
        let k = ((into + 1e-9) / step).floor();
        (k, (t - seg.secs_at(seg.beat + k * step)).max(0.0))
    } else {
        let step = seg.spq() * seg.unit_beats() / per as f64;
        let into = (t - seg.secs).max(0.0);
        // A hair of slack so a click computed a little late still sounds.
        let k = ((into + 1e-9) / step).floor();
        (k, (into - k * step).max(0.0))
    };
    if local >= CLICK_LEN {
        return 0.0;
    }
    let k = k as i64;
    let (beat, sub) = (k.div_euclid(per), k.rem_euclid(per));
    // Each stretch starts on a bar line.
    let downbeat = beat.rem_euclid(seg.beats_per_bar.max(1) as i64) == 0;
    let (pitch, level) = if sub != 0 {
        (1.0, 0.4)
    } else if downbeat && s.accent {
        (1.5, 1.0)
    } else {
        (1.0, 0.75)
    };
    let gain = af27_core::time::db_to_gain(s.level_db) * level;
    click_sound(s.sound, local as f32, pitch) * gain
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine as it was before the newer controls, copied unchanged,
    /// to hold the default tone to it bit for bit.
    mod before {
        use af27_core::midi::{Instrument, Wave};
        use std::f32::consts::{PI, TAU};

        #[derive(Debug, Clone, Copy)]
        pub struct Note {
            pub start: f64,
            pub end: f64,
            pub freq: f32,
            pub gain: f32,
            pub pl: f32,
            pub pr: f32,
        }

        pub fn note(start: f64, end: f64, pitch: u8, velocity: u8, pan: f32) -> Note {
            let p = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
            let v = velocity.clamp(1, 127) as f32 / 127.0;
            Note {
                start,
                end,
                freq: af27_core::midi::note_freq(pitch as f32),
                gain: v * v * 0.5,
                pl: p.cos() * std::f32::consts::SQRT_2,
                pr: p.sin() * std::f32::consts::SQRT_2,
            }
        }

        pub struct Synth {
            pub inst: Instrument,
            pub notes: Vec<Note>,
            gain: f32,
            svf: [Svf; 2],
            rate: f32,
        }

        impl Synth {
            pub fn new(inst: Instrument, mut notes: Vec<Note>, rate: u32) -> Synth {
                notes.sort_by(|a, b| a.start.total_cmp(&b.start));
                let mut s = Synth {
                    inst,
                    notes,
                    gain: 1.0,
                    svf: [Svf::default(); 2],
                    rate: rate as f32,
                };
                s.set(inst);
                s
            }

            pub fn set(&mut self, inst: Instrument) {
                self.inst = inst;
                self.gain = af27_core::time::db_to_gain(inst.gain_db);
                for f in &mut self.svf {
                    f.tune(inst.cutoff, inst.resonance, self.rate);
                }
            }

            pub fn render(&mut self, t0: f64, dt: f64, l: &mut [f32], r: &mut [f32]) {
                let n = l.len();
                let t1 = t0 + dt * n as f64;
                let rel = self.inst.release.max(0.001) as f64;
                for note in self.notes.iter().take_while(|x| x.start < t1) {
                    if note.end + rel <= t0 {
                        continue;
                    }
                    for i in 0..n {
                        let t = t0 + dt * i as f64;
                        let local = t - note.start;
                        if local < 0.0 {
                            continue;
                        }
                        let env =
                            envelope(&self.inst, local as f32, (note.end - note.start) as f32);
                        if env <= 0.0 {
                            continue;
                        }
                        let s = voice(&self.inst, note.freq, local, dt as f32) * env * note.gain;
                        l[i] += s * note.pl;
                        r[i] += s * note.pr;
                    }
                }
                for i in 0..n {
                    l[i] = self.svf[0].run(l[i]) * self.gain;
                    r[i] = self.svf[1].run(r[i]) * self.gain;
                }
            }
        }

        pub fn envelope(i: &Instrument, t: f32, held: f32) -> f32 {
            let held_level = |t: f32| -> f32 {
                let a = i.attack.max(0.0005);
                let d = i.decay.max(0.001);
                if t < a {
                    t / a
                } else if t < a + d {
                    1.0 - (1.0 - i.sustain) * (t - a) / d
                } else {
                    i.sustain
                }
            };
            if t <= held {
                held_level(t)
            } else {
                let r = i.release.max(0.001);
                let from = held_level(held);
                let x = (t - held) / r;
                if x >= 1.0 {
                    0.0
                } else {
                    from * (1.0 - x)
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

        fn osc(wave: Wave, freq: f32, local: f64, dt: f32) -> f32 {
            let ph = ((freq as f64 * local).fract()) as f32;
            let inc = (freq * dt).min(0.5);
            match wave {
                Wave::Sine => (TAU * ph).sin(),
                Wave::Saw => 2.0 * ph - 1.0 - poly_blep(ph, inc),
                Wave::Square | Wave::Pulse => {
                    let mut v = if ph < 0.5 { 1.0 } else { -1.0 };
                    v += poly_blep(ph, inc);
                    v -= poly_blep((ph + 0.5).fract(), inc);
                    v
                }
                Wave::Triangle => 1.0 - 4.0 * (ph - 0.5).abs(),
            }
        }

        fn voice(i: &Instrument, freq: f32, local: f64, dt: f32) -> f32 {
            let a = osc(i.wave, freq, local, dt);
            if i.detune.abs() < 0.01 {
                return a;
            }
            let f2 = freq * 2f32.powf(i.detune / 1200.0);
            (a + osc(i.wave, f2, local, dt)) * 0.5
        }

        #[derive(Debug, Clone, Copy, Default)]
        struct Svf {
            g: f32,
            k: f32,
            a1: f32,
            a2: f32,
            a3: f32,
            ic1: f32,
            ic2: f32,
        }

        impl Svf {
            fn tune(&mut self, cutoff: f32, resonance: f32, rate: f32) {
                let f = cutoff.clamp(20.0, rate * 0.45);
                self.g = (PI * f / rate).tan();
                self.k = 2.0 - 1.9 * resonance.clamp(0.0, 1.0);
                self.a1 = 1.0 / (1.0 + self.g * (self.g + self.k));
                self.a2 = self.g * self.a1;
                self.a3 = self.g * self.a2;
            }

            fn run(&mut self, x: f32) -> f32 {
                let v3 = x - self.ic2;
                let v1 = self.a1 * self.ic1 + self.a2 * v3;
                let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
                self.ic1 = 2.0 * v1 - self.ic1;
                self.ic2 = 2.0 * v2 - self.ic2;
                v2
            }
        }
    }

    const RATE: u32 = 48_000;
    const DT: f64 = 1.0 / RATE as f64;

    /// (start, end, pitch, velocity, pan) for a little phrase with chords,
    /// overlaps, a repeated key and pans.
    const PHRASE: [(f64, f64, u8, u8, f32); 7] = [
        (0.0, 0.4, 48, 100, 0.0),
        (0.05, 0.3, 60, 127, -0.5),
        (0.05, 0.3, 64, 90, 0.5),
        (0.31, 0.9, 67, 40, 0.0),
        (0.5, 0.52, 72, 110, 1.0),
        (0.6, 1.2, 72, 70, -1.0),
        (1.0, 1.01, 36, 1, 0.2),
    ];

    /// The same phrase through today's and the old engine, in blocks of
    /// `block`, compared bit for bit.
    fn same_as_before(inst: Instrument, block: usize) {
        let phrase = std::hint::black_box(PHRASE);
        let new_notes = phrase
            .iter()
            .map(|&(a, b, p, v, pan)| note(a, b, p, v, pan))
            .collect();
        let old_notes = phrase
            .iter()
            .map(|&(a, b, p, v, pan)| before::note(a, b, p, v, pan))
            .collect();
        // Opaque to the optimiser, so neither side has its maths folded
        // at compile time (which can round differently).
        let inst = std::hint::black_box(inst);
        let mut s = Synth::new(std::hint::black_box(inst), new_notes, RATE);
        let mut o = before::Synth::new(std::hint::black_box(inst), old_notes, RATE);
        let total = (1.8 * RATE as f64) as usize;
        let mut t = 0usize;
        let mut heard = 0.0f32;
        while t < total {
            let n = block.min(total - t);
            let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
            let (mut ol, mut or) = (vec![0.0f32; n], vec![0.0f32; n]);
            let t0 = t as f64 * DT;
            s.render(t0, DT, &mut l, &mut r);
            o.render(t0, DT, &mut ol, &mut or);
            for i in 0..n {
                assert_eq!(
                    l[i].to_bits(),
                    ol[i].to_bits(),
                    "left at {} ({inst:?})",
                    t + i
                );
                assert_eq!(
                    r[i].to_bits(),
                    or[i].to_bits(),
                    "right at {} ({inst:?})",
                    t + i
                );
                heard = heard.max(l[i].abs());
            }
            t += n;
        }
        assert!(heard > 0.01);
    }

    #[test]
    fn the_default_tone_is_unchanged_bit_for_bit() {
        same_as_before(Instrument::default(), 256);
        same_as_before(Instrument::default(), 1024);
        same_as_before(Instrument::default(), 137);
        // The original controls still play the original engine.
        for wave in [Wave::Sine, Wave::Triangle, Wave::Square, Wave::Saw] {
            same_as_before(
                Instrument {
                    wave,
                    detune: 0.0,
                    attack: 0.08,
                    decay: 0.6,
                    sustain: 0.3,
                    release: 0.9,
                    cutoff: 900.0,
                    resonance: 0.7,
                    gain_db: -3.0,
                    ..Instrument::default()
                },
                512,
            );
        }
    }

    #[test]
    fn the_default_tone_played_by_hand_is_unchanged() {
        let inst = Instrument::default();
        let mut s = Synth::live(inst, RATE);
        s.notes.reserve(32);
        let mut o = before::Synth::new(inst, Vec::new(), RATE);
        let mut t = 0.0;
        let n = 480;
        for k in 0..100 {
            if k == 3 {
                s.key_on(t, 60, 100, 0.0);
                o.notes.push(before::note(t, t + 3600.0, 60, 100, 0.0));
            }
            if k == 40 {
                s.key_off(t, 60);
                for x in o.notes.iter_mut() {
                    x.end = t;
                }
            }
            let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
            let (mut ol, mut or) = (vec![0.0f32; n], vec![0.0f32; n]);
            s.render(t, DT, &mut l, &mut r);
            o.render(t, DT, &mut ol, &mut or);
            assert_eq!(l, ol);
            assert_eq!(r, or);
            t += n as f64 * DT;
        }
    }

    fn render_all(s: &mut Synth, secs: f64) -> (Vec<f32>, Vec<f32>) {
        let total = (secs * RATE as f64) as usize;
        let (mut l, mut r) = (vec![0.0f32; total], vec![0.0f32; total]);
        let mut t = 0;
        while t < total {
            let n = 256.min(total - t);
            s.render(t as f64 * DT, DT, &mut l[t..t + n], &mut r[t..t + n]);
            t += n;
        }
        (l, r)
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn every_model_plays_cleanly() {
        use af27_core::synth_model::SynthModel;
        for m in SynthModel::ALL {
            let notes = PHRASE
                .iter()
                .map(|&(a, b, p, v, pan)| note(a, b, p, v, pan))
                .collect();
            let mut s = Synth::new(m.preset(), notes, RATE);
            let (l, r) = render_all(&mut s, 3.5);
            assert!(l.iter().chain(&r).all(|v| v.is_finite()), "{m:?}");
            let p = peak(&l).max(peak(&r));
            assert!(p > 0.02 && p < 2.0, "{m:?} peaks at {p}");
            // Silent once everything has let go (the chorus tail too).
            let tail = &l[(3.3 * RATE as f64) as usize..];
            assert!(peak(tail) < 1e-3, "{m:?} still sounds: {}", peak(tail));
        }
    }

    /// Zero crossings upward per second over `x` (a pitch estimate).
    fn freq_of(x: &[f32]) -> f64 {
        let ups = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        ups as f64 / (x.len() as f64 / RATE as f64)
    }

    #[test]
    fn pitch_bend_and_the_mod_wheel_move_both_engines() {
        use af27_core::midi::{PlannedControl, MOD_WHEEL};
        let sine = Instrument {
            wave: Wave::Sine,
            detune: 0.0,
            attack: 0.001,
            sustain: 1.0,
            cutoff: 18_000.0,
            resonance: 0.0,
            ..Instrument::default()
        };
        let mut full = sine;
        full.ext.filter_kind = af27_core::synth_model::FilterKind::Ladder;
        for inst in [sine, full] {
            let bend = vec![PlannedControl {
                t: 1.0,
                controller: PITCH_BEND,
                value: 1.0,
            }];
            let mut s =
                Synth::new(inst, vec![note(0.0, 2.0, 69, 100, 0.0)], RATE).with_controls(bend);
            let (l, _) = render_all(&mut s, 2.0);
            let a = freq_of(&l[(0.2 * RATE as f64) as usize..(0.9 * RATE as f64) as usize]);
            let b = freq_of(&l[(1.2 * RATE as f64) as usize..(1.9 * RATE as f64) as usize]);
            assert!((a - 440.0).abs() < 3.0, "{a}");
            let want = 440.0 * 2f64.powf(2.0 / 12.0);
            assert!(
                (b - want).abs() < 3.0,
                "{b} for {want} ({:?})",
                inst.ext.filter_kind
            );
            // The wheel adds vibrato: the pitch wanders, on average the same.
            let wheel = vec![PlannedControl {
                t: 0.0,
                controller: MOD_WHEEL,
                value: 1.0,
            }];
            let mut s =
                Synth::new(inst, vec![note(0.0, 2.0, 69, 100, 0.0)], RATE).with_controls(wheel);
            let (l, _) = render_all(&mut s, 2.0);
            let short = |a: f64| {
                freq_of(&l[(a * RATE as f64) as usize..((a + 0.05) * RATE as f64) as usize])
            };
            let fs: Vec<f64> = (0..20).map(|k| short(0.2 + k as f64 * 0.07)).collect();
            let spread =
                fs.iter().cloned().fold(0.0, f64::max) - fs.iter().cloned().fold(1e9, f64::min);
            assert!(spread > 15.0, "{fs:?}");
            let avg = freq_of(&l[(0.2 * RATE as f64) as usize..(1.8 * RATE as f64) as usize]);
            assert!((avg - 440.0).abs() < 3.0, "{avg}");
        }
    }

    #[test]
    fn the_sustain_pedal_holds_clip_notes_until_it_lifts() {
        use af27_core::midi::{plan_controls, plan_notes, ControlEvent, MidiClip, MidiNote, Tempo};
        // 120 BPM: a beat is half a second. The key is down for a beat;
        // the pedal goes down during it and comes up at beat 4.
        let mut c = MidiClip::new(1, "c", 0.0, 8.0);
        c.notes = vec![MidiNote {
            pitch: 60,
            start: 0.0,
            length: 1.0,
            velocity: 100,
            channel: 0,
            pan: 0.0,
        }];
        let cc = |at, value| ControlEvent {
            at,
            controller: 64,
            value,
            channel: 0,
        };
        c.controls = vec![cc(0.5, 1.0), cc(4.0, 0.0)];
        let tempo = Tempo::default();
        for inst in [
            Instrument::default(),
            af27_core::synth_model::SynthModel::PolyChorus.preset(),
        ] {
            let notes = plan_notes(std::slice::from_ref(&c), &tempo)
                .iter()
                .map(|n| note(n.start, n.end, n.pitch, 100, n.pan))
                .collect();
            let mut s = Synth::new(inst, notes, RATE)
                .with_controls(plan_controls(std::slice::from_ref(&c), &tempo));
            let (l, _) = render_all(&mut s, 4.0);
            let at =
                |a: f64, b: f64| peak(&l[(a * RATE as f64) as usize..(b * RATE as f64) as usize]);
            // Key up at 0.5 s; still sounding at 1.5 s with the pedal down.
            assert!(at(1.4, 1.6) > 0.02, "{:?}", inst.ext.model);
            // Pedal up at 2 s: gone after the release.
            assert!(at(3.0, 4.0) < 1e-3, "{:?}", inst.ext.model);
        }
    }

    #[test]
    fn mono_plays_one_voice_and_glides() {
        let mut inst = af27_core::synth_model::SynthModel::MonoLadder.preset();
        inst.ext.glide = 0.2;
        inst.ext.osc2_level = 0.0;
        inst.ext.sub = 0.0;
        inst.wave = Wave::Sine;
        inst.cutoff = 18_000.0;
        inst.ext.filter_env = 0.0;
        inst.ext.key_track = 0.0;
        inst.resonance = 0.0;
        inst.ext.drive = 0.0;
        inst.ext.filter_kind = af27_core::synth_model::FilterKind::Classic;
        // A held, then B played over it: one voice, gliding up to B.
        let notes = vec![note(0.0, 2.0, 57, 100, 0.0), note(1.0, 2.0, 69, 100, 0.0)];
        let mut s = Synth::new(inst, notes, RATE);
        let (l, _) = render_all(&mut s, 2.0);
        let f =
            |a: f64, b: f64| freq_of(&l[(a * RATE as f64) as usize..(b * RATE as f64) as usize]);
        assert!((f(0.4, 0.9) - 220.0).abs() < 3.0);
        let mid = f(1.02, 1.08);
        assert!(mid > 225.0 && mid < 430.0, "{mid}");
        assert!((f(1.6, 1.95) - 440.0).abs() < 3.0);
    }

    /// How fast the full engine runs at its heaviest (16 voices, 5-way
    /// unison, ladder, chorus): `cargo test --release -p af27-engine
    /// heaviest -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn heaviest_patch_speed() {
        let mut inst = af27_core::synth_model::SynthModel::BrassPoly.preset();
        inst.ext.unison = 5;
        inst.ext.chorus = 0.5;
        inst.ext.sub = 0.5;
        inst.ext.noise = 0.1;
        let notes = (0..16)
            .map(|k| note(0.0, 10.0, 48 + k * 2, 100, 0.0))
            .collect();
        let mut s = Synth::new(inst, notes, RATE);
        let t = std::time::Instant::now();
        let (l, _) = render_all(&mut s, 5.0);
        let e = t.elapsed().as_secs_f64();
        println!(
            "5 s of 16 voices x 5 unison in {e:.3} s ({:.0}x real time)",
            5.0 / e
        );
        assert!(peak(&l) > 0.0);
    }

    #[test]
    fn a_jump_into_a_held_note_sounds_at_once() {
        let inst = af27_core::synth_model::SynthModel::BrassPoly.preset();
        let mut s = Synth::new(inst, vec![note(0.0, 4.0, 60, 100, 0.0)], RATE);
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        s.render(2.0, DT, &mut l, &mut r);
        assert!(peak(&l) > 0.02);
        // And back before it: silent.
        l.fill(0.0);
        r.fill(0.0);
        s.render(-1.0, DT, &mut l, &mut r);
        assert!(peak(&l) == 0.0);
    }

    #[test]
    fn envelope_shape() {
        let i = Instrument {
            attack: 0.01,
            decay: 0.1,
            sustain: 0.5,
            release: 0.2,
            ..Instrument::default()
        };
        assert!((envelope(&i, 0.005, 1.0) - 0.5).abs() < 1e-5);
        assert!((envelope(&i, 0.5, 1.0) - 0.5).abs() < 1e-5);
        assert!((envelope(&i, 1.1, 1.0) - 0.25).abs() < 1e-5);
        assert_eq!(envelope(&i, 1.3, 1.0), 0.0);
    }

    #[test]
    fn a_note_sounds_and_stops() {
        let n = note(0.0, 0.5, 69, 100, 0.0);
        let mut s = Synth::new(Instrument::default(), vec![n], 48_000);
        let mut l = vec![0.0; 4800];
        let mut r = vec![0.0; 4800];
        s.render(0.1, 1.0 / 48_000.0, &mut l, &mut r);
        assert!(l.iter().any(|v| v.abs() > 0.01));
        let mut l = vec![0.0; 480];
        let mut r = vec![0.0; 480];
        s.reset();
        s.render(2.0, 1.0 / 48_000.0, &mut l, &mut r);
        assert!(l.iter().all(|v| v.abs() < 1e-6));
    }

    #[test]
    fn a_detuned_note_plays_off_its_key() {
        let n = note(0.0, 1.0, 69, 100, 0.0);
        assert_eq!(n.detuned(0.0).freq, n.freq);
        assert!((n.detuned(0.5).freq - 440.0 * 2f32.powf(0.5 / 12.0)).abs() < 1e-2);
        assert!((n.detuned(-0.25).freq - 440.0 * 2f32.powf(-0.25 / 12.0)).abs() < 1e-2);
    }

    #[test]
    fn clicks_land_on_beats() {
        let t = af27_core::midi::Tempo::default();
        let segs: Vec<_> = t.segments().collect();
        assert!(click(0.001, &segs).abs() > 0.0);
        assert_eq!(click(0.25, &segs), 0.0);
        assert!(click(0.5005, &segs).abs() > 0.0);
    }

    #[test]
    fn clicks_follow_the_tempo_map_with_the_bar_accented() {
        use af27_core::midi::{Tempo, TempoChange};
        // One bar of 4/4 at 120 (2 s), then 6/8 at 60: eighths every 0.5 s.
        let mut t = Tempo::default();
        t.changes.push(TempoChange::new(1, 60.0, 6, 8));
        let segs: Vec<_> = t.segments().collect();
        let at = |x: f64| click(x + 0.0005, &segs);
        // Accent (higher, louder) on each bar's first beat.
        let peak = |x: f64| {
            (0..400)
                .map(|i| click(x + i as f64 / 20_000.0, &segs).abs())
                .fold(0.0f32, f32::max)
        };
        assert!(peak(0.0) > peak(0.5) * 1.2);
        assert!(peak(2.0) > peak(2.5) * 1.2);
        assert!(at(2.5).abs() > 0.0 && at(3.0).abs() > 0.0);
        assert_eq!(click(2.25, &segs), 0.0);
        // Six eighths later (3 s into 6/8) the next bar is accented again.
        assert!(peak(5.0) > peak(4.5) * 1.2);
        assert!(peak(4.5) > 0.1);
    }

    #[test]
    fn clicks_follow_a_ritardando_beat_by_beat() {
        use af27_core::midi::{Tempo, TempoChange, TempoKind};
        // 120 to 60 BPM across bars 1..5, then steady.
        let mut t = Tempo::default();
        t.changes.push(TempoChange {
            kind: TempoKind::Ramp { from_bar: 1 },
            ..TempoChange::new(5, 60.0, 4, 4)
        });
        let segs: Vec<_> = t.segments().collect();
        for b in 4..24 {
            let at = t.secs(b as f64);
            assert!(click(at + 0.0005, &segs).abs() > 0.0, "beat {b}");
            // Halfway between two beats (in time) is silent.
            let mid = (at + t.secs(b as f64 + 1.0)) / 2.0;
            assert_eq!(click(mid, &segs), 0.0, "between {b} and {}", b + 1);
        }
    }
}
