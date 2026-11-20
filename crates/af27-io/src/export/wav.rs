// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! A streaming WAV writer: blocks go to disk as they are rendered, and the
//! sizes are filled in at the end. Files that grow past 4 GB become RF64
//! (EBU Tech 3306): a `JUNK` chunk reserved at the front turns into `ds64`.
//! Files with more than two channels use WAVE_FORMAT_EXTENSIBLE with a
//! speaker mask. Markers go in a `cue ` chunk with a `LIST adtl` holding
//! their names (`labl`) and range lengths (`ltxt`); tags in `LIST INFO`.
//! Both sit before the audio, so a reader that stops at `data` sees them.
//!
//! [`inspect`] reads the headers back (RIFF and RF64) without loading the
//! audio, for checks after writing and for tests.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::{SampleFormat, Tag};

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// The tail shared by the KSDATAFORMAT_SUBTYPE GUIDs; the first two bytes
/// are the format tag.
const GUID_TAIL: [u8; 14] = [
    0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];
/// `JUNK` payload reserved for a `ds64` chunk: RIFF size, data size and
/// sample count (u64 each) and an empty table length (u32).
const DS64_LEN: u32 = 28;

/// What a WAV file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavSpec {
    pub rate: u32,
    pub channels: u16,
    pub format: SampleFormat,
    /// Speaker mask for WAVE_FORMAT_EXTENSIBLE; `None` picks the usual one
    /// for the channel count.
    pub channel_mask: Option<u32>,
}

impl WavSpec {
    pub fn block_align(&self) -> u16 {
        self.channels * self.format.bytes() as u16
    }

    /// Extensible is used for more than two channels (the speaker mask
    /// matters there); mono and stereo stay in the plain form every
    /// reader knows.
    pub fn extensible(&self) -> bool {
        self.channels > 2
    }
}

/// A cue point: a marker at `frame`, a range when `length` > 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuePoint {
    pub id: u32,
    pub frame: u32,
    pub length: u32,
    pub label: String,
}

/// The usual WAVE speaker mask for a channel count (in the order the
/// layouts use: L R C LFE, rear, side, heights).
pub fn default_channel_mask(channels: u16) -> u32 {
    const FL: u32 = 0x1;
    const FR: u32 = 0x2;
    const FC: u32 = 0x4;
    const LFE: u32 = 0x8;
    const BL: u32 = 0x10;
    const BR: u32 = 0x20;
    const BC: u32 = 0x100;
    const SL: u32 = 0x200;
    const SR: u32 = 0x400;
    const TFL: u32 = 0x1000;
    const TFR: u32 = 0x4000;
    const TBL: u32 = 0x8000;
    const TBR: u32 = 0x20000;
    match channels {
        1 => FC,
        2 => FL | FR,
        3 => FL | FR | FC,
        4 => FL | FR | BL | BR,
        5 => FL | FR | FC | BL | BR,
        6 => FL | FR | FC | LFE | BL | BR,
        7 => FL | FR | FC | LFE | BL | BR | BC,
        8 => FL | FR | FC | LFE | BL | BR | SL | SR,
        // 7.1.2 (the top pair as top front)
        10 => FL | FR | FC | LFE | BL | BR | SL | SR | TFL | TFR,
        // 7.1.4
        12 => FL | FR | FC | LFE | BL | BR | SL | SR | TFL | TFR | TBL | TBR,
        _ => 0,
    }
}

fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(payload.len() + 9);
    v.extend_from_slice(id);
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        v.push(0);
    }
    v
}

/// The `fmt ` chunk.
pub fn fmt_chunk(spec: &WavSpec) -> Vec<u8> {
    let bits = spec.format.bits();
    let tag = if spec.format == SampleFormat::F32 {
        WAVE_FORMAT_IEEE_FLOAT
    } else {
        WAVE_FORMAT_PCM
    };
    let mut p = Vec::with_capacity(40);
    p.extend_from_slice(
        &(if spec.extensible() {
            WAVE_FORMAT_EXTENSIBLE
        } else {
            tag
        })
        .to_le_bytes(),
    );
    p.extend_from_slice(&spec.channels.to_le_bytes());
    p.extend_from_slice(&spec.rate.to_le_bytes());
    p.extend_from_slice(&(spec.rate * spec.block_align() as u32).to_le_bytes());
    p.extend_from_slice(&spec.block_align().to_le_bytes());
    p.extend_from_slice(&bits.to_le_bytes());
    if spec.extensible() {
        let mask = spec
            .channel_mask
            .unwrap_or_else(|| default_channel_mask(spec.channels));
        p.extend_from_slice(&22u16.to_le_bytes());
        p.extend_from_slice(&bits.to_le_bytes()); // valid bits
        p.extend_from_slice(&mask.to_le_bytes());
        p.extend_from_slice(&tag.to_le_bytes());
        p.extend_from_slice(&GUID_TAIL);
    } else if tag == WAVE_FORMAT_IEEE_FLOAT {
        p.extend_from_slice(&0u16.to_le_bytes());
    }
    chunk(b"fmt ", &p)
}

fn info_id(tag: Tag) -> &'static [u8; 4] {
    match tag {
        Tag::Title => b"INAM",
        Tag::Artist => b"IART",
        Tag::Album => b"IPRD",
        Tag::Comment => b"ICMT",
        Tag::Date => b"ICRD",
        Tag::Genre => b"IGNR",
        Tag::Track => b"ITRK",
        Tag::Copyright => b"ICOP",
        Tag::Software => b"ISFT",
    }
}

fn tag_of_info(id: &[u8]) -> Option<Tag> {
    Tag::ALL.into_iter().find(|t| info_id(*t) == id)
}

fn zstr(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.bytes().filter(|&b| b != 0).collect();
    v.push(0);
    v
}

/// A `LIST INFO` chunk, or nothing when there are no tags.
pub fn info_chunk(tags: &[(Tag, String)]) -> Vec<u8> {
    let mut p = b"INFO".to_vec();
    for (t, v) in tags {
        if !v.is_empty() {
            p.extend(chunk(info_id(*t), &zstr(v)));
        }
    }
    if p.len() == 4 {
        return Vec::new();
    }
    chunk(b"LIST", &p)
}

/// A `cue ` chunk and its `LIST adtl`, or nothing without cue points.
pub fn cue_chunks(points: &[CuePoint]) -> Vec<u8> {
    if points.is_empty() {
        return Vec::new();
    }
    let mut cue = (points.len() as u32).to_le_bytes().to_vec();
    for c in points {
        cue.extend_from_slice(&c.id.to_le_bytes());
        cue.extend_from_slice(&c.frame.to_le_bytes()); // play order position
        cue.extend_from_slice(b"data");
        cue.extend_from_slice(&0u32.to_le_bytes()); // chunk start
        cue.extend_from_slice(&0u32.to_le_bytes()); // block start
        cue.extend_from_slice(&c.frame.to_le_bytes()); // sample offset
    }
    let mut adtl = b"adtl".to_vec();
    for c in points {
        let mut l = c.id.to_le_bytes().to_vec();
        l.extend(zstr(&c.label));
        adtl.extend(chunk(b"labl", &l));
        if c.length > 0 {
            let mut t = c.id.to_le_bytes().to_vec();
            t.extend_from_slice(&c.length.to_le_bytes());
            t.extend_from_slice(b"rgn ");
            t.extend_from_slice(&[0u8; 8]); // country, language, dialect, code page
            adtl.extend(chunk(b"ltxt", &t));
        }
    }
    let mut v = chunk(b"cue ", &cue);
    v.extend(chunk(b"LIST", &adtl));
    v
}

/// Where the size fields sit in a header written by [`WavWriter`].
#[derive(Debug, Clone, Copy)]
struct Layout {
    /// Offset of the `fact` chunk's sample count, for float files.
    fact_at: Option<u64>,
    /// Offset of the `data` chunk's size field.
    data_size_at: u64,
    /// Offset of the first audio byte.
    data_start: u64,
}

/// The header up to and including the `data` chunk header.
fn header(spec: &WavSpec, extra: &[u8]) -> (Vec<u8>, Layout) {
    let mut h = Vec::with_capacity(256 + extra.len());
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(b"WAVE");
    h.extend(chunk(b"JUNK", &[0u8; DS64_LEN as usize]));
    h.extend(fmt_chunk(spec));
    let fact_at = (spec.format == SampleFormat::F32).then(|| {
        let at = h.len() as u64 + 8;
        h.extend(chunk(b"fact", &0u32.to_le_bytes()));
        at
    });
    h.extend_from_slice(extra);
    h.extend_from_slice(b"data");
    let data_size_at = h.len() as u64;
    h.extend_from_slice(&0u32.to_le_bytes());
    let data_start = h.len() as u64;
    (
        h,
        Layout {
            fact_at,
            data_size_at,
            data_start,
        },
    )
}

/// Whether a file with this much audio must be RF64.
pub fn needs_rf64(data_start: u64, data_len: u64) -> bool {
    let riff_size = data_start + data_len + (data_len & 1) - 8;
    riff_size > u32::MAX as u64 || data_len > u32::MAX as u64
}

/// The writes that finish a header: (offset, bytes).
fn patches(layout: &Layout, data_len: u64, frames: u64) -> Vec<(u64, Vec<u8>)> {
    let pad = data_len & 1;
    let riff_size = layout.data_start + data_len + pad - 8;
    let mut v = Vec::new();
    if needs_rf64(layout.data_start, data_len) {
        v.push((0, b"RF64".to_vec()));
        v.push((4, u32::MAX.to_le_bytes().to_vec()));
        let mut ds = b"ds64".to_vec();
        ds.extend_from_slice(&DS64_LEN.to_le_bytes());
        ds.extend_from_slice(&riff_size.to_le_bytes());
        ds.extend_from_slice(&data_len.to_le_bytes());
        ds.extend_from_slice(&frames.to_le_bytes());
        ds.extend_from_slice(&0u32.to_le_bytes());
        v.push((12, ds));
        v.push((layout.data_size_at, u32::MAX.to_le_bytes().to_vec()));
        if let Some(at) = layout.fact_at {
            v.push((at, u32::MAX.to_le_bytes().to_vec()));
        }
    } else {
        v.push((4, (riff_size as u32).to_le_bytes().to_vec()));
        v.push((
            layout.data_size_at,
            (data_len as u32).to_le_bytes().to_vec(),
        ));
        if let Some(at) = layout.fact_at {
            v.push((at, (frames as u32).to_le_bytes().to_vec()));
        }
    }
    v
}

/// Writes samples as they come and finishes the header at the end.
pub struct WavWriter<W: Write + Seek> {
    out: W,
    spec: WavSpec,
    layout: Layout,
    data_len: u64,
    bytes: Vec<u8>,
}

impl<W: Write + Seek> WavWriter<W> {
    /// Writes the header, with `tags` and `cues` before the audio.
    pub fn new(
        mut out: W,
        spec: WavSpec,
        tags: &[(Tag, String)],
        cues: &[CuePoint],
    ) -> std::io::Result<WavWriter<W>> {
        let mut extra = info_chunk(tags);
        extra.extend(cue_chunks(cues));
        let (h, layout) = header(&spec, &extra);
        out.write_all(&h)?;
        Ok(WavWriter {
            out,
            spec,
            layout,
            data_len: 0,
            bytes: Vec::new(),
        })
    }

    pub fn spec(&self) -> &WavSpec {
        &self.spec
    }

    /// Integer samples, interleaved, already in range for the bit depth.
    pub fn write_int(&mut self, samples: &[i32]) -> std::io::Result<()> {
        self.bytes.clear();
        match self.spec.format {
            SampleFormat::U8 => self
                .bytes
                .extend(samples.iter().map(|&s| (s + 128).clamp(0, 255) as u8)),
            SampleFormat::I16 => {
                for &s in samples {
                    self.bytes.extend_from_slice(&(s as i16).to_le_bytes());
                }
            }
            SampleFormat::I24 => {
                for &s in samples {
                    self.bytes.extend_from_slice(&s.to_le_bytes()[..3]);
                }
            }
            SampleFormat::F32 => {
                for &s in samples {
                    self.bytes.extend_from_slice(&(s as f32).to_le_bytes());
                }
            }
        }
        self.data_len += self.bytes.len() as u64;
        self.out.write_all(&self.bytes)
    }

    /// Float samples, interleaved (32-bit float files).
    pub fn write_float(&mut self, samples: &[f32]) -> std::io::Result<()> {
        self.bytes.clear();
        for &s in samples {
            self.bytes.extend_from_slice(&s.to_le_bytes());
        }
        self.data_len += self.bytes.len() as u64;
        self.out.write_all(&self.bytes)
    }

    /// Bytes of audio written so far.
    pub fn data_len(&self) -> u64 {
        self.data_len
    }

    /// Pads the audio to an even length and fills in the sizes (RF64
    /// when needed). Returns the writer and the file's length.
    pub fn finish(mut self) -> std::io::Result<(W, u64)> {
        if self.data_len & 1 == 1 {
            self.out.write_all(&[0])?;
        }
        let end = self.layout.data_start + self.data_len + (self.data_len & 1);
        let frames = self.data_len / self.spec.block_align().max(1) as u64;
        for (at, b) in patches(&self.layout, self.data_len, frames) {
            self.out.seek(SeekFrom::Start(at))?;
            self.out.write_all(&b)?;
        }
        self.out.seek(SeekFrom::Start(end))?;
        self.out.flush()?;
        Ok((self.out, end))
    }
}

/// What [`inspect`] found in a WAV file's headers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WavInfo {
    pub rf64: bool,
    pub format_tag: u16,
    /// The real format for extensible files (1 PCM, 3 float).
    pub sub_format: u16,
    pub channels: u16,
    pub rate: u32,
    pub bits: u16,
    pub block_align: u16,
    pub channel_mask: Option<u32>,
    pub data_start: u64,
    pub data_len: u64,
    /// From `ds64` or `fact`, if present.
    pub declared_frames: Option<u64>,
    pub cues: Vec<CuePoint>,
    pub tags: Vec<(Tag, String)>,
}

impl WavInfo {
    pub fn frames(&self) -> u64 {
        self.data_len / self.block_align.max(1) as u64
    }
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn le64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}
fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// Reads a WAV file's headers (RIFF or RF64), skipping over the audio.
pub fn inspect(path: &Path) -> Result<WavInfo, String> {
    let f = File::open(path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    let len = f.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
    inspect_reader(BufReader::new(f), len)
}

/// [`inspect`] over any reader of `len` bytes.
pub fn inspect_reader<R: Read + Seek>(mut r: R, len: u64) -> Result<WavInfo, String> {
    let bad = |m: &str| format!("Not a usable WAV file: {m}");
    let mut head = [0u8; 12];
    r.read_exact(&mut head).map_err(|_| bad("too short"))?;
    let rf64 = match &head[0..4] {
        b"RIFF" => false,
        b"RF64" => true,
        _ => return Err(bad("no RIFF header")),
    };
    if &head[8..12] != b"WAVE" {
        return Err(bad("not WAVE"));
    }
    let mut info = WavInfo {
        rf64,
        ..Default::default()
    };
    let mut ds64_data = None;
    let mut pos = 12u64;
    let mut labels: Vec<(u32, String)> = Vec::new();
    let mut lengths: Vec<(u32, u32)> = Vec::new();
    while pos + 8 <= len {
        let mut h = [0u8; 8];
        if r.read_exact(&mut h).is_err() {
            break;
        }
        let id = [h[0], h[1], h[2], h[3]];
        let mut size = le32(&h, 4) as u64;
        let body = pos + 8;
        if &id == b"data" {
            if size == u32::MAX as u64 {
                size = ds64_data.ok_or_else(|| bad("RF64 without ds64"))?;
            }
            info.data_start = body;
            info.data_len = size.min(len.saturating_sub(body));
        } else if matches!(&id, b"fmt " | b"ds64" | b"fact" | b"cue " | b"LIST") {
            if size > 16 << 20 {
                return Err(bad("oversized header chunk"));
            }
            let mut p = vec![0u8; size as usize];
            r.read_exact(&mut p).map_err(|_| bad("cut short"))?;
            match &id {
                b"fmt " if p.len() >= 16 => {
                    info.format_tag = le16(&p, 0);
                    info.sub_format = info.format_tag;
                    info.channels = le16(&p, 2);
                    info.rate = le32(&p, 4);
                    info.block_align = le16(&p, 12);
                    info.bits = le16(&p, 14);
                    if info.format_tag == WAVE_FORMAT_EXTENSIBLE && p.len() >= 40 {
                        info.channel_mask = Some(le32(&p, 20));
                        info.sub_format = le16(&p, 24);
                    }
                }
                b"ds64" if p.len() >= 24 => {
                    ds64_data = Some(le64(&p, 8));
                    info.declared_frames = Some(le64(&p, 16));
                }
                b"fact" if p.len() >= 4 => {
                    let n = le32(&p, 0);
                    if info.declared_frames.is_none() && n != u32::MAX {
                        info.declared_frames = Some(n as u64);
                    }
                }
                b"cue " if p.len() >= 4 => {
                    let n = le32(&p, 0) as usize;
                    for i in 0..n {
                        let at = 4 + i * 24;
                        if at + 24 > p.len() {
                            break;
                        }
                        info.cues.push(CuePoint {
                            id: le32(&p, at),
                            frame: le32(&p, at + 20),
                            length: 0,
                            label: String::new(),
                        });
                    }
                }
                b"LIST" if p.len() >= 4 => {
                    let kind = &p[0..4];
                    let mut at = 4;
                    while at + 8 <= p.len() {
                        let sid = &p[at..at + 4];
                        let sl = le32(&p, at + 4) as usize;
                        let s = &p[at + 8..(at + 8 + sl).min(p.len())];
                        match (kind, sid) {
                            (b"adtl", b"labl") | (b"adtl", b"note") if s.len() >= 4 => {
                                if sid == b"labl" {
                                    labels.push((le32(s, 0), cstr(&s[4..])));
                                }
                            }
                            (b"adtl", b"ltxt") if s.len() >= 8 => {
                                lengths.push((le32(s, 0), le32(s, 4)));
                            }
                            (b"INFO", _) => {
                                if let Some(t) = tag_of_info(sid) {
                                    info.tags.push((t, cstr(s)));
                                }
                            }
                            _ => {}
                        }
                        at += 8 + sl + (sl & 1);
                    }
                }
                _ => {}
            }
        }
        pos = body + size + (size & 1);
        r.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
    }
    if info.channels == 0 || info.block_align == 0 {
        return Err(bad("no format chunk"));
    }
    for c in &mut info.cues {
        if let Some((_, l)) = labels.iter().find(|(id, _)| *id == c.id) {
            c.label = l.clone();
        }
        if let Some((_, n)) = lengths.iter().find(|(id, _)| *id == c.id) {
            c.length = *n;
        }
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn spec(ch: u16, format: SampleFormat) -> WavSpec {
        WavSpec {
            rate: 48_000,
            channels: ch,
            format,
            channel_mask: None,
        }
    }

    fn write(spec: WavSpec, frames: usize, cues: &[CuePoint]) -> Vec<u8> {
        let tags = vec![
            (Tag::Title, "Test take".to_string()),
            (Tag::Software, "AudiForm 2".to_string()),
        ];
        let mut w = WavWriter::new(Cursor::new(Vec::new()), spec, &tags, cues).unwrap();
        let n = frames * spec.channels as usize;
        let samples: Vec<i32> = (0..n as i32).map(|i| (i % 100) - 50).collect();
        // In two blocks, as a render would hand them over.
        w.write_int(&samples[..n / 2]).unwrap();
        w.write_int(&samples[n / 2..]).unwrap();
        w.finish().unwrap().0.into_inner()
    }

    #[test]
    fn stereo_16_bit_header_is_plain_pcm() {
        let b = write(spec(2, SampleFormat::I16), 1001, &[]);
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(le32(&b, 4) as usize, b.len() - 8);
        assert_eq!(&b[12..16], b"JUNK");
        assert_eq!(&b[48..52], b"fmt ");
        assert_eq!(le32(&b, 52), 16);
        assert_eq!(le16(&b, 56), WAVE_FORMAT_PCM);
        let info = inspect_reader(Cursor::new(&b), b.len() as u64).unwrap();
        assert!(!info.rf64);
        assert_eq!((info.channels, info.rate, info.bits), (2, 48_000, 16));
        assert_eq!(info.block_align, 4);
        assert_eq!(info.data_len, 4004);
        assert_eq!(info.frames(), 1001);
        assert_eq!(info.tags[0], (Tag::Title, "Test take".to_string()));
        // hound reads it too, sample for sample.
        let mut r = hound::WavReader::new(Cursor::new(&b)).unwrap();
        assert_eq!(r.spec().channels, 2);
        assert_eq!(r.len(), 2002);
        let first: Vec<i16> = r.samples::<i16>().take(3).map(|s| s.unwrap()).collect();
        assert_eq!(first, vec![-50, -49, -48]);
    }

    #[test]
    fn odd_data_is_padded_and_float_has_fact() {
        let b = write(spec(1, SampleFormat::U8), 7, &[]);
        assert_eq!(b.len() % 2, 0);
        let info = inspect_reader(Cursor::new(&b), b.len() as u64).unwrap();
        assert_eq!(info.data_len, 7);
        assert_eq!(le32(&b, 4) as usize, b.len() - 8);

        let mut w = WavWriter::new(
            Cursor::new(Vec::new()),
            spec(2, SampleFormat::F32),
            &[],
            &[],
        )
        .unwrap();
        w.write_float(&[0.5, -0.5, 0.25, -0.25, 1.5, -1.5]).unwrap();
        let b = w.finish().unwrap().0.into_inner();
        let info = inspect_reader(Cursor::new(&b), b.len() as u64).unwrap();
        assert_eq!(info.format_tag, WAVE_FORMAT_IEEE_FLOAT);
        assert_eq!(info.declared_frames, Some(3));
        let mut r = hound::WavReader::new(Cursor::new(&b)).unwrap();
        let s: Vec<f32> = r.samples::<f32>().map(|s| s.unwrap()).collect();
        assert_eq!(s, vec![0.5, -0.5, 0.25, -0.25, 1.5, -1.5]);
    }

    #[test]
    fn surround_uses_extensible_with_its_mask() {
        for (ch, mask) in [(6u16, 0x3Fu32), (8, 0x63F), (12, 0x2D63F)] {
            let b = write(spec(ch, SampleFormat::I24), 10, &[]);
            let info = inspect_reader(Cursor::new(&b), b.len() as u64).unwrap();
            assert_eq!(info.format_tag, WAVE_FORMAT_EXTENSIBLE);
            assert_eq!(info.sub_format, WAVE_FORMAT_PCM);
            assert_eq!(info.channel_mask, Some(mask), "{ch} channels");
            assert_eq!(info.bits, 24);
            assert_eq!(info.frames(), 10);
            assert_eq!((ch as u32 * 3 * 10) as u64, info.data_len);
        }
        // A given mask wins (a 5.1 bed with side surrounds).
        let s = WavSpec {
            channel_mask: Some(0x60F),
            ..spec(6, SampleFormat::F32)
        };
        let b = write(s, 4, &[]);
        let info = inspect_reader(Cursor::new(&b), b.len() as u64).unwrap();
        assert_eq!(info.channel_mask, Some(0x60F));
        assert_eq!(info.sub_format, WAVE_FORMAT_IEEE_FLOAT);
    }

    #[test]
    fn cue_points_read_back_with_names_and_lengths() {
        let cues = vec![
            CuePoint {
                id: 1,
                frame: 0,
                length: 0,
                label: "Intro".into(),
            },
            CuePoint {
                id: 2,
                frame: 480,
                length: 240,
                label: "Ad break".into(),
            },
        ];
        let b = write(spec(2, SampleFormat::I16), 1000, &cues);
        let info = inspect_reader(Cursor::new(&b), b.len() as u64).unwrap();
        assert_eq!(info.cues, cues);
        assert_eq!(info.frames(), 1000);
        // Other readers skip the chunks and still find the audio.
        let r = hound::WavReader::new(Cursor::new(&b)).unwrap();
        assert_eq!(r.len(), 2000);
    }

    #[test]
    fn rf64_switch_by_size() {
        let (h, layout) = header(&spec(2, SampleFormat::F32), &[]);
        let small = 1000u64;
        assert!(!needs_rf64(layout.data_start, small));
        assert!(!needs_rf64(layout.data_start, u32::MAX as u64 - 200));
        assert!(needs_rf64(layout.data_start, u32::MAX as u64 - 8));
        assert!(needs_rf64(layout.data_start, 5 << 30));

        // Simulate a 5 GB file: patch the header as `finish` would and
        // read it back with the data size taken from ds64.
        let data_len = 5u64 << 30;
        let frames = data_len / 8;
        let mut b = h.clone();
        for (at, p) in patches(&layout, data_len, frames) {
            let at = at as usize;
            b[at..at + p.len()].copy_from_slice(&p);
        }
        assert_eq!(&b[0..4], b"RF64");
        assert_eq!(le32(&b, 4), u32::MAX);
        assert_eq!(&b[12..16], b"ds64");
        assert_eq!(le64(&b, 20), layout.data_start + data_len - 8);
        assert_eq!(le32(&b, layout.data_size_at as usize), u32::MAX);
        let fake_len = layout.data_start + data_len;
        let info = inspect_reader(Cursor::new(&b), fake_len).unwrap();
        assert!(info.rf64);
        assert_eq!(info.data_len, data_len);
        assert_eq!(info.frames(), frames);
        assert_eq!(info.declared_frames, Some(frames));

        // The same header for a small file stays RIFF, the JUNK in place.
        let mut b = h;
        for (at, p) in patches(&layout, small, small / 8) {
            let at = at as usize;
            b[at..at + p.len()].copy_from_slice(&p);
        }
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(&b[12..16], b"JUNK");
    }
}
