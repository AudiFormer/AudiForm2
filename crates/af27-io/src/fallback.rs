// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! When the open decoders cannot read a file: the system's own decoders
//! (`af27-osdecode`, Windows), then an installed ffmpeg (any system), and
//! otherwise an error that names the codec.
//!
//! These are for formats with no open decoder AudiForm 2 can ship: Dolby
//! Digital (AC-3), Dolby Digital Plus (E-AC-3; Dolby Atmos music in M4A
//! and MP4 is this, and opens as its 5.1 or 7.1 bed), Dolby AC-4, HE-AAC
//! and multichannel AAC.
//!
//! ffmpeg is never shipped or downloaded: it is used only if the user has
//! it (on the PATH, or named by `AF2_FFMPEG`). It is run as a separate
//! program writing 32-bit float WAV to a pipe.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use af27_core::AudioBuffer;

/// What a file's audio is, from its sample entry, for messages.
pub fn codec_name(path: &Path) -> Option<&'static str> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let span = 4u64 << 20;
    let mut bytes = Vec::new();
    f.by_ref().take(span).read_to_end(&mut bytes).ok()?;
    // MP4 writers often put the index (with the sample entries) last.
    if len > span {
        f.seek(SeekFrom::Start(len.saturating_sub(span).max(span))).ok()?;
        f.read_to_end(&mut bytes).ok()?;
    }
    let has = |tag: &[u8]| bytes.windows(tag.len()).any(|w| w == tag);
    [
        (&b"ec-3"[..], "Dolby Digital Plus (E-AC-3)"),
        (b"ac-4", "Dolby AC-4"),
        (b"ac-3", "Dolby Digital (AC-3)"),
        (b"mha1", "MPEG-H 3D Audio"),
        (b"mhm1", "MPEG-H 3D Audio"),
        (b"mp4a", "AAC"),
    ]
    .into_iter()
    .find(|(t, _)| has(t))
    .map(|(_, n)| n)
}

/// The ffmpeg to use, if the user has one.
fn ffmpeg() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("AF2_FFMPEG").map(PathBuf::from) {
        return p.is_file().then_some(p);
    }
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(exe))
        .find(|p| p.is_file())
}

/// Decodes the first audio stream with the user's ffmpeg.
fn with_ffmpeg(exe: &Path, path: &Path) -> Result<AudioBuffer, String> {
    let mut cmd = Command::new(exe);
    cmd.args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args(["-vn", "-map", "0:a:0", "-c:a", "pcm_f32le", "-f", "wav", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up.
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("ffmpeg did not run: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("ffmpeg could not decode it: {}", err.trim()));
    }
    float_wav(&out.stdout).ok_or_else(|| "ffmpeg gave no audio".to_string())
}

/// Reads 32-bit float WAV as ffmpeg writes it to a pipe (the sizes in
/// the header may be unknown, so the data runs to the end).
fn float_wav(b: &[u8]) -> Option<AudioBuffer> {
    if b.len() < 12 || &b[..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let mut at = 12;
    let (mut ch, mut rate) = (0usize, 0u32);
    while at + 8 <= b.len() {
        let id = &b[at..at + 4];
        let size = u32::from_le_bytes(b[at + 4..at + 8].try_into().ok()?) as usize;
        let body = at + 8;
        if id == b"fmt " {
            ch = u16::from_le_bytes(b.get(body + 2..body + 4)?.try_into().ok()?) as usize;
            rate = u32::from_le_bytes(b.get(body + 4..body + 8)?.try_into().ok()?);
        } else if id == b"data" {
            if ch == 0 || rate == 0 {
                return None;
            }
            let data = &b[body..];
            let frames = data.len() / 4 / ch;
            let mut channels: Vec<Vec<f32>> =
                (0..ch).map(|_| Vec::with_capacity(frames)).collect();
            for f in data[..frames * ch * 4].chunks_exact(ch * 4) {
                for (c, s) in channels.iter_mut().zip(f.chunks_exact(4)) {
                    c.push(f32::from_le_bytes([s[0], s[1], s[2], s[3]]));
                }
            }
            return Some(AudioBuffer { rate, channels });
        }
        at = body + size + (size & 1);
    }
    None
}

/// Tries the system's decoders, then ffmpeg, for a file the open
/// decoders refused with `own`. The error names the codec and what was
/// tried.
pub fn decode(path: &Path, own: &str) -> Result<(AudioBuffer, String), String> {
    let name = codec_name(path);
    let mut tried = Vec::new();
    if af27_osdecode::available() {
        match af27_osdecode::decode(path) {
            Ok(d) if !d.channels.is_empty() && d.channels[0].len() > 0 => {
                return Ok((
                    AudioBuffer {
                        rate: d.rate,
                        channels: d.channels,
                    },
                    format!("{} decoded by Windows", name.unwrap_or("Audio")),
                ));
            }
            Ok(_) => tried.push("Windows gave no audio for it".to_string()),
            Err(e) => tried.push(e),
        }
    }
    if let Some(exe) = ffmpeg() {
        match with_ffmpeg(&exe, path) {
            Ok(b) if b.frames() > 0 => {
                return Ok((b, format!("{} decoded by ffmpeg", name.unwrap_or("Audio"))));
            }
            Ok(_) => tried.push("ffmpeg gave no audio".into()),
            Err(e) => tried.push(e),
        }
    }
    let what = name.map_or_else(|| own.to_string(), |n| format!("{n} audio could not be decoded"));
    let mut msg = what;
    for t in tried {
        msg.push_str(". ");
        msg.push_str(&t);
    }
    if name == Some("Dolby AC-4") && cfg!(windows) {
        msg.push_str(". Windows decodes AC-4 only with Dolby's decoder installed (it comes with Dolby Access)");
    }
    if ffmpeg().is_none() && !cfg!(windows) {
        msg.push_str(". Installing ffmpeg lets AudiForm 2 open it");
    }
    msg.push('.');
    Err(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_piped_float_wav_reads_with_unknown_sizes() {
        let mut b = b"RIFF\xff\xff\xff\xffWAVE".to_vec();
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&3u16.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&44_100u32.to_le_bytes());
        b.extend_from_slice(&[0; 8]);
        b.extend_from_slice(b"LIST");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(b"abc\0");
        b.extend_from_slice(b"data\xff\xff\xff\xff");
        for v in [0.5f32, -0.5, 0.25, -0.25] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let a = float_wav(&b).unwrap();
        assert_eq!(a.rate, 44_100);
        assert_eq!(a.channels, vec![vec![0.5, 0.25], vec![-0.5, -0.25]]);
    }
}
