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

pub mod capture;
pub mod mix;
pub mod pool;
pub mod scope;
pub mod soundfont;
pub mod stretch;
pub mod synth;
pub mod transport;

#[cfg(test)]
mod tests_keys;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use af27_core::AudioBuffer;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub use capture::{AudioApp, CaptureSupport, OutputDevice, RecordSource};
pub use mix::{space_gains, stereo_frame, track_gains, Mix, BLOCK, MAX_SPEAKERS};
pub use transport::{Plan, Region};

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
    Metronome(bool),
    /// The click's sound, level, accent and subdivisions.
    MetronomeSettings(af27_core::midi::MetronomeSettings),
    /// Recording started or stopped (for a click only while recording).
    Recording(bool),
    /// A chain's rack: power, input and output gain, mix.
    Rack {
        track: u16,
        rack: af27_core::Rack,
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
}

/// What the audio thread hands back to be freed on the UI thread.
#[allow(dead_code)]
enum Garbage {
    Program(Box<Program>),
    Audition(Box<soundfont::Audition>),
}

struct Shared {
    playing: AtomicBool,
    pos: AtomicU64,
    peak_l: AtomicU32,
    peak_r: AtomicU32,
}

pub(crate) fn store_max_f32(a: &AtomicU32, v: f32) {
    if v > f32::from_bits(a.load(Ordering::Relaxed)) {
        a.store(v.to_bits(), Ordering::Relaxed);
    }
}

struct Voice {
    program: Option<Box<Program>>,
    audition: Option<Box<soundfont::Audition>>,
    playing: bool,
    t: f64,
    plan: Plan,
    /// The click's settings (the app's, kept across programs).
    metro: af27_core::midi::MetronomeSettings,
    recording: bool,
    /// Live playback only: the click for the frames last rendered, one
    /// value per frame, added after the meters and the scope. Offline
    /// renders have none, so the click never reaches an export.
    clicks: Option<Vec<f32>>,
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
            Cmd::Program(p) => {
                if let Some(old) = self.program.replace(p) {
                    if let Err(rtrb::PushError::Full(old)) = trash.push(Garbage::Program(old)) {
                        // Never free on the audio thread.
                        std::mem::forget(old);
                    }
                }
            }
            Cmd::Play { from, plan } => {
                self.t = plan.start_at(from);
                self.plan = plan;
                self.playing = true;
                if let Some(Program::Mix(m)) = self.program.as_deref_mut() {
                    m.reset();
                }
            }
            Cmd::Retarget { to, plan } => {
                if let Some(t) = to {
                    self.t = plan.start_at(t);
                }
                self.plan = plan;
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
                    }
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
            Cmd::AllNotesOff => {
                if let Some(a) = &mut self.audition {
                    a.all_off();
                }
            }
        }
    }

    /// Fills interleaved `out` (`channels` wide) and returns the peaks.
    fn render(&mut self, out: &mut [f32], channels: usize, rate: f64, bed_out: bool) -> (f32, f32) {
        out.fill(0.0);
        if let Some(c) = &mut self.clicks {
            let frames = (out.len() / channels.max(1)).min(c.len());
            c[..frames].fill(0.0);
        }
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
            let n = (frames - done).min(BLOCK).min(until_seam);
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
                Program::Mix(m) => {
                    let click_on = !self.metro.recording_only || self.recording;
                    if let (Some(c), true) = (&mut self.clicks, click_on) {
                        let end = (done + n).min(c.len());
                        if done < end {
                            m.click_into(self.t, dt, &mut c[done..end], &self.metro);
                        }
                    }
                    m.render(self.t, dt, n, &mut block[..n * width], bed_out)
                }
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
            if self.t >= seam - eps && !cross(&mut self.t, &mut self.playing, seam, action) {
                break;
            }
        }
        peaks
    }
}

/// Adds the click (one value per frame) to the first two channels of
/// interleaved `out`.
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

/// Which devices to use, by name. None (or a name no longer present)
/// means the system default.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceChoice {
    pub output: Option<String>,
    pub input: Option<String>,
    /// Frames per audio callback; None lets the driver choose.
    pub buffer: Option<u32>,
}

/// The output devices present now, by name.
pub fn output_devices() -> Vec<String> {
    cpal::default_host()
        .output_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

/// The recording devices present now, by name.
pub fn input_devices() -> Vec<String> {
    cpal::default_host()
        .input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

/// The system's default output and input device names.
pub fn default_devices() -> (Option<String>, Option<String>) {
    let h = cpal::default_host();
    (
        h.default_output_device().and_then(|d| d.name().ok()),
        h.default_input_device().and_then(|d| d.name().ok()),
    )
}

/// The driver layer in use ("WASAPI" on Windows).
pub fn driver_name() -> String {
    format!("{:?}", cpal::default_host().id())
}

static INPUT_CHOICE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The recording device new recordings use (None: the system default).
pub fn set_input_device(name: Option<String>) {
    *INPUT_CHOICE.lock().unwrap() = name;
}

fn find_output(host: &cpal::Host, name: Option<&str>) -> Option<cpal::Device> {
    name.and_then(|n| {
        host.output_devices()
            .ok()?
            .find(|d| d.name().is_ok_and(|x| x == n))
    })
    .or_else(|| host.default_output_device())
}

fn find_input(host: &cpal::Host) -> Option<cpal::Device> {
    let want = INPUT_CHOICE.lock().unwrap().clone();
    want.and_then(|n| {
        host.input_devices()
            .ok()?
            .find(|d| d.name().is_ok_and(|x| x == n))
    })
    .or_else(|| host.default_input_device())
}

pub struct Engine {
    _stream: cpal::Stream,
    /// Set when the output device went away (unplugged) or failed.
    lost: Arc<AtomicBool>,
    /// The frames per callback asked for, if any.
    pub buffer: Option<u32>,
    tx: rtrb::Producer<Cmd>,
    trash: rtrb::Consumer<Garbage>,
    shared: Arc<Shared>,
    pub rate: u32,
    pub device: String,
    recorders: Vec<Recorder>,
}

impl Engine {
    /// Opens the default output device.
    pub fn new() -> Result<Engine, String> {
        Engine::open(&DeviceChoice::default())
    }

    /// Opens the chosen output device, or the system default when it is
    /// not there.
    pub fn open(choice: &DeviceChoice) -> Result<Engine, String> {
        set_input_device(choice.input.clone());
        let host = cpal::default_host();
        let device = find_output(&host, choice.output.as_deref())
            .ok_or("No audio output device was found.")?;
        let name = device.name().unwrap_or_else(|_| "Output".into());
        let config = device
            .default_output_config()
            .map_err(|e| format!("The output device gave no format: {e}"))?;
        let rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        scope::set_rate(rate);
        let (tx, mut rx) = rtrb::RingBuffer::<Cmd>::new(1024);
        let (mut trash_tx, trash) = rtrb::RingBuffer::<Garbage>::new(256);
        let shared = Arc::new(Shared {
            playing: AtomicBool::new(false),
            pos: AtomicU64::new(0f64.to_bits()),
            peak_l: AtomicU32::new(0),
            peak_r: AtomicU32::new(0),
        });
        let sh = shared.clone();
        let mut voice = Voice {
            program: None,
            audition: None,
            playing: false,
            t: 0.0,
            plan: Plan::default(),
            metro: Default::default(),
            recording: false,
            clicks: Some(vec![0.0; CLICK_FRAMES]),
        };
        let mut callback = move |out: &mut [f32]| {
            while let Ok(cmd) = rx.pop() {
                voice.apply(cmd, &mut trash_tx);
            }
            let (l, r) = voice.render(out, channels, rate as f64, false);
            // Hand-played notes, heard whether or not the transport runs.
            if let Some(a) = &mut voice.audition {
                a.render(out, channels, 1.0 / rate as f64);
            }
            if voice.playing {
                for f in out.chunks_exact(channels) {
                    scope::push(f[0], if channels > 1 { f[1] } else { f[0] });
                }
            }
            sh.playing.store(voice.playing, Ordering::Relaxed);
            sh.pos.store(voice.t.to_bits(), Ordering::Relaxed);
            store_max_f32(&sh.peak_l, l);
            store_max_f32(&sh.peak_r, r);
            // The click last: after the master fader, the meters and the
            // scope, heard but never measured.
            if let Some(c) = &voice.clicks {
                add_clicks(out, channels, c);
            }
        };
        let lost = Arc::new(AtomicBool::new(false));
        let lost2 = lost.clone();
        let err = move |e: cpal::StreamError| {
            eprintln!("audio output error: {e}");
            lost2.store(true, Ordering::Relaxed);
        };
        let mut cfg: cpal::StreamConfig = config.clone().into();
        if let Some(n) = choice.buffer {
            let n = match config.buffer_size() {
                cpal::SupportedBufferSize::Range { min, max } => n.clamp(*min, *max),
                cpal::SupportedBufferSize::Unknown => n,
            };
            cfg.buffer_size = cpal::BufferSize::Fixed(n);
        }
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
            lost,
            buffer: choice.buffer,
            tx,
            trash,
            shared,
            rate,
            device: name,
            recorders: Vec::new(),
        })
    }

    /// The output device went away or failed: reopen (the default).
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
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

    /// The click on or off (the session's `tempo.metronome`).
    pub fn set_metronome(&mut self, on: bool) {
        self.send(Cmd::Metronome(on));
    }

    /// The click's sound, level, accent, subdivisions, mute and whether it
    /// plays only while recording. Kept across programs.
    pub fn set_metronome_settings(&mut self, settings: af27_core::midi::MetronomeSettings) {
        self.send(Cmd::MetronomeSettings(settings));
    }

    /// New rack settings (power, input and output gain, dry/wet mix) for a
    /// track's chain, by track index, or [`MASTER`] for the master chain.
    /// Live, without rebuilding the mix.
    pub fn set_rack(&mut self, track: u16, rack: af27_core::Rack) {
        self.send(Cmd::Rack { track, rack });
    }

    pub fn set_font_gain(&mut self, track: usize, db: f32) {
        self.send(Cmd::FontGain {
            track: track as u16,
            db,
        });
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
        for r in &mut self.recorders {
            r.drain();
        }
    }

    /// Records from the default input device.
    pub fn start_recording(&mut self) -> Result<(), String> {
        self.start_recording_from(&[RecordSource::Input])
    }

    /// Records each source at once (a game on one track, a voice chat on
    /// another). All or nothing: if one fails to open, none run.
    pub fn start_recording_from(&mut self, sources: &[RecordSource]) -> Result<(), String> {
        let mut recs = Vec::with_capacity(sources.len());
        for s in sources {
            let r = Recorder::new(s).map_err(|e| match s {
                RecordSource::Input => e,
                _ => format!("{}: {e}", s.label()),
            })?;
            recs.push(r);
        }
        self.recorders = recs;
        self.send(Cmd::Recording(true));
        Ok(())
    }

    pub fn is_recording(&self) -> bool {
        !self.recorders.is_empty()
    }

    pub fn recording_len_secs(&self) -> f64 {
        self.recorders
            .iter()
            .map(|r| r.data.len() as f64 / (r.channels.max(1) as f64 * r.rate.max(1) as f64))
            .fold(0.0, f64::max)
    }

    pub fn recording_level(&self) -> f32 {
        self.recorders
            .iter()
            .map(|r| f32::from_bits(r.peak.swap(0, Ordering::Relaxed)))
            .fold(0.0, f32::max)
    }

    /// Problems a running capture hit (a device went away), taken once.
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
        std::mem::take(&mut self.recorders)
            .into_iter()
            .map(Recorder::finish)
            .collect()
    }
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
        audition: None,
        playing: true,
        t: from,
        plan: Plan::new(to, Region::None),
        metro: Default::default(),
        recording: false,
        clicks: None,
    };
    if let Some(Program::Mix(m)) = v.program.as_deref_mut() {
        m.reset();
        // Offline: every stretched clip as it will finally sound.
        m.prepare_now();
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
    };
    if let Some(Program::Mix(m)) = v.program.as_deref_mut() {
        m.reset();
        // Offline: every stretched clip as it will finally sound.
        m.prepare_now();
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

enum Backend {
    Device(#[allow(dead_code)] cpal::Stream),
    Loopback(capture::LoopbackCapture),
}

struct Recorder {
    backend: Backend,
    source: RecordSource,
    rx: rtrb::Consumer<f32>,
    data: Vec<f32>,
    rate: u32,
    channels: usize,
    peak: Arc<AtomicU32>,
    errors: Vec<String>,
}

impl Recorder {
    fn new(source: &RecordSource) -> Result<Recorder, String> {
        if source.is_system() {
            Self::loopback(source)
        } else {
            Self::input()
        }
    }

    fn loopback(source: &RecordSource) -> Result<Recorder, String> {
        // Ten seconds of 192 kHz stereo: the UI drains every frame.
        let (tx, rx) = rtrb::RingBuffer::<f32>::new(192_000 * 2 * 10);
        let peak = Arc::new(AtomicU32::new(0));
        let cap = capture::LoopbackCapture::open(source, tx, peak.clone())?;
        Ok(Recorder {
            rate: cap.rate,
            channels: 2,
            backend: Backend::Loopback(cap),
            source: source.clone(),
            rx,
            data: Vec::new(),
            peak,
            errors: Vec::new(),
        })
    }

    fn input() -> Result<Recorder, String> {
        let host = cpal::default_host();
        let device = find_input(&host).ok_or("No recording device was found.")?;
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
            backend: Backend::Device(stream),
            source: RecordSource::Input,
            rx,
            data: Vec::new(),
            rate,
            channels,
            peak,
            errors: Vec::new(),
        })
    }

    fn drain(&mut self) {
        while let Ok(s) = self.rx.pop() {
            self.data.push(s);
        }
    }

    fn take_error(&mut self) -> Option<String> {
        match &self.backend {
            Backend::Loopback(c) => c.take_error().inspect(|e| self.errors.push(e.clone())),
            Backend::Device(_) => None,
        }
    }

    fn finish(mut self) -> Take {
        let mut note = None;
        if let Backend::Loopback(c) = &mut self.backend {
            c.stop();
            note = c.note.clone();
        }
        let _ = self.take_error();
        self.drain();
        if let Some(e) = self.errors.last() {
            note = Some(e.clone());
        }
        Take {
            buf: AudioBuffer::from_interleaved(self.rate, self.channels, &self.data),
            source: self.source,
            note,
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
            gain_db: 0.0,
            controls: Vec::new(),
            ext_id: None,
        });
        let out = render(mix_program(&s, &|_| None, 48_000), 48_000, 0.0, 1.0);
        assert!(out.peak() > 0.01);
    }

    #[test]
    fn notes_follow_the_tempo_map() {
        use af27_core::midi::{MidiClip, MidiNote, TempoChange};
        // One bar of 4/4 at 120 (2 s), then 60 BPM: beat 5 is at 3 s.
        let mut s = Session::new("m", 48_000, 0);
        s.tempo.changes.push(TempoChange {
            bar: 1,
            bpm: 60.0,
            beats_per_bar: 4,
            beat_unit: 4,
            ext_id: None,
        });
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

/// `cargo test --release -p af27-engine bench -- --ignored --nocapture`
#[cfg(test)]
mod bench {
    use super::*;
    use af27_core::session::{Clip, MediaItem, Send};
    use af27_core::{EffectSlot, Session, TrackKind};

    fn slot(id: &str) -> EffectSlot {
        let e = af27_dsp::Effect::from_id(id).unwrap();
        EffectSlot {
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
