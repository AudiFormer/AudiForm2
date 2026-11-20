// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! `.sesx` multitrack sessions (the XML session format of a widely used
//! commercial editor): read into an Audiform27 session.
//!
//! Units in the file, as checked against real sessions on the project
//! lead's machine:
//! * positions (`startPoint`, `endPoint`, keyframe `sampleOffset`) are
//!   samples at the session rate;
//! * a clip's `sourceInPoint`/`sourceOutPoint` are samples at the session
//!   rate too, in stretched time: every clip in the real sessions has
//!   `sourceOutPoint - sourceInPoint == endPoint - startPoint`, stretched
//!   or not, and files of another rate are played through copies
//!   converted to the session rate. So the point in the file, in seconds,
//!   is `sourceInPoint / ratio / rate`;
//! * pan is -100..100 (real sessions hold values up to 100), and drawn
//!   pan keyframes are 0..1 with 0.5 in the middle;
//! * gains (faders, sends, rack levels) are linear factors.
//!
//! Everything that cannot come across is counted in the [`Report`] and
//! named in the import notice; nothing is left out silently.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use af27_core::automation::{Curve, Lane, LanePoint, LaneTarget};
use af27_core::plugin::PluginData;
use af27_core::session::{
    Clip, ClipStretch, EffectSlot, Marker, MediaItem, Rack, Send, Session, StretchKind,
    StretchMode, TrackId, TrackKind, CHAIN_SLOTS,
};
use af27_core::space::{SpaceLayout, SpacePos};
use roxmltree::Node;

use crate::Imported;

/// Where a 5.1 session's surround speakers were when it was mixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Surrounds {
    /// Behind the listener (the usual placement).
    #[default]
    Behind,
    /// Beside the listener.
    Sides,
}

/// What came across from a `.sesx`, and what did not.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Report {
    pub tracks: usize,
    pub buses: usize,
    pub clips: usize,
    /// Plug-ins brought in with their saved settings, by name.
    pub plugins: Vec<String>,
    /// Built-in effects turned into Audiform27's own, by kind.
    pub converted: BTreeMap<String, usize>,
    /// Built-in effects left out, by name (each once), and how many.
    pub skipped_names: Vec<String>,
    pub skipped: usize,
    pub stretched: usize,
    pub sends: usize,
    pub markers: usize,
    /// Drawn volume and pan lines that became automation lanes.
    pub lines: usize,
    /// Things that did not come across, by what they are.
    pub dropped: BTreeMap<String, usize>,
    /// Things that came across as an estimate, by what they are.
    pub approximated: BTreeMap<String, usize>,
    pub missing_files: usize,
}

impl Report {
    fn drop(&mut self, what: &str) {
        *self.dropped.entry(what.into()).or_default() += 1;
    }
    fn approx(&mut self, what: &str) {
        *self.approximated.entry(what.into()).or_default() += 1;
    }
    fn convert(&mut self, what: &str) {
        *self.converted.entry(what.into()).or_default() += 1;
    }
    fn skip(&mut self, name: &str) {
        self.skipped += 1;
        if !name.is_empty() && !self.skipped_names.iter().any(|n| n == name) {
            self.skipped_names.push(name.into());
        }
    }

    /// The import notice, one line per subject.
    pub fn notices(&self) -> Vec<String> {
        let mut out = vec![format!(
            "Imported {} track(s), {} bus(es) and {} clip(s).",
            self.tracks, self.buses, self.clips
        )];
        if !self.plugins.is_empty() {
            let mut names = self.plugins.clone();
            names.sort();
            names.dedup();
            out.push(format!(
                "{} plug-in(s) came across with their settings ({}); any not installed here show in the Effects Rack until they are installed and scanned.",
                self.plugins.len(),
                list(&names)
            ));
        }
        for (what, n) in &self.converted {
            out.push(format!("{n} {what}."));
        }
        if self.stretched > 0 {
            out.push(format!(
                "{} clip(s) keep their stretch and pitch.",
                self.stretched
            ));
        }
        if self.lines > 0 {
            out.push(format!(
                "{} drawn volume or pan line(s) came across as automation.",
                self.lines
            ));
        }
        if self.sends > 0 {
            out.push(format!("{} send(s) came across.", self.sends));
        }
        if self.markers > 0 {
            out.push(format!("{} marker(s) came across.", self.markers));
        }
        if self.skipped > 0 {
            out.push(format!(
                "{} built-in effect(s) were left out ({}); rebuild them in the Effects Rack.",
                self.skipped,
                list(&self.skipped_names)
            ));
        }
        if !self.dropped.is_empty() {
            let parts: Vec<String> = self
                .dropped
                .iter()
                .map(|(w, n)| format!("{n} {w}"))
                .collect();
            out.push(format!("Not carried over: {}.", parts.join(", ")));
        }
        if !self.approximated.is_empty() {
            let parts: Vec<String> = self
                .approximated
                .iter()
                .map(|(w, n)| format!("{n} {w}"))
                .collect();
            out.push(format!(
                "Carried over as an estimate: {}.",
                parts.join(", ")
            ));
        }
        if self.missing_files > 0 {
            out.push(format!(
                "{} media file(s) were not found; their clips stay in place (use Link Media to find them).",
                self.missing_files
            ));
        }
        out
    }
}

fn list(names: &[String]) -> String {
    const SHOWN: usize = 6;
    let mut s = names
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > SHOWN {
        s.push_str(&format!(" and {} more", names.len() - SHOWN));
    }
    s
}

// ------------------------------------------------------------ helpers

fn num(n: Node, k: &str) -> Option<f64> {
    n.attribute(k)?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

fn child<'a, 'i>(n: Node<'a, 'i>, tag: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(tag))
}

/// The first `component` among the children of `nodes` whose
/// `componentID` is `cid` (or, in older files, whose `id` is `alias`).
fn comp<'a, 'i>(nodes: &[Node<'a, 'i>], cid: &str, alias: &str) -> Option<Node<'a, 'i>> {
    nodes.iter().find_map(|n| {
        n.children().find(|c| {
            c.has_tag_name("component")
                && (c.attribute("componentID") == Some(cid)
                    || (c.attribute("componentID").is_none() && c.attribute("id") == Some(alias)))
        })
    })
}

fn param<'a, 'i>(comp: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    comp.children()
        .find(|p| p.has_tag_name("parameter") && p.attribute("name") == Some(name))
}

fn param_at<'a, 'i>(comp: Node<'a, 'i>, index: &str) -> Option<Node<'a, 'i>> {
    comp.children()
        .find(|p| p.has_tag_name("parameter") && p.attribute("index") == Some(index))
}

fn value(p: Node) -> Option<f64> {
    num(p, "parameterValue")
}

/// A parameter's drawn keyframes: (sample, value, holds). They sit
/// straight under the parameter or inside `<parameterKeyframes>`,
/// depending on the version that saved the file.
fn keyframes(p: Node) -> Vec<(f64, f64, bool)> {
    let mut k: Vec<(f64, f64, bool)> = p
        .descendants()
        .filter(|n| n.has_tag_name("parameterKeyframe"))
        .filter_map(|n| {
            Some((
                num(n, "sampleOffset")?.max(0.0),
                num(n, "value")?,
                n.attribute("type") == Some("hold"),
            ))
        })
        .collect();
    k.sort_by(|a, b| a.0.total_cmp(&b.0));
    k
}

/// A linear gain in dB (-96 for silence), to 0.01 dB.
pub fn gain_db(g: f64) -> f32 {
    if g <= 1e-6 {
        -96.0
    } else {
        ((20.0 * g.log10()) * 100.0).round() as f32 / 100.0
    }
}

fn on(v: Option<f64>) -> bool {
    v.is_some_and(|v| v >= 0.5)
}

/// A hue in degrees as a colour (None for -1, "no colour").
fn hue_rgb(h: Option<f64>) -> Option<[u8; 3]> {
    let h = h.filter(|h| *h >= 0.0)?.rem_euclid(360.0);
    let (s, v) = (0.5, 0.85);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let b8 = |u: f64| ((u + m) * 255.0).round() as u8;
    Some([b8(r), b8(g), b8(b)])
}

/// A fader: the fixed level in dB and, when drawn, the volume line as
/// (seconds from `origin`, dB). The fader keeps two gains that multiply:
/// `volume` (the one drawn on) and `static gain`. With a line, the line
/// is the volume and the static gain alone is the fixed level.
fn level(nodes: &[Node], cid_alias: &str, rate: f64) -> (f32, Option<Vec<LanePoint>>) {
    let Some(f) = comp(nodes, "Audition.Fader", cid_alias) else {
        return (0.0, None);
    };
    let vol = param(f, "volume").or_else(|| param_at(f, "0"));
    let sg = param(f, "static gain").and_then(value).unwrap_or(1.0);
    let v = vol.and_then(value).unwrap_or(1.0);
    let kf = vol.map(keyframes).unwrap_or_default();
    if kf.is_empty() {
        return (gain_db(v * sg), None);
    }
    let pts = kf
        .iter()
        .map(|&(at, g, hold)| LanePoint {
            time: at / rate,
            value: gain_db(g) as f64,
            curve: if hold { Curve::Hold } else { Curve::Linear },
        })
        .collect();
    (gain_db(sg), Some(pts))
}

/// Pan as -1..1 and its drawn line.
fn pan(nodes: &[Node], rate: f64) -> (f32, Option<Vec<LanePoint>>) {
    let Some(c) = comp(nodes, "Audition.StereoPanner", "trackPan") else {
        return (0.0, None);
    };
    let Some(p) = param(c, "Pan").or_else(|| param_at(c, "0")) else {
        return (0.0, None);
    };
    let fixed = (value(p).unwrap_or(0.0) / 100.0).clamp(-1.0, 1.0) as f32;
    let kf = keyframes(p);
    if kf.is_empty() {
        return (fixed, None);
    }
    let pts = kf
        .iter()
        .map(|&(at, v, hold)| LanePoint {
            time: at / rate,
            value: (v * 2.0 - 1.0).clamp(-1.0, 1.0),
            curve: if hold { Curve::Hold } else { Curve::Linear },
        })
        .collect();
    (fixed, Some(pts))
}

fn muted(nodes: &[Node], alias: &str) -> bool {
    comp(nodes, "Audition.Mute", alias)
        .and_then(|c| param(c, "mute").or_else(|| param_at(c, "1")))
        .and_then(value)
        .is_some_and(|v| v >= 0.5)
}

/// Counts drawn lines on parameters other than the ones read.
fn other_lines(nodes: &[Node], report: &mut Report) {
    for n in nodes {
        for c in n.children().filter(|c| c.has_tag_name("component")) {
            let id = c.attribute("componentID").unwrap_or_default();
            if matches!(id, "Audition.Fader" | "Audition.StereoPanner") || id.ends_with("51Panner")
            {
                continue;
            }
            for p in c.children().filter(|p| p.has_tag_name("parameter")) {
                if !keyframes(p).is_empty() {
                    report.drop("other automation line(s)");
                }
            }
        }
    }
}

// ---------------------------------------------------------- effects

/// Parametric Equalizer.
const PARAMETRIC_EQ: &str = "8edcd326-33de-4a4d-985b-f1bf48ed1427";
/// Hard Limiter.
const HARD_LIMITER: &str = "e0b23f05-f1a7-4ef7-9b50-7ec3e3002058";
/// Graphic Phase Shifter (each frequency turned by its own angle).
const PHASE_SHIFTER: &str = "7354704a-d87c-11e0-b05b-763c4824019b";
/// The xvox vocal strip plug-in.
const XVOX: &str = "3:abcdef01 9182faeb 58766f78 58766f78";

/// The rack's input and output levels (dB) and dry/wet mix, from its
/// `rackInput`, `rackOutput` and `rackMix` components. The mix is read as
/// the dry share (0 is fully wet).
fn rack_levels(rack: Node) -> Rack {
    let mut r = Rack::default();
    for c in rack.children().filter(|c| c.has_tag_name("component")) {
        if c.attribute("powered") == Some("false") {
            continue;
        }
        let Some(v) = param_at(c, "0").and_then(value) else {
            continue;
        };
        let db = || ((20.0 * v.max(1e-6).log10()) * 10.0).round() as f32 / 10.0;
        match c.attribute("id") {
            Some("rackInput") => r.input_db = db(),
            Some("rackOutput") => r.output_db = db(),
            Some("rackMix") => r.mix = (1.0 - v).clamp(0.0, 1.0) as f32,
            _ => {}
        }
    }
    r
}

/// A component's parameters by name.
fn named_params(c: Node) -> HashMap<String, f64> {
    c.children()
        .filter(|p| p.has_tag_name("parameter"))
        .filter_map(|p| Some((p.attribute("name")?.to_string(), value(p)?)))
        .collect()
}

fn chunk(c: Node) -> Option<Vec<u8>> {
    let text = child(c, "presetChunkData")?.text()?;
    crate::b64::decode(text.trim())
}

/// A slot of one of Audiform27's own effects (from `af27_fx`), with
/// `values` by key and the rest at their defaults, each in range.
fn fx_slot(id: &str, values: &[(&str, f64)]) -> Option<EffectSlot> {
    let info = af27_fx::info(id)?;
    let v = info
        .params
        .iter()
        .map(|p| {
            let x = values
                .iter()
                .find(|(k, _)| *k == p.key)
                .map_or(p.default, |&(_, x)| x);
            x.clamp(p.min.min(p.max), p.max.max(p.min)) as f32
        })
        .collect();
    Some(slot(id, v))
}

fn slot(effect: &str, values: Vec<f32>) -> EffectSlot {
    EffectSlot {
        effect: effect.into(),
        values,
        bypass: false,
        sidechain: None,
        data: None,
        position: None,
        ext_id: None,
    }
}

/// Hard Limiter to the Limiter. Maximum Amplitude is 1 at 0 dB and each
/// 0.01 below is 1 dB; Input Boost is an estimate (2/3 is 0 dB, 30 dB
/// across the knob). Look-ahead and release keep their defaults.
pub fn limiter(p: &HashMap<String, f64>) -> EffectSlot {
    let e = af27_dsp::Effect::Limiter;
    let mut v: Vec<f32> = e.params().iter().map(|p| p.default).collect();
    if let Some(a) = p.get("Maximum Amplitude") {
        v[0] = (((a - 1.0) * 100.0).clamp(-24.0, 0.0) * 10.0).round() as f32 / 10.0;
    }
    if let Some(b) = p.get("Input Boost") {
        v[1] = (((b - 2.0 / 3.0) * 30.0).clamp(0.0, 24.0) * 10.0).round() as f32 / 10.0;
    }
    slot(e.id(), v)
}

fn round_to(x: f64, step: f64) -> f64 {
    (x / step).round() * step
}

/// Parametric Equalizer to Audiform's Parametric EQ keys
/// (`altaudio.parametric_eq`), kept within that effect's ranges. Knobs
/// are 0..1: frequencies run from 20 Hz to half the "Original Sample
/// Rate" (a share of 192 kHz), gains ±48 dB with 0.5 at 0, overall gain
/// -40..20 dB, Q is the knob times 10000, pass slopes 12 to 42 dB/oct.
pub fn parametric_eq(p: &HashMap<String, f64>) -> (BTreeMap<String, f64>, bool) {
    let nyq = p
        .get("Original Sample Rate")
        .map_or(22_050.0, |r| r * 192_000.0 / 2.0)
        .max(1000.0);
    let hz = |v: f64| (20.0 + v * (nyq - 20.0)).round();
    let db = |v: f64| round_to((v - 0.5) * 96.0, 0.01).clamp(-24.0, 24.0);
    let slope = |v: f64| {
        let d = 12.0 + (v * 5.0).round() * 6.0;
        if d < 18.0 {
            0.0
        } else if d < 36.0 {
            1.0
        } else {
            2.0
        }
    };
    let mut out = BTreeMap::new();
    let mut put = |k: &str, v: f64| {
        out.insert(k.to_string(), v);
    };
    for (name, &v) in p {
        let n = name.as_str();
        match n {
            "Gain" => put("gain", round_to(v * 60.0 - 40.0, 0.1)),
            "High-Pass Filter Enable" => put("hp_on", on(Some(v)) as u8 as f64),
            "High-Pass Cutoff Frequency" => put("hp_freq", hz(v).clamp(10.0, 2000.0)),
            "High-Pass Slope" => put("hp_slope", slope(v)),
            "Low Shelf Enable" => put("ls_on", on(Some(v)) as u8 as f64),
            "Low Shelf Frequency" => put("ls_freq", hz(v)),
            "Low Shelf Gain" => put("ls_gain", db(v)),
            "High Shelf Enable" => put("hs_on", on(Some(v)) as u8 as f64),
            "High Shelf Frequency" => put("hs_freq", hz(v).clamp(1000.0, 20_000.0)),
            "High Shelf Gain" => put("hs_gain", db(v)),
            "Low-Pass Filter Enable" => put("lp_on", on(Some(v)) as u8 as f64),
            "Low-Pass Cutoff Frequency" => put("lp_freq", hz(v).clamp(1000.0, 22_000.0)),
            "Low-Pass Slope" => put("lp_slope", slope(v)),
            _ => {
                // "EQ Band N Enable", "... Center Frequency", "... Gain", "... Q".
                let Some(rest) = n.strip_prefix("EQ Band ") else {
                    continue;
                };
                let Some((band, what)) = rest.split_once(' ') else {
                    continue;
                };
                if !matches!(band, "1" | "2" | "3" | "4" | "5") {
                    continue;
                }
                match what {
                    "Enable" => put(&format!("b{band}_on"), on(Some(v)) as u8 as f64),
                    "Center Frequency" => put(&format!("b{band}_freq"), hz(v)),
                    "Gain" => put(&format!("b{band}_gain"), db(v)),
                    "Q" => put(
                        &format!("b{band}_q"),
                        round_to(v * 10_000.0, 0.01).clamp(0.1, 100.0),
                    ),
                    _ => {}
                }
            }
        }
    }
    let bypass = on(p.get("Bypass").copied());
    (out, bypass)
}

/// Graphic Phase Shifter settings (little-endian floats) to a Phase
/// Curve: [0] sample rate, [1] the channel turned (0 left, 1 right),
/// [6] and [7] the axis ends as shares of half the rate, [10] the point
/// count, then (x along a log axis, phase 0..1 for -180..180) per point.
pub fn phase_curve(bytes: &[u8]) -> Option<EffectSlot> {
    let f: Vec<f64> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b) as f64)
        .collect();
    let rate = *f.first()?;
    if !(8000.0..=768_000.0).contains(&rate) || f.len() < 11 {
        return None;
    }
    let nyq = rate / 2.0;
    let lo = (f[6] * nyq).clamp(10.0, nyq);
    let hi = (f[7] * nyq).clamp(lo + 1.0, nyq.max(lo + 1.0));
    let n = f[10];
    if !(2.0..=4096.0).contains(&n) {
        return None;
    }
    let n = n as usize;
    if f.len() < 11 + 2 * n {
        return None;
    }
    let mut pts: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            let (x, y) = (f[11 + 2 * i], f[12 + 2 * i]);
            (
                round_to(lo * (hi / lo).powf(x.clamp(0.0, 1.0)), 0.1),
                round_to((y - 0.5) * 360.0, 0.1),
            )
        })
        .collect();
    let max = af27_fx::effects::phase_curve::MAX_POINTS;
    if pts.len() > max {
        // Thinned evenly, keeping the first and last.
        let last = pts.len() - 1;
        pts = (0..max)
            .map(|i| pts[(i * last + (max - 1) / 2) / (max - 1)])
            .collect();
    }
    let mut vals: Vec<(String, f64)> = vec![
        ("points".into(), pts.len() as f64),
        ("spline".into(), 0.0),
        ("channels".into(), if f[1] >= 0.5 { 2.0 } else { 1.0 }),
    ];
    for (i, (hz, deg)) in pts.iter().enumerate() {
        vals.push((af27_fx::effects::phase_curve::FREQ_KEYS[i].to_string(), *hz));
        vals.push((
            af27_fx::effects::phase_curve::PHASE_KEYS[i].to_string(),
            *deg,
        ));
    }
    let refs: Vec<(&str, f64)> = vals.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    fx_slot(af27_fx::effects::phase_curve::ID, &refs)
}

/// The xvox plug-in's settings (an `<XVox>` block of `<PARAM id value>`
/// inside its state) to Vox Strip. None when the block is not there.
pub fn vox_strip(state: &[u8]) -> Option<EffectSlot> {
    let text = String::from_utf8_lossy(state);
    let start = text.find("<XVox")?;
    let end = text[start..]
        .find("</XVox>")
        .map_or(text.len(), |e| start + e);
    let block = &text[start..end];
    let mut params: HashMap<&str, f64> = HashMap::new();
    let mut rest = block;
    while let Some(i) = rest.find("<PARAM") {
        rest = &rest[i + 6..];
        let tag = &rest[..rest.find('>').unwrap_or(rest.len())];
        let attr = |name: &str| {
            let key = format!("{name}=\"");
            let s = tag.find(&key)? + key.len();
            Some(&tag[s..s + tag[s..].find('"')?])
        };
        if let (Some(id), Some(v)) = (attr("id"), attr("value").and_then(|v| v.parse().ok())) {
            params.insert(id, v);
        }
    }
    if params.is_empty() {
        return None;
    }
    let get = |k: &str| params.get(k).copied();
    let db = |g: f64| 20.0 * g.max(1e-4).log10();
    let mut vals: Vec<(&str, f64)> = Vec::new();
    type Knob = (&'static str, &'static str, fn(f64) -> f64);
    let pairs: [Knob; 17] = [
        ("hpf", "low_cut", |v| (v >= 0.5) as u8 as f64),
        ("dyn_enable", "dyn_on", |v| (v >= 0.5) as u8 as f64),
        ("dyn_mode", "dyn_style", f64::round),
        ("dyn_comp", "comp", |v| (v * 100.0).round()),
        ("deesser_amount", "deess", |v| (v * 100.0).round()),
        ("dyn_trim", "dyn_trim", |v| v),
        ("tone_enable", "tone_on", |v| (v >= 0.5) as u8 as f64),
        ("tone_mode", "tone_style", f64::round),
        ("tone_low", "low", |v| v * 12.0),
        ("tone_mid", "mid", |v| v * 12.0),
        ("tone_high", "high", |v| v * 12.0),
        ("tone_exciter_gain", "air", |v| (v * 100.0).round()),
        ("tone_trim", "tone_trim", |v| v),
        ("sfx_enable", "fx_on", |v| (v >= 0.5) as u8 as f64),
        ("sfx_mode", "fx_style", f64::round),
        ("sfx_control", "fx", |v| (v * 100.0).round()),
        ("sfx_trim", "fx_trim", |v| v),
    ];
    for (from, to, f) in pairs {
        if let Some(v) = get(from) {
            vals.push((to, f(v)));
        }
    }
    if let Some(g) = get("input_gain") {
        vals.push(("input", db(g)));
    }
    if let Some(g) = get("output_gain") {
        vals.push(("output", db(g)));
    }
    let mut s = fx_slot(af27_fx::effects::vocal::VOCAL, &vals)?;
    s.bypass = get("globalEnable").is_some_and(|v| v < 0.5);
    Some(s)
}

/// A VST3 class id written `3:aaaaaaaa bbbbbbbb cccccccc dddddddd` as the
/// slot id `vst3:<32 hex>`. `com` gives Windows' GUID byte layout (the
/// first word and the two halves of the second little-endian), which is
/// how plug-ins are known on Windows; otherwise the words in order.
pub fn vst3_effect(id: &str, com: bool) -> Option<String> {
    let words: Vec<u32> = id
        .strip_prefix("3:")?
        .split_whitespace()
        .map(|w| u32::from_str_radix(w, 16).ok())
        .collect::<Option<_>>()?;
    let [a, b, c, d] = words[..] else {
        return None;
    };
    let mut bytes = Vec::with_capacity(16);
    if com {
        bytes.extend(a.to_le_bytes());
        bytes.extend(((b >> 16) as u16).to_le_bytes());
        bytes.extend((b as u16).to_le_bytes());
    } else {
        bytes.extend(a.to_be_bytes());
        bytes.extend(b.to_be_bytes());
    }
    bytes.extend(c.to_be_bytes());
    bytes.extend(d.to_be_bytes());
    let hex: String = bytes.iter().map(|x| format!("{x:02X}")).collect();
    Some(format!("{}{hex}", af27_core::plugin::PREFIX))
}

/// A plug-in's saved settings: the file holds `u64` component length,
/// `u64` controller length, then both blobs; Audiform keeps a `u32`
/// component length, then both blobs. None when the lengths do not add
/// up (the plug-in then starts from its defaults).
pub fn chunk_to_state(chunk: &[u8]) -> Option<Vec<u8>> {
    let len = |at: usize| -> Option<usize> {
        Some(u64::from_le_bytes(chunk.get(at..at + 8)?.try_into().ok()?) as usize)
    };
    let (comp, ctl) = (len(0)?, len(8)?);
    if comp.checked_add(ctl)?.checked_add(16)? != chunk.len() || comp > u32::MAX as usize {
        return None;
    }
    let mut out = Vec::with_capacity(4 + comp + ctl);
    out.extend((comp as u32).to_le_bytes());
    out.extend(&chunk[16..]);
    Some(out)
}

/// The other way, for writing a `.sesx`.
pub fn state_to_chunk(state: &[u8]) -> Option<Vec<u8>> {
    let comp = u32::from_le_bytes(state.get(..4)?.try_into().ok()?) as usize;
    let ctl = state.len().checked_sub(4 + comp)?;
    let mut out = Vec::with_capacity(16 + comp + ctl);
    out.extend((comp as u64).to_le_bytes());
    out.extend((ctl as u64).to_le_bytes());
    out.extend(&state[4..]);
    Some(out)
}

fn plugin_slot(c: Node, id: &str, report: &mut Report) -> Option<EffectSlot> {
    let effect = vst3_effect(id, cfg!(windows))?;
    let name = c.attribute("name").unwrap_or("Plug-in").to_string();
    let state = chunk(c)
        .and_then(|ch| chunk_to_state(&ch))
        .map(|s| crate::b64::encode(&s));
    let data = PluginData {
        inst: af27_core::plugin::new_inst(),
        name: name.clone(),
        state,
        ..Default::default()
    };
    report.plugins.push(name);
    let mut s = slot(&effect, Vec::new());
    s.data = Some(data.to_json());
    Some(s)
}

/// One rack slot's component as an effect slot (or None, counted).
fn convert(c: Node, report: &mut Report) -> Option<EffectSlot> {
    let id = c.attribute("componentID").unwrap_or_default();
    let name = c.attribute("name").unwrap_or_default();
    match id {
        PHASE_SHIFTER => {
            if let Some(s) = chunk(c).as_deref().and_then(phase_curve) {
                report.convert("phase shifter(s) came across as Phase Curve");
                return Some(s);
            }
        }
        HARD_LIMITER => {
            report.convert("Hard Limiter(s) came across as Limiter");
            return Some(limiter(&named_params(c)));
        }
        PARAMETRIC_EQ => {
            let (params, bypass) = parametric_eq(&named_params(c));
            report.convert("Parametric Equalizer(s) came across as Parametric EQ");
            let vals: Vec<(&str, f64)> = params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
            let mut s = fx_slot(af27_fx::effects::parametric_eq::ID, &vals)?;
            s.bypass = bypass;
            return Some(s);
        }
        XVOX => {
            let state = chunk(c).map(|ch| chunk_to_state(&ch).unwrap_or(ch));
            if let Some(s) = state.as_deref().and_then(vox_strip) {
                report.convert("xvox strip(s) came across as Vox Strip");
                return Some(s);
            }
        }
        _ => {}
    }
    if id.starts_with("3:") {
        if let Some(s) = plugin_slot(c, id, report) {
            return Some(s);
        }
    }
    report.skip(name);
    None
}

/// Every `effectsRack` directly inside `node`, as one chain (up to 16
/// slots, each keeping its slot number) and the first rack's levels.
fn racks(node: Node, report: &mut Report) -> (Vec<EffectSlot>, Rack) {
    let mut chain: Vec<EffectSlot> = Vec::new();
    let mut levels = None;
    for rack in node.children().filter(|c| c.has_tag_name("effectsRack")) {
        levels.get_or_insert_with(|| rack_levels(rack));
        let mut slots: Vec<Node> = rack
            .children()
            .filter(|s| s.has_tag_name("slot") && child(*s, "component").is_some())
            .collect();
        slots.sort_by_key(|s| num(*s, "slotIndex").map_or(u64::MAX, |v| v as u64));
        for s in slots {
            let c = child(s, "component").unwrap();
            if chain.len() >= CHAIN_SLOTS {
                report.drop("effect(s) past the 16th slot");
                continue;
            }
            if let Some(mut e) = convert(c, report) {
                if s.attribute("powered") == Some("false") {
                    e.bypass = true;
                }
                e.position = num(s, "slotIndex")
                    .filter(|&i| i < CHAIN_SLOTS as f64)
                    .map(|i| i as u8);
                chain.push(e);
            }
        }
    }
    // Slot numbers must not repeat (two racks merged).
    let mut used = [false; CHAIN_SLOTS];
    for e in &mut chain {
        if let Some(p) = e.position {
            if used[p as usize] {
                e.position = None;
            } else {
                used[p as usize] = true;
            }
        }
    }
    (chain, levels.unwrap_or_default())
}

// ------------------------------------------------------------- 5.1

/// Bends a panner direction (speakers at 30 and 110 degrees) into the
/// square room, whose front speakers sit in its front corners.
fn room_angle(deg: f64, surrounds: Surrounds) -> f64 {
    let a = deg.abs().min(180.0);
    let side = match surrounds {
        Surrounds::Behind => 135.0,
        Surrounds::Sides => 90.0,
    };
    let lerp = |x: f64, x0: f64, x1: f64, y0: f64, y1: f64| y0 + (x - x0) / (x1 - x0) * (y1 - y0);
    let b = if a <= 30.0 {
        lerp(a, 0.0, 30.0, 0.0, 45.0)
    } else if a <= 110.0 {
        lerp(a, 30.0, 110.0, 45.0, side)
    } else {
        lerp(a, 110.0, 180.0, side, 180.0)
    };
    b * deg.signum()
}

/// A direction and distance as a spot on the square room.
fn room_point(deg: f64, r: f64) -> (f64, f64) {
    let a = deg.to_radians();
    let (s, c) = a.sin_cos();
    let k = s.abs().max(c.abs()).max(1e-9);
    (r * s / k, r * c / k)
}

/// A 5.1 panner's fixed settings as a spot. None without a panner.
fn spot_51(
    nodes: &[Node],
    surrounds: Surrounds,
    stereo: bool,
    report: &mut Report,
) -> Option<SpacePos> {
    let (c, mono) = match comp(nodes, "Audition.StereoTo51Panner", "") {
        Some(c) => (c, false),
        None => (comp(nodes, "Audition.MonoTo51Panner", "")?, true),
    };
    let get = |n: &str| param(c, n);
    let fixed = |n: &str, d: f64| get(n).and_then(value).unwrap_or(d);
    for n in [
        "Pan: Angle",
        "Pan: Radius",
        "Pan: Stereo Spread",
        "Pan: LFE",
    ] {
        if get(n).is_some_and(|p| !keyframes(p).is_empty()) {
            report.drop("5.1 panner line(s)");
        }
    }
    let angle = fixed("Pan: Angle", std::f64::consts::FRAC_PI_2).to_degrees() - 90.0;
    let radius = fixed("Pan: Radius", 1.0).clamp(0.0, 1.0);
    let spread = fixed("Pan: Stereo Spread", 0.0).to_degrees();
    let lfe = fixed("Pan: LFE", 0.0).clamp(0.0, 1.0);
    let mut pos = SpacePos {
        lfe,
        ..Default::default()
    };
    if mono || !stereo {
        let (x, y) = room_point(room_angle(angle, surrounds), radius);
        pos.x = x;
        pos.y = y;
        return Some(pos);
    }
    let (lx, ly) = room_point(room_angle(angle - spread / 2.0, surrounds), radius);
    let (rx, ry) = room_point(room_angle(angle + spread / 2.0, surrounds), radius);
    pos.x = (lx + rx) / 2.0;
    pos.y = (ly + ry) / 2.0;
    let (dx, dy) = (rx - lx, ry - ly);
    pos.width = (dx.hypot(dy) / 2.0).clamp(0.0, 1.0);
    pos.rotation = if pos.width > 1e-6 {
        round_to((-dy).atan2(dx).to_degrees(), 0.1)
    } else {
        0.0
    };
    Some(pos)
}

/// Whether a `.sesx` is a 5.1 session (looked for in its first 8 KB).
pub fn is_surround(text: &str) -> bool {
    let head: String = text.chars().take(8192).collect();
    head.find("<session")
        .map(|i| &head[i..])
        .and_then(|s| s.find('>').map(|e| &s[..e]))
        .is_some_and(|tag| tag.contains("audioChannelType=\"fivePointOne\""))
}

// ----------------------------------------------------------- media

struct FileEntry {
    abs: Option<String>,
    rel: Option<String>,
    channels: Option<u16>,
    rate: Option<u32>,
}

fn private_setting(s: &str, key: &str) -> Option<u32> {
    s.split(';')
        .filter_map(|kv| kv.strip_prefix(key))
        .filter_map(|v| v.rsplit(':').next()?.parse().ok())
        .next_back()
}

/// Finds a media file: relative to the session, then absolute, then a
/// file of that name beside the session.
fn find_file(f: &FileEntry, base: &Path) -> Option<PathBuf> {
    if let Some(r) = f.rel.as_deref().filter(|r| !r.is_empty()) {
        let p = crate::paths::normalize(&base.join(r.replace('\\', "/")));
        if p.is_file() {
            return Some(p);
        }
    }
    let abs = f.abs.as_deref().filter(|a| !a.is_empty())?;
    let p = PathBuf::from(abs);
    if p.is_file() {
        return Some(p);
    }
    let name = abs.replace('\\', "/");
    let near = base.join(name.rsplit('/').next()?);
    near.is_file().then_some(near)
}

// ------------------------------------------------------------ markers

/// Markers kept in the session's XMP block: each has a start time in
/// frames of its track's frame rate (`f44100` is 44100 per second), a
/// name and maybe a duration.
fn xmp_markers(root: Node) -> Vec<Marker> {
    let Some(text) = root
        .descendants()
        .find(|n| n.has_tag_name("xmpMetadata"))
        .and_then(|n| n.text())
    else {
        return Vec::new();
    };
    let Ok(doc) = roxmltree::Document::parse_with_options(
        text.trim_start_matches('\u{feff}'),
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    ) else {
        return Vec::new();
    };
    let local = |n: Node, name: &str| n.is_element() && n.tag_name().name() == name;
    let text_of = |n: Node, name: &str| {
        n.children()
            .find(|c| local(*c, name))
            .and_then(|c| c.text())
            .map(str::trim)
            .map(str::to_string)
    };
    let mut out = Vec::new();
    for list in doc.descendants().filter(|n| local(*n, "markers")) {
        // The track (rdf:li) holding this list gives the frame rate.
        let fps = list
            .parent()
            .and_then(|t| text_of(t, "frameRate"))
            .and_then(|r| {
                r.trim_start_matches('f')
                    .split('s')
                    .next()?
                    .parse::<f64>()
                    .ok()
            })
            .filter(|r| *r > 0.0)
            .unwrap_or(1.0);
        for m in list.descendants().filter(|n| local(*n, "li")) {
            let Some(start) = text_of(m, "startTime").and_then(|t| t.parse::<f64>().ok()) else {
                continue;
            };
            out.push(Marker {
                name: text_of(m, "name").unwrap_or_else(|| "Marker".into()),
                time: start / fps,
                length: text_of(m, "duration")
                    .and_then(|t| t.parse::<f64>().ok())
                    .map_or(0.0, |d| d / fps),
                ext_id: None,
            });
        }
    }
    out.sort_by(|a, b| a.time.total_cmp(&b.time));
    out
}

// ------------------------------------------------------------- read

/// Reads a `.sesx` file into a new, unsaved session.
pub fn import_file(path: &Path, surrounds: Surrounds) -> Result<Imported, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    let text = decode(&bytes);
    let base = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Imported Session");
    read(&text, base, name, surrounds)
}

/// The file's text: UTF-8 (with or without a byte order mark), or UTF-16
/// when it starts with a UTF-16 byte order mark.
pub fn decode(bytes: &[u8]) -> String {
    let utf16 = |be: bool| {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| {
                if be {
                    u16::from_be_bytes(*b)
                } else {
                    u16::from_le_bytes([b[0], b[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xff, 0xfe, ..] => utf16(false),
        [0xfe, 0xff, ..] => utf16(true),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Reads `.sesx` text. `base` is the folder the file is in (relative
/// media paths start there); `name` names the session.
pub fn read(text: &str, base: &Path, name: &str, surrounds: Surrounds) -> Result<Imported, String> {
    let (session, report) = read_report(text, base, name, surrounds)?;
    Ok(Imported {
        session,
        notices: report.notices(),
        base: None,
    })
}

/// [`read`], with the report as a value.
pub fn read_report(
    text: &str,
    base: &Path,
    name: &str,
    surrounds: Surrounds,
) -> Result<(Session, Report), String> {
    let text = text.trim_start_matches('\u{feff}');
    let doc = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| format!("This is not a readable .sesx session: {e}"))?;
    let root = doc.root_element();
    let sess = root
        .descendants()
        .find(|n| n.has_tag_name("session"))
        .ok_or("This file has no multitrack session in it.")?;
    let rate = num(sess, "sampleRate")
        .map_or(48_000.0, |r| r.clamp(8000.0, 384_000.0))
        .round();
    let rate_u = rate as u32;
    let mut out = Session::new(name, rate_u, 0);
    out.bit_depth = num(sess, "bitDepth")
        .map(|b| b as u16)
        .filter(|b| matches!(b, 16 | 24 | 32))
        .unwrap_or(32);
    let surround = sess.attribute("audioChannelType") == Some("fivePointOne");
    if surround {
        out.space = Some(SpaceLayout::Surround51);
    }
    if let Some(st) = child(sess, "sessionState") {
        out.playhead = num(st, "ctiPosition").unwrap_or(0.0).max(0.0) / rate;
    }
    let mut report = Report::default();

    let files: HashMap<String, FileEntry> = root
        .descendants()
        .filter(|n| n.has_tag_name("file"))
        .filter_map(|f| {
            let ips = f.attribute("importerPrivateSettings").unwrap_or_default();
            Some((
                f.attribute("id")?.to_string(),
                FileEntry {
                    abs: f.attribute("absolutePath").map(str::to_string),
                    rel: f.attribute("relativePath").map(str::to_string),
                    channels: private_setting(ips, "Channels:").map(|c| c as u16),
                    rate: private_setting(ips, "SampleRate:"),
                },
            ))
        })
        .collect();
    // Media by file id, made on first use: (media id, found).
    let mut media: HashMap<String, (u64, bool)> = HashMap::new();

    let tracks_node = child(sess, "tracks").unwrap_or(sess);
    // Buses and the master get their ids first, so outputs can name them.
    let mut by_xml: HashMap<String, Option<TrackId>> = HashMap::new();
    let master_xml = tracks_node
        .children()
        .find(|n| n.has_tag_name("masterTrack"))
        .and_then(|m| m.attribute("id"))
        .map(str::to_string);
    if let Some(m) = &master_xml {
        by_xml.insert(m.clone(), None);
    }
    let mut pending_routes: Vec<(TrackId, Node, Node)> = Vec::new();
    // In the order the file numbers them (`index`), else as written.
    let mut track_nodes: Vec<(f64, usize, Node)> = tracks_node
        .children()
        .filter(|n| n.is_element())
        .enumerate()
        .map(|(i, n)| (num(n, "index").unwrap_or(f64::MAX), i, n))
        .collect();
    track_nodes.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    for t in track_nodes.into_iter().map(|(_, _, n)| n) {
        let tag = t.tag_name().name();
        let kind = match tag {
            "audioTrack" => TrackKind::Audio,
            "busTrack" => TrackKind::Bus,
            "masterTrack" => continue,
            "videoTrack" => {
                report.drop("video track(s)");
                continue;
            }
            other => {
                report.drop(&format!("{other} track(s)"));
                continue;
            }
        };
        let tid = out.add_track_of(kind);
        if let Some(x) = t.attribute("id") {
            by_xml.insert(x.to_string(), Some(tid));
        }
        let params = child(t, "trackParameters");
        let audio = child(t, "trackAudioParameters");
        let nodes: Vec<Node> = audio.into_iter().chain(std::iter::once(t)).collect();
        let tname = params
            .and_then(|p| child(p, "name"))
            .and_then(|n| n.text())
            .map(str::to_string);
        let (vol, vol_line) = level(&nodes, "trackFader", rate);
        let (pan_v, pan_line) = pan(&nodes, rate);
        other_lines(&nodes, &mut report);
        let ch_type = audio.and_then(|a| a.attribute("audioChannelType"));
        let (chain, rack) = racks(t, &mut report);
        if t.attribute("preRendered") == Some("true") {
            report.drop("Pre-render switch(es)");
        }
        let spot = if surround {
            spot_51(&nodes, surrounds, ch_type != Some("mono"), &mut report)
        } else {
            None
        };

        let mut clips = Vec::new();
        if kind == TrackKind::Audio {
            let mut nodes_c: Vec<Node> = t
                .children()
                .filter(|n| n.has_tag_name("audioClip"))
                .collect();
            nodes_c.sort_by(|a, b| {
                num(*a, "zOrder")
                    .unwrap_or(0.0)
                    .total_cmp(&num(*b, "zOrder").unwrap_or(0.0))
            });
            for c in nodes_c {
                if let Some(clip) = read_clip(
                    c,
                    rate,
                    &files,
                    &mut media,
                    &mut out,
                    base,
                    ch_type,
                    &mut report,
                ) {
                    clips.push(clip);
                }
            }
        }
        let tr = out.track_mut(tid).unwrap();
        if let Some(n) = tname {
            tr.name = n;
        }
        if let Some(h) = params.and_then(|p| num(p, "trackHeight")) {
            tr.height = h.clamp(40.0, 600.0) as f32;
        }
        if let Some(c) = hue_rgb(params.and_then(|p| num(p, "trackHue"))) {
            tr.color = c;
        }
        tr.volume_db = vol;
        tr.pan = pan_v;
        tr.mute = muted(&nodes, "trackMute");
        tr.solo = audio.and_then(|a| a.attribute("solo")) == Some("true");
        tr.solo_safe = audio.and_then(|a| a.attribute("soloSafe")) == Some("true");
        tr.armed = audio.and_then(|a| a.attribute("recordArmed")) == Some("true");
        if ch_type == Some("mono") {
            tr.mono = true;
            tr.spatial.stereo = Some(false);
        } else if surround {
            tr.spatial.stereo = Some(true);
        }
        if let Some(pos) = spot {
            tr.pos = pos;
            tr.spatial.had_spot = true;
        }
        if kind == TrackKind::Bus && surround && ch_type == Some("fivePointOne") {
            tr.spatial.room = true;
        }
        for (target, line) in [(LaneTarget::Volume, vol_line), (LaneTarget::Pan, pan_line)] {
            if let Some(points) = line {
                let mut l = Lane::new(target);
                l.points = points;
                tr.lanes.push(l);
                report.lines += 1;
            }
        }
        tr.chain = chain;
        tr.rack = rack;
        tr.clips = clips;
        match kind {
            TrackKind::Bus => report.buses += 1,
            _ => report.tracks += 1,
        }
        if let Some(a) = audio {
            pending_routes.push((tid, a, t));
        }
    }

    // Outputs and sends, now every bus has an id.
    for (tid, audio, track) in pending_routes {
        let output = child(audio, "trackOutput")
            .filter(|o| o.attribute("type") == Some("trackID"))
            .and_then(|o| o.attribute("outputID"))
            .and_then(|id| by_xml.get(id).copied().flatten())
            .filter(|&b| b != tid);
        let mut sends = Vec::new();
        for s in track.descendants().filter(|n| n.has_tag_name("sendSlot")) {
            let Some(bus) = s
                .attribute("sendOutputID")
                .and_then(|id| by_xml.get(id).copied().flatten())
                .filter(|&b| b != tid)
            else {
                continue;
            };
            let sc = s
                .children()
                .find(|c| c.attribute("id") == Some("sendComponent"));
            let power = sc
                .and_then(|c| param(c, "Power On/Off"))
                .and_then(value)
                .unwrap_or(1.0);
            let level = sc
                .and_then(|c| param(c, "Level"))
                .and_then(value)
                .unwrap_or(1.0);
            let pan = s
                .children()
                .find(|c| c.attribute("id") == Some("sendPan"))
                .and_then(|c| param(c, "Pan"))
                .and_then(value)
                .unwrap_or(0.0);
            let mut send = Send::new(bus);
            send.level_db = gain_db(level);
            send.pre_fader = s.attribute("preFader") == Some("true");
            send.pan = (pan / 100.0) as f32;
            send.enabled = power >= 0.5 && sc.and_then(|c| c.attribute("powered")) != Some("false");
            send.tidy();
            sends.push(send);
            report.sends += 1;
        }
        let tr = out.track_mut(tid).unwrap();
        tr.output = output;
        tr.sends = sends;
    }

    if let Some(m) = tracks_node
        .children()
        .find(|n| n.has_tag_name("masterTrack"))
    {
        let audio = child(m, "trackAudioParameters");
        let nodes: Vec<Node> = audio.into_iter().chain(std::iter::once(m)).collect();
        let (vol, line) = level(&nodes, "trackFader", rate);
        out.master_db = vol;
        if let Some(points) = line {
            let mut l = Lane::new(LaneTarget::Volume);
            l.points = points;
            out.master_lanes.push(l);
            report.lines += 1;
        }
        if muted(&nodes, "trackMute") {
            report.drop("master mute (the master has none)");
        }
        if pan(&nodes, rate).0 != 0.0 {
            report.drop("master pan (the master has none)");
        }
        other_lines(&nodes, &mut report);
        let (chain, rack) = racks(m, &mut report);
        out.master_chain = chain;
        out.master_rack = rack;
    }

    // Missing files: a placeholder as long as the clips that use it.
    for (fid, &(mid, found)) in &media {
        if found {
            continue;
        }
        report.missing_files += 1;
        let end = out
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| c.media == mid)
            .map(|c| c.offset + c.source_len())
            .fold(0.0, f64::max);
        let _ = fid;
        if let Some(m) = out.media.iter_mut().find(|m| m.id == mid) {
            m.duration = end;
        }
    }
    out.media.sort_by_key(|m| m.id);

    out.markers = xmp_markers(root);
    report.markers = out.markers.len();
    report.clips = out.tracks.iter().map(|t| t.clips.len()).sum();
    if root.descendants().any(|n| n.has_tag_name("clipGroup")) {
        report.drop("clip group(s)");
    }
    Ok((out, report))
}

#[allow(clippy::too_many_arguments)]
fn read_clip(
    c: Node,
    rate: f64,
    files: &HashMap<String, FileEntry>,
    media: &mut HashMap<String, (u64, bool)>,
    out: &mut Session,
    base: &Path,
    track_type: Option<&str>,
    report: &mut Report,
) -> Option<Clip> {
    let fid = c.attribute("fileID")?;
    let start = num(c, "startPoint").unwrap_or(0.0).max(0.0);
    let end = num(c, "endPoint").unwrap_or(start);
    if end <= start {
        report.drop("empty clip(s)");
        return None;
    }
    let entry = files.get(fid);
    let (mid, _) = *media.entry(fid.to_string()).or_insert_with(|| {
        let found = entry.and_then(|f| find_file(f, base));
        let id = out.alloc_id();
        let path = found.clone().or_else(|| {
            entry
                .and_then(|f| {
                    f.abs
                        .clone()
                        .filter(|a| !a.is_empty())
                        .or_else(|| f.rel.clone())
                })
                .map(PathBuf::from)
        });
        let channels = entry
            .and_then(|f| f.channels)
            .unwrap_or(if found.is_some() {
                0
            } else if track_type == Some("mono") {
                1
            } else {
                2
            });
        out.media.push(MediaItem {
            id,
            name: path
                .as_deref()
                .map(crate::file_name)
                .unwrap_or_else(|| format!("File {fid}")),
            path,
            duration: 0.0,
            rate: entry.and_then(|f| f.rate).unwrap_or(rate as u32),
            channels,
            ext_id: None,
        });
        (id, found.is_some())
    });

    let mut stretch = ClipStretch::default();
    let mut ratio = 1.0;
    if let Some(st) = child(c, "clipStretch") {
        report.stretched += 1;
        ratio = num(st, "stretchRatio")
            .filter(|r| *r > 0.0)
            .unwrap_or(1.0)
            .clamp(0.25, 4.0);
        let kind = st
            .attribute("stretchType")
            .unwrap_or("")
            .to_ascii_lowercase();
        stretch.kind = match kind.as_str() {
            "varispeed" | "resample" => StretchKind::Varispeed,
            "solo" => StretchKind::Monophonic,
            _ => StretchKind::Polyphonic,
        };
        stretch.ratio = ratio;
        stretch.pitch = if stretch.kind == StretchKind::Varispeed {
            0.0
        } else {
            num(st, "pitchAdjustment").unwrap_or(0.0).clamp(-24.0, 24.0) as f32
        };
        stretch.mode = if st.attribute("stretchMode") == Some("rendered") {
            StretchMode::Rendered
        } else {
            StretchMode::Live
        };
        if let Some(w) = num(st, "adaptiveWindowSize").filter(|w| *w > 0.0) {
            stretch.window_ms = w.clamp(5.0, 200.0) as f32;
        }
        if let Some(t) = num(st, "transientSensitivity") {
            stretch.transients = t.clamp(0.0, 100.0) as f32;
        }
        stretch.preserve_formants = st.attribute("preserveFormants") == Some("true");
        stretch.high_precision = st.attribute("stretchQuality") == Some("high");
    }

    let nodes = [c];
    let (gain, line) = level(&nodes, "clipGain", rate);
    let mut gain_db = gain;
    if let Some(points) = line {
        // A line that never moves is a level; Audiform27 clips have no
        // automation of their own yet.
        let first = points[0].value;
        if points.iter().all(|p| (p.value - first).abs() < 0.01) {
            gain_db = (gain as f64 + first) as f32;
        } else {
            report.drop("clip volume line(s)");
        }
    }
    other_lines(&nodes, report);
    if child(c, "effectsRack").is_some_and(|r| {
        r.children()
            .any(|s| s.has_tag_name("slot") && child(s, "component").is_some())
    }) {
        report.drop("clip effect rack(s)");
    }
    if c.attribute("looped") == Some("true") {
        report.drop("looped clip(s) (played once)");
    }
    if channel_map_unusual(c, files.get(fid).and_then(|f| f.channels)) {
        report.drop("Source Channels map(s) (a clip playing one side of its file)");
    }

    let mut fade_skew = |tag: &str| -> (f64, f32) {
        let Some(f) = child(c, tag) else {
            return (0.0, 0.0);
        };
        let len =
            (num(f, "endPoint").unwrap_or(0.0) - num(f, "startPoint").unwrap_or(0.0)).abs() / rate;
        if len <= 0.0 {
            return (0.0, 0.0);
        }
        let shape = num(f, "shape").unwrap_or(0.0).clamp(-100.0, 100.0) as f32;
        match f.attribute("type") {
            Some("cosine") => {
                report.approx("S-curve fade(s) (as straight curves)");
                (len, 0.0)
            }
            _ => {
                if shape != 0.0 {
                    report.approx("fade curve(s) (from the file's curve amount)");
                }
                (len, shape)
            }
        }
    };
    let (fade_in, fade_in_skew) = fade_skew("fadeIn");
    let (fade_out, fade_out_skew) = fade_skew("fadeOut");

    let id = out.alloc_id();
    let mut clip = Clip {
        id,
        media: mid,
        name: c.attribute("name").unwrap_or("Clip").to_string(),
        start: start / rate,
        offset: num(c, "sourceInPoint").unwrap_or(0.0).max(0.0) / ratio / rate,
        length: (end - start) / rate,
        gain_db,
        fade_in,
        fade_out,
        muted: muted(&nodes, "clipMute"),
        props: Default::default(),
        ext_id: None,
    };
    clip.props.stretch = stretch;
    clip.props.locked = c.attribute("lockedInTime") == Some("true");
    clip.props.color = hue_rgb(num(c, "hue"));
    clip.props.fade_in_skew = fade_in_skew;
    clip.props.fade_out_skew = fade_out_skew;
    Some(clip)
}

/// A channel map other than the file's usual one (every channel in
/// place, or a mono file from channel 0).
fn channel_map_unusual(c: Node, file_channels: Option<u16>) -> bool {
    let Some(map) = child(c, "channelMap") else {
        return false;
    };
    let pairs: Vec<(i64, i64)> = map
        .children()
        .filter(|n| n.has_tag_name("channel"))
        .filter_map(|n| Some((num(n, "index")? as i64, num(n, "sourceIndex")? as i64)))
        .collect();
    if pairs.is_empty() {
        return false;
    }
    let in_place = pairs.iter().all(|&(i, s)| i == s);
    // The channel count is not known for files found on disk (only
    // missing ones note it): channel 0 alone is then most likely a mono
    // file, so only a channel out of place counts.
    let Some(n) = file_channels.map(|n| n as i64) else {
        return !in_place;
    };
    if n == 1 {
        return !pairs.iter().all(|&(_, s)| s == 0);
    }
    !(pairs.len() as i64 >= n.min(2) && in_place)
}

#[cfg(test)]
mod tests;
