// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! `.aup3` projects (an open-source editor's SQLite project files):
//! imported as a new session. The project file is opened read-only. Its
//! audio lives in sample blocks inside the database, so each distinct
//! recording is written out as a 32-bit float WAV in a folder beside the
//! project (`<project name> audio`), and the clips point at those.
//!
//! The layout is a binary tree in the `project` table: `dict` names the
//! element and attribute ids, `doc` is a stream of tagged records. Both
//! start with a record giving the width of their text characters.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use af27_core::automation::{self, Lane, LaneTarget};
use af27_core::session::{Clip, Marker, MediaItem, Session, StretchKind, StretchMode};
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::Imported;

#[derive(Debug, Clone, PartialEq)]
enum Val {
    Text(String),
    Num(f64),
}

/// An element of the project tree: a name, attributes and children.
#[derive(Debug, Default, Clone)]
pub struct Elem {
    pub name: String,
    attrs: Vec<(String, Val)>,
    pub children: Vec<Elem>,
}

impl Elem {
    fn num(&self, k: &str) -> Option<f64> {
        self.attrs
            .iter()
            .find(|(n, _)| n == k)
            .and_then(|(_, v)| match v {
                Val::Num(x) => Some(*x),
                Val::Text(t) => t.trim().parse().ok(),
            })
    }
    fn text(&self, k: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == k)
            .and_then(|(_, v)| match v {
                Val::Text(t) => Some(t.as_str()),
                Val::Num(_) => None,
            })
    }
    fn kids<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Elem> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
}

struct Bytes<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Bytes<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.b.len());
        let Some(end) = end else {
            return Err("the project data ends early".into());
        };
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn text(&mut self, len: usize, width: u8) -> Result<String, String> {
        let raw = self.take(len)?;
        Ok(match width {
            1 => String::from_utf8_lossy(raw).into_owned(),
            4 => raw
                .as_chunks::<4>()
                .0
                .iter()
                .filter_map(|c| char::from_u32(u32::from_le_bytes(*c)))
                .collect(),
            _ => String::from_utf16_lossy(
                &raw.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .collect::<Vec<_>>(),
            ),
        })
    }
}

// Record kinds in `dict` and `doc`.
const CHAR_SIZE: u8 = 0;
const START: u8 = 1;
const END: u8 = 2;
const STRING: u8 = 3;
const INT: u8 = 4;
const BOOL: u8 = 5;
const LONG: u8 = 6;
const LONG_LONG: u8 = 7;
const SIZE: u8 = 8;
const FLOAT: u8 = 9;
const DOUBLE: u8 = 10;
const DATA: u8 = 11;
const RAW: u8 = 12;
const PUSH: u8 = 13;
const POP: u8 = 14;
const NAME: u8 = 15;

/// The id-to-name table. Starts with its character width (bytes per
/// text unit; 2 for UTF-16), then names: id, byte length, text.
pub fn names(dict: &[u8]) -> Result<HashMap<u16, String>, String> {
    let mut r = Bytes { b: dict, at: 0 };
    let mut width = 2u8;
    let mut out = HashMap::new();
    while r.at < dict.len() {
        match r.u8()? {
            CHAR_SIZE => width = r.u8()?,
            NAME => {
                let id = r.u16()?;
                let len = r.u16()? as usize;
                out.insert(id, r.text(len, width)?);
            }
            k => {
                return Err(format!(
                    "the project's name table has an unknown record ({k})"
                ))
            }
        }
    }
    Ok(out)
}

/// The element tree from `doc`.
pub fn tree(doc: &[u8], names: &HashMap<u16, String>) -> Result<Elem, String> {
    let mut r = Bytes { b: doc, at: 0 };
    let mut width = 2u8;
    let mut stack: Vec<Elem> = Vec::new();
    let mut root: Option<Elem> = None;
    let name = |id: u16| names.get(&id).cloned().unwrap_or_default();
    while r.at < doc.len() {
        let kind = r.u8()?;
        let attr = |stack: &mut Vec<Elem>, id: u16, v: Val| {
            if let Some(top) = stack.last_mut() {
                top.attrs.push((name(id), v));
            }
        };
        match kind {
            CHAR_SIZE => width = r.u8()?,
            START => {
                let id = r.u16()?;
                stack.push(Elem {
                    name: name(id),
                    ..Default::default()
                });
            }
            END => {
                r.u16()?;
                let Some(done) = stack.pop() else {
                    return Err("the project data closes more than it opens".into());
                };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => root = root.or(Some(done)),
                }
            }
            STRING => {
                let id = r.u16()?;
                let len = r.u32()? as usize;
                let t = r.text(len, width)?;
                attr(&mut stack, id, Val::Text(t));
            }
            INT | LONG => {
                let id = r.u16()?;
                let v = i32::from_le_bytes(r.take(4)?.try_into().unwrap());
                attr(&mut stack, id, Val::Num(v as f64));
            }
            BOOL => {
                let id = r.u16()?;
                let v = r.u8()?;
                attr(&mut stack, id, Val::Num(v as f64));
            }
            LONG_LONG => {
                let id = r.u16()?;
                let v = i64::from_le_bytes(r.take(8)?.try_into().unwrap());
                attr(&mut stack, id, Val::Num(v as f64));
            }
            SIZE => {
                let id = r.u16()?;
                let v = r.u32()?;
                attr(&mut stack, id, Val::Num(v as f64));
            }
            FLOAT => {
                let id = r.u16()?;
                let v = f32::from_le_bytes(r.take(4)?.try_into().unwrap());
                r.take(4)?; // digits
                attr(&mut stack, id, Val::Num(v as f64));
            }
            DOUBLE => {
                let id = r.u16()?;
                let v = f64::from_le_bytes(r.take(8)?.try_into().unwrap());
                r.take(4)?; // digits
                attr(&mut stack, id, Val::Num(v));
            }
            DATA | RAW => {
                let len = r.u32()? as usize;
                r.take(len)?;
            }
            PUSH | POP => {}
            k => return Err(format!("the project data has an unknown record ({k})")),
        }
    }
    let root = root.ok_or("the project data is empty")?;
    if root.name != "project" {
        return Err("the project data does not start as expected".into());
    }
    Ok(root)
}

/// How a block's samples are stored.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Format {
    F32,
    I16,
    I24,
}

impl Format {
    fn of(code: i64) -> Option<Format> {
        match code {
            0x0004_000F => Some(Format::F32),
            0x0002_0001 => Some(Format::I16),
            0x0004_0001 => Some(Format::I24),
            _ => None,
        }
    }
    fn decode(self, b: &[u8], out: &mut VecDeque<f32>) {
        match self {
            Format::F32 => out.extend(b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c))),
            Format::I16 => out.extend(
                b.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| i16::from_le_bytes(*c) as f32 / 32_768.0),
            ),
            Format::I24 => out.extend(
                b.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| i32::from_le_bytes(*c) as f32 / 8_388_608.0),
            ),
        }
    }
}

/// One channel's audio: its blocks as (block id, sample count).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Channel {
    blocks: Vec<(i64, u64)>,
}

impl Channel {
    fn len(&self) -> u64 {
        self.blocks.iter().map(|b| b.1).sum()
    }
    fn starts_with(&self, other: &Channel) -> bool {
        self.blocks.len() >= other.blocks.len()
            && self.blocks[..other.blocks.len()]
                .iter()
                .zip(&other.blocks)
                .all(|(a, b)| a.0 == b.0)
    }
}

fn channel_of(seq: &Elem) -> Channel {
    let total = seq.num("numsamples").unwrap_or(0.0).max(0.0) as u64;
    let starts: Vec<(u64, i64)> = seq
        .kids("waveblock")
        .map(|b| {
            (
                b.num("start").unwrap_or(0.0).max(0.0) as u64,
                b.num("blockid").unwrap_or(0.0) as i64,
            )
        })
        .collect();
    let blocks = starts
        .iter()
        .enumerate()
        .map(|(i, &(start, id))| {
            let next = starts.get(i + 1).map_or(total, |n| n.0);
            (id, next.saturating_sub(start))
        })
        .collect();
    Channel { blocks }
}

fn read_block(
    stmt: &mut rusqlite::CachedStatement,
    id: i64,
    len: u64,
    buf: &mut VecDeque<f32>,
) -> Result<(), String> {
    let row: Option<(i64, Vec<u8>)> = stmt
        .query_row([id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map_err(|e| e.to_string())?;
    let before = buf.len();
    if let Some((code, bytes)) = row {
        let f = Format::of(code).ok_or("the project uses an unsupported sample format")?;
        f.decode(&bytes, buf);
    }
    // A block without a row (silence) or a short one is padded.
    let want = before + len as usize;
    buf.resize(want.max(buf.len()), 0.0);
    buf.truncate(want);
    Ok(())
}

/// Writes one recording (one or two channels) as a 32-bit float WAV.
fn write_source(
    conn: &Connection,
    chans: &[Channel],
    rate: u32,
    path: &Path,
) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: chans.len() as u16,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    let frames = chans.iter().map(Channel::len).max().unwrap_or(0);
    let mut stmt = conn
        .prepare_cached("SELECT sampleformat, samples FROM sampleblocks WHERE blockid = ?1")
        .map_err(|e| e.to_string())?;
    type Pull = (std::vec::IntoIter<(i64, u64)>, VecDeque<f32>);
    let mut readers: Vec<Pull> = chans
        .iter()
        .map(|c| (c.blocks.clone().into_iter(), VecDeque::new()))
        .collect();
    const CHUNK: usize = 65_536;
    let mut done = 0u64;
    while done < frames {
        let n = CHUNK.min((frames - done) as usize);
        for (blocks, buf) in &mut readers {
            while buf.len() < n {
                match blocks.next() {
                    Some((id, len)) => read_block(&mut stmt, id, len, buf)?,
                    None => buf.resize(n, 0.0),
                }
            }
        }
        for _ in 0..n {
            for (_, buf) in &mut readers {
                w.write_sample(buf.pop_front().unwrap_or(0.0))
                    .map_err(|e| e.to_string())?;
            }
        }
        done += n as u64;
    }
    w.finalize().map_err(|e| e.to_string())
}

/// Where an import writes the project's audio: `<folder>/<name> audio`.
pub fn audio_folder(project: &Path) -> PathBuf {
    let stem = project
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Project".into());
    project
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!("{stem} audio"))
}

fn to_db(g: f64) -> f32 {
    if g <= 1e-5 {
        -96.0
    } else {
        ((20.0 * g.log10()) * 10.0).round() as f32 / 10.0
    }
}

/// A clip's gain envelope (seconds, linear gain, moving straight in
/// gain) as volume points in dB, with points between those far apart in
/// level so the line in dB follows the straight line in gain.
fn volume_line(env: &[(f64, f64)]) -> Vec<automation::LanePoint> {
    let pt = |t: f64, g: f64| automation::LanePoint {
        time: t,
        value: to_db(g) as f64,
        curve: automation::Curve::Linear,
    };
    let mut out = Vec::new();
    for (k, &(t, g)) in env.iter().enumerate() {
        if let Some(&(t0, g0)) = k.checked_sub(1).and_then(|j| env.get(j)) {
            if (to_db(g) - to_db(g0)).abs() > 1.0 && t > t0 {
                for s in 1..8 {
                    let x = s as f64 / 8.0;
                    out.push(pt(t0 + (t - t0) * x, g0 + (g - g0) * x));
                }
            }
        }
        out.push(pt(t, g));
    }
    out
}

/// One clip as read: offset and trims in seconds, its channels.
struct RawClip {
    name: String,
    offset: f64,
    trim_left: f64,
    trim_right: f64,
    ratio: f64,
    cents: f64,
    chans: Vec<Channel>,
    envelope: Vec<(f64, f64)>,
}

struct RawTrack {
    name: String,
    channel: i64,
    linked: bool,
    mute: bool,
    solo: bool,
    rate: u32,
    gain: f64,
    pan: f64,
    clips: Vec<RawClip>,
}

fn raw_tracks(root: &Elem, project_rate: u32) -> Vec<RawTrack> {
    root.kids("wavetrack")
        .map(|t| {
            let mut clips: Vec<RawClip> = t
                .kids("waveclip")
                .map(|c| RawClip {
                    name: c.text("name").unwrap_or_default().to_string(),
                    offset: c.num("offset").unwrap_or(0.0),
                    trim_left: c.num("trimLeft").unwrap_or(0.0),
                    trim_right: c.num("trimRight").unwrap_or(0.0),
                    ratio: c
                        .num("clipStretchRatio")
                        .filter(|r| r.is_finite() && *r > 0.0)
                        .unwrap_or(1.0)
                        .clamp(0.25, 4.0),
                    cents: c.num("centShift").unwrap_or(0.0),
                    chans: c.kids("sequence").map(channel_of).collect(),
                    envelope: c
                        .kids("envelope")
                        .next()
                        .map(|e| {
                            e.kids("controlpoint")
                                .filter_map(|p| Some((p.num("t")?, p.num("val")?)))
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .filter(|c| !c.chans.is_empty())
                .collect();
            clips.sort_by(|a, b| (a.offset + a.trim_left).total_cmp(&(b.offset + b.trim_left)));
            RawTrack {
                name: t.text("name").unwrap_or("Track").to_string(),
                channel: t.num("channel").unwrap_or(2.0) as i64,
                linked: t.num("linked").unwrap_or(0.0) != 0.0,
                mute: t.num("mute").unwrap_or(0.0) != 0.0,
                solo: t.num("solo").unwrap_or(0.0) != 0.0,
                rate: t
                    .num("rate")
                    .map_or(project_rate, |r| r.clamp(1000.0, 768_000.0) as u32),
                gain: t.num("gain").unwrap_or(1.0),
                pan: t.num("pan").unwrap_or(0.0),
                clips,
            }
        })
        .collect()
}

/// A readable name from a real-time effect's plug-in id (its last part,
/// which names the effect).
fn effect_name(id: &str) -> String {
    id.rsplit(['_', ';', '\\', '/'])
        .find(|p| !p.trim().is_empty())
        .unwrap_or(id)
        .trim()
        .to_string()
}

/// Imports a project, writing its audio into `audio_dir`.
pub fn import(path: &Path, audio_dir: &Path) -> Result<Imported, String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    let (dict, doc): (Vec<u8>, Vec<u8>) = conn
        .query_row("SELECT dict, doc FROM project LIMIT 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .map_err(|_| {
            format!(
                "{} has no saved project data (it may only hold an autosave).",
                path.display()
            )
        })?;
    let root = tree(&doc, &names(&dict)?)
        .map_err(|e| format!("{} could not be read: {e}", path.display()))?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported Project".into());
    let rate = root
        .num("rate")
        .map_or(44_100.0, |r| r.clamp(8000.0, 384_000.0))
        .round() as u32;
    let mut out = Session::new(&name, rate, 0);
    let mut notices = Vec::new();
    let mut dropped: Vec<String> = Vec::new();

    // Tracks, with left and right halves of a stereo track joined.
    let raw = raw_tracks(&root, rate);
    let mut layout: Vec<(usize, Option<usize>)> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let t = &raw[i];
        let pair = raw.get(i + 1).filter(|n| {
            t.channel == 0 && t.linked && n.channel == 1 && n.clips.len() == t.clips.len()
        });
        if pair.is_some() {
            layout.push((i, Some(i + 1)));
            i += 2;
        } else {
            layout.push((i, None));
            i += 1;
        }
    }
    if raw.is_empty() && root.kids("labeltrack").next().is_none() {
        return Err(format!("{} has no audio tracks.", path.display()));
    }

    // Distinct recordings: one per set of channels; a recording whose
    // blocks run on from another's (a clip and its longer original)
    // shares the longer one's file.
    let mut sources: Vec<(Vec<Channel>, u32)> = Vec::new();
    let mut clip_source: HashMap<(usize, usize), usize> = HashMap::new();
    let mut wants: Vec<((usize, usize), Vec<Channel>, u32)> = Vec::new();
    for (k, &(l, r)) in layout.iter().enumerate() {
        for ci in 0..raw[l].clips.len() {
            let mut chans = raw[l].clips[ci].chans.clone();
            if let Some(r) = r {
                chans.extend(raw[r].clips[ci].chans.iter().take(1).cloned());
            }
            wants.push(((k, ci), chans, raw[l].rate));
        }
    }
    let mut order: Vec<usize> = (0..wants.len()).collect();
    order.sort_by_key(|&w| std::cmp::Reverse(wants[w].1.iter().map(Channel::len).sum::<u64>()));
    for w in order {
        let (key, chans, mrate) = &wants[w];
        let found = sources.iter().position(|(have, r)| {
            r == mrate
                && have.len() == chans.len()
                && have.iter().zip(chans).all(|(h, c)| h.starts_with(c))
        });
        let idx = found.unwrap_or_else(|| {
            sources.push((chans.clone(), *mrate));
            sources.len() - 1
        });
        clip_source.insert(*key, idx);
    }

    std::fs::create_dir_all(audio_dir)
        .map_err(|e| format!("Could not make {}: {e}", audio_dir.display()))?;
    let mut media_ids = Vec::new();
    for (n, (chans, mrate)) in sources.iter().enumerate() {
        let file = audio_dir.join(format!("{name} {:02}.wav", n + 1));
        write_source(&conn, chans, *mrate, &file)
            .map_err(|e| format!("Could not write {}: {e}", file.display()))?;
        let id = out.alloc_id();
        let frames = chans.iter().map(Channel::len).max().unwrap_or(0);
        out.media.push(MediaItem {
            id,
            name: crate::file_name(&file),
            path: Some(file),
            duration: frames as f64 / *mrate as f64,
            rate: *mrate,
            channels: chans.len() as u16,
            ext_id: None,
        });
        media_ids.push((id, frames, *mrate));
    }

    let mut stretched = 0;
    let mut clip_count = 0;
    let mut lines = 0;
    for (k, &(l, _)) in layout.iter().enumerate() {
        let t = &raw[l];
        let tid = out.add_track();
        let mut clips = Vec::new();
        for (ci, c) in t.clips.iter().enumerate() {
            let (media, mframes, mrate) = media_ids[clip_source[&(k, ci)]];
            let mrate_f = mrate as f64;
            let natural = c.chans[0].len() as f64 / mrate_f;
            let mut left = c.trim_left.max(0.0);
            let mut at = c.offset + left;
            if at < 0.0 {
                left -= at;
                at = 0.0;
            }
            let seen = (natural * c.ratio - left - c.trim_right.max(0.0)).max(0.0);
            let src_in = (left / c.ratio * mrate_f).round();
            let src_out = (src_in + (seen / c.ratio * mrate_f).round())
                .min(c.chans[0].len() as f64)
                .min(mframes as f64);
            if src_out <= src_in {
                continue;
            }
            let mut clip = Clip {
                id: out.alloc_id(),
                media,
                name: if c.name.is_empty() {
                    t.name.clone()
                } else {
                    c.name.clone()
                },
                start: (at * rate as f64).round() / rate as f64,
                offset: src_in / mrate_f,
                length: (src_out - src_in) / mrate_f * c.ratio,
                gain_db: 0.0,
                fade_in: 0.0,
                fade_out: 0.0,
                muted: false,
                props: Default::default(),
                ext_id: None,
            };
            if (c.ratio - 1.0).abs() > 1e-6 || c.cents.abs() > 0.5 {
                stretched += 1;
                let st = &mut clip.props.stretch;
                st.mode = StretchMode::Live;
                st.kind = StretchKind::Polyphonic;
                st.ratio = c.ratio;
                st.pitch = (c.cents / 100.0).clamp(-24.0, 24.0) as f32;
            }
            // A clip volume line: flat is a level; a moving one becomes
            // the clip's own volume lane, cut to the part heard.
            let inside: Vec<f64> = c
                .envelope
                .iter()
                .filter(|(t, _)| *t >= left && *t <= left + seen)
                .map(|&(_, v)| v)
                .collect();
            if let Some(&first) = inside.first().or(c.envelope.first().map(|p| &p.1)) {
                if inside.iter().all(|v| (v - first).abs() <= 0.001) {
                    clip.gain_db = to_db(first);
                } else {
                    let mut lane = Lane::new(LaneTarget::Volume);
                    lane.points = automation::crop(&volume_line(&c.envelope), left, left + seen);
                    clip.props.lanes.push(lane);
                    lines += 1;
                }
            }
            clips.push(clip);
            clip_count += 1;
        }
        let tr = out.track_mut(tid).unwrap();
        tr.name = t.name.clone();
        tr.volume_db = to_db(t.gain).max(-60.0);
        tr.pan = t.pan.clamp(-1.0, 1.0) as f32;
        tr.mute = t.mute;
        tr.solo = t.solo;
        tr.clips = clips;
    }

    // Labels: markers (a label with an end is a range).
    for lt in root.kids("labeltrack") {
        for l in lt.kids("label") {
            let t = l.num("t").unwrap_or(0.0).max(0.0);
            let t1 = l.num("t1").unwrap_or(t).max(t);
            out.markers.push(Marker {
                name: l.text("title").unwrap_or("Label").to_string(),
                time: t,
                length: t1 - t,
                ext_id: None,
            });
        }
    }
    out.markers.sort_by(|a, b| a.time.total_cmp(&b.time));
    if !out.markers.is_empty() {
        notices.push(format!(
            "{} label(s) came across as markers.",
            out.markers.len()
        ));
    }
    for (tag, what) in [
        ("notetrack", "note track(s)"),
        ("timetrack", "time track(s)"),
    ] {
        let n = root.kids(tag).count();
        if n > 0 {
            dropped.push(format!("{n} {what}"));
        }
    }
    // Real-time effects on tracks and the master: their lists are in
    // every project, so only the effects in them count.
    let mut fx_names: Vec<String> = Vec::new();
    let lists = root
        .children
        .iter()
        .flat_map(|e| std::iter::once(e).chain(e.children.iter()))
        .filter(|e| e.name == "effects");
    for list in lists {
        for fx in list.children.iter().filter(|e| e.name == "effect") {
            fx_names.push(
                fx.text("id")
                    .map(effect_name)
                    .unwrap_or_else(|| "effect".into()),
            );
        }
    }
    if !fx_names.is_empty() {
        let n = fx_names.len();
        fx_names.sort();
        fx_names.dedup();
        dropped.push(format!(
            "{n} real-time effect(s) ({}; rebuild them in the Effects Rack)",
            fx_names.join(", ")
        ));
    }
    notices.push(format!(
        "Imported {} track(s) and {clip_count} clip(s); the audio was written to {}.",
        layout.len(),
        audio_dir.display()
    ));
    if lines > 0 {
        notices.push(format!(
            "{lines} clip volume line(s) came across as clip automation."
        ));
    }
    if stretched > 0 {
        notices.push(format!("{stretched} clip(s) keep their stretch and pitch."));
    }
    if !dropped.is_empty() {
        notices.push(format!("Not carried over: {}.", dropped.join(", ")));
    }
    Ok(Imported {
        session: out,
        notices,
        base: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds `dict` and `doc` bytes for a small tree.
    struct W {
        names: Vec<String>,
        doc: Vec<u8>,
    }

    impl W {
        fn id(&mut self, n: &str) -> u16 {
            match self.names.iter().position(|x| x == n) {
                Some(i) => i as u16,
                None => {
                    self.names.push(n.into());
                    (self.names.len() - 1) as u16
                }
            }
        }
        fn start(&mut self, n: &str) {
            let id = self.id(n);
            self.doc.push(START);
            self.doc.extend(id.to_le_bytes());
        }
        fn end(&mut self, n: &str) {
            let id = self.id(n);
            self.doc.push(END);
            self.doc.extend(id.to_le_bytes());
        }
        fn s(&mut self, n: &str, v: &str) {
            let id = self.id(n);
            let t: Vec<u8> = v.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
            self.doc.push(STRING);
            self.doc.extend(id.to_le_bytes());
            self.doc.extend((t.len() as u32).to_le_bytes());
            self.doc.extend(t);
        }
        fn d(&mut self, n: &str, v: f64) {
            let id = self.id(n);
            self.doc.push(DOUBLE);
            self.doc.extend(id.to_le_bytes());
            self.doc.extend(v.to_le_bytes());
            self.doc.extend(0i32.to_le_bytes());
        }
        fn i(&mut self, n: &str, v: i32) {
            let id = self.id(n);
            self.doc.push(INT);
            self.doc.extend(id.to_le_bytes());
            self.doc.extend(v.to_le_bytes());
        }
        fn ll(&mut self, n: &str, v: i64) {
            let id = self.id(n);
            self.doc.push(LONG_LONG);
            self.doc.extend(id.to_le_bytes());
            self.doc.extend(v.to_le_bytes());
        }
        fn dict(&self) -> Vec<u8> {
            let mut d = vec![CHAR_SIZE, 2];
            for (i, n) in self.names.iter().enumerate() {
                let t: Vec<u8> = n.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
                d.push(NAME);
                d.extend((i as u16).to_le_bytes());
                d.extend((t.len() as u16).to_le_bytes());
                d.extend(t);
            }
            d
        }
    }

    /// A project with one mono track holding two clips: one plays blocks
    /// 1 and 2, the other starts on block 1 too but goes on to block 3.
    fn project(dir: &Path) -> PathBuf {
        let mut w = W {
            names: Vec::new(),
            doc: vec![CHAR_SIZE, 2],
        };
        w.start("project");
        w.i("rate", 8000);
        w.start("wavetrack");
        w.s("name", "Voice");
        w.i("channel", 2);
        w.d("gain", 0.5);
        w.d("pan", -0.25);
        for (name, offset, blocks) in [("a", 0.0, [1i64, 2]), ("b", 1.0, [1i64, 3])] {
            w.start("waveclip");
            w.s("name", name);
            w.d("offset", offset);
            w.d("trimLeft", 0.0);
            w.d("trimRight", 0.0);
            w.d("clipStretchRatio", 1.0);
            w.start("sequence");
            w.ll("numsamples", 8);
            for (k, b) in blocks.iter().enumerate() {
                w.start("waveblock");
                w.ll("start", 4 * k as i64);
                w.ll("blockid", *b);
                w.end("waveblock");
            }
            w.end("sequence");
            w.end("waveclip");
        }
        // An empty effect list (every track has one) and one effect.
        w.start("effects");
        w.end("effects");
        w.end("wavetrack");
        w.start("effects");
        w.start("effect");
        w.s("id", "Effect_VST3_Vendor_Big Reverb");
        w.end("effect");
        w.end("effects");
        w.start("labeltrack");
        w.start("label");
        w.d("t", 1.5);
        w.d("t1", 2.0);
        w.s("title", "Chorus");
        w.end("label");
        w.end("labeltrack");
        w.end("project");
        let path = dir.join("Song.aup3");
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE project(id INTEGER PRIMARY KEY, dict BLOB, doc BLOB);
             CREATE TABLE sampleblocks(blockid INTEGER PRIMARY KEY AUTOINCREMENT, sampleformat INTEGER, samples BLOB);",
        )
        .unwrap();
        c.execute(
            "INSERT INTO project(id, dict, doc) VALUES (1, ?1, ?2)",
            rusqlite::params![w.dict(), w.doc],
        )
        .unwrap();
        for (id, v) in [(1i64, 0.25f32), (2, 0.5), (3, -0.5)] {
            let bytes: Vec<u8> = std::iter::repeat_n(v, 4)
                .flat_map(|x| x.to_le_bytes())
                .collect();
            c.execute(
                "INSERT INTO sampleblocks(blockid, sampleformat, samples) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, 0x0004_000Fi64, bytes],
            )
            .unwrap();
        }
        path
    }

    #[test]
    fn a_project_imports_and_clips_that_diverge_play_their_own_audio() {
        let dir = std::env::temp_dir().join(format!("af27-aup3-{}", crate::audfrm::new_uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = project(&dir);
        let audio = audio_folder(&path);
        assert!(audio.ends_with("Song audio"));
        let imp = import(&path, &audio).unwrap();
        let s = &imp.session;
        assert_eq!(s.rate, 8000);
        assert_eq!(s.tracks.len(), 1);
        let t = &s.tracks[0];
        assert_eq!(t.name, "Voice");
        assert!((t.volume_db + 6.0).abs() < 0.05);
        assert_eq!(t.pan, -0.25);
        assert_eq!(t.clips.len(), 2);
        assert_eq!(t.clips[1].start, 1.0);
        assert_eq!(t.clips[0].length, 1.0 / 1000.0);
        // Two recordings, each with its own second block.
        assert_eq!(s.media.len(), 2);
        let read = |m: &MediaItem| -> Vec<f32> {
            hound::WavReader::open(m.path.as_ref().unwrap())
                .unwrap()
                .samples::<f32>()
                .map(Result::unwrap)
                .collect()
        };
        let a = s.media(t.clips[0].media).unwrap();
        let b = s.media(t.clips[1].media).unwrap();
        assert_eq!(
            read(a),
            [0.25; 4]
                .iter()
                .chain(&[0.5; 4])
                .copied()
                .collect::<Vec<_>>()
        );
        assert_eq!(
            read(b),
            [0.25; 4]
                .iter()
                .chain(&[-0.5; 4])
                .copied()
                .collect::<Vec<_>>()
        );
        // The label is a range marker; the effect is named in the notice
        // and the empty lists are not counted.
        let m = &imp.session.markers;
        assert_eq!(m.len(), 1);
        assert_eq!(
            (m[0].name.as_str(), m[0].time, m[0].length),
            ("Chorus", 1.5, 0.5)
        );
        let all = imp.notices.join("\n");
        assert!(all.contains("1 real-time effect(s) (Big Reverb;"), "{all}");
        assert!(!all.contains("label track"), "{all}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_records_and_other_files_are_refused() {
        let n = names(&[CHAR_SIZE, 2]).unwrap();
        assert!(tree(&[99], &n).is_err());
        assert!(tree(&[], &n).is_err());
        assert!(names(&[NAME, 0]).is_err());
        let dir = std::env::temp_dir();
        assert!(import(&dir.join("no such project.aup3"), &dir).is_err());
    }

    /// `AUP3_TEST=<project.aup3>`: imports a real project into a temporary
    /// folder (the project itself is only read).
    #[test]
    #[ignore]
    fn real_project() {
        let Some(p) = std::env::var_os("AUP3_TEST") else {
            return;
        };
        let out =
            std::env::temp_dir().join(format!("af27-aup3-real-{}", crate::audfrm::new_uuid()));
        let imp = import(Path::new(&p), &out).unwrap();
        eprintln!("{:?}", imp.notices);
        assert!(!imp.session.tracks.is_empty());
        for t in &imp.session.tracks {
            eprintln!("{}: {} clip(s)", t.name, t.clips.len());
        }
        let _ = std::fs::remove_dir_all(&out);
    }
}
