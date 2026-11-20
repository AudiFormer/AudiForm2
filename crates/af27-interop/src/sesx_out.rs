// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Writing a session as a `.sesx`, a copy for the other editor. It is an
//! export, never the session's own file: a `.sesx` cannot hold
//! Audiform27's effects that have no built-in counterpart there, reversed
//! clips, MIDI or 3D Space, and saving to it would quietly lose them.
//! Effects with a counterpart go back as the built-in effect (see
//! [`crate::sesx::builtin`]), kept components as they came, plug-ins
//! with their settings and drawn controls, and volume, pan, mute, send
//! and effect lines on tracks and clips. What is left out is counted in
//! the [`ExportReport`].
//!
//! Units are the ones [`crate::sesx`] reads, so an export reads back the
//! same: positions and source points in samples at the session rate
//! (source points in stretched time), pan -100..100, gains linear.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use af27_core::automation::{Lane, LaneTarget};
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
pub(crate) fn esc(s: &str) -> String {
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

/// A mute, with its drawn line (`keys`) on the "mute" parameter.
fn mute(out: &mut String, g: &mut Guids, id: &str, on: bool, keys: &str) {
    let v = on as u8;
    let _ = write!(
        out,
        r#"<component componentGuid="{}" componentID="Audition.Mute" id="{id}" name="Mute" powered="true"><parameter index="0" parameterValue="{v}"/><parameter index="1" name="mute" parameterValue="{v}">{keys}</parameter></component>"#,
        g.next()
    );
}

/// Keyframes for a mute line (each point holds).
fn hold_keyframes(points: &[(f64, f64)]) -> String {
    keyframes(points).replace(r#"type="linear""#, r#"type="hold""#)
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

/// A slot value's drawn line: (value index, key, points as (file sample,
/// value)).
type SlotLane = (usize, Option<String>, Vec<(f64, f64)>);

/// The playing lanes of `chain`'s slot `s`: (value index, key, points as
/// (file sample, value)). Lane times are seconds after `origin` samples.
fn slot_lanes(s: &EffectSlot, lanes: &[Lane], origin: f64, rate: f64) -> Vec<SlotLane> {
    let Some(id) = s.ext_id.as_deref() else {
        return Vec::new();
    };
    lanes
        .iter()
        .filter(|l| l.plays())
        .filter_map(|l| match &l.target {
            LaneTarget::Effect { slot, param, key } if slot == id => Some((
                *param,
                key.clone(),
                l.points
                    .iter()
                    .map(|p| (origin + p.time * rate, p.value))
                    .collect(),
            )),
            _ => None,
        })
        .collect()
}

/// A plug-in lane named by the control's id (`p<id>`, from Audiform)
/// rather than its number in the plug-in: a `.sesx` cannot place it.
fn by_id(key: &Option<String>) -> bool {
    key.as_deref()
        .is_some_and(|k| af27_core::plugin::param_id(k).is_some())
}

/// Writes a rack. Returns how many of `lanes` (effect lanes) went in.
#[allow(clippy::too_many_arguments)]
fn rack(
    out: &mut String,
    g: &mut Guids,
    chain: &[EffectSlot],
    r: &Rack,
    lanes: &[Lane],
    origin: f64,
    rate: f64,
    report: &mut ExportReport,
) -> usize {
    let writable = |s: &EffectSlot| {
        af27_core::plugin::is_plugin(&s.effect)
            || crate::sesx::is_kept(&s.effect)
            || crate::sesx::builtin::exports(s)
    };
    let slots: Vec<&EffectSlot> = chain
        .iter()
        .filter(|s| {
            let w = writable(s);
            if !w {
                report.effects_left_out += 1;
            }
            w
        })
        .collect();
    let plain = *r == Rack::default();
    if slots.is_empty() && plain {
        return 0;
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
    let mut written = 0usize;
    for s in slots {
        let index = s.position.map_or(next, |p| (p as usize).max(next));
        next = index + 1;
        let powered = !s.bypass && !r.bypass;
        let mine = slot_lanes(s, lanes, origin, rate);
        let component = if crate::sesx::is_kept(&s.effect) {
            // As the file had it, with a fresh component id.
            crate::sesx::kept_xml(s).map(|x| fresh_guid(&x, &g.next()))
        } else if af27_core::plugin::is_plugin(&s.effect) {
            plugin_component(s, &mine, &mut written, g)
        } else {
            let pts: Vec<(usize, Vec<(f64, f64)>)> =
                mine.into_iter().map(|(k, _, p)| (k, p)).collect();
            crate::sesx::builtin::to_component(s, &pts, &g.next()).map(|(x, n)| {
                written += n;
                x
            })
        };
        let Some(component) = component else {
            report.effects_left_out += 1;
            continue;
        };
        let _ = write!(
            out,
            r#"<slot powered="{powered}" slotIndex="{index}">{component}</slot>"#
        );
    }
    out.push_str("</effectsRack>");
    written
}

/// A kept component's XML with its `componentGuid` replaced (a copied
/// clip must not share one).
fn fresh_guid(xml: &str, guid: &str) -> String {
    const KEY: &str = "componentGuid=\"";
    let Some(a) = xml.find(KEY).map(|i| i + KEY.len()) else {
        return xml.to_string();
    };
    let Some(len) = xml[a..].find('"') else {
        return xml.to_string();
    };
    format!("{}{guid}{}", &xml[..a], &xml[a + len..])
}

/// A plug-in's component: its settings chunk and, for each drawn control
/// named by its number in the plug-in, that control's line.
fn plugin_component(
    s: &EffectSlot,
    lanes: &[SlotLane],
    written: &mut usize,
    g: &mut Guids,
) -> Option<String> {
    let data = s.plugin_data();
    let cid = vst3_component(&s.effect, cfg!(windows))?;
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
    let mut params = String::new();
    for (index, key, pts) in lanes {
        if by_id(key) || pts.is_empty() {
            continue;
        }
        let pname = key.clone().unwrap_or_else(|| format!("Control {index}"));
        let _ = write!(
            params,
            r#"<parameter index="{index}" name="{}" parameterValue="{}">{}</parameter>"#,
            esc(&pname),
            pts[0].1,
            keyframes(pts)
        );
        *written += 1;
    }
    Some(format!(
        r#"<component componentGuid="{}" componentID="{cid}" name="{}" presetName="(Custom)"><presetChunkData><![CDATA[{chunk}]]></presetChunkData>{params}</component>"#,
        g.next(),
        esc(&name)
    ))
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
    // Hardware inputs by name, numbered as `<inputPort>`s (1 is the
    // default input).
    let mut inputs: Vec<&str> = Vec::new();
    for t in &s.tracks {
        if let Some(n) = t.input.as_deref().filter(|_| t.kind == TrackKind::Audio) {
            if !inputs.contains(&n) {
                inputs.push(n);
            }
        }
    }
    let mut clip_no = 0usize;
    for (i, t) in s.tracks.iter().enumerate() {
        let is_bus = t.kind == TrackKind::Bus;
        if t.kind == TrackKind::Midi {
            report.midi_tracks += 1;
        }
        let tag = if is_bus { "busTrack" } else { "audioTrack" };
        let _ = write!(
            x,
            r#"      <{tag} automationLaneOpenState="false" id="{}" index="{}"{} select="false" visible="true">"#,
            xml_id[&t.id],
            i + 1,
            if t.prerender && !is_bus {
                r#" preRendered="true""#
            } else {
                ""
            }
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
            r#"<trackOutput outputID="{out_id}" type="trackID"/><trackInput inputID="{}"/>"#,
            match t.input.as_deref().filter(|_| t.kind == TrackKind::Audio) {
                Some(n) => inputs.iter().position(|i| *i == n).map_or(1, |k| k + 2) as i64,
                None if is_bus => -1,
                None => 1,
            }
        );
        let mut lanes_written = 0usize;
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
        let mkeys = lane(LaneTarget::Mute).map_or(String::new(), |l| {
            lanes_written += 1;
            hold_keyframes(
                &l.points
                    .iter()
                    .map(|p| (p.time * rate, p.value.round().clamp(0.0, 1.0)))
                    .collect::<Vec<_>>(),
            )
        });
        lanes_written += lane(LaneTarget::Volume).is_some() as usize;
        lanes_written += lane(LaneTarget::Pan).is_some() as usize;
        mute(&mut x, &mut g, "trackMute", t.mute, &mkeys);
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
        if !t.sends.is_empty() {
            x.push_str("<sendList>");
            for (k, sd) in t.sends.iter().enumerate() {
                let Some(dest) = xml_id.get(&sd.bus) else {
                    continue;
                };
                // Its drawn level and pan lines.
                let id = sd.ext_id.as_deref();
                let send_lane = |f: &dyn Fn(&LaneTarget) -> bool| {
                    t.lanes.iter().find(|l| f(&l.target) && l.plays())
                };
                let lkeys = send_lane(
                    &|tg| matches!(tg, LaneTarget::SendLevel(s) if Some(s.as_str()) == id),
                )
                .map_or(String::new(), |l| {
                    lanes_written += 1;
                    keyframes(
                        &l.points
                            .iter()
                            .map(|p| (p.time * rate, gain(p.value as f32)))
                            .collect::<Vec<_>>(),
                    )
                });
                let pkeys =
                    send_lane(&|tg| matches!(tg, LaneTarget::SendPan(s) if Some(s.as_str()) == id))
                        .map_or(String::new(), |l| {
                            lanes_written += 1;
                            keyframes(
                                &l.points
                                    .iter()
                                    .map(|p| {
                                        (p.time * rate, (p.value.clamp(-1.0, 1.0) + 1.0) / 2.0)
                                    })
                                    .collect::<Vec<_>>(),
                            )
                        });
                let _ = write!(
                    x,
                    r#"<sendSlot preFader="{}" sendOutputID="{dest}" slotIndex="{k}"><component componentGuid="{}" componentID="Audition.Send" id="sendComponent" powered="true"><parameter index="0" name="Power On/Off" parameterValue="{}"/><parameter index="1" name="Level" parameterValue="{}">{lkeys}</parameter></component><component componentGuid="{}" componentID="Audition.StereoPanner" id="sendPan" name="StereoPanner" powered="true"><parameter index="0" name="Pan" parameterValue="{}">{pkeys}</parameter></component></sendSlot>"#,
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
        lanes_written += rack(
            &mut x,
            &mut g,
            &t.chain,
            &t.rack,
            &t.lanes,
            0.0,
            rate,
            &mut report,
        );
        let playing = t.lanes.iter().filter(|l| l.plays()).count();
        report.lanes_left_out += playing.saturating_sub(lanes_written) + t.spatial.lanes.len();
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
    mute(&mut x, &mut g, "trackMute", false, "");
    x.push_str("</trackAudioParameters>");
    let fx_written = rack(
        &mut x,
        &mut g,
        &s.master_chain,
        &s.master_rack,
        &s.master_lanes,
        0.0,
        rate,
        &mut report,
    );
    let master_written = fx_written
        + s.master_lanes
            .iter()
            .any(|l| l.target == LaneTarget::Volume && l.plays()) as usize;
    let master_playing = s.master_lanes.iter().filter(|l| l.plays()).count();
    report.lanes_left_out += master_playing.saturating_sub(master_written);
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
    x.push_str("  </files>\n");
    if !inputs.is_empty() {
        // The inputs the tracks record from, by name; the editor matches
        // them to its own device's ports.
        x.push_str("  <audioDevice>\n    <inputPort id=\"1\" name=\"Default Stereo Input\"/>\n");
        for (k, n) in inputs.iter().enumerate() {
            let _ = writeln!(x, r#"    <inputPort id="{}" name="{}"/>"#, k + 2, esc(n));
        }
        x.push_str("    <outputPort id=\"1\" name=\"Default Output\"/>\n  </audioDevice>\n");
    }
    x.push_str("</sesx>\n");
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
    // Clip lines count from the start of the file, like `sourceInPoint`.
    let lanes = c.cropped_lanes();
    let lane = |target: LaneTarget| lanes.iter().find(|l| l.target == target && l.plays());
    let line = |pts: &[af27_core::automation::LanePoint], f: &dyn Fn(f64) -> f64| {
        keyframes(
            &pts.iter()
                .map(|p| (src_in as f64 + p.time * rate, f(p.value)))
                .collect::<Vec<_>>(),
        )
    };
    match lane(LaneTarget::Volume) {
        Some(l) => {
            let keys = line(&l.points, &|v| gain(v as f32));
            let first = gain(l.points[0].value as f32);
            fader(x, g, "clipGain", first, gain(c.gain_db), &keys);
        }
        None => fader(x, g, "clipGain", gain(c.gain_db), 1.0, ""),
    }
    let mut lanes_written = 0usize;
    let mkeys = lane(LaneTarget::Mute).map_or(String::new(), |l| {
        lanes_written += 1;
        hold_keyframes(
            &l.points
                .iter()
                .map(|p| {
                    (
                        src_in as f64 + p.time * rate,
                        p.value.round().clamp(0.0, 1.0),
                    )
                })
                .collect::<Vec<_>>(),
        )
    });
    lanes_written += lane(LaneTarget::Volume).is_some() as usize;
    lanes_written += lane(LaneTarget::Pan).is_some() as usize;
    mute(x, g, "clipMute", c.muted, &mkeys);
    let (pan_v, pan_keys) = match lane(LaneTarget::Pan) {
        Some(l) if l.points.len() == 1 => (l.points[0].value, String::new()),
        Some(l) => (0.0, line(&l.points, &|v| (v.clamp(-1.0, 1.0) + 1.0) / 2.0)),
        None => (0.0, String::new()),
    };
    let _ = write!(
        x,
        r#"<component componentGuid="{}" componentID="Audition.StereoPanner" id="clipPan" name="StereoPanner" powered="true"><parameter index="0" name="Pan" parameterValue="{}">{pan_keys}</parameter></component>"#,
        g.next(),
        pan_v.clamp(-1.0, 1.0) * 100.0
    );
    lanes_written += rack(
        x,
        g,
        &c.props.chain,
        &c.props.rack,
        &lanes,
        src_in as f64,
        rate,
        report,
    );
    let playing = lanes.iter().filter(|l| l.plays()).count();
    report.lanes_left_out += playing.saturating_sub(lanes_written);
    if let Some(map) = &c.props.channel_map {
        if let Some(text) = channel_map_xml(map) {
            x.push_str(&text);
        } else {
            report.clips_simplified += 1;
        }
    }
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
        r#"<fadeIn crossFadeLinkType="linkedAsymmetric" endPoint="{fin}" shape="{}" startPoint="0" type="{}"/><fadeOut crossFadeLinkType="linkedAsymmetric" endPoint="{len}" shape="{}" startPoint="{}" type="{}"/></audioClip>"#,
        c.props.fade_in_skew.round() as i32,
        if c.props.fade_in_bell {
            "cosine"
        } else {
            "log"
        },
        c.props.fade_out_skew.round() as i32,
        len - fout,
        if c.props.fade_out_bell {
            "cosine"
        } else {
            "log"
        }
    );
}

/// A clip's Source Channels as a `channelMap`: the file channel heard on
/// the left (index 0) and on the right (index 1). None when the map has amounts or flips a `.sesx` cannot hold.
fn channel_map_xml(map: &[af27_core::session::ChannelSend]) -> Option<String> {
    let full = |v: f32| (v - 1.0).abs() < 1e-4;
    let off = |v: f32| v.abs() < 1e-4;
    if map
        .iter()
        .any(|s| !(full(s.left) || off(s.left)) || !(full(s.right) || off(s.right)))
    {
        return None;
    }
    let left: Vec<usize> = (0..map.len()).filter(|&k| full(map[k].left)).collect();
    let right: Vec<usize> = (0..map.len()).filter(|&k| full(map[k].right)).collect();
    // Both sides listed, even when one file channel plays on both: that
    // reads the same whatever the file's channel count.
    let pairs: Vec<(usize, usize)> = match (left.as_slice(), right.as_slice()) {
        ([l], [r]) => vec![(0, *l), (1, *r)],
        _ => return None,
    };
    let mut s = String::from("<channelMap>");
    for (i, k) in pairs {
        let _ = write!(s, r#"<channel index="{i}" sourceIndex="{k}"/>"#);
    }
    s.push_str("</channelMap>");
    Some(s)
}

/// Writes the session to `path` as a `.sesx` (the session itself is not
/// changed and stays tied to its own file).
pub fn export(s: &Session, path: &Path) -> Result<ExportReport, String> {
    let (text, report) = to_sesx(s, path.parent());
    crate::paths::write_atomic(path, text.as_bytes())?;
    Ok(report)
}
