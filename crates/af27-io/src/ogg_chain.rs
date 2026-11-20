// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Chained Ogg files (one complete Ogg stream after another, as made by
//! concatenating files or by internet radio rips).
//!
//! Symphonia 0.5's Ogg reader skips straight to the last link of a chain,
//! so each link is found here by scanning page headers and handed to the
//! decoder on its own through a [`Window`] onto the file.

use std::io::{Read, Seek, SeekFrom};

/// Byte ranges of the links of a chained Ogg file: a new link starts at
/// a beginning-of-stream page that follows a non-BOS page. Returns one
/// range for an ordinary file, and nothing if the data is not Ogg.
pub(crate) fn links<R: Read + Seek>(r: &mut R) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let Ok(end) = r.seek(SeekFrom::End(0)) else {
        return out;
    };
    let mut pos = 0u64;
    let mut start = 0u64;
    let mut prev_bos = true;
    while pos + 27 <= end {
        if r.seek(SeekFrom::Start(pos)).is_err() {
            break;
        }
        let mut h = [0u8; 27];
        if r.read_exact(&mut h).is_err() || &h[..4] != b"OggS" {
            // Damage: leave the rest to the decoder as part of this link.
            break;
        }
        let mut segs = vec![0u8; h[26] as usize];
        if r.read_exact(&mut segs).is_err() {
            break;
        }
        let bos = h[5] & 0x02 != 0;
        if bos && !prev_bos && pos > start {
            out.push((start, pos));
            start = pos;
        }
        prev_bos = bos;
        pos += 27 + segs.len() as u64 + segs.iter().map(|&s| s as u64).sum::<u64>();
    }
    if pos > 0 || end > 0 {
        out.push((start, end));
    }
    let _ = r.seek(SeekFrom::Start(0));
    out
}

/// A byte range of a file seen as a whole file.
pub(crate) struct Window<R> {
    inner: R,
    start: u64,
    len: u64,
    pos: u64,
}

impl<R: Read + Seek> Window<R> {
    pub(crate) fn new(inner: R, (start, end): (u64, u64)) -> Window<R> {
        Window {
            inner,
            start,
            len: end.saturating_sub(start),
            pos: 0,
        }
    }
}

impl<R: Read + Seek> Read for Window<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.len.saturating_sub(self.pos);
        if left == 0 {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(left) as usize;
        self.inner.seek(SeekFrom::Start(self.start + self.pos))?;
        let n = self.inner.read(&mut buf[..want])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for Window<R> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(d) => self.len as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if p < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

impl<R: Read + Seek + Send + Sync> symphonia::core::io::MediaSource for Window<R> {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(serial: u32, flags: u8, body: &[u8]) -> Vec<u8> {
        let mut p = b"OggS".to_vec();
        p.push(0);
        p.push(flags);
        p.extend_from_slice(&0u64.to_le_bytes());
        p.extend_from_slice(&serial.to_le_bytes());
        p.extend_from_slice(&[0; 8]);
        p.push(1);
        p.push(body.len() as u8);
        p.extend_from_slice(body);
        p
    }

    #[test]
    fn finds_links_and_windows_read_them() {
        let mut f = Vec::new();
        f.extend(page(1, 2, b"head1"));
        f.extend(page(1, 4, b"end1"));
        let second = f.len() as u64;
        f.extend(page(2, 2, b"head2"));
        f.extend(page(3, 2, b"other"));
        f.extend(page(2, 0, b"data2"));
        let mut c = std::io::Cursor::new(f.clone());
        let l = links(&mut c);
        assert_eq!(l, vec![(0, second), (second, f.len() as u64)]);
        let mut w = Window::new(std::io::Cursor::new(f.clone()), l[1]);
        let mut v = Vec::new();
        w.read_to_end(&mut v).unwrap();
        assert_eq!(v, &f[second as usize..]);
        assert_eq!(w.seek(SeekFrom::End(-5)).unwrap(), v.len() as u64 - 5);
        assert!(links(&mut std::io::Cursor::new(b"RIFF".to_vec())).len() <= 1);
    }
}
