// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Writing audio files: WAV (our own streaming RIFF/RF64 writer) and FLAC
//! (`flacenc`), with channel and sample-rate conversion on the way (the
//! length kept exact), dither for integer depths, an optional loudness
//! target with a true-peak ceiling, markers and tags, and a read-back
//! check of the finished file.

pub mod flac;
pub mod wav;

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use af27_core::AudioBuffer;
use serde::{Deserialize, Serialize};

/// File formats AudiForm 2 writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Container {
    #[default]
    Wav,
    Flac,
}

impl Container {
    pub const ALL: [Container; 2] = [Container::Wav, Container::Flac];

    pub fn label(self) -> &'static str {
        match self {
            Container::Wav => "WAV",
            Container::Flac => "FLAC",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Container::Wav => "wav",
            Container::Flac => "flac",
        }
    }

    /// The format a file name asks for by its extension.
    pub fn from_path(p: &Path) -> Option<Container> {
        match p.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "wav" | "wave" | "bwf" | "rf64" => Some(Container::Wav),
            "flac" => Some(Container::Flac),
            _ => None,
        }
    }

    pub fn formats(self) -> &'static [SampleFormat] {
        match self {
            Container::Wav => &[
                SampleFormat::U8,
                SampleFormat::I16,
                SampleFormat::I24,
                SampleFormat::F32,
            ],
            Container::Flac => &[SampleFormat::I16, SampleFormat::I24],
        }
    }
}

/// How samples are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SampleFormat {
    U8,
    I16,
    #[default]
    I24,
    F32,
}

impl SampleFormat {
    pub fn bits(self) -> u16 {
        match self {
            SampleFormat::U8 => 8,
            SampleFormat::I16 => 16,
            SampleFormat::I24 => 24,
            SampleFormat::F32 => 32,
        }
    }

    pub fn bytes(self) -> usize {
        self.bits() as usize / 8
    }

    pub fn is_int(self) -> bool {
        self != SampleFormat::F32
    }

    pub fn label(self) -> &'static str {
        match self {
            SampleFormat::U8 => "8-bit",
            SampleFormat::I16 => "16-bit",
            SampleFormat::I24 => "24-bit",
            SampleFormat::F32 => "32-bit float",
        }
    }
}

/// The channels to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ChannelChoice {
    #[default]
    Keep,
    Mono,
    Stereo,
}

impl ChannelChoice {
    pub const ALL: [ChannelChoice; 3] = [
        ChannelChoice::Keep,
        ChannelChoice::Mono,
        ChannelChoice::Stereo,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ChannelChoice::Keep => "Keep",
            ChannelChoice::Mono => "Mono (mixed)",
            ChannelChoice::Stereo => "Stereo",
        }
    }

    pub fn count(self, source: usize) -> usize {
        match self {
            ChannelChoice::Keep => source,
            ChannelChoice::Mono => 1,
            ChannelChoice::Stereo => 2,
        }
    }
}

/// Sample rates offered (None keeps the source's).
pub const RATES: [Option<u32>; 6] = [
    None,
    Some(44_100),
    Some(48_000),
    Some(88_200),
    Some(96_000),
    Some(192_000),
];

/// A file tag, named as in Vorbis comments and RIFF INFO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tag {
    Title,
    Artist,
    Album,
    Comment,
    Date,
    Genre,
    Track,
    Copyright,
    Software,
}

impl Tag {
    pub const ALL: [Tag; 9] = [
        Tag::Title,
        Tag::Artist,
        Tag::Album,
        Tag::Comment,
        Tag::Date,
        Tag::Genre,
        Tag::Track,
        Tag::Copyright,
        Tag::Software,
    ];

    pub fn vorbis_name(self) -> &'static str {
        match self {
            Tag::Title => "TITLE",
            Tag::Artist => "ARTIST",
            Tag::Album => "ALBUM",
            Tag::Comment => "COMMENT",
            Tag::Date => "DATE",
            Tag::Genre => "GENRE",
            Tag::Track => "TRACKNUMBER",
            Tag::Copyright => "COPYRIGHT",
            Tag::Software => "ENCODER",
        }
    }
}

/// A loudness to reach, and the true peak not to pass.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LoudnessTarget {
    pub lufs: f64,
    pub true_peak_db: f64,
}

/// Named loudness targets.
pub const LOUDNESS_PRESETS: [(&str, LoudnessTarget); 3] = [
    (
        "EBU R128 (-23 LUFS, -1 dBTP)",
        LoudnessTarget {
            lufs: -23.0,
            true_peak_db: -1.0,
        },
    ),
    (
        "ATSC A/85 (-24 LUFS, -2 dBTP)",
        LoudnessTarget {
            lufs: -24.0,
            true_peak_db: -2.0,
        },
    ),
    (
        "Podcast (-16 LUFS, -1 dBTP)",
        LoudnessTarget {
            lufs: -16.0,
            true_peak_db: -1.0,
        },
    ),
];

/// Everything about how a file is written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    pub container: Container,
    pub rate: Option<u32>,
    pub channels: ChannelChoice,
    pub format: SampleFormat,
    pub dither: bool,
    /// FLAC compression, 0 to 8.
    pub flac_level: u8,
    pub loudness: Option<LoudnessTarget>,
    /// WAV speaker mask for more than two channels (None: the usual one).
    pub channel_mask: Option<u32>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            container: Container::Wav,
            rate: None,
            channels: ChannelChoice::Keep,
            format: SampleFormat::I24,
            dither: true,
            flac_level: 8,
            loudness: None,
            channel_mask: None,
        }
    }
}

impl Options {
    /// The format actually written (FLAC has no 8-bit or float here).
    pub fn effective_format(&self) -> SampleFormat {
        if self.container.formats().contains(&self.format) {
            self.format
        } else if self.format == SampleFormat::U8 {
            SampleFormat::I16
        } else {
            SampleFormat::I24
        }
    }

    /// Whether the audio must be whole in memory before writing (rate
    /// change or loudness); otherwise it can be written as it renders.
    pub fn needs_whole(&self, source_rate: u32) -> bool {
        self.loudness.is_some() || self.rate.is_some_and(|r| r != source_rate)
    }
}

/// A marker to write, in seconds from the file's start.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkerOut {
    pub name: String,
    pub time: f64,
    pub length: f64,
}

/// Markers and tags for a file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Meta {
    pub markers: Vec<MarkerOut>,
    pub tags: Vec<(Tag, String)>,
}

/// What loudness normalisation did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessResult {
    /// Before the change (None: too quiet or short to measure).
    pub measured_lufs: Option<f64>,
    pub gain_db: f64,
    pub result_lufs: Option<f64>,
    pub true_peak_db: f64,
    /// The gain was held back so the true peak stays under the ceiling.
    pub peak_limited: bool,
}

/// What an export wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub path: PathBuf,
    pub frames: u64,
    pub rate: u32,
    pub channels: usize,
    pub bytes: u64,
    /// Samples over full scale (clipped in integer files).
    pub clipped: u64,
    /// Highest sample written, dBFS.
    pub peak_db: f64,
    pub loudness: Option<LoudnessResult>,
}

impl Report {
    /// A line about clipping, if there was any.
    pub fn clip_note(&self, float: bool) -> Option<String> {
        if self.clipped == 0 {
            return None;
        }
        Some(if float {
            format!(
                "The audio goes {:.1} dB over full scale ({} samples); the float file keeps it, but other formats would clip.",
                self.peak_db, self.clipped
            )
        } else {
            format!(
                "Clipped: {} samples went over full scale, by up to {:.1} dB.",
                self.clipped, self.peak_db
            )
        })
    }
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-10).log10()
}

// ----------------------------------------------------------- dither

/// Turns float samples into integers of a bit depth, with optional TPDF
/// dither of +-1 LSB (the difference of two uniform values).
#[derive(Debug, Clone)]
pub struct Quantizer {
    scale: f64,
    min: f64,
    max: f64,
    dither: bool,
    rng: u32,
}

impl Quantizer {
    pub fn new(bits: u16, dither: bool) -> Quantizer {
        let scale = (1u64 << (bits - 1)) as f64;
        Quantizer {
            scale,
            min: -scale,
            max: scale - 1.0,
            dither,
            rng: 0x2545_f491,
        }
    }

    fn uniform(&mut self) -> f64 {
        // xorshift32
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng as f64 / (u32::MAX as f64 + 1.0)
    }

    /// The next dither value, in LSBs: triangular, strictly inside +-1.
    pub fn noise(&mut self) -> f64 {
        self.uniform() - self.uniform()
    }

    pub fn quantize(&mut self, s: f32) -> i32 {
        let n = if self.dither { self.noise() } else { 0.0 };
        (s as f64 * self.scale + n)
            .round()
            .clamp(self.min, self.max) as i32
    }
}

// ----------------------------------------------------------- writing

enum Sink {
    Wav(wav::WavWriter<BufWriter<File>>),
    Flac(Box<flac::FlacWriter<BufWriter<File>>>),
}

/// An open file being written block by block.
pub struct Exporter {
    path: PathBuf,
    sink: Sink,
    format: SampleFormat,
    rate: u32,
    channels: usize,
    quant: Option<Quantizer>,
    ints: Vec<i32>,
    frames: u64,
    clipped: u64,
    peak: f32,
}

impl Exporter {
    /// Creates (or replaces) `path`. Markers and tags go in the header.
    pub fn create(
        path: &Path,
        container: Container,
        format: SampleFormat,
        rate: u32,
        channels: usize,
        opts: &Options,
        meta: &Meta,
    ) -> Result<Exporter, String> {
        let open_err = |e: std::io::Error| format!("Could not write {}: {e}", path.display());
        if channels == 0 || channels > u16::MAX as usize {
            return Err("Nothing to write: no channels.".into());
        }
        if container == Container::Flac && channels > flac::MAX_CHANNELS {
            return Err(format!(
                "FLAC holds at most {} channels; this has {channels}. Use WAV.",
                flac::MAX_CHANNELS
            ));
        }
        let file = BufWriter::with_capacity(1 << 20, File::create(path).map_err(open_err)?);
        let sink = match container {
            Container::Wav => {
                let cues: Vec<wav::CuePoint> = meta
                    .markers
                    .iter()
                    .filter(|m| m.time >= 0.0)
                    .enumerate()
                    .filter_map(|(i, m)| {
                        let frame = (m.time * rate as f64).round();
                        (frame <= u32::MAX as f64).then(|| wav::CuePoint {
                            id: i as u32 + 1,
                            frame: frame as u32,
                            length: (m.length.max(0.0) * rate as f64)
                                .round()
                                .min(u32::MAX as f64) as u32,
                            label: m.name.clone(),
                        })
                    })
                    .collect();
                let spec = wav::WavSpec {
                    rate,
                    channels: channels as u16,
                    format,
                    channel_mask: opts.channel_mask,
                };
                Sink::Wav(wav::WavWriter::new(file, spec, &meta.tags, &cues).map_err(open_err)?)
            }
            Container::Flac => Sink::Flac(Box::new(flac::FlacWriter::new(
                file,
                rate,
                channels,
                format.bits() as usize,
                opts.flac_level,
                &meta.tags,
            )?)),
        };
        Ok(Exporter {
            path: path.to_path_buf(),
            sink,
            format,
            rate,
            channels,
            quant: format
                .is_int()
                .then(|| Quantizer::new(format.bits(), opts.dither)),
            ints: Vec::new(),
            frames: 0,
            clipped: 0,
            peak: 0.0,
        })
    }

    /// Writes interleaved float frames.
    pub fn push(&mut self, interleaved: &[f32]) -> Result<(), String> {
        for &s in interleaved {
            let a = s.abs();
            if a > 1.0 {
                self.clipped += 1;
            }
            if a > self.peak {
                self.peak = a;
            }
        }
        self.frames += (interleaved.len() / self.channels) as u64;
        let werr = |e: std::io::Error| format!("Could not write {}: {e}", self.path.display());
        match (&mut self.quant, &mut self.sink) {
            (None, Sink::Wav(w)) => w.write_float(interleaved).map_err(werr),
            (Some(q), sink) => {
                self.ints.clear();
                self.ints.extend(interleaved.iter().map(|&s| q.quantize(s)));
                match sink {
                    Sink::Wav(w) => w.write_int(&self.ints).map_err(werr),
                    Sink::Flac(f) => f.write(&self.ints),
                }
            }
            (None, Sink::Flac(_)) => Err("FLAC needs an integer depth".into()),
        }
    }

    /// Pushes a whole buffer in blocks.
    pub fn push_buffer(&mut self, buf: &AudioBuffer) -> Result<(), String> {
        const BLOCK: usize = 8192;
        let ch = buf.num_channels();
        let mut inter = Vec::with_capacity(BLOCK * ch);
        let mut at = 0;
        while at < buf.frames() {
            let n = BLOCK.min(buf.frames() - at);
            inter.clear();
            for i in at..at + n {
                for c in &buf.channels {
                    inter.push(c[i]);
                }
            }
            self.push(&inter)?;
            at += n;
        }
        Ok(())
    }

    /// Finishes the file.
    pub fn finish(self) -> Result<Report, String> {
        let werr = |e: std::io::Error| format!("Could not write {}: {e}", self.path.display());
        let bytes = match self.sink {
            Sink::Wav(w) => w.finish().map_err(werr)?.1,
            Sink::Flac(f) => f.finish()?.1,
        };
        let _ = self.format;
        Ok(Report {
            path: self.path,
            frames: self.frames,
            rate: self.rate,
            channels: self.channels,
            bytes,
            clipped: self.clipped,
            peak_db: db(self.peak as f64),
            loudness: None,
        })
    }
}

// ------------------------------------------------------- conversion

/// Folds or widens to `n` channels. Surround folds to stereo with the
/// usual coefficients (centre and surrounds at -3 dB, no LFE).
pub fn convert_channels(buf: &AudioBuffer, n: usize) -> AudioBuffer {
    let have = buf.num_channels();
    if have == n || have == 0 {
        return buf.clone();
    }
    if n == 2 && have > 2 {
        let frames = buf.frames();
        let mut l = vec![0.0f32; frames];
        let mut r = vec![0.0f32; frames];
        let k = std::f32::consts::FRAC_1_SQRT_2;
        for (ci, c) in buf.channels.iter().enumerate() {
            let (gl, gr) = match ci {
                0 => (1.0, 0.0),
                1 => (0.0, 1.0),
                2 => (k, k),
                3 => (0.0, 0.0), // LFE
                i if i % 2 == 0 => (k, 0.0),
                _ => (0.0, k),
            };
            for i in 0..frames {
                l[i] += c[i] * gl;
                r[i] += c[i] * gr;
            }
        }
        return AudioBuffer {
            rate: buf.rate,
            channels: vec![l, r],
        };
    }
    af27_dsp::resample::remix(buf, n)
}

/// 4x oversampled peak (BS.1770 annex 2 style, 12 taps per phase),
/// linear. Quick enough for whole sessions.
pub fn true_peak(buf: &AudioBuffer) -> f32 {
    const PHASES: usize = 4;
    const TAPS: usize = 12;
    // Windowed-sinc polyphase interpolator; phase 0 is the sample itself.
    let mut coef = [[0f32; TAPS]; PHASES];
    for (p, row) in coef.iter_mut().enumerate() {
        let mut sum = 0.0;
        for (k, c) in row.iter_mut().enumerate() {
            let x = (k as f64 - (TAPS / 2) as f64 + 1.0) - p as f64 / PHASES as f64;
            let sinc = if x.abs() < 1e-12 {
                1.0
            } else {
                (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
            };
            let w = 0.5 + 0.5 * (std::f64::consts::PI * x / (TAPS as f64 / 2.0 + 1.0)).cos();
            *c = (sinc * w) as f32;
            sum += sinc * w;
        }
        for c in row.iter_mut() {
            *c /= sum as f32;
        }
    }
    let mut peak = 0f32;
    for ch in &buf.channels {
        for s in ch {
            peak = peak.max(s.abs());
        }
        if ch.len() < TAPS {
            continue;
        }
        for win in ch.windows(TAPS) {
            for row in &coef[1..] {
                let v: f32 = win.iter().zip(row).map(|(a, b)| a * b).sum();
                peak = peak.max(v.abs());
            }
        }
    }
    peak
}

/// Scales `buf` towards `target`, holding back so the true peak stays
/// at or under the ceiling.
pub fn apply_loudness(buf: &mut AudioBuffer, target: LoudnessTarget) -> LoudnessResult {
    let measured = af27_dsp::loudness::integrated_loudness(buf);
    let tp = true_peak(buf) as f64;
    let mut gain_db = measured.map_or(0.0, |m| target.lufs - m);
    let mut peak_limited = false;
    let tp_db = db(tp);
    if tp > 0.0 && tp_db + gain_db > target.true_peak_db {
        gain_db = target.true_peak_db - tp_db;
        peak_limited = measured.is_some();
    }
    let g = 10f64.powf(gain_db / 20.0) as f32;
    for c in &mut buf.channels {
        for s in c.iter_mut() {
            *s *= g;
        }
    }
    LoudnessResult {
        measured_lufs: measured,
        gain_db,
        result_lufs: measured.map(|m| m + gain_db),
        true_peak_db: tp_db + gain_db,
        peak_limited,
    }
}

/// Converts channels and rate (the length kept exact) and applies the
/// loudness target.
pub fn prepare(buf: &AudioBuffer, opts: &Options) -> (AudioBuffer, Option<LoudnessResult>) {
    let n = opts.channels.count(buf.num_channels());
    let mut out = convert_channels(buf, n);
    if let Some(r) = opts.rate {
        if r != out.rate {
            let want = (buf.frames() as f64 * r as f64 / buf.rate as f64).round() as usize;
            out = af27_dsp::resample::resample(&out, r);
            for c in &mut out.channels {
                c.resize(want, 0.0);
            }
        }
    }
    let loud = opts.loudness.map(|t| apply_loudness(&mut out, t));
    (out, loud)
}

/// Writes `buf` to `path` as `opts` says, then checks the file reads
/// back at the right length.
pub fn export_buffer(
    path: &Path,
    buf: &AudioBuffer,
    opts: &Options,
    meta: &Meta,
) -> Result<Report, String> {
    let (out, loud) = prepare(buf, opts);
    let format = opts.effective_format();
    let mut ex = Exporter::create(
        path,
        opts.container,
        format,
        out.rate,
        out.num_channels(),
        opts,
        meta,
    )?;
    ex.push_buffer(&out)?;
    let mut rep = ex.finish()?;
    rep.loudness = loud;
    readback_check(path, rep.frames, rep.channels)?;
    Ok(rep)
}

/// Checks a written file decodes to `frames` frames of `channels`
/// channels, decoding packet by packet (nothing kept in memory). RF64
/// files are checked from their headers.
pub fn readback_check(path: &Path, frames: u64, channels: usize) -> Result<(), String> {
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::errors::Error as SymError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let fail = |m: String| format!("The check after saving failed for {}: {m}", path.display());
    if Container::from_path(path) == Some(Container::Wav) || path.extension().is_none() {
        if let Ok(info) = wav::inspect(path) {
            if info.channels as usize != channels {
                return Err(fail(format!(
                    "{} channels, expected {channels}",
                    info.channels
                )));
            }
            if info.frames() != frames {
                return Err(fail(format!("{} frames, expected {frames}", info.frames())));
            }
            if info.rf64 {
                return Ok(());
            }
        }
    }
    let file = File::open(path).map_err(|e| fail(e.to_string()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(e) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(e);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| fail(e.to_string()))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| fail("no audio track".into()))?
        .clone();
    let mut dec = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions { verify: true })
        .map_err(|e| fail(e.to_string()))?;
    let mut got = 0u64;
    let mut got_ch = 0usize;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(fail(e.to_string())),
        };
        if packet.track_id() != track.id {
            continue;
        }
        let d = dec.decode(&packet).map_err(|e| fail(e.to_string()))?;
        got += d.frames() as u64;
        got_ch = d.spec().channels.count();
    }
    if frames > 0 && got_ch != channels {
        return Err(fail(format!("{got_ch} channels, expected {channels}")));
    }
    if got != frames {
        return Err(fail(format!("decoded {got} frames, expected {frames}")));
    }
    Ok(())
}

/// About how many bytes a file will take.
pub fn estimate_bytes(opts: &Options, frames: u64, channels: usize) -> u64 {
    let ch = opts.channels.count(channels) as u64;
    let pcm = frames * ch * opts.effective_format().bytes() as u64;
    match opts.container {
        Container::Wav => pcm + 100,
        // Typical music and speech land near 55-65 % of PCM.
        Container::Flac => pcm * (66 - opts.flac_level.min(8) as u64) / 100 + 8192,
    }
}

/// "12.3 MB" and the like.
pub fn human_bytes(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{b:.0} bytes")
    }
}

/// A name safe on every file system (no path separators or reserved
/// characters).
pub fn safe_file_name(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let t = t.trim().trim_end_matches('.').trim();
    if t.is_empty() {
        "Untitled".into()
    } else {
        t.to_string()
    }
}

/// A stem's file name: "Session - 01 Track.wav". `number` counts from 1
/// over the tracks that are written.
pub fn stem_file_name(session: &str, number: usize, track: &str, container: Container) -> String {
    format!(
        "{} - {number:02} {}.{}",
        safe_file_name(session),
        safe_file_name(track),
        container.extension()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, ch: usize, secs: f64, amp: f32) -> AudioBuffer {
        let n = (rate as f64 * secs) as usize;
        let mut b = AudioBuffer::new(rate, ch, n);
        for (ci, c) in b.channels.iter_mut().enumerate() {
            for (i, s) in c.iter_mut().enumerate() {
                let t = i as f64 / rate as f64;
                *s = amp
                    * ((2.0 * std::f64::consts::PI * (440.0 + 110.0 * ci as f64) * t).sin() as f32);
            }
        }
        b
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("af27-export-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    #[test]
    fn dither_stays_within_one_lsb() {
        let mut q = Quantizer::new(16, true);
        let mut sum = 0.0;
        for _ in 0..100_000 {
            let n = q.noise();
            assert!(n.abs() < 1.0, "{n}");
            sum += n;
        }
        assert!((sum / 100_000.0).abs() < 0.01);
        // The written value is the rounded sample plus that noise: never
        // more than 1.5 LSB from the exact value, 0.5 without dither.
        let mut plain = Quantizer::new(16, false);
        for i in 0..20_000 {
            let s = (i as f32 * 0.000_37).sin() * 0.9;
            let exact = s as f64 * 32768.0;
            assert!((q.quantize(s) as f64 - exact).abs() <= 1.5);
            assert!((plain.quantize(s) as f64 - exact).abs() <= 0.5);
        }
        // Dither of silence never strays past one step.
        let mut q8 = Quantizer::new(8, true);
        assert!((0..10_000).all(|_| q8.quantize(0.0).abs() <= 1));
        // Full scale clamps.
        assert_eq!(plain.quantize(1.5), 32767);
        assert_eq!(plain.quantize(-1.5), -32768);
    }

    #[test]
    fn flac_round_trip_matches_samples() {
        let src = tone(44_100, 2, 1.3, 0.7);
        for (format, level) in [
            (SampleFormat::I16, 0u8),
            (SampleFormat::I16, 5),
            (SampleFormat::I24, 8),
        ] {
            let p = tmp(&format!("rt-{format:?}-{level}.flac"));
            let opts = Options {
                container: Container::Flac,
                format,
                dither: false,
                flac_level: level,
                ..Default::default()
            };
            let meta = Meta {
                tags: vec![(Tag::Title, "Round trip".into())],
                ..Default::default()
            };
            let rep = export_buffer(&p, &src, &opts, &meta).unwrap();
            assert_eq!(rep.frames, src.frames() as u64);
            let back = crate::decode_file(&p).unwrap();
            assert_eq!(back.rate, 44_100);
            assert_eq!(back.num_channels(), 2);
            assert_eq!(back.frames(), src.frames());
            // Lossless: exactly the quantised input.
            let mut q = Quantizer::new(format.bits(), false);
            let scale = (1u64 << (format.bits() - 1)) as f32;
            for i in 0..src.frames() {
                for c in 0..2 {
                    let want = q.quantize(src.channels[c][i]) as f32 / scale;
                    assert_eq!(back.channels[c][i], want, "{format:?} frame {i}");
                }
            }
            let bytes = std::fs::read(&p).unwrap();
            assert_eq!(&bytes[..4], b"fLaC");
            assert!(bytes.windows(16).any(|w| w == b"TITLE=Round trip"));
        }
    }

    #[test]
    fn higher_levels_are_not_bigger() {
        let src = tone(48_000, 2, 2.0, 0.5);
        let size = |level| {
            let p = tmp(&format!("lvl-{level}.flac"));
            let opts = Options {
                container: Container::Flac,
                format: SampleFormat::I16,
                flac_level: level,
                ..Default::default()
            };
            export_buffer(&p, &src, &opts, &Meta::default())
                .unwrap()
                .bytes
        };
        assert!(size(8) <= size(0));
    }

    #[test]
    fn wav_export_converts_and_keeps_exact_length() {
        let src = tone(44_100, 6, 1.0, 0.3);
        let p = tmp("conv.wav");
        let opts = Options {
            rate: Some(48_000),
            channels: ChannelChoice::Stereo,
            format: SampleFormat::I16,
            ..Default::default()
        };
        let meta = Meta {
            markers: vec![MarkerOut {
                name: "Half".into(),
                time: 0.5,
                length: 0.25,
            }],
            ..Default::default()
        };
        let rep = export_buffer(&p, &src, &opts, &meta).unwrap();
        assert_eq!(rep.frames, 48_000);
        assert_eq!(rep.channels, 2);
        let info = wav::inspect(&p).unwrap();
        assert_eq!(info.cues[0].frame, 24_000);
        assert_eq!(info.cues[0].length, 12_000);
        assert_eq!(info.cues[0].label, "Half");
        let back = crate::decode_file(&p).unwrap();
        assert_eq!(
            (back.rate, back.num_channels(), back.frames()),
            (48_000, 2, 48_000)
        );
    }

    #[test]
    fn every_wav_depth_reads_back() {
        let src = tone(22_050, 1, 0.2, 0.5);
        for f in Container::Wav.formats() {
            let p = tmp(&format!("d-{f:?}.wav"));
            let opts = Options {
                format: *f,
                ..Default::default()
            };
            export_buffer(&p, &src, &opts, &Meta::default()).unwrap();
            let back = crate::decode_file(&p).unwrap();
            assert_eq!(back.frames(), src.frames());
            let tol = match f {
                SampleFormat::U8 => 0.02,
                SampleFormat::F32 => 1e-7,
                _ => 1e-3,
            };
            for i in (0..src.frames()).step_by(31) {
                assert!(
                    (back.channels[0][i] - src.channels[0][i]).abs() < tol,
                    "{f:?}"
                );
            }
        }
    }

    #[test]
    fn loudness_target_and_peak_ceiling() {
        // A quiet tone comes up to -16 LUFS.
        let mut b = tone(48_000, 2, 3.0, 0.05);
        let r = apply_loudness(
            &mut b,
            LoudnessTarget {
                lufs: -16.0,
                true_peak_db: -1.0,
            },
        );
        assert!(!r.peak_limited);
        let after = af27_dsp::loudness::integrated_loudness(&b).unwrap();
        assert!((after + 16.0).abs() < 0.1, "{after}");
        // A loud target the peaks cannot reach stops at the ceiling.
        let mut b = tone(48_000, 2, 3.0, 0.05);
        let r = apply_loudness(
            &mut b,
            LoudnessTarget {
                lufs: 0.0,
                true_peak_db: -1.0,
            },
        );
        assert!(r.peak_limited);
        assert!(db(true_peak(&b) as f64) <= -0.99);
    }

    #[test]
    fn true_peak_sees_between_samples() {
        // A tone at a quarter of the rate, phased so samples miss the crest.
        let n = 4800;
        let b = AudioBuffer {
            rate: 48_000,
            channels: vec![(0..n)
                .map(|i| {
                    (std::f64::consts::FRAC_PI_2 * i as f64 + std::f64::consts::FRAC_PI_4).sin()
                        as f32
                })
                .collect()],
        };
        let sample_peak = b.channels[0].iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!(sample_peak < 0.72);
        assert!(true_peak(&b) > 0.95);
    }

    #[test]
    fn stems_are_named_session_number_track() {
        assert_eq!(
            stem_file_name("My Show", 1, "Host", Container::Wav),
            "My Show - 01 Host.wav"
        );
        assert_eq!(
            stem_file_name("Ep: 4?", 12, "Guest/2", Container::Flac),
            "Ep_ 4_ - 12 Guest_2.flac"
        );
        assert_eq!(
            stem_file_name("", 3, " ", Container::Wav),
            "Untitled - 03 Untitled.wav"
        );
    }

    #[test]
    fn container_from_typed_name() {
        assert_eq!(
            Container::from_path(Path::new("a.FLAC")),
            Some(Container::Flac)
        );
        assert_eq!(
            Container::from_path(Path::new("a.wav")),
            Some(Container::Wav)
        );
        assert_eq!(Container::from_path(Path::new("a.mp3")), None);
    }

    #[test]
    fn size_estimate() {
        let o = Options {
            format: SampleFormat::I16,
            ..Default::default()
        };
        assert_eq!(estimate_bytes(&o, 44_100, 2), 176_500);
        let f = Options {
            container: Container::Flac,
            ..o
        };
        assert!(estimate_bytes(&f, 44_100, 2) < 176_400);
    }
}
