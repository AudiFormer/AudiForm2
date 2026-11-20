// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! What the app and a plug-in host process say to each other over the
//! host's stdin and stdout, and what a scan process prints.
//!
//! Every message is one frame: a kind byte, the plug-in's number in the
//! process (`u32` LE), the payload length (`u32` LE), then the payload.
//! Both sides open with a `HELLO` frame carrying [`VERSION`], so a host
//! from another build is refused instead of misread.
//!
//! `CONTROL` payloads are JSON: a [`Request`] to the host or a [`Message`]
//! from it, each with a request id (`rid`). Replies carry the id of their
//! request; events the host sends unasked carry 0.
//!
//! `AUDIO` to the host: `seq u32`, `frames u32`, `keyed u8`, `n u32`
//! parameter changes (`u32` id, `f64` normalized value), then `frames * 2`
//! interleaved stereo `f32` samples, then (when keyed) as many sidechain
//! key samples. Back: `seq u32`, `frames u32`, `ok u8` (0: the plug-in did
//! not run and the samples are the input), then the processed samples.

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// Bumped whenever a frame or message changes shape.
pub const VERSION: u32 = 1;

pub const HELLO: u8 = 0;
pub const CONTROL: u8 = 1;
pub const AUDIO: u8 = 2;

/// The longest block a plug-in is handed at once; longer ones are split.
pub const MAX_BLOCK: usize = 8192;
/// Frames bigger than this are refused as garbage.
pub const MAX_FRAME: usize = 64 << 20;

/// Scan output lines start with this (stdout), stage lines with
/// [`STAGE_PREFIX`] (stderr). Anything else a plug-in prints is ignored.
pub const SCAN_PREFIX: &str = "@@af2-scan ";
pub const STAGE_PREFIX: &str = "@@af2-stage ";

/// One frame read off a pipe.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub kind: u8,
    pub id: u32,
    pub payload: Vec<u8>,
}

pub fn write_frame(w: &mut impl Write, kind: u8, id: u32, payload: &[u8]) -> io::Result<()> {
    let mut head = [0u8; 9];
    head[0] = kind;
    head[1..5].copy_from_slice(&id.to_le_bytes());
    head[5..9].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    w.write_all(&head)?;
    w.write_all(payload)?;
    w.flush()
}

/// Reads one frame; `Ok(None)` at a clean end of the pipe.
pub fn read_frame(r: &mut impl Read) -> io::Result<Option<Frame>> {
    let mut head = [0u8; 9];
    let mut got = 0;
    while got < head.len() {
        match r.read(&mut head[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let id = u32::from_le_bytes(head[1..5].try_into().unwrap());
    let len = u32::from_le_bytes(head[5..9].try_into().unwrap()) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok(Some(Frame {
        kind: head[0],
        id,
        payload,
    }))
}

/// One control of a plug-in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamInfo {
    pub id: u32,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub units: String,
    /// 0: continuous; otherwise the number of steps (1 = a switch).
    #[serde(default)]
    pub steps: i32,
    pub default: f64,
    /// Read-only, hidden or the program changer: not shown as a control.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// One plug-in in a bundle, as a scan read it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ClassInfo {
    /// The class id, 32 hex digits in the bytes' own order.
    pub cid: String,
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub version: String,
    /// For example `Fx|Dynamics`.
    #[serde(default)]
    pub sub_categories: String,
    #[serde(default)]
    pub params: Vec<ParamInfo>,
    /// Samples, at 48 kHz in stereo with the sidechain off.
    #[serde(default)]
    pub latency: u32,
    #[serde(default)]
    pub has_editor: bool,
    #[serde(default)]
    pub instrument: bool,
    /// It has a second audio input (a key).
    #[serde(default)]
    pub sidechain: bool,
    /// Set when the plug-in was named but not read in full.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What a host reports once a plug-in is running.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Loaded {
    pub latency: u32,
    /// The latency with the key input switched on (only with a sidechain).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyed_latency: Option<u32>,
    #[serde(default)]
    pub tail: u32,
    #[serde(default)]
    pub params: Vec<ParamInfo>,
    #[serde(default)]
    pub has_editor: bool,
    #[serde(default)]
    pub sidechain: bool,
    #[serde(default)]
    pub instrument: bool,
    /// The parameter values right after loading (state and settings in).
    #[serde(default)]
    pub values: Vec<(u32, f64)>,
}

/// To the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "req", rename_all = "snake_case")]
pub enum Request {
    /// Starts plug-in `id` (the frame's) from `path` (a bundle).
    Load {
        path: String,
        cid: String,
        rate: f64,
        /// The settings blob, base64 (see [`join_state`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<String>,
        #[serde(default)]
        params: Vec<(u32, f64)>,
    },
    Unload,
    /// Clears tails (playback jumped).
    Reset,
    GetState,
    SetState {
        data: String,
    },
    /// New control values while no audio flows.
    SetParams {
        changes: Vec<(u32, f64)>,
    },
    OpenEditor {
        title: String,
        /// The window that owns the plug-in window (an HWND), 0 for none.
        #[serde(default, skip_serializing_if = "is_zero")]
        owner: u64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        bypassed: bool,
    },
    CloseEditor,
    /// The slot's power changed in the app: show it on the window's strip.
    ShowBypassed {
        bypassed: bool,
    },
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// From the host: replies (with the request's id) and events (id 0).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "msg", rename_all = "snake_case")]
pub enum Message {
    Loaded(Loaded),
    State {
        data: String,
    },
    Ok,
    Error {
        message: String,
    },
    /// A control moved in the plug-in's own window.
    ParamEdited {
        id: u32,
        value: f64,
    },
    /// The user took hold of a control in the plug-in's window (Touch).
    BeginEdit {
        id: u32,
    },
    EndEdit {
        id: u32,
    },
    LatencyChanged {
        latency: u32,
    },
    /// Settings changed that are not controls: fetch the state.
    Dirty,
    EditorClosed,
    /// The power button on the window's strip.
    BypassPicked {
        bypassed: bool,
    },
    /// Space pressed in the plug-in's window (Shift+Space: record).
    SpacePressed {
        record: bool,
    },
}

/// A control frame's JSON: a request id plus the request or message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub rid: u32,
    #[serde(flatten)]
    pub body: T,
}

pub fn encode_control<T: Serialize>(rid: u32, body: &T) -> Vec<u8> {
    serde_json::to_vec(&Envelope { rid, body }).unwrap_or_default()
}

pub fn decode_control<T: for<'de> Deserialize<'de>>(payload: &[u8]) -> Option<(u32, T)> {
    let e: Envelope<T> = serde_json::from_slice(payload).ok()?;
    Some((e.rid, e.body))
}

/// Writes an AUDIO payload to the host into `out` (cleared first).
pub fn encode_audio(
    out: &mut Vec<u8>,
    seq: u32,
    input: &[f32],
    key: Option<&[f32]>,
    changes: &[(u32, f64)],
) {
    let frames = input.len() / 2;
    out.clear();
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&(frames as u32).to_le_bytes());
    out.push(key.is_some() as u8);
    out.extend_from_slice(&(changes.len() as u32).to_le_bytes());
    for (id, v) in changes {
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&v.to_le_bytes());
    }
    for s in &input[..frames * 2] {
        out.extend_from_slice(&s.to_le_bytes());
    }
    if let Some(k) = key {
        for i in 0..frames * 2 {
            out.extend_from_slice(&k.get(i).copied().unwrap_or(0.0).to_le_bytes());
        }
    }
}

/// An AUDIO payload as the host reads it.
#[derive(Debug, Default)]
pub struct AudioIn {
    pub seq: u32,
    pub frames: usize,
    pub changes: Vec<(u32, f64)>,
    pub samples: Vec<f32>,
    pub key: Option<Vec<f32>>,
}

fn take<'a>(b: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if b.len() < n {
        return None;
    }
    let (a, rest) = b.split_at(n);
    *b = rest;
    Some(a)
}

fn u32_at(b: &mut &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(take(b, 4)?.try_into().ok()?))
}

fn f32s(b: &mut &[u8], n: usize, out: &mut Vec<f32>) -> Option<()> {
    let raw = take(b, n.checked_mul(4)?)?;
    out.clear();
    out.extend(
        raw.as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c)),
    );
    Some(())
}

/// Reads an AUDIO payload into `a` (its buffers reused).
pub fn decode_audio(payload: &[u8], a: &mut AudioIn) -> Option<()> {
    let mut b = payload;
    a.seq = u32_at(&mut b)?;
    a.frames = u32_at(&mut b)? as usize;
    let keyed = take(&mut b, 1)?[0] != 0;
    let n = u32_at(&mut b)? as usize;
    a.changes.clear();
    for _ in 0..n {
        let id = u32_at(&mut b)?;
        let v = f64::from_le_bytes(take(&mut b, 8)?.try_into().ok()?);
        a.changes.push((id, v));
    }
    f32s(&mut b, a.frames.checked_mul(2)?, &mut a.samples)?;
    if keyed {
        let mut k = a.key.take().unwrap_or_default();
        f32s(&mut b, a.frames * 2, &mut k)?;
        a.key = Some(k);
    } else if let Some(k) = &mut a.key {
        // Keep the buffer for next time, but mark "no key".
        k.clear();
        a.key = None;
    }
    Some(())
}

/// Writes the host's AUDIO answer.
pub fn encode_audio_reply(out: &mut Vec<u8>, seq: u32, ok: bool, samples: &[f32]) {
    out.clear();
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&((samples.len() / 2) as u32).to_le_bytes());
    out.push(ok as u8);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
}

/// Reads the host's AUDIO answer into `out`: (seq, ok).
pub fn decode_audio_reply(payload: &[u8], out: &mut Vec<f32>) -> Option<(u32, bool)> {
    let mut b = payload;
    let seq = u32_at(&mut b)?;
    let frames = u32_at(&mut b)? as usize;
    let ok = take(&mut b, 1)?[0] != 0;
    f32s(&mut b, frames.checked_mul(2)?, out)?;
    Some((seq, ok))
}

// ---- Settings blobs ----------------------------------------------------

/// Joins a plug-in's settings into one blob, the form Audiform sessions
/// keep (`plugin_state`): the component part's length (`u32` LE), the
/// component part, then the controller part (empty when the controller
/// is the component itself).
pub fn join_state(component: &[u8], controller: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + component.len() + controller.len());
    out.extend_from_slice(&(component.len() as u32).to_le_bytes());
    out.extend_from_slice(component);
    out.extend_from_slice(controller);
    out
}

/// Splits a blob from [`join_state`] into (component, controller).
pub fn split_state(blob: &[u8]) -> Option<(&[u8], &[u8])> {
    let n = u32::from_le_bytes(blob.get(..4)?.try_into().ok()?) as usize;
    let rest = &blob[4..];
    if n > rest.len() {
        return None;
    }
    Some(rest.split_at(n))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn base64_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for k in 0..4 {
            if k <= c.len() {
                s.push(B64[((n >> (18 - 6 * k)) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// Reads standard base64 (padding optional, whitespace skipped).
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0;
    for ch in text.bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

// ---- Class ids ---------------------------------------------------------

pub fn cid_hex(cid: &[u8; 16]) -> String {
    cid.iter().map(|b| format!("{b:02X}")).collect()
}

pub fn cid_from_hex(s: &str) -> Option<[u8; 16]> {
    let s = s.trim();
    if s.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// The same class id with its first three fields byte-swapped: Windows
/// (COM) and other systems write a class id's bytes in different orders,
/// so ids from files made elsewhere are tried both ways.
pub fn cid_other_order(s: &str) -> Option<String> {
    let b = cid_from_hex(s)?;
    let mut o = b;
    o[0..4].copy_from_slice(&[b[3], b[2], b[1], b[0]]);
    o[4..6].copy_from_slice(&[b[5], b[4]]);
    o[6..8].copy_from_slice(&[b[7], b[6]]);
    Some(cid_hex(&o))
}

// ---- Scans -------------------------------------------------------------

/// One step of a scan, printed by the scan process as it goes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "line", rename_all = "snake_case")]
pub enum ScanLine {
    /// Every plug-in in the bundle by name, before any is started.
    Found {
        classes: Vec<ClassInfo>,
    },
    /// One plug-in read in full.
    Read {
        class: ClassInfo,
    },
    Failed {
        cid: String,
        error: String,
    },
    /// The bundle could not be opened at all.
    Error {
        message: String,
    },
    Done,
}

pub fn scan_line(line: &ScanLine) -> String {
    format!(
        "{SCAN_PREFIX}{}",
        serde_json::to_string(line).unwrap_or_default()
    )
}

/// What a scan of one bundle found.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScanResult {
    pub classes: Vec<ClassInfo>,
    /// The bundle failed as a whole (nothing usable in it).
    pub error: Option<String>,
    /// The scan printed `done`.
    pub done: bool,
}

/// Words for a Windows exit code.
pub fn crash_reason(code: Option<i32>) -> String {
    let Some(c) = code else {
        return "it was stopped".into();
    };
    let u = c as u32;
    let what = match u {
        0xC000_0005 => "a memory access violation",
        0xC000_001D => "an illegal instruction",
        0xC000_00FD => "a stack overflow",
        0xC000_0094 => "a division by zero",
        0xC000_0135 => "a file it needs (a DLL) is missing",
        0xC000_007B => "it is not a 64-bit plug-in",
        0xC000_0409 => "it stopped itself (a security check)",
        0x8000_0003 => "a breakpoint",
        0xC000_0374 => "heap corruption",
        0xC000_0602 | 0xE06D_7363 => "an unhandled error",
        _ => "",
    };
    if what.is_empty() {
        format!("exit code {u:X}")
    } else {
        format!("{what} (code {u:X})")
    }
}

/// Rebuilds a scan's result from what it printed, even when it died part
/// way: plug-ins named but never read are kept, with a note saying where
/// it stopped.
pub fn parse_scan_output(stdout: &str, stderr: &str, exit: Option<i32>) -> ScanResult {
    let mut r = ScanResult::default();
    let mut named: Vec<ClassInfo> = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    for line in stdout.lines() {
        let Some(json) = line.trim_end().strip_prefix(SCAN_PREFIX) else {
            continue;
        };
        match serde_json::from_str::<ScanLine>(json) {
            Ok(ScanLine::Found { classes }) => named = classes,
            Ok(ScanLine::Read { class }) => {
                r.classes.retain(|c| c.cid != class.cid);
                r.classes.push(class);
            }
            Ok(ScanLine::Failed { cid, error }) => failed.push((cid, error)),
            Ok(ScanLine::Error { message }) => r.error = Some(message),
            Ok(ScanLine::Done) => r.done = true,
            Err(_) => {}
        }
    }
    let stage = stderr
        .lines()
        .filter_map(|l| l.trim_end().strip_prefix(STAGE_PREFIX))
        .next_back()
        .map(str::to_string);
    let died = !r.done;
    for c in named {
        if r.classes.iter().any(|x| x.cid == c.cid) {
            continue;
        }
        let mut c = c;
        if let Some((_, e)) = failed.iter().find(|(id, _)| *id == c.cid) {
            c.note = Some(format!("It could not be started: {e}"));
        } else if died {
            let at = stage.as_deref().unwrap_or("loading it");
            c.note = Some(format!(
                "Not fully read. It crashed while {at}: {}. It may still work; if it crashes when used, it is bypassed.",
                crash_reason(exit)
            ));
        } else {
            continue;
        }
        r.classes.push(c);
    }
    if died && r.classes.is_empty() && r.error.is_none() {
        let at = stage.as_deref().unwrap_or("loading its file");
        r.error = Some(format!("It crashed while {at}: {}.", crash_reason(exit)));
    }
    r
}

/// Whether a class's sub-categories call it an instrument.
pub fn named_instrument(sub_categories: &str) -> bool {
    let s = sub_categories.to_ascii_lowercase();
    s.contains("instrument") && !s.contains("fx")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_stop_cleanly() {
        let mut buf = Vec::new();
        write_frame(&mut buf, CONTROL, 7, b"hello").unwrap();
        write_frame(&mut buf, AUDIO, 0, &[]).unwrap();
        let mut r = &buf[..];
        let a = read_frame(&mut r).unwrap().unwrap();
        assert_eq!((a.kind, a.id, &a.payload[..]), (CONTROL, 7, &b"hello"[..]));
        let b = read_frame(&mut r).unwrap().unwrap();
        assert_eq!((b.kind, b.payload.len()), (AUDIO, 0));
        assert!(read_frame(&mut r).unwrap().is_none());
        // A cut-off frame is an error, not a clean end.
        let mut cut = &buf[..4];
        assert!(read_frame(&mut cut).is_err());
    }

    #[test]
    fn audio_round_trips_with_and_without_a_key() {
        let input: Vec<f32> = (0..20).map(|i| i as f32 * 0.25).collect();
        let key: Vec<f32> = (0..20).map(|i| -(i as f32)).collect();
        let mut out = Vec::new();
        let mut a = AudioIn::default();
        encode_audio(
            &mut out,
            9,
            &input,
            Some(&key),
            &[(3, 0.5), (u32::MAX, 1.0)],
        );
        decode_audio(&out, &mut a).unwrap();
        assert_eq!((a.seq, a.frames), (9, 10));
        assert_eq!(a.changes, vec![(3, 0.5), (u32::MAX, 1.0)]);
        assert_eq!(a.samples, input);
        assert_eq!(a.key.as_deref(), Some(&key[..]));
        encode_audio(&mut out, 10, &input, None, &[]);
        decode_audio(&out, &mut a).unwrap();
        assert!(a.key.is_none());
        let mut back = Vec::new();
        encode_audio_reply(&mut out, 10, true, &input);
        assert_eq!(decode_audio_reply(&out, &mut back), Some((10, true)));
        assert_eq!(back, input);
        // A short payload is refused, not read past its end.
        assert!(decode_audio(&out[..5], &mut a).is_none());
    }

    #[test]
    fn control_messages_are_tagged_with_request_ids() {
        let req = Request::OpenEditor {
            title: "Vocal".into(),
            owner: 0,
            bypassed: false,
        };
        let bytes = encode_control(4, &req);
        assert_eq!(
            std::str::from_utf8(&bytes).unwrap(),
            r#"{"rid":4,"req":"open_editor","title":"Vocal"}"#
        );
        assert_eq!(decode_control::<Request>(&bytes), Some((4, req)));
        let ev = Message::ParamEdited { id: 3, value: 0.5 };
        let bytes = encode_control(0, &ev);
        assert_eq!(
            std::str::from_utf8(&bytes).unwrap(),
            r#"{"rid":0,"msg":"param_edited","id":3,"value":0.5}"#
        );
        assert_eq!(decode_control::<Message>(&bytes), Some((0, ev)));
        let loaded = Message::Loaded(Loaded {
            latency: 32,
            ..Default::default()
        });
        let bytes = encode_control(1, &loaded);
        assert_eq!(decode_control::<Message>(&bytes), Some((1, loaded)));
    }

    #[test]
    fn base64_and_state_blobs_round_trip() {
        for n in 0..40 {
            let data: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            let s = base64_encode(&data);
            assert_eq!(base64_decode(&s).unwrap(), data, "{n}");
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert!(base64_decode("not*base64").is_none());
        let blob = join_state(b"comp", b"ctrl!");
        assert_eq!(&blob[..4], &4u32.to_le_bytes());
        assert_eq!(split_state(&blob), Some((&b"comp"[..], &b"ctrl!"[..])));
        assert_eq!(
            split_state(&join_state(b"x", b"")),
            Some((&b"x"[..], &b""[..]))
        );
        assert!(split_state(&[9, 0, 0, 0, 1]).is_none());
    }

    #[test]
    fn class_ids_in_both_byte_orders() {
        let id = "0123456789ABCDEF0011223344556677";
        let b = cid_from_hex(id).unwrap();
        assert_eq!(cid_hex(&b), id);
        let other = cid_other_order(id).unwrap();
        assert_eq!(other, "67452301AB89EFCD0011223344556677");
        assert_eq!(cid_other_order(&other).unwrap(), id);
        assert!(cid_from_hex("12").is_none());
    }

    fn class(cid: &str) -> ClassInfo {
        ClassInfo {
            cid: cid.into(),
            name: format!("P{cid}"),
            ..Default::default()
        }
    }

    #[test]
    fn scan_output_survives_noise_and_crashes() {
        let found = scan_line(&ScanLine::Found {
            classes: vec![class("A"), class("B"), class("C")],
        });
        let read_a = scan_line(&ScanLine::Read {
            class: ClassInfo {
                latency: 32,
                ..class("A")
            },
        });
        let fail_b = scan_line(&ScanLine::Failed {
            cid: "B".into(),
            error: "no audio processor".into(),
        });
        // A finished scan with noise from the plug-in in between.
        let out = format!(
            "{found}\nplug-in says hi\n{read_a}\n{fail_b}\n{}\n",
            scan_line(&ScanLine::Done)
        );
        let r = parse_scan_output(&out, "", Some(0));
        assert!(r.done && r.error.is_none());
        assert_eq!(r.classes.len(), 2, "A read, B noted, C dropped");
        assert_eq!(r.classes[0].latency, 32);
        assert!(r.classes[1]
            .note
            .as_ref()
            .unwrap()
            .contains("no audio processor"));
        // It crashed after reading A, while switching C on.
        let out = format!("{found}\n{read_a}\n");
        let err = format!("{STAGE_PREFIX}creating it\n{STAGE_PREFIX}switching it on\n");
        let r = parse_scan_output(&out, &err, Some(0xC000_0005u32 as i32));
        assert!(!r.done);
        assert_eq!(r.classes.len(), 3);
        let note = r.classes[2].note.as_ref().unwrap();
        assert!(note.contains("switching it on"), "{note}");
        assert!(note.contains("memory access violation"), "{note}");
        // Died before naming anything.
        let r = parse_scan_output("", &format!("{STAGE_PREFIX}loading its file\n"), Some(-1));
        assert!(r.classes.is_empty());
        assert!(r.error.unwrap().contains("loading its file"));
    }

    #[test]
    fn instruments_by_their_categories() {
        assert!(named_instrument("Instrument|Synth"));
        assert!(!named_instrument("Fx|Instrument"));
        assert!(!named_instrument("Fx|Dynamics"));
    }
}
