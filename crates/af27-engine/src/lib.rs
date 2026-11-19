// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) The Audiform27 contributors.

//! Real-time playback and recording.
//!
//! The UI builds an immutable [`Program`] (a file to play, or a
//! multitrack mix) and hands it to the audio thread over a lock-free
//! queue. The audio thread never allocates, locks or frees: programs it
//! is done with go back over a second queue and are dropped by
//! [`Engine::collect`] on the UI thread. Position and meters come back
//! through atomics.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use af27_core::time::db_to_gain;
use af27_core::AudioBuffer;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// One clip as the mixer sees it.
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

#[derive(Clone)]
pub struct MixTrack {
    pub gain_l: f32,
    pub gain_r: f32,
    pub clips: Vec<MixClip>,
}

/// What the audio thread plays.
pub enum Program {
    /// One buffer (the Waveform editor). `channel_mask` bit 0 is left,
    /// bit 1 right, for listening to one side.
    Buffer {
        buf: Arc<AudioBuffer>,
        channel_mask: u8,
    },
    Mix {
        master: f32,
        tracks: Vec<MixTrack>,
        duration: f64,
    },
}

enum Cmd {
    Program(Arc<Program>),
    /// Play from `from` to `to` seconds, looping if asked.
    Play {
        from: f64,
        to: f64,
        looping: bool,
    },
    Stop,
}

struct Shared {
    playing: AtomicBool,
    /// Playhead in seconds (f64 bits).
    pos: AtomicU64,
    peak_l: AtomicU32,
    peak_r: AtomicU32,
}

fn store_f64(a: &AtomicU64, v: f64) {
    a.store(v.to_bits(), Ordering::Relaxed);
}
fn load_f64(a: &AtomicU64) -> f64 {
    f64::from_bits(a.load(Ordering::Relaxed))
}
fn store_max_f32(a: &AtomicU32, v: f32) {
    let cur = f32::from_bits(a.load(Ordering::Relaxed));
    if v > cur {
        a.store(v.to_bits(), Ordering::Relaxed);
    }
}

pub struct Engine {
    _stream: cpal::Stream,
    tx: rtrb::Producer<Cmd>,
    trash: rtrb::Consumer<Arc<Program>>,
    shared: Arc<Shared>,
    pub rate: u32,
    pub device: String,
    recorder: Option<Recorder>,
}

/// Linear interpolation into a channel at fractional index `x`.
#[inline]
fn sample(c: &[f32], x: f64) -> f32 {
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

/// One frame as stereo at fractional index `x`. Mono plays on both
/// sides; more than two channels fold down in WAV channel order
/// (L R C LFE, then surround pairs) with the centre and surrounds at
/// -3 dB, LFE left out, scaled so a full-scale bed does not clip.
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
            let norm = 1.0 / (1.0 + H + H * pairs.max(0.0));
            (l * norm, r * norm)
        }
    }
}

struct Voice {
    program: Option<Arc<Program>>,
    playing: bool,
    t: f64,
    from: f64,
    to: f64,
    looping: bool,
    /// Indices of clips active in the current block, per track.
    active: [(u16, u16); 512],
}

impl Voice {
    fn render(&mut self, out: &mut [f32], channels: usize, rate: f64) -> (f32, f32) {
        out.iter_mut().for_each(|s| *s = 0.0);
        let (mut pk_l, mut pk_r) = (0.0f32, 0.0f32);
        let Some(program) = self.program.clone() else {
            return (0.0, 0.0);
        };
        if !self.playing {
            return (0.0, 0.0);
        }
        let dt = 1.0 / rate;
        let frames = out.len() / channels;
        // Pick the clips touching this block once, not per sample.
        let mut n_active = 0usize;
        if let Program::Mix { tracks, .. } = &*program {
            let t1 = self.t + frames as f64 * dt;
            'outer: for (ti, tr) in tracks.iter().enumerate() {
                for (ci, c) in tr.clips.iter().enumerate() {
                    let wraps = self.looping && t1 > self.to;
                    if wraps || (c.start < t1 && c.start + c.length > self.t) {
                        if n_active == self.active.len() {
                            break 'outer;
                        }
                        self.active[n_active] = (ti as u16, ci as u16);
                        n_active += 1;
                    }
                }
            }
        }
        for f in 0..frames {
            if self.t >= self.to {
                if self.looping && self.to > self.from {
                    self.t = self.from;
                } else {
                    self.playing = false;
                    break;
                }
            }
            let (l, r) = match &*program {
                Program::Buffer { buf, channel_mask } => {
                    let (l, r) = stereo_frame(buf, self.t * buf.rate as f64);
                    let l = if channel_mask & 1 != 0 { l } else { 0.0 };
                    let r = if channel_mask & 2 != 0 { r } else { 0.0 };
                    (l, r)
                }
                Program::Mix { master, tracks, .. } => {
                    let (mut l, mut r) = (0.0f32, 0.0f32);
                    for &(ti, ci) in &self.active[..n_active] {
                        let tr = &tracks[ti as usize];
                        let c = &tr.clips[ci as usize];
                        let local = self.t - c.start;
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
                        let x = (c.offset + local) * c.buf.rate as f64;
                        let (a, b) = stereo_frame(&c.buf, x);
                        l += a * g as f32 * tr.gain_l;
                        r += b * g as f32 * tr.gain_r;
                    }
                    (l * master, r * master)
                }
            };
            pk_l = pk_l.max(l.abs());
            pk_r = pk_r.max(r.abs());
            let frame = &mut out[f * channels..(f + 1) * channels];
            match channels {
                1 => frame[0] = (l + r) * 0.5,
                _ => {
                    frame[0] = l;
                    frame[1] = r;
                }
            }
            self.t += dt;
        }
        (pk_l, pk_r)
    }
}

impl Engine {
    /// Opens the default output device.
    pub fn new() -> Result<Engine, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("No audio output device was found.")?;
        let name = device.name().unwrap_or_else(|_| "Output".into());
        let config = device
            .default_output_config()
            .map_err(|e| format!("The output device gave no format: {e}"))?;
        let rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let (tx, mut rx) = rtrb::RingBuffer::<Cmd>::new(256);
        let (mut trash_tx, trash) = rtrb::RingBuffer::<Arc<Program>>::new(256);
        let shared = Arc::new(Shared {
            playing: AtomicBool::new(false),
            pos: AtomicU64::new(0f64.to_bits()),
            peak_l: AtomicU32::new(0),
            peak_r: AtomicU32::new(0),
        });
        let sh = shared.clone();
        let mut voice = Voice {
            program: None,
            playing: false,
            t: 0.0,
            from: 0.0,
            to: 0.0,
            looping: false,
            active: [(0, 0); 512],
        };
        let mut callback = move |out: &mut [f32]| {
            while let Ok(cmd) = rx.pop() {
                match cmd {
                    Cmd::Program(p) => {
                        if let Some(old) = voice.program.replace(p) {
                            // If the trash queue is full the old program
                            // leaks rather than being freed here.
                            if let Err(rtrb::PushError::Full(old)) = trash_tx.push(old) {
                                std::mem::forget(old);
                            }
                        }
                    }
                    Cmd::Play { from, to, looping } => {
                        voice.t = from;
                        voice.from = from;
                        voice.to = to;
                        voice.looping = looping;
                        voice.playing = true;
                    }
                    Cmd::Stop => voice.playing = false,
                }
            }
            let (l, r) = voice.render(out, channels, rate as f64);
            sh.playing.store(voice.playing, Ordering::Relaxed);
            store_f64(&sh.pos, voice.t);
            store_max_f32(&sh.peak_l, l);
            store_max_f32(&sh.peak_r, r);
        };
        let err = |e| eprintln!("audio output error: {e}");
        let cfg: cpal::StreamConfig = config.clone().into();
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                device.build_output_stream(&cfg, move |d: &mut [f32], _| callback(d), err, None)
            }
            cpal::SampleFormat::I16 => {
                let mut tmp = vec![0.0f32; 1 << 16];
                device.build_output_stream(
                    &cfg,
                    move |d: &mut [i16], _| {
                        let n = d.len().min(tmp.len());
                        callback(&mut tmp[..n]);
                        for (o, s) in d.iter_mut().zip(&tmp[..n]) {
                            *o = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                        }
                    },
                    err,
                    None,
                )
            }
            other => return Err(format!("Unsupported output sample format {other:?}.")),
        }
        .map_err(|e| format!("Could not open the audio output: {e}"))?;
        stream
            .play()
            .map_err(|e| format!("Could not start audio: {e}"))?;
        Ok(Engine {
            _stream: stream,
            tx,
            trash,
            shared,
            rate,
            device: name,
            recorder: None,
        })
    }

    pub fn set_program(&mut self, p: Program) {
        let _ = self.tx.push(Cmd::Program(Arc::new(p)));
    }

    /// Plays from `from` up to `to` (None = the end).
    pub fn play(&mut self, from: f64, to: Option<f64>, looping: bool, program_len: f64) {
        let to = to.unwrap_or(program_len).max(from);
        let _ = self.tx.push(Cmd::Play { from, to, looping });
        self.shared.playing.store(true, Ordering::Relaxed);
    }

    pub fn stop(&mut self) {
        let _ = self.tx.push(Cmd::Stop);
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    pub fn position(&self) -> f64 {
        load_f64(&self.shared.pos)
    }

    /// Peak levels since the last call (left, right), linear.
    pub fn take_peaks(&self) -> (f32, f32) {
        let l = f32::from_bits(self.shared.peak_l.swap(0, Ordering::Relaxed));
        let r = f32::from_bits(self.shared.peak_r.swap(0, Ordering::Relaxed));
        (l, r)
    }

    /// Frees programs the audio thread has finished with. Call every frame.
    pub fn collect(&mut self) {
        while self.trash.pop().is_ok() {}
        if let Some(r) = &mut self.recorder {
            r.drain();
        }
    }

    pub fn start_recording(&mut self) -> Result<(), String> {
        self.recorder = Some(Recorder::new()?);
        Ok(())
    }

    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// Recorded so far, for a live waveform.
    pub fn recording_len_secs(&self) -> f64 {
        self.recorder.as_ref().map_or(0.0, |r| {
            r.data.len() as f64 / (r.channels.max(1) as f64 * r.rate.max(1) as f64)
        })
    }

    pub fn recording_level(&self) -> f32 {
        self.recorder
            .as_ref()
            .map_or(0.0, |r| f32::from_bits(r.peak.swap(0, Ordering::Relaxed)))
    }

    pub fn stop_recording(&mut self) -> Option<AudioBuffer> {
        let mut r = self.recorder.take()?;
        r.drain();
        Some(AudioBuffer::from_interleaved(r.rate, r.channels, &r.data))
    }
}

/// Program for a buffer playing both sides.
pub fn buffer_program(buf: Arc<AudioBuffer>) -> Program {
    Program::Buffer {
        buf,
        channel_mask: 0b11,
    }
}

/// Builds a mix program from session data and loaded media.
pub fn mix_program(
    session: &af27_core::Session,
    media: &dyn Fn(af27_core::MediaId) -> Option<Arc<AudioBuffer>>,
) -> Program {
    let tracks = session
        .tracks
        .iter()
        .filter(|t| session.audible(t))
        .map(|t| {
            let (gain_l, gain_r) = t.gains();
            MixTrack {
                gain_l,
                gain_r,
                clips: t
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
                    .collect(),
            }
        })
        .collect();
    Program::Mix {
        master: db_to_gain(session.master_db),
        tracks,
        duration: session.duration(),
    }
}

/// Offline mixdown of a program to a buffer at `rate` (for export).
pub fn render(program: &Program, rate: u32, from: f64, to: f64) -> AudioBuffer {
    let mut v = Voice {
        program: Some(Arc::new(match program {
            Program::Buffer { buf, channel_mask } => Program::Buffer {
                buf: buf.clone(),
                channel_mask: *channel_mask,
            },
            Program::Mix {
                master,
                tracks,
                duration,
            } => Program::Mix {
                master: *master,
                tracks: tracks.clone(),
                duration: *duration,
            },
        })),
        playing: true,
        t: from,
        from,
        to,
        looping: false,
        active: [(0, 0); 512],
    };
    let frames = ((to - from).max(0.0) * rate as f64).round() as usize;
    let mut inter = vec![0.0f32; frames * 2];
    for chunk in inter.chunks_mut(2048) {
        v.render(chunk, 2, rate as f64);
    }
    AudioBuffer::from_interleaved(rate, 2, &inter)
}

struct Recorder {
    _stream: cpal::Stream,
    rx: rtrb::Consumer<f32>,
    data: Vec<f32>,
    rate: u32,
    channels: usize,
    peak: Arc<AtomicU32>,
}

impl Recorder {
    fn new() -> Result<Recorder, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or("No recording device was found.")?;
        let config = device
            .default_input_config()
            .map_err(|e| format!("The recording device gave no format: {e}"))?;
        let rate = config.sample_rate().0;
        let channels = (config.channels() as usize).max(1);
        let (mut tx, rx) = rtrb::RingBuffer::<f32>::new(rate as usize * channels * 10);
        let peak = Arc::new(AtomicU32::new(0));
        let pk = peak.clone();
        let cfg: cpal::StreamConfig = config.clone().into();
        let err = |e| eprintln!("audio input error: {e}");
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &cfg,
                move |d: &[f32], _| {
                    let mut m = 0.0f32;
                    for &s in d {
                        m = m.max(s.abs());
                        let _ = tx.push(s);
                    }
                    store_max_f32(&pk, m);
                },
                err,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &cfg,
                move |d: &[i16], _| {
                    let mut m = 0.0f32;
                    for &s in d {
                        let v = s as f32 / 32768.0;
                        m = m.max(v.abs());
                        let _ = tx.push(v);
                    }
                    store_max_f32(&pk, m);
                },
                err,
                None,
            ),
            other => return Err(format!("Unsupported input sample format {other:?}.")),
        }
        .map_err(|e| format!("Could not open the recording device: {e}"))?;
        stream
            .play()
            .map_err(|e| format!("Could not start recording: {e}"))?;
        Ok(Recorder {
            _stream: stream,
            rx,
            data: Vec::new(),
            rate,
            channels,
            peak,
        })
    }

    fn drain(&mut self) {
        while let Ok(s) = self.rx.pop() {
            self.data.push(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surround_folds_without_clipping() {
        let b = AudioBuffer {
            rate: 48_000,
            channels: vec![vec![1.0; 4]; 6],
        };
        let (l, r) = stereo_frame(&b, 1.0);
        assert!(l <= 1.0 + 1e-6 && r <= 1.0 + 1e-6 && l > 0.9);
        let mono = AudioBuffer {
            rate: 48_000,
            channels: vec![vec![0.5; 4]],
        };
        assert_eq!(stereo_frame(&mono, 0.0), (0.5, 0.5));
    }

    #[test]
    fn offline_mix_places_clips_and_fades() {
        let buf = Arc::new(AudioBuffer {
            rate: 100,
            channels: vec![vec![1.0; 200]],
        });
        let program = Program::Mix {
            master: 1.0,
            tracks: vec![MixTrack {
                gain_l: 1.0,
                gain_r: 0.5,
                clips: vec![MixClip {
                    buf,
                    start: 1.0,
                    offset: 0.0,
                    length: 1.0,
                    gain: 1.0,
                    fade_in: 0.0,
                    fade_out: 0.5,
                }],
            }],
            duration: 2.0,
        };
        let out = render(&program, 100, 0.0, 3.0);
        assert_eq!(out.frames(), 300);
        assert_eq!(out.channels[0][50], 0.0);
        assert!((out.channels[0][120] - 1.0).abs() < 1e-6);
        assert!((out.channels[1][120] - 0.5).abs() < 1e-6);
        assert!((out.channels[0][175] - 0.5).abs() < 0.03);
        assert_eq!(out.channels[0][250], 0.0);
    }
}
