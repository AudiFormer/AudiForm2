// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Reading any common audio file into an [`AudioBuffer`], and writing WAV.

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

/// File extensions the Files panel offers to open.
pub const EXTENSIONS: &[&str] = &[
    "wav", "wave", "bwf", "rf64", "aif", "aiff", "aifc", "flac", "mp3", "mp2", "ogg", "oga",
    "opus", "mogg", "m4a", "mp4", "m4b", "mov", "aac", "caf", "mka", "mkv", "webm", "dts",
];

/// Decodes a whole file into memory as 32-bit float, keeping every
/// channel (5.1, 7.1 and other multichannel files stay multichannel).
///
/// Handles, beyond the formats the general decoders read:
/// * DTS streams hidden in WAV files (DTS music CDs) and raw `.dts`;
/// * `.mogg` multitrack Ogg files (unencrypted ones).
pub fn decode_file(path: &Path) -> Result<AudioBuffer, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(ext.as_str(), "wav" | "wave" | "dts" | "bwf") {
        if let Some(r) = af27_dts::decode_file(path) {
            let (rate, channels, data) =
                r.map_err(|e| format!("Could not decode the DTS audio: {e}"))?;
            return Ok(AudioBuffer::from_interleaved(rate, channels, &data));
        }
    }
    if ext == "mogg" {
        return decode_mogg(path);
    }
    let file = File::open(path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    decode_source(Box::new(file), Some(&ext), path)
}

/// A `.mogg` is an Ogg Vorbis stream (usually many channels, one pair per
/// instrument) after a small header: a little-endian version word and
/// the byte offset of the Ogg data. Version 10 is unencrypted; higher
/// versions are encrypted and are refused.
fn decode_mogg(path: &Path) -> Result<AudioBuffer, String> {
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

fn decode_source(
    src: Box<dyn symphonia::core::io::MediaSource>,
    ext: Option<&str>,
    path: &Path,
) -> Result<AudioBuffer, String> {
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
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("The file has no audio track.")?
        .clone();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("Unsupported audio codec: {e}"))?;
    let mut rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(0);
    let mut interleaved: Vec<f32> = Vec::new();
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(format!("Could not read the file: {e}")),
        };
        if packet.track_id() != track.id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                rate = spec.rate;
                channels = spec.channels.count();
                let cap = decoded.capacity() as u64;
                let buf = match &mut sample_buf {
                    Some(b) if b.capacity() >= decoded.capacity() * channels => b,
                    _ => sample_buf.insert(SampleBuffer::new(cap, spec)),
                };
                buf.copy_interleaved_ref(decoded);
                interleaved.extend_from_slice(buf.samples());
            }
            // A damaged packet is skipped rather than failing the whole file.
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(format!("Could not decode the file: {e}")),
        }
    }
    if rate == 0 || channels == 0 {
        return Err("The file holds no audio.".into());
    }
    Ok(AudioBuffer::from_interleaved(rate, channels, &interleaved))
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

    #[test]
    fn missing_file_says_so() {
        let e = decode_file(Path::new("definitely-not-here.wav")).unwrap_err();
        assert!(e.contains("Could not open"));
    }
}
