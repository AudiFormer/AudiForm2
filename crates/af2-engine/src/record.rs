// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Recording from input devices and system audio.
//!
//! Sources on one input device share one input stream opened with all of
//! the device's channels; each take picks its own channels from it when
//! recording stops. On ASIO the input is the driver the output already
//! uses (its input and output streams must share one device value).
//!
//! When a device asks for a restart (an ASIO driver's reset request) or
//! fails, the stream is opened again with the same format and the gap is
//! filled with silence so the take stays in time; a device that changed
//! its sample rate ends the take there instead, with a note.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use af2_core::AudioBuffer;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use crate::capture::{self, RecordSource};
use crate::devices::{self, HostKind};
use crate::health;
use crate::{store_max_f32, Take};

/// Restarts allowed in one take before the device counts as gone.
const MAX_RESTARTS: u32 = 8;

enum Backend {
    Device(#[allow(dead_code)] cpal::Stream),
    Loopback(capture::LoopbackCapture),
    /// Lost, not yet (or no longer) reopened.
    None,
}

/// What each take of a recorder keeps: the index of its source in the
/// list given to [`open_all`], and its channels (None: all of them).
struct Target {
    index: usize,
    source: RecordSource,
    channels: Option<Vec<u16>>,
}

pub(crate) struct Recorder {
    backend: Backend,
    targets: Vec<Target>,
    rx: rtrb::Consumer<f32>,
    data: Vec<f32>,
    /// The take so far as min/max pairs, one per [`LIVE_BIN`] frames, for
    /// each target (drawn behind the playhead while recording).
    live: Vec<Vec<[f32; 2]>>,
    /// Frames of `data` already summed into `live`.
    scanned: usize,
    pub(crate) rate: u32,
    pub(crate) channels: usize,
    pub(crate) peak: Arc<AtomicU32>,
    /// Sum of squares and sample count of the audio drained since the
    /// meters last asked (interface thread only).
    power: (f64, u64),
    errors: Vec<String>,
    /// Input devices: how to open the stream again after a reset.
    reopen: Option<Reopen>,
    /// Set by the stream's error callback.
    reset: Arc<AtomicBool>,
    heard: Instant,
    restarts: u32,
    /// Waits between restart attempts.
    retry: health::Backoff,
    /// A problem not yet shown to the user.
    fresh: Option<String>,
    /// Input devices: where the stream also hands its buffers to input
    /// monitoring ([`crate::monitor`]), and the device's name.
    pub(crate) tap: Option<crate::monitor::Tap>,
    pub(crate) device_name: String,
}

struct Reopen {
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    name: String,
}

/// Frames of silence for a gap of `gap` at `rate` (the time no sound came
/// while the stream restarted).
/// Frames per min/max pair in a recorder's live overview.
pub const LIVE_BIN: usize = 256;

pub fn gap_frames(gap: Duration, rate: u32) -> usize {
    (gap.as_secs_f64() * rate as f64).round() as usize
}

/// A device's input stream, its ring, and its peak. Each buffer is also
/// offered to `tap` for input monitoring.
fn input_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    peak: Arc<AtomicU32>,
    reset: Arc<AtomicBool>,
    tap: crate::monitor::Tap,
) -> Result<(cpal::Stream, rtrb::Consumer<f32>), String> {
    let rate = config.sample_rate().0;
    let channels = (config.channels() as usize).max(1);
    // Ten seconds: the UI drains every frame.
    let (tx, rx) = rtrb::RingBuffer::<f32>::new(rate as usize * channels * 10);
    let cfg: cpal::StreamConfig = config.clone().into();
    macro_rules! go {
        ($t:ty) => {{
            let (mut tx, pk, rs, tp) = (tx, peak.clone(), reset.clone(), tap.clone());
            device.build_input_stream(
                &cfg,
                move |d: &[$t], _| {
                    push_input(d, &mut tx, &pk);
                    crate::monitor::push(&tp, d, channels);
                },
                move |_e: cpal::StreamError| rs.store(true, Ordering::Relaxed),
                None,
            )
        }};
    }
    let stream = match config.sample_format() {
        SampleFormat::F32 => go!(f32),
        SampleFormat::F64 => go!(f64),
        SampleFormat::I16 => go!(i16),
        SampleFormat::I32 => go!(i32),
        SampleFormat::U16 => go!(u16),
        SampleFormat::U32 => go!(u32),
        SampleFormat::I8 => go!(i8),
        SampleFormat::U8 => go!(u8),
        other => return Err(format!("Unsupported input sample format {other:?}.")),
    }
    .map_err(|e| format!("Could not open the recording device: {e}"))?;
    stream
        .play()
        .map_err(|e| format!("Could not start recording: {e}"))?;
    Ok((stream, rx))
}

/// The input callback: convert, push, note the peak. Nothing else.
fn push_input<T: SizedSample>(d: &[T], tx: &mut rtrb::Producer<f32>, peak: &AtomicU32)
where
    f32: FromSample<T>,
{
    let mut m = 0.0f32;
    for &s in d {
        let v: f32 = s.to_sample();
        m = m.max(v.abs());
        let _ = tx.push(v);
    }
    store_max_f32(peak, m);
}

/// Where an input source records from: its device and the channels it
/// takes (None: all).
pub(crate) struct Resolved {
    pub(crate) device: cpal::Device,
    pub(crate) name: String,
    pub(crate) channels: Option<Vec<u16>>,
}

pub(crate) fn resolve(
    source: &RecordSource,
    host: HostKind,
    asio: Option<&cpal::Device>,
) -> Result<Resolved, String> {
    let (want, channels) = match source {
        RecordSource::Channels(p) => (p.device.clone(), Some(p.channels.clone())),
        _ => (None, None),
    };
    if host == HostKind::Asio {
        let dev = asio.ok_or("The ASIO driver is not open.")?;
        let name = dev.name().unwrap_or_else(|_| "ASIO".into());
        if let Some(w) = want.as_deref().filter(|w| *w != name) {
            return Err(format!(
                "\"{w}\" is not the ASIO driver in use ({name}). Pick the driver's inputs on \
                 the track, or switch the driver in Settings > Audio Devices."
            ));
        }
        return Ok(Resolved {
            device: dev.clone(),
            name,
            channels,
        });
    }
    let h = cpal::default_host();
    let device = match want {
        Some(w) => h
            .input_devices()
            .ok()
            .and_then(|mut it| it.find(|d| d.name().is_ok_and(|n| n == w)))
            .ok_or_else(|| format!("The recording device \"{w}\" is not connected."))?,
        None => crate::find_input(&h).ok_or("No recording device was found.")?,
    };
    let name = device.name().unwrap_or_else(|_| "Input".into());
    Ok(Resolved {
        device,
        name,
        channels,
    })
}

impl Recorder {
    fn loopback(index: usize, source: &RecordSource) -> Result<Recorder, String> {
        // Ten seconds of 192 kHz stereo: the UI drains every frame.
        let (tx, rx) = rtrb::RingBuffer::<f32>::new(192_000 * 2 * 10);
        let peak = Arc::new(AtomicU32::new(0));
        let cap = capture::LoopbackCapture::open(source, tx, peak.clone())?;
        Ok(Recorder {
            rate: cap.rate,
            channels: 2,
            backend: Backend::Loopback(cap),
            targets: vec![Target {
                index,
                source: source.clone(),
                channels: None,
            }],
            rx,
            data: Vec::new(),
            live: Vec::new(),
            scanned: 0,
            peak,
            power: (0.0, 0),
            errors: Vec::new(),
            reopen: None,
            reset: Arc::new(AtomicBool::new(false)),
            heard: Instant::now(),
            restarts: 0,
            retry: health::Backoff::default(),
            fresh: None,
            tap: None,
            device_name: String::new(),
        })
    }

    fn input(device: cpal::Device, name: String, targets: Vec<Target>) -> Result<Recorder, String> {
        let config = device
            .default_input_config()
            .map_err(|e| format!("The recording device gave no format: {e}"))?;
        let config = cpal::SupportedStreamConfig::new(
            config.channels(),
            cpal::SampleRate(devices::sane_rate(config.sample_rate().0)),
            *config.buffer_size(),
            config.sample_format(),
        );
        let peak = Arc::new(AtomicU32::new(0));
        let reset = Arc::new(AtomicBool::new(false));
        let tap = crate::monitor::new_tap();
        let (stream, rx) =
            input_stream(&device, &config, peak.clone(), reset.clone(), tap.clone())?;
        health::log(&format!(
            "record: \"{name}\", {} Hz, {} channels, {:?}",
            config.sample_rate().0,
            config.channels(),
            config.sample_format()
        ));
        Ok(Recorder {
            backend: Backend::Device(stream),
            targets,
            rx,
            data: Vec::new(),
            live: Vec::new(),
            scanned: 0,
            rate: config.sample_rate().0,
            channels: (config.channels() as usize).max(1),
            peak,
            power: (0.0, 0),
            errors: Vec::new(),
            reopen: Some(Reopen {
                device,
                config,
                name: name.clone(),
            }),
            reset,
            heard: Instant::now(),
            restarts: 0,
            retry: health::Backoff::default(),
            fresh: None,
            tap: Some(tap),
            device_name: name,
        })
    }

    /// Moves what arrived into the take, and restarts an input that asked
    /// for it. Call every frame.
    pub(crate) fn drain(&mut self) {
        let mut got = false;
        while let Ok(s) = self.rx.pop() {
            self.data.push(s);
            self.power.0 += (s as f64) * (s as f64);
            self.power.1 += 1;
            got = true;
        }
        if got {
            self.heard = Instant::now();
        }
        self.scan();
        if self.reset.load(Ordering::Relaxed) && self.retry.due(Instant::now()) {
            self.reset.store(false, Ordering::Relaxed);
            self.restart();
        }
    }

    /// The mean square of the audio drained since the last call.
    pub(crate) fn take_mean_square(&mut self) -> Option<f32> {
        let (sum, n) = std::mem::take(&mut self.power);
        (n > 0).then(|| (sum / n as f64) as f32)
    }

    fn restart(&mut self) {
        let Some(re) = self.reopen.take() else {
            return;
        };
        // Let go of the old stream before asking for a new one.
        self.backend = Backend::None;
        while let Ok(s) = self.rx.pop() {
            self.data.push(s);
        }
        self.restarts += 1;
        if self.restarts > MAX_RESTARTS {
            self.errors.push(format!(
                "{} kept failing, so the take stopped there.",
                re.name
            ));
            self.fresh = self.errors.last().cloned();
            return;
        }
        let now_rate = re
            .device
            .default_input_config()
            .ok()
            .map(|c| devices::sane_rate(c.sample_rate().0));
        if let Some(r) = now_rate.filter(|&r| r != self.rate) {
            self.errors.push(format!(
                "The audio driver switched from {} Hz to {r} Hz while recording, so the take \
                 stopped there.",
                self.rate
            ));
            self.fresh = self.errors.last().cloned();
            return;
        }
        let tap = self.tap.clone().unwrap_or_else(crate::monitor::new_tap);
        match input_stream(
            &re.device,
            &re.config,
            self.peak.clone(),
            self.reset.clone(),
            tap,
        ) {
            Ok((stream, rx)) => {
                // Silence for the time no sound came, so the take stays in time.
                let gap = gap_frames(self.heard.elapsed(), self.rate) * self.channels;
                self.data.resize(self.data.len() + gap, 0.0);
                self.heard = Instant::now();
                self.rx = rx;
                self.backend = Backend::Device(stream);
                self.retry.reset();
                health::log(&format!(
                    "record: \"{}\" asked for a restart; the input started again",
                    re.name
                ));
                self.reopen = Some(re);
            }
            Err(e) => {
                health::log(&format!("record: restart failed: {e}"));
                self.errors
                    .push(format!("The recording device went away: {e}"));
                self.fresh = self.errors.last().cloned();
                // Try again after a wait.
                self.reset.store(true, Ordering::Relaxed);
                self.reopen = Some(re);
            }
        }
    }

    /// Sums the frames that came in since the last call into `live`
    /// (whole bins only; the rest waits for more).
    fn scan(&mut self) {
        let ch = self.channels.max(1);
        let frames = self.data.len() / ch;
        if self.live.len() != self.targets.len() {
            self.live = vec![Vec::new(); self.targets.len()];
        }
        while self.scanned + LIVE_BIN <= frames {
            let block = &self.data[self.scanned * ch..(self.scanned + LIVE_BIN) * ch];
            for (t, out) in self.targets.iter().zip(self.live.iter_mut()) {
                let (mut lo, mut hi) = (0.0f32, 0.0f32);
                let mut each = |c: usize| {
                    if c < ch {
                        for f in 0..LIVE_BIN {
                            let s = block[f * ch + c];
                            lo = lo.min(s);
                            hi = hi.max(s);
                        }
                    }
                };
                match &t.channels {
                    None => (0..ch).for_each(&mut each),
                    Some(cs) => cs.iter().for_each(|&c| each(usize::from(c))),
                }
                out.push([lo, hi]);
            }
            self.scanned += LIVE_BIN;
        }
    }

    /// The take so far for each target: (source index, min/max per
    /// [`LIVE_BIN`] frames).
    pub(crate) fn live(&self) -> impl Iterator<Item = (usize, &[[f32; 2]])> {
        self.targets
            .iter()
            .zip(&self.live)
            .map(|(t, l)| (t.index, l.as_slice()))
    }

    pub(crate) fn len_secs(&self) -> f64 {
        self.data.len() as f64 / (self.channels.max(1) as f64 * self.rate.max(1) as f64)
    }

    pub(crate) fn take_error(&mut self) -> Option<String> {
        match &self.backend {
            Backend::Loopback(c) => c.take_error().inspect(|e| self.errors.push(e.clone())),
            _ => self.fresh.take(),
        }
    }

    fn finish(mut self) -> Vec<(usize, Take)> {
        let mut note = None;
        if let Backend::Loopback(c) = &mut self.backend {
            c.stop();
            note = c.note.clone();
        }
        if let Backend::Loopback(c) = &self.backend {
            if let Some(e) = c.take_error() {
                self.errors.push(e);
            }
        }
        self.backend = Backend::None;
        while let Ok(s) = self.rx.pop() {
            self.data.push(s);
        }
        if let Some(e) = self.errors.last() {
            note = Some(e.clone());
        }
        let (rate, ch) = (self.rate, self.channels);
        let data = std::mem::take(&mut self.data);
        self.targets
            .into_iter()
            .map(|t| {
                let buf = match &t.channels {
                    None => AudioBuffer::from_interleaved(rate, ch, &data),
                    Some(c) => crate::routing::pick_channels(&data, ch, rate, c),
                };
                (
                    t.index,
                    Take {
                        source: t.source,
                        buf,
                        note: note.clone(),
                    },
                )
            })
            .collect()
    }
}

/// An input opened only to be heard (not recorded): its buffers go
/// straight to the monitor's tap.
pub(crate) struct MonitorStream {
    _stream: cpal::Stream,
    pub(crate) name: String,
    pub(crate) tap: crate::monitor::Tap,
    pub(crate) rate: u32,
    pub(crate) channels: usize,
    /// Set by the stream's error callback (a driver reset or a device gone).
    pub(crate) reset: Arc<AtomicBool>,
}

/// Opens `device` for monitoring at `rate` (the output's), in the format
/// the recorder would use when that runs at `rate`.
pub(crate) fn monitor_stream(
    device: &cpal::Device,
    name: &str,
    rate: u32,
) -> Result<MonitorStream, String> {
    let def = device
        .default_input_config()
        .map_err(|e| format!("{name} gave no input format: {e}"))?;
    let config = if devices::sane_rate(def.sample_rate().0) == rate {
        cpal::SupportedStreamConfig::new(
            def.channels(),
            cpal::SampleRate(rate),
            *def.buffer_size(),
            def.sample_format(),
        )
    } else {
        device
            .supported_input_configs()
            .ok()
            .and_then(|mut it| {
                it.find(|c| {
                    c.channels() == def.channels()
                        && c.min_sample_rate().0 <= rate
                        && rate <= c.max_sample_rate().0
                })
            })
            .map(|c| c.with_sample_rate(cpal::SampleRate(rate)))
            .ok_or_else(|| {
                format!(
                    "{name} runs at {} Hz and the output at {rate} Hz, so it can't be heard live.",
                    def.sample_rate().0
                )
            })?
    };
    let channels = (config.channels() as usize).max(1);
    let cfg: cpal::StreamConfig = config.clone().into();
    let tap = crate::monitor::new_tap();
    let reset = Arc::new(AtomicBool::new(false));
    macro_rules! go {
        ($t:ty) => {{
            let (tp, rs) = (tap.clone(), reset.clone());
            device.build_input_stream(
                &cfg,
                move |d: &[$t], _| crate::monitor::push(&tp, d, channels),
                move |_e: cpal::StreamError| rs.store(true, Ordering::Relaxed),
                None,
            )
        }};
    }
    let stream = match config.sample_format() {
        SampleFormat::F32 => go!(f32),
        SampleFormat::F64 => go!(f64),
        SampleFormat::I16 => go!(i16),
        SampleFormat::I32 => go!(i32),
        SampleFormat::U16 => go!(u16),
        SampleFormat::U32 => go!(u32),
        SampleFormat::I8 => go!(i8),
        SampleFormat::U8 => go!(u8),
        other => return Err(format!("Unsupported input sample format {other:?}.")),
    }
    .map_err(|e| format!("Could not open {name} to hear it live: {e}"))?;
    stream
        .play()
        .map_err(|e| format!("Could not start {name} to hear it live: {e}"))?;
    health::log(&format!(
        "monitor: \"{name}\", {rate} Hz, {channels} channels, {:?}",
        config.sample_format()
    ));
    Ok(MonitorStream {
        _stream: stream,
        name: name.to_string(),
        tap,
        rate,
        channels,
        reset,
    })
}

/// Opens every source: input sources grouped by device (one stream each),
/// system sources one capture each. All or nothing.
pub(crate) fn open_all(
    sources: &[RecordSource],
    host: HostKind,
    asio: Option<&cpal::Device>,
) -> Result<Vec<Recorder>, String> {
    let mut recs = Vec::new();
    let mut groups: Vec<(String, cpal::Device, Vec<Target>)> = Vec::new();
    for (index, s) in sources.iter().enumerate() {
        if s.is_system() {
            recs.push(Recorder::loopback(index, s).map_err(|e| format!("{}: {e}", s.label()))?);
            continue;
        }
        let r = resolve(s, host, asio)?;
        let t = Target {
            index,
            source: s.clone(),
            channels: r.channels,
        };
        match groups.iter_mut().find(|(n, _, _)| *n == r.name) {
            Some((_, _, ts)) => ts.push(t),
            None => groups.push((r.name, r.device, vec![t])),
        }
    }
    for (name, device, targets) in groups {
        recs.push(Recorder::input(device, name, targets)?);
    }
    Ok(recs)
}

/// Stops every recorder and returns the takes in source order.
pub(crate) fn finish_all(recs: Vec<Recorder>) -> Vec<Take> {
    let mut all: Vec<(usize, Take)> = recs.into_iter().flat_map(Recorder::finish).collect();
    all.sort_by_key(|(i, _)| *i);
    all.into_iter().map(|(_, t)| t).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_live_overview_follows_each_targets_channels() {
        let frames = LIVE_BIN * 2 + 10;
        let mut data = Vec::new();
        for f in 0..frames {
            data.push(if f < LIVE_BIN { 0.5 } else { 0.0 });
            data.push(-0.25);
        }
        let targets = vec![
            Target { index: 0, source: RecordSource::Input, channels: Some(vec![0]) },
            Target { index: 1, source: RecordSource::Input, channels: Some(vec![1]) },
        ];
        let mut r = fake(targets, 2, data);
        r.scan();
        let live: Vec<_> = r.live().map(|(i, l)| (i, l.to_vec())).collect();
        assert_eq!(live[0].1, vec![[0.0, 0.5], [0.0, 0.0]]);
        assert_eq!(live[1].1, vec![[-0.25, 0.0], [-0.25, 0.0]]);
        assert_eq!(r.scanned, LIVE_BIN * 2);
    }

    #[test]
    fn gaps_are_filled_by_the_clock() {
        assert_eq!(gap_frames(Duration::from_millis(250), 48_000), 12_000);
        assert_eq!(gap_frames(Duration::ZERO, 48_000), 0);
        assert_eq!(gap_frames(Duration::from_secs(2), 44_100), 88_200);
    }

    fn fake(targets: Vec<Target>, channels: usize, data: Vec<f32>) -> Recorder {
        let (_tx, rx) = rtrb::RingBuffer::<f32>::new(4);
        Recorder {
            backend: Backend::None,
            targets,
            rx,
            data,
            live: Vec::new(),
            scanned: 0,
            rate: 48_000,
            channels,
            peak: Arc::new(AtomicU32::new(0)),
            power: (0.0, 0),
            errors: Vec::new(),
            reopen: None,
            reset: Arc::new(AtomicBool::new(false)),
            heard: Instant::now(),
            restarts: 0,
            retry: health::Backoff::default(),
            fresh: None,
            tap: None,
            device_name: String::new(),
        }
    }

    #[test]
    fn tracks_on_one_device_get_their_own_channels_in_source_order() {
        use crate::routing::InputPick;
        let pick = |c: Vec<u16>| {
            RecordSource::Channels(InputPick {
                device: Some("SSL 2+ MKII".into()),
                channels: c,
            })
        };
        // Two channels, three frames; sources 0 and 2 on this device,
        // source 1 elsewhere.
        let a = fake(
            vec![
                Target {
                    index: 2,
                    source: pick(vec![1]),
                    channels: Some(vec![1]),
                },
                Target {
                    index: 0,
                    source: pick(vec![0, 1]),
                    channels: Some(vec![0, 1]),
                },
            ],
            2,
            vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        );
        let b = fake(
            vec![Target {
                index: 1,
                source: RecordSource::Input,
                channels: None,
            }],
            1,
            vec![0.5, 0.5],
        );
        let takes = finish_all(vec![a, b]);
        assert_eq!(takes.len(), 3);
        assert_eq!(
            takes[0].buf.channels,
            vec![vec![1.0, 3.0, 5.0], vec![2.0, 4.0, 6.0]]
        );
        assert_eq!(takes[1].source, RecordSource::Input);
        assert_eq!(takes[1].buf.channels, vec![vec![0.5, 0.5]]);
        assert_eq!(takes[2].buf.channels, vec![vec![2.0, 4.0, 6.0]]);
    }

    #[test]
    fn a_failed_take_keeps_what_it_got_and_says_why() {
        let mut r = fake(
            vec![Target {
                index: 0,
                source: RecordSource::Input,
                channels: None,
            }],
            1,
            vec![0.1; 10],
        );
        r.errors.push("The recording device went away.".into());
        let t = finish_all(vec![r]);
        assert_eq!(t[0].buf.frames(), 10);
        assert_eq!(
            t[0].note.as_deref(),
            Some("The recording device went away.")
        );
    }

    #[test]
    fn asio_inputs_must_be_the_driver_in_use() {
        use crate::routing::InputPick;
        let s = RecordSource::Channels(InputPick {
            device: Some("Other ASIO".into()),
            channels: vec![0],
        });
        // No driver open at all.
        let e = resolve(&s, HostKind::Asio, None).err().unwrap();
        assert_eq!(e, "The ASIO driver is not open.");
    }
}
