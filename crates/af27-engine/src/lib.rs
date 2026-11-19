// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

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

pub mod mix;
pub mod scope;
pub mod synth;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use af27_core::AudioBuffer;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub use mix::{space_gains, stereo_frame, track_gains, Mix, BLOCK, MAX_SPEAKERS};

/// What the audio thread plays.
pub enum Program {
    /// One buffer (the Waveform editor). `channel_mask` bit 0 is left,
    /// bit 1 right, for listening to one side.
    Buffer {
        buf: Arc<AudioBuffer>,
        channel_mask: u8,
    },
    Mix(Box<Mix>),
}

/// The master chain in [`Cmd::Param`] and friends.
pub const MASTER: u16 = u16::MAX;
/// Most values one effect takes.
pub const MAX_VALUES: usize = 24;

enum Cmd {
    Program(Box<Program>),
    Play {
        from: f64,
        to: f64,
        looping: bool,
    },
    Stop,
    Gains {
        track: u16,
        l: f32,
        r: f32,
    },
    Audible {
        track: u16,
        on: bool,
    },
    Master(f32),
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
    Space {
        track: u16,
        l: [f32; MAX_SPEAKERS],
        r: [f32; MAX_SPEAKERS],
        lfe: f32,
    },
    Instrument {
        track: u16,
        inst: af27_core::midi::Instrument,
    },
    Metronome(Option<(f64, u32)>),
}

struct Shared {
    playing: AtomicBool,
    pos: AtomicU64,
    peak_l: AtomicU32,
    peak_r: AtomicU32,
}

fn store_max_f32(a: &AtomicU32, v: f32) {
    if v > f32::from_bits(a.load(Ordering::Relaxed)) {
        a.store(v.to_bits(), Ordering::Relaxed);
    }
}

struct Voice {
    program: Option<Box<Program>>,
    playing: bool,
    t: f64,
    from: f64,
    to: f64,
    looping: bool,
}

impl Voice {
    fn apply(&mut self, cmd: Cmd, trash: &mut rtrb::Producer<Box<Program>>) {
        let mix = match self.program.as_deref_mut() {
            Some(Program::Mix(m)) => Some(m.as_mut()),
            _ => None,
        };
        match cmd {
            Cmd::Program(p) => {
                if let Some(old) = self.program.replace(p) {
                    if let Err(rtrb::PushError::Full(old)) = trash.push(old) {
                        // Never free on the audio thread.
                        std::mem::forget(old);
                    }
                }
            }
            Cmd::Play { from, to, looping } => {
                self.t = from;
                self.from = from;
                self.to = to;
                self.looping = looping;
                self.playing = true;
                if let Some(Program::Mix(m)) = self.program.as_deref_mut() {
                    m.reset();
                }
            }
            Cmd::Stop => self.playing = false,
            Cmd::Gains { track, l, r } => {
                if let Some(t) = mix.and_then(|m| m.tracks.get_mut(track as usize)) {
                    t.gain_l = l;
                    t.gain_r = r;
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
                    }
                }
            }
            Cmd::Bypass { track, slot, on } => {
                if let Some(m) = mix {
                    if track == MASTER {
                        m.set_master_bypass(slot as usize, on);
                    } else if let Some(t) = m.tracks.get_mut(track as usize) {
                        t.chain.set_bypass(slot as usize, on);
                    }
                }
            }
            Cmd::ChainMix { track, mix: amount } => {
                if let Some(m) = mix {
                    if track == MASTER {
                        m.set_master_mix(amount);
                    } else if let Some(t) = m.tracks.get_mut(track as usize) {
                        t.chain.mix = amount;
                    }
                }
            }
            Cmd::Space { track, l, r, lfe } => {
                if let Some(t) = mix.and_then(|m| m.tracks.get_mut(track as usize)) {
                    t.space_l = l;
                    t.space_r = r;
                    t.lfe = lfe;
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
            Cmd::Metronome(m2) => {
                if let Some(m) = mix {
                    m.metronome = m2;
                }
            }
        }
    }

    /// Fills interleaved `out` (`channels` wide) and returns the peaks.
    fn render(&mut self, out: &mut [f32], channels: usize, rate: f64, bed_out: bool) -> (f32, f32) {
        out.fill(0.0);
        let Some(program) = self.program.as_deref_mut() else {
            return (0.0, 0.0);
        };
        if !self.playing {
            return (0.0, 0.0);
        }
        let dt = 1.0 / rate;
        let frames = out.len() / channels;
        let width = if bed_out {
            match program {
                Program::Mix(m) => m.channels_out(),
                _ => 2,
            }
        } else {
            2
        };
        let mut done = 0;
        let mut peaks = (0.0f32, 0.0f32);
        let mut block = [0.0f32; BLOCK * MAX_SPEAKERS];
        while done < frames && self.playing {
            if self.t >= self.to {
                if self.looping && self.to > self.from {
                    self.t = self.from;
                    if let Program::Mix(m) = program {
                        m.reset();
                    }
                } else {
                    self.playing = false;
                    break;
                }
            }
            let until_end = (((self.to - self.t) / dt).ceil() as usize).max(1);
            let n = (frames - done).min(BLOCK).min(until_end);
            let p = match program {
                Program::Buffer { buf, channel_mask } => {
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
                Program::Mix(m) => m.render(self.t, dt, n, &mut block[..n * width], bed_out),
            };
            peaks = (peaks.0.max(p.0), peaks.1.max(p.1));
            for i in 0..n {
                let o = &mut out[(done + i) * channels..(done + i + 1) * channels];
                if bed_out {
                    o[..width].copy_from_slice(&block[i * width..(i + 1) * width]);
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
            self.t += dt * n as f64;
            done += n;
        }
        peaks
    }
}

pub struct Engine {
    _stream: cpal::Stream,
    tx: rtrb::Producer<Cmd>,
    trash: rtrb::Consumer<Box<Program>>,
    shared: Arc<Shared>,
    pub rate: u32,
    pub device: String,
    recorder: Option<Recorder>,
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
        scope::set_rate(rate);
        let (tx, mut rx) = rtrb::RingBuffer::<Cmd>::new(1024);
        let (mut trash_tx, trash) = rtrb::RingBuffer::<Box<Program>>::new(256);
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
        };
        let mut callback = move |out: &mut [f32]| {
            while let Ok(cmd) = rx.pop() {
                voice.apply(cmd, &mut trash_tx);
            }
            let (l, r) = voice.render(out, channels, rate as f64, false);
            if voice.playing {
                for f in out.chunks_exact(channels) {
                    scope::push(f[0], if channels > 1 { f[1] } else { f[0] });
                }
            }
            sh.playing.store(voice.playing, Ordering::Relaxed);
            sh.pos.store(voice.t.to_bits(), Ordering::Relaxed);
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

    fn send(&mut self, c: Cmd) {
        let _ = self.tx.push(c);
    }

    pub fn set_program(&mut self, p: Program) {
        self.send(Cmd::Program(Box::new(p)));
    }

    /// Plays from `from` up to `to` (None = the end).
    pub fn play(&mut self, from: f64, to: Option<f64>, looping: bool, program_len: f64) {
        let to = to.unwrap_or(program_len).max(from);
        self.send(Cmd::Play { from, to, looping });
        self.shared.playing.store(true, Ordering::Relaxed);
    }

    pub fn stop(&mut self) {
        self.send(Cmd::Stop);
    }

    pub fn set_gains(&mut self, track: usize, l: f32, r: f32) {
        self.send(Cmd::Gains {
            track: track as u16,
            l,
            r,
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

    pub fn set_chain_mix(&mut self, track: u16, mix: f32) {
        self.send(Cmd::ChainMix { track, mix });
    }

    pub fn set_space(
        &mut self,
        track: usize,
        l: [f32; MAX_SPEAKERS],
        r: [f32; MAX_SPEAKERS],
        lfe: f32,
    ) {
        self.send(Cmd::Space {
            track: track as u16,
            l,
            r,
            lfe,
        });
    }

    pub fn set_instrument(&mut self, track: usize, inst: af27_core::midi::Instrument) {
        self.send(Cmd::Instrument {
            track: track as u16,
            inst,
        });
    }

    pub fn set_metronome(&mut self, m: Option<(f64, u32)>) {
        self.send(Cmd::Metronome(m));
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

/// Builds a mix program from session data and loaded media, at `rate`.
pub fn mix_program(
    session: &af27_core::Session,
    media: &dyn Fn(af27_core::MediaId) -> Option<Arc<AudioBuffer>>,
    rate: u32,
) -> Program {
    Program::Mix(Box::new(mix::build(session, media, rate)))
}

fn render_with(program: Program, rate: u32, from: f64, to: f64, bed_out: bool) -> AudioBuffer {
    let channels = match (&program, bed_out) {
        (Program::Mix(m), true) => m.channels_out(),
        _ => 2,
    };
    let mut v = Voice {
        program: Some(Box::new(program)),
        playing: true,
        t: from,
        from,
        to,
        looping: false,
    };
    if let Some(Program::Mix(m)) = v.program.as_deref_mut() {
        m.reset();
    }
    let frames = ((to - from).max(0.0) * rate as f64).round() as usize;
    let mut inter = vec![0.0f32; frames * channels];
    for chunk in inter.chunks_mut(BLOCK * channels) {
        v.render(chunk, channels, rate as f64, bed_out);
    }
    AudioBuffer::from_interleaved(rate, channels, &inter)
}

/// Offline stereo mixdown (3D sessions as heard: binaural or folded).
pub fn render(program: Program, rate: u32, from: f64, to: f64) -> AudioBuffer {
    render_with(program, rate, from, to, false)
}

/// Offline mixdown of a 3D session's speaker bed, one channel per speaker.
pub fn render_bed(program: Program, rate: u32, from: f64, to: f64) -> AudioBuffer {
    render_with(program, rate, from, to, true)
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
    use af27_core::session::{Clip, MediaItem, Send};
    use af27_core::{Session, TrackKind};

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
        s.tracks[bi].chain.push(af27_core::EffectSlot {
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
        s.space = Some(af27_core::space::SpaceLayout::Surround51);
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

    fn amp(db: f32) -> af27_core::EffectSlot {
        af27_core::EffectSlot {
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
        s.space = Some(af27_core::space::SpaceLayout::Surround714);
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
        s.space = Some(af27_core::space::SpaceLayout::Surround51);
        s.tracks[0].pos.x = 0.0;
        s.tracks[0].pos.y = 1.0;
        s.master_chain.push(amp(-6.0206));
        let b2 = buf.clone();
        let program = mix_program(&s, &move |_| Some(b2.clone()), 100);
        let (mut trash, _rx) = rtrb::RingBuffer::<Box<Program>>::new(4);
        let mut v = Voice {
            program: Some(Box::new(program)),
            playing: false,
            t: 0.0,
            from: 0.0,
            to: 0.0,
            looping: false,
        };
        let mut centre = |v: &mut Voice| {
            v.apply(
                Cmd::Play {
                    from: 1.2,
                    to: 1.3,
                    looping: false,
                },
                &mut trash,
            );
            let mut out = vec![0.0f32; 6 * 4];
            v.render(&mut out, 6, 100.0, true);
            out[2]
        };
        assert!((centre(&mut v) - 0.5).abs() < 1e-3);
        let (mut trash, _rx) = rtrb::RingBuffer::<Box<Program>>::new(4);
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

    #[test]
    fn midi_tracks_sound() {
        let mut s = Session::new("m", 48_000, 0);
        let t = s.add_track_of(TrackKind::Midi);
        let ti = s.track_index(t).unwrap();
        s.tracks[ti].midi.push(af27_core::midi::MidiClip {
            id: 1,
            name: "c".into(),
            start: 0.0,
            length: 4.0,
            notes: vec![af27_core::midi::MidiNote {
                pitch: 60,
                start: 0.0,
                length: 1.0,
                velocity: 100,
                channel: 0,
                pan: 0.0,
            }],
            muted: false,
            ext_id: None,
        });
        let out = render(mix_program(&s, &|_| None, 48_000), 48_000, 0.0, 1.0);
        assert!(out.peak() > 0.01);
    }
}

/// `cargo test --release -p af27-engine bench -- --ignored --nocapture`
#[cfg(test)]
mod bench {
    use super::*;
    use af27_core::session::{Clip, MediaItem, Send};
    use af27_core::{EffectSlot, Session, TrackKind};

    fn slot(id: &str) -> EffectSlot {
        let e = af27_dsp::Effect::from_id(id).unwrap();
        EffectSlot {
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
        run_bench(Some(af27_core::space::SpaceLayout::Surround714));
    }

    fn run_bench(space: Option<af27_core::space::SpaceLayout>) {
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
