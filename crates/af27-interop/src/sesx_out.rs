// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Writing a session as a `.sesx`, a copy for the other editor. It is an
//! export, never the session's own file: a `.sesx` cannot hold most of
//! Audiform27's effects, clip automation, reversed clips, MIDI or 3D
//! Space, and saving to it would quietly lose them. What is left out is
//! counted in the [`ExportReport`].
//!
//! Units are the ones [`crate::sesx`] reads, so an export reads back the
//! same: positions and source points in samples at the session rate
//! (source points in stretched time), pan -100..100, gains linear.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use af27_core::automation::LaneTarget;
use af27_core::session::{Clip, EffectSlot, Rack, Session, StretchKind, StretchMode, TrackKind};

/// What a `.sesx` export could not hold.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ExportReport {
    /// Effects other than plug-ins (the plug-ins in those chains are
    /// still written).
    pub effects_left_out: usize,
    /// Automation lanes other than track volume and pan.
    pub lanes_left_out: usize,
    /// Reversed or polarity-flipped clips, written as plain clips.
    pub clips_simplified: usize,
    pub markers_left_out: usize,
    pub midi_tracks: usize,
    /// A 3D Space session is written as stereo.
    pub space_left_out: bool,
    /// Clips whose media has no file yet.
    pub clips_without_files: usize,
}

impl ExportReport {
    /// One line for the user (None when nothing was left out).
    pub fn notice(&self) -> Option<String> {
        let mut parts = Vec::new();
        let mut add = |n: usize, what: &str| {
            if n > 0 {
                parts.push(format!("{n} {what}"));
            }
        };
        add(self.effects_left_out, "effect(s) other than plug-ins");
        add(self.lanes_left_out, "automation lane(s)");
        add(
            self.clips_simplified,
            "reversed or inverted clip(s) (written plain)",
        );
        add(self.markers_left_out, "marker(s)");
        add(self.midi_tracks, "MIDI track(s) (written empty)");
        add(self.clips_without_files, "clip(s) with no file yet");
        if self.space_left_out {
            parts.push("3D Space (written as stereo)".into());
        }
        (!parts.is_empty()).then(|| format!("The .sesx could not hold: {}.", parts.join(", ")))
    }
}

/// XML text: `& < > " '` escaped, control characters (other than tab)
/// left out.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' => out.push(c),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

fn gain(db: f32) -> f64 {
    if db <= -96.0 {
        0.0
    } else {
        10f64.powf(db as f64 / 20.0)
    }
}

/// A colour's hue in degrees (-1 for grey).
fn hue(c: [u8; 3]) -> i32 {
    let [r, g, b] = c.map(|x| x as f64 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    if d < 1e-6 {
        return -1;
    }
    let h = if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    h.round() as i32 % 360
}

/// Hands out `componentGuid`s, distinct within one file.
struct Guids(u64);

impl Guids {
    fn next(&mut self) -> String {
        self.0 += 1;
        crate::audfrm::new_uuid()
    }
}

fn fader(out: &mut String, g: &mut Guids, id: &str, volume: f64, static_gain: f64, keys: &str) {
    let _ = write!(
        out,
        r#"<component componentGuid="{}" componentID="Audition.Fader" id="{id}" name="volume" powered="true"><parameter index="0" name="volume" parameterValue="{volume}">{keys}</parameter><parameter index="1" name="static gain" parameterValue="{static_gain}"/></component>"#,
        g.next()
    );
}

fn mute(out: &mut String, g: &mut Guids, id: &str, on: bool) {
    let v = on as u8;
    let _ = write!(
        out,
        r#"<component componentGuid="{}" componentID="Audition.Mute" id="{id}" name="Mute" powered="true"><parameter index="0" parameterValue="{v}"/><parameter index="1" name="mute" parameterValue="{v}"/></component>"#,
        g.next()
    );
}

fn keyframes(points: &[(f64, f64)]) -> String {
    if points.is_empty() {
        return String::new();
    }
    let mut s = String::from("<parameterKeyframes>");
    for (at, v) in points {
        let _ = write!(
            s,
            r#"<parameterKeyframe sampleOffset="{}" type="linear" value="{v}"/>"#,
            at.round().max(0.0) as u64
        );
    }
    s.push_str("</parameterKeyframes>");
    s
}

fn rack(
    out: &mut String,
    g: &mut Guids,
    chain: &[EffectSlot],
    r: &Rack,
    report: &mut ExportReport,
) {
    let plugins: Vec<&EffectSlot> = chain
        .iter()
        .filter(|s| {
            let p = af27_core::plugin::is_plugin(&s.effect);
            if !p {
                report.effects_left_out += 1;
            }
            p
        })
        .collect();
    let plain = *r == Rack::default();
    if plugins.is_empty() && plain {
        return;
    }
    let level = |db: f32| 10f64.powf(db as f64 / 20.0);
    let _ = write!(
        out,
        r#"<effectsRack preFader="true"><component id="rackInput" powered="true"><parameter index="0" parameterValue="{}"/></component><component id="rackOutput" powered="true"><parameter index="0" parameterValue="{}"/></component><component id="rackMix" powered="true"><parameter index="0" parameterValue="{}"/></component>"#,
        level(r.input_db),
        level(r.output_db),
        (1.0 - r.mix.clamp(0.0, 1.0)) as f64
    );
    let mut next = 0usize;
    for s in plugins {
        let index = s.position.map_or(next, |p| (p as usize).max(next));
        next = index + 1;
        let data = s.plugin_data();
        let Some(cid) = vst3_component(&s.effect, cfg!(windows)) else {
            continue;
        };
        let chunk = data
            .state
            .as_deref()
            .and_then(crate::b64::decode)
            .and_then(|st| crate::sesx::state_to_chunk(&st))
            .map(|c| crate::b64::encode(&c))
            .unwrap_or_default();
        let name = if data.name.is_empty() {
            "Plug-in".to_string()
        } else {
            data.name.clone()
        };
        let _ = write!(
            out,
            r#"<slot powered="{}" slotIndex="{index}"><component componentGuid="{}" componentID="{cid}" name="{}" presetName="(Custom)"><presetChunkData><![CDATA[{chunk}]]></presetChunkData></component></slot>"#,
            !s.bypass && !r.bypass,
            g.next(),
            esc(&name)
        );
    }
    out.push_str("</effectsRack>");
}

/// The other way from [`crate::sesx::vst3_effect`]: `vst3:<32 hex>` as
/// `3:aaaaaaaa bbbbbbbb cccccccc dddddddd`.
pub fn vst3_component(effect: &str, com: bool) -> Option<String> {
    let hex = effect.strip_prefix(af27_core::plugin::PREFIX)?;
    if hex.len() != 32 {
        return None;
    }
    let b: Vec<u8> = (0..16)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok())
        .collect::<Option<_>>()?;
    let be = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let (a, w2) = if com {
        let a = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let hi = u16::from_le_bytes([b[4], b[5]]) as u32;
        let lo = u16::from_le_bytes([b[6], b[7]]) as u32;
        (a, hi << 16 | lo)
    } else {
        (be(0), be(4))
    };
    Some(format!("3:{a:08x} {w2:08x} {:08x} {:08x}", be(8), be(12)))
}

/// The session as `.sesx` text. `dir` is the folder it will be written
/// to (relative media paths start there).
pub fn to_sesx(s: &Session, dir: Option<&Path>) -> (String, ExportReport) {
    let mut report = ExportReport {
        markers_left_out: s.markers.len(),
        space_left_out: s.space.is_some(),
        ..Default::default()
    };
    let rate = s.rate as f64;
    let at = |t: f64| (t * rate).round().max(0.0) as u64;
    let mut g = Guids(0);
    // Only media that clips use, numbered in order of first use.
    let mut file_ids: HashMap<u64, usize> = HashMap::new();
    let mut files: Vec<u64> = Vec::new();
    for c in s.tracks.iter().flat_map(|t| &t.clips) {
        let has_file = s.media(c.media).is_some_and(|m| m.path.is_some());
        if has_file && !file_ids.contains_key(&c.media) {
            file_ids.insert(c.media, files.len());
            files.push(c.media);
        }
    }
    let xml_id: HashMap<u64, usize> = s
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id, 10_001 + i))
        .collect();
    let bits = match s.bit_depth {
        16 | 24 => s.bit_depth,
        _ => 32,
    };
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\" ?>\n<!DOCTYPE sesx>\n<sesx version=\"1.9\">\n");
    let _ = writeln!(
        x,
        r#"  <session audioChannelType="stereo" bitDepth="{bits}" duration="{}" sampleRate="{}">"#,
        at(s.duration()).max(1),
        s.rate
    );
    x.push_str("    <tracks>\n");
    let mut clip_no = 0usize;
    for (i, t) in s.tracks.iter().enumerate() {
        let is_bus = t.kind == TrackKind::Bus;
        if t.kind == TrackKind::Midi {
            report.midi_tracks += 1;
        }
        let tag = if is_bus { "busTrack" } else { "audioTrack" };
        let _ = write!(
            x,
            r#"      <{tag} automationLaneOpenState="false" id="{}" index="{}" select="false" visible="true">"#,
            xml_id[&t.id],
            i + 1
        );
        let _ = write!(
            x,
            r#"<trackParameters trackHeight="{}" trackHue="{}" trackMinimized="false"><name>{}</name></trackParameters>"#,
            t.height.round() as u32,
            hue(t.color),
            esc(&t.name)
        );
        let _ = write!(
            x,
            r#"<trackAudioParameters audioChannelType="{}" automationMode="1" monitoring="false" recordArmed="{}" solo="{}" soloSafe="{}">"#,
            if t.mono { "mono" } else { "stereo" },
            t.armed && !is_bus,
            t.solo,
            t.solo_safe
        );
        let out_id = t
            .output
            .and_then(|b| xml_id.get(&b))
            .copied()
            .unwrap_or(10_000);
        let _ = write!(
            x,
            r#"<trackOutput outputID="{out_id}" type="trackID"/><trackInput inputID="-1"/>"#
        );
        // Volume and pan, with their lines.
        let lane = |target: LaneTarget| t.lanes.iter().find(|l| l.target == target && l.plays());
        let (volume, sg, vkeys) = match lane(LaneTarget::Volume) {
            Some(l) => (
                gain(l.points[0].value as f32),
                gain(t.volume_db),
                keyframes(
                    &l.points
                        .iter()
                        .map(|p| (p.time * rate, gain(p.value as f32)))
                        .collect::<Vec<_>>(),
                ),
            ),
            None => (gain(t.volume_db), 1.0, String::new()),
        };
        fader(&mut x, &mut g, "trackFader", volume, sg, &vkeys);
        mute(&mut x, &mut g, "trackMute", t.mute);
        let pkeys = lane(LaneTarget::Pan).map_or(String::new(), |l| {
            keyframes(
                &l.points
                    .iter()
                    .map(|p| (p.time * rate, (p.value.clamp(-1.0, 1.0) + 1.0) / 2.0))
                    .collect::<Vec<_>>(),
            )
        });
        let _ = write!(
            x,
            r#"<component componentGuid="{}" componentID="Audition.StereoPanner" id="trackPan" name="StereoPanner" powered="true"><parameter index="0" name="Pan" parameterValue="{}">{pkeys}</parameter></component>"#,
            g.next(),
            (t.pan.clamp(-1.0, 1.0) * 100.0) as f64
        );
        report.lanes_left_out += t
            .lanes
            .iter()
            .filter(|l| !matches!(l.target, LaneTarget::Volume | LaneTarget::Pan) && l.plays())
            .count()
            + t.spatial.lanes.len();
        if !t.sends.is_empty() {
            x.push_str("<sendList>");
            for (k, sd) in t.sends.iter().enumerate() {
                let Some(dest) = xml_id.get(&sd.bus) else {
                    continue;
                };
                let _ = write!(
                    x,
                    r#"<sendSlot preFader="{}" sendOutputID="{dest}" slotIndex="{k}"><component componentGuid="{}" componentID="Audition.Send" id="sendComponent" powered="true"><parameter index="0" name="Power On/Off" parameterValue="{}"/><parameter index="1" name="Level" parameterValue="{}"/></component><component componentGuid="{}" componentID="Audition.StereoPanner" id="sendPan" name="StereoPanner" powered="true"><parameter index="0" name="Pan" parameterValue="{}"/></component></sendSlot>"#,
                    sd.pre_fader,
                    g.next(),
                    sd.enabled as u8,
                    gain(sd.level_db),
                    g.next(),
                    (sd.pan * 100.0) as f64
                );
            }
            x.push_str("</sendList>");
        }
        x.push_str("</trackAudioParameters>");
        rack(&mut x, &mut g, &t.chain, &t.rack, &mut report);
        for (z, c) in t.clips.iter().enumerate() {
            let Some(&fid) = file_ids.get(&c.media) else {
                report.clips_without_files += 1;
                continue;
            };
            write_clip(&mut x, &mut g, c, fid, clip_no, z, rate, &mut report);
            clip_no += 1;
        }
        let _ = writeln!(x, "</{tag}>");
    }
    // The master.
    x.push_str(r#"      <masterTrack automationLaneOpenState="false" id="10000" index=""#);
    let _ = write!(
        x,
        r#"{}" select="false" visible="true"><trackParameters trackHeight="72" trackHue="-1" trackMinimized="false"><name>Mix</name></trackParameters><trackAudioParameters audioChannelType="stereo" automationMode="1" monitoring="false" recordArmed="false" solo="false" soloSafe="true"><trackOutput outputID="1" type="hardwareOutput"/>"#,
        s.tracks.len() + 1
    );
    let (volume, sg, keys) = match s
        .master_lanes
        .iter()
        .find(|l| l.target == LaneTarget::Volume && l.plays())
    {
        Some(l) => (
            gain(l.points[0].value as f32),
            gain(s.master_db),
            keyframes(
                &l.points
                    .iter()
                    .map(|p| (p.time * rate, gain(p.value as f32)))
                    .collect::<Vec<_>>(),
            ),
        ),
        None => (gain(s.master_db), 1.0, String::new()),
    };
    fader(&mut x, &mut g, "trackFader", volume, sg, &keys);
    mute(&mut x, &mut g, "trackMute", false);
    x.push_str("</trackAudioParameters>");
    rack(&mut x, &mut g, &s.master_chain, &s.master_rack, &mut report);
    x.push_str("</masterTrack>\n    </tracks>\n");
    let cti = at(s.playhead);
    let _ = writeln!(
        x,
        r#"    <sessionState ctiPosition="{cti}" smpteStart="0"><selectionState selectionDuration="0" selectionStart="{cti}"/></sessionState>"#
    );
    x.push_str("  </session>\n  <files>\n");
    for (i, mid) in files.iter().enumerate() {
        let m = s.media(*mid).unwrap();
        let p = m.path.as_deref().unwrap();
        let rel = dir
            .and_then(|d| crate::paths::relative(p, d))
            .map(|r| format!(r#" relativePath="{}""#, esc(&r)))
            .unwrap_or_default();
        let _ = writeln!(
            x,
            r#"    <file absolutePath="{}" id="{i}"{rel}/>"#,
            esc(&p.to_string_lossy())
        );
    }
    x.push_str("  </files>\n</sesx>\n");
    (x, report)
}

#[allow(clippy::too_many_arguments)]
fn write_clip(
    x: &mut String,
    g: &mut Guids,
    c: &Clip,
    fid: usize,
    id: usize,
    z: usize,
    rate: f64,
    report: &mut ExportReport,
) {
    let at = |t: f64| (t * rate).round().max(0.0) as u64;
    let start = at(c.start);
    let len = at(c.length).max(1);
    let st = &c.props.stretch;
    let ratio = st.applied_ratio();
    let src_in = (c.offset * ratio * rate).round().max(0.0) as u64;
    if c.props.reversed || c.props.inverted {
        report.clips_simplified += 1;
    }
    let hue_v = c.props.color.map_or(-1, hue);
    let _ = write!(
        x,
        r#"<audioClip clipAutoCrossfade="true" crossFadeHeadClipID="-1" crossFadeTailClipID="-1" endPoint="{}" fileID="{fid}" hue="{hue_v}" id="{id}" lockedInTime="{}" looped="false" name="{}" offline="false" select="false" sourceInPoint="{src_in}" sourceOutPoint="{}" startPoint="{start}" zOrder="{z}">"#,
        start + len,
        c.props.locked,
        esc(&c.name),
        src_in + len
    );
    fader(x, g, "clipGain", gain(c.gain_db), 1.0, "");
    mute(x, g, "clipMute", c.muted);
    if st.is_on() {
        let kind = match st.kind {
            StretchKind::Varispeed => "varispeed",
            StretchKind::Polyphonic => "normal",
            StretchKind::Monophonic => "solo",
        };
        let pitch = if st.kind == StretchKind::Varispeed {
            0.0
        } else {
            st.pitch
        };
        let _ = write!(
            x,
            r#"<clipStretch adaptiveWindowSize="{}" pitchAdjustment="{pitch}" preserveFormants="{}" stretchMode="{}" stretchQuality="{}" stretchRatio="{ratio}" stretchType="{kind}" transientSensitivity="{}"/>"#,
            st.window_ms.round(),
            st.preserve_formants,
            if st.mode == StretchMode::Rendered {
                "rendered"
            } else {
                "realtime"
            },
            if st.high_precision { "high" } else { "medium" },
            st.transients.round()
        );
    }
    let fin = at(c.fade_in).min(len);
    let fout = at(c.fade_out).min(len);
    let _ = write!(
        x,
        r#"<fadeIn crossFadeLinkType="linkedAsymmetric" endPoint="{fin}" shape="{}" startPoint="0" type="log"/><fadeOut crossFadeLinkType="linkedAsymmetric" endPoint="{len}" shape="{}" startPoint="{}" type="log"/></audioClip>"#,
        c.props.fade_in_skew.round() as i32,
        c.props.fade_out_skew.round() as i32,
        len - fout
    );
}

/// Writes the session to `path` as a `.sesx` (the session itself is not
/// changed and stays tied to its own file).
pub fn export(s: &Session, path: &Path) -> Result<ExportReport, String> {
    let (text, report) = to_sesx(s, path.parent());
    crate::paths::write_atomic(path, text.as_bytes())?;
    Ok(report)
}
