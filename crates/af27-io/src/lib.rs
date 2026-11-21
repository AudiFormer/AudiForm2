// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Reading any common audio file into an [`AudioBuffer`], and writing WAV.

pub mod chunked;
pub mod fallback;
pub mod export;

use std::fs::File;
use std::path::Path;

use af27_core::AudioBuffer;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub mod meta;
mod ogg_chain;
mod opus;
mod riff;

/// File extensions the Files panel offers to open.
pub const EXTENSIONS: &[&str] = &[
    "wav", "wave", "bwf", "rf64", "aif", "aiff", "aifc", "flac", "mp3", "mp2", "ogg", "oga",
    "opus", "mogg", "m4a", "mp4", "m4b", "mov", "aac", "caf", "mka", "mkv", "webm", "dts",
];

/// A decoded file with what went wrong along the way.
pub(crate) struct Decoded {
    pub buf: AudioBuffer,
    /// Packets that could not be decoded and were skipped or concealed.
    pub damaged: usize,
    /// Other things worth telling the user (chained streams, a cut-off end).
    pub notes: Vec<String>,
}

impl Decoded {
    fn clean(buf: AudioBuffer) -> Decoded {
        Decoded {
            buf,
            damaged: 0,
            notes: Vec::new(),
        }
    }
}

/// An opened file: its audio, how cleanly it decoded, and its markers,
/// chapters and tags.
pub struct Opened {
    pub buf: AudioBuffer,
    /// Damaged packets that were skipped (or concealed) while decoding.
    pub damaged_packets: usize,
    /// Other decoding notes, in plain words.
    pub notes: Vec<String>,
    pub info: meta::FileInfo,
}

impl Opened {
    /// A one-line notice for the user when the file did not decode
    /// cleanly, e.g. "12 damaged packets were skipped".
    pub fn notice(&self) -> Option<String> {
        let mut parts = Vec::new();
        match self.damaged_packets {
            0 => {}
            1 => parts.push("1 damaged packet was skipped".to_string()),
            n => parts.push(format!("{n} damaged packets were skipped")),
        }
        parts.extend(self.notes.iter().cloned());
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(". "))
        }
    }
}

/// Opens a file for editing: decodes it (see [`decode_file`]) and reads
/// its markers, chapters and tags.
pub fn open_file(path: &Path) -> Result<Opened, String> {
    let d = decode_checked(path)?;
    let info = meta::read_info(path, d.buf.rate);
    Ok(Opened {
        buf: d.buf,
        damaged_packets: d.damaged,
        notes: d.notes,
        info,
    })
}

/// Decodes a whole file into memory as 32-bit float, keeping every
/// channel (5.1, 7.1 and other multichannel files stay multichannel).
///
/// Handles, beyond the formats the general decoders read:
/// * DTS streams hidden in WAV files (DTS music CDs) and raw `.dts`;
/// * `.mogg` multitrack Ogg files (unencrypted ones);
/// * Ogg Opus, including chained and multichannel files.
pub fn decode_file(path: &Path) -> Result<AudioBuffer, String> {
    decode_checked(path).map(|d| d.buf)
}

fn decode_checked(path: &Path) -> Result<Decoded, String> {
    match decode_own(path) {
        Ok(d) => Ok(d),
        // Missing or unreadable files: nothing else will do better.
        Err(e) if e.starts_with("Could not open") => Err(e),
        Err(e) => fallback::decode(path, &e).map(|(buf, note)| Decoded {
            buf,
            damaged: 0,
            notes: vec![note],
        }),
    }
}

/// The open decoders only.
fn decode_own(path: &Path) -> Result<Decoded, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(ext.as_str(), "wav" | "wave" | "dts" | "bwf") {
        if let Some(r) = af27_dts::decode_file(path) {
            let (rate, channels, data) =
                r.map_err(|e| format!("Could not decode the DTS audio: {e}"))?;
            return Ok(Decoded::clean(AudioBuffer::from_interleaved(
                rate, channels, &data,
            )));
        }
    }
    if ext == "mogg" {
        return decode_mogg(path);
    }
    let mut file =
        File::open(path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    // RF64/BW64 (WAV over 4 GB), which the general decoder does not open.
    let mut magic = [0u8; 4];
    if std::io::Read::read_exact(&mut file, &mut magic).is_ok()
        && (&magic == b"RF64" || &magic == b"BW64")
    {
        let mut r = std::io::BufReader::new(file);
        return riff::decode_pcm(&mut r).map(Decoded::clean);
    }
    std::io::Seek::rewind(&mut file).map_err(|e| e.to_string())?;
    decode_source(Box::new(file), Some(&ext), path)
}

/// A `.mogg` is an Ogg Vorbis stream (usually many channels, one pair per
/// instrument) after a small header: a little-endian version word and
/// the byte offset of the Ogg data. Version 10 is unencrypted; higher
/// versions are encrypted and are refused.
fn decode_mogg(path: &Path) -> Result<Decoded, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    if bytes.len() < 8 {
        return Err("The .mogg file is too short.".into());
    }
    let version = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let start = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    if bytes.starts_with(b"OggS") {
        return decode_source(Box::new(std::io::Cursor::new(bytes)), Some("ogg"), path);
    }
    if version != 0x0A {
        return Err(
            "This .mogg is encrypted. Audiform27 only opens unencrypted .mogg files.".into(),
        );
    }
    if start >= bytes.len() || !bytes[start..].starts_with(b"OggS") {
        return Err("The .mogg file's audio could not be found.".into());
    }
    let ogg = bytes[start..].to_vec();
    decode_source(Box::new(std::io::Cursor::new(ogg)), Some("ogg"), path)
}

/// Gives up on a stream after this many unreadable packets in a row.
const MAX_CONSECUTIVE_ERRORS: usize = 1000;

fn make_decoder(
    format: &dyn symphonia::core::formats::FormatReader,
) -> Result<(u32, Box<dyn symphonia::core::codecs::Decoder>), String> {
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("The file has no audio track.")?;
    let dec = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("Unsupported audio codec: {e}"))?;
    Ok((track.id, dec))
}

type Source = Box<dyn symphonia::core::io::MediaSource>;

/// One reader shared by the windows onto the links of a chained file.
#[derive(Clone)]
struct Shared(std::sync::Arc<std::sync::Mutex<Source>>);

impl std::io::Read for Shared {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().read(buf)
    }
}

impl std::io::Seek for Shared {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        self.0.lock().unwrap().seek(to)
    }
}

fn decode_source(mut src: Source, ext: Option<&str>, path: &Path) -> Result<Decoded, String> {
    if opus::sniff(&mut src) {
        return opus::decode(src);
    }
    let mut magic = [0u8; 4];
    let ogg = std::io::Read::read_exact(&mut src, &mut magic).is_ok() && &magic == b"OggS";
    std::io::Seek::rewind(&mut src).map_err(|e| e.to_string())?;
    if ogg {
        let links = ogg_chain::links(&mut src);
        if links.len() > 1 {
            let shared = Shared(std::sync::Arc::new(std::sync::Mutex::new(src)));
            let mut parts = Vec::new();
            for l in links {
                let w = ogg_chain::Window::new(shared.clone(), l);
                match decode_symphonia(Box::new(w), Some("ogg"), path) {
                    Ok(d) => parts.push(d),
                    Err(e) if !parts.is_empty() => {
                        let mut d = join(parts);
                        d.notes.push(format!(
                            "A later section of the file could not be read ({e})"
                        ));
                        return Ok(d);
                    }
                    Err(e) => return Err(e),
                }
            }
            return Ok(join(parts));
        }
    }
    decode_symphonia(src, ext, path)
}

/// Joins decoded sections end to end; later ones are fitted to the
/// first one's channel count.
fn join(parts: Vec<Decoded>) -> Decoded {
    let count = parts.len();
    let mut it = parts.into_iter();
    let Some(mut first) = it.next() else {
        return Decoded::clean(AudioBuffer::new(44_100, 1, 0));
    };
    let ch = first.buf.num_channels();
    let rate = first.buf.rate;
    for p in it {
        first.damaged += p.damaged;
        first.notes.extend(p.notes);
        if p.buf.rate != rate {
            first.notes.push(format!(
                "Part of the file is at {} Hz instead of {rate} Hz and will play at the wrong speed",
                p.buf.rate
            ));
        }
        let pc = p.buf.num_channels();
        if pc != ch {
            first.notes.push(format!(
                "Part of the file has {pc} channels instead of {ch}; it was fitted to {ch}"
            ));
        }
        for (c, dst) in first.buf.channels.iter_mut().enumerate() {
            let src = if pc == 1 { 0 } else { c };
            match p.buf.channels.get(src) {
                Some(s) => dst.extend_from_slice(s),
                None => dst.resize(dst.len() + p.buf.frames(), 0.0),
            }
        }
    }
    if count > 1 {
        first.notes.push(format!(
            "The file has {count} chained sections; they were joined"
        ));
    }
    first
}

fn decode_symphonia(src: Source, ext: Option<&str>, path: &Path) -> Result<Decoded, String> {
    let mss = MediaSourceStream::new(src, Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = ext {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|e| {
            format!(
                "{} is not an audio file this app can read ({e})",
                path.display()
            )
        })?;
    let mut format = probed.format;
    // Opus in Matroska/WebM: Symphonia demuxes it but has no decoder.
    if let Some(t) = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec == symphonia::core::codecs::CODEC_TYPE_OPUS)
    {
        let id = t.id;
        let head = t
            .codec_params
            .extra_data
            .clone()
            .ok_or("The Opus track has no header.")?;
        return opus::decode_packets(&head, || loop {
            match format.next_packet() {
                Ok(p) if p.track_id() == id => return Some(p.data.to_vec()),
                Ok(_) => continue,
                Err(_) => return None,
            }
        });
    }
    let (mut track_id, mut decoder) = make_decoder(format.as_ref())?;
    let params = decoder.codec_params().clone();
    let mut rate = params.sample_rate.unwrap_or(0);
    let mut channels = params.channels.map(|c| c.count()).unwrap_or(0);
    // Fixed once audio has been produced; later sections are fitted to it.
    let mut out_spec: Option<(u32, usize)> = None;
    // Sized from the file's own frame count when it gives one, so a long
    // file does not grow by doubling (and hold up to twice its size).
    let mut interleaved: Vec<f32> = Vec::with_capacity(
        params
            .n_frames
            .map_or(0, |n| (n as usize).saturating_mul(channels.max(1)))
            .min(1 << 31),
    );
    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut damaged = 0usize;
    let mut consecutive = 0usize;
    let mut resets = 0usize;
    let mut notes: Vec<String> = Vec::new();
    let mut rate_note = false;
    let mut chan_note = false;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            // A chained Ogg stream or a format change: pick the track
            // again, start a fresh decoder and carry on.
            Err(SymError::ResetRequired) => {
                resets += 1;
                match make_decoder(format.as_ref()) {
                    Ok((id, dec)) => {
                        track_id = id;
                        decoder = dec;
                        sample_buf = None;
                        continue;
                    }
                    Err(e) => {
                        notes.push(format!(
                            "A later section of the file could not be read ({e})"
                        ));
                        break;
                    }
                }
            }
            Err(SymError::DecodeError(_)) => {
                damaged += 1;
                consecutive += 1;
                if consecutive > MAX_CONSECUTIVE_ERRORS {
                    notes.push("The rest of the file is too damaged to read".into());
                    break;
                }
                continue;
            }
            Err(e) if !interleaved.is_empty() => {
                notes.push(format!(
                    "The file is damaged or cut short; reading stopped ({e})"
                ));
                break;
            }
            Err(e) => return Err(format!("Could not read the file: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // A damaged packet is skipped rather than failing the whole file.
            Err(SymError::DecodeError(_)) | Err(SymError::IoError(_)) => {
                damaged += 1;
                consecutive += 1;
                if consecutive > MAX_CONSECUTIVE_ERRORS {
                    notes.push("The rest of the file is too damaged to read".into());
                    break;
                }
                continue;
            }
            Err(SymError::ResetRequired) => {
                resets += 1;
                decoder.reset();
                continue;
            }
            Err(e) => return Err(format!("Could not decode the file: {e}")),
        };
        consecutive = 0;
        let spec = *decoded.spec();
        let n = spec.channels.count();
        let cap = decoded.capacity() as u64;
        let buf = match &mut sample_buf {
            Some(b) if b.capacity() >= decoded.capacity() * n => b,
            _ => sample_buf.insert(SampleBuffer::new(cap, spec)),
        };
        buf.copy_interleaved_ref(decoded);
        let (r0, c0) = *out_spec.get_or_insert((spec.rate, n));
        rate = r0;
        channels = c0;
        if spec.rate != r0 && !rate_note {
            rate_note = true;
            notes.push(format!(
                "Part of the file is at {} Hz instead of {r0} Hz and will play at the wrong speed",
                spec.rate
            ));
        }
        if n == c0 {
            interleaved.extend_from_slice(buf.samples());
        } else {
            if !chan_note {
                chan_note = true;
                notes.push(format!(
                    "Part of the file has {n} channels instead of {c0}; it was fitted to {c0}"
                ));
            }
            for frame in buf.samples().chunks_exact(n) {
                for c in 0..c0 {
                    interleaved.push(if n == 1 {
                        frame[0]
                    } else {
                        frame.get(c).copied().unwrap_or(0.0)
                    });
                }
            }
        }
    }
    if rate == 0 || channels == 0 {
        return Err("The file holds no audio.".into());
    }
    if resets > 0 && !interleaved.is_empty() {
        notes.push(format!(
            "The file has {} chained sections; they were joined",
            resets + 1
        ));
    }
    Ok(Decoded {
        buf: AudioBuffer::from_interleaved(rate, channels, &interleaved),
        damaged,
        notes,
    })
}

/// Sample formats offered when saving a WAV file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WavFormat {
    Pcm16,
    Pcm24,
    Float32,
}

impl WavFormat {
    pub const ALL: [WavFormat; 3] = [WavFormat::Pcm16, WavFormat::Pcm24, WavFormat::Float32];
    pub fn label(self) -> &'static str {
        match self {
            WavFormat::Pcm16 => "16-bit integer",
            WavFormat::Pcm24 => "24-bit integer",
            WavFormat::Float32 => "32-bit float",
        }
    }
}

/// Writes `buf` as a WAV file. Integer formats get triangular dither.
pub fn write_wav(path: &Path, buf: &AudioBuffer, fmt: WavFormat) -> Result<(), String> {
    let (bits, sample_format) = match fmt {
        WavFormat::Pcm16 => (16, hound::SampleFormat::Int),
        WavFormat::Pcm24 => (24, hound::SampleFormat::Int),
        WavFormat::Float32 => (32, hound::SampleFormat::Float),
    };
    let spec = hound::WavSpec {
        channels: buf.num_channels() as u16,
        sample_rate: buf.rate,
        bits_per_sample: bits,
        sample_format,
    };
    let err = |e: hound::Error| format!("Could not write {}: {e}", path.display());
    let mut w = hound::WavWriter::create(path, spec).map_err(err)?;
    let mut rng = 0x2545_f491_u32;
    let mut noise = move || {
        // xorshift, two uniforms summed for a triangular distribution.
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            rng as f32 / u32::MAX as f32
        };
        next() - next()
    };
    for i in 0..buf.frames() {
        for c in &buf.channels {
            let s = c[i];
            match fmt {
                WavFormat::Float32 => w.write_sample(s).map_err(err)?,
                WavFormat::Pcm16 => {
                    let v = (s * 32767.0 + noise()).round().clamp(-32768.0, 32767.0);
                    w.write_sample(v as i16).map_err(err)?
                }
                WavFormat::Pcm24 => {
                    let v = (s * 8_388_607.0 + noise())
                        .round()
                        .clamp(-8_388_608.0, 8_388_607.0);
                    w.write_sample(v as i32).map_err(err)?
                }
            }
        }
    }
    w.finalize().map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_round_trip_float_and_int() {
        let dir = std::env::temp_dir().join(format!("af27-io-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut buf = AudioBuffer::new(44_100, 2, 4410);
        for i in 0..4410 {
            let s = (i as f32 * 0.05).sin() * 0.5;
            buf.channels[0][i] = s;
            buf.channels[1][i] = -s;
        }
        for fmt in WavFormat::ALL {
            let p = dir.join(format!("{fmt:?}.wav"));
            write_wav(&p, &buf, fmt).unwrap();
            let back = decode_file(&p).unwrap();
            assert_eq!(back.rate, 44_100);
            assert_eq!(back.num_channels(), 2);
            assert_eq!(back.frames(), 4410);
            let tol = if fmt == WavFormat::Float32 {
                1e-7
            } else {
                1e-3
            };
            for i in (0..4410).step_by(97) {
                assert!((back.channels[0][i] - buf.channels[0][i]).abs() < tol);
                assert!((back.channels[1][i] - buf.channels[1][i]).abs() < tol);
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mogg_unencrypted_and_encrypted() {
        let dir = std::env::temp_dir().join(format!("af27-mogg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let enc = dir.join("enc.mogg");
        let mut b = vec![0x0Bu8, 0, 0, 0, 8, 0, 0, 0];
        b.extend_from_slice(b"OggS-not-really");
        std::fs::write(&enc, &b).unwrap();
        assert!(decode_file(&enc).unwrap_err().contains("encrypted"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dts_wav_fixture_decodes_six_channels() {
        let p =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../af27-dts/tests/media/dts_51_14bit.wav");
        let b = decode_file(&p).unwrap();
        assert_eq!((b.rate, b.num_channels()), (44_100, 6));
        assert!(b.peak() > 0.2);
    }

    fn media(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/media")
            .join(name)
    }

    #[test]
    fn chained_ogg_vorbis_keeps_every_link() {
        // Two Vorbis files back to back (0.3 s + 0.2 s at 22.05 kHz);
        // Symphonia alone would return only the last one.
        let o = open_file(&media("chained.ogg")).unwrap();
        assert_eq!(o.buf.rate, 22_050);
        let secs = o.buf.duration_secs();
        assert!((secs - 0.5).abs() < 0.03, "{secs}");
        assert!(o.notice().unwrap().contains("2 chained sections"));
    }

    #[test]
    fn opus_in_webm_opens() {
        let b = decode_file(&media("opus.webm")).unwrap();
        assert_eq!((b.rate, b.num_channels()), (48_000, 1));
        assert!((b.duration_secs() - 0.3).abs() < 0.03);
        assert!(b.peak() > 0.05);
    }

    #[test]
    fn opus_file_opens_through_the_public_api() {
        let o = open_file(&media("chapters.opus")).unwrap();
        assert_eq!(o.buf.frames(), 48_000);
        assert_eq!(o.notice(), None);
        assert_eq!(o.info.markers.len(), 2);
    }

    #[test]
    fn notice_wording() {
        let mut o = Opened {
            buf: AudioBuffer::new(8000, 1, 1),
            damaged_packets: 12,
            notes: vec![],
            info: Default::default(),
        };
        assert_eq!(o.notice().unwrap(), "12 damaged packets were skipped");
        o.damaged_packets = 1;
        o.notes
            .push("The file has 2 chained sections; they were joined".into());
        assert_eq!(
            o.notice().unwrap(),
            "1 damaged packet was skipped. The file has 2 chained sections; they were joined"
        );
    }

    #[test]
    fn missing_file_says_so() {
        let e = decode_file(Path::new("definitely-not-here.wav")).unwrap_err();
        assert!(e.contains("Could not open"));
    }
}
