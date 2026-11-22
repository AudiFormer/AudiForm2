// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! A streaming FLAC writer on the `flacenc` encoder (Apache-2.0): each
//! 4096-frame block is encoded and written as it arrives, and the
//! STREAMINFO (length, frame sizes, MD5) is rewritten at the end. Tags go
//! in a Vorbis comment block.

use std::io::{Seek, SeekFrom, Write};

use flacenc::bitsink::MemSink;
use flacenc::component::{BitRepr, MetadataBlockData, Stream, StreamInfo};
use flacenc::config;
use flacenc::error::{Verified, Verify};
use flacenc::source::{Context, Fill, FrameBuf};

use super::Tag;

pub const BLOCK_SIZE: usize = 4096;
/// The most channels a FLAC stream holds.
pub const MAX_CHANNELS: usize = 8;

/// The encoder settings for compression level 0 (fastest) to 8 (smallest).
/// Every level is lossless. 0 to 2 use only the fixed predictors (2 adds
/// mid-side stereo); 3 and up add LPC, its order rising to 12.
pub fn level_config(level: u8) -> config::Encoder {
    let mut c = config::Encoder::default();
    c.block_size = BLOCK_SIZE;
    c.multithread = false;
    let level = level.min(8);
    let sc = &mut c.subframe_coding;
    if level <= 2 {
        sc.use_lpc = false;
        sc.fixed.max_order = if level == 0 { 2 } else { 4 };
        c.stereo_coding.use_midside = level == 2;
        c.stereo_coding.use_leftside = level >= 1;
        c.stereo_coding.use_rightside = level >= 1;
    } else {
        sc.use_lpc = true;
        let (order, precision) = match level {
            3 => (6, 12),
            4 => (8, 12),
            5 => (8, 15),
            6 => (10, 15),
            _ => (12, 15),
        };
        sc.qlpc.lpc_order = order;
        sc.qlpc.quant_precision = precision;
        if level == 8 {
            sc.qlpc.window = config::Window::Tukey { alpha: 0.25 };
        }
    }
    c
}

/// A VORBIS_COMMENT block's body.
pub fn vorbis_comment(tags: &[(Tag, String)], vendor: &str) -> Vec<u8> {
    vorbis_comment_with(tags, &[], vendor)
}

/// The same with more `KEY=value` fields after the tags (chapters).
pub fn vorbis_comment_with(tags: &[(Tag, String)], extra: &[String], vendor: &str) -> Vec<u8> {
    let mut v = (vendor.len() as u32).to_le_bytes().to_vec();
    v.extend_from_slice(vendor.as_bytes());
    let mut fields: Vec<String> = tags
        .iter()
        .filter(|(_, s)| !s.is_empty())
        .map(|(t, s)| format!("{}={s}", t.vorbis_name()))
        .collect();
    fields.extend(extra.iter().cloned());
    v.extend_from_slice(&(fields.len() as u32).to_le_bytes());
    for f in fields {
        v.extend_from_slice(&(f.len() as u32).to_le_bytes());
        v.extend_from_slice(f.as_bytes());
    }
    v
}

fn ferr<E: std::fmt::Debug>(what: &str) -> impl Fn(E) -> String + '_ {
    move |e| format!("FLAC {what}: {e:?}")
}

/// `fLaC`, STREAMINFO and the Vorbis comment, as they stand now. Its
/// length never changes, so the end can write it over the first one.
fn header(info: &StreamInfo, comment: &[u8], frames_written: usize) -> Result<Vec<u8>, String> {
    let mut info = info.clone();
    if frames_written == 0 {
        // Nothing encoded yet: placeholder sizes that are in range.
        info.set_frame_sizes(0, 0).map_err(ferr("header"))?;
        info.set_block_sizes(BLOCK_SIZE, BLOCK_SIZE)
            .map_err(ferr("header"))?;
    }
    let mut s = Stream::with_stream_info(info);
    s.add_metadata_block(MetadataBlockData::new_unknown(4, comment).map_err(ferr("tags"))?);
    let mut sink = MemSink::<u8>::new();
    s.write(&mut sink).map_err(|_| "FLAC header".to_string())?;
    Ok(sink.into_inner())
}

/// Encodes blocks as they arrive.
pub struct FlacWriter<W: Write + Seek> {
    out: W,
    cfg: Verified<config::Encoder>,
    info: StreamInfo,
    comment: Vec<u8>,
    header_len: usize,
    fb: FrameBuf,
    ctx: Context,
    pending: Vec<i32>,
    channels: usize,
    sink: MemSink<u8>,
    frames_written: usize,
}

impl<W: Write + Seek> FlacWriter<W> {
    pub fn new(
        out: W,
        rate: u32,
        channels: usize,
        bits: usize,
        level: u8,
        tags: &[(Tag, String)],
    ) -> Result<FlacWriter<W>, String> {
        Self::with_comments(out, rate, channels, bits, level, tags, &[])
    }

    /// [`FlacWriter::new`] with more Vorbis comment fields (chapters).
    pub fn with_comments(
        mut out: W,
        rate: u32,
        channels: usize,
        bits: usize,
        level: u8,
        tags: &[(Tag, String)],
        extra: &[String],
    ) -> Result<FlacWriter<W>, String> {
        if channels == 0 || channels > MAX_CHANNELS {
            return Err(format!(
                "FLAC holds 1 to {MAX_CHANNELS} channels, not {channels}. Use WAV for this one."
            ));
        }
        if rate == 0 || rate > 655_350 {
            return Err(format!("FLAC cannot store a {rate} Hz sample rate."));
        }
        let cfg = level_config(level)
            .into_verified()
            .map_err(|(_, e)| format!("FLAC settings: {e}"))?;
        let info = StreamInfo::new(rate as usize, channels, bits).map_err(ferr("stream"))?;
        let comment = vorbis_comment_with(tags, extra, "AudiForm 2 (flacenc)");
        let h = header(&info, &comment, 0)?;
        out.write_all(&h).map_err(|e| e.to_string())?;
        Ok(FlacWriter {
            fb: FrameBuf::with_size(channels, BLOCK_SIZE).map_err(ferr("buffer"))?,
            ctx: Context::new(bits, channels),
            out,
            cfg,
            info,
            comment,
            header_len: h.len(),
            pending: Vec::with_capacity(BLOCK_SIZE * channels),
            channels,
            sink: MemSink::new(),
            frames_written: 0,
        })
    }

    /// Interleaved integer samples, in range for the bit depth.
    pub fn write(&mut self, samples: &[i32]) -> Result<(), String> {
        let block = BLOCK_SIZE * self.channels;
        let mut rest = samples;
        while !rest.is_empty() {
            let take = (block - self.pending.len()).min(rest.len());
            self.pending.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.pending.len() == block {
                self.encode_pending()?;
            }
        }
        Ok(())
    }

    fn encode_pending(&mut self) -> Result<(), String> {
        if self.pending.is_empty() {
            return Ok(());
        }
        (&mut self.fb, &mut self.ctx)
            .fill_interleaved(&self.pending)
            .map_err(ferr("input"))?;
        self.pending.clear();
        let n = self.ctx.current_frame_number().unwrap_or(0);
        let frame = flacenc::encode_fixed_size_frame(&self.cfg, &self.fb, n, &self.info)
            .map_err(|e| format!("FLAC encoding failed: {e}"))?;
        self.info.update_frame_info(&frame);
        self.sink.clear();
        frame
            .write(&mut self.sink)
            .map_err(|_| "FLAC frame".to_string())?;
        self.out
            .write_all(self.sink.as_slice())
            .map_err(|e| e.to_string())?;
        self.frames_written += 1;
        Ok(())
    }

    /// Encodes what is left and writes the final STREAMINFO.
    pub fn finish(mut self) -> Result<(W, u64), String> {
        self.encode_pending()?;
        self.info.set_md5_digest(&self.ctx.md5_digest());
        if self.frames_written > 0 {
            self.info
                .set_block_sizes(BLOCK_SIZE, BLOCK_SIZE)
                .map_err(ferr("header"))?;
        }
        let end = self.out.stream_position().map_err(|e| e.to_string())?;
        let h = header(&self.info, &self.comment, self.frames_written)?;
        if h.len() != self.header_len {
            return Err("FLAC header changed size".into());
        }
        self.out
            .seek(SeekFrom::Start(0))
            .and_then(|_| self.out.write_all(&h))
            .and_then(|_| self.out.seek(SeekFrom::Start(end)))
            .and_then(|_| self.out.flush())
            .map_err(|e| e.to_string())?;
        Ok((self.out, end))
    }
}
