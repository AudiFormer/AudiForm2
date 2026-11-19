// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Opening other editors' projects as Audiform27 sessions.
//!
//! * `.sesx`: the XML multitrack session format used by a widely used
//!   commercial editor. Read from the files themselves: tracks, clip
//!   positions, source in/out points, clip gain and mute, fades, track
//!   volume, pan, mute and solo, and the file table (absolute and
//!   relative paths). Effects and automation are reported as skipped.
//! * `.audfrm` / `.altaudio`: Audiform's JSON sessions (same layout).
//!
//! Media durations and formats that a file does not state are filled in
//! when the audio is loaded.

use std::collections::HashMap;
pub mod audfrm;

use std::path::{Path, PathBuf};

use af27_core::session::{Clip, Marker, MediaItem, Session};

/// The result of an import, with anything that could not be carried over.
#[derive(Debug)]
pub struct Imported {
    pub session: Session,
    pub notices: Vec<String>,
    /// The document as read, for lossless saving back (Audiform sessions).
    pub base: Option<serde_json::Value>,
}

pub fn import(path: &Path) -> Result<Imported, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Imported Session")
        .to_string();
    match ext.as_str() {
        "sesx" => sesx(&text, base, &name),
        "audfrm" | "altaudio" => audfrm::import(&text, base),
        _ => Err(format!(
            "{} is not a session type this app can open.",
            path.display()
        )),
    }
}

/// Picks the first of absolute or relative-to-the-session paths that
/// exists, else the absolute one (shown as missing media).
pub(crate) fn resolve(abs: Option<&str>, rel: Option<&str>, base: &Path) -> Option<PathBuf> {
    let a = abs.filter(|s| !s.is_empty()).map(PathBuf::from);
    let r = rel
        .filter(|s| !s.is_empty())
        .map(|r| base.join(r.replace('\\', "/")));
    if let Some(p) = a.as_ref().filter(|p| p.exists()) {
        return Some(p.clone());
    }
    if let Some(p) = r.as_ref().filter(|p| p.exists()) {
        return Some(p.clone());
    }
    // A file moved alongside the session: try its name next to the session.
    if let Some(name) = a.as_ref().and_then(|p| {
        let s = p.to_string_lossy().replace('\\', "/");
        s.rsplit('/').next().map(str::to_string)
    }) {
        let near = base.join(&name);
        if near.exists() {
            return Some(near);
        }
    }
    a.or(r)
}

pub(crate) fn file_name(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let last = s.rsplit('/').next().unwrap_or(&s);
    last.rsplit_once('.').map_or(last, |(a, _)| a).to_string()
}

fn gain_db(linear: f64) -> f32 {
    if linear <= 1e-9 {
        -96.0
    } else {
        (20.0 * linear.log10()) as f32
    }
}

// ---------------------------------------------------------------- .sesx

fn attr<'a>(n: roxmltree::Node<'a, 'a>, k: &str) -> Option<&'a str> {
    n.attribute(k)
}

fn num(n: roxmltree::Node, k: &str) -> f64 {
    n.attribute(k).and_then(|v| v.parse().ok()).unwrap_or(0.0)
}

/// The value of parameter `pname` (or index `pidx`) of the child
/// component with id `cid`.
fn component_param(
    node: roxmltree::Node,
    cid: &str,
    pname: Option<&str>,
    pidx: &str,
) -> Option<f64> {
    let comp = node
        .descendants()
        .find(|c| c.has_tag_name("component") && c.attribute("id") == Some(cid))?;
    comp.children()
        .filter(|p| p.has_tag_name("parameter"))
        .find(|p| match pname {
            Some(n) => p.attribute("name") == Some(n),
            None => p.attribute("index") == Some(pidx),
        })
        .or_else(|| {
            comp.children()
                .find(|p| p.has_tag_name("parameter") && p.attribute("index") == Some(pidx))
        })
        .and_then(|p| p.attribute("parameterValue"))
        .and_then(|v| v.parse().ok())
}

pub fn sesx(text: &str, base: &Path, name: &str) -> Result<Imported, String> {
    let text = text.trim_start_matches('\u{feff}');
    let doc = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| format!("The .sesx file could not be read: {e}"))?;
    let root = doc.root_element();
    let sess = root
        .descendants()
        .find(|n| n.has_tag_name("session"))
        .ok_or("The .sesx file has no session.")?;
    let rate = num(sess, "sampleRate").max(1.0) as u32;
    let mut out = Session::new(name, rate, 0);
    let mut notices = Vec::new();

    // File table.
    let mut files: HashMap<String, u64> = HashMap::new();
    for f in root.descendants().filter(|n| n.has_tag_name("file")) {
        let Some(fid) = attr(f, "id") else { continue };
        let path = resolve(attr(f, "absolutePath"), attr(f, "relativePath"), base);
        let id = out.alloc_id();
        let rate_hint = attr(f, "importerPrivateSettings")
            .and_then(|s| s.split(';').find_map(|kv| kv.strip_prefix("SampleRate:0:")))
            .and_then(|v| v.parse().ok())
            .unwrap_or(rate);
        out.media.push(MediaItem {
            id,
            name: path
                .as_deref()
                .map(file_name)
                .unwrap_or_else(|| format!("File {fid}")),
            path,
            duration: 0.0,
            rate: rate_hint,
            channels: 0,
            ext_id: None,
        });
        files.insert(fid.to_string(), id);
    }

    let s = |v: f64| v / rate as f64;
    let mut skipped_fx = 0usize;
    let tracks: Vec<_> = sess
        .descendants()
        .filter(|n| n.has_tag_name("audioTrack"))
        .collect();
    let mut ordered: Vec<_> = tracks.iter().collect();
    ordered.sort_by_key(|t| {
        attr(**t, "index")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0)
    });
    for t in ordered {
        let tid = out.add_track();
        let tname = t
            .descendants()
            .find(|n| n.has_tag_name("name"))
            .and_then(|n| n.text())
            .unwrap_or("Track")
            .to_string();
        let params = t
            .children()
            .find(|n| n.has_tag_name("trackAudioParameters"));
        let volume = params
            .and_then(|p| component_param(p, "trackFader", Some("volume"), "0"))
            .unwrap_or(1.0);
        let mute = params
            .and_then(|p| component_param(p, "trackMute", Some("mute"), "1"))
            .unwrap_or(0.0);
        let pan = params
            .and_then(|p| component_param(p, "trackPan", Some("Pan"), "0"))
            .unwrap_or(0.0);
        let solo = params.and_then(|p| attr(p, "solo")) == Some("true");
        // Track inserts other than the fixed fader, mute, pan and EQ.
        if let Some(p) = params {
            skipped_fx += p
                .children()
                .filter(|c| c.has_tag_name("component"))
                .filter(|c| {
                    !matches!(
                        c.attribute("id"),
                        Some("trackFader" | "trackMute" | "trackPan" | "trackEQ")
                    )
                })
                .filter(|c| c.attribute("powered") != Some("false"))
                .count();
        }
        let mut clips = Vec::new();
        for c in t.children().filter(|n| n.has_tag_name("audioClip")) {
            let Some(&media) = attr(c, "fileID").and_then(|f| files.get(f)) else {
                continue;
            };
            let start = num(c, "startPoint");
            let end = num(c, "endPoint");
            let src_in = num(c, "sourceInPoint");
            let cgain = component_param(c, "clipGain", Some("volume"), "0").unwrap_or(1.0);
            let cmute = component_param(c, "clipMute", Some("mute"), "1").unwrap_or(0.0);
            let fade = |tag: &str| {
                c.children()
                    .find(|n| n.has_tag_name(tag))
                    .map(|f| (num(f, "endPoint") - num(f, "startPoint")).abs())
                    .unwrap_or(0.0)
            };
            let id = out.alloc_id();
            clips.push(Clip {
                id,
                media,
                name: attr(c, "name").unwrap_or("Clip").to_string(),
                start: s(start),
                offset: s(src_in),
                length: s((end - start).max(0.0)),
                gain_db: gain_db(cgain),
                fade_in: s(fade("fadeIn")),
                fade_out: s(fade("fadeOut")),
                muted: cmute >= 0.5,
                ext_id: None,
            });
        }
        let tr = out.track_mut(tid).unwrap();
        tr.name = tname;
        tr.volume_db = gain_db(volume);
        // Stored as -100..100 in some versions, -1..1 in others.
        tr.pan = if pan.abs() > 1.0 {
            (pan / 100.0) as f32
        } else {
            pan as f32
        };
        tr.mute = mute >= 0.5;
        tr.solo = solo;
        tr.clips = clips;
    }
    for m in root.descendants().filter(|n| n.has_tag_name("marker")) {
        let time = m
            .attribute("startPoint")
            .or_else(|| m.attribute("start"))
            .and_then(|v| v.parse::<f64>().ok());
        if let Some(t) = time {
            out.markers.push(Marker {
                name: m.attribute("name").unwrap_or("Marker").to_string(),
                time: s(t),
                length: m
                    .attribute("duration")
                    .and_then(|v| v.parse::<f64>().ok())
                    .map(s)
                    .unwrap_or(0.0),
                ext_id: None,
            });
        }
    }
    if root.descendants().any(|n| n.has_tag_name("busTrack")) {
        notices.push(
            "Bus tracks were not carried over yet; their clips play through the master.".into(),
        );
    }
    if skipped_fx > 0 {
        notices.push(format!(
            "{skipped_fx} track effect(s) were skipped; Audiform27 uses its own effects."
        ));
    }
    if root
        .descendants()
        .any(|n| n.has_tag_name("automationLane") || n.has_tag_name("parameterEnvelope"))
    {
        notices.push("Automation was not carried over yet.".into());
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

    const SESX: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no" ?>
<!DOCTYPE sesx>
<sesx version="1.9">
  <session audioChannelType="stereo" bitDepth="32" duration="441000" sampleRate="44100">
    <tracks>
      <audioTrack id="10002" index="2"><trackParameters><name>Second</name></trackParameters>
        <trackAudioParameters solo="true">
          <component id="trackFader"><parameter index="0" name="volume" parameterValue="0.5"/></component>
          <component id="trackMute"><parameter index="0" parameterValue="1"/><parameter index="1" name="mute" parameterValue="1"/></component>
          <component id="trackPan"><parameter index="0" name="Pan" parameterValue="-50"/></component>
        </trackAudioParameters>
      </audioTrack>
      <audioTrack id="10001" index="1"><trackParameters><name>First</name></trackParameters>
        <trackAudioParameters solo="false"/>
        <audioClip endPoint="88200" fileID="0" name="Take" sourceInPoint="4410" sourceOutPoint="48510" startPoint="44100">
          <component id="clipGain"><parameter index="0" name="volume" parameterValue="2"/></component>
          <component id="clipMute"><parameter index="0" parameterValue="0"/><parameter index="1" name="mute" parameterValue="0"/></component>
          <fadeIn endPoint="4410" startPoint="0" type="log"/>
          <fadeOut endPoint="44100" startPoint="22050" type="log"/>
        </audioClip>
      </audioTrack>
    </tracks>
  </session>
  <files>
    <file absolutePath="C:\nowhere\take.wav" id="0" relativePath="Rec/take.wav"/>
  </files>
</sesx>"#;

    #[test]
    fn reads_a_sesx() {
        let imp = sesx(SESX, Path::new("."), "Test").unwrap();
        let s = &imp.session;
        assert_eq!(s.rate, 44_100);
        assert_eq!(s.tracks.len(), 2);
        assert_eq!(s.tracks[0].name, "First");
        let second = &s.tracks[1];
        assert!(second.mute && second.solo);
        assert!((second.volume_db + 6.02).abs() < 0.01);
        assert!((second.pan + 0.5).abs() < 1e-6);
        let c = &s.tracks[0].clips[0];
        assert!((c.start - 1.0).abs() < 1e-9);
        assert!((c.offset - 0.1).abs() < 1e-9);
        assert!((c.length - 1.0).abs() < 1e-9);
        assert!((c.gain_db - 6.02).abs() < 0.01);
        assert!((c.fade_in - 0.1).abs() < 1e-9);
        assert!((c.fade_out - 0.5).abs() < 1e-9);
        assert_eq!(s.media[0].name, "take");
    }

    #[test]
    fn real_files_on_this_machine_open() {
        // Optional smoke test against the project lead's own sessions.
        for p in [
            r"C:\Users\Nash\Desktop\Untitled Session 125\Untitled Session 125.sesx",
            r"C:\Users\Nash\AppData\Roaming\io.github.nwh212.audiform\autosave\58ff1bfc-d8dc-4a19-ab18-becb7c8e1cfa.audfrm",
        ] {
            let p = Path::new(p);
            if p.exists() {
                let imp = import(p).unwrap();
                assert!(!imp.session.tracks.is_empty());
                assert!(imp.session.tracks.iter().any(|t| !t.clips.is_empty()));
            }
        }
    }
}
