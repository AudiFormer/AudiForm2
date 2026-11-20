// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Audiform sessions (`.audfrm`, and the older `.altaudio`): read and
//! written losslessly, so a session can go back and forth between
//! Audiform and Audiform27.
//!
//! How it stays lossless: every object Audiform27 reads keeps its id from
//! the file (`ext_id`). Saving starts from the document that was opened
//! (the "base") and, for each object, takes the original by its id and
//! writes only the fields Audiform27 owns back into it. Everything else
//! (automation envelopes, slot routing and presets, the
//! view, libraries, playlist, episode metadata, fields added by later
//! Audiform versions) passes through untouched. New objects get every
//! field Audiform requires.
//!
//! Effects: Audiform's own effects that Audiform27 also has (`altaudio.X`
//! with an `af.X` twin) load into Audiform27 and are written back with
//! their saved settings. Others stay in the chain, in place, silent here
//! and saved back as they were. Audiform27-only effects are saved as
//! `af27.<id>` slots, which Audiform skips when playing but keeps when it
//! saves. VST3 plug-in slots (`vst3:<class id>`) keep the same id; their
//! controls (`params`, `p<id>` keys) and settings (`plugin_state`) live in
//! the slot's data as `af27_core::plugin::PluginData`, and go back only
//! when they changed.
//!
//! Units: positions are samples at the session rate (clip start, fades,
//! markers), source points are samples at the file's own rate, MIDI is in
//! beats.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use af27_core::automation::{AutomationMode, Curve, Lane, LanePoint as AutoPoint, LaneTarget};
use af27_core::midi::{ControlEvent, MidiClip, MidiNote, SoundFontRef, TempoChange};
use af27_core::plugin::PluginData;
use af27_core::session::{
    Clip, ClipProps, ClipStretch, EffectSlot, Marker, MediaItem, Rack, Send, Session, StretchKind,
    StretchMode, TrackId, TrackKind,
};
use af27_core::space::{
    LaneCurve, LanePoint, Monitor, SpaceLane, SpaceLayout, SpaceParam, SpacePos, SpatializeMode,
};
use serde_json::{json, Map, Value};

use crate::Imported;

pub const FORMAT: &str = "altaudio.session";
const AF27: &str = "af27.";

/// A fresh random (version 4) UUID.
pub fn new_uuid() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = t
        ^ COUNTER.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed)
        ^ (std::process::id() as u64) << 32;
    let mut next = || {
        // splitmix64
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    let (a, b) = (next(), next());
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&a.to_le_bytes());
    bytes[8..].copy_from_slice(&b.to_le_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn now_iso() -> String {
    // Seconds since the epoch as an RFC 3339 UTC time, without a date crate.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil from days (Howard Hinnant's public algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

fn layout_from(s: &str) -> Option<SpaceLayout> {
    SpaceLayout::from_label(s)
}

fn listen_from(sp: &Value) -> Monitor {
    match sp["listen"].as_str() {
        Some("binaural") => Monitor::Binaural,
        Some("speakers") => Monitor::Speakers,
        _ => Monitor::StereoFold,
    }
}

fn listen_key(m: Monitor) -> &'static str {
    match m {
        Monitor::Binaural => "binaural",
        Monitor::StereoFold => "stereo",
        Monitor::Speakers => "speakers",
    }
}

/// A spot as the file has it (missing values take Audiform's defaults).
/// Kept as read, untidied, so an unchanged spot saves the same.
fn pos_from(p: &Value) -> SpacePos {
    let d = SpacePos::default();
    let f = |k: &str, or: f64| p[k].as_f64().unwrap_or(or);
    SpacePos {
        x: f("x", d.x),
        y: f("y", d.y),
        z: f("z", d.z),
        size: f("size", d.size),
        width: f("width", d.width),
        rotation: f("rotation", d.rotation),
        lfe: f("lfe", d.lfe),
        spread: f("spread", d.spread),
        spatialize: f("spatialize", d.spatialize),
        spatialize_mode: p["spatialize_mode"]
            .as_str()
            .map_or(d.spatialize_mode, SpatializeMode::from_key),
    }
}

/// Writes a spot into `o` (the file's spot object, or a new one): only
/// values that differ from what `o` already says. A new spot gets the
/// values Audiform always writes (x, y, z, size, width); values it leaves
/// out at their defaults (rotation, lfe, spread, spatialize and the
/// method) are left out again when set back to the default.
fn write_pos(o: &mut Map<String, Value>, p: &SpacePos) {
    let had = pos_from(&Value::Object(o.clone()));
    let fresh = o.is_empty();
    for (key, now, was, always) in [
        ("x", p.x, had.x, true),
        ("y", p.y, had.y, true),
        ("z", p.z, had.z, true),
        ("size", p.size, had.size, true),
        ("width", p.width, had.width, true),
        ("rotation", p.rotation, had.rotation, false),
        ("lfe", p.lfe, had.lfe, false),
        ("spread", p.spread, had.spread, false),
        ("spatialize", p.spatialize, had.spatialize, false),
    ] {
        if now == was && !(fresh && always) {
            continue;
        }
        if !always && now == 0.0 {
            o.remove(key);
        } else {
            o.insert(key.into(), json!(now));
        }
    }
    if p.spatialize_mode != had.spatialize_mode {
        if p.spatialize_mode == SpatializeMode::default() {
            o.remove("spatialize_mode");
        } else {
            o.insert("spatialize_mode".into(), json!(p.spatialize_mode.key()));
        }
    }
}

/// A track's or bus's spot automation, from its envelopes.
fn lanes_from(t: &Value, rate: u32) -> Vec<SpaceLane> {
    t["envelopes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["target"]["type"].as_str() == Some("space"))
        .filter_map(|e| {
            let param = SpaceParam::from_key(e["target"]["param"].as_str()?)?;
            let points = e["points"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| LanePoint {
                    t: p["position"].as_f64().unwrap_or(0.0) / rate as f64,
                    v: p["value"].as_f64().unwrap_or(0.0),
                    curve: if p["curve"].as_str() == Some("hold") {
                        LaneCurve::Hold
                    } else {
                        LaneCurve::Linear
                    },
                })
                .collect();
            Some(SpaceLane {
                param,
                points,
                enabled: e["enabled"].as_bool().unwrap_or(true),
            })
        })
        .collect()
}

fn hex_color(v: &Value) -> Option<[u8; 3]> {
    let s = v.as_str()?.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some([p(0)?, p(2)?, p(4)?])
}

/// A clip's stretch as the file has it. Absent, null, or on a looped clip
/// (Audiform ignores it there): off. `ratio` is timeline length over the
/// source's (as Audiform27's); `keep_pitch: false` is varispeed.
fn stretch_from(c: &Value) -> ClipStretch {
    let v = &c["stretch"];
    let d = ClipStretch::default();
    if !v.is_object() || !c["looped"].is_null() {
        return d;
    }
    let adv = &v["advanced"];
    let f32_or = |x: &Value, or: f32| x.as_f64().map_or(or, |x| x as f32);
    ClipStretch {
        mode: if v["rendered"].as_bool().unwrap_or(false) {
            StretchMode::Rendered
        } else {
            StretchMode::Live
        },
        kind: if !v["keep_pitch"].as_bool().unwrap_or(true) {
            StretchKind::Varispeed
        } else if v["polyphonic"].as_bool().unwrap_or(false) {
            StretchKind::Polyphonic
        } else {
            StretchKind::Monophonic
        },
        ratio: v["ratio"].as_f64().filter(|r| *r > 0.0).unwrap_or(1.0),
        pitch: f32_or(&v["pitch"], 0.0),
        window_ms: f32_or(&adv["window_ms"], d.window_ms),
        transients: f32_or(&adv["transient_sensitivity"], d.transients),
        preserve_formants: adv["preserve_formants"]
            .as_bool()
            .unwrap_or(d.preserve_formants),
        high_precision: adv["high_precision"].as_bool().unwrap_or(d.high_precision),
    }
}

/// A fade's skew (-100..100) from Audiform's shape and curve bend: the
/// linear shape bent by `curve` maps straight across; the other shapes
/// map to the skew closest in feel, plus their bend.
fn skew_from(fade: &Value) -> f32 {
    let base = match fade["shape"].as_str() {
        Some("logarithmic") => 50.0,
        Some("exponential") => -50.0,
        _ => 0.0,
    };
    let curve = fade["curve"].as_f64().unwrap_or(0.0) * 100.0;
    ((base + curve) as f32).clamp(-100.0, 100.0)
}

/// Everything in [`ClipProps`] as the file has it (also what a saved
/// clip's original implies, so only changes are written back).
fn props_from(c: &Value) -> ClipProps {
    ClipProps {
        stretch: stretch_from(c),
        reversed: c["reversed"].as_bool().unwrap_or(false),
        inverted: c["inverted"].as_bool().unwrap_or(false),
        locked: c["locked"].as_bool().unwrap_or(false),
        color: hex_color(&c["color"]),
        fade_in_skew: skew_from(&c["fade_in"]),
        fade_out_skew: skew_from(&c["fade_out"]),
        // A Source Channels mix that does not pass through to speakers.
        fold_channels: c["channel_mix"].is_object()
            && !c["channel_mix"]["to_speakers"].as_bool().unwrap_or(false),
    }
}

/// A float setting as JSON, rounded to six places (so 0.1 stays 0.1 and
/// does not become 0.10000000149).
fn short(x: f32) -> Value {
    json!((x as f64 * 1e6).round() / 1e6)
}

/// Writes the clip settings that differ from what `co` (the original
/// clip, or a new one) already says.
fn write_props(co: &mut Map<String, Value>, p: &ClipProps) {
    let had = props_from(&Value::Object(co.clone()));
    for (key, now, was) in [
        ("reversed", p.reversed, had.reversed),
        ("inverted", p.inverted, had.inverted),
        ("locked", p.locked, had.locked),
    ] {
        // `locked` is required, so a new clip always gets it.
        if now != was || (key == "locked" && !co.contains_key(key)) {
            co.insert(key.into(), json!(now));
        }
    }
    if p.color != had.color {
        co.insert(
            "color".into(),
            match p.color {
                Some([r, g, b]) => json!(format!("#{r:02x}{g:02x}{b:02x}")),
                None => Value::Null,
            },
        );
    }
    for (key, now, was) in [
        ("fade_in", p.fade_in_skew, had.fade_in_skew),
        ("fade_out", p.fade_out_skew, had.fade_out_skew),
    ] {
        if now != was {
            if let Some(f) = co.get_mut(key).and_then(Value::as_object_mut) {
                f.insert("shape".into(), json!("linear"));
                f.insert("curve".into(), short(now / 100.0));
            }
        }
    }
    let st = &p.stretch;
    if *st == had.stretch {
        return;
    }
    if st.mode == StretchMode::Off {
        if co.contains_key("stretch") {
            co.insert("stretch".into(), Value::Null);
        }
        return;
    }
    let mut so = obj(co.get("stretch").cloned().unwrap_or(Value::Null));
    set_num(&mut so, "ratio", st.ratio);
    let vari = st.kind == StretchKind::Varispeed;
    so.insert("keep_pitch".into(), json!(!vari));
    if !vari {
        so.insert("pitch".into(), short(st.pitch));
        so.insert(
            "polyphonic".into(),
            json!(st.kind == StretchKind::Polyphonic),
        );
    }
    so.insert("rendered".into(), json!(st.mode == StretchMode::Rendered));
    let mut adv = obj(so.get("advanced").cloned().unwrap_or(Value::Null));
    adv.insert("window_ms".into(), short(st.window_ms));
    adv.insert("transient_sensitivity".into(), short(st.transients));
    adv.insert("preserve_formants".into(), json!(st.preserve_formants));
    adv.insert("high_precision".into(), json!(st.high_precision));
    so.insert("advanced".into(), Value::Object(adv));
    co.insert("stretch".into(), Value::Object(so));
}

/// `altaudio.X` -> `af.X` when Audiform27 has the effect.
fn fx_id(audiform_id: &str) -> Option<String> {
    let name = audiform_id.strip_prefix("altaudio.")?;
    let id = format!("af.{name}");
    af27_fx::info(&id).map(|_| id)
}

fn slot_from(v: &Value) -> EffectSlot {
    let effect = v["effect"].as_str().unwrap_or_default().to_string();
    let enabled = v["enabled"].as_bool().unwrap_or(true);
    let ext_id = v["id"].as_str().map(str::to_string);
    let data = match &v["data"] {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    };
    if let Some(id) = fx_id(&effect) {
        let info = af27_fx::info(&id).unwrap();
        let values = info
            .params
            .iter()
            .map(|p| v["params"][p.key].as_f64().unwrap_or(p.default) as f32)
            .collect();
        return EffectSlot {
            sidechain: None,
            effect: id,
            values,
            bypass: !enabled,
            data,
            ext_id,
        };
    }
    if af27_core::plugin::is_plugin(&effect) {
        return EffectSlot {
            sidechain: None,
            data: Some(plugin_data_from(v, ext_id.as_deref()).to_json()),
            effect,
            values: Vec::new(),
            bypass: !enabled,
            ext_id,
        };
    }
    if let Some(own) = effect.strip_prefix(AF27) {
        let values = v["params"]["values"]
            .as_array()
            .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect())
            .unwrap_or_default();
        return EffectSlot {
            sidechain: None,
            effect: own.to_string(),
            values,
            bypass: !enabled,
            data,
            ext_id,
        };
    }
    // An effect Audiform27 does not have: kept by its own id, silent here.
    EffectSlot {
        sidechain: None,
        effect,
        values: Vec::new(),
        bypass: !enabled,
        data,
        ext_id,
    }
}

/// A plug-in slot's controls and settings as the file has them. Its
/// running copy is named after the slot's id in the file.
fn plugin_data_from(v: &Value, ext_id: Option<&str>) -> PluginData {
    PluginData {
        inst: ext_id.map_or_else(af27_core::plugin::new_inst, str::to_string),
        params: plugin_params(v),
        state: v["plugin_state"].as_str().map(str::to_string),
        ..Default::default()
    }
}

fn plugin_params(v: &Value) -> BTreeMap<String, f64> {
    v["params"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, x)| Some((k.clone(), x.as_f64()?)))
                .collect()
        })
        .unwrap_or_default()
}

/// Writes a plug-in slot's controls and settings, each only when it
/// differs from what the slot already holds (so an unchanged file keeps
/// its exact text).
fn write_plugin(o: &mut Map<String, Value>, s: &EffectSlot) {
    if o.get("effect").and_then(Value::as_str) != Some(s.effect.as_str()) {
        o.insert("effect".into(), json!(s.effect));
    }
    let d = s.plugin_data();
    let had = Value::Object(o.clone());
    if plugin_params(&had) != d.params || !had["params"].is_object() {
        let m: Map<String, Value> = d
            .params
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        o.insert("params".into(), Value::Object(m));
    }
    let had_state = had["plugin_state"].as_str();
    if had_state != d.state.as_deref() {
        match &d.state {
            Some(st) => {
                o.insert("plugin_state".into(), json!(st));
            }
            None => {
                if o.contains_key("plugin_state") {
                    o.insert("plugin_state".into(), Value::Null);
                }
            }
        }
    }
}

/// A chain's power, input and output gain and mix.
fn rack_from(v: &Value) -> Rack {
    Rack {
        bypass: !v["enabled"].as_bool().unwrap_or(true),
        input_db: v["input_gain_db"].as_f64().unwrap_or(0.0) as f32,
        output_db: v["output_gain_db"].as_f64().unwrap_or(0.0) as f32,
        mix: v["mix"].as_f64().unwrap_or(1.0) as f32,
    }
}

/// Points each slot's sidechain at its track or bus, by the file's ids
/// (`chain` was read from `v` slot for slot). An id that names nothing
/// leaves the slot listening to itself.
fn resolve_sidechains(chain: &mut [EffectSlot], v: &Value, by_uuid: &HashMap<String, TrackId>) {
    let Some(slots) = v["slots"].as_array() else {
        return;
    };
    for (slot, sv) in chain.iter_mut().zip(slots) {
        slot.sidechain = sv["sidechain"]
            .as_str()
            .and_then(|u| by_uuid.get(u))
            .copied();
    }
}

/// The fields of a track or bus object Audiform27 reads (also used to
/// tell, on saving, which of them changed since the file was read).
#[derive(Debug, Clone, PartialEq)]
struct Strip {
    volume_db: f32,
    pan: f32,
    muted: bool,
    solo: bool,
    mono: bool,
    polarity: bool,
    solo_safe: bool,
    automation: AutomationMode,
    show_lanes: bool,
}

fn strip_from(t: &Value) -> Strip {
    let flag = |k: &str| t[k].as_bool().unwrap_or(false);
    Strip {
        volume_db: t["volume_db"].as_f64().unwrap_or(0.0) as f32,
        // Audiform pans -100..100; Audiform27 -1..1.
        pan: (t["pan"].as_f64().unwrap_or(0.0) / 100.0).clamp(-1.0, 1.0) as f32,
        muted: flag("muted"),
        solo: flag("solo"),
        mono: flag("mono"),
        polarity: flag("polarity_inverted"),
        solo_safe: flag("af27_solo_safe"),
        automation: t
            .get("automation_mode")
            .and_then(|m| serde_json::from_value(m.clone()).ok())
            .unwrap_or_default(),
        show_lanes: flag("show_automation"),
    }
}

/// Writes the strip fields that differ from what `o` (the original, or
/// an empty object for a new one) already says. Required fields are
/// always present; optional ones Audiform leaves out at their default.
fn write_strip(o: &mut Map<String, Value>, now: &Strip, is_bus: bool) {
    let was = strip_from(&Value::Object(o.clone()));
    let fresh = !o.contains_key("volume_db");
    if fresh || now.volume_db != was.volume_db {
        put_f32(o, "volume_db", now.volume_db);
    }
    if fresh || now.pan != was.pan {
        put_f32(o, "pan", now.pan * 100.0);
    }
    for (key, n, w) in [
        ("muted", now.muted, was.muted),
        ("solo", now.solo, was.solo),
    ] {
        if fresh || n != w {
            o.insert(key.into(), json!(n));
        }
    }
    let mut optional = vec![
        ("mono", now.mono, was.mono),
        ("af27_solo_safe", now.solo_safe, was.solo_safe),
    ];
    if !is_bus {
        optional.push(("polarity_inverted", now.polarity, was.polarity));
        optional.push(("show_automation", now.show_lanes, was.show_lanes));
    }
    for (key, n, w) in optional {
        if n != w {
            if n {
                o.insert(key.into(), json!(true));
            } else {
                o.remove(key);
            }
        }
    }
    if now.automation != was.automation {
        if now.automation == AutomationMode::Read {
            o.remove("automation_mode");
        } else {
            o.insert(
                "automation_mode".into(),
                serde_json::to_value(now.automation).unwrap_or(Value::Null),
            );
        }
    }
}

/// An f32 setting as the shortest decimal that reads back the same.
fn put_f32(o: &mut Map<String, Value>, key: &str, v: f32) {
    let x: f64 = format!("{v}").parse().unwrap_or(v as f64);
    o.insert(key.into(), json!(x));
}

/// The param index of `key` in the effect `slot_id` names in `chain`.
fn param_index(chain: &[EffectSlot], slot_id: &str, key: &str) -> usize {
    let Some(slot) = chain.iter().find(|s| s.ext_id.as_deref() == Some(slot_id)) else {
        return usize::MAX;
    };
    if let Some(info) = af27_fx::info(&slot.effect) {
        if let Some(i) = info.params.iter().position(|p| p.key == key) {
            return i;
        }
    }
    key.strip_prefix("values.")
        .and_then(|n| n.parse().ok())
        .unwrap_or(usize::MAX)
}

/// A track's, bus's or the master's automation lanes. Positions are
/// session samples; pan lanes hold -100..100.
fn auto_lanes_from(envs: &Value, rate: f64, chain: &[EffectSlot]) -> Vec<Lane> {
    let pan = |t: &LaneTarget, v: f64| {
        if *t == LaneTarget::Pan {
            v / 100.0
        } else {
            v
        }
    };
    envs.as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            let tg = &e["target"];
            let target = match tg["type"].as_str() {
                Some("volume") => LaneTarget::Volume,
                Some("pan") => LaneTarget::Pan,
                Some("effect_param") => {
                    let slot = tg["slot_id"].as_str().unwrap_or_default().to_string();
                    let key = tg["param"].as_str().unwrap_or_default().to_string();
                    LaneTarget::Effect {
                        param: param_index(chain, &slot, &key),
                        slot,
                        key: Some(key),
                    }
                }
                _ => LaneTarget::Space(tg["param"].as_str().unwrap_or_default().to_string()),
            };
            let points = e["points"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| AutoPoint {
                    time: p["position"].as_f64().unwrap_or(0.0) / rate,
                    value: pan(&target, p["value"].as_f64().unwrap_or(0.0)),
                    curve: match p["curve"].as_str() {
                        Some("hold") => Curve::Hold,
                        Some("spline") => Curve::Spline,
                        _ => Curve::Linear,
                    },
                })
                .collect();
            Lane {
                enabled: e["enabled"].as_bool().unwrap_or(true),
                points,
                height: e["height"].as_u64().map(|h| h as u32),
                ext_id: e["id"].as_str().map(str::to_string),
                target,
            }
        })
        .collect()
}

/// Writes lanes back as envelopes, unless they read the same as `base`
/// (then `base` is kept as it was).
fn auto_lanes_to(
    lanes: &mut [Lane],
    base: Option<&Value>,
    rate: f64,
    chain: &[EffectSlot],
) -> Value {
    if let Some(b) = base.filter(|b| b.is_array()) {
        if auto_lanes_from(b, rate, chain) == lanes {
            return b.clone();
        }
    }
    let base_list = base.and_then(Value::as_array);
    Value::Array(
        lanes
            .iter_mut()
            .map(|l| {
                let mut o = obj(by_id(base_list, l.ext_id.as_deref())
                    .cloned()
                    .unwrap_or(Value::Null));
                o.insert(
                    "id".into(),
                    json!(l.ext_id.get_or_insert_with(new_uuid).clone()),
                );
                let target = match &l.target {
                    LaneTarget::Volume => json!({"type": "volume"}),
                    LaneTarget::Pan => json!({"type": "pan"}),
                    LaneTarget::Effect { slot, param, key } => json!({
                        "type": "effect_param",
                        "slot_id": slot,
                        "param": key.clone().unwrap_or_else(|| format!("values.{param}")),
                    }),
                    LaneTarget::Space(p) => json!({"type": "space", "param": p}),
                };
                o.insert("target".into(), target);
                o.insert("enabled".into(), json!(l.enabled));
                let scale = if l.target == LaneTarget::Pan {
                    100.0
                } else {
                    1.0
                };
                let points: Vec<Value> = l
                    .points
                    .iter()
                    .map(|p| {
                        json!({
                            "position": (p.time * rate).round().max(0.0) as u64,
                            "value": (p.value * scale * 1e9).round() / 1e9,
                            "curve": match p.curve {
                                Curve::Linear => "linear",
                                Curve::Hold => "hold",
                                Curve::Spline => "spline",
                            },
                        })
                    })
                    .collect();
                o.insert("points".into(), Value::Array(points));
                match l.height {
                    Some(h) => o.insert("height".into(), json!(h)),
                    None => o.insert("height".into(), Value::Null),
                };
                Value::Object(o)
            })
            .collect(),
    )
}

/// A MIDI clip as the file has it.
fn midi_clip_from(m: &Value, id: u64) -> MidiClip {
    MidiClip {
        id,
        name: m["name"].as_str().unwrap_or("MIDI").to_string(),
        start: m["start"].as_f64().unwrap_or(0.0),
        length: m["length"].as_f64().unwrap_or(4.0),
        notes: m["notes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|n| MidiNote {
                pitch: n["pitch"].as_u64().unwrap_or(60).min(127) as u8,
                start: n["start"].as_f64().unwrap_or(0.0),
                length: n["length"].as_f64().unwrap_or(1.0),
                velocity: n["velocity"].as_u64().unwrap_or(100).clamp(1, 127) as u8,
                channel: n["channel"].as_u64().unwrap_or(0).min(15) as u8,
                pan: n["pan"].as_f64().unwrap_or(0.0) as f32,
            })
            .collect(),
        muted: m["muted"].as_bool().unwrap_or(false),
        gain_db: m["gain_db"].as_f64().unwrap_or(0.0) as f32,
        controls: m["controls"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| ControlEvent {
                at: c["at"].as_f64().unwrap_or(0.0),
                controller: c["controller"].as_u64().unwrap_or(0).min(129) as u8,
                value: c["value"].as_f64().unwrap_or(0.0) as f32,
                channel: c["channel"].as_u64().unwrap_or(0).min(15) as u8,
            })
            .collect(),
        ext_id: m["id"].as_str().map(str::to_string),
    }
}

/// Writes the MIDI clip fields that differ from what `mo` (the original
/// clip, or a new one) already says. Notes and controller moves are
/// written as Audiform writes them: channel and pan left out at 0.
fn write_midi_clip(mo: &mut Map<String, Value>, m: &MidiClip) {
    let fresh = !mo.contains_key("notes");
    let was = midi_clip_from(&Value::Object(mo.clone()), m.id);
    if fresh || was.name != m.name {
        mo.insert("name".into(), json!(m.name));
    }
    if fresh || was.start != m.start {
        mo.insert("start".into(), json!(m.start));
    }
    if fresh || was.length != m.length {
        mo.insert("length".into(), json!(m.length));
    }
    if fresh || was.muted != m.muted {
        mo.insert("muted".into(), json!(m.muted));
    }
    if was.gain_db != m.gain_db {
        put_f32(mo, "gain_db", m.gain_db);
    }
    if fresh || was.notes != m.notes {
        let notes = m
            .notes
            .iter()
            .map(|n| {
                let mut no = Map::new();
                no.insert("pitch".into(), json!(n.pitch));
                no.insert("velocity".into(), json!(n.velocity));
                no.insert("start".into(), json!(n.start));
                no.insert("length".into(), json!(n.length));
                if n.channel != 0 {
                    no.insert("channel".into(), json!(n.channel));
                }
                if n.pan != 0.0 {
                    put_f32(&mut no, "pan", n.pan);
                }
                Value::Object(no)
            })
            .collect();
        mo.insert("notes".into(), Value::Array(notes));
    }
    if was.controls != m.controls {
        if m.controls.is_empty() {
            mo.remove("controls");
        } else {
            let list = m
                .controls
                .iter()
                .map(|c| {
                    let mut co = Map::new();
                    co.insert("at".into(), json!(c.at));
                    co.insert("controller".into(), json!(c.controller));
                    put_f32(&mut co, "value", c.value);
                    co.insert("channel".into(), json!(c.channel));
                    Value::Object(co)
                })
                .collect();
            mo.insert("controls".into(), Value::Array(list));
        }
    }
}

/// Puts buses where `position` says among the tracks (below that many
/// tracks); a bus with none, or past the last track, goes at the end.
fn place_buses(s: &mut Session, positions: &HashMap<TrackId, u32>) {
    let (buses, tracks): (Vec<_>, Vec<_>) = std::mem::take(&mut s.tracks)
        .into_iter()
        .partition(|t| t.kind == TrackKind::Bus);
    let n = tracks.len();
    let pos = |b: &af27_core::Track| positions.get(&b.id).map_or(n, |&p| (p as usize).min(n));
    let mut out = Vec::with_capacity(n + buses.len());
    for (k, t) in tracks.into_iter().enumerate() {
        out.extend(buses.iter().filter(|b| pos(b) == k).cloned());
        out.push(t);
    }
    out.extend(buses.iter().filter(|b| pos(b) == n).cloned());
    s.tracks = out;
}

fn chain_from(v: &Value) -> Vec<EffectSlot> {
    v["slots"]
        .as_array()
        .map(|a| a.iter().map(slot_from).collect())
        .unwrap_or_default()
}

/// Reads an Audiform session.
pub fn import(text: &str, base_dir: &Path) -> Result<Imported, String> {
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("Not an Audiform session: {e}"))?;
    let rate = v["sample_rate"].as_u64().unwrap_or(48_000).max(1) as u32;
    let mut out = Session::new(v["name"].as_str().unwrap_or("Audiform Session"), rate, 0);
    let mut notices = Vec::new();
    let secs = |x: &Value| x.as_f64().unwrap_or(0.0) / rate as f64;
    out.playhead = secs(&v["view"]["playhead"]);

    if let Some(t) = v.get("tempo").filter(|t| t.is_object()) {
        out.tempo.bpm = t["bpm"].as_f64().unwrap_or(120.0);
        out.tempo.beats_per_bar = t["beats_per_bar"].as_u64().unwrap_or(4) as u32;
        out.tempo.beat_unit = t["beat_unit"].as_u64().unwrap_or(4) as u32;
        // Kept in the file's order; the map reads them as Audiform does.
        out.tempo.changes = t["changes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| TempoChange {
                bar: c["bar"].as_u64().unwrap_or(0) as u32,
                bpm: c["bpm"].as_f64().unwrap_or(out.tempo.bpm),
                beats_per_bar: c["beats_per_bar"].as_u64().unwrap_or(4) as u32,
                beat_unit: c["beat_unit"].as_u64().unwrap_or(4) as u32,
                ext_id: c["id"].as_str().map(str::to_string),
            })
            .collect();
    }
    if let Some(sp) = v.get("space").filter(|s| s.is_object()) {
        out.space = sp["layout"].as_str().and_then(layout_from);
        out.monitor = listen_from(sp);
    }

    let mut media_ids: HashMap<String, (u64, u32, f64)> = HashMap::new();
    for m in v["media"].as_array().into_iter().flatten() {
        let key = m["id"].as_str().unwrap_or_default().to_string();
        let path = crate::resolve(m["path"].as_str(), m["relative_path"].as_str(), base_dir);
        let mrate = m["sample_rate"].as_u64().unwrap_or(rate as u64).max(1) as u32;
        let id = out.alloc_id();
        out.media.push(MediaItem {
            id,
            name: path
                .as_deref()
                .map(crate::file_name)
                .unwrap_or_else(|| "Missing file".into()),
            path,
            duration: m["length"].as_f64().unwrap_or(0.0) / mrate as f64,
            rate: mrate,
            channels: m["channels"].as_u64().unwrap_or(0) as u16,
            ext_id: Some(key.clone()),
        });
        media_ids.insert(key, (id, mrate, m["length"].as_f64().unwrap_or(0.0)));
    }

    // Tracks first, then buses (as bus tracks); outputs and sends resolve after.
    let mut by_uuid: HashMap<String, u64> = HashMap::new();
    let mut pending: Vec<(u64, Value)> = Vec::new();
    let mut silent_fx = 0usize;
    let mut envelopes = 0usize;
    let mut bus_pos: HashMap<TrackId, u32> = HashMap::new();
    let lists = [
        (v["tracks"].as_array(), false),
        (v["buses"].as_array(), true),
    ];
    for (list, is_bus) in lists {
        for t in list.into_iter().flatten() {
            let kind = if is_bus {
                TrackKind::Bus
            } else if t["kind"].as_str() == Some("midi") {
                TrackKind::Midi
            } else {
                TrackKind::Audio
            };
            let tid = out.add_track_of(kind);
            let uuid = t["id"].as_str().unwrap_or_default().to_string();
            by_uuid.insert(uuid.clone(), tid);
            pending.push((tid, t.clone()));
            let mut clips = Vec::new();
            for c in t["clips"].as_array().into_iter().flatten() {
                let Some(&(media, mrate, mlen)) =
                    c["media_id"].as_str().and_then(|k| media_ids.get(k))
                else {
                    continue;
                };
                let src_in = c["source_in"].as_f64().unwrap_or(0.0);
                let src_out = c["source_out"].as_f64().unwrap_or(src_in);
                let props = props_from(c);
                // A reversed clip's source points count from the file's
                // end (positions in a reversed copy); `offset` is where its
                // span starts in the file itself.
                let first = if props.reversed {
                    (mlen - src_out).max(0.0)
                } else {
                    src_in
                };
                envelopes += c["envelopes"].as_array().map_or(0, Vec::len);
                silent_fx += c["effects"]["slots"].as_array().map_or(0, Vec::len);
                let id = out.alloc_id();
                clips.push(Clip {
                    id,
                    media,
                    name: c["name"].as_str().unwrap_or("Clip").to_string(),
                    start: secs(&c["start"]),
                    offset: first / mrate as f64,
                    length: (src_out - src_in).max(0.0) / mrate as f64
                        * props.stretch.applied_ratio(),
                    props,
                    gain_db: c["gain_db"].as_f64().unwrap_or(0.0) as f32,
                    fade_in: secs(&c["fade_in"]["length"]),
                    fade_out: secs(&c["fade_out"]["length"]),
                    muted: c["muted"].as_bool().unwrap_or(false),
                    ext_id: c["id"].as_str().map(str::to_string),
                });
            }
            let midi: Vec<MidiClip> = t["midi_clips"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|m| midi_clip_from(m, out.alloc_id()))
                .collect();
            let chain = chain_from(&t["effects"]);
            silent_fx += chain
                .iter()
                .filter(|s| {
                    s.values.is_empty()
                        && !s.effect.is_empty()
                        && !af27_core::plugin::is_plugin(&s.effect)
                        && af27_dsp::Effect::from_id(&s.effect).is_none()
                        && af27_fx::info(&s.effect).is_none()
                })
                .count();
            let tr = out.track_mut(tid).unwrap();
            tr.name = t["name"].as_str().unwrap_or("Track").to_string();
            let sf = strip_from(t);
            tr.volume_db = sf.volume_db;
            tr.pan = sf.pan;
            tr.mute = sf.muted;
            tr.solo = sf.solo;
            tr.mono = sf.mono;
            tr.polarity = sf.polarity;
            tr.solo_safe = sf.solo_safe;
            tr.automation = sf.automation;
            tr.show_lanes = sf.show_lanes;
            if kind == TrackKind::Midi {
                tr.midi_in = t["input"].as_str().map(str::to_string);
            }
            tr.armed = t["record_armed"].as_bool().unwrap_or(false);
            if let Some(h) = t["height"].as_f64() {
                tr.height = (h as f32).max(40.0);
            }
            if let Some(c) = hex_color(&t["color"]) {
                tr.color = c;
            }
            if let Some(p) = t.get("space").filter(|p| p.is_object()) {
                tr.pos = pos_from(p);
                tr.spatial.had_spot = true;
            }
            tr.spatial.room = t["room"].as_bool().unwrap_or(false);
            tr.spatial.lanes = lanes_from(t, rate);
            tr.spatial.stereo = Some(
                t["channels"].as_str() != Some("mono") && !t["mono"].as_bool().unwrap_or(false),
            );
            if let Some(inst) = t
                .get("af27_synth")
                .and_then(|s| serde_json::from_value(s.clone()).ok())
            {
                tr.instrument = inst;
            }
            tr.soundfont = t
                .get("af27_soundfont")
                .and_then(|s| serde_json::from_value::<SoundFontRef>(s.clone()).ok());
            tr.clips = clips;
            tr.midi = midi;
            tr.chain = chain;
            tr.rack = rack_from(&t["effects"]);
            tr.ext_id = Some(uuid);
        }
    }
    for (tid, t) in pending {
        let output = match t["output"]["type"].as_str() {
            Some("bus") => t["output"]["bus_id"]
                .as_str()
                .and_then(|b| by_uuid.get(b))
                .copied(),
            _ => None,
        };
        let sends: Vec<Send> = t["sends"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| {
                Some(Send {
                    bus: *by_uuid.get(s["bus_id"].as_str()?)?,
                    ..send_from(s)
                })
            })
            .collect();
        let tr = out.track_mut(tid).unwrap();
        tr.output = output;
        tr.sends = sends;
        resolve_sidechains(&mut tr.chain, &t["effects"], &by_uuid);
        tr.lanes = auto_lanes_from(&t["envelopes"], rate as f64, &tr.chain);
        if let Some(p) = t["position"].as_u64().filter(|_| tr.kind == TrackKind::Bus) {
            bus_pos.insert(tid, p as u32);
        }
    }
    place_buses(&mut out, &bus_pos);
    out.master_db = v["master"]["volume_db"].as_f64().unwrap_or(0.0) as f32;
    out.master_chain = chain_from(&v["master"]["effects"]);
    out.master_rack = rack_from(&v["master"]["effects"]);
    resolve_sidechains(&mut out.master_chain, &v["master"]["effects"], &by_uuid);
    out.master_lanes = auto_lanes_from(&v["master"]["envelopes"], rate as f64, &out.master_chain);
    for m in v["markers"].as_array().into_iter().flatten() {
        out.markers.push(Marker {
            name: m["name"].as_str().unwrap_or("Marker").to_string(),
            time: secs(&m["position"]),
            length: secs(&m["duration"]),
            ext_id: m["id"].as_str().map(str::to_string),
        });
    }
    if envelopes > 0 {
        notices.push(format!("{envelopes} clip automation lane(s) are kept in the session and saved back unchanged; Audiform27 does not play clip automation yet."));
    }
    if silent_fx > 0 {
        notices.push(format!("{silent_fx} effect(s) Audiform27 does not have stay in their chains, silent here, and are saved back unchanged."));
    }
    Ok(Imported {
        session: out,
        notices,
        base: Some(v),
    })
}

/// A send's own settings as the file has them (the bus is resolved by
/// the caller). Audiform pans -100..100.
fn send_from(s: &Value) -> Send {
    Send {
        bus: 0,
        level_db: s["level_db"].as_f64().unwrap_or(0.0) as f32,
        pre_fader: s["pre_fader"].as_bool().unwrap_or(false),
        pan: (s["pan"].as_f64().unwrap_or(0.0) / 100.0).clamp(-1.0, 1.0) as f32,
        enabled: s["enabled"].as_bool().unwrap_or(true),
        ext_id: s["id"].as_str().map(str::to_string),
    }
}

/// Finds an object by id in `list` (for looking up originals).
fn by_id<'a>(list: Option<&'a Vec<Value>>, id: Option<&str>) -> Option<&'a Value> {
    let id = id?;
    list?.iter().find(|o| o["id"].as_str() == Some(id))
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// Writes a number unless the object already holds the same value (so
/// `9` stays `9` and `120.0` stays `120.0`). Whole values are written as
/// integers.
fn set_num(o: &mut Map<String, Value>, key: &str, v: f64) {
    if o.get(key).and_then(Value::as_f64) == Some(v) {
        return;
    }
    let val = if v.fract() == 0.0 && (0.0..9.0e15).contains(&v) && key != "bpm" {
        json!(v as u64)
    } else {
        json!(v)
    };
    o.insert(key.into(), val);
}

fn empty_chain() -> Value {
    json!({"enabled": true, "input_gain_db": 0.0, "output_gain_db": 0.0, "mix": 1.0, "slots": []})
}

/// Writes a gain or mix unless the object already holds that value as
/// it reads back (`default` when it is missing), so an unchanged file
/// keeps its exact text.
fn set_level(o: &mut Map<String, Value>, key: &str, v: f32, default: f32) {
    let had = o
        .get(key)
        .and_then(Value::as_f64)
        .map_or(default, |x| x as f32);
    if had == v {
        return;
    }
    // Shortest decimal for the f32, not its f64 expansion.
    let x: f64 = format!("{v}").parse().unwrap_or(v as f64);
    o.insert(key.into(), json!(x));
}

/// Writes a slot's sidechain as the key's file id. A slot listening to
/// itself keeps what the file had, unless that named a track still here
/// (the key was cleared).
fn write_sidechain(
    o: &mut Map<String, Value>,
    key: Option<TrackId>,
    uuid_of: &HashMap<u64, String>,
) {
    match key.and_then(|k| uuid_of.get(&k)) {
        Some(u) => {
            if o.get("sidechain").and_then(Value::as_str) != Some(u.as_str()) {
                o.insert("sidechain".into(), json!(u));
            }
        }
        None => {
            let named = o
                .get("sidechain")
                .and_then(Value::as_str)
                .is_some_and(|old| uuid_of.values().any(|u| u == old));
            if named {
                o.insert("sidechain".into(), Value::Null);
            }
        }
    }
}

fn write_chain(
    slots: &mut [EffectSlot],
    rack: &Rack,
    base_chain: Option<&Value>,
    uuid_of: &HashMap<u64, String>,
) -> Value {
    let mut chain = obj(base_chain.cloned().unwrap_or_else(empty_chain));
    if chain
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true)
        == rack.bypass
    {
        chain.insert("enabled".into(), json!(!rack.bypass));
    }
    set_level(&mut chain, "input_gain_db", rack.input_db, 0.0);
    set_level(&mut chain, "output_gain_db", rack.output_db, 0.0);
    set_level(&mut chain, "mix", rack.mix, 1.0);
    let base_slots = base_chain.and_then(|c| c["slots"].as_array());
    let out: Vec<Value> = slots
        .iter_mut()
        .map(|s| {
            let orig = by_id(base_slots, s.ext_id.as_deref());
            let id = s.ext_id.get_or_insert_with(new_uuid).clone();
            let mut o = obj(orig.cloned().unwrap_or(Value::Null));
            o.insert("id".into(), json!(id));
            o.insert("enabled".into(), json!(!s.bypass));
            write_sidechain(&mut o, s.sidechain, uuid_of);
            if let Some(info) = af27_fx::info(&s.effect) {
                o.insert(
                    "effect".into(),
                    json!(s.effect.replacen("af.", "altaudio.", 1)),
                );
                let mut params = obj(o.get("params").cloned().unwrap_or(Value::Null));
                for (p, val) in info.params.iter().zip(&s.values) {
                    params.insert(p.key.into(), json!(*val as f64));
                }
                o.insert("params".into(), Value::Object(params));
                if let Some(d) = &s.data {
                    o.insert(
                        "data".into(),
                        serde_json::from_str(d).unwrap_or_else(|_| json!(d)),
                    );
                }
            } else if af27_core::plugin::is_plugin(&s.effect) {
                write_plugin(&mut o, s);
            } else if orig.is_none() || af27_dsp::Effect::from_id(&s.effect).is_some() {
                // Audiform27's own effect: a slot Audiform skips but keeps.
                o.insert("effect".into(), json!(format!("{AF27}{}", s.effect)));
                o.insert("params".into(), json!({ "values": s.values }));
            }
            // Otherwise an effect only Audiform has: its original slot, as is.
            o.entry("params").or_insert_with(|| json!({}));
            Value::Object(o)
        })
        .collect();
    chain.insert("slots".into(), Value::Array(out));
    Value::Object(chain)
}

/// Writes `session` as an Audiform session. `base` is the document it was
/// opened from, if any; ids for new objects are filled into `session`, so
/// later saves keep them.
pub fn export(session: &mut Session, base: Option<&Value>) -> Value {
    let mut doc = obj(base.cloned().unwrap_or(Value::Null));
    let rate = session.rate as f64;
    let samples = |s: f64| (s * rate).round().max(0.0) as u64;
    let now = now_iso();
    doc.entry("format").or_insert_with(|| json!(FORMAT));
    doc.entry("schema_version").or_insert_with(|| json!(1));
    doc.entry("id").or_insert_with(|| json!(new_uuid()));
    doc.insert("name".into(), json!(session.name));
    doc.insert("sample_rate".into(), json!(session.rate));
    doc.entry("bit_depth").or_insert_with(|| json!(32));
    doc.entry("created").or_insert_with(|| json!(now));
    doc.insert("modified".into(), json!(now));
    doc.entry("view").or_insert_with(|| json!({"samples_per_pixel": 512.0, "scroll": 0, "scroll_y": 0, "playhead": 0, "selection": null}));
    if let Some(view) = doc.get_mut("view").and_then(Value::as_object_mut) {
        view.insert("playhead".into(), json!(samples(session.playhead)));
    }
    doc.entry("modes")
        .or_insert_with(|| json!({"podcast": false, "voiceover": false}));
    doc.entry("libraries").or_insert_with(|| json!([]));
    doc.entry("template_id").or_insert(Value::Null);
    doc.entry("metadata").or_insert_with(|| json!({"show_id": null, "title": null, "show": null, "episode_number": null, "season_number": null, "description": null, "artist": null, "artwork": null, "explicit": false}));

    // Tempo: the map is Audiform27's. A value read from the file and not
    // changed is written back exactly as it was, and a map with no changes
    // keeps the file's own `changes`, so an unedited session saves the same.
    let base_tempo = base.map(|b| &b["tempo"]);
    let mut tempo = obj(base_tempo.cloned().unwrap_or(Value::Null));
    tempo.insert("bpm".into(), json!(session.tempo.bpm));
    tempo.insert("beats_per_bar".into(), json!(session.tempo.beats_per_bar));
    set_num(&mut tempo, "beat_unit", session.tempo.beat_unit as f64);
    let base_changes = base_tempo.and_then(|t| t["changes"].as_array());
    if session.tempo.changes.is_empty() && !base_changes.is_some_and(|c| !c.is_empty()) {
        tempo.entry("changes").or_insert_with(|| json!([]));
    } else {
        let changes: Vec<Value> = session
            .tempo
            .changes
            .iter_mut()
            .map(|c| {
                let mut co = obj(by_id(base_changes, c.ext_id.as_deref())
                    .cloned()
                    .unwrap_or(Value::Null));
                co.insert(
                    "id".into(),
                    json!(c.ext_id.get_or_insert_with(new_uuid).clone()),
                );
                set_num(&mut co, "bar", c.bar as f64);
                set_num(&mut co, "bpm", c.bpm);
                set_num(&mut co, "beats_per_bar", c.beats_per_bar as f64);
                set_num(&mut co, "beat_unit", c.beat_unit as f64);
                Value::Object(co)
            })
            .collect();
        tempo.insert("changes".into(), Value::Array(changes));
    }
    doc.insert("tempo".into(), Value::Object(tempo));
    match session.space {
        Some(l) => {
            let mut sp = obj(base.map(|b| b["space"].clone()).unwrap_or(Value::Null));
            if sp.get("layout").and_then(Value::as_str) != Some(l.label()) {
                sp.insert("layout".into(), json!(l.label()));
            }
            // `listen` left out reads as stereo; written only when changed.
            if listen_from(&Value::Object(sp.clone())) != session.monitor {
                sp.insert("listen".into(), json!(listen_key(session.monitor)));
            }
            doc.insert("space".into(), Value::Object(sp));
        }
        None => {
            doc.remove("space");
        }
    }

    // Media.
    let base_media = base.and_then(|b| b["media"].as_array());
    let media: Vec<Value> = session
        .media
        .iter_mut()
        .map(|m| {
            let mut o = obj(by_id(base_media, m.ext_id.as_deref())
                .cloned()
                .unwrap_or(Value::Null));
            let id = m.ext_id.get_or_insert_with(new_uuid).clone();
            o.insert("id".into(), json!(id));
            if let Some(p) = &m.path {
                o.insert("path".into(), json!(p.to_string_lossy()));
            }
            o.entry("path").or_insert_with(|| json!(""));
            o.entry("relative_path").or_insert(Value::Null);
            o.insert("sample_rate".into(), json!(m.rate));
            o.insert("channels".into(), json!(m.channels));
            o.insert(
                "length".into(),
                json!((m.duration * m.rate as f64).round() as u64),
            );
            Value::Object(o)
        })
        .collect();
    doc.insert("media".into(), Value::Array(media));
    let media_rate: HashMap<u64, (String, f64, f64)> = session
        .media
        .iter()
        .map(|m| {
            let len = (m.duration * m.rate as f64).round();
            (
                m.id,
                (m.ext_id.clone().unwrap_or_default(), m.rate as f64, len),
            )
        })
        .collect();

    // Ids for every track first, so outputs and sends can name buses.
    for t in &mut session.tracks {
        t.ext_id.get_or_insert_with(new_uuid);
    }
    let uuid_of: HashMap<u64, String> = session
        .tracks
        .iter()
        .map(|t| (t.id, t.ext_id.clone().unwrap()))
        .collect();
    let base_tracks = base.and_then(|b| b["tracks"].as_array());
    let base_buses = base.and_then(|b| b["buses"].as_array());
    let mut tracks = Vec::new();
    let mut buses = Vec::new();
    let n_tracks = session
        .tracks
        .iter()
        .filter(|t| t.kind != TrackKind::Bus)
        .count();
    // Tracks above the one being written (a bus's `position`).
    let mut above = 0usize;
    let rate_f = session.rate as f64;
    for t in &mut session.tracks {
        let is_bus = t.kind == TrackKind::Bus;
        if !is_bus {
            above += 1;
        }
        let orig = by_id(
            if is_bus { base_buses } else { base_tracks },
            t.ext_id.as_deref(),
        )
        .or_else(|| {
            by_id(
                if is_bus { base_tracks } else { base_buses },
                t.ext_id.as_deref(),
            )
        });
        let mut o = obj(orig.cloned().unwrap_or(Value::Null));
        o.insert("id".into(), json!(t.ext_id.clone().unwrap()));
        o.insert("name".into(), json!(t.name));
        o.entry("channels").or_insert_with(|| json!("stereo"));
        let had_height = o.get("height").and_then(Value::as_f64);
        if had_height.map(|h| (h as f32).max(40.0)) != Some(t.height) {
            o.insert("height".into(), json!(t.height.round() as u32));
        }
        write_strip(
            &mut o,
            &Strip {
                volume_db: t.volume_db,
                pan: t.pan,
                muted: t.mute,
                solo: t.solo,
                mono: t.mono,
                polarity: t.polarity,
                solo_safe: t.solo_safe,
                automation: t.automation,
                show_lanes: t.show_lanes,
            },
            is_bus,
        );
        if is_bus {
            // Display only: below this many tracks (None: below the last).
            let was = o
                .get("position")
                .and_then(Value::as_u64)
                .map_or(n_tracks, |p| (p as usize).min(n_tracks));
            if was != above {
                o.insert("position".into(), json!(above));
            }
        }
        if t.kind == TrackKind::Midi {
            let was = o.get("input").and_then(Value::as_str).map(str::to_string);
            if was != t.midi_in {
                o.insert("input".into(), json!(t.midi_in));
            }
        }
        o.insert(
            "output".into(),
            match t.output.and_then(|b| uuid_of.get(&b)) {
                Some(b) => json!({"type": "bus", "bus_id": b}),
                None => json!({"type": "master"}),
            },
        );
        let base_sends = orig.and_then(|x| x["sends"].as_array());
        let sends: Vec<Value> = t
            .sends
            .iter_mut()
            .filter_map(|s| {
                let bus = uuid_of.get(&s.bus)?.clone();
                let mut so = obj(by_id(base_sends, s.ext_id.as_deref())
                    .cloned()
                    .unwrap_or(Value::Null));
                so.insert(
                    "id".into(),
                    json!(s.ext_id.get_or_insert_with(new_uuid).clone()),
                );
                so.insert("bus_id".into(), json!(bus));
                let fresh = !so.contains_key("level_db");
                let was = send_from(&Value::Object(so.clone()));
                if fresh || was.level_db != s.level_db {
                    put_f32(&mut so, "level_db", s.level_db);
                }
                if fresh || was.pre_fader != s.pre_fader {
                    so.insert("pre_fader".into(), json!(s.pre_fader));
                }
                if fresh || was.pan != s.pan {
                    put_f32(&mut so, "pan", s.pan * 100.0);
                }
                if fresh || was.enabled != s.enabled {
                    so.insert("enabled".into(), json!(s.enabled));
                }
                Some(Value::Object(so))
            })
            .collect();
        o.insert("sends".into(), Value::Array(sends));
        o.insert(
            "effects".into(),
            write_chain(&mut t.chain, &t.rack, orig.map(|x| &x["effects"]), &uuid_of),
        );
        let envs = auto_lanes_to(
            &mut t.lanes,
            orig.map(|x| &x["envelopes"]),
            rate_f,
            &t.chain,
        );
        o.insert("envelopes".into(), envs);
        // The spot: kept whether or not 3D Space is on (Audiform keeps
        // every spot when it is turned off). A track with none in the file
        // and still at the default gets none.
        let had = orig.and_then(|x| x.get("space")).filter(|v| v.is_object());
        if had.is_some() || t.spatial.had_spot || t.pos != SpacePos::default() {
            let mut sp = obj(had.cloned().unwrap_or(Value::Null));
            write_pos(&mut sp, &t.pos);
            o.insert("space".into(), Value::Object(sp));
            t.spatial.had_spot = true;
        }
        // 3D Bus: written only when on (or when it changed).
        let was_room = orig.and_then(|x| x["room"].as_bool()).unwrap_or(false);
        if is_bus && t.spatial.room != was_room {
            if t.spatial.room {
                o.insert("room".into(), json!(true));
            } else {
                o.remove("room");
            }
        }
        if is_bus {
            buses.push(Value::Object(o));
            continue;
        }
        o.entry("role").or_insert_with(|| json!("standard"));
        o.insert("record_armed".into(), json!(t.armed));
        o.entry("monitor_input").or_insert_with(|| json!(false));
        let base_clips = orig.and_then(|x| x["clips"].as_array());
        let clips: Vec<Value> = t
            .clips
            .iter_mut()
            .map(|c| {
                let mut co = obj(by_id(base_clips, c.ext_id.as_deref())
                    .cloned()
                    .unwrap_or(Value::Null));
                let (media_uuid, mrate, mlen) =
                    media_rate.get(&c.media).cloned().unwrap_or_default();
                co.insert(
                    "id".into(),
                    json!(c.ext_id.get_or_insert_with(new_uuid).clone()),
                );
                co.insert("name".into(), json!(c.name));
                co.insert("media_id".into(), json!(media_uuid));
                co.insert("start".into(), json!(samples(c.start)));
                // The span in the file, in samples (from the file's end for
                // a reversed clip, as Audiform counts it).
                let (a, b) = (c.offset * mrate, (c.offset + c.source_len()) * mrate);
                let (src_in, src_out) = if c.props.reversed {
                    (mlen - b, mlen - a)
                } else {
                    (a, b)
                };
                co.insert("source_in".into(), json!(src_in.round().max(0.0) as u64));
                co.insert("source_out".into(), json!(src_out.round().max(0.0) as u64));
                if !co.contains_key("gain_db") {
                    co.insert("gain_db".into(), json!(c.gain_db as f64));
                }
                set_level(&mut co, "gain_db", c.gain_db, 0.0);
                co.insert("muted".into(), json!(c.muted));
                for (key, len) in [("fade_in", c.fade_in), ("fade_out", c.fade_out)] {
                    let mut f = obj(co.get(key).cloned().unwrap_or(Value::Null));
                    f.insert("length".into(), json!(samples(len)));
                    f.entry("shape").or_insert_with(|| json!("linear"));
                    f.entry("curve").or_insert_with(|| json!(0.0));
                    co.insert(key.into(), Value::Object(f));
                }
                write_props(&mut co, &c.props);
                co.entry("envelopes").or_insert_with(|| json!([]));
                co.entry("effects").or_insert_with(empty_chain);
                Value::Object(co)
            })
            .collect();
        o.insert("clips".into(), Value::Array(clips));
        if t.kind == TrackKind::Midi || !t.midi.is_empty() {
            o.insert("kind".into(), json!("midi"));
            let base_midi = orig.and_then(|x| x["midi_clips"].as_array());
            let midi: Vec<Value> = t
                .midi
                .iter_mut()
                .map(|m| {
                    let mut mo = obj(by_id(base_midi, m.ext_id.as_deref())
                        .cloned()
                        .unwrap_or(Value::Null));
                    mo.insert(
                        "id".into(),
                        json!(m.ext_id.get_or_insert_with(new_uuid).clone()),
                    );
                    write_midi_clip(&mut mo, m);
                    Value::Object(mo)
                })
                .collect();
            o.insert("midi_clips".into(), Value::Array(midi));
            // Audiform27's own synth settings: written when the file had
            // them or they are not the defaults, so a session from
            // Audiform saves back unchanged.
            let had_synth = o.contains_key("af27_synth");
            let synth = serde_json::to_value(t.instrument).unwrap_or(Value::Null);
            let default_synth =
                serde_json::to_value(af27_core::midi::Instrument::default()).unwrap_or(Value::Null);
            if had_synth || synth != default_synth {
                let same = o.get("af27_synth").and_then(|s| {
                    serde_json::from_value::<af27_core::midi::Instrument>(s.clone()).ok()
                }) == Some(t.instrument);
                if !same {
                    o.insert("af27_synth".into(), synth);
                }
            }
            // Audiform27's own field, like `af27_synth`: Audiform keeps it.
            match &t.soundfont {
                Some(sf) => {
                    o.insert(
                        "af27_soundfont".into(),
                        serde_json::to_value(sf).unwrap_or(Value::Null),
                    );
                }
                None => {
                    o.remove("af27_soundfont");
                }
            }
        }
        tracks.push(Value::Object(o));
    }
    doc.insert("tracks".into(), Value::Array(tracks));
    doc.insert("buses".into(), Value::Array(buses));

    let mut master = obj(base.map(|b| b["master"].clone()).unwrap_or(Value::Null));
    master.entry("channels").or_insert_with(|| json!("stereo"));
    master.entry("height").or_insert_with(|| json!(72));
    if !master.contains_key("volume_db") {
        master.insert("volume_db".into(), json!(session.master_db as f64));
    }
    set_level(&mut master, "volume_db", session.master_db, 0.0);
    let base_master_fx = base
        .map(|b| &b["master"]["effects"])
        .filter(|v| !v.is_null());
    master.insert(
        "effects".into(),
        write_chain(
            &mut session.master_chain,
            &session.master_rack,
            base_master_fx,
            &uuid_of,
        ),
    );
    let envs = auto_lanes_to(
        &mut session.master_lanes,
        base.map(|b| &b["master"]["envelopes"]),
        rate_f,
        &session.master_chain,
    );
    master.insert("envelopes".into(), envs);
    doc.insert("master".into(), Value::Object(master));

    let base_markers = base.and_then(|b| b["markers"].as_array());
    let markers: Vec<Value> = session
        .markers
        .iter_mut()
        .map(|m| {
            let mut mo = obj(by_id(base_markers, m.ext_id.as_deref())
                .cloned()
                .unwrap_or(Value::Null));
            mo.insert(
                "id".into(),
                json!(m.ext_id.get_or_insert_with(new_uuid).clone()),
            );
            mo.insert("name".into(), json!(m.name));
            mo.entry("kind").or_insert_with(|| json!("cue"));
            mo.insert("position".into(), json!(samples(m.time)));
            mo.insert("duration".into(), json!(samples(m.length)));
            mo.entry("description").or_insert_with(|| json!(""));
            Value::Object(mo)
        })
        .collect();
    doc.insert("markers".into(), Value::Array(markers));
    Value::Object(doc)
}

/// Saves `session` to `path` as an Audiform session, pretty-printed as
/// Audiform writes it. Returns the document written (the new base).
pub fn save(session: &mut Session, base: Option<&Value>, path: &Path) -> Result<Value, String> {
    let mut doc = export(session, base);
    if let Some(dir) = path.parent() {
        fill_relative_paths(&mut doc, base, dir);
    }
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    let tmp: PathBuf = path.with_extension("audfrm.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("Could not save {}: {e}", path.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("Could not save {}: {e}", path.display()))?;
    Ok(doc)
}

/// Media added since the file was read, and kept inside the session's
/// folder, gets a path relative to the session file as Audiform writes
/// it, so the folder can move. Media the file already had keeps what it
/// said, so an unedited session saves the same.
fn fill_relative_paths(doc: &mut Value, base: Option<&Value>, dir: &Path) {
    let known: Vec<&str> = base
        .and_then(|b| b["media"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str())
        .collect();
    let Some(media) = doc.get_mut("media").and_then(Value::as_array_mut) else {
        return;
    };
    for m in media {
        if m["id"].as_str().is_some_and(|id| known.contains(&id)) || !m["relative_path"].is_null() {
            continue;
        }
        let Some(p) = m["path"]
            .as_str()
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
        else {
            continue;
        };
        if let Ok(rel) = p.strip_prefix(dir) {
            m["relative_path"] = json!(rel.to_string_lossy());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_media_in_the_session_folder_gets_a_relative_path() {
        let dir = std::env::temp_dir().join(format!("af27-rel-{}", new_uuid()));
        std::fs::create_dir_all(dir.join("Audio")).unwrap();
        let file = dir.join("Show.audfrm");
        let imp = import(SAMPLE, &dir).unwrap();
        let base = imp.base.clone().unwrap();
        let mut s = imp.session;
        let id = s.alloc_id();
        s.media.push(MediaItem {
            id,
            name: "guest.wav".into(),
            path: Some(dir.join("Audio").join("guest.wav")),
            duration: 1.0,
            rate: 48_000,
            channels: 1,
            ext_id: None,
        });
        let outside = s.alloc_id();
        s.media.push(MediaItem {
            id: outside,
            name: "music.wav".into(),
            path: Some(std::env::temp_dir().join("elsewhere").join("music.wav")),
            duration: 1.0,
            rate: 48_000,
            channels: 2,
            ext_id: None,
        });
        let doc = save(&mut s, Some(&base), &file).unwrap();
        let media = doc["media"].as_array().unwrap();
        // The file's own media: untouched.
        assert_eq!(media[0], base["media"][0]);
        let rel = std::path::Path::new("Audio").join("guest.wav");
        assert_eq!(media[1]["relative_path"], json!(rel.to_string_lossy()));
        assert!(media[2]["relative_path"].is_null());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A session with fields Audiform27 does not use anywhere: they must
    /// all survive a load and save.
    pub(super) const SAMPLE: &str = r##"{
      "format": "altaudio.session", "schema_version": 1, "id": "11111111-1111-4111-8111-111111111111",
      "name": "Round Trip", "sample_rate": 48000, "bit_depth": 32,
      "created": "2026-11-08T05:29:43Z", "modified": "2026-11-08T05:29:43Z",
      "media": [{"id": "m1", "path": "C:\\x\\voice.wav", "relative_path": null, "sample_rate": 44100, "channels": 2, "length": 441000, "linked": null}],
      "tracks": [{
        "id": "t1", "name": "Host", "role": "standard", "host_profile_id": "close_mic", "channels": "stereo", "color": "#ff8800",
        "height": 96, "volume_db": -3.0, "pan": 0.25, "muted": false, "solo": false, "record_armed": false, "monitor_input": false,
        "input": {"device": "SSL 2+", "channels": [0, 1]}, "output": {"type": "bus", "bus_id": "b1"},
        "sends": [{"id": "s1", "bus_id": "b1", "level_db": -12.0, "pan": 0.5, "pre_fader": true, "enabled": true}],
        "effects": {"enabled": true, "input_gain_db": 1.5, "output_gain_db": -0.5, "mix": 0.8, "slots": [
          {"id": "fx1", "effect": "altaudio.dynamics", "enabled": true, "params": {"threshold": -20.0, "ratio": 3.0}, "preset": "Podcast Voice"},
          {"id": "fx2", "effect": "altaudio.stereo_width", "enabled": false, "params": {"pan": 10.0, "width": 150.0}}
        ]},
        "clips": [{"id": "c1", "name": "voice", "media_id": "m1", "start": 96000, "source_in": 44100, "source_out": 88200,
          "gain_db": 1.5, "muted": false, "locked": true, "color": null, "audio_type": "dialogue",
          "fade_in": {"length": 4800, "shape": "cosine", "curve": 0.3}, "fade_out": {"length": 0, "shape": "linear", "curve": 0.0},
          "stretch": {"ratio": 1.1, "pitch": -1.0, "keep_pitch": true, "polyphonic": true, "rendered": false},
          "envelopes": [{"id": "e1", "target": {"type": "volume"}, "enabled": true, "height": 40, "points": [{"position": 0, "value": 0.5, "curve": "linear"}]}],
          "effects": {"enabled": true, "input_gain_db": 0.0, "output_gain_db": 0.0, "mix": 1.0, "slots": []}}],
        "envelopes": [{"id": "e2", "target": {"type": "pan"}, "enabled": true, "height": 40, "points": []}]
      }],
      "buses": [{"id": "b1", "name": "Reverb Bus", "channels": "stereo", "height": 72, "volume_db": -6.0, "pan": 0.0, "muted": false, "solo": false,
        "output": {"type": "master"}, "sends": [], "effects": {"enabled": true, "input_gain_db": 0.0, "output_gain_db": 0.0, "mix": 1.0, "slots": []}, "envelopes": []}],
      "master": {"channels": "stereo", "height": 72, "volume_db": -1.0, "effects": {"enabled": true, "input_gain_db": 0.0, "output_gain_db": 0.0, "mix": 1.0, "slots": []}, "envelopes": []},
      "markers": [{"id": "k1", "name": "Intro", "kind": "chapter", "position": 48000, "duration": 0, "description": "Cold open", "color": "#ff0000", "chapter": {"url": "https://x"}}],
      "view": {"samples_per_pixel": 6103.5, "scroll": 10, "scroll_y": 0, "playhead": 3658228, "selection": null},
      "modes": {"podcast": true, "voiceover": false}, "libraries": [{"path": "D:\\lib"}], "template_id": null,
      "metadata": {"show_id": null, "title": "Ep 1", "show": "Show", "episode_number": 1, "season_number": null, "description": null, "artist": null, "artwork": null, "explicit": false},
      "playlist": [{"id": "p1"}],
      "tempo": {"bpm": 96.0, "beats_per_bar": 3, "beat_unit": 4, "changes": [{"id": "tc1", "bar": 9, "bpm": 120.0, "beats_per_bar": 4, "beat_unit": 4}]},
      "a_field_from_the_future": {"keep": "me"}
    }"##;

    /// SAMPLE as a 3D Space session with every 3D field set.
    fn sample_3d() -> Value {
        let mut v: Value = serde_json::from_str(SAMPLE).unwrap();
        v["space"] = json!({"layout": "7.1.4", "listen": "binaural"});
        v["tracks"][0]["space"] = json!({"x": -0.5, "y": 0.25, "z": -0.3, "size": 0.2, "width": 0.6,
            "rotation": 30.0, "lfe": 0.5, "spread": 0.4, "spatialize": 0.7, "spatialize_mode": "ambience"});
        v["tracks"][0]["envelopes"].as_array_mut().unwrap().push(json!({"id": "e3",
            "target": {"type": "space", "param": "x"}, "enabled": true, "height": null,
            "points": [{"position": 0, "value": -1.0, "curve": "hold"}, {"position": 96000, "value": 1.0, "curve": "linear"}]}));
        v["tracks"][0]["clips"][0]["channel_mix"] =
            json!({"sends": [{"left": 1.0, "right": 0.0}, {"left": 0.0, "right": 1.0}]});
        v["buses"][0]["space"] = json!({"x": 0.0, "y": 1.0});
        v["buses"][0]["room"] = json!(true);
        v
    }

    fn reload(v: &Value) -> (Session, Value) {
        let imp = import(&v.to_string(), Path::new(".")).unwrap();
        (imp.session, imp.base.unwrap())
    }

    fn saved(s: &mut Session, base: &Value) -> Value {
        let mut out = export(s, Some(base));
        out.as_object_mut().unwrap().remove("modified");
        out
    }

    fn unmodified(base: &Value) -> Value {
        let mut b = base.clone();
        b.as_object_mut().unwrap().remove("modified");
        b
    }

    #[test]
    fn every_3d_field_reads_and_an_unchanged_session_saves_the_same() {
        let (mut s, base) = reload(&sample_3d());
        assert_eq!(s.space, Some(SpaceLayout::Surround714));
        assert_eq!(s.monitor, Monitor::Binaural);
        let t = &s.tracks[0];
        let p = t.pos;
        assert_eq!(
            (p.x, p.y, p.z, p.size, p.width),
            (-0.5, 0.25, -0.3, 0.2, 0.6)
        );
        assert_eq!(
            (p.rotation, p.lfe, p.spread, p.spatialize),
            (30.0, 0.5, 0.4, 0.7)
        );
        assert_eq!(p.spatialize_mode, SpatializeMode::Ambience);
        assert_eq!(t.spatial.lanes.len(), 1);
        let lane = &t.spatial.lanes[0];
        assert_eq!(lane.param, SpaceParam::X);
        assert_eq!(lane.points[1].t, 2.0);
        assert_eq!(lane.points[0].curve, LaneCurve::Hold);
        assert_eq!(t.spatial.stereo, Some(true));
        assert!(t.clips[0].props.fold_channels);
        let bus = &s.tracks[1];
        assert!(bus.spatial.room);
        // Only x and y in the file: the rest take Audiform's defaults.
        assert_eq!(bus.pos, SpacePos::default());
        assert_eq!(saved(&mut s, &base), unmodified(&base));
    }

    #[test]
    fn edited_3d_fields_save_as_audiform_writes_them() {
        let (mut s, base) = reload(&sample_3d());
        s.monitor = Monitor::Speakers;
        s.tracks[0].pos.rotation = 0.0;
        s.tracks[0].pos.spread = 0.9;
        s.tracks[0].pos.spatialize_mode = SpatializeMode::SideSpread;
        s.tracks[1].spatial.room = false;
        s.tracks[1].pos.lfe = 0.25;
        let out = saved(&mut s, &base);
        assert_eq!(
            out["space"],
            json!({"layout": "7.1.4", "listen": "speakers"})
        );
        let sp = &out["tracks"][0]["space"];
        assert!(sp.get("rotation").is_none() && sp.get("spatialize_mode").is_none());
        assert_eq!(sp["spread"], json!(0.9));
        assert_eq!(sp["x"], json!(-0.5));
        assert!(out["buses"][0].get("room").is_none());
        assert_eq!(
            out["buses"][0]["space"],
            json!({"x": 0.0, "y": 1.0, "lfe": 0.25})
        );
        // The envelopes and the clip's channel mix pass through untouched.
        assert_eq!(
            out["tracks"][0]["envelopes"],
            base["tracks"][0]["envelopes"]
        );
        assert_eq!(
            out["tracks"][0]["clips"][0]["channel_mix"],
            base["tracks"][0]["clips"][0]["channel_mix"]
        );
        // It reads back the same.
        let (again, _) = reload(&out);
        assert_eq!(again.tracks[0].pos, s.tracks[0].pos);
        assert_eq!(again.monitor, Monitor::Speakers);
        assert!(!again.tracks[1].spatial.room);
    }

    #[test]
    fn spots_and_layouts_without_optional_fields_stay_that_way() {
        // No `listen`, no spots: nothing is added on save.
        let mut v: Value = serde_json::from_str(SAMPLE).unwrap();
        v["space"] = json!({"layout": "5.1"});
        let (mut s, base) = reload(&v);
        assert_eq!(s.monitor, Monitor::StereoFold);
        assert_eq!(saved(&mut s, &base), unmodified(&base));
        // A new track gets a spot only once it moves; then the values
        // Audiform always writes, and none it leaves out at defaults.
        let a = s.add_track();
        let b = s.add_track();
        s.track_mut(b).unwrap().pos.x = 0.5;
        let out = saved(&mut s, &base);
        let uuid = |s: &Session, id: u64| {
            json!(s
                .tracks
                .iter()
                .find(|t| t.id == id)
                .unwrap()
                .ext_id
                .clone()
                .unwrap())
        };
        let find = |out: &Value, u: &Value| {
            out["tracks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| &t["id"] == u)
                .unwrap()
                .clone()
        };
        let (ua, ub) = (uuid(&s, a), uuid(&s, b));
        assert!(find(&out, &ua).get("space").is_none());
        assert_eq!(
            find(&out, &ub)["space"],
            json!({"x": 0.5, "y": 1.0, "z": 0.0, "size": 0.0, "width": 1.0})
        );
        // Turning 3D Space off keeps the spots (and drops the layout).
        s.space = None;
        let out = saved(&mut s, &base);
        assert!(out.get("space").is_none());
        assert_eq!(find(&out, &ub)["space"]["x"], json!(0.5));
    }

    #[test]
    fn reads_everything_audiform27_uses() {
        let imp = import(SAMPLE, Path::new(".")).unwrap();
        let s = &imp.session;
        assert_eq!(s.tracks.len(), 2);
        let host = &s.tracks[0];
        let bus = &s.tracks[1];
        assert_eq!(bus.kind, TrackKind::Bus);
        assert_eq!(host.output, Some(bus.id));
        assert_eq!(host.sends[0].bus, bus.id);
        assert!(host.sends[0].pre_fader);
        assert_eq!(host.color, [0xff, 0x88, 0x00]);
        let c = &host.clips[0];
        // One second of source stretched 1.1 times.
        assert!(
            (c.start - 2.0).abs() < 1e-9
                && (c.offset - 1.0).abs() < 1e-9
                && (c.length - 1.1).abs() < 1e-9
        );
        assert!((c.fade_in - 0.1).abs() < 1e-9);
        let st = &c.props.stretch;
        assert_eq!(st.mode, StretchMode::Live);
        assert_eq!(st.kind, StretchKind::Polyphonic);
        assert_eq!((st.ratio, st.pitch), (1.1, -1.0));
        assert!(c.props.locked && !c.props.reversed && c.props.color.is_none());
        assert_eq!(c.props.fade_in_skew, 30.0);
        // Audiform's Stereo Width loads as the ported effect, bypassed.
        let w = &host.chain[1];
        assert_eq!(w.effect, "af.stereo_width");
        assert!(w.bypass);
        // AAE's Dynamics stays in its place.
        assert_eq!(host.chain[0].effect, "altaudio.dynamics");
        assert_eq!(s.tempo.bpm, 96.0);
        assert_eq!(s.tempo.beats_per_bar, 3);
        assert_eq!(s.markers[0].name, "Intro");
        assert!((s.master_db + 1.0).abs() < 1e-6);
    }

    #[test]
    fn saving_back_changes_only_what_was_edited() {
        let imp = import(SAMPLE, Path::new(".")).unwrap();
        let base = imp.base.clone().unwrap();
        let mut s = imp.session;
        // Unchanged: the document comes back the same apart from "modified".
        let mut out = export(&mut s, Some(&base));
        let mut want = base.clone();
        out.as_object_mut().unwrap().remove("modified");
        want.as_object_mut().unwrap().remove("modified");
        // Values Audiform27 writes as numbers it read as numbers compare equal.
        assert_eq!(
            out,
            want,
            "\n--- got\n{}\n--- want\n{}",
            serde_json::to_string_pretty(&out).unwrap(),
            serde_json::to_string_pretty(&want).unwrap()
        );
        // An edit touches only its own field.
        s.tracks[0].volume_db = -9.0;
        s.tracks[0].clips[0].start = 3.0;
        let out = export(&mut s, Some(&base));
        assert_eq!(out["tracks"][0]["volume_db"], json!(-9.0));
        assert_eq!(out["tracks"][0]["clips"][0]["start"], json!(144000));
        assert_eq!(out["tracks"][0]["clips"][0]["stretch"]["ratio"], json!(1.1));
        assert_eq!(
            out["tracks"][0]["clips"][0]["envelopes"][0]["id"],
            json!("e1")
        );
        assert_eq!(
            out["tracks"][0]["effects"]["slots"][0]["preset"],
            json!("Podcast Voice")
        );
        assert_eq!(out["a_field_from_the_future"]["keep"], json!("me"));
        assert_eq!(out["tempo"]["changes"][0]["bar"], json!(9));
    }

    #[test]
    fn sidechains_and_rack_levels_map_and_round_trip() {
        let text = SAMPLE
            .replace(
                r#""enabled": true, "params": {"threshold""#,
                r#""enabled": true, "sidechain": "b1", "params": {"threshold""#,
            )
            .replace(
                r#""enabled": false, "params": {"pan""#,
                r#""enabled": false, "sidechain": null, "params": {"pan""#,
            );
        let imp = import(&text, Path::new(".")).unwrap();
        let base = imp.base.clone().unwrap();
        let mut s = imp.session;
        let bus = s.tracks[1].id;
        assert_eq!(s.tracks[0].chain[0].sidechain, Some(bus));
        assert_eq!(s.tracks[0].chain[1].sidechain, None);
        let r = s.tracks[0].rack;
        assert!(!r.bypass);
        assert_eq!((r.input_db, r.output_db, r.mix), (1.5, -0.5, 0.8));
        assert_eq!(s.master_rack, Rack::default());
        // Unchanged: the same document.
        let mut out = export(&mut s, Some(&base));
        let mut want = base.clone();
        out.as_object_mut().unwrap().remove("modified");
        want.as_object_mut().unwrap().remove("modified");
        assert_eq!(out, want);
        // Edits land in Audiform's fields.
        let host = s.tracks[0].id;
        s.tracks[0].chain[0].sidechain = None;
        s.tracks[0].chain[1].sidechain = Some(bus);
        s.tracks[1].rack.bypass = true;
        s.tracks[0].rack.mix = 0.25;
        s.master_rack.output_db = -3.0;
        let out = export(&mut s, Some(&base));
        let slots = &out["tracks"][0]["effects"]["slots"];
        assert_eq!(slots[0]["sidechain"], Value::Null);
        assert_eq!(slots[1]["sidechain"], json!("b1"));
        assert_eq!(out["tracks"][0]["effects"]["mix"], json!(0.25));
        assert_eq!(out["tracks"][0]["effects"]["input_gain_db"], json!(1.5));
        assert_eq!(out["buses"][0]["effects"]["enabled"], json!(false));
        assert_eq!(out["master"]["effects"]["output_gain_db"], json!(-3.0));
        // And read back the same.
        let again = import(&out.to_string(), Path::new(".")).unwrap().session;
        assert_eq!(again.tracks[0].chain[1].sidechain, Some(again.tracks[1].id));
        assert!(again.tracks[1].rack.bypass);
        assert_eq!(again.tracks[0].rack.mix, 0.25);
        assert_eq!(again.master_rack.output_db, -3.0);
        // A key on a new slot gets the field; one listening to itself none.
        s.tracks[1].chain.push(EffectSlot {
            sidechain: Some(host),
            effect: "compressor".into(),
            values: vec![-18.0],
            bypass: false,
            data: None,
            ext_id: None,
        });
        let out = export(&mut s, Some(&base));
        assert_eq!(
            out["buses"][0]["effects"]["slots"][0]["sidechain"],
            json!("t1")
        );
        let fresh = &out["tracks"][0]["effects"]["slots"];
        assert!(fresh[0].get("sidechain").is_some());
    }

    #[test]
    fn clip_stretch_reverse_and_looks_round_trip() {
        let imp = import(SAMPLE, Path::new(".")).unwrap();
        let base = imp.base.clone().unwrap();
        let mut s = imp.session;
        {
            let c = &mut s.tracks[0].clips[0];
            c.props.reversed = true;
            c.props.inverted = true;
            c.props.locked = false;
            c.props.color = Some([0x12, 0xab, 0xff]);
            c.props.fade_out_skew = -40.0;
            let st = &mut c.props.stretch;
            st.kind = StretchKind::Varispeed;
            st.mode = StretchMode::Rendered;
            st.ratio = 2.0;
            st.window_ms = 45.0;
            c.length = 2.0; // the same second of source, at ratio 2
        }
        let want = s.tracks[0].clips[0].clone();
        let out = export(&mut s, Some(&base));
        let co = &out["tracks"][0]["clips"][0];
        // 441000 samples in the file; the span 1 s..2 s counts from its end.
        assert_eq!(co["source_in"], json!(441_000 - 88_200));
        assert_eq!(co["source_out"], json!(441_000 - 44_100));
        assert_eq!(co["reversed"], json!(true));
        assert_eq!(co["inverted"], json!(true));
        assert_eq!(co["locked"], json!(false));
        assert_eq!(co["color"], json!("#12abff"));
        assert_eq!(co["fade_out"]["curve"], json!(-0.4));
        // Unedited fade: as it was.
        assert_eq!(co["fade_in"], base["tracks"][0]["clips"][0]["fade_in"]);
        let st = &co["stretch"];
        assert_eq!(st["ratio"], json!(2));
        assert_eq!(st["keep_pitch"], json!(false));
        assert_eq!(st["rendered"], json!(true));
        assert_eq!(st["advanced"]["window_ms"], json!(45.0));
        // Read back: the same clip.
        let back = import(&out.to_string(), Path::new(".")).unwrap();
        let c = &back.session.tracks[0].clips[0];
        assert_eq!(c.props, want.props);
        assert!((c.offset - want.offset).abs() < 1e-9);
        assert!((c.length - want.length).abs() < 1e-9);
        // And saving that again changes nothing.
        let base2 = back.base.clone().unwrap();
        let mut s2 = back.session;
        let mut again = export(&mut s2, Some(&base2));
        let mut was = base2;
        again.as_object_mut().unwrap().remove("modified");
        was.as_object_mut().unwrap().remove("modified");
        assert_eq!(again, was);
        // Stretch off: the field goes to null.
        s2.tracks[0].clips[0].props.stretch.mode = StretchMode::Off;
        s2.tracks[0].clips[0].length = 1.0;
        let out = export(&mut s2, Some(&was));
        assert!(out["tracks"][0]["clips"][0]["stretch"].is_null());
        assert_eq!(
            out["tracks"][0]["clips"][0]["source_in"],
            json!(441_000 - 88_200)
        );
    }

    #[test]
    fn tempo_map_and_soundfont_round_trip() {
        let imp = import(SAMPLE, Path::new(".")).unwrap();
        let base = imp.base.clone().unwrap();
        let mut s = imp.session;
        assert_eq!(s.tempo.changes.len(), 1);
        assert_eq!(s.tempo.changes[0].bar, 9);
        assert_eq!(s.tempo.changes[0].bpm, 120.0);
        assert_eq!(s.tempo.changes[0].ext_id.as_deref(), Some("tc1"));
        // Eight bars of 3/4 at 96 BPM before the change.
        assert!((s.tempo.secs(24.0) - 15.0).abs() < 1e-9);
        // An edit writes only that value; a new change gets an id.
        s.tempo.changes[0].bpm = 130.0;
        s.tempo.add_change(12);
        let out = export(&mut s, Some(&base));
        let ch = &out["tempo"]["changes"];
        assert_eq!(ch[0]["id"], json!("tc1"));
        assert_eq!(ch[0]["bar"], json!(9));
        assert_eq!(ch[0]["bpm"], json!(130.0));
        assert_eq!(ch[1]["bar"], json!(12));
        assert!(ch[1]["id"].as_str().is_some_and(|x| !x.is_empty()));
        assert_eq!(out["tempo"]["beat_unit"], json!(4));
        let back = import(&out.to_string(), Path::new(".")).unwrap().session;
        assert_eq!(back.tempo.changes.len(), 2);
        assert_eq!(back.tempo.changes[1].bpm, 130.0);

        // A SoundFont on a MIDI track rides in Audiform27's own field.
        let mut s = Session::new("sf", 48_000, 0);
        let t = s.add_track_of(TrackKind::Midi);
        s.track_mut(t).unwrap().soundfont = Some(SoundFontRef {
            path: "C:/fonts/gm.sf2".into(),
            bank: 0,
            program: 24,
            preset_name: "Nylon Guitar".into(),
            gain_db: -3.0,
        });
        let out = export(&mut s, None);
        assert_eq!(out["tracks"][0]["af27_soundfont"]["program"], json!(24));
        let back = import(&out.to_string(), Path::new(".")).unwrap();
        assert_eq!(back.session.tracks[0].soundfont, s.tracks[0].soundfont);
        // Back to the built-in synth: the field goes.
        let base = back.base.unwrap();
        let mut s2 = back.session;
        s2.tracks[0].soundfont = None;
        let out = export(&mut s2, Some(&base));
        assert!(out["tracks"][0].get("af27_soundfont").is_none());
    }

    /// An Audiform session using buses, sends, keys, lanes, modes, MIDI
    /// with controller moves and channels, and bus positions.
    const MIX: &str = r##"{
      "format": "altaudio.session", "schema_version": 1, "id": "22222222-2222-4222-8222-222222222222",
      "name": "Mix", "sample_rate": 48000, "bit_depth": 32, "created": "x", "modified": "x", "media": [],
      "tracks": [
        {"id": "a", "name": "Voice", "role": "standard", "channels": "mono", "height": 30, "volume_db": -3.3, "pan": -25.0,
         "muted": false, "solo": false, "record_armed": false, "monitor_input": false, "mono": true, "polarity_inverted": true,
         "automation_mode": "touch", "show_automation": true,
         "output": {"type": "master"},
         "sends": [{"id": "s1", "bus_id": "rv", "level_db": -7.7, "pan": 40.0, "pre_fader": false, "enabled": false}],
         "effects": {"enabled": true, "input_gain_db": 0.0, "output_gain_db": 0.0, "mix": 1.0, "slots": [
            {"id": "sl1", "effect": "altaudio.stereo_width", "enabled": true, "params": {"pan": 0.0, "width": 100.0}}]},
         "clips": [],
         "envelopes": [
           {"id": "e1", "target": {"type": "volume"}, "enabled": true, "height": null, "points": [{"position": 0, "value": -6.0, "curve": "linear"}, {"position": 48000, "value": 0.0, "curve": "hold"}]},
           {"id": "e2", "target": {"type": "pan"}, "enabled": false, "height": 50, "points": [{"position": 24000, "value": 50.0, "curve": "linear"}]},
           {"id": "e3", "target": {"type": "effect_param", "slot_id": "sl1", "param": "width"}, "enabled": true, "points": [{"position": 0, "value": 120.0, "curve": "spline"}]}
         ]},
        {"id": "k", "name": "Keys", "kind": "midi", "role": "standard", "channels": "stereo", "height": 72, "volume_db": 0.0, "pan": 0.0,
         "muted": false, "solo": false, "record_armed": true, "monitor_input": false, "input": "Keystation 49",
         "output": {"type": "bus", "bus_id": "grp"}, "sends": [], "effects": {"enabled": true, "input_gain_db": 0.0, "output_gain_db": 0.0, "mix": 1.0, "slots": []},
         "clips": [], "envelopes": [],
         "midi_clips": [{"id": "mc", "name": "Chords", "start": 4.0, "length": 8.0, "muted": false, "color": null, "gain_db": -2.5,
           "notes": [{"pitch": 60, "velocity": 90, "start": 0.0, "length": 1.5}, {"pitch": 64, "velocity": 80, "start": 0.5, "length": 1.0, "channel": 3, "pan": -0.5}],
           "controls": [{"at": 0.0, "controller": 64, "value": 1.0, "channel": 0}, {"at": 2.0, "controller": 64, "value": 0.0, "channel": 0}]}]}
      ],
      "buses": [
        {"id": "rv", "name": "Reverb", "channels": "stereo", "height": 72, "volume_db": -6.0, "pan": 0.0, "muted": false, "solo": false,
         "output": {"type": "master"}, "sends": [], "effects": {"enabled": true, "input_gain_db": 0.0, "output_gain_db": 0.0, "mix": 1.0, "slots": []}, "envelopes": [], "position": 1},
        {"id": "grp", "name": "Group", "channels": "stereo", "height": 72, "volume_db": 0.0, "pan": 0.0, "muted": false, "solo": false, "mono": true,
         "output": {"type": "bus", "bus_id": "rv"}, "sends": [], "effects": {"enabled": true, "input_gain_db": 0.0, "output_gain_db": 0.0, "mix": 1.0, "slots": []}, "envelopes": [], "automation_mode": "latch"}
      ],
      "master": {"channels": "stereo", "height": 72, "volume_db": -0.7, "effects": {"enabled": true, "input_gain_db": 0.0, "output_gain_db": 0.0, "mix": 1.0, "slots": []},
        "envelopes": [{"id": "me", "target": {"type": "volume"}, "enabled": true, "points": [{"position": 96000, "value": -3.0, "curve": "linear"}]}]},
      "markers": [], "view": {"samples_per_pixel": 512.0, "scroll": 0, "scroll_y": 0, "playhead": 0, "selection": null},
      "modes": {"podcast": false, "voiceover": false}, "libraries": [], "template_id": null, "metadata": {},
      "tempo": {"bpm": 120.0, "beats_per_bar": 6, "beat_unit": 8, "changes": [{"id": "t2", "bar": 2, "bpm": 90.0, "beats_per_bar": 4, "beat_unit": 4}]}
    }"##;

    fn same_doc(mut a: Value, mut b: Value) {
        a.as_object_mut().unwrap().remove("modified");
        b.as_object_mut().unwrap().remove("modified");
        assert_eq!(
            a,
            b,
            "\n--- got\n{}\n--- want\n{}",
            serde_json::to_string_pretty(&a).unwrap(),
            serde_json::to_string_pretty(&b).unwrap()
        );
    }

    #[test]
    fn mix_fields_load_and_round_trip_exactly() {
        let imp = import(MIX, Path::new(".")).unwrap();
        let base = imp.base.clone().unwrap();
        let mut s = imp.session;
        // The Reverb bus shows below the first track; the Group at the end.
        let names: Vec<&str> = s.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Voice", "Reverb", "Keys", "Group"]);
        let v = &s.tracks[0];
        assert_eq!((v.pan, v.mono, v.polarity), (-0.25, true, true));
        assert_eq!(v.automation, AutomationMode::Touch);
        assert!(v.show_lanes);
        let send = &v.sends[0];
        assert_eq!((send.pan, send.enabled, send.level_db), (0.4, false, -7.7));
        assert_eq!(v.lanes.len(), 3);
        assert_eq!(v.lanes[0].value_at(0.5), Some(-6.0 + 3.0));
        assert_eq!(v.lanes[0].points[1].curve, Curve::Hold);
        assert_eq!(v.lanes[1].points[0].value, 0.5);
        assert!(!v.lanes[1].enabled);
        // Stereo Width's "width" is its second value.
        assert!(matches!(
            &v.lanes[2].target,
            LaneTarget::Effect { slot, param: 1, .. } if slot == "sl1"
        ));
        let k = &s.tracks[2];
        assert_eq!(k.midi_in.as_deref(), Some("Keystation 49"));
        let mc = &k.midi[0];
        assert_eq!(mc.gain_db, -2.5);
        assert_eq!(mc.controls.len(), 2);
        assert_eq!(mc.notes[1].channel, 3);
        assert_eq!(s.tracks[3].automation, AutomationMode::Latch);
        assert!(s.tracks[3].mono);
        assert_eq!(s.master_lanes[0].points[0].time, 2.0);
        assert_eq!(s.tempo.beat_unit, 8);
        // Unchanged: the same document.
        same_doc(export(&mut s, Some(&base)), base.clone());

        // Edits land in Audiform's fields and units.
        s.tracks[0].sends[0].enabled = true;
        s.tracks[0].sends[0].pan = -1.0;
        s.tracks[0].pan = 0.5;
        s.tracks[0].mono = false;
        s.tracks[0].automation = AutomationMode::Read;
        s.tracks[0].lanes[1].points[0].value = -0.25;
        s.tracks[2].midi[0].notes[0].channel = 2;
        s.tracks[2].midi[0].controls.clear();
        s.tracks[2].midi_in = None;
        // Move the Reverb bus to the end.
        let rv = s.tracks.remove(1);
        s.tracks.push(rv);
        let out = export(&mut s, Some(&base));
        let t0 = &out["tracks"][0];
        assert_eq!(t0["pan"], json!(50.0));
        assert!(t0.get("mono").is_none());
        assert!(t0.get("automation_mode").is_none());
        assert_eq!(t0["volume_db"], json!(-3.3));
        assert_eq!(t0["sends"][0]["enabled"], json!(true));
        assert_eq!(t0["sends"][0]["pan"], json!(-100.0));
        assert_eq!(t0["sends"][0]["level_db"], json!(-7.7));
        assert_eq!(t0["envelopes"][1]["points"][0]["value"], json!(-25.0));
        assert_eq!(t0["envelopes"][2]["target"]["param"], json!("width"));
        let notes = &out["tracks"][1]["midi_clips"][0]["notes"];
        assert_eq!(notes[0]["channel"], json!(2));
        assert!(notes[1].get("pan").is_some());
        assert!(out["tracks"][1]["midi_clips"][0].get("controls").is_none());
        assert!(out["tracks"][1]["input"].is_null());
        assert!(out["tracks"][1].get("af27_synth").is_none());
        let rv = out["buses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["id"] == "rv")
            .unwrap();
        assert_eq!(rv["position"], json!(2));
        // And it reads back as edited.
        let back = import(&out.to_string(), Path::new(".")).unwrap().session;
        assert_eq!(back.tracks[0].sends[0].pan, -1.0);
        assert_eq!(back.tracks[0].lanes[1].points[0].value, -0.25);
        assert_eq!(back.tracks.last().unwrap().name, "Reverb");
    }

    #[test]
    fn new_objects_get_every_required_field() {
        let mut s = Session::new("New", 48_000, 1);
        let bus = s.add_track_of(TrackKind::Bus);
        s.tracks[0].output = Some(bus);
        s.tracks[0].chain.push(EffectSlot {
            sidechain: None,
            effect: "compressor".into(),
            values: vec![-18.0, 3.0],
            bypass: false,
            data: None,
            ext_id: None,
        });
        s.markers.push(Marker {
            name: "M".into(),
            time: 1.0,
            length: 0.0,
            ext_id: None,
        });
        let out = export(&mut s, None);
        for k in [
            "format",
            "schema_version",
            "id",
            "name",
            "sample_rate",
            "bit_depth",
            "created",
            "modified",
            "media",
            "tracks",
            "buses",
            "master",
            "markers",
            "view",
            "modes",
            "libraries",
            "metadata",
        ] {
            assert!(out.get(k).is_some(), "missing {k}");
        }
        let t = &out["tracks"][0];
        for k in [
            "id",
            "name",
            "role",
            "channels",
            "height",
            "volume_db",
            "pan",
            "muted",
            "solo",
            "record_armed",
            "monitor_input",
            "output",
            "sends",
            "effects",
            "clips",
            "envelopes",
        ] {
            assert!(t.get(k).is_some(), "track missing {k}");
        }
        assert_eq!(t["output"]["type"], json!("bus"));
        assert_eq!(t["effects"]["slots"][0]["effect"], json!("af27.compressor"));
        // Ids were given to the session, so the next save keeps them.
        assert!(s.tracks[0].ext_id.is_some());
        // And it reads back the same.
        let again = import(&out.to_string(), Path::new(".")).unwrap().session;
        assert_eq!(again.tracks[0].chain[0].effect, "compressor");
        assert_eq!(again.tracks[0].chain[0].values, vec![-18.0, 3.0]);
        assert_eq!(again.tracks[0].output, Some(again.tracks[1].id));
    }
}

#[cfg(test)]
mod plugin_slots {
    use super::*;

    const ID: &str = "vst3:5454535630357432723520636C617373";

    fn doc() -> String {
        let slot = format!(
            r#"{{"id": "fx9", "effect": "{ID}", "enabled": true, "params": {{"p458681842": 0.25, "p7": 1}}, "plugin_state": "AAECAw==", "preset": null}}"#
        );
        let master = r#""volume_db": -1.0, "effects": {"enabled": true, "input_gain_db": 0.0, "output_gain_db": 0.0, "mix": 1.0, "slots": []}"#;
        super::tests::SAMPLE.replacen(
            master,
            &master.replace(r#""slots": []"#, &format!(r#""slots": [{slot}]"#)),
            1,
        )
    }

    #[test]
    fn plug_in_slots_load_with_their_settings_and_round_trip() {
        let text = doc();
        assert!(text.contains("vst3:"), "the sample has a slot to replace");
        let imp = import(&text, Path::new(".")).unwrap();
        let base = imp.base.clone().unwrap();
        let mut s = imp.session;
        let slot = s.master_chain[0].clone();
        assert_eq!(slot.effect, ID);
        let d = slot.plugin_data();
        assert_eq!(d.inst, "fx9", "the running copy is named after the slot");
        assert_eq!(d.state.as_deref(), Some("AAECAw=="));
        assert_eq!(d.param(458_681_842), Some(0.25));
        assert_eq!(d.param(7), Some(1.0));
        assert!(!slot.bypass);
        assert!(
            imp.notices.iter().any(|n| n.starts_with("1 effect(s)")),
            "only the sample's own Dynamics counts as missing, not the plug-in: {:?}",
            imp.notices
        );
        // Unchanged: the file comes back exactly (`1` stays `1`).
        let mut got = export(&mut s.clone(), Some(&base));
        let mut want = base.clone();
        want.as_object_mut().unwrap().remove("modified");
        got.as_object_mut().unwrap().remove("modified");
        assert_eq!(got, want);
        // A changed control and new settings go back; the rest stays.
        let mut d = s.master_chain[0].plugin_data();
        d.set_param(458_681_842, 0.5);
        d.state = Some("BBBB".into());
        s.master_chain[0].set_plugin_data(&d);
        let out = export(&mut s, Some(&base));
        let o = &out["master"]["effects"]["slots"][0];
        assert_eq!(o["params"]["p458681842"], json!(0.5));
        assert_eq!(o["plugin_state"], json!("BBBB"));
        assert_eq!(o["preset"], Value::Null);
        assert_eq!(o["effect"], json!(ID));
        assert!(o.get("data").is_none(), "no data field on plug-in slots");
        // A new plug-in slot gets every field Audiform needs.
        let mut fresh = s.master_chain[0].clone();
        fresh.ext_id = None;
        let mut d = fresh.plugin_data();
        d.params.clear();
        d.set_param(3, 0.75);
        d.state = None;
        fresh.set_plugin_data(&d);
        s.master_chain.push(fresh);
        let out = export(&mut s, Some(&base));
        let n = &out["master"]["effects"]["slots"][1];
        assert_eq!(n["effect"], json!(ID));
        assert_eq!(n["enabled"], json!(true));
        assert_eq!(n["params"], json!({"p3": 0.75}));
        assert!(n["id"].is_string());
        assert!(n.get("plugin_state").is_none());
    }
}

/// Round-trips every Audiform session under `AF27_AUDFRM_DIR`:
/// `cargo test -p af27-interop real_sessions -- --ignored`.
#[cfg(test)]
mod real {
    #[test]
    #[ignore]
    fn real_sessions_round_trip_exactly() {
        let Some(dir) = std::env::var_os("AF27_AUDFRM_DIR") else {
            return;
        };
        let mut n = 0;
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
                if ext != "audfrm" && ext != "altaudio" {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap();
                let imp = super::import(&text, p.parent().unwrap()).unwrap();
                let base = imp.base.clone().unwrap();
                let mut s = imp.session;
                let mut out = super::export(&mut s, Some(&base));
                let mut want = base;
                out.as_object_mut().unwrap().remove("modified");
                want.as_object_mut().unwrap().remove("modified");
                // Fields Audiform27 adds that older files did not have.
                for k in ["tempo"] {
                    if want.get(k).is_none() {
                        out.as_object_mut().unwrap().remove(k);
                    }
                }
                assert_eq!(out, want, "{} changed on a load and save", p.display());
                n += 1;
            }
        }
        eprintln!("{n} sessions round-tripped exactly");
        assert!(n > 0);
    }
}
