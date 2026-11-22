// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Cache files for rendered audio that plays from disk (pre-rendered
//! tracks): 24-bit FLAC, in chunks that each decode on their own, behind
//! an index, so a reader can start at any chunk without a seek table.
//!
//! The audio is stored [`HEADROOM_DB`] down, so a render that goes over
//! full scale (it is taken before the fader, in float) keeps its peaks;
//! reading turns it back up. 24 bits less 24 dB still leaves about 120 dB
//! of range, far below hearing.
//!
//! Layout (little-endian): `AF2CHNK1`, rate u32, channels u32, frames u64,
//! chunk frames u32, chunk count u32, then count + 1 byte offsets u64 (the
//! last is the end), then each chunk as a whole FLAC stream.

use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use af2_core::AudioBuffer;

const MAGIC: &[u8; 8] = b"AF2CHNK1";
/// How far down the audio is stored.
pub const HEADROOM_DB: f32 = 24.0;
/// FLAC compression level (lossless at every level; 5 is quick to make).
const LEVEL: u8 = 5;

fn store_gain() -> f32 {
    10f32.powf(-HEADROOM_DB / 20.0)
}

/// Writes `buf` in chunks of `chunk` frames (through a temporary file, so
/// a half-written file never has the final name).
pub fn write(path: &Path, buf: &AudioBuffer, chunk: usize) -> Result<(), String> {
    let ch = buf.num_channels();
    if ch == 0 || ch > crate::export::flac::MAX_CHANNELS {
        return Err(format!("{ch} channels cannot be stored"));
    }
    let chunk = chunk.max(4096);
    let frames = buf.frames();
    let count = frames.div_ceil(chunk);
    let g = store_gain() * 8_388_608.0;
    let mut body: Vec<u8> = Vec::new();
    let mut offsets = Vec::with_capacity(count + 1);
    let head_len = 8 + 4 + 4 + 8 + 4 + 4 + 8 * (count + 1);
    let mut inter: Vec<i32> = Vec::with_capacity(chunk * ch);
    for k in 0..count {
        offsets.push((head_len + body.len()) as u64);
        let a = k * chunk;
        let b = (a + chunk).min(frames);
        inter.clear();
        for f in a..b {
            for c in &buf.channels {
                let s = c.get(f).copied().unwrap_or(0.0);
                inter.push(((s * g).round() as i32).clamp(-8_388_608, 8_388_607));
            }
        }
        let mut w = crate::export::flac::FlacWriter::new(
            Cursor::new(Vec::new()),
            buf.rate,
            ch,
            24,
            LEVEL,
            &[],
        )?;
        w.write(&inter)?;
        let (cur, _) = w.finish()?;
        body.extend_from_slice(&cur.into_inner());
    }
    offsets.push((head_len + body.len()) as u64);
    let mut out = Vec::with_capacity(head_len + body.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&buf.rate.to_le_bytes());
    out.extend_from_slice(&(ch as u32).to_le_bytes());
    out.extend_from_slice(&(frames as u64).to_le_bytes());
    out.extend_from_slice(&(chunk as u32).to_le_bytes());
    out.extend_from_slice(&(count as u32).to_le_bytes());
    for o in &offsets {
        out.extend_from_slice(&o.to_le_bytes());
    }
    out.extend_from_slice(&body);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("part");
    File::create(&tmp)
        .and_then(|mut f| f.write_all(&out).and_then(|_| f.sync_all()))
        .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// An open cache file: its shape and index.
#[derive(Debug, Clone)]
pub struct Chunked {
    pub path: PathBuf,
    pub rate: u32,
    pub channels: usize,
    pub frames: usize,
    pub chunk: usize,
    offsets: Vec<u64>,
}

impl Chunked {
    /// Reads the header and index.
    pub fn open(path: &Path) -> Result<Chunked, String> {
        let mut f = File::open(path).map_err(|e| e.to_string())?;
        let mut h = [0u8; 32];
        f.read_exact(&mut h).map_err(|e| e.to_string())?;
        if &h[..8] != MAGIC {
            return Err("not a chunked render".into());
        }
        let u32_at = |i: usize| u32::from_le_bytes(h[i..i + 4].try_into().unwrap());
        let rate = u32_at(8);
        let channels = u32_at(12) as usize;
        let frames = u64::from_le_bytes(h[16..24].try_into().unwrap()) as usize;
        let chunk = u32_at(24) as usize;
        let count = u32_at(28) as usize;
        if rate == 0 || channels == 0 || chunk == 0 || count != frames.div_ceil(chunk) {
            return Err("damaged chunked render".into());
        }
        let mut idx = vec![0u8; 8 * (count + 1)];
        f.read_exact(&mut idx).map_err(|e| e.to_string())?;
        let offsets: Vec<u64> = idx
            .chunks_exact(8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
            .collect();
        let len = f.metadata().map_err(|e| e.to_string())?.len();
        if offsets.windows(2).any(|w| w[1] < w[0]) || offsets.last() != Some(&len) {
            return Err("damaged chunked render".into());
        }
        Ok(Chunked {
            path: path.to_path_buf(),
            rate,
            channels,
            frames,
            chunk,
            offsets,
        })
    }

    pub fn count(&self) -> usize {
        self.offsets.len() - 1
    }

    /// Chunk `k`'s audio, one Vec per channel, at its full level. `file`
    /// is the cache file, opened by the caller and kept between reads.
    pub fn read(&self, file: &mut File, k: usize) -> Result<Vec<Vec<f32>>, String> {
        if k >= self.count() {
            return Err("past the end".into());
        }
        let (a, b) = (self.offsets[k], self.offsets[k + 1]);
        let mut bytes = vec![0u8; (b - a) as usize];
        file.seek(SeekFrom::Start(a))
            .and_then(|_| file.read_exact(&mut bytes))
            .map_err(|e| e.to_string())?;
        let src: Box<dyn symphonia::core::io::MediaSource> = Box::new(Cursor::new(bytes));
        let d = crate::decode_symphonia(src, Some("flac"), &self.path)?;
        let want = (self.frames - k * self.chunk).min(self.chunk);
        let up = 1.0 / store_gain();
        let mut out = d.buf.channels;
        out.resize(self.channels, Vec::new());
        for c in &mut out {
            c.resize(want, 0.0);
            c.iter_mut().for_each(|s| *s *= up);
        }
        Ok(out)
    }

    /// The whole file in memory.
    pub fn read_all(&self) -> Result<AudioBuffer, String> {
        let mut f = File::open(&self.path).map_err(|e| e.to_string())?;
        let mut channels: Vec<Vec<f32>> = (0..self.channels)
            .map(|_| Vec::with_capacity(self.frames))
            .collect();
        for k in 0..self.count() {
            for (dst, src) in channels.iter_mut().zip(self.read(&mut f, k)?) {
                dst.extend_from_slice(&src);
            }
        }
        Ok(AudioBuffer {
            rate: self.rate,
            channels,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_render_comes_back_within_24_bit_steps_peaks_over_full_scale_included() {
        let dir = std::env::temp_dir().join(format!("af2-chunked-{}", std::process::id()));
        let path = dir.join("r.afr");
        let frames = 10_000 + 4096 * 3;
        let l: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.01).sin() * 3.0).collect();
        let r: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.003).cos() * 0.1).collect();
        let buf = AudioBuffer {
            rate: 48_000,
            channels: vec![l.clone(), r.clone()],
        };
        write(&path, &buf, 4096).unwrap();
        let c = Chunked::open(&path).unwrap();
        assert_eq!((c.rate, c.channels, c.frames, c.chunk), (48_000, 2, frames, 4096));
        assert_eq!(c.count(), frames.div_ceil(4096));
        let mut f = File::open(&path).unwrap();
        let last = c.read(&mut f, c.count() - 1).unwrap();
        assert_eq!(last[0].len(), frames - 4096 * (c.count() - 1));
        let all = c.read_all().unwrap();
        // One stored step is 2^-23 at -24 dB: about 2e-6 at full level.
        let step = 1.0 / (8_388_608.0 * store_gain());
        for (a, b) in all.channels[0].iter().zip(&l) {
            assert!((a - b).abs() <= step, "{a} {b}");
        }
        for (a, b) in all.channels[1].iter().zip(&r) {
            assert!((a - b).abs() <= step);
        }
        let size = std::fs::metadata(&path).unwrap().len() as usize;
        assert!(size < frames * 2 * 4, "smaller than float: {size}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_file_is_refused() {
        let dir = std::env::temp_dir().join(format!("af2-chunked-bad-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("bad.afr");
        std::fs::write(&path, b"AF2CHNK1 not really").unwrap();
        assert!(Chunked::open(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
