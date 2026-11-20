// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The built-in MIDI instrument and the metronome click.
//!
//! Notes are known ahead (they come from MIDI clips), so each voice is
//! worked out from its note's own time: a band-limited (PolyBLEP)
//! oscillator plus a detuned second one, an ADSR envelope with its release
//! after the note ends, velocity and pan. Only the track's resonant
//! low-pass filter keeps state. Nothing allocates while playing.

use af27_core::midi::{
    note_freq, ClickSound, Instrument, MetronomeSettings, Segment, Subdivide, Wave,
};
use af27_core::time::db_to_gain;
use std::f32::consts::{PI, TAU};

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
}

pub struct Synth {
    pub inst: Instrument,
    /// Sorted by start.
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
        self.gain = db_to_gain(inst.gain_db);
        for f in &mut self.svf {
            f.tune(inst.cutoff, inst.resonance, self.rate);
        }
    }

    /// Adds the notes sounding over `n` frames from `t0` into `l` and `r`.
    pub fn render(&mut self, t0: f64, dt: f64, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        let t1 = t0 + dt * n as f64;
        let rel = self.inst.release.max(0.001) as f64;
        // Notes are sorted by start; stop once they start after this block.
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
                let env = envelope(&self.inst, local as f32, (note.end - note.start) as f32);
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

    pub fn reset(&mut self) {
        self.svf = [Svf::default(); 2];
        let inst = self.inst;
        self.set(inst);
    }
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
        Wave::Square => {
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
