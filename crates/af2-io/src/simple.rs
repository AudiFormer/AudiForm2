// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Two plain PCM formats the general decoder does not open:
//! - Sun/NeXT `.au` (`.snd`): a big-endian header, then linear PCM
//!   (8/16/24/32-bit), float, or G.711 mu-law and A-law.
//! - Sony Wave64 `.w64`: WAV with 64-bit sizes and GUID chunk ids, as
//!   Sound Forge and Vegas write long recordings.

use std::io::Read;
use std::path::Path;

use af2_core::AudioBuffer;

#[derive(Clone, Copy)]
enum Kind {
    Int(u8),
    Float(u8),
    MuLaw,
    ALaw,
    /// 8-bit unsigned (WAV's 8-bit).
    U8,
}

fn read_all(path: &Path) -> Result<Vec<u8>, String> {
    let mut v = Vec::new();
    std::fs::File::open(path)
        .and_then(|mut f| f.read_to_end(&mut v))
        .map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    Ok(v)
}

fn mulaw(b: u8) -> f32 {
    let u = !b;
    let sign = u & 0x80;
    let exp = (u >> 4) & 7;
    let man = u & 0x0f;
    let mag = ((((man as i32) << 3) + 0x84) << exp) - 0x84;
    (if sign != 0 { -mag } else { mag }) as f32 / 32_768.0
}

fn alaw(b: u8) -> f32 {
    let a = b ^ 0x55;
    let sign = a & 0x80;
    let exp = (a >> 4) & 7;
    let man = (a & 0x0f) as i32;
    let mag = if exp == 0 { (man << 4) + 8 } else { ((man << 4) + 0x108) << (exp - 1) };
    (if sign != 0 { mag } else { -mag }) as f32 / 32_768.0
}

fn pcm(data: &[u8], kind: Kind, big: bool, rate: u32, ch: usize) -> Result<AudioBuffer, String> {
    if rate == 0 || ch == 0 {
        return Err("The file's header is damaged.".into());
    }
    let bytes = match kind {
        Kind::Int(b) | Kind::Float(b) => b as usize / 8,
        _ => 1,
    };
    let frame = bytes * ch;
    let frames = data.len() / frame;
    let mut out = AudioBuffer::new(rate, ch, frames);
    let word = |s: &[u8]| -> [u8; 8] {
        let mut w = [0u8; 8];
        if big {
            for (i, x) in s.iter().rev().enumerate() {
                w[i] = *x;
            }
        } else {
            w[..s.len()].copy_from_slice(s);
        }
        w
    };
    for i in 0..frames {
        for (c, chan) in out.channels.iter_mut().enumerate() {
            let s = &data[i * frame + c * bytes..i * frame + (c + 1) * bytes];
            chan[i] = match kind {
                Kind::MuLaw => mulaw(s[0]),
                Kind::ALaw => alaw(s[0]),
                Kind::U8 => (s[0] as f32 - 128.0) / 128.0,
                Kind::Int(8) => s[0] as i8 as f32 / 128.0,
                Kind::Int(16) => i16::from_le_bytes([word(s)[0], word(s)[1]]) as f32 / 32_768.0,
                Kind::Int(24) => {
                    let w = word(s);
                    (i32::from_le_bytes([0, w[0], w[1], w[2]]) >> 8) as f32 / 8_388_608.0
                }
                Kind::Int(_) => {
                    let w = word(s);
                    i32::from_le_bytes([w[0], w[1], w[2], w[3]]) as f32 / 2_147_483_648.0
                }
                Kind::Float(32) => {
                    let w = word(s);
                    f32::from_le_bytes([w[0], w[1], w[2], w[3]])
                }
                Kind::Float(_) => f64::from_le_bytes(word(s)) as f32,
            };
        }
    }
    Ok(out)
}

/// A Sun/NeXT `.au` file.
pub fn decode_au(path: &Path) -> Result<AudioBuffer, String> {
    let b = read_all(path)?;
    if b.len() < 24 || &b[..4] != b".snd" {
        return Err("This is not a Sun/NeXT .au file.".into());
    }
    let be = |at: usize| u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let offset = (be(4) as usize).clamp(24, b.len());
    let size = be(8);
    let kind = match be(12) {
        1 => Kind::MuLaw,
        2 => Kind::Int(8),
        3 => Kind::Int(16),
        4 => Kind::Int(24),
        5 => Kind::Int(32),
        6 => Kind::Float(32),
        7 => Kind::Float(64),
        27 => Kind::ALaw,
        e => return Err(format!("This .au encoding ({e}) is not supported.")),
    };
    // 0xffffffff: the size wasn't known when it was written.
    let end = if size == u32::MAX { b.len() } else { (offset + size as usize).min(b.len()) };
    pcm(&b[offset..end], kind, true, be(16), be(20) as usize)
}

/// A Sony Wave64 `.w64` file.
pub fn decode_w64(path: &Path) -> Result<AudioBuffer, String> {
    const RIFF: [u8; 16] = *b"riff\x2e\x91\xcf\x11\xa5\xd6\x28\xdb\x04\xc1\x00\x00";
    let b = read_all(path)?;
    if b.len() < 40 || b[..16] != RIFF || &b[24..28] != b"wave" {
        return Err("This is not a Wave64 file.".into());
    }
    let le16 = |at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
    let le32 = |at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let mut at = 40;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    while at + 24 <= b.len() {
        let id = &b[at..at + 4];
        let size = u64::from_le_bytes(b[at + 16..at + 24].try_into().unwrap()) as usize;
        if size < 24 {
            break;
        }
        let body = at + 24;
        let end = (at + size).min(b.len());
        if id == b"fmt " && end >= body + 16 {
            let mut tag = le16(body);
            let bits = le16(body + 14);
            // WAVE_FORMAT_EXTENSIBLE: the real tag starts the sub-format.
            if tag == 0xfffe && end >= body + 26 {
                tag = le16(body + 24);
            }
            fmt = Some((tag, le16(body + 2), le32(body + 4), bits));
        } else if id == b"data" {
            let (tag, ch, rate, bits) = fmt.ok_or("The Wave64 file has no format before its audio.")?;
            let kind = match (tag, bits) {
                (1, 8) => Kind::U8,
                (1, 16 | 24 | 32) => Kind::Int(bits as u8),
                (3, 32 | 64) => Kind::Float(bits as u8),
                (6, 8) => Kind::ALaw,
                (7, 8) => Kind::MuLaw,
                _ => return Err(format!("This Wave64 sample format (tag {tag}, {bits}-bit) is not supported.")),
            };
            return pcm(&b[body..end], kind, false, rate, ch as usize);
        }
        // Chunks are padded to 8 bytes.
        at += size.div_ceil(8) * 8;
    }
    Err("The Wave64 file has no audio data.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn au_and_wave64_pcm_read_back() {
        let dir = std::env::temp_dir().join(format!("af2-simple-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let samples: [i16; 4] = [0, 16_384, -16_384, 32_767];
        // .au: 16-bit big-endian, mono, 8 kHz.
        let mut au = b".snd".to_vec();
        for v in [24u32, 8, 3, 8000, 1] {
            au.extend_from_slice(&v.to_be_bytes());
        }
        for s in samples {
            au.extend_from_slice(&s.to_be_bytes());
        }
        let p = dir.join("t.au");
        std::fs::write(&p, &au).unwrap();
        let b = decode_au(&p).unwrap();
        assert_eq!((b.rate, b.num_channels(), b.frames()), (8000, 1, 4));
        assert_eq!(b.channels[0][1], 0.5);
        assert_eq!(b.channels[0][2], -0.5);
        // .w64: the same as 16-bit little-endian PCM.
        let guid = |four: &[u8; 4], rest: &[u8; 12]| [four.as_slice(), rest.as_slice()].concat();
        let chunk_tail: [u8; 12] = [0xf3, 0xac, 0xd3, 0x11, 0x8c, 0xd1, 0x00, 0xc0, 0x4f, 0x8e, 0xdb, 0x8a];
        let mut fmt = Vec::new();
        for v in [1u16, 1] {
            fmt.extend_from_slice(&v.to_le_bytes());
        }
        fmt.extend_from_slice(&8000u32.to_le_bytes());
        fmt.extend_from_slice(&16000u32.to_le_bytes());
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut body = [b"wave".as_slice(), chunk_tail.as_slice()].concat();
        for (id, c) in [(b"fmt ", &fmt), (b"data", &data)] {
            body.extend(guid(id, &chunk_tail));
            body.extend_from_slice(&(24 + c.len() as u64).to_le_bytes());
            body.extend_from_slice(c);
            while body.len() % 8 != 0 {
                body.push(0);
            }
        }
        let mut w = [b"riff".as_slice(), [0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00, 0x00].as_slice()].concat();
        w.extend_from_slice(&(24 + body.len() as u64).to_le_bytes());
        w.extend(body);
        let p = dir.join("t.w64");
        std::fs::write(&p, &w).unwrap();
        let b = decode_w64(&p).unwrap();
        assert_eq!((b.rate, b.num_channels(), b.frames()), (8000, 1, 4));
        assert_eq!(b.channels[0][1], 0.5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn g711_tables_match_known_values() {
        assert_eq!(mulaw(0xff), 0.0);
        assert!((mulaw(0x80) - 32_124.0 / 32_768.0).abs() < 1e-6);
        assert!((mulaw(0x00) + 32_124.0 / 32_768.0).abs() < 1e-6);
        assert!((alaw(0xd5) - 8.0 / 32_768.0).abs() < 1e-6);
        assert!((alaw(0xaa) - 32_256.0 / 32_768.0).abs() < 1e-6);
    }
}
