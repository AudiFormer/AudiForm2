// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Markers, chapters and tags read when a file is opened.
//!
//! * WAV/BWF/RF64: `cue ` points with `adtl` labels, notes and ranges,
//!   and the `bext` description, originator and time reference
//!   (our own chunk reader in [`crate::riff`]);
//! * MP3: ID3v2 `CHAP` chapters (`id3` crate);
//! * FLAC: `CHAPTERnnn` / `CHAPTERnnnNAME` Vorbis comments (`metaflac`);
//! * Ogg Vorbis / Opus / FLAC-in-Ogg: the same comments, read from the
//!   stream's comment packet;
//! * M4A/MP4: Nero `chpl` chapters;
//! * Tags (title, artist, album, year, genre, comment, cover art) for
//!   every format `lofty` reads.
//!
//! Everything here is best-effort: an unreadable tag never stops a file
//! from opening.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// What a marker read from a file stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    /// A point (WAV cue).
    Cue,
    /// A span with a length (WAV `ltxt` region).
    Range,
    /// A chapter (ID3 CHAP, Vorbis CHAPTERnnn, MP4 chapters).
    Chapter,
}

/// A marker, range or chapter found in a file, times in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct FileMarker {
    pub name: String,
    pub time: f64,
    /// 0 for a point.
    pub length: f64,
    pub kind: MarkerKind,
    /// A WAV `note` or `ltxt` text, if any.
    pub note: Option<String>,
}

/// The descriptive tags of a file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub comment: Option<String>,
    /// The file carries embedded cover art.
    pub cover_art: bool,
    /// Further fields worth showing (BWF description, originator, ...).
    pub extra: Vec<(String, String)>,
}

impl Tags {
    pub fn is_empty(&self) -> bool {
        self.rows().is_empty()
    }

    /// The tags as (label, value) rows for display, empty fields left out.
    pub fn rows(&self) -> Vec<(String, String)> {
        let mut rows = Vec::new();
        for (label, v) in [
            ("Title", &self.title),
            ("Artist", &self.artist),
            ("Album", &self.album),
            ("Year", &self.year),
            ("Genre", &self.genre),
            ("Comment", &self.comment),
        ] {
            if let Some(v) = v.as_deref().filter(|v| !v.trim().is_empty()) {
                rows.push((label.to_string(), v.to_string()));
            }
        }
        if self.cover_art {
            rows.push(("Cover art".into(), "Yes".into()));
        }
        rows.extend(self.extra.iter().cloned());
        rows
    }
}

/// Markers, chapters and tags of one file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileInfo {
    pub tags: Tags,
    pub markers: Vec<FileMarker>,
}

/// Reads what it can; `rate` is the decoded sample rate, used when the
/// file's own header gives none.
pub fn read_info(path: &Path, rate: u32) -> FileInfo {
    let mut info = FileInfo {
        tags: read_tags(path),
        markers: Vec::new(),
    };
    let Ok(f) = File::open(path) else {
        return info;
    };
    let mut r = BufReader::new(f);
    let mut magic = [0u8; 12];
    let n = r.read(&mut magic).unwrap_or(0);
    let _ = r.seek(SeekFrom::Start(0));
    let magic = &magic[..n];
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if magic.starts_with(b"RIFF") || magic.starts_with(b"RF64") || magic.starts_with(b"BW64") {
        if let Some(w) = crate::riff::read(&mut r) {
            wav_info(&mut info, &w, if w.rate > 0 { w.rate } else { rate });
        }
    } else if magic.starts_with(b"fLaC") {
        if let Ok(t) = metaflac::Tag::read_from_path(path) {
            if let Some(vc) = t.vorbis_comments() {
                let pairs: Vec<(String, String)> = vc
                    .comments
                    .iter()
                    .flat_map(|(k, vs)| vs.iter().map(move |v| (k.clone(), v.clone())))
                    .collect();
                info.markers = vorbis_chapters(&pairs);
            }
        }
    } else if magic.starts_with(b"OggS") {
        if let Some(pairs) = ogg_comments(&mut r) {
            info.markers = vorbis_chapters(&pairs);
        }
    } else if magic.starts_with(b"ID3") || ext == "mp3" {
        if let Ok(tag) = id3::Tag::read_from_path(path) {
            info.markers = id3_chapters(&tag);
        }
    } else if n >= 8 && &magic[4..8] == b"ftyp" {
        info.markers = mp4_chapters(&mut r);
    }
    info
}

fn wav_info(info: &mut FileInfo, w: &crate::riff::WavMeta, rate: u32) {
    let rate = rate.max(1) as f64;
    for (i, c) in w.cues.iter().enumerate() {
        let name = c
            .label
            .clone()
            .or_else(|| c.text.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("Marker {}", i + 1));
        let note = match (&c.note, &c.text) {
            (Some(n), _) if !n.is_empty() => Some(n.clone()),
            (_, Some(t)) if c.label.is_some() && !t.is_empty() => Some(t.clone()),
            _ => None,
        };
        info.markers.push(FileMarker {
            name,
            time: c.offset as f64 / rate,
            length: c.length as f64 / rate,
            kind: if c.length > 0 {
                MarkerKind::Range
            } else {
                MarkerKind::Cue
            },
            note,
        });
    }
    if let Some(b) = &w.bext {
        let t = &mut info.tags;
        if !b.description.is_empty() {
            if t.comment.is_none() {
                t.comment = Some(b.description.clone());
            }
            t.extra
                .push(("BWF description".into(), b.description.clone()));
        }
        if !b.originator.is_empty() {
            t.extra.push(("Originator".into(), b.originator.clone()));
        }
        if !b.originator_reference.is_empty() {
            t.extra
                .push(("Originator ref.".into(), b.originator_reference.clone()));
        }
        if !b.origination_date.is_empty() {
            let when = format!("{} {}", b.origination_date, b.origination_time);
            t.extra.push(("Originated".into(), when.trim().to_string()));
        }
        t.extra.push((
            "Time reference".into(),
            format!(
                "{} ({} samples)",
                clock(b.time_reference as f64 / rate),
                b.time_reference
            ),
        ));
    }
}

/// `h:mm:ss.mmm`.
fn clock(secs: f64) -> String {
    let ms = (secs * 1000.0).round() as u64;
    format!(
        "{}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

fn read_tags(path: &Path) -> Tags {
    use lofty::file::TaggedFileExt;
    use lofty::prelude::{Accessor, ItemKey};
    let mut tags = Tags::default();
    let Ok(f) = lofty::read_from_path(path) else {
        return tags;
    };
    let Some(t) = f.primary_tag().or_else(|| f.first_tag()) else {
        return tags;
    };
    let s = |v: Option<std::borrow::Cow<str>>| {
        v.map(|c| c.trim().to_string()).filter(|c| !c.is_empty())
    };
    tags.title = s(t.title());
    tags.artist = s(t.artist());
    tags.album = s(t.album());
    tags.genre = s(t.genre());
    tags.comment = s(t.comment());
    tags.year = t.date().map(|d| d.year.to_string()).or_else(|| {
        t.get_string(ItemKey::Year)
            .or_else(|| t.get_string(ItemKey::RecordingDate))
            .map(str::to_string)
    });
    tags.cover_art = f.tags().iter().any(|t| !t.pictures().is_empty());
    tags
}

/// Parses `H:MM:SS.mmm` (hours optional, fraction optional) to seconds.
fn parse_clock(s: &str) -> Option<f64> {
    let mut total = 0.0;
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    for p in &parts {
        let v: f64 = p.trim().parse().ok()?;
        if !v.is_finite() || v < 0.0 {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}

/// Chapters from Vorbis comments: `CHAPTER001=00:01:30.000` with
/// `CHAPTER001NAME=Title`, keys in any case.
pub(crate) fn vorbis_chapters(pairs: &[(String, String)]) -> Vec<FileMarker> {
    let mut starts: std::collections::BTreeMap<u32, f64> = Default::default();
    let mut names: std::collections::HashMap<u32, String> = Default::default();
    for (k, v) in pairs {
        let k = k.to_ascii_uppercase();
        let Some(rest) = k.strip_prefix("CHAPTER") else {
            continue;
        };
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let Ok(num) = digits.parse::<u32>() else {
            continue;
        };
        match &rest[digits.len()..] {
            "" => {
                if let Some(t) = parse_clock(v) {
                    starts.insert(num, t);
                }
            }
            "NAME" => {
                names.insert(num, v.clone());
            }
            _ => {}
        }
    }
    let mut out: Vec<FileMarker> = starts
        .into_iter()
        .map(|(num, time)| FileMarker {
            name: names
                .remove(&num)
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("Chapter {num}")),
            time,
            length: 0.0,
            kind: MarkerKind::Chapter,
            note: None,
        })
        .collect();
    out.sort_by(|a, b| a.time.total_cmp(&b.time));
    out
}

/// Comment pairs from the comment header of the first logical stream of
/// an Ogg file (Vorbis, Opus or FLAC-in-Ogg).
fn ogg_comments<R: Read + Seek>(r: &mut R) -> Option<Vec<(String, String)>> {
    let mut pr = ogg::PacketReader::new(r);
    let mut serial = None;
    for _ in 0..8 {
        let p = pr.read_packet().ok()??;
        if *serial.get_or_insert(p.stream_serial()) != p.stream_serial() {
            continue;
        }
        let d = &p.data;
        let body = if let Some(b) = d.strip_prefix(b"OpusTags") {
            b
        } else if let Some(b) = d.strip_prefix(b"\x03vorbis") {
            b
        } else if !d.is_empty() && d[0] & 0x7F == 4 && d.len() >= 4 {
            // FLAC-in-Ogg: a metadata block header, type 4 = comments.
            &d[4..]
        } else {
            continue;
        };
        return parse_vorbis_comment(body);
    }
    None
}

/// The Vorbis comment structure: vendor string, then `KEY=value` items,
/// all lengths 32-bit little-endian.
fn parse_vorbis_comment(b: &[u8]) -> Option<Vec<(String, String)>> {
    let rd = |at: usize| -> Option<usize> {
        b.get(at..at + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
    };
    let vendor = rd(0)?;
    let mut at = 4usize.checked_add(vendor)?;
    let n = rd(at)?;
    at += 4;
    let mut out = Vec::new();
    for _ in 0..n.min(100_000) {
        let len = rd(at)?;
        at += 4;
        let item = b.get(at..at.checked_add(len)?)?;
        at += len;
        let s = String::from_utf8_lossy(item);
        if let Some((k, v)) = s.split_once('=') {
            out.push((k.to_string(), v.to_string()));
        }
    }
    Some(out)
}

fn id3_chapters(tag: &id3::Tag) -> Vec<FileMarker> {
    let mut out: Vec<FileMarker> = tag
        .chapters()
        .enumerate()
        .map(|(i, c)| {
            let title = c
                .frames
                .iter()
                .find(|f| f.id() == "TIT2")
                .and_then(|f| f.content().text())
                .map(str::to_string)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("Chapter {}", i + 1));
            let start = c.start_time as f64 / 1000.0;
            let end = c.end_time as f64 / 1000.0;
            FileMarker {
                name: title,
                time: start,
                length: (end - start).max(0.0),
                kind: MarkerKind::Chapter,
                note: None,
            }
        })
        .collect();
    out.sort_by(|a, b| a.time.total_cmp(&b.time));
    out
}

/// Nero-style chapters (`moov/udta/chpl`), which ffmpeg and many
/// podcast tools write. QuickTime text-track chapters are not read.
fn mp4_chapters<R: Read + Seek>(r: &mut R) -> Vec<FileMarker> {
    let end = r.seek(SeekFrom::End(0)).unwrap_or(0);
    let Some((s, e)) = find_box(r, 0, end, b"moov") else {
        return Vec::new();
    };
    let Some((s, e)) = find_box(r, s, e, b"udta") else {
        return Vec::new();
    };
    let Some((s, e)) = find_box(r, s, e, b"chpl") else {
        return Vec::new();
    };
    let len = (e - s) as usize;
    if len > 1 << 20 || r.seek(SeekFrom::Start(s)).is_err() {
        return Vec::new();
    }
    let mut b = vec![0u8; len];
    if r.read_exact(&mut b).is_err() {
        return Vec::new();
    }
    parse_chpl(&b)
}

/// Finds the first box called `name` among the boxes in `[start, end)`;
/// returns the byte range of its contents.
fn find_box<R: Read + Seek>(r: &mut R, start: u64, end: u64, name: &[u8; 4]) -> Option<(u64, u64)> {
    let mut pos = start;
    while pos + 8 <= end {
        r.seek(SeekFrom::Start(pos)).ok()?;
        let mut h = [0u8; 8];
        r.read_exact(&mut h).ok()?;
        let mut size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let mut head = 8;
        if size == 1 {
            let mut big = [0u8; 8];
            r.read_exact(&mut big).ok()?;
            size = u64::from_be_bytes(big);
            head = 16;
        } else if size == 0 {
            size = end - pos;
        }
        if size < head {
            return None;
        }
        if &h[4..8] == name {
            return Some((pos + head, (pos + size).min(end)));
        }
        pos += size;
    }
    None
}

/// `chpl` contents: version, 24-bit flags, (4 reserved bytes in version
/// 1), a chapter count, then per chapter a 64-bit start in 100 ns units
/// and a length-prefixed title.
fn parse_chpl(b: &[u8]) -> Vec<FileMarker> {
    let mut out = Vec::new();
    if b.len() < 5 {
        return out;
    }
    let mut at = if b[0] == 0 { 4 } else { 8 };
    let Some(&n) = b.get(at) else {
        return out;
    };
    at += 1;
    for i in 0..n as usize {
        let Some(t) = b.get(at..at + 8) else { break };
        let start = u64::from_be_bytes(t.try_into().unwrap());
        let Some(&len) = b.get(at + 8) else { break };
        let Some(title) = b.get(at + 9..at + 9 + len as usize) else {
            break;
        };
        at += 9 + len as usize;
        let name = String::from_utf8_lossy(title).trim().to_string();
        out.push(FileMarker {
            name: if name.is_empty() {
                format!("Chapter {}", i + 1)
            } else {
                name
            },
            time: start as f64 / 1e7,
            length: 0.0,
            kind: MarkerKind::Chapter,
            note: None,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("af27-meta-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn wav_cues_ranges_and_bext() {
        let p = tmp("cues.wav");
        std::fs::write(&p, crate::riff::tests::test_wav()).unwrap();
        let info = read_info(&p, 0);
        assert_eq!(info.markers.len(), 2);
        let m = &info.markers[0];
        assert_eq!(
            (m.name.as_str(), m.time, m.kind),
            ("Verse", 0.25, MarkerKind::Cue)
        );
        let r = &info.markers[1];
        assert_eq!(r.name, "Chorus");
        assert_eq!((r.time, r.length, r.kind), (0.5, 0.2, MarkerKind::Range));
        assert_eq!(r.note.as_deref(), Some("Loud bit"));
        let rows = info.tags.rows();
        assert!(rows.contains(&("Originator".into(), "Zoomer".into())));
        assert!(rows
            .iter()
            .any(|(k, v)| k == "Time reference" && v.starts_with("1:00:00.000")));
        assert_eq!(info.tags.comment.as_deref(), Some("Field notes"));
        // The file still decodes.
        assert_eq!(crate::decode_file(&p).unwrap().frames(), 8000);
    }

    #[test]
    fn mp3_id3_chapters() {
        use id3::frame::{Chapter, Content, Frame, TableOfContents};
        use id3::{TagLike, Version};
        let p = tmp("chap.mp3");
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/media/tiny.mp3");
        std::fs::copy(&src, &p).unwrap();
        let mut tag = id3::Tag::new();
        tag.set_title("Show");
        for (i, (a, b, name)) in [(0u32, 30_000u32, "Intro"), (30_000, 95_500, "Interview")]
            .into_iter()
            .enumerate()
        {
            tag.add_frame(Chapter {
                element_id: format!("ch{i}"),
                start_time: a,
                end_time: b,
                start_offset: u32::MAX,
                end_offset: u32::MAX,
                frames: vec![Frame::text("TIT2", name)],
            });
        }
        tag.add_frame(Frame::with_content(
            "CTOC",
            Content::TableOfContents(TableOfContents {
                element_id: "toc".into(),
                top_level: true,
                ordered: true,
                elements: vec!["ch0".into(), "ch1".into()],
                frames: vec![],
            }),
        ));
        tag.write_to_path(&p, Version::Id3v24).unwrap();
        let info = read_info(&p, 44_100);
        assert_eq!(info.markers.len(), 2);
        assert_eq!(info.markers[1].name, "Interview");
        assert_eq!(info.markers[1].time, 30.0);
        assert!((info.markers[1].length - 65.5).abs() < 1e-9);
        assert_eq!(info.markers[1].kind, MarkerKind::Chapter);
        assert_eq!(info.tags.title.as_deref(), Some("Show"));
        assert!(crate::decode_file(&p).unwrap().frames() > 5000);
    }

    #[test]
    fn flac_vorbis_chapters_and_tags() {
        // A tiny real FLAC file, then chapter comments added by metaflac.
        let p = tmp("chap.flac");
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/media/tiny.flac");
        std::fs::copy(&src, &p).unwrap();
        let mut t = metaflac::Tag::read_from_path(&p).unwrap();
        let vc = t.vorbis_comments_mut();
        vc.set("TITLE", vec!["Album Side"]);
        vc.set("ARTIST", vec!["Band"]);
        vc.set("DATE", vec!["1999"]);
        vc.set("CHAPTER001", vec!["00:00:00.000"]);
        vc.set("CHAPTER001NAME", vec!["Opening"]);
        vc.set("CHAPTER002", vec!["00:00:00.250"]);
        vc.set("chapter002name", vec!["Second"]);
        vc.set("CHAPTER003", vec!["1:02:03.5"]);
        t.save().unwrap();
        let info = read_info(&p, 0);
        let names: Vec<&str> = info.markers.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["Opening", "Second", "Chapter 3"]);
        assert_eq!(info.markers[1].time, 0.25);
        assert_eq!(info.markers[2].time, 3723.5);
        assert_eq!(info.tags.title.as_deref(), Some("Album Side"));
        assert_eq!(info.tags.artist.as_deref(), Some("Band"));
        assert_eq!(info.tags.year.as_deref(), Some("1999"));
        assert!(crate::decode_file(&p).unwrap().frames() > 0);
    }

    #[test]
    fn opus_comment_chapters_and_tags() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/media/chapters.opus");
        let info = read_info(&p, 48_000);
        let names: Vec<&str> = info.markers.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["Intro", "Main"]);
        assert!((info.markers[1].time - 0.4).abs() < 1e-9);
        assert_eq!(info.tags.title.as_deref(), Some("Chaptered"));
        assert_eq!(info.tags.artist.as_deref(), Some("Tester"));
    }

    #[test]
    fn mp4_nero_chapters() {
        fn bx(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(name);
            v.extend_from_slice(body);
            v
        }
        let mut chpl = vec![1, 0, 0, 0, 0, 0, 0, 0, 2];
        for (t, name) in [(0u64, "One"), (125_000_000u64, "Two")] {
            chpl.extend_from_slice(&t.to_be_bytes());
            chpl.push(name.len() as u8);
            chpl.extend_from_slice(name.as_bytes());
        }
        let mut file = bx(b"ftyp", b"M4A \0\0\0\0");
        file.extend(bx(
            b"moov",
            &[bx(b"mvhd", &[0; 20]), bx(b"udta", &bx(b"chpl", &chpl))].concat(),
        ));
        let m = mp4_chapters(&mut std::io::Cursor::new(file));
        assert_eq!(m.len(), 2);
        assert_eq!(m[1].name, "Two");
        assert_eq!(m[1].time, 12.5);
    }

    #[test]
    fn clock_parsing() {
        assert_eq!(parse_clock("00:01:30.5"), Some(90.5));
        assert_eq!(parse_clock("2:00"), Some(120.0));
        assert_eq!(parse_clock("x"), None);
        assert_eq!(clock(3661.25), "1:01:01.250");
    }
}
