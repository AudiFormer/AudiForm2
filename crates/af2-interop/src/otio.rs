// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! OpenTimelineIO (`.otio`): the open timeline format video editors
//! (Kdenlive, DaVinci Resolve, others through adapters) save, read from
//! its public JSON schema.
//!
//! A `Timeline` holds a `Stack` of `Track`s. Each audio track becomes a
//! track here; its `Clip`s go in order, each `Gap` and clip moving the
//! time on, and a clip plays `source_range` (start and duration, as
//! value/rate pairs) of the file its media reference names. Transitions
//! overlap neighbours and take no time; nested stacks are left out with a
//! notice. The first video track's first file becomes the session's video
//! (its sound is on the audio tracks already).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use af2_core::session::{MediaItem, Session, TrackKind, VideoTrack};
use serde_json::Value;

use crate::Imported;

fn schema(v: &Value) -> &str {
    v["OTIO_SCHEMA"].as_str().map_or("", |s| s.split('.').next().unwrap_or(""))
}

/// A `RationalTime` in seconds.
fn secs(v: &Value) -> Option<f64> {
    let rate = v["rate"].as_f64().filter(|r| *r > 0.0)?;
    Some(v["value"].as_f64()? / rate)
}

/// A `TimeRange`: (start, duration) in seconds.
fn range(v: &Value) -> Option<(f64, f64)> {
    Some((secs(&v["start_time"])?, secs(&v["duration"])?))
}

/// A `file://` URL or a plain path, as a path.
fn url_to_path(url: &str, base: &Path) -> Option<PathBuf> {
    // A file URL is always absolute, a Windows one (file:///C:/x) too,
    // which elsewhere would read as relative.
    let p = match url.strip_prefix("file://") {
        Some(rest) => {
            let rest = percent_decode(rest);
            // file:///C:/x on Windows; file://host/x keeps the path.
            let rest = match rest.find('/') {
                Some(0) => rest,
                Some(i) => rest[i..].to_string(),
                None => rest,
            };
            let win_drive = rest.len() > 2 && rest.as_bytes()[0] == b'/' && rest.as_bytes()[2] == b':';
            return Some(PathBuf::from(if win_drive { &rest[1..] } else { rest.as_str() }));
        }
        None if url.contains("://") => return None,
        None => PathBuf::from(url),
    };
    Some(if p.is_relative() { base.join(p) } else { p })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The media reference a clip uses (the older single one, or the active
/// one of several), and the file's own length when it says.
fn media_of(clip: &Value) -> Option<(&str, Option<f64>)> {
    let r = match clip.get("media_references").and_then(Value::as_object) {
        Some(refs) => {
            let key = clip["active_media_reference_key"].as_str().unwrap_or("DEFAULT_MEDIA");
            refs.get(key).or_else(|| refs.values().next())?
        }
        None => clip.get("media_reference")?,
    };
    let url = r["target_url"].as_str().filter(|u| !u.is_empty())?;
    let avail = range(&r["available_range"]).map(|(s, d)| s + d);
    Some((url, avail))
}

pub fn import(text: &str, base_dir: &Path) -> Result<Imported, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("This OpenTimelineIO file could not be read: {e}"))?;
    if schema(&v) != "Timeline" {
        return Err("This file is not an OpenTimelineIO timeline.".into());
    }
    let mut notices = Vec::new();
    let name = v["name"].as_str().filter(|n| !n.is_empty()).unwrap_or("Imported timeline");
    let mut s = Session::new(name, 48_000, 0);
    let tracks = v["tracks"]["children"].as_array().cloned().unwrap_or_default();
    let mut media: HashMap<PathBuf, (u64, f64)> = HashMap::new();
    let mut skipped = 0;
    let mut n_audio = 0;
    for tr in &tracks {
        if schema(tr) != "Track" {
            skipped += 1;
            continue;
        }
        let kind = tr["kind"].as_str().unwrap_or("Video");
        let children = tr["children"].as_array().cloned().unwrap_or_default();
        if kind == "Video" {
            if s.video.is_none() {
                let mut at = 0.0;
                for ch in &children {
                    let (_, dur) = range(&ch["source_range"]).unwrap_or((0.0, 0.0));
                    match schema(ch) {
                        "Clip" => {
                            if let Some(p) = media_of(ch).and_then(|(u, _)| url_to_path(u, base_dir)) {
                                let (start, _) = range(&ch["source_range"]).unwrap_or((0.0, 0.0));
                                s.video = Some(VideoTrack { path: p, start: (at - start).max(0.0) });
                                break;
                            }
                            at += dur;
                        }
                        "Gap" => at += dur,
                        _ => {}
                    }
                }
            }
            continue;
        }
        if kind != "Audio" {
            skipped += 1;
            continue;
        }
        n_audio += 1;
        let t = s.add_track_of(TrackKind::Audio);
        if let Some(n) = tr["name"].as_str().filter(|n| !n.is_empty()) {
            if let Some(track) = s.track_mut(t) {
                track.name = n.to_string();
            }
        }
        if tr["enabled"].as_bool() == Some(false) {
            if let Some(track) = s.track_mut(t) {
                track.mute = true;
            }
        }
        let mut at = 0.0;
        for ch in &children {
            match schema(ch) {
                "Gap" => at += range(&ch["source_range"]).map_or(0.0, |(_, d)| d),
                "Clip" => {
                    let Some((start, dur)) = range(&ch["source_range"]) else {
                        skipped += 1;
                        continue;
                    };
                    let Some((url, avail)) = media_of(ch) else {
                        at += dur;
                        skipped += 1;
                        continue;
                    };
                    let Some(path) = url_to_path(url, base_dir) else {
                        at += dur;
                        skipped += 1;
                        continue;
                    };
                    let need = avail.unwrap_or(start + dur).max(start + dur);
                    let id = match media.get_mut(&path) {
                        Some((id, len)) => {
                            *len = len.max(need);
                            *id
                        }
                        None => {
                            let id = s.alloc_id();
                            media.insert(path.clone(), (id, need));
                            s.media.push(MediaItem {
                                id,
                                name: crate::file_name(&path),
                                path: Some(path.clone()),
                                duration: need,
                                rate: 48_000,
                                channels: 2,
                                ext_id: None,
                            });
                            id
                        }
                    };
                    if let Some(c) = s.add_clip(t, id, at) {
                        if let Some((ti, ci)) = s.find_clip(c) {
                            let clip = &mut s.tracks[ti].clips[ci];
                            clip.offset = start;
                            clip.length = dur;
                            if let Some(n) = ch["name"].as_str().filter(|n| !n.is_empty()) {
                                clip.name = n.to_string();
                            }
                            if ch["enabled"].as_bool() == Some(false) {
                                clip.muted = true;
                            }
                        }
                    }
                    at += dur;
                }
                // Overlaps its neighbours; no time of its own.
                "Transition" => {}
                _ => skipped += 1,
            }
        }
    }
    for m in &mut s.media {
        if let Some((_, len)) = m.path.as_ref().and_then(|p| media.get(p)) {
            m.duration = *len;
        }
    }
    if n_audio == 0 {
        notices.push("The timeline has no audio tracks.".into());
    }
    if skipped > 0 {
        notices.push(format!(
            "{skipped} parts of the timeline (nested sequences, generated clips or missing files) were left out."
        ));
    }
    Ok(Imported { session: s, notices, base: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "OTIO_SCHEMA": "Timeline.1", "name": "Episode 4",
      "tracks": {"OTIO_SCHEMA": "Stack.1", "children": [
        {"OTIO_SCHEMA": "Track.1", "kind": "Video", "name": "V1", "children": [
          {"OTIO_SCHEMA": "Clip.2", "name": "cam", "source_range": {"OTIO_SCHEMA": "TimeRange.1",
            "start_time": {"OTIO_SCHEMA": "RationalTime.1", "value": 0, "rate": 25},
            "duration": {"OTIO_SCHEMA": "RationalTime.1", "value": 250, "rate": 25}},
           "media_references": {"DEFAULT_MEDIA": {"OTIO_SCHEMA": "ExternalReference.1", "target_url": "file:///C:/shows/cam%20a.mp4"}},
           "active_media_reference_key": "DEFAULT_MEDIA"}]},
        {"OTIO_SCHEMA": "Track.1", "kind": "Audio", "name": "Host", "children": [
          {"OTIO_SCHEMA": "Gap.1", "source_range": {"start_time": {"value": 0, "rate": 48000}, "duration": {"value": 96000, "rate": 48000}}},
          {"OTIO_SCHEMA": "Clip.1", "name": "intro", "source_range": {
            "start_time": {"value": 48000, "rate": 48000}, "duration": {"value": 144000, "rate": 48000}},
           "media_reference": {"OTIO_SCHEMA": "ExternalReference.1", "target_url": "audio/host.wav",
             "available_range": {"start_time": {"value": 0, "rate": 48000}, "duration": {"value": 4800000, "rate": 48000}}}},
          {"OTIO_SCHEMA": "Transition.1"},
          {"OTIO_SCHEMA": "Clip.1", "name": "outro", "enabled": false, "source_range": {
            "start_time": {"value": 0, "rate": 30}, "duration": {"value": 60, "rate": 30}},
           "media_reference": {"OTIO_SCHEMA": "ExternalReference.1", "target_url": "audio/host.wav"}}
        ]},
        {"OTIO_SCHEMA": "Stack.1", "children": []}
      ]}
    }"#;

    #[test]
    fn audio_tracks_clips_gaps_and_the_video_come_across() {
        let im = import(SAMPLE, Path::new("C:/shows")).unwrap();
        let s = im.session;
        assert_eq!(s.name, "Episode 4");
        assert_eq!(s.tracks.len(), 1);
        let t = &s.tracks[0];
        assert_eq!(t.name, "Host");
        assert_eq!(t.clips.len(), 2);
        let (a, b) = (&t.clips[0], &t.clips[1]);
        // After a 2 s gap: 1 s into the file, 3 s long.
        assert_eq!((a.start, a.offset, a.length), (2.0, 1.0, 3.0));
        assert_eq!(a.name, "intro");
        // Right after it (the transition takes no time), muted.
        assert_eq!((b.start, b.length, b.muted), (5.0, 2.0, true));
        assert_eq!(s.media.len(), 1, "one file, used twice");
        assert_eq!(s.media[0].duration, 100.0, "the file's own length");
        assert!(s.media[0].path.as_ref().unwrap().ends_with("audio/host.wav"));
        let v = s.video.unwrap();
        assert_eq!(v.path, PathBuf::from("C:/shows/cam a.mp4"));
        assert_eq!(im.notices.len(), 1, "{:?}", im.notices);
    }

    #[test]
    fn not_a_timeline_says_so() {
        assert!(import(r#"{"OTIO_SCHEMA": "Clip.1"}"#, Path::new(".")).is_err());
        assert!(import("nope", Path::new(".")).is_err());
    }
}
