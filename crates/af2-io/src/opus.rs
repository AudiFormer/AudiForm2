// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Ogg Opus reading (RFC 7845) with the pure-Rust `opus-pure` decoder
//! (BSD-3-Clause), since Symphonia 0.5 has no Opus decoder.
//!
//! Handles the pre-skip, the header's output gain, end trimming from the
//! last page's granule position, any channel mapping table (family 0,
//! family 1 surround reordered to WAV order, and family 255), and
//! chained streams (one Opus stream after another in the same file).
//! Damaged packets are concealed and counted rather than ending the read.
//!
//! Multistream packets are split here (RFC 6716 appendix B) and each
//! elementary stream goes to its own single-stream decoder, so files with
//! non-standard mapping tables open too.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

use af2_core::AudioBuffer;
use ogg::PacketReader;
use opus_pure::OpusDecoder;

use crate::Decoded;

/// Opus always decodes at 48 kHz here (the format's native rate).
pub(crate) const RATE: u32 = 48_000;
/// The longest Opus packet: 120 ms at 48 kHz.
const MAX_FRAME: usize = 5760;

/// True when `r` starts with an Ogg page whose first packet is an Opus
/// identification header. Leaves the reader at the start.
pub(crate) fn sniff<R: Read + Seek>(r: &mut R) -> bool {
    let mut head = [0u8; 300];
    let mut n = 0;
    while n < head.len() {
        match r.read(&mut head[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(_) => break,
        }
    }
    let _ = r.seek(SeekFrom::Start(0));
    if n < 27 || &head[..4] != b"OggS" {
        return false;
    }
    let data = 27 + head[26] as usize;
    n >= data + 8 && &head[data..data + 8] == b"OpusHead"
}

/// The parsed `OpusHead` packet.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Head {
    pub channels: usize,
    pub pre_skip: usize,
    /// Output gain in dB (stored as Q7.8 in the header).
    pub gain_db: f32,
    pub family: u8,
    pub streams: usize,
    pub coupled: usize,
    pub mapping: Vec<u8>,
}

impl Head {
    pub(crate) fn parse(p: &[u8]) -> Result<Head, String> {
        if p.len() < 19 || &p[..8] != b"OpusHead" {
            return Err("The Opus header is damaged".into());
        }
        if p[8] >> 4 != 0 {
            return Err(format!("Unsupported Opus version {}", p[8]));
        }
        let channels = p[9] as usize;
        if channels == 0 {
            return Err("The Opus header says the stream has no channels".into());
        }
        let pre_skip = u16::from_le_bytes([p[10], p[11]]) as usize;
        let gain_db = i16::from_le_bytes([p[16], p[17]]) as f32 / 256.0;
        let family = p[18];
        let (streams, coupled, mapping) = match family {
            0 => {
                if channels > 2 {
                    return Err("The Opus header is damaged (too many channels)".into());
                }
                (1, channels - 1, (0..channels as u8).collect())
            }
            3 => return Err("Ambisonic Opus files (mapping family 3) are not supported".into()),
            _ => {
                if p.len() < 21 + channels {
                    return Err("The Opus header is damaged (short channel table)".into());
                }
                let streams = p[19] as usize;
                let coupled = p[20] as usize;
                if streams == 0 || coupled > streams || streams + coupled > 255 {
                    return Err("The Opus header is damaged (bad stream counts)".into());
                }
                let mapping = p[21..21 + channels].to_vec();
                if mapping
                    .iter()
                    .any(|&m| m != 255 && m as usize >= streams + coupled)
                {
                    return Err("The Opus header is damaged (bad channel table)".into());
                }
                (streams, coupled, mapping)
            }
        };
        Ok(Head {
            channels,
            pre_skip,
            gain_db,
            family,
            streams,
            coupled,
            mapping,
        })
    }

    /// Linear factor for the output gain.
    pub(crate) fn gain(&self) -> f32 {
        10f32.powf(self.gain_db / 20.0)
    }

    /// For each output channel (in WAV order), the file channel it takes.
    /// Family 1 uses the Vorbis channel order, which is reordered to the
    /// WAV order the rest of the app (and Symphonia) uses.
    pub(crate) fn order(&self) -> Vec<usize> {
        let vorbis_to_wav: &[usize] = match (self.family, self.channels) {
            (1, 3) => &[0, 2, 1],
            (1, 5) => &[0, 2, 1, 3, 4],
            (1, 6) => &[0, 2, 1, 5, 3, 4],
            (1, 7) => &[0, 2, 1, 6, 5, 3, 4],
            (1, 8) => &[0, 2, 1, 7, 5, 6, 3, 4],
            _ => &[],
        };
        if vorbis_to_wav.is_empty() {
            (0..self.channels).collect()
        } else {
            vorbis_to_wav.to_vec()
        }
    }

    /// Where each file channel comes from: (stream, side), or `None` for
    /// a silent channel (mapping 255).
    fn sources(&self) -> Vec<Option<(usize, usize)>> {
        self.mapping
            .iter()
            .map(|&m| {
                let m = m as usize;
                if m == 255 {
                    None
                } else if m < 2 * self.coupled {
                    Some((m / 2, m % 2))
                } else {
                    Some((self.coupled + m - 2 * self.coupled, 0))
                }
            })
            .collect()
    }
}

/// A frame length in the Opus packet format: one byte below 252, else two.
fn frame_len(d: &[u8], at: usize) -> Option<(usize, usize)> {
    let b0 = *d.get(at)? as usize;
    if b0 < 252 {
        Some((b0, 1))
    } else {
        Some((b0 + 4 * *d.get(at + 1)? as usize, 2))
    }
}

/// Turns the self-delimited packet at the start of `d` into a normal one
/// (dropping its extra length field); returns it and the bytes it took.
pub(crate) fn undelimit(d: &[u8]) -> Option<(Vec<u8>, usize)> {
    let toc = *d.first()?;
    let take = |a: usize, b: usize| d.get(a..b);
    match toc & 3 {
        0 | 1 => {
            let (l, n) = frame_len(d, 1)?;
            let total = if toc & 3 == 0 { l } else { 2 * l };
            let mut out = vec![toc];
            out.extend_from_slice(take(1 + n, 1 + n + total)?);
            Some((out, 1 + n + total))
        }
        2 => {
            let (l1, n1) = frame_len(d, 1)?;
            let (l2, n2) = frame_len(d, 1 + n1)?;
            let body = 1 + n1 + n2;
            let mut out = take(0, 1 + n1)?.to_vec();
            out.extend_from_slice(take(body, body + l1 + l2)?);
            Some((out, body + l1 + l2))
        }
        _ => {
            let c = *d.get(1)?;
            let vbr = c & 0x80 != 0;
            let m = (c & 0x3F) as usize;
            if m == 0 {
                return None;
            }
            let mut pos = 2;
            let mut pad = 0usize;
            if c & 0x40 != 0 {
                loop {
                    let b = *d.get(pos)?;
                    pos += 1;
                    if b == 255 {
                        pad += 254;
                    } else {
                        pad += b as usize;
                        break;
                    }
                }
            }
            if vbr {
                let mut sum = 0;
                for _ in 0..m - 1 {
                    let (l, n) = frame_len(d, pos)?;
                    sum += l;
                    pos += n;
                }
                let (last, n) = frame_len(d, pos)?;
                let body = pos + n;
                let mut out = take(0, pos)?.to_vec();
                let end = body + sum + last + pad;
                out.extend_from_slice(take(body, end)?);
                Some((out, end))
            } else {
                let (l, n) = frame_len(d, pos)?;
                let body = pos + n;
                let end = body + m * l + pad;
                let mut out = take(0, pos)?.to_vec();
                out.extend_from_slice(take(body, end)?);
                Some((out, end))
            }
        }
    }
}

/// Decodes one stream's multistream packets.
struct MultiDecoder {
    decs: Vec<OpusDecoder>,
    coupled: usize,
    sources: Vec<Option<(usize, usize)>>,
    /// Per-stream scratch output, interleaved per stream.
    scratch: Vec<Vec<f32>>,
    last_len: usize,
}

impl MultiDecoder {
    fn new(h: &Head) -> Result<MultiDecoder, String> {
        let mut decs = Vec::with_capacity(h.streams);
        for s in 0..h.streams {
            let ch = if s < h.coupled { 2 } else { 1 };
            decs.push(
                OpusDecoder::new(RATE as i32, ch)
                    .map_err(|e| format!("Could not set up the Opus decoder: {e}"))?,
            );
        }
        Ok(MultiDecoder {
            scratch: (0..h.streams)
                .map(|s| vec![0.0; MAX_FRAME * if s < h.coupled { 2 } else { 1 }])
                .collect(),
            decs,
            coupled: h.coupled,
            sources: h.sources(),
            last_len: 960,
        })
    }

    /// Decodes a packet (or conceals one, for an empty `packet`) into
    /// `out`, interleaved in file channel order. Returns samples per channel.
    fn decode(&mut self, packet: &[u8], out: &mut [f32]) -> Result<usize, ()> {
        let ns = self.decs.len();
        let mut rest = packet;
        let mut n = None;
        for s in 0..ns {
            let owned;
            let p: &[u8] = if packet.is_empty() {
                &[]
            } else if s + 1 == ns {
                rest
            } else {
                let (normal, used) = undelimit(rest).ok_or(())?;
                rest = &rest[used..];
                owned = normal;
                &owned
            };
            let size = if p.is_empty() {
                self.last_len
            } else {
                MAX_FRAME
            };
            let got = self.decs[s]
                .decode(p, size, &mut self.scratch[s])
                .map_err(|_| ())?;
            if n.is_some_and(|n| n != got) {
                return Err(());
            }
            n = Some(got);
        }
        let n = n.unwrap_or(0);
        self.last_len = n.max(120);
        let ch = self.sources.len();
        for (c, src) in self.sources.iter().enumerate() {
            for i in 0..n {
                out[i * ch + c] = match *src {
                    None => 0.0,
                    Some((s, side)) => {
                        if s < self.coupled {
                            self.scratch[s][i * 2 + side]
                        } else {
                            self.scratch[s][i]
                        }
                    }
                };
            }
        }
        Ok(n)
    }
}

struct Stream {
    dec: MultiDecoder,
    head: Head,
    order: Vec<usize>,
    gain: f32,
    /// Seen the `OpusTags` packet (always the second one).
    tags_seen: bool,
    /// Samples per channel decoded (or filled in) so far, pre-skip included.
    decoded: u64,
    /// Packets of the current page, decoded once the page is complete so
    /// its granule position can reveal pages lost before it.
    pending: Vec<ogg::Packet>,
    /// Granule position of the first sample; streams cut from a longer
    /// one may not start at zero.
    base: Option<u64>,
}

/// Where decoded audio goes.
struct Sink {
    out: Vec<f32>,
    ch: usize,
    pcm: Vec<f32>,
    damaged: usize,
}

/// Lost audio longer than this is not filled in (a broken granule).
const MAX_GAP: u64 = 48_000 * 600;

impl Stream {
    fn new(head: Head) -> Result<Stream, String> {
        Ok(Stream {
            dec: MultiDecoder::new(&head)?,
            order: head.order(),
            gain: head.gain(),
            head,
            tags_seen: false,
            decoded: 0,
            pending: Vec::new(),
            base: None,
        })
    }

    /// Decodes one packet (concealing it if damaged) and appends it.
    fn decode_one(&mut self, sink: &mut Sink, data: &[u8]) {
        let mut pcm = std::mem::take(&mut sink.pcm);
        let n = match self.dec.decode(data, &mut pcm) {
            Ok(n) => Some(n),
            Err(()) => {
                sink.damaged += 1;
                // Conceal the packet so later audio keeps its timing.
                self.dec.decode(&[], &mut pcm).ok()
            }
        };
        if let Some(n) = n {
            self.emit(sink, Some(&pcm), n);
        }
        sink.pcm = pcm;
    }

    /// Appends `n` frames from `pcm` (or silence when `pcm` is `None`),
    /// dropping the pre-skip.
    fn emit(&mut self, sink: &mut Sink, pcm: Option<&[f32]>, n: usize) {
        let ch = self.head.channels;
        let pre = self.head.pre_skip as u64;
        for i in 0..n {
            if self.decoded + (i as u64) < pre {
                continue;
            }
            for c in 0..sink.ch {
                let v = match pcm {
                    None => 0.0,
                    Some(_) if c >= ch && ch != 1 => 0.0,
                    Some(p) => p[i * ch + if ch == 1 { 0 } else { self.order[c] }],
                };
                sink.out.push(v * self.gain);
            }
        }
        self.decoded += n as u64;
    }

    /// Decodes the buffered page.
    fn flush(&mut self, sink: &mut Sink, gp: Option<u64>) {
        let pkts = std::mem::take(&mut self.pending);
        // Pages lost before this one show as a jump in the granule
        // position: fill the gap with silence so later audio keeps time.
        if let Some(gp) = gp {
            let dur: u64 = pkts
                .iter()
                .map(|p| opus_pure::packet::samples_48k(&p.data).unwrap_or(0) as u64)
                .sum();
            let start = gp.saturating_sub(dur);
            let base = *self.base.get_or_insert(start.saturating_sub(self.decoded));
            let start = start.saturating_sub(base);
            if start > self.decoded && start - self.decoded <= MAX_GAP {
                let gap = (start - self.decoded) as usize;
                self.emit(sink, None, gap);
            }
        }
        for p in &pkts {
            self.decode_one(sink, &p.data);
        }
        // The last page's granule position gives the true length; the
        // final packet may decode a little padding past it.
        if let (Some(gp), true) = (gp, pkts.last().is_some_and(|p| p.last_in_stream())) {
            let pre = self.head.pre_skip as u64;
            let wanted = gp.saturating_sub(self.base.unwrap_or(0) + pre);
            let have = self.decoded.saturating_sub(pre);
            if wanted < have {
                let extra = ((have - wanted) as usize).min(MAX_FRAME * pkts.len());
                let cut = (extra * sink.ch).min(sink.out.len());
                sink.out.truncate(sink.out.len() - cut);
                self.decoded -= (cut / sink.ch) as u64;
            }
        }
    }
}

/// Decodes a whole Ogg Opus file.
pub(crate) fn decode<R: Read + Seek>(r: R) -> Result<Decoded, String> {
    let mut reader = PacketReader::new(r);
    let mut streams: HashMap<u32, Stream> = HashMap::new();
    let mut sink = Sink {
        out: Vec::new(),
        ch: 0,
        pcm: Vec::new(),
        damaged: 0,
    };
    let mut notes = Vec::new();
    let mut read_errors = 0usize;
    let mut chained = 0usize;

    loop {
        let pkt = match reader.read_packet() {
            Ok(Some(p)) => {
                read_errors = 0;
                p
            }
            Ok(None) => break,
            Err(e) => {
                if streams.is_empty() {
                    return Err(format!("Could not read the Opus file: {e}"));
                }
                // A broken page (bad checksum, cut file): count it and
                // carry on with the next good page; give up if nothing
                // more can be read.
                sink.damaged += 1;
                read_errors += 1;
                if read_errors > 32 {
                    notes.push("The end of the file is damaged and could not be read".into());
                    break;
                }
                continue;
            }
        };
        let serial = pkt.stream_serial();
        if pkt.data.starts_with(b"OpusHead") {
            let head = Head::parse(&pkt.data)?;
            let channels = head.channels;
            let stream = Stream::new(head)?;
            if sink.ch == 0 {
                sink.ch = channels;
            } else {
                chained += 1;
                if channels != sink.ch {
                    notes.push(format!(
                        "A later section of the file has {channels} channels instead of {}; it was fitted to {}",
                        sink.ch, sink.ch
                    ));
                }
            }
            let need = MAX_FRAME * channels;
            if sink.pcm.len() < need {
                sink.pcm.resize(need, 0.0);
            }
            if let Some(mut old) = streams.remove(&serial) {
                old.flush(&mut sink, None);
            }
            streams.insert(serial, stream);
            continue;
        }
        let Some(s) = streams.get_mut(&serial) else {
            continue;
        };
        if !s.tags_seen {
            s.tags_seen = true;
            if pkt.data.starts_with(b"OpusTags") {
                continue;
            }
        }
        let page_done = pkt.last_in_page() || pkt.last_in_stream();
        let gp = pkt.absgp_page();
        s.pending.push(pkt);
        if page_done {
            s.flush(&mut sink, (gp != u64::MAX).then_some(gp));
        }
    }
    for s in streams.values_mut() {
        if !s.pending.is_empty() {
            s.flush(&mut sink, None);
        }
    }
    if sink.ch == 0 {
        return Err("The file holds no Opus audio".into());
    }
    if chained > 0 {
        notes.push(format!(
            "The file has {} chained sections; they were joined",
            chained + 1
        ));
    }
    Ok(Decoded {
        buf: AudioBuffer::from_interleaved(RATE, sink.ch, &sink.out),
        damaged: sink.damaged,
        notes,
    })
}

/// Decodes Opus packets from another container (Matroska/WebM), given
/// the `OpusHead` from the track's codec private data.
pub(crate) fn decode_packets(
    head: &[u8],
    mut next: impl FnMut() -> Option<Vec<u8>>,
) -> Result<Decoded, String> {
    let head = Head::parse(head)?;
    let ch = head.channels;
    let mut s = Stream::new(head)?;
    let mut sink = Sink {
        out: Vec::new(),
        ch,
        pcm: vec![0.0; MAX_FRAME * ch],
        damaged: 0,
    };
    while let Some(p) = next() {
        s.decode_one(&mut sink, &p);
    }
    Ok(Decoded {
        buf: AudioBuffer::from_interleaved(RATE, ch, &sink.out),
        damaged: sink.damaged,
        notes: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn media(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/media")
            .join(name)
    }

    fn open(name: &str) -> Decoded {
        decode(std::fs::File::open(media(name)).unwrap()).unwrap()
    }

    #[test]
    fn sniffs_opus_only() {
        let mut f = std::fs::File::open(media("stereo.opus")).unwrap();
        assert!(sniff(&mut f));
        let mut c = std::io::Cursor::new(b"RIFF....WAVEfmt ".to_vec());
        assert!(!sniff(&mut c));
    }

    #[test]
    fn stereo_length_and_level() {
        let d = open("stereo.opus");
        assert_eq!((d.buf.rate, d.buf.num_channels()), (48_000, 2));
        // One second exactly: pre-skip removed, end padding trimmed.
        assert_eq!(d.buf.frames(), 48_000);
        assert_eq!(d.damaged, 0);
        // ffmpeg's sine source is at 1/8 full scale.
        let p = d.buf.peak();
        assert!(p > 0.08 && p < 0.2, "peak {p}");
    }

    #[test]
    fn mono_from_44k_source_is_48k() {
        let d = open("mono.opus");
        assert_eq!((d.buf.rate, d.buf.num_channels()), (48_000, 1));
        assert!((d.buf.frames() as i64 - 24_000).abs() < 50);
        assert!(d.buf.peak() > 0.05);
    }

    #[test]
    fn surround_family_one_matches_reference_decoder() {
        let d = open("surround51.opus");
        assert_eq!(d.buf.num_channels(), 6);
        assert_eq!(d.buf.frames(), 24_000);
        for c in &d.buf.channels {
            assert!(c.iter().fold(0f32, |m, s| m.max(s.abs())) > 0.05);
        }
        // Our own stream splitting must agree with the crate's standard
        // surround decoder (which uses the same Vorbis layout).
        let f = std::fs::File::open(media("surround51.opus")).unwrap();
        let mut r = PacketReader::new(f);
        let head = Head::parse(&r.read_packet().unwrap().unwrap().data).unwrap();
        r.read_packet().unwrap();
        let mut ours = MultiDecoder::new(&head).unwrap();
        let mut theirs = opus_pure::OpusMSDecoder::new(48_000, 6, 1).unwrap();
        let (mut a, mut b) = (vec![0f32; MAX_FRAME * 6], vec![0f32; MAX_FRAME * 6]);
        let mut packets = 0;
        while let Ok(Some(p)) = r.read_packet() {
            let n = ours.decode(&p.data, &mut a).unwrap();
            let m = theirs.decode(&p.data, MAX_FRAME, &mut b).unwrap();
            assert_eq!(n, m);
            assert_eq!(a[..n * 6], b[..n * 6]);
            packets += 1;
        }
        assert!(packets > 10);
    }

    #[test]
    fn chained_streams_join() {
        let d = open("chained.opus");
        assert_eq!(d.buf.num_channels(), 2);
        assert_eq!(d.buf.frames(), 48_000 + 24_000);
        assert!(d.notes.iter().any(|n| n.contains("chained")));
    }

    #[test]
    fn damaged_page_is_counted_and_gap_keeps_time() {
        // 100 ms pages; corrupt the payload of the one at byte 3418.
        let mut bytes = std::fs::read(media("paged.opus")).unwrap();
        for b in &mut bytes[3418 + 40..3418 + 80] {
            *b ^= 0x5A;
        }
        let d = decode(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(d.damaged, 1);
        // The lost 100 ms is filled with silence, so the length holds.
        assert_eq!(d.buf.frames(), 48_000);
        let l = &d.buf.channels[0];
        assert!(l[19_500..23_500].iter().all(|&s| s == 0.0));
        assert!(l[25_000..30_000].iter().any(|&s| s.abs() > 0.05));
    }

    #[test]
    fn undelimit_all_codes() {
        // Code 0: TOC, length 3, frame.
        assert_eq!(
            undelimit(&[0x00, 3, 1, 2, 3, 9, 9]),
            Some((vec![0x00, 1, 2, 3], 5))
        );
        // Code 1: two frames of 2.
        assert_eq!(
            undelimit(&[0x01, 2, 1, 2, 3, 4, 9]),
            Some((vec![0x01, 1, 2, 3, 4], 6))
        );
        // Code 2: first length 1, second (self-delimiting) length 2.
        assert_eq!(
            undelimit(&[0x02, 1, 2, 7, 8, 9, 0]),
            Some((vec![0x02, 1, 7, 8, 9], 6))
        );
        // Code 3 CBR, 2 frames of 1, with 1 byte of padding.
        assert_eq!(
            undelimit(&[0x03, 0x42, 1, 1, 5, 6, 0, 9]),
            Some((vec![0x03, 0x42, 1, 5, 6, 0], 7))
        );
        // Code 3 VBR, 2 frames: lengths 1 then (self-delimiting) 2.
        assert_eq!(
            undelimit(&[0x03, 0x82, 1, 2, 5, 6, 7, 9]),
            Some((vec![0x03, 0x82, 1, 5, 6, 7], 7))
        );
        // Two-byte length: 252 + 4*1 = 256.
        let mut d = vec![0x00, 252, 1];
        d.extend(std::iter::repeat_n(7u8, 256));
        let (p, used) = undelimit(&d).unwrap();
        assert_eq!((p.len(), used), (257, 259));
        assert_eq!(undelimit(&[0x00, 10, 1]), None);
    }

    #[test]
    fn head_parsing_gain_and_family() {
        let mut h = b"OpusHead".to_vec();
        h.extend_from_slice(&[1, 2, 0x38, 0x01, 0x80, 0xBB, 0, 0]);
        h.extend_from_slice(&(6i16 * 256).to_le_bytes());
        h.push(0);
        let head = Head::parse(&h).unwrap();
        assert_eq!(
            (head.channels, head.pre_skip, head.streams, head.coupled),
            (2, 312, 1, 1)
        );
        assert!((head.gain() - 1.995).abs() < 0.01);

        let mut s = b"OpusHead".to_vec();
        s.extend_from_slice(&[1, 6, 0, 0, 0x80, 0xBB, 0, 0, 0, 0, 1, 4, 2]);
        s.extend_from_slice(&[0, 4, 1, 2, 3, 5]);
        let head = Head::parse(&s).unwrap();
        assert_eq!((head.streams, head.coupled), (4, 2));
        assert_eq!(head.order(), vec![0, 2, 1, 5, 3, 4]);
        assert_eq!(
            head.sources(),
            vec![
                Some((0, 0)),
                Some((2, 0)),
                Some((0, 1)),
                Some((1, 0)),
                Some((1, 1)),
                Some((3, 0))
            ]
        );

        let mut a = s.clone();
        a[18] = 3;
        assert!(Head::parse(&a).unwrap_err().contains("Ambisonic"));
    }
}
