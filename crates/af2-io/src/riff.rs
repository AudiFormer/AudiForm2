// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! A small RIFF/RF64 chunk reader for the metadata in WAV files: cue
//! points (`cue `), their labels, notes and ranges (`LIST`/`adtl` with
//! `labl`, `note` and `ltxt`), and the Broadcast Wave `bext` chunk.
//!
//! Only the chunks named above are read; the audio data is skipped over
//! with a seek, so this is cheap even for very large files.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};

/// One cue point with what the `adtl` list says about it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cue {
    pub id: u32,
    /// Position in sample frames.
    pub offset: u64,
    pub label: Option<String>,
    pub note: Option<String>,
    /// Length in sample frames from an `ltxt` entry (0 for a point).
    pub length: u64,
    /// Text of the `ltxt` entry, if any.
    pub text: Option<String>,
}

/// The Broadcast Wave extension chunk.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bext {
    pub description: String,
    pub originator: String,
    pub originator_reference: String,
    pub origination_date: String,
    pub origination_time: String,
    /// Samples since midnight of the first sample.
    pub time_reference: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WavMeta {
    pub rate: u32,
    pub cues: Vec<Cue>,
    pub bext: Option<Bext>,
    /// Format tag (1 PCM, 3 float; the sub-format for extensible files).
    pub format: u16,
    pub channels: u16,
    pub bits: u16,
    /// Byte range of the audio data.
    pub data: Option<(u64, u64)>,
}

fn u16le(b: &[u8], at: usize) -> u16 {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .unwrap_or(0)
}

fn u32le(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .unwrap_or(0)
}

/// Text from a fixed or zero-terminated field, trailing NULs and spaces
/// dropped. Not valid UTF-8 is read as Latin-1, which most tools write.
fn text(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    let b = &b[..end];
    let s = match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    };
    s.trim_end().to_string()
}

/// Reads the metadata of a RIFF WAVE or RF64 file; `None` if it is not one.
pub fn read<R: Read + Seek>(r: &mut R) -> Option<WavMeta> {
    let mut head = [0u8; 12];
    r.read_exact(&mut head).ok()?;
    let rf64 = match &head[..4] {
        b"RIFF" => false,
        b"RF64" | b"BW64" => true,
        _ => return None,
    };
    if &head[8..12] != b"WAVE" {
        return None;
    }
    let file_len = r.seek(SeekFrom::End(0)).ok()?;
    r.seek(SeekFrom::Start(12)).ok()?;
    let mut meta = WavMeta::default();
    let mut points: BTreeMap<u32, Cue> = BTreeMap::new();
    let mut order: Vec<u32> = Vec::new();
    let mut data_size64: Option<u64> = None;
    let mut pos = 12u64;
    while pos + 8 <= file_len {
        let mut ch = [0u8; 8];
        if r.read_exact(&mut ch).is_err() {
            break;
        }
        let id = [ch[0], ch[1], ch[2], ch[3]];
        let mut size = u32le(&ch, 4) as u64;
        if rf64 && size == 0xFFFF_FFFF && &id == b"data" {
            size = data_size64.unwrap_or(file_len.saturating_sub(pos + 8));
        }
        let body = pos + 8;
        if &id == b"data" {
            meta.data = Some((body, size.min(file_len.saturating_sub(body))));
        }
        // Metadata chunks are small; refuse silly sizes from damaged files.
        let small = size <= 16 * 1024 * 1024 && body + size <= file_len;
        let wanted = matches!(&id, b"fmt " | b"cue " | b"LIST" | b"bext" | b"ds64");
        if wanted && small {
            let mut b = vec![0u8; size as usize];
            if r.read_exact(&mut b).is_err() {
                break;
            }
            match &id {
                b"ds64" => {
                    let lo = u32le(&b, 8) as u64;
                    let hi = u32le(&b, 12) as u64;
                    data_size64 = Some(lo | (hi << 32));
                }
                b"fmt " => {
                    let tag = u16le(&b, 0);
                    meta.channels = u16le(&b, 2);
                    meta.rate = u32le(&b, 4);
                    meta.bits = u16le(&b, 14);
                    // WAVE_FORMAT_EXTENSIBLE: the real tag opens the GUID.
                    meta.format = if tag == 0xFFFE && b.len() >= 26 {
                        u16le(&b, 24)
                    } else {
                        tag
                    };
                }
                b"cue " => {
                    let n = u32le(&b, 0) as usize;
                    for i in 0..n {
                        let at = 4 + i * 24;
                        if at + 24 > b.len() {
                            break;
                        }
                        let cid = u32le(&b, at);
                        // Field 5 is the sample offset into the data chunk.
                        let offset = u32le(&b, at + 20) as u64;
                        if !points.contains_key(&cid) {
                            order.push(cid);
                        }
                        points.entry(cid).or_default().id = cid;
                        points.get_mut(&cid).unwrap().offset = offset;
                    }
                }
                b"LIST" if b.len() >= 4 && &b[..4] == b"adtl" => {
                    let mut at = 4;
                    while at + 8 <= b.len() {
                        let sid = &b[at..at + 4];
                        let sz = u32le(&b, at + 4) as usize;
                        let s = at + 8;
                        let e = (s + sz).min(b.len());
                        let sub = &b[s..e];
                        let cid = u32le(sub, 0);
                        let cue = points.entry(cid).or_insert_with(|| Cue {
                            id: cid,
                            ..Default::default()
                        });
                        match sid {
                            b"labl" if sub.len() >= 4 => cue.label = Some(text(&sub[4..])),
                            b"note" if sub.len() >= 4 => cue.note = Some(text(&sub[4..])),
                            b"ltxt" if sub.len() >= 20 => {
                                cue.length = u32le(sub, 4) as u64;
                                let t = text(&sub[20..]);
                                if !t.is_empty() {
                                    cue.text = Some(t);
                                }
                            }
                            _ => {}
                        }
                        at = s + sz + (sz & 1);
                    }
                }
                b"bext" if b.len() >= 346 => {
                    meta.bext = Some(Bext {
                        description: text(&b[0..256]),
                        originator: text(&b[256..288]),
                        originator_reference: text(&b[288..320]),
                        origination_date: text(&b[320..330]),
                        origination_time: text(&b[330..338]),
                        time_reference: u32le(&b, 338) as u64 | ((u32le(&b, 342) as u64) << 32),
                    });
                }
                _ => {}
            }
        }
        pos = body + size + (size & 1);
        if r.seek(SeekFrom::Start(pos)).is_err() {
            break;
        }
    }
    // Cues listed only in `adtl` without a `cue ` entry have no position.
    meta.cues = order
        .into_iter()
        .filter_map(|id| points.remove(&id))
        .collect();
    meta.cues.sort_by_key(|c| c.offset);
    Some(meta)
}

/// Decodes the PCM or float audio of a WAV file with our own reader;
/// used for RF64/BW64 files, which the general decoder does not open.
pub(crate) fn decode_pcm<R: Read + Seek>(r: &mut R) -> Result<af2_core::AudioBuffer, String> {
    r.rewind().map_err(|e| e.to_string())?;
    let m = read(r).ok_or("The file is not a WAV file.")?;
    let (start, len) = m.data.ok_or("The WAV file has no audio data.")?;
    let ch = m.channels.max(1) as usize;
    let bytes = (m.bits as usize).div_ceil(8);
    let sample: fn(&[u8]) -> f32 = match (m.format, m.bits) {
        (1, 8) => |b| (b[0] as f32 - 128.0) / 128.0,
        (1, 16) => |b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32_768.0,
        (1, 24) => |b| (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
        (1, 32) => |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
        (3, 32) => |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        (3, 64) => |b| f64::from_le_bytes(b[..8].try_into().unwrap()) as f32,
        (f, b) => {
            return Err(format!(
                "This WAV sample format (tag {f}, {b}-bit) is not supported."
            ))
        }
    };
    if m.rate == 0 {
        return Err("The WAV file's header is damaged.".into());
    }
    let frame = bytes * ch;
    let frames = (len / frame as u64) as usize;
    let mut out = af2_core::AudioBuffer::new(m.rate, ch, frames);
    r.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
    let mut chunk = vec![0u8; frame * 65_536];
    let mut done = 0;
    while done < frames {
        let n = (frames - done).min(65_536);
        r.read_exact(&mut chunk[..n * frame])
            .map_err(|e| format!("Could not read the audio: {e}"))?;
        for i in 0..n {
            for (c, chan) in out.channels.iter_mut().enumerate() {
                let at = i * frame + c * bytes;
                chan[done + i] = sample(&chunk[at..at + bytes]);
            }
        }
        done += n;
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn cue_point(id: u32, offset: u32) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&id.to_le_bytes());
        v.extend_from_slice(&offset.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&offset.to_le_bytes());
        v
    }

    fn zstr(id: u32, s: &str) -> Vec<u8> {
        let mut v = id.to_le_bytes().to_vec();
        v.extend_from_slice(s.as_bytes());
        v.push(0);
        v
    }

    /// A 1-second 8 kHz mono 16-bit WAV with two cues (one a range with a
    /// note), and a `bext` chunk.
    pub(crate) fn test_wav() -> Vec<u8> {
        let rate = 8000u32;
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&(rate * 2).to_le_bytes());
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());

        let mut bext = vec![0u8; 602];
        bext[..11].copy_from_slice(b"Field notes");
        bext[256..262].copy_from_slice(b"Zoomer");
        bext[320..330].copy_from_slice(b"2026-11-19");
        bext[330..338].copy_from_slice(b"12:34:56");
        let tr: u64 = 8000 * 3600;
        bext[338..346].copy_from_slice(&tr.to_le_bytes());

        let mut cue = 2u32.to_le_bytes().to_vec();
        cue.extend(cue_point(1, 2000));
        cue.extend(cue_point(2, 4000));

        let mut ltxt = 2u32.to_le_bytes().to_vec();
        ltxt.extend_from_slice(&1600u32.to_le_bytes());
        ltxt.extend_from_slice(b"rgn ");
        ltxt.extend_from_slice(&[0u8; 8]);
        let mut adtl = b"adtl".to_vec();
        adtl.extend(chunk(b"labl", &zstr(1, "Verse")));
        adtl.extend(chunk(b"labl", &zstr(2, "Chorus")));
        adtl.extend(chunk(b"note", &zstr(2, "Loud bit")));
        adtl.extend(chunk(b"ltxt", &ltxt));

        let data = vec![0u8; (rate * 2) as usize];
        let mut body = b"WAVE".to_vec();
        body.extend(chunk(b"fmt ", &fmt));
        body.extend(chunk(b"bext", &bext));
        body.extend(chunk(b"data", &data));
        body.extend(chunk(b"cue ", &cue));
        body.extend(chunk(b"LIST", &adtl));
        chunk(b"RIFF", &body)
    }

    #[test]
    fn reads_cues_labels_ranges_and_bext() {
        let m = read(&mut std::io::Cursor::new(test_wav())).unwrap();
        assert_eq!(m.rate, 8000);
        assert_eq!(m.cues.len(), 2);
        assert_eq!(m.cues[0].offset, 2000);
        assert_eq!(m.cues[0].label.as_deref(), Some("Verse"));
        assert_eq!(m.cues[0].length, 0);
        assert_eq!(m.cues[1].label.as_deref(), Some("Chorus"));
        assert_eq!(m.cues[1].note.as_deref(), Some("Loud bit"));
        assert_eq!(m.cues[1].length, 1600);
        let b = m.bext.unwrap();
        assert_eq!(b.description, "Field notes");
        assert_eq!(b.originator, "Zoomer");
        assert_eq!(b.origination_date, "2026-11-19");
        assert_eq!(b.time_reference, 8000 * 3600);
    }

    #[test]
    fn rf64_pcm_decodes() {
        // An RF64 file: ds64 holds the real data size, the 32-bit field is -1.
        let mut ds64 = Vec::new();
        let data: Vec<u8> = (0..400i16).flat_map(|i| (i * 50).to_le_bytes()).collect();
        ds64.extend_from_slice(&0u64.to_le_bytes());
        ds64.extend_from_slice(&(data.len() as u64).to_le_bytes());
        ds64.extend_from_slice(&200u64.to_le_bytes());
        ds64.extend_from_slice(&0u32.to_le_bytes());
        let mut fmt = Vec::new();
        for v in [1u16, 2] {
            fmt.extend_from_slice(&v.to_le_bytes());
        }
        fmt.extend_from_slice(&48_000u32.to_le_bytes());
        fmt.extend_from_slice(&192_000u32.to_le_bytes());
        fmt.extend_from_slice(&4u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        let mut body = b"WAVE".to_vec();
        body.extend(chunk(b"ds64", &ds64));
        body.extend(chunk(b"fmt ", &fmt));
        body.extend_from_slice(b"data");
        body.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        body.extend_from_slice(&data);
        let mut file = b"RF64".to_vec();
        file.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        file.extend_from_slice(&body);
        let b = decode_pcm(&mut std::io::Cursor::new(file)).unwrap();
        assert_eq!((b.rate, b.num_channels(), b.frames()), (48_000, 2, 200));
        assert!((b.channels[1][3] - 350.0 / 32_768.0).abs() < 1e-6);
    }

    #[test]
    fn not_riff_is_none_and_truncated_is_safe() {
        assert!(read(&mut std::io::Cursor::new(b"fLaC....".to_vec())).is_none());
        let mut w = test_wav();
        w.truncate(200);
        let m = read(&mut std::io::Cursor::new(w)).unwrap();
        assert_eq!(m.rate, 8000);
        assert!(m.cues.is_empty());
    }
}
