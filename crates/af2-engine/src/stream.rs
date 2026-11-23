// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The one output stream: everything that sounds (playback, hand-played
//! notes, the click) is mixed into a single device callback, so on ASIO
//! there is never a second stream to replay stale buffers, ask for another
//! channel count, or be starved by the driver.
//!
//! The callback renders the stereo mix (or a 3D session's speaker bed)
//! into scratch, maps it onto the device's channels ([`crate::routing`]),
//! adds the click on the stereo pair and converts to the device's sample
//! format. It is wrapped so a panic leaves silence, and it plays silence
//! while the stream closes ([`crate::health::Hush`]).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use crate::devices::{self, HostKind};
use crate::health::{self, CallbackStats, Hush, Watchdog};
use crate::routing::{self, Routes};
use crate::{
    scope, store_max_f32, Cmd, DeviceChoice, Engine, Garbage, Plan, Program, Shared, Voice,
    CLICK_FRAMES, MAX_SPEAKERS,
};

/// Frames rendered at a time inside one callback.
pub(crate) const CHUNK: usize = 4096;

/// The renderer the callback owns.
pub(crate) struct Core {
    rx: rtrb::Consumer<Cmd>,
    trash: rtrb::Producer<Garbage>,
    voice: Voice,
    shared: Arc<Shared>,
    rate: u32,
    /// The device's channels.
    channels: usize,
    routes: Routes,
    /// The stereo mix or the bed, `CHUNK` frames of up to `MAX_SPEAKERS`.
    scratch: Vec<f32>,
    /// One chunk in the device's channels, before format conversion.
    dev: Vec<f32>,
    /// Notes played live (MIDI input), from another thread.
    notes: Option<rtrb::Consumer<crate::LiveNote>>,
    /// A track's plug-in instrument played live, and its events from the
    /// MIDI input's thread.
    keys: Option<Box<crate::plugin_inst::KeysPlayer>>,
    keys_rx: Option<rtrb::Consumer<af2_dsp::instrument::NoteEvent>>,
    /// Inputs heard live through their tracks ([`crate::monitor`]).
    monitor: Option<Box<crate::monitor::Feed>>,
}

impl Core {
    pub(crate) fn new(
        rx: rtrb::Consumer<Cmd>,
        trash: rtrb::Producer<Garbage>,
        shared: Arc<Shared>,
        rate: u32,
        channels: usize,
        routes: Routes,
    ) -> Core {
        let channels = channels.max(1);
        Core {
            rx,
            trash,
            voice: Voice {
                program: None,
                audition: None,
                playing: false,
                t: 0.0,
                plan: Plan::default(),
                metro: Default::default(),
                recording: false,
                clicks: Some(vec![0.0; CLICK_FRAMES.max(CHUNK)]),
                declick: Default::default(),
            },
            shared,
            rate,
            channels,
            routes,
            scratch: vec![0.0; CHUNK * MAX_SPEAKERS.max(2)],
            dev: vec![0.0; CHUNK * channels],
            notes: None,
            keys: None,
            keys_rx: None,
            monitor: None,
        }
    }

    /// Fills `out` (interleaved device frames, at most [`CHUNK`]).
    pub(crate) fn process(&mut self, out: &mut [f32]) {
        while let Ok(cmd) = self.rx.pop() {
            match cmd {
                Cmd::Routes(r) => self.routes = r,
                Cmd::Keys(p) => {
                    let old = std::mem::replace(&mut self.keys, p);
                    if let Some(old) = old {
                        if let Err(rtrb::PushError::Full(old)) =
                            self.trash.push(Garbage::Keys(old))
                        {
                            // Never free on the audio thread.
                            std::mem::forget(old);
                        }
                    }
                }
                Cmd::KeysGain(g) => {
                    if let Some(k) = &mut self.keys {
                        k.gain = g;
                    }
                }
                Cmd::KeysEvent(e) => {
                    if let Some(k) = &mut self.keys {
                        k.queue(e);
                    }
                }
                Cmd::KeysAllOff => {
                    if let Some(k) = &mut self.keys {
                        k.all_off();
                    }
                }
                Cmd::Monitor(f) => {
                    if let Some(old) = std::mem::replace(&mut self.monitor, f) {
                        if let Err(rtrb::PushError::Full(old)) =
                            self.trash.push(Garbage::Monitor(old))
                        {
                            // Never free on the audio thread.
                            std::mem::forget(old);
                        }
                    }
                }
                // Monitoring while stopped: an edit's new mix takes over
                // the running effects, as it does while playing, so what
                // the player hears doesn't cut out or restart.
                Cmd::Program(mut p) if self.monitor.is_some() && !self.voice.playing => {
                    if let (Program::Mix(m), Some(Program::Mix(old))) =
                        (p.as_mut(), self.voice.program.as_deref_mut())
                    {
                        m.adopt(old);
                    }
                    self.voice.apply(Cmd::Program(p), &mut self.trash);
                }
                other => self.voice.apply(other, &mut self.trash),
            }
        }
        if let Some(rx) = &mut self.notes {
            while let Ok(n) = rx.pop() {
                if let Some(a) = &mut self.voice.audition {
                    match n {
                        crate::LiveNote::On {
                            pitch,
                            velocity,
                            pan,
                        } => a.note_on(pitch, velocity, pan),
                        crate::LiveNote::Off(p) => a.note_off(p),
                        crate::LiveNote::AllOff => a.all_off(),
                        crate::LiveNote::Control { controller, value } => {
                            a.control(controller, value)
                        }
                    }
                }
            }
        }
        let ch = self.channels;
        let frames = (out.len() / ch).min(CHUNK);
        // The bed goes out speaker by speaker only when the program's bed
        // is the one the routes were made for.
        let bed = self.routes.bed_len > 0
            && matches!(self.voice.program.as_deref(),
                Some(Program::Mix(m)) if m.channels_out() == self.routes.bed_len)
                || self.routes.bed_len > 0
                    && matches!(self.voice.program.as_deref(),
                        Some(Program::Buffer { head: Some(h), .. })
                            if h.is_direct() && h.speakers() == self.routes.bed_len);
        let width = if bed { self.routes.bed_len } else { 2 };
        let src = &mut self.scratch[..frames * width];
        if let Some(f) = &mut self.monitor {
            f.begin(frames);
            f.silence();
        }
        let t0 = self.voice.t;
        let (l, r) =
            self.voice
                .render_live(src, width, self.rate as f64, bed, self.monitor.as_deref_mut());
        // A surround file's own channels, metered before the fold.
        let mut src_n = 0u32;
        if let Some(Program::Buffer { buf, .. }) = self.voice.program.as_deref() {
            let n = buf.num_channels();
            if n > 2 {
                src_n = n.min(crate::MAX_SPEAKERS) as u32;
                if self.voice.playing {
                    let t1 = self.voice.t;
                    // Forward: what was played; a jump or loop: the block
                    // from where it began.
                    let span = if t1 > t0 && t1 - t0 < 1.0 {
                        t1 - t0
                    } else {
                        frames as f64 / self.rate.max(1) as f64
                    };
                    let rate = buf.rate as f64;
                    let a = (t0.max(0.0) * rate) as usize;
                    let b = ((t0.max(0.0) + span) * rate).ceil() as usize;
                    for (k, c) in buf.channels.iter().take(src_n as usize).enumerate() {
                        let (a, b) = (a.min(c.len()), b.min(c.len()));
                        let pk = c[a..b].iter().fold(0.0f32, |m, v| m.max(v.abs()));
                        store_max_f32(&self.shared.src_peaks[k], pk);
                    }
                }
            }
        }
        self.shared.src_count.store(src_n, Ordering::Relaxed);
        // The RMS meters' power, over the same sound as the peaks (silence
        // while stopped, so they settle).
        let (sl, sr, sn) = crate::meter::sum_squares(src, width);
        self.shared.mean_square.add(sl, sr, sn);
        // Hand-played notes, heard whether or not the transport runs.
        if let Some(a) = &mut self.voice.audition {
            a.render(src, width, 1.0 / self.rate as f64);
        }
        // A plug-in instrument played from a MIDI keyboard, likewise.
        if let Some(rx) = &mut self.keys_rx {
            while let Ok(e) = rx.pop() {
                if let Some(k) = &mut self.keys {
                    k.queue(e);
                }
            }
        }
        if let Some(k) = &mut self.keys {
            k.render(src, width);
        }
        if self.voice.playing {
            for f in src.chunks_exact(width) {
                scope::push(f[0], f[1]);
            }
        }
        let sh = &self.shared;
        sh.playing.store(self.voice.playing, Ordering::Relaxed);
        sh.pos.store(self.voice.t.to_bits(), Ordering::Relaxed);
        store_max_f32(&sh.peak_l, l);
        store_max_f32(&sh.peak_r, r);
        // Per-speaker meters of a 3D session's bed.
        if let Some(Program::Mix(m)) = self.voice.program.as_deref_mut() {
            let mut pk = [0.0f32; crate::MAX_SPEAKERS];
            let n = m.take_bed_peaks(&mut pk);
            for (a, v) in sh.bed_peaks.iter().zip(pk) {
                store_max_f32(a, v);
            }
            sh.bed_count.store(n as u32, Ordering::Relaxed);
        }
        let out = &mut out[..frames * ch];
        routing::map_frames(src, width, out, ch, &self.routes);
        // The click last: after the master fader, the meters and the
        // scope, heard but never measured.
        if let Some(c) = &self.voice.clicks {
            routing::add_clicks_at(out, ch, &c[..frames.min(c.len())], self.routes.stereo);
        }
    }

    /// The device callback for sample type `T`.
    fn callback<T: SizedSample + FromSample<f32>>(
        &mut self,
        data: &mut [T],
        hush: &Hush,
        fault: &AtomicBool,
        stats: &CallbackStats,
    ) {
        crate::denormal::flush_to_zero();
        stats.tick(data.len() / self.channels);
        if hush.silenced(data, T::EQUILIBRIUM) {
            return;
        }
        let float = T::FORMAT.is_float();
        health::guarded(data, T::EQUILIBRIUM, fault, |data| {
            let step = CHUNK * self.channels;
            let mut dev = std::mem::take(&mut self.dev);
            for chunk in data.chunks_mut(step) {
                let buf = &mut dev[..chunk.len()];
                self.process(buf);
                for (o, s) in chunk.iter_mut().zip(buf.iter()) {
                    let s = if float { *s } else { s.clamp(-1.0, 1.0) };
                    *o = T::from_sample(s);
                }
            }
            self.dev = dev;
        });
    }
}

/// What a built stream hands back to the engine.
struct Built {
    stream: cpal::Stream,
    tx: rtrb::Producer<Cmd>,
    trash: rtrb::Consumer<Garbage>,
    shared: Arc<Shared>,
    hush: Arc<Hush>,
    stats: Arc<CallbackStats>,
    fault: Arc<AtomicBool>,
    lost: Arc<AtomicBool>,
    notes: rtrb::Producer<crate::LiveNote>,
    keys: rtrb::Producer<af2_dsp::instrument::NoteEvent>,
}

fn build(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    format: SampleFormat,
    routes: Routes,
) -> Result<Built, String> {
    let (tx, rx) = rtrb::RingBuffer::<Cmd>::new(1024);
    let (trash_tx, trash) = rtrb::RingBuffer::<Garbage>::new(256);
    let shared = Arc::new(Shared {
        playing: AtomicBool::new(false),
        pos: AtomicU64::new(0f64.to_bits()),
        peak_l: AtomicU32::new(0),
        peak_r: AtomicU32::new(0),
        mean_square: Default::default(),
        bed_peaks: std::array::from_fn(|_| AtomicU32::new(0)),
        bed_count: AtomicU32::new(0),
        src_peaks: std::array::from_fn(|_| AtomicU32::new(0)),
        src_count: AtomicU32::new(0),
    });
    let hush = Arc::new(Hush::default());
    let stats = Arc::new(CallbackStats::default());
    let fault = Arc::new(AtomicBool::new(false));
    let lost = Arc::new(AtomicBool::new(false));
    let core = Core::new(
        rx,
        trash_tx,
        shared.clone(),
        cfg.sample_rate.0,
        cfg.channels as usize,
        routes,
    );
    let (notes, notes_rx) = rtrb::RingBuffer::<crate::LiveNote>::new(512);
    let mut core = core;
    core.notes = Some(notes_rx);
    let (keys, keys_rx) = rtrb::RingBuffer::<af2_dsp::instrument::NoteEvent>::new(1024);
    core.keys_rx = Some(keys_rx);
    macro_rules! go {
        ($t:ty) => {{
            let (h, s, f, l) = (hush.clone(), stats.clone(), fault.clone(), lost.clone());
            let mut core = core;
            device.build_output_stream(
                cfg,
                move |d: &mut [$t], _| core.callback(d, &h, &f, &s),
                move |e: cpal::StreamError| {
                    // Not logged here: this may run on the audio thread.
                    let _ = e;
                    l.store(true, Ordering::Relaxed);
                },
                None,
            )
        }};
    }
    let stream = match format {
        SampleFormat::F32 => go!(f32),
        SampleFormat::F64 => go!(f64),
        SampleFormat::I16 => go!(i16),
        SampleFormat::I32 => go!(i32),
        SampleFormat::U16 => go!(u16),
        SampleFormat::U32 => go!(u32),
        SampleFormat::I8 => go!(i8),
        SampleFormat::U8 => go!(u8),
        other => return Err(format!("Unsupported output sample format {other:?}.")),
    }
    .map_err(|e| format!("Could not open the audio output: {e}"))?;
    stream
        .play()
        .map_err(|e| format!("Could not start audio: {e}"))?;
    Ok(Built {
        stream,
        tx,
        trash,
        shared,
        hush,
        stats,
        fault,
        lost,
        notes,
        keys,
    })
}

/// The output device for `choice`: the ASIO driver, or the system's chosen
/// (or default) output. When the ASIO driver will not open (another
/// program has it, it is gone) the system driver plays instead, and the
/// note says so.
fn output_device(
    choice: &DeviceChoice,
) -> Result<(cpal::Device, HostKind, String, Option<String>), String> {
    let mut note = None;
    if choice.host == HostKind::Asio {
        let installed = devices::asio_drivers();
        match devices::pick_driver(&installed, choice.asio_driver.as_deref()) {
            Some(name) if devices::ASIO_BUILT => {
                // Load it afresh, so a new buffer size takes: cpal keeps a
                // driver's buffers for as long as the device value lives.
                crate::asio::release();
                match crate::asio::device(&name) {
                    Ok(d) => return Ok((d, HostKind::Asio, format!("ASIO ({name})"), None)),
                    Err(e) => {
                        health::log(&format!("asio: {e}"));
                        note = Some(format!(
                            "{e} Playing through {} instead.",
                            devices::system_name()
                        ));
                    }
                }
            }
            Some(_) => note = Some(devices::NO_ASIO.into()),
            None => {
                note = Some(format!(
                    "No ASIO drivers are installed; playing through {}.",
                    devices::system_name()
                ))
            }
        }
    } else {
        crate::asio::release();
    }
    let host = cpal::default_host();
    let device = crate::find_output(&host, choice.output.as_deref())
        .ok_or("No audio output device was found.")?;
    Ok((device, HostKind::System, devices::system_name(), note))
}

/// Opens the output for `choice`.
pub(crate) fn open(choice: &DeviceChoice) -> Result<Engine, String> {
    crate::set_input_device(choice.input.clone());
    let (device, host, driver, mut note) = output_device(choice)?;
    let name = device.name().unwrap_or_else(|_| "Output".into());
    let default = device
        .default_output_config()
        .map_err(|e| format!("The output device gave no format: {e}"))?;
    let ranges: Vec<cpal::SupportedStreamConfigRange> = device
        .supported_output_configs()
        .map(|it| it.collect())
        .unwrap_or_default();
    // Speakers each to their own output: as many outputs as there are.
    let want = if choice.map.speakers_out {
        usize::MAX
    } else {
        choice.map.width_needed(None)
    };
    let (config, rate_note) = devices::choose_config(&default, &ranges, want, choice.rate);
    if rate_note.is_some() {
        note = join(note, rate_note);
    }
    let rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let format = config.sample_format();
    let mut cfg: cpal::StreamConfig = config.clone().into();
    if let Some(n) = choice.buffer {
        cfg.buffer_size = cpal::BufferSize::Fixed(devices::fit_buffer(n, config.buffer_size()));
    }
    let routes = choice.map.routes(None);
    let built = match build(&device, &cfg, format, routes) {
        Ok(b) => b,
        Err(e) if choice.buffer.is_some() => {
            // Some drivers refuse a size they list: try their own.
            health::log(&format!("play: {e}; trying the driver's own buffer size"));
            cfg.buffer_size = cpal::BufferSize::Default;
            note = join(
                note,
                Some("The device refused the buffer size, so it uses its own.".into()),
            );
            build(&device, &cfg, format, routes)?
        }
        Err(e) => {
            health::log(&format!("play: {e}"));
            return Err(e);
        }
    };
    scope::set_rate(rate);
    let buffer = match cfg.buffer_size {
        cpal::BufferSize::Fixed(n) => Some(n),
        cpal::BufferSize::Default => None,
    };
    health::log(&format!(
        "play: {driver} on \"{name}\", {rate} Hz, {channels} channels, {format:?}, buffer {}",
        buffer.map_or("driver's own".to_string(), |n| n.to_string())
    ));
    if let Some(n) = &note {
        health::log(&format!("play: {n}"));
    }
    let asio_inputs = (host == HostKind::Asio)
        .then(|| device.default_input_config().ok())
        .flatten()
        .filter(|c| c.channels() > 0)
        .map(|c| devices::DeviceInfo {
            name: name.clone(),
            channels: c.channels(),
        });
    Ok(Engine {
        stream: Some(built.stream),
        lost: built.lost,
        fault: built.fault,
        hush: built.hush,
        stats: built.stats,
        watchdog: Watchdog::new(Instant::now(), health::STALL),
        buffer,
        tx: built.tx,
        trash: built.trash,
        shared: built.shared,
        rate,
        device: name,
        host,
        driver,
        channels,
        format,
        note,
        map: choice.map.clone(),
        layout: None,
        asio_inputs,
        asio_device: (host == HostKind::Asio).then(|| device.clone()),
        recorders: Vec::new(),
        monitor: Default::default(),
        notes: Some(built.notes),
        keys: Some(built.keys),
    })
}

fn join(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(format!("{a} {b}")),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use af2_core::AudioBuffer;

    fn core(channels: usize, routes: Routes) -> (Core, rtrb::Producer<Cmd>) {
        let (tx, rx) = rtrb::RingBuffer::<Cmd>::new(64);
        let (trash_tx, _trash) = rtrb::RingBuffer::<Garbage>::new(8);
        let shared = Arc::new(Shared {
            playing: AtomicBool::new(false),
            pos: AtomicU64::new(0),
            peak_l: AtomicU32::new(0),
            peak_r: AtomicU32::new(0),
            mean_square: Default::default(),
            bed_peaks: std::array::from_fn(|_| AtomicU32::new(0)),
            bed_count: AtomicU32::new(0),
            src_peaks: std::array::from_fn(|_| AtomicU32::new(0)),
            src_count: AtomicU32::new(0),
        });
        (
            Core::new(rx, trash_tx, shared, 48_000, channels, routes),
            tx,
        )
    }

    fn tone(rate: u32, secs: f64, hz: f64) -> Arc<AudioBuffer> {
        let n = (rate as f64 * secs) as usize;
        let s: Vec<f32> = (0..n)
            .map(|i| (std::f64::consts::TAU * hz * i as f64 / rate as f64).sin() as f32 * 0.5)
            .collect();
        Arc::new(AudioBuffer {
            rate,
            channels: vec![s.clone(), s],
        })
    }

    fn play(tx: &mut rtrb::Producer<Cmd>, buf: Arc<AudioBuffer>, to: f64) {
        let _ = tx.push(Cmd::Program(Box::new(Program::Buffer {
            buf,
            channel_mask: 0b11,
            head: None,
        })));
        let _ = tx.push(Cmd::Play {
            from: 0.0,
            plan: Plan::new(to, crate::Region::None),
        });
    }

    #[test]
    fn playback_lands_on_the_mapped_pair_of_a_wide_device() {
        let map = crate::OutputMap {
            stereo: [2, 3],
            ..Default::default()
        };
        let (mut c, mut tx) = core(4, map.routes(None));
        play(&mut tx, tone(48_000, 0.1, 1_000.0), 0.1);
        let mut out = vec![0.0f32; 256 * 4];
        c.process(&mut out);
        let energy = |k: usize| out.chunks(4).map(|f| f[k].abs()).sum::<f32>();
        assert_eq!(energy(0), 0.0);
        assert_eq!(energy(1), 0.0);
        assert!(energy(2) > 1.0 && energy(3) > 1.0);
    }

    #[test]
    fn a_session_at_another_rate_plays_at_the_right_speed() {
        // A 44.1 kHz file through a 48 kHz device: converted as it plays,
        // so a second of audio takes a second, at the same pitch.
        let (mut c, mut tx) = core(2, Routes::default());
        play(&mut tx, tone(44_100, 1.0, 441.0), 10.0);
        let mut out = vec![0.0f32; 48_000 * 2];
        for chunk in out.chunks_mut(CHUNK * 2) {
            c.process(chunk);
        }
        let left: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
        // Sound up to 1 s, then silence.
        assert!(left[..47_000].iter().any(|s| s.abs() > 0.4));
        let crossings = left[..47_900]
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        // 441 Hz over 47 900 frames at 48 kHz: about 440 cycles.
        assert!((crossings as i64 - 440).abs() <= 2, "{crossings}");
        let mut more = vec![1.0f32; 1024];
        c.process(&mut more);
        // (The first frame may still blend in the file's last sample.)
        assert!(more[4..].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn routes_change_live() {
        let (mut c, mut tx) = core(4, Routes::default());
        play(&mut tx, tone(48_000, 1.0, 500.0), 1.0);
        let mut out = vec![0.0f32; 128 * 4];
        c.process(&mut out);
        assert!(out.chunks(4).any(|f| f[0] != 0.0));
        let r = Routes {
            stereo: [3, 2],
            ..Routes::default()
        };
        let _ = tx.push(Cmd::Routes(r));
        c.process(&mut out);
        assert!(out.chunks(4).all(|f| f[0] == 0.0 && f[1] == 0.0));
        assert!(out.chunks(4).any(|f| f[3] != 0.0));
    }

    #[test]
    fn integer_devices_get_clamped_samples_and_a_panic_leaves_silence() {
        let (mut c, mut tx) = core(2, Routes::default());
        let loud = Arc::new(AudioBuffer {
            rate: 48_000,
            channels: vec![vec![4.0; 4800], vec![-4.0; 4800]],
        });
        play(&mut tx, loud, 0.1);
        let (hush, fault, stats) = (
            Hush::default(),
            AtomicBool::new(false),
            CallbackStats::default(),
        );
        let mut out = vec![0i16; 64 * 2];
        c.callback(&mut out, &hush, &fault, &stats);
        assert_eq!(out[0], i16::MAX);
        assert!(out[1] <= -i16::MAX);
        assert_eq!(stats.buffer(), Some(64));
        // Closing: silence, whatever is playing.
        hush.settle_within(std::time::Duration::from_millis(1));
        c.callback(&mut out, &hush, &fault, &stats);
        assert!(out.iter().all(|s| *s == 0));
        assert!(!fault.load(Ordering::Relaxed));
    }

    #[test]
    fn long_callbacks_are_rendered_in_chunks() {
        let (mut c, mut tx) = core(2, Routes::default());
        play(&mut tx, tone(48_000, 1.0, 1_000.0), 1.0);
        let (hush, fault, stats) = (
            Hush::default(),
            AtomicBool::new(false),
            CallbackStats::default(),
        );
        let mut out = vec![0.0f32; (CHUNK * 2 + 100) * 2];
        c.callback(&mut out, &hush, &fault, &stats);
        // Sound all the way through, past the first chunk.
        let tail = &out[(CHUNK * 2) * 2..];
        assert!(tail.iter().any(|s| s.abs() > 0.1));
    }
}

/// On real hardware: `cargo test -p af2-engine -- --ignored device_`.
#[cfg(test)]
mod device_tests {
    use crate::{DeviceChoice, Engine, InputPick, RecordSource};
    use std::time::{Duration, Instant};

    #[test]
    #[ignore = "needs an audio device"]
    fn device_output_runs_and_reports_its_buffer() {
        let mut e = Engine::open(&DeviceChoice::default()).expect("open");
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(3) {
            assert_eq!(e.check(Instant::now()), None);
            std::thread::sleep(Duration::from_millis(50));
        }
        let b = e.real_buffer().expect("the device called back");
        eprintln!(
            "{} on {}: {} Hz, {} ch, {:?}, real buffer {b}",
            e.driver, e.device, e.rate, e.channels, e.format
        );
        assert!(b > 0);
    }

    #[test]
    #[ignore = "needs an audio device with an input"]
    fn device_records_picked_channels_from_one_stream() {
        let mut e = Engine::open(&DeviceChoice::default()).expect("open");
        let srcs = [
            RecordSource::Input,
            RecordSource::Channels(InputPick {
                device: None,
                channels: vec![0],
            }),
            RecordSource::Channels(InputPick {
                device: None,
                channels: vec![0, 1],
            }),
        ];
        e.start_recording_from(&srcs).expect("record");
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(1500) {
            e.collect();
            std::thread::sleep(Duration::from_millis(20));
        }
        let takes = e.stop_recording_all();
        assert_eq!(takes.len(), 3);
        let secs = |k: usize| takes[k].buf.frames() as f64 / takes[k].buf.rate as f64;
        eprintln!(
            "takes: {} ch {:.2} s, {} ch, {} ch",
            takes[0].buf.num_channels(),
            secs(0),
            takes[1].buf.num_channels(),
            takes[2].buf.num_channels()
        );
        assert!(secs(0) > 1.0);
        assert_eq!(takes[1].buf.num_channels(), 1);
        assert_eq!(takes[2].buf.num_channels(), 2);
        assert_eq!(takes[1].buf.frames(), takes[0].buf.frames());
        assert_eq!(takes[1].buf.channels[0], takes[0].buf.channels[0]);
    }
}

#[cfg(test)]
mod device_buffer_tests {
    use crate::{DeviceChoice, Engine};

    #[test]
    #[ignore = "needs an audio device"]
    fn device_opens_whatever_buffer_size_is_asked() {
        let e = Engine::open(&DeviceChoice {
            buffer: Some(256),
            rate: Some(96_000),
            ..Default::default()
        })
        .expect("open");
        eprintln!("buffer {:?}, rate {}, note {:?}", e.buffer, e.rate, e.note);
    }
}
