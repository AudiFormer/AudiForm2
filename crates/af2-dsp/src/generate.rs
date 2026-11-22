// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Generated audio: tones, noise and silence.
//!
//! - [`tone`] and [`noise`]: the plain one-line forms.
//! - [`noise_with`]: coloured noise (white, pink, brown, grey, blue,
//!   violet) shaped exactly on the spectrum, in four stereo styles, at an
//!   RMS level with a DC offset.
//! - [`tones`]: a base frequency with up to five frequency components
//!   (multiples of the base), frequency modulation, a waveform shape, and
//!   a sweep from a start setting to an end setting over the duration.

use af2_core::AudioBuffer;
use rustfft::num_complex::Complex;
use std::f64::consts::TAU;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wave {
    Sine,
    Square,
    Triangle,
    Saw,
    /// A falling saw.
    InverseSaw,
}

impl Wave {
    pub const ALL: [Wave; 5] = [
        Wave::Sine,
        Wave::Triangle,
        Wave::Square,
        Wave::Saw,
        Wave::InverseSaw,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Wave::Sine => "Sine",
            Wave::Square => "Square",
            Wave::Triangle => "Triangle",
            Wave::Saw => "Saw",
            Wave::InverseSaw => "Inverse Saw",
        }
    }

    /// Whether the shape control changes this wave (the square's duty
    /// cycle, the triangle's symmetry).
    pub fn has_shape(self) -> bool {
        matches!(self, Wave::Square | Wave::Triangle)
    }

    /// One cycle's value at phase `ph` (0..1); `shape` 0..1 (0.5 is the
    /// plain wave).
    pub fn at(self, ph: f64, shape: f64) -> f64 {
        let ph = ph - ph.floor();
        match self {
            Wave::Sine => (TAU * ph).sin(),
            Wave::Square => {
                let duty = shape.clamp(0.01, 0.99);
                if ph < duty {
                    1.0
                } else {
                    -1.0
                }
            }
            Wave::Triangle => {
                // Rises for `peak` of the cycle and falls for the rest,
                // starting at zero going up like a sine.
                let peak = shape.clamp(0.01, 0.99);
                let p = (ph + peak / 2.0).fract();
                if p < peak {
                    -1.0 + 2.0 * p / peak
                } else {
                    1.0 - 2.0 * (p - peak) / (1.0 - peak)
                }
            }
            Wave::Saw => 2.0 * (ph + 0.5).fract() - 1.0,
            Wave::InverseSaw => 1.0 - 2.0 * (ph + 0.5).fract(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Noise {
    White,
    Pink,
    Brown,
    /// Loud where the ear is less sensitive (an inverted A-weighting), so
    /// it sounds about equally loud at every frequency.
    Grey,
    Blue,
    Violet,
}

impl Noise {
    pub const ALL: [Noise; 6] = [
        Noise::White,
        Noise::Pink,
        Noise::Brown,
        Noise::Grey,
        Noise::Blue,
        Noise::Violet,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Noise::White => "White",
            Noise::Pink => "Pink",
            Noise::Brown => "Brown",
            Noise::Grey => "Grey",
            Noise::Blue => "Blue",
            Noise::Violet => "Violet",
        }
    }

    /// Power spectral density at `hz`, in dB relative to 1 kHz.
    pub fn psd_db(self, hz: f64) -> f64 {
        let f = hz.max(10.0);
        let oct = (f / 1000.0).log2();
        match self {
            Noise::White => 0.0,
            Noise::Pink => -3.0103 * oct,
            Noise::Brown => -6.0206 * oct,
            Noise::Blue => 3.0103 * oct,
            Noise::Violet => 6.0206 * oct,
            Noise::Grey => (-a_weight_db(f)).min(GREY_MAX_DB),
        }
    }
}

/// The most grey noise lifts the deep end (the inverted A-weighting goes
/// on rising below 50 Hz).
pub const GREY_MAX_DB: f64 = 30.0;

/// The A-weighting curve in dB (0 at 1 kHz).
pub fn a_weight_db(f: f64) -> f64 {
    let f2 = f * f;
    let ra = (12194.0f64.powi(2) * f2 * f2)
        / ((f2 + 20.6f64.powi(2))
            * ((f2 + 107.7f64.powi(2)) * (f2 + 737.9f64.powi(2))).sqrt()
            * (f2 + 12194.0f64.powi(2)));
    20.0 * ra.log10() + 2.0
}

/// How the channels of generated noise relate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoiseStyle {
    /// One noise reaching each channel a little later than the one before
    /// (milliseconds): a wide, airy image.
    Spatial(f64),
    /// A noise of its own on every channel.
    Independent,
    /// The same noise on every channel.
    Mono,
    /// The same noise, upside down on every other channel.
    Inverse,
}

/// Everything [`noise_with`] needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoiseSpec {
    pub color: Noise,
    pub style: NoiseStyle,
    /// RMS level, dBFS.
    pub level_db: f64,
    /// Added to every sample, as a percentage of full scale.
    pub dc_percent: f64,
    pub seed: u32,
}

impl Default for NoiseSpec {
    fn default() -> Self {
        NoiseSpec {
            color: Noise::Pink,
            style: NoiseStyle::Independent,
            level_db: -20.0,
            dc_percent: 0.0,
            seed: 0x9e37_79b9,
        }
    }
}

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (f64::from(self.0) + 0.5) / 4_294_967_296.0
    }
    /// A normal (Gaussian) pair.
    fn gauss(&mut self) -> (f64, f64) {
        let (u, v) = (self.next(), self.next());
        let r = (-2.0 * u.ln()).sqrt();
        (r * (TAU * v).cos(), r * (TAU * v).sin())
    }
}

/// Noise of one colour, `frames` long, with its spectrum shaped exactly:
/// random-phase blocks with the colour's magnitude, joined by 50%
/// overlapping sine windows (whose squares add to one, so the level stays
/// steady). Unscaled.
fn coloured(rate: u32, frames: usize, color: Noise, seed: u32) -> Vec<f64> {
    const N: usize = 8192;
    const HOP: usize = N / 2;
    let mut out = vec![0.0f64; frames + N];
    let mut rng = Rng(seed.max(1));
    let mag: Vec<f64> = (0..N)
        .map(|k| {
            let bin = k.min(N - k);
            if bin == 0 {
                return 0.0;
            }
            let hz = bin as f64 * rate as f64 / N as f64;
            10f64.powf(color.psd_db(hz) / 20.0)
        })
        .collect();
    let win: Vec<f64> = (0..N)
        .map(|i| (std::f64::consts::PI * (i as f64 + 0.5) / N as f64).sin())
        .collect();
    let mut planner = rustfft::FftPlanner::<f64>::new();
    let ifft = planner.plan_fft_inverse(N);
    let mut buf = vec![Complex::new(0.0, 0.0); N];
    // Each inverse transform gives two blocks: the real and the imaginary
    // parts are independent noises with the same spectrum.
    let mut at = 0usize;
    let starts = frames.div_ceil(HOP) + 2;
    let mut k = 0;
    while k < starts {
        for (b, m) in buf.iter_mut().zip(&mag) {
            let (g1, g2) = rng.gauss();
            *b = Complex::new(g1 * m, g2 * m);
        }
        ifft.process(&mut buf);
        for part in 0..2 {
            if k >= starts {
                break;
            }
            for i in 0..N {
                if at + i >= out.len() {
                    break;
                }
                let v = if part == 0 { buf[i].re } else { buf[i].im };
                out[at + i] += v * win[i];
            }
            at += HOP;
            k += 1;
        }
    }
    // The first half block only has one window over it: start after it.
    out.drain(..HOP.min(out.len()));
    out.truncate(frames);
    out
}

/// Coloured noise in a stereo style, at an RMS level, with a DC offset.
pub fn noise_with(rate: u32, channels: usize, secs: f64, spec: &NoiseSpec) -> AudioBuffer {
    let frames = (secs.max(0.0) * rate as f64) as usize;
    let channels = channels.max(1);
    let delay = match spec.style {
        NoiseStyle::Spatial(ms) => (ms.max(0.0) / 1000.0 * rate as f64) as usize,
        _ => 0,
    };
    let make = |seed: u32, len: usize| coloured(rate, len, spec.color, seed);
    let mut chans: Vec<Vec<f64>> = match spec.style {
        NoiseStyle::Independent => (0..channels)
            .map(|c| {
                make(
                    spec.seed
                        .wrapping_add(0x6d2b_79f5u32.wrapping_mul(c as u32 + 1)),
                    frames,
                )
            })
            .collect(),
        NoiseStyle::Mono | NoiseStyle::Inverse => {
            let one = make(spec.seed, frames);
            (0..channels)
                .map(|c| {
                    if spec.style == NoiseStyle::Inverse && c % 2 == 1 {
                        one.iter().map(|v| -v).collect()
                    } else {
                        one.clone()
                    }
                })
                .collect()
        }
        NoiseStyle::Spatial(_) => {
            let src = make(spec.seed, frames + delay * channels);
            (0..channels)
                .map(|c| {
                    // Each channel hears the source one delay after the
                    // channel before it.
                    let off = delay * (channels - 1 - c);
                    src[off..off + frames].to_vec()
                })
                .collect()
        }
    };
    let rms = {
        let n = chans.iter().map(|c| c.len()).sum::<usize>().max(1);
        (chans.iter().flatten().map(|v| v * v).sum::<f64>() / n as f64).sqrt()
    };
    let g = if rms > 1e-12 {
        10f64.powf(spec.level_db / 20.0) / rms
    } else {
        0.0
    };
    let dc = spec.dc_percent / 100.0;
    for c in &mut chans {
        for v in c.iter_mut() {
            *v = (*v * g + dc).clamp(-1.0, 1.0);
        }
    }
    AudioBuffer {
        rate,
        channels: chans
            .into_iter()
            .map(|c| c.into_iter().map(|v| v as f32).collect())
            .collect(),
    }
}

/// How many frequency components a tone has.
pub const COMPONENTS: usize = 5;

/// One frequency component: `multiplier` × the base frequency.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Component {
    pub on: bool,
    /// dBFS (-120 is silent).
    pub amp_db: f64,
    pub multiplier: f64,
}

/// One end of a tone (the start, or the end it sweeps to).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneSet {
    /// Hz.
    pub base: f64,
    /// Frequency modulation: how far (Hz) and how fast (Hz).
    pub mod_depth: f64,
    pub mod_rate: f64,
    pub components: [Component; COMPONENTS],
    /// dB over all the components.
    pub volume_db: f64,
}

impl Default for ToneSet {
    fn default() -> Self {
        let mut components = [Component {
            on: false,
            amp_db: -12.0,
            multiplier: 1.0,
        }; COMPONENTS];
        for (k, c) in components.iter_mut().enumerate() {
            c.multiplier = (k + 1) as f64;
        }
        components[0].on = true;
        components[0].amp_db = 0.0;
        ToneSet {
            base: 440.0,
            mod_depth: 0.0,
            mod_rate: 0.0,
            components,
            volume_db: -6.0,
        }
    }
}

/// Everything [`tones`] needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneSpec {
    pub start: ToneSet,
    /// Sweep: the start glides to this over the duration (frequencies on
    /// a log scale, levels in dB).
    pub end: Option<ToneSet>,
    pub wave: Wave,
    /// 0..1, 0.5 plain (see [`Wave::at`]).
    pub shape: f64,
    pub dc_percent: f64,
    /// Where every component starts, degrees.
    pub phase_deg: f64,
    /// Each channel starts this far (degrees) after the one before.
    pub channel_phase_deg: f64,
}

impl Default for ToneSpec {
    fn default() -> Self {
        ToneSpec {
            start: ToneSet::default(),
            end: None,
            wave: Wave::Sine,
            shape: 0.5,
            dc_percent: 0.0,
            phase_deg: 0.0,
            channel_phase_deg: 0.0,
        }
    }
}

fn mix(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn mix_log(a: f64, b: f64, t: f64) -> f64 {
    if a > 0.0 && b > 0.0 {
        a * (b / a).powf(t)
    } else {
        mix(a, b, t)
    }
}

fn gain(db: f64) -> f64 {
    if db <= -120.0 {
        0.0
    } else {
        10f64.powf(db / 20.0)
    }
}

/// The tone's setting `t` (0..1) of the way through.
pub fn tone_at(spec: &ToneSpec, t: f64) -> ToneSet {
    let Some(e) = spec.end else {
        return spec.start;
    };
    let s = spec.start;
    let mut c = s.components;
    for (k, x) in c.iter_mut().enumerate() {
        let y = e.components[k];
        x.on = x.on || y.on;
        let (a, b) = (
            if s.components[k].on { x.amp_db } else { -120.0 },
            if y.on { y.amp_db } else { -120.0 },
        );
        x.amp_db = mix(a, b, t);
        x.multiplier = mix_log(x.multiplier, y.multiplier, t);
    }
    ToneSet {
        base: mix_log(s.base, e.base, t),
        mod_depth: mix(s.mod_depth, e.mod_depth, t),
        mod_rate: mix_log(s.mod_rate, e.mod_rate, t),
        components: c,
        volume_db: mix(s.volume_db, e.volume_db, t),
    }
}

/// A tone with frequency components, modulation and an optional sweep.
pub fn tones(rate: u32, channels: usize, secs: f64, spec: &ToneSpec) -> AudioBuffer {
    let frames = (secs.max(0.0) * rate as f64) as usize;
    let dc = spec.dc_percent / 100.0;
    // Channels that start at the same phase share one rendering.
    let mut made: Vec<(f64, Vec<f32>)> = Vec::new();
    let chans = (0..channels.max(1))
        .map(|ch| {
            let shift =
                (spec.phase_deg + spec.channel_phase_deg * ch as f64).rem_euclid(360.0) / 360.0;
            if let Some((_, v)) = made.iter().find(|(s, _)| (s - shift).abs() < 1e-12) {
                return v.clone();
            }
            let v: Vec<f32> = render(rate, frames, spec, shift)
                .into_iter()
                .map(|v| (v + dc).clamp(-1.0, 1.0) as f32)
                .collect();
            made.push((shift, v.clone()));
            v
        })
        .collect();
    AudioBuffer {
        rate,
        channels: chans,
    }
}

/// One channel of [`tones`] with every component starting `shift` of its
/// cycle on (no DC, unclipped). Settings are worked out every 64 frames.
fn render(rate: u32, frames: usize, spec: &ToneSpec, shift: f64) -> Vec<f64> {
    const STEP: usize = 64;
    let r = rate as f64;
    let mut ph = [shift; COMPONENTS];
    let mut mod_ph = 0.0f64;
    let mut out = vec![0.0f64; frames];
    let mut at = 0;
    while at < frames {
        let n = STEP.min(frames - at);
        let set = tone_at(spec, at as f64 / frames.max(1) as f64);
        let vol = gain(set.volume_db);
        let amps: [f64; COMPONENTS] = std::array::from_fn(|k| {
            if set.components[k].on {
                gain(set.components[k].amp_db)
            } else {
                0.0
            }
        });
        for o in &mut out[at..at + n] {
            let f = (set.base + set.mod_depth * (TAU * mod_ph).sin()).max(0.0);
            let mut v = 0.0;
            for (k, c) in set.components.iter().enumerate() {
                if amps[k] > 0.0 {
                    v += amps[k] * spec.wave.at(ph[k], spec.shape);
                }
                ph[k] = (ph[k] + f * c.multiplier / r).fract();
            }
            *o = v * vol;
            mod_ph = (mod_ph + set.mod_rate / r).fract();
        }
        at += n;
    }
    out
}

pub fn tone(
    rate: u32,
    channels: usize,
    secs: f64,
    freq: f64,
    amp_db: f32,
    wave: Wave,
) -> AudioBuffer {
    let frames = (secs * rate as f64) as usize;
    let amp = af2_core::time::db_to_gain(amp_db);
    let data: Vec<f32> = (0..frames)
        .map(|i| {
            let ph = (freq * i as f64 / rate as f64).fract();
            let v = match wave {
                Wave::Sine => (TAU * ph).sin(),
                Wave::Square => {
                    if ph < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                Wave::Triangle => 1.0 - 4.0 * (ph - 0.5).abs(),
                Wave::Saw => 2.0 * ph - 1.0,
                Wave::InverseSaw => 1.0 - 2.0 * ph,
            };
            v as f32 * amp
        })
        .collect();
    AudioBuffer {
        rate,
        channels: vec![data; channels.max(1)],
    }
}

pub fn noise(rate: u32, channels: usize, secs: f64, amp_db: f32, kind: Noise) -> AudioBuffer {
    let frames = (secs * rate as f64) as usize;
    let amp = af2_core::time::db_to_gain(amp_db);
    if !matches!(kind, Noise::White | Noise::Pink | Noise::Brown) {
        // The other colours are shaped on the spectrum, at a level near
        // the old forms' (about 10 dB under the peak).
        let spec = NoiseSpec {
            color: kind,
            level_db: f64::from(amp_db) - 10.0,
            ..NoiseSpec::default()
        };
        return noise_with(rate, channels, secs, &spec);
    }
    let mut seed = 0x9e37_79b9_u32;
    let chans = (0..channels.max(1))
        .map(|c| {
            seed = seed.wrapping_add(0x6d2b_79f5 * (c as u32 + 1));
            let mut rng = move || {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
            };
            // Pink: Paul Kellet's economy filter on white noise.
            let (mut b0, mut b1, mut b2) = (0.0f32, 0.0f32, 0.0f32);
            let mut brown = 0.0f32;
            (0..frames)
                .map(|_| {
                    let w = rng();
                    let v = match kind {
                        Noise::Pink => {
                            b0 = 0.99765 * b0 + w * 0.099_046;
                            b1 = 0.96300 * b1 + w * 0.296_516_4;
                            b2 = 0.57000 * b2 + w * 1.052_691_3;
                            (b0 + b1 + b2 + w * 0.1848) * 0.25
                        }
                        Noise::Brown => {
                            brown = (brown + w * 0.02).clamp(-1.0, 1.0) * 0.998;
                            brown * 3.0
                        }
                        _ => w,
                    };
                    v.clamp(-1.0, 1.0) * amp
                })
                .collect()
        })
        .collect();
    AudioBuffer {
        rate,
        channels: chans,
    }
}

pub fn silence(rate: u32, channels: usize, secs: f64) -> AudioBuffer {
    AudioBuffer::new(rate, channels, (secs * rate as f64) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_levels() {
        let t = tone(48_000, 2, 1.0, 440.0, -6.0, Wave::Sine);
        assert_eq!(t.frames(), 48_000);
        assert!((t.peak() - 0.501).abs() < 0.01);
        let n = noise(48_000, 1, 1.0, -12.0, Noise::Pink);
        assert!(n.peak() <= 0.252);
    }

    /// Power in a band, averaged over Hann-windowed frames.
    fn band_power(x: &[f32], rate: u32, lo: f64, hi: f64) -> f64 {
        const N: usize = 4096;
        let mut planner = rustfft::FftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(N);
        let win = crate::fft::hann(N);
        let mut sum = 0.0;
        let mut frames = 0;
        let mut at = 0;
        while at + N <= x.len() {
            let mut b: Vec<Complex<f64>> = (0..N)
                .map(|i| Complex::new(f64::from(x[at + i]) * win[i], 0.0))
                .collect();
            fft.process(&mut b);
            for (k, v) in b.iter().enumerate().take(N / 2) {
                let hz = k as f64 * rate as f64 / N as f64;
                if hz >= lo && hz < hi {
                    sum += v.norm_sqr();
                }
            }
            frames += 1;
            at += N / 2;
        }
        sum / frames.max(1) as f64
    }

    fn peak_hz(x: &[f32], rate: u32) -> f64 {
        let n = x.len().next_power_of_two() / 2;
        let mut planner = rustfft::FftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(n);
        let win = crate::fft::hann(n);
        let mut b: Vec<Complex<f64>> = (0..n)
            .map(|i| Complex::new(f64::from(x[i]) * win[i], 0.0))
            .collect();
        fft.process(&mut b);
        let k = (1..n / 2)
            .max_by(|a, c| b[*a].norm_sqr().total_cmp(&b[*c].norm_sqr()))
            .unwrap();
        k as f64 * rate as f64 / n as f64
    }

    /// Each colour's slope per octave, measured from 500 Hz-1 kHz to
    /// 4-8 kHz (three octaves).
    #[test]
    fn every_noise_colour_has_its_spectral_slope() {
        for (color, per_oct) in [
            (Noise::White, 0.0),
            (Noise::Pink, -3.0),
            (Noise::Brown, -6.0),
            (Noise::Blue, 3.0),
            (Noise::Violet, 6.0),
        ] {
            let spec = NoiseSpec {
                color,
                level_db: -20.0,
                ..NoiseSpec::default()
            };
            let b = noise_with(48_000, 1, 4.0, &spec);
            // Per-Hz density: band power over band width.
            let lo = band_power(&b.channels[0], 48_000, 500.0, 1000.0) / 500.0;
            let hi = band_power(&b.channels[0], 48_000, 4000.0, 8000.0) / 4000.0;
            let slope = 10.0 * (hi / lo).log10() / 3.0;
            assert!(
                (slope - per_oct).abs() < 0.6,
                "{color:?}: {slope:.2} dB/oct, wanted {per_oct}"
            );
            let rms = (b.channels[0]
                .iter()
                .map(|v| f64::from(*v).powi(2))
                .sum::<f64>()
                / b.frames() as f64)
                .sqrt();
            assert!((20.0 * rms.log10() + 20.0).abs() < 0.3, "{color:?} level");
        }
        // Grey: louder deep down and at the top than in the middle.
        let b = noise_with(
            48_000,
            1,
            4.0,
            &NoiseSpec {
                color: Noise::Grey,
                ..NoiseSpec::default()
            },
        );
        let low = band_power(&b.channels[0], 48_000, 60.0, 120.0) / 60.0;
        let mid = band_power(&b.channels[0], 48_000, 2000.0, 4000.0) / 2000.0;
        assert!(low > mid * 10.0);
    }

    #[test]
    fn noise_styles_and_dc_offset() {
        let base = NoiseSpec {
            color: Noise::White,
            dc_percent: 10.0,
            level_db: -30.0,
            ..NoiseSpec::default()
        };
        let mono = noise_with(
            48_000,
            2,
            0.5,
            &NoiseSpec {
                style: NoiseStyle::Mono,
                ..base
            },
        );
        assert_eq!(mono.channels[0], mono.channels[1]);
        let mean =
            mono.channels[0].iter().map(|v| f64::from(*v)).sum::<f64>() / mono.frames() as f64;
        assert!((mean - 0.1).abs() < 0.01, "{mean}");
        let inv = noise_with(
            48_000,
            2,
            0.5,
            &NoiseSpec {
                style: NoiseStyle::Inverse,
                dc_percent: 0.0,
                ..base
            },
        );
        assert!(inv.channels[0]
            .iter()
            .zip(&inv.channels[1])
            .all(|(a, b)| (a + b).abs() < 1e-6));
        let ind = noise_with(
            48_000,
            2,
            0.5,
            &NoiseSpec {
                style: NoiseStyle::Independent,
                ..base
            },
        );
        assert_ne!(ind.channels[0], ind.channels[1]);
        // Spatial: the right channel is the left one, 10 ms later.
        let sp = noise_with(
            48_000,
            2,
            0.5,
            &NoiseSpec {
                style: NoiseStyle::Spatial(10.0),
                dc_percent: 0.0,
                ..base
            },
        );
        let d = 480;
        assert!(sp.channels[1][d..]
            .iter()
            .zip(&sp.channels[0])
            .all(|(a, b)| (a - b).abs() < 1e-6));
        assert_eq!(sp.frames(), 24_000);
    }

    #[test]
    fn tone_components_sit_at_base_times_multiplier() {
        let mut spec = ToneSpec::default();
        spec.start.base = 300.0;
        spec.start.components[0].on = false;
        spec.start.components[2] = Component {
            on: true,
            amp_db: -6.0,
            multiplier: 3.0,
        };
        let b = tones(48_000, 1, 1.0, &spec);
        let hz = peak_hz(&b.channels[0], 48_000);
        assert!((hz - 900.0).abs() < 6.0, "{hz}");
        // Two components: both there, at their levels.
        spec.start.components[0] = Component {
            on: true,
            amp_db: 0.0,
            multiplier: 1.0,
        };
        let b = tones(48_000, 1, 1.0, &spec);
        let p1 = band_power(&b.channels[0], 48_000, 280.0, 320.0);
        let p3 = band_power(&b.channels[0], 48_000, 880.0, 920.0);
        let diff = 10.0 * (p1 / p3).log10();
        assert!((diff - 6.0).abs() < 0.5, "{diff}");
    }

    #[test]
    fn a_sweep_ends_at_the_end_frequency() {
        let mut spec = ToneSpec::default();
        spec.start.base = 200.0;
        let mut end = spec.start;
        end.base = 2000.0;
        spec.end = Some(end);
        let b = tones(48_000, 1, 2.0, &spec);
        let x = &b.channels[0];
        let tail = &x[x.len() - 4800..];
        let ups = tail
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        // 0.1 s of about 2 kHz (just under, as it is still arriving).
        assert!((185..=201).contains(&ups), "{ups}");
        let head = &x[..4800];
        let ups = head
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        assert!((19..=22).contains(&ups), "{ups}");
    }

    #[test]
    fn tone_dc_offset_phase_and_waves() {
        let spec = ToneSpec {
            dc_percent: -20.0,
            channel_phase_deg: 180.0,
            ..ToneSpec::default()
        };
        let b = tones(48_000, 2, 1.0, &spec);
        let mean = b.channels[0].iter().map(|v| f64::from(*v)).sum::<f64>() / b.frames() as f64;
        assert!((mean + 0.2).abs() < 0.005, "{mean}");
        // Half a cycle apart: the channels mirror around the offset.
        for i in (0..48_000).step_by(997) {
            let (l, r) = (b.channels[0][i] + 0.2, b.channels[1][i] + 0.2);
            assert!((l + r).abs() < 1e-3, "{i}: {l} {r}");
        }
        for w in Wave::ALL {
            for k in 0..100 {
                let v = w.at(k as f64 / 100.0, 0.5);
                assert!((-1.0..=1.0).contains(&v), "{w:?}");
            }
        }
        assert!(Wave::Saw.at(0.25, 0.5) > 0.0 && Wave::InverseSaw.at(0.25, 0.5) < 0.0);
        assert!(Wave::Square.at(0.3, 0.25) < 0.0 && Wave::Square.at(0.2, 0.25) > 0.0);
    }

    #[test]
    fn frequency_modulation_moves_the_pitch() {
        let mut spec = ToneSpec::default();
        spec.start.base = 1000.0;
        spec.start.mod_depth = 200.0;
        spec.start.mod_rate = 5.0;
        let b = tones(48_000, 1, 1.0, &spec);
        let wide = band_power(&b.channels[0], 48_000, 1100.0, 1300.0);
        let plain = tones(
            48_000,
            1,
            1.0,
            &ToneSpec {
                start: ToneSet {
                    base: 1000.0,
                    ..ToneSet::default()
                },
                ..ToneSpec::default()
            },
        );
        let narrow = band_power(&plain.channels[0], 48_000, 1100.0, 1300.0);
        assert!(wide > narrow * 100.0);
    }
}
