// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The other editor's built-in effects as Audiform2's own, and back.
//!
//! Written for Audiform2 from the effects' behaviour and from what the
//! project lead's own `.sesx` sessions hold (no other program's code or
//! tables). Two kinds of storage turn up in those sessions:
//!
//! * named `<parameter>`s, each a knob position 0..1 (Parametric
//!   Equalizer, Stereo Expander, Studio Reverb, Chorus, Center Channel
//!   Extractor, Channel Mixer, Amplify, Hard Limiter, Delay, Echo,
//!   Adaptive Noise Reduction, Notch Filter). These are converted by a
//!   table per effect: each Audiform2 value comes from one knob through
//!   a function, and goes back through its inverse, so drawn automation
//!   on a knob becomes automation of the new effect's value, and back;
//! * a settings chunk: a byte (the effect's kind), the text `Stefan`
//!   written backwards, a count, then the settings as 32-bit floats in
//!   the order the effect's window shows them (Single-band and
//!   Tube-modeled Compressor, DeEsser, Analog Delay, Convolution Reverb;
//!   the Graphic Phase Shifter keeps its own float list). These have no
//!   automation in the file.
//!
//! Where the knob's scale could only be inferred from the values found
//! (no readout of the other editor was available), the conversion is an
//! estimate, and the import notice says so.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use af2_core::session::EffectSlot;
use roxmltree::Node;

use super::{chunk, keyframes, named_params, on, round_to};

/// An effect's values by key: `af2_fx` keys, or the parameter names of
/// Audiform2's own processors (`af2_dsp`).
pub type Vals = BTreeMap<String, f64>;

/// The knob scale context: half of the "Original Sample Rate" the
/// frequency knobs span.
#[derive(Debug, Clone, Copy)]
pub struct Ctx {
    pub nyq: f64,
}

impl Ctx {
    fn from(p: &HashMap<String, f64>) -> Ctx {
        Ctx {
            nyq: p
                .get("Original Sample Rate")
                .map_or(22_050.0, |r| r * 96_000.0)
                .max(1000.0),
        }
    }
}

/// The rate written for exported effects (48 kHz).
const EXPORT_RATE_SHARE: f64 = 0.25;

type F = fn(f64, Ctx) -> f64;
/// Work on the values after the table (from more than one knob).
type Post = fn(&mut Vals, &HashMap<String, f64>, Ctx);
/// Work on the knobs written back.
type PostInv = fn(&Vals, &mut BTreeMap<&'static str, f64>, Ctx);
/// Drawn points: (file sample, value, holds).
pub type Points = Vec<(f64, f64, bool)>;

/// One Audiform2 value: from the knob `src` by `fwd`, back by `inv`.
struct Entry {
    key: &'static str,
    src: &'static str,
    fwd: F,
    inv: F,
}

fn e(key: &'static str, src: &'static str, fwd: F, inv: F) -> Entry {
    Entry { key, src, fwd, inv }
}

/// A knob-stored effect and how it maps.
struct Spec {
    guid: &'static str,
    name: &'static str,
    effect: &'static str,
    /// Notice text for each one converted.
    label: &'static str,
    estimate: bool,
    /// Every knob, in the file's order, with the value written for one
    /// that has no Audiform2 counterpart.
    knobs: &'static [(&'static str, f64)],
    entries: Vec<Entry>,
    /// Values set whatever the file says (a mode).
    fixed: &'static [(&'static str, f64)],
    /// Further work after the table (values from more than one knob).
    post: Option<Post>,
    /// Further work on the knobs written back.
    post_inv: Option<PostInv>,
    /// Whether a slot of `effect` may be written back as this one.
    exports: bool,
}

pub const PARAMETRIC_EQ: &str = "8edcd326-33de-4a4d-985b-f1bf48ed1427";
pub const STEREO_EXPANDER: &str = "fe767733-0969-46cf-bf45-fbf3d58ec46a";
pub const STUDIO_REVERB: &str = "7ea99d31-ca8f-454f-acd7-81a258efb336";
pub const CHORUS: &str = "31d33f85-3288-489e-8390-d03c60d9ce6c";
pub const CENTER_EXTRACTOR: &str = "0a1016a3-74e6-4c93-8b6c-e058a1419bf1";
pub const CHANNEL_MIXER: &str = "c5ca9e29-df07-42a6-8800-ba80324e5cbc";
pub const AMPLIFY: &str = "f14fee7d-fe5f-4202-b790-f53209e56411";
pub const HARD_LIMITER: &str = "e0b23f05-f1a7-4ef7-9b50-7ec3e3002058";
pub const DELAY: &str = "5e85f068-b39d-4c63-9912-2030f6f0041e";
pub const ECHO: &str = "f205d7b7-5e0b-421b-bc6d-5f27096ba192";
pub const ADAPTIVE_NR: &str = "f83ad984-04c6-4474-b8c0-e31d98875cfe";
pub const NOTCH: &str = "ee38cdc5-5914-4a75-b7dd-73a265e55c31";
pub const SINGLE_BAND_COMP: &str = "2da3e2bc-5005-481c-bbeb-7689d3169edf";
pub const TUBE_COMP: &str = "f527fd12-24bf-411f-a46e-5b997cec32d4";
pub const DEESSER: &str = "ffbe710f-cd69-4139-ad26-65603616d9d4";
pub const ANALOG_DELAY: &str = "c4ad21d8-9748-4d79-b4a3-d88ab7e4be34";
pub const CONVOLUTION: &str = "99c2e75f-0bdb-48d8-8b51-2bac2e26b2cf";
pub const PHASE_SHIFTER: &str = "7354704a-d87c-11e0-b05b-763c4824019b";

// ------------------------------------------------------------ scales

fn hz(v: f64, c: Ctx) -> f64 {
    (20.0 + v * (c.nyq - 20.0)).round()
}
fn hz_inv(f: f64, c: Ctx) -> f64 {
    ((f - 20.0) / (c.nyq - 20.0)).clamp(0.0, 1.0)
}
/// ±48 dB with 0.5 at 0, kept within ±24.
fn db48(v: f64, _: Ctx) -> f64 {
    round_to((v - 0.5) * 96.0, 0.01).clamp(-24.0, 24.0)
}
fn db48_inv(d: f64, _: Ctx) -> f64 {
    (d / 96.0 + 0.5).clamp(0.0, 1.0)
}
fn flag(v: f64, _: Ctx) -> f64 {
    on(Some(v)) as u8 as f64
}
fn pct(v: f64, _: Ctx) -> f64 {
    round_to(v * 100.0, 0.01)
}
fn pct_inv(p: f64, _: Ctx) -> f64 {
    (p / 100.0).clamp(0.0, 1.0)
}
/// -100..100 with 0.5 in the middle.
fn centred(v: f64, _: Ctx) -> f64 {
    round_to((v - 0.5) * 200.0, 0.01)
}
fn centred_inv(p: f64, _: Ctx) -> f64 {
    (p / 200.0 + 0.5).clamp(0.0, 1.0)
}
fn same(v: f64, _: Ctx) -> f64 {
    v
}

/// Pass filter slopes: the knob gives 12 to 42 dB/oct in steps of 6;
/// Parametric EQ's choices are 12, 24, 48, 6, 18 and 36 dB/oct.
const SLOPES: [f64; 6] = [12.0, 24.0, 48.0, 6.0, 18.0, 36.0];
fn slope(v: f64, _: Ctx) -> f64 {
    let d = 12.0 + (v * 5.0).round() * 6.0;
    if d < 18.0 {
        0.0
    } else if d < 36.0 {
        1.0
    } else {
        2.0
    }
}
fn slope_inv(choice: f64, _: Ctx) -> f64 {
    let d = SLOPES[(choice.round().max(0.0) as usize).min(5)].clamp(12.0, 42.0);
    ((d - 12.0) / 30.0).clamp(0.0, 1.0)
}

// ------------------------------------------------------------ tables

const PEQ_KNOBS: &[(&str, f64)] = &[
    ("Low Shelf Frequency", 0.0009),
    ("Low Shelf Gain", 0.5),
    ("High Shelf Frequency", 0.8162),
    ("High Shelf Gain", 0.5),
    ("EQ Band 1 Center Frequency", 0.0014),
    ("EQ Band 1 Q", 0.0002),
    ("EQ Band 1 Gain", 0.5),
    ("EQ Band 1 Enable", 1.0),
    ("EQ Band 2 Center Frequency", 0.0082),
    ("EQ Band 2 Q", 0.0002),
    ("EQ Band 2 Gain", 0.5),
    ("EQ Band 2 Enable", 1.0),
    ("EQ Band 3 Center Frequency", 0.0354),
    ("EQ Band 3 Q", 0.0002),
    ("EQ Band 3 Gain", 0.5),
    ("EQ Band 3 Enable", 1.0),
    ("EQ Band 4 Center Frequency", 0.1443),
    ("EQ Band 4 Q", 0.0002),
    ("EQ Band 4 Gain", 0.5),
    ("EQ Band 4 Enable", 1.0),
    ("EQ Band 5 Center Frequency", 0.5801),
    ("EQ Band 5 Q", 0.0002),
    ("EQ Band 5 Gain", 0.5),
    ("EQ Band 5 Enable", 1.0),
    ("Gain", 2.0 / 3.0),
    ("ConstantQ", 1.0),
    ("Low Shelf Filter Order", 0.0),
    ("High Shelf Filter Order", 0.0),
    ("Ultra-Quiet Mode", 0.0),
    ("Low Shelf Enable", 1.0),
    ("High Shelf Enable", 1.0),
    ("Original Sample Rate", EXPORT_RATE_SHARE),
    ("EQ Band 1 Bandwidth", 0.0025),
    ("EQ Band 2 Bandwidth", 0.01),
    ("EQ Band 3 Bandwidth", 0.04),
    ("EQ Band 4 Bandwidth", 0.16),
    ("EQ Band 5 Bandwidth", 0.64),
    ("High-Pass Filter Enable", 0.0),
    ("High-Pass Cutoff Frequency", 0.0009),
    ("High-Pass Slope", 0.4),
    ("Low-Pass Filter Enable", 0.0),
    ("Low-Pass Cutoff Frequency", 0.8162),
    ("Low-Pass Slope", 0.4),
    ("Bypass", 0.0),
    ("LowGraphRange", 1.0),
];

fn peq_entries() -> Vec<Entry> {
    let mut v = vec![
        e(
            "gain",
            "Gain",
            |v, _| round_to(v * 60.0 - 40.0, 0.1),
            |g, _| ((g + 40.0) / 60.0).clamp(0.0, 1.0),
        ),
        e("hp_on", "High-Pass Filter Enable", flag, same),
        e(
            "hp_freq",
            "High-Pass Cutoff Frequency",
            |v, c| hz(v, c).clamp(10.0, 2000.0),
            hz_inv,
        ),
        e("hp_slope", "High-Pass Slope", slope, slope_inv),
        e("ls_on", "Low Shelf Enable", flag, same),
        e("ls_freq", "Low Shelf Frequency", hz, hz_inv),
        e("ls_gain", "Low Shelf Gain", db48, db48_inv),
        e("hs_on", "High Shelf Enable", flag, same),
        e(
            "hs_freq",
            "High Shelf Frequency",
            |v, c| hz(v, c).clamp(1000.0, 20_000.0),
            hz_inv,
        ),
        e("hs_gain", "High Shelf Gain", db48, db48_inv),
        e("lp_on", "Low-Pass Filter Enable", flag, same),
        e(
            "lp_freq",
            "Low-Pass Cutoff Frequency",
            |v, c| hz(v, c).clamp(1000.0, 22_000.0),
            hz_inv,
        ),
        e("lp_slope", "Low-Pass Slope", slope, slope_inv),
    ];
    const BANDS: [[&str; 5]; 5] = [
        [
            "b1_on",
            "b1_freq",
            "b1_gain",
            "b1_q",
            "EQ Band 1 Enable|EQ Band 1 Center Frequency|EQ Band 1 Gain|EQ Band 1 Q",
        ],
        [
            "b2_on",
            "b2_freq",
            "b2_gain",
            "b2_q",
            "EQ Band 2 Enable|EQ Band 2 Center Frequency|EQ Band 2 Gain|EQ Band 2 Q",
        ],
        [
            "b3_on",
            "b3_freq",
            "b3_gain",
            "b3_q",
            "EQ Band 3 Enable|EQ Band 3 Center Frequency|EQ Band 3 Gain|EQ Band 3 Q",
        ],
        [
            "b4_on",
            "b4_freq",
            "b4_gain",
            "b4_q",
            "EQ Band 4 Enable|EQ Band 4 Center Frequency|EQ Band 4 Gain|EQ Band 4 Q",
        ],
        [
            "b5_on",
            "b5_freq",
            "b5_gain",
            "b5_q",
            "EQ Band 5 Enable|EQ Band 5 Center Frequency|EQ Band 5 Gain|EQ Band 5 Q",
        ],
    ];
    for b in BANDS {
        let src: Vec<&'static str> = b[4].split('|').collect();
        v.push(e(b[0], src[0], flag, same));
        v.push(e(b[1], src[1], hz, hz_inv));
        v.push(e(b[2], src[2], db48, db48_inv));
        v.push(e(
            b[3],
            src[3],
            |v, _| round_to(v * 10_000.0, 0.01).clamp(0.1, 100.0),
            |q, _| (q / 10_000.0).clamp(0.0, 1.0),
        ));
    }
    v
}

const EXPANDER_KNOBS: &[(&str, f64)] =
    &[("Center Channel Pan", 0.5), ("Stereo Expander", 1.0 / 3.0)];

const STUDIO_REVERB_KNOBS: &[(&str, f64)] = &[
    ("Low Frequency Cut", 0.0413),
    ("High Frequency Cut", 0.2718),
    ("Room Size", 0.697),
    ("Width", 1.0),
    ("Diffusion", 0.5),
    ("Damping", 0.5),
    ("Decay", 0.2347),
    ("Early Reflections", 0.52),
    ("Early Reflections Delay", 0.5),
    ("Dry Output Level", 1.0),
    ("Wet Output Level", 0.25),
    ("Is Processing", 0.25126),
    ("Number of Channels", 0.0625),
];

const CHORUS_KNOBS: &[(&str, f64)] = &[
    ("Delay Time", 0.6273),
    ("Delay Rate", 0.0),
    ("Feedback", 0.0),
    ("Dry Output Level", 0.35),
    ("Wet Output Level", 0.15),
    ("Spread", 0.15),
    ("Stereo Field", 1.0),
    ("Modulation Rate", 0.0),
    ("Modulation Depth", 0.0),
    ("Voices", 2.0),
    ("Average Left & Right channel input", 0.0),
    ("Add Binaural Cues", 0.0),
    ("Highest Quality", 0.0),
];

const CCE_KNOBS: &[(&str, f64)] = &[
    ("Phase Degree", 0.5),
    ("Pan", 0.5),
    ("Delay", 0.5),
    ("Extract", 4.0),
    ("Center Frequency", 0.5),
    ("Frequency Width", 0.9991),
    ("Start Frequency", 0.0),
    ("End Frequency", 1.0),
    ("Crossover Bleed", 1.0),
    ("Phase Discrimination", 0.5057),
    ("Amplitude Discrimination", 0.125),
    ("Amplitude Bandwidth", 0.2386),
    ("Spectral Decay Rate", 0.0),
    ("Center Channel Level", 0.5),
    ("Side Channel Levels", 0.5),
    ("FFT Size", 5.0),
    ("Interval", 0.051),
    ("Overlays", 2.0),
    ("Window Width", 0.27),
    ("Spectrum", 4.0),
    ("Is Processing", 0.25126),
    ("Original Sample Rate", EXPORT_RATE_SHARE),
];

/// Channel Mixer knobs: the 8x8 matrix, `X to Y` with 0.5 silent, 1 all
/// of it and 0 all of it with the polarity flipped.
const CHANNEL_NAMES: [&str; 8] = ["L", "R", "3", "4", "5", "6", "7", "8"];

fn mixer_knobs() -> &'static [(&'static str, f64)] {
    use std::sync::OnceLock;
    static K: OnceLock<Vec<(&'static str, f64)>> = OnceLock::new();
    K.get_or_init(|| {
        let mut k = vec![("Channel Count", 0.25)];
        for (o, out) in CHANNEL_NAMES.iter().enumerate() {
            for (i, inp) in CHANNEL_NAMES.iter().enumerate() {
                let name: &'static str = Box::leak(format!("{inp} to {out}").into_boxed_str());
                k.push((name, if i == o { 1.0 } else { 0.5 }));
            }
        }
        k
    })
}

const AMPLIFY_KNOBS: &[(&str, f64)] = &[
    ("Channel Count", 0.0625),
    ("Link Sliders", 1.0),
    ("Left", 2.0 / 3.0),
    ("Right", 2.0 / 3.0),
    ("Link Keyframes", 1.0),
];

/// Amplify's sliders: 2/3 is 0 dB; an estimate of 90 dB across the
/// slider (-60..+30 dB).
fn amp_db(v: f64, _: Ctx) -> f64 {
    round_to((v - 2.0 / 3.0) * 90.0, 0.01)
}
fn amp_db_inv(d: f64, _: Ctx) -> f64 {
    (d / 90.0 + 2.0 / 3.0).clamp(0.0, 1.0)
}

const LIMITER_KNOBS: &[(&str, f64)] = &[
    ("Maximum Amplitude", 0.997),
    ("Input Boost", 2.0 / 3.0),
    ("Look-Ahead Time", 0.8636),
    ("Release Time", 0.375),
    ("Link Channels", 1.0),
    ("Decay to Ceiling", 1.0),
    ("Limit True Peak", 1.0),
];

const ADAPTIVE_NR_KNOBS: &[(&str, f64)] = &[
    ("Reduce Noise By", 0.3148),
    ("Noisiness", 0.2444),
    ("Fine Tune Noise Floor", 0.375),
    ("Signal Threshold", 0.7106),
    ("Spectral Decay Rate", 0.1644),
    ("Broadband Preservation", 0.2),
    ("FFT Size", 0.0625),
    ("High Quality Mode (slower)", 1.0),
    ("Is Processing", 0.0),
];

fn specs() -> Vec<Spec> {
    vec![
        Spec {
            guid: PARAMETRIC_EQ,
            name: "Parametric Equalizer",
            effect: af2_fx::effects::parametric_eq::ID,
            label: "Parametric Equalizer(s) came across as Parametric EQ",
            estimate: false,
            knobs: PEQ_KNOBS,
            entries: peq_entries(),
            fixed: &[],
            post: None,
            post_inv: None,
            exports: true,
        },
        Spec {
            guid: STEREO_EXPANDER,
            name: "Stereo Expander",
            effect: "af.stereo_width",
            label: "Stereo Expander(s) came across as Stereo Width",
            estimate: false,
            knobs: EXPANDER_KNOBS,
            entries: vec![
                e("pan", "Center Channel Pan", centred, centred_inv),
                // A third of the knob is the usual width (100 %).
                e(
                    "width",
                    "Stereo Expander",
                    |v, _| round_to(v * 300.0, 0.01),
                    |w, _| (w / 300.0).clamp(0.0, 1.0),
                ),
            ],
            fixed: &[],
            post: None,
            post_inv: None,
            exports: true,
        },
        Spec {
            guid: STUDIO_REVERB,
            name: "Studio Reverb",
            effect: "af.reverb",
            label: "Studio Reverb(s) came across as Reverb (an estimate)",
            estimate: true,
            knobs: STUDIO_REVERB_KNOBS,
            entries: vec![
                e("size", "Room Size", pct, pct_inv),
                e(
                    "decay",
                    "Decay",
                    |v, _| (100.0 + v * 9900.0).round(),
                    |d, _| ((d - 100.0) / 9900.0).clamp(0.0, 1.0),
                ),
                e("early", "Early Reflections", pct, pct_inv),
                e("width", "Width", pct, pct_inv),
                e("diffusion", "Diffusion", pct, pct_inv),
                e("damping", "Damping", pct, pct_inv),
                e(
                    "low_cut",
                    "Low Frequency Cut",
                    |v, _| (20.0 + v * 980.0).round(),
                    |f, _| ((f - 20.0) / 980.0).clamp(0.0, 1.0),
                ),
                e(
                    "high_cut",
                    "High Frequency Cut",
                    |v, _| (20.0 + v * 19_980.0).round().clamp(1000.0, 20_000.0),
                    |f, _| ((f - 20.0) / 19_980.0).clamp(0.0, 1.0),
                ),
                e("dry", "Dry Output Level", pct, pct_inv),
                e("wet", "Wet Output Level", pct, pct_inv),
            ],
            fixed: &[],
            post: None,
            post_inv: None,
            exports: true,
        },
        Spec {
            guid: CHORUS,
            name: "Chorus",
            effect: "af.modulation",
            label: "Chorus(es) came across as Modulation (an estimate)",
            estimate: true,
            knobs: CHORUS_KNOBS,
            entries: vec![
                e(
                    "chorus_delay",
                    "Delay Time",
                    |v, _| round_to(5.0 + v * 35.0, 0.01),
                    |d, _| ((d - 5.0) / 35.0).clamp(0.0, 1.0),
                ),
                e(
                    "feedback",
                    "Feedback",
                    |v, _| round_to(v * 100.0, 0.01).min(95.0),
                    pct_inv,
                ),
                e("dry", "Dry Output Level", pct, pct_inv),
                e("wet", "Wet Output Level", pct, pct_inv),
                e("spread", "Spread", pct, pct_inv),
                e("width", "Stereo Field", pct, pct_inv),
                e(
                    "rate",
                    "Modulation Rate",
                    |v, _| round_to(v * 10.0, 0.01).max(0.02),
                    |r, _| (r / 10.0).clamp(0.0, 1.0),
                ),
                e("depth", "Modulation Depth", pct, pct_inv),
                e(
                    "voices",
                    "Voices",
                    |v, _| v.round().clamp(2.0, 6.0),
                    |v, _| v.round().clamp(2.0, 5.0),
                ),
            ],
            fixed: &[("kind", 0.0)],
            post: None,
            post_inv: None,
            exports: true,
        },
        Spec {
            guid: CENTER_EXTRACTOR,
            name: "Center Channel Extractor",
            effect: "af.center_extractor",
            label:
                "Center Channel Extractor(s) came across as Center Channel Extractor (an estimate)",
            estimate: true,
            knobs: CCE_KNOBS,
            entries: vec![
                e(
                    "extract",
                    "Extract",
                    |v, _| v.round().clamp(0.0, 4.0),
                    |v, _| v.round().clamp(0.0, 4.0),
                ),
                e(
                    "phase",
                    "Phase Degree",
                    |v, _| round_to((v - 0.5) * 360.0, 0.1),
                    |d, _| (d / 360.0 + 0.5).clamp(0.0, 1.0),
                ),
                e("pan", "Pan", centred, centred_inv),
                e(
                    "delay",
                    "Delay",
                    |v, _| round_to((v - 0.5) * 20.0, 0.01),
                    |d, _| (d / 20.0 + 0.5).clamp(0.0, 1.0),
                ),
                e("bleed", "Crossover Bleed", pct, pct_inv),
                e(
                    "phase_disc",
                    "Phase Discrimination",
                    |v, _| round_to(1.0 + v * 89.0, 0.1),
                    |d, _| ((d - 1.0) / 89.0).clamp(0.0, 1.0),
                ),
                e(
                    "amp_disc",
                    "Amplitude Discrimination",
                    |v, _| round_to(0.5 + v * 19.5, 0.01),
                    |d, _| ((d - 0.5) / 19.5).clamp(0.0, 1.0),
                ),
                e(
                    "amp_band",
                    "Amplitude Bandwidth",
                    |v, _| round_to(v * 20.0, 0.1),
                    |b, _| (b / 20.0).clamp(0.0, 1.0),
                ),
                e("decay", "Spectral Decay Rate", pct, pct_inv),
                e(
                    "center",
                    "Center Channel Level",
                    |v, _| round_to((v - 0.5) * 96.0, 0.01).clamp(-48.0, 12.0),
                    |d, _| (d / 96.0 + 0.5).clamp(0.0, 1.0),
                ),
                e(
                    "side",
                    "Side Channel Levels",
                    |v, _| round_to((v - 0.5) * 96.0, 0.01).clamp(-48.0, 12.0),
                    |d, _| (d / 96.0 + 0.5).clamp(0.0, 1.0),
                ),
                e(
                    "fft",
                    "FFT Size",
                    |v, _| (v.round() - 2.0).clamp(0.0, 4.0),
                    |k, _| (k.round() + 2.0).clamp(2.0, 6.0),
                ),
            ],
            fixed: &[],
            post: Some(|vals, p, c| {
                // The band between the start and end frequencies.
                let (Some(s), Some(e)) = (p.get("Start Frequency"), p.get("End Frequency")) else {
                    return;
                };
                let (s, e) = (hz(*s, c).max(20.0), hz(*e, c).max(21.0));
                let (lo, hi) = (s.min(e), s.max(e).max(s.min(e) + 1.0));
                vals.insert("range".into(), 4.0);
                vals.insert(
                    "center_freq".into(),
                    (lo * hi).sqrt().round().clamp(20.0, 20_000.0),
                );
                vals.insert(
                    "width".into(),
                    round_to((hi / lo).log2(), 0.01).clamp(0.5, 10.0),
                );
            }),
            post_inv: Some(|vals, k, c| {
                let (Some(cf), Some(w)) = (vals.get("center_freq"), vals.get("width")) else {
                    return;
                };
                let half = 2f64.powf(w / 2.0);
                k.insert("Start Frequency", hz_inv(cf / half, c));
                k.insert("End Frequency", hz_inv(cf * half, c));
            }),
            exports: true,
        },
        Spec {
            guid: CHANNEL_MIXER,
            name: "Channel Mixer",
            effect: "af.channel_mixer",
            label: "Channel Mixer(s) came across as Channel Mixer",
            estimate: false,
            knobs: mixer_knobs(),
            entries: vec![
                e(
                    "ll",
                    "L to L",
                    |v, _| round_to((v - 0.5).abs() * 200.0, 0.01),
                    same,
                ),
                e(
                    "lr",
                    "R to L",
                    |v, _| round_to((v - 0.5).abs() * 200.0, 0.01),
                    same,
                ),
                e(
                    "rl",
                    "L to R",
                    |v, _| round_to((v - 0.5).abs() * 200.0, 0.01),
                    same,
                ),
                e(
                    "rr",
                    "R to R",
                    |v, _| round_to((v - 0.5).abs() * 200.0, 0.01),
                    same,
                ),
                e(
                    "ll_inv",
                    "L to L",
                    |v, _| (v < 0.5 - 1e-6) as u8 as f64,
                    same,
                ),
                e(
                    "lr_inv",
                    "R to L",
                    |v, _| (v < 0.5 - 1e-6) as u8 as f64,
                    same,
                ),
                e(
                    "rl_inv",
                    "L to R",
                    |v, _| (v < 0.5 - 1e-6) as u8 as f64,
                    same,
                ),
                e(
                    "rr_inv",
                    "R to R",
                    |v, _| (v < 0.5 - 1e-6) as u8 as f64,
                    same,
                ),
            ],
            fixed: &[],
            post: None,
            post_inv: Some(|vals, k, _| {
                for (key, knob) in [
                    ("ll", "L to L"),
                    ("lr", "R to L"),
                    ("rl", "L to R"),
                    ("rr", "R to R"),
                ] {
                    let amount = vals.get(key).copied().unwrap_or(0.0) / 200.0;
                    let flip = vals.get(&format!("{key}_inv")).copied().unwrap_or(0.0) >= 0.5;
                    k.insert(knob, if flip { 0.5 - amount } else { 0.5 + amount });
                }
            }),
            exports: true,
        },
        Spec {
            guid: AMPLIFY,
            name: "Amplify",
            effect: "amplify",
            label: "Amplify(s) came across as Amplify (an estimate)",
            estimate: true,
            knobs: AMPLIFY_KNOBS,
            entries: vec![e("Gain", "Left", amp_db, amp_db_inv)],
            fixed: &[],
            post: Some(|vals, p, c| {
                // One gain for both sides: their average.
                if let (Some(l), Some(r)) = (p.get("Left"), p.get("Right")) {
                    vals.insert(
                        "Gain".into(),
                        round_to((amp_db(*l, c) + amp_db(*r, c)) / 2.0, 0.01),
                    );
                }
            }),
            post_inv: Some(|vals, k, c| {
                let g = amp_db_inv(vals.get("Gain").copied().unwrap_or(0.0), c);
                k.insert("Right", g);
            }),
            exports: true,
        },
        Spec {
            guid: HARD_LIMITER,
            name: "Hard Limiter",
            effect: "limiter",
            label: "Hard Limiter(s) came across as Limiter",
            estimate: false,
            knobs: LIMITER_KNOBS,
            entries: vec![
                // 1 is 0 dB and each 0.01 below is 1 dB.
                e(
                    "Ceiling",
                    "Maximum Amplitude",
                    |a, _| round_to(((a - 1.0) * 100.0).clamp(-24.0, 0.0), 0.1),
                    |d, _| (1.0 + d / 100.0).clamp(0.0, 1.0),
                ),
                // An estimate: 2/3 is 0 dB, 30 dB across the knob.
                e(
                    "Input boost",
                    "Input Boost",
                    |b, _| round_to(((b - 2.0 / 3.0) * 30.0).clamp(0.0, 24.0), 0.1),
                    |d, _| (2.0 / 3.0 + d / 30.0).clamp(0.0, 1.0),
                ),
            ],
            fixed: &[],
            post: None,
            post_inv: None,
            exports: true,
        },
        Spec {
            guid: DELAY,
            name: "Delay",
            effect: "echo",
            label: "Delay(s) came across as Echo (an estimate)",
            estimate: true,
            knobs: &[],
            entries: vec![e("Mix", "Left Channel Mix", |v, _| v.clamp(0.0, 1.0), same)],
            fixed: &[("Feedback", 0.0)],
            post: Some(|vals, p, _| {
                // Each side's time is ±500 ms with 0.5 at none.
                let ms = |k: &str| p.get(k).map(|v| ((v - 0.5) * 1000.0).abs());
                let t: Vec<f64> = [
                    ms("Left Channel Delay Time"),
                    ms("Right Channel Delay Time"),
                ]
                .into_iter()
                .flatten()
                .collect();
                if !t.is_empty() {
                    let avg = t.iter().sum::<f64>() / t.len() as f64;
                    vals.insert("Delay".into(), round_to(avg.max(1.0), 0.1));
                }
                let m: Vec<f64> = ["Left Channel Mix", "Right Channel Mix"]
                    .iter()
                    .filter_map(|k| p.get(*k).copied())
                    .collect();
                if !m.is_empty() {
                    vals.insert("Mix".into(), m.iter().sum::<f64>() / m.len() as f64);
                }
            }),
            post_inv: None,
            exports: false,
        },
        Spec {
            guid: ECHO,
            name: "Echo",
            effect: "echo",
            label: "Echo(es) came across as Echo (an estimate)",
            estimate: true,
            knobs: &[],
            entries: vec![
                e(
                    "Delay",
                    "Delay Time (Left Channel)",
                    |v, _| round_to((v * 2000.0).max(1.0), 0.1),
                    same,
                ),
                e(
                    "Feedback",
                    "Feedback (Left Channel)",
                    |v, _| v.clamp(0.0, 0.95),
                    same,
                ),
                e(
                    "Mix",
                    "Echo Level (Left Channel)",
                    |v, _| v.clamp(0.0, 1.0),
                    same,
                ),
            ],
            fixed: &[],
            post: Some(|vals, p, _| {
                let avg = |a: &str, b: &str| match (p.get(a), p.get(b)) {
                    (Some(x), Some(y)) => Some((x + y) / 2.0),
                    (x, y) => x.or(y).copied(),
                };
                if let Some(d) = avg("Delay Time (Left Channel)", "Delay Time (Right Channel)") {
                    vals.insert("Delay".into(), round_to((d * 2000.0).max(1.0), 0.1));
                }
                if let Some(f) = avg("Feedback (Left Channel)", "Feedback (Right Channel)") {
                    vals.insert("Feedback".into(), f.clamp(0.0, 0.95));
                }
                if let Some(m) = avg("Echo Level (Left Channel)", "Echo Level (Right Channel)") {
                    vals.insert("Mix".into(), m.clamp(0.0, 1.0));
                }
            }),
            post_inv: None,
            exports: false,
        },
        Spec {
            guid: ADAPTIVE_NR,
            name: "Adaptive Noise Reduction",
            effect: "noise-reduction",
            label: "Adaptive Noise Reduction(s) came across as Noise Reduction (an estimate)",
            estimate: true,
            knobs: ADAPTIVE_NR_KNOBS,
            entries: vec![e(
                "Reduction",
                "Reduce Noise By",
                |v, _| round_to(v * 100.0, 0.1).clamp(0.0, 40.0),
                |r, _| (r / 100.0).clamp(0.0, 1.0),
            )],
            fixed: &[],
            post: None,
            post_inv: None,
            exports: true,
        },
        Spec {
            guid: NOTCH,
            name: "Notch Filter",
            effect: af2_fx::effects::parametric_eq::ID,
            label: "Notch Filter(s) came across as Parametric EQ cuts (an estimate)",
            estimate: true,
            knobs: &[],
            entries: Vec::new(),
            // Shelves and output flat; up to five narrow deep cuts.
            fixed: &[
                ("ls_on", 0.0),
                ("hs_on", 0.0),
                ("b1_on", 0.0),
                ("b2_on", 0.0),
                ("b3_on", 0.0),
                ("b4_on", 0.0),
                ("b5_on", 0.0),
            ],
            post: Some(|vals, p, c| {
                let mut band = 1;
                for n in 1..=6 {
                    if band > 5 {
                        break;
                    }
                    if !on(p.get(&format!("Enable {n}")).copied()) {
                        continue;
                    }
                    let Some(f) = p.get(&format!("Frequency {n}")) else {
                        continue;
                    };
                    vals.insert(format!("b{band}_on"), 1.0);
                    vals.insert(format!("b{band}_freq"), hz(*f, c));
                    vals.insert(format!("b{band}_gain"), -24.0);
                    vals.insert(format!("b{band}_q"), 20.0);
                    band += 1;
                }
            }),
            post_inv: None,
            exports: false,
        },
    ]
}

// ------------------------------------------------------- slot values

/// An effect's controls: (key, min, max, default).
fn controls(effect: &str) -> Vec<(String, f64, f64, f64)> {
    if let Some(i) = af2_fx::info(effect) {
        return i
            .params
            .iter()
            .map(|p| (p.key.to_string(), p.min, p.max, p.default))
            .collect();
    }
    af2_dsp::Effect::from_id(effect)
        .map(|e| {
            e.params()
                .iter()
                .map(|p| {
                    (
                        p.name.to_string(),
                        p.min as f64,
                        p.max as f64,
                        p.default as f64,
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The index of the value `key` names in `effect`.
pub fn key_index(effect: &str, key: &str) -> Option<usize> {
    controls(effect).iter().position(|c| c.0 == key)
}

/// The key of value `index` of `effect`, as lanes name it: the `af2_fx`
/// key, or None for Audiform2's own processors (Audiform names those
/// `values.N`).
pub fn lane_key(effect: &str, index: usize) -> Option<String> {
    af2_fx::info(effect)?
        .params
        .get(index)
        .map(|p| p.key.to_string())
}

/// A slot of `effect` with `vals` by key (the rest at their defaults),
/// each kept in range.
pub fn make_slot(effect: &str, vals: &Vals) -> EffectSlot {
    let values = controls(effect)
        .iter()
        .map(|(k, min, max, def)| {
            vals.get(k)
                .copied()
                .unwrap_or(*def)
                .clamp(min.min(*max), max.max(*min)) as f32
        })
        .collect();
    super::slot(effect, values)
}

/// A slot's values by key.
pub fn slot_vals(s: &EffectSlot) -> Vals {
    controls(&s.effect)
        .iter()
        .zip(&s.values)
        .map(|(c, v)| (c.0.clone(), *v as f64))
        .collect()
}

// ------------------------------------------------------------ import

/// The values a knob-stored effect's knobs give, by key (only those the
/// knobs name), and its Bypass switch. None for an effect not listed.
pub fn from_knobs(guid: &str, p: &HashMap<String, f64>) -> Option<(Vals, bool)> {
    let spec = specs().into_iter().find(|s| s.guid == guid)?;
    let ctx = Ctx::from(p);
    let mut vals = Vals::new();
    for en in &spec.entries {
        if let Some(v) = p.get(en.src) {
            vals.insert(en.key.to_string(), (en.fwd)(*v, ctx));
        }
    }
    Some((vals, on(p.get("Bypass").copied())))
}

/// A built-in effect brought across.
#[derive(Debug, Clone)]
pub struct FromSesx {
    pub slot: EffectSlot,
    /// Drawn automation of the new effect's values: (value index, points
    /// as (file sample, value in the effect's units, holds)).
    pub lines: Vec<(usize, Points)>,
    pub label: &'static str,
    pub estimate: bool,
    /// Drawn lines on knobs the new effect has no value for.
    pub lost_lines: usize,
}

/// Converts a rack component the table knows (None otherwise).
pub fn from_component(c: Node) -> Option<FromSesx> {
    let id = c.attribute("componentID").unwrap_or_default();
    if let Some(spec) = specs().into_iter().find(|s| s.guid == id) {
        return Some(from_spec(&spec, c));
    }
    let bytes = chunk(c)?;
    let (slot, label, estimate) = match id {
        SINGLE_BAND_COMP => (
            compressor_from(&bytes)?,
            "Single-band Compressor(s) came across as Compressor",
            false,
        ),
        TUBE_COMP => (
            compressor_from(&bytes)?,
            "Tube-modeled Compressor(s) came across as Compressor (without the tube colour)",
            true,
        ),
        DEESSER => (
            deesser_from(&bytes)?,
            "DeEsser(s) came across as De-esser (an estimate)",
            true,
        ),
        ANALOG_DELAY => (
            analog_delay_from(&bytes)?,
            "Analog Delay(s) came across as Echo (an estimate)",
            true,
        ),
        CONVOLUTION => (
            convolution_from(&bytes)?,
            "Convolution Reverb(s) came across as Convolution Reverb with their impulse file",
            true,
        ),
        _ => return None,
    };
    Some(FromSesx {
        slot,
        lines: Vec::new(),
        label,
        estimate,
        lost_lines: 0,
    })
}

fn from_spec(spec: &Spec, c: Node) -> FromSesx {
    let p = named_params(c);
    let ctx = Ctx::from(&p);
    let mut vals = Vals::new();
    for (k, v) in spec.fixed {
        vals.insert(k.to_string(), *v);
    }
    for en in &spec.entries {
        if let Some(v) = p.get(en.src) {
            vals.insert(en.key.to_string(), (en.fwd)(*v, ctx));
        }
    }
    if let Some(f) = spec.post {
        f(&mut vals, &p, ctx);
    }
    let mut slot = make_slot(spec.effect, &vals);
    slot.bypass = on(p.get("Bypass").copied());
    let mut lines = Vec::new();
    let mut lost = 0;
    for prm in c.children().filter(|n| n.has_tag_name("parameter")) {
        let kf = keyframes(prm);
        if kf.is_empty() {
            continue;
        }
        let name = prm.attribute("name").unwrap_or_default();
        let mut any = false;
        for en in spec.entries.iter().filter(|en| en.src == name) {
            let Some(k) = key_index(spec.effect, en.key) else {
                continue;
            };
            any = true;
            lines.push((
                k,
                kf.iter()
                    .map(|&(at, v, hold)| (at, (en.fwd)(v, ctx), hold))
                    .collect(),
            ));
        }
        if !any {
            lost += 1;
        }
    }
    FromSesx {
        slot,
        lines,
        label: spec.label,
        estimate: spec.estimate,
        lost_lines: lost,
    }
}

/// A settings chunk's floats after its 12-byte header (kind byte,
/// `Stefan` backwards, count). None for another layout.
fn stefan(bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.len() < 12 || &bytes[1..8] != b" nafetS" {
        return None;
    }
    Some(
        bytes[12..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect(),
    )
}

fn stefan_bytes(kind: u8, count: u32, words: &[[u8; 4]]) -> Vec<u8> {
    let mut b = vec![kind];
    b.extend_from_slice(b" nafetS");
    b.extend(count.to_le_bytes());
    for w in words {
        b.extend(w);
    }
    b
}

/// Compressor chunks: output gain, threshold (dB), ratio, attack and
/// release (ms), then a switch.
fn compressor_from(bytes: &[u8]) -> Option<EffectSlot> {
    let f = stefan(bytes)?;
    if f.len() < 5 {
        return None;
    }
    let vals: Vals = [
        ("Makeup", f[0] as f64),
        ("Threshold", f[1] as f64),
        ("Ratio", f[2] as f64),
        ("Attack", f[3] as f64),
        ("Release", f[4] as f64),
        // Its knee is hard.
        ("Knee", 0.0),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    Some(make_slot("compressor", &vals))
}

/// DeEsser chunks: threshold (dB), two settings, centre frequency and
/// bandwidth (Hz), two switches.
fn deesser_from(bytes: &[u8]) -> Option<EffectSlot> {
    let f = stefan(bytes)?;
    if f.len() < 4 {
        return None;
    }
    let vals: Vals = [("Threshold", f[0] as f64), ("Frequency", f[3] as f64)]
        .into_iter()
        .map(|(k, v)| (k.to_string(), round_to(v, 0.1)))
        .collect();
    Some(make_slot("de-esser", &vals))
}

/// Analog Delay chunks: mode (a whole number), dry and wet (%), delay
/// (ms), feedback, trash and spread (%).
fn analog_delay_from(bytes: &[u8]) -> Option<EffectSlot> {
    let f = stefan(bytes)?;
    if f.len() < 5 {
        return None;
    }
    let (dry, wet) = (f[1].max(0.0) as f64, f[2].max(0.0) as f64);
    let vals: Vals = [
        ("Delay", (f[3] as f64).clamp(1.0, 2000.0)),
        ("Feedback", (f[4] as f64 / 100.0).clamp(0.0, 0.95)),
        (
            "Mix",
            if dry + wet > 0.0 {
                wet / (dry + wet)
            } else {
                0.0
            },
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), round_to(v, 0.001)))
    .collect();
    Some(make_slot("echo", &vals))
}

/// Convolution Reverb chunks: a version, mix, room size, damping LF and
/// HF (%), pre-delay (ms), width (%), gain (dB); then two whole numbers
/// and the impulse file's path (UTF-16).
fn convolution_from(bytes: &[u8]) -> Option<EffectSlot> {
    let f = stefan(bytes)?;
    if f.len() < 8 {
        return None;
    }
    let mix = (f[1] as f64 / 100.0).clamp(0.0, 1.0);
    let gain = f[7] as f64;
    let db = |x: f64| if x <= 1e-4 { -48.0 } else { 20.0 * x.log10() };
    let vals: Vals = [
        ("dry", db(1.0 - mix).clamp(-48.0, 6.0)),
        ("wet", (db(mix) + gain).clamp(-48.0, 6.0)),
        ("length", (f[2] as f64).clamp(5.0, 100.0)),
        ("predelay", (f[5] as f64).clamp(0.0, 250.0)),
        ("width", (f[6] as f64).clamp(0.0, 200.0)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), round_to(v, 0.01)))
    .collect();
    let mut s = make_slot("af.convolution_reverb", &vals);
    if let Some(path) = impulse_path(bytes) {
        let name = path
            .replace('\\', "/")
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string();
        s.data = serde_json::to_string(&af2_fx::effects::reverb::ReverbData {
            path: path.into(),
            name,
        })
        .ok();
    }
    Some(s)
}

/// The impulse file's path in a Convolution Reverb chunk: UTF-16 after
/// the eight settings and two whole numbers (byte 52), up to a nul.
pub fn impulse_path(bytes: &[u8]) -> Option<String> {
    let units: Vec<u16> = bytes
        .get(52..)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .take_while(|u| *u != 0)
        .collect();
    let s = String::from_utf16(&units).ok()?;
    (!s.trim().is_empty()).then_some(s)
}

// ------------------------------------------------------------ export

/// Whether a slot of `effect` can be written as a built-in effect.
pub fn exports(s: &EffectSlot) -> bool {
    match s.effect.as_str() {
        "compressor" | "de-esser" | "echo" | "af.convolution_reverb" => true,
        "af.phase_curve" => true,
        "af.modulation" => slot_vals(s).get("kind").copied().unwrap_or(0.0) < 0.5,
        e => specs().iter().any(|sp| sp.exports && sp.effect == e),
    }
}

fn component_head(out: &mut String, guid: &str, cid: &str, name: &str) {
    let _ = write!(
        out,
        r#"<component componentGuid="{guid}" componentID="{cid}" name="{name}" presetName="(Custom)">"#
    );
}

fn keys_xml(points: &[(f64, f64)]) -> String {
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

/// `slot` as a built-in effect's `<component>` (None when it has no
/// counterpart). `lanes` are its drawn values: (value index, points as
/// (file sample, value)). Returns the XML and how many lanes went in.
pub fn to_component(
    slot: &EffectSlot,
    lanes: &[(usize, Vec<(f64, f64)>)],
    guid: &str,
) -> Option<(String, usize)> {
    if !exports(slot) {
        return None;
    }
    let mut out = String::new();
    let chunked = |out: &mut String, cid: &str, name: &str, bytes: Vec<u8>| {
        component_head(out, guid, cid, name);
        let _ = write!(
            out,
            "<presetChunkData><![CDATA[{}]]></presetChunkData></component>",
            crate::b64::encode(&bytes)
        );
    };
    let v = slot_vals(slot);
    let get = |k: &str| v.get(k).copied().unwrap_or(0.0) as f32;
    match slot.effect.as_str() {
        "compressor" => {
            let w = [
                get("Makeup"),
                get("Threshold"),
                get("Ratio"),
                get("Attack"),
                get("Release"),
                0.0,
            ]
            .map(f32::to_le_bytes);
            chunked(
                &mut out,
                SINGLE_BAND_COMP,
                "Single-band Compressor",
                stefan_bytes(1, 6, &w),
            );
            return Some((out, 0));
        }
        "de-esser" => {
            let w = [
                get("Threshold"),
                100.0,
                100.0,
                get("Frequency"),
                4000.0,
                0.0,
                1.0,
            ]
            .map(f32::to_le_bytes);
            chunked(&mut out, DEESSER, "DeEsser", stefan_bytes(4, 7, &w));
            return Some((out, 0));
        }
        "echo" => {
            let mix = get("Mix").clamp(0.0, 1.0);
            let mut w = vec![1u32.to_le_bytes()];
            w.extend(
                [
                    (1.0 - mix) * 100.0,
                    mix * 100.0,
                    get("Delay"),
                    get("Feedback") * 100.0,
                    0.0,
                    100.0,
                    0.0,
                ]
                .map(f32::to_le_bytes),
            );
            chunked(
                &mut out,
                ANALOG_DELAY,
                "Analog Delay",
                stefan_bytes(8, 7, &w),
            );
            return Some((out, 0));
        }
        "af.convolution_reverb" => {
            let lin = |d: f32| 10f32.powf(d / 20.0);
            let (dry, wet) = (lin(get("dry")), lin(get("wet")));
            let mix = if dry + wet > 0.0 {
                wet / (dry + wet)
            } else {
                0.0
            };
            let gain = 20.0 * (wet / mix.max(1e-4)).max(1e-6).log10();
            let mut w = vec![1u32.to_le_bytes()];
            w.extend(
                [
                    mix * 100.0,
                    get("length"),
                    70.0,
                    73.0,
                    get("predelay"),
                    get("width"),
                    gain.clamp(-48.0, 24.0),
                ]
                .map(f32::to_le_bytes),
            );
            w.push(17u32.to_le_bytes());
            w.push(8192u32.to_le_bytes());
            let mut bytes = stefan_bytes(6, 8, &w);
            let path = slot
                .data
                .as_deref()
                .and_then(|d| serde_json::from_str::<af2_fx::effects::reverb::ReverbData>(d).ok())
                .map(|d| d.path.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut text: Vec<u8> = path.encode_utf16().flat_map(u16::to_le_bytes).collect();
            text.resize(8192, 0);
            bytes.extend(text);
            chunked(&mut out, CONVOLUTION, "Convolution Reverb", bytes);
            return Some((out, 0));
        }
        "af.phase_curve" => {
            chunked(
                &mut out,
                PHASE_SHIFTER,
                "Graphic Phase Shifter",
                phase_shifter_bytes(slot)?,
            );
            return Some((out, 0));
        }
        _ => {}
    }
    let spec = specs()
        .into_iter()
        .find(|sp| sp.exports && sp.effect == slot.effect)?;
    let ctx = Ctx {
        nyq: EXPORT_RATE_SHARE * 96_000.0,
    };
    let mut knobs: BTreeMap<&'static str, f64> = spec.knobs.iter().copied().collect();
    for en in &spec.entries {
        if let (Some(val), true) = (v.get(en.key), knobs.contains_key(en.src)) {
            knobs.insert(en.src, (en.inv)(*val, ctx));
        }
    }
    if let Some(f) = spec.post_inv {
        f(&v, &mut knobs, ctx);
    }
    if spec.knobs.iter().any(|k| k.0 == "Bypass") {
        knobs.insert("Bypass", slot.bypass as u8 as f64);
    }
    // Drawn lines, by the knob they move.
    let mut lines: HashMap<&str, String> = HashMap::new();
    let mut written = 0;
    for (k, pts) in lanes {
        let Some(key) = controls(&slot.effect).get(*k).map(|c| c.0.clone()) else {
            continue;
        };
        let Some(en) = spec.entries.iter().find(|en| en.key == key) else {
            continue;
        };
        if lines.contains_key(en.src) {
            continue;
        }
        let mapped: Vec<(f64, f64)> = pts.iter().map(|&(at, v)| (at, (en.inv)(v, ctx))).collect();
        lines.insert(en.src, keys_xml(&mapped));
        written += 1;
    }
    component_head(&mut out, guid, spec.guid, spec.name);
    for (i, (name, _)) in spec.knobs.iter().enumerate() {
        let val = knobs[name];
        let keys = lines.get(name).cloned().unwrap_or_default();
        let _ = write!(
            out,
            r#"<parameter index="{i}" name="{}" parameterValue="{val}">{keys}</parameter>"#,
            crate::sesx_out::esc(name)
        );
    }
    out.push_str("</component>");
    Some((out, written))
}

/// A Phase Curve as the Graphic Phase Shifter's float list (see
/// [`super::phase_curve`]): rate, channel, two settings kept as found in
/// real sessions, the axis ends as shares of half the rate, the point
/// count, then (place along a log axis 0..1, phase 0..1 for -180..180).
pub fn phase_shifter_bytes(s: &EffectSlot) -> Option<Vec<u8>> {
    use af2_fx::effects::phase_curve::{FREQ_KEYS, MAX_POINTS, PHASE_KEYS};
    let v = slot_vals(s);
    let rate = 48_000.0f64;
    let nyq = rate / 2.0;
    let n = (v.get("points").copied().unwrap_or(2.0).round() as usize).clamp(2, MAX_POINTS);
    let mut pts: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            (
                v.get(FREQ_KEYS[i])
                    .copied()
                    .unwrap_or(1000.0)
                    .clamp(10.0, nyq),
                v.get(PHASE_KEYS[i])
                    .copied()
                    .unwrap_or(0.0)
                    .clamp(-180.0, 180.0),
            )
        })
        .collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let lo = 20.0f64.min(pts[0].0);
    let hi = nyq;
    let mut f: Vec<f32> = vec![
        rate as f32,
        if v.get("channels").copied().unwrap_or(0.0) >= 1.5 {
            1.0
        } else {
            0.0
        },
        0.5,
        0.030303,
        0.0,
        1.0,
        (lo / nyq) as f32,
        (hi / nyq) as f32,
        0.0,
        0.0,
        n as f32,
    ];
    for (hz, deg) in pts {
        f.push(((hz / lo).ln() / (hi / lo).ln()).clamp(0.0, 1.0) as f32);
        f.push((deg / 360.0 + 0.5) as f32);
    }
    Some(f.iter().flat_map(|x| x.to_le_bytes()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(component: &str) -> String {
        format!("<r>{component}</r>")
    }

    fn convert(xml: &str) -> FromSesx {
        let d = roxmltree::Document::parse(xml).unwrap();
        let c = d
            .descendants()
            .find(|n| n.has_tag_name("component"))
            .unwrap();
        from_component(c).expect("converted")
    }

    fn val(s: &EffectSlot, k: &str) -> f64 {
        slot_vals(s)[k]
    }

    #[test]
    fn eq_bands_shelves_and_passes_convert_and_go_back() {
        let xml = doc(&format!(
            r#"<component componentID="{PARAMETRIC_EQ}" name="Parametric Equalizer">
            <parameter index="31" name="Original Sample Rate" parameterValue="0.25"/>
            <parameter index="1" name="Low Shelf Gain" parameterValue="0.4375"/>
            <parameter index="12" name="EQ Band 3 Center Frequency" parameterValue="0.0354"/>
            <parameter index="13" name="EQ Band 3 Q" parameterValue="0.0002"/>
            <parameter index="14" name="EQ Band 3 Gain" parameterValue="0.5625">
              <parameterKeyframes><parameterKeyframe sampleOffset="0" value="0.5"/><parameterKeyframe sampleOffset="48000" value="0.625"/></parameterKeyframes></parameter>
            <parameter index="15" name="EQ Band 3 Enable" parameterValue="0"/>
            <parameter index="37" name="High-Pass Filter Enable" parameterValue="1"/>
            <parameter index="38" name="High-Pass Cutoff Frequency" parameterValue="0.0025"/>
            <parameter index="39" name="High-Pass Slope" parameterValue="0.8"/>
            <parameter index="25" name="ConstantQ" parameterValue="1"><parameterKeyframe sampleOffset="0" value="1"/></parameter>
            </component>"#
        ));
        let f = convert(&xml);
        let s = &f.slot;
        assert_eq!(s.effect, "af.parametric_eq");
        assert_eq!(val(s, "ls_gain"), -6.0);
        // 20 Hz plus the share of 24 kHz less 20.
        assert_eq!(val(s, "b3_freq"), (20.0 + 0.0354 * 23_980.0f64).round());
        assert_eq!(val(s, "b3_q"), 2.0);
        assert_eq!(val(s, "b3_gain"), 6.0);
        assert_eq!(val(s, "b3_on"), 0.0);
        assert_eq!(val(s, "hp_on"), 1.0);
        assert_eq!(val(s, "hp_freq"), (20.0 + 0.0025 * 23_980.0f64).round());
        assert_eq!(val(s, "hp_slope"), 2.0, "36 dB/oct and up is the steepest");
        // The drawn band gain is a lane on b3_gain, in dB.
        let k = key_index(&s.effect, "b3_gain").unwrap();
        assert_eq!(f.lines.len(), 1);
        assert_eq!(f.lines[0].0, k);
        let vals: Vec<f64> = f.lines[0].1.iter().map(|p| p.1).collect();
        assert_eq!(vals, vec![0.0, 12.0]);
        assert_eq!(f.lost_lines, 1, "the ConstantQ line has nowhere to go");

        // Back: every knob written, the band and line where they were.
        let lane = vec![(k, vec![(0.0, 0.0), (48_000.0, 12.0)])];
        let (out, n) = to_component(s, &lane, "g").unwrap();
        assert_eq!(n, 1);
        assert_eq!(out.matches("<parameter ").count(), PEQ_KNOBS.len());
        let back = convert(&doc(&out));
        for key in [
            "ls_gain", "b3_freq", "b3_q", "b3_gain", "b3_on", "hp_on", "hp_slope",
        ] {
            assert!(
                (val(&back.slot, key) - val(s, key)).abs() < 1.0,
                "{key}: {} vs {}",
                val(&back.slot, key),
                val(s, key)
            );
        }
        assert_eq!(
            back.lines[0].1.iter().map(|p| p.1).collect::<Vec<_>>(),
            vals
        );
    }

    #[test]
    fn expander_mixer_reverb_and_chorus_convert() {
        let w = convert(&doc(&format!(
            r#"<component componentID="{STEREO_EXPANDER}"><parameter name="Center Channel Pan" parameterValue="0.75"/><parameter name="Stereo Expander" parameterValue="0.6666667"/></component>"#
        )));
        assert_eq!(w.slot.effect, "af.stereo_width");
        assert_eq!(val(&w.slot, "pan"), 50.0);
        assert!((val(&w.slot, "width") - 200.0).abs() < 0.01);

        let m = convert(&doc(&format!(
            r#"<component componentID="{CHANNEL_MIXER}"><parameter name="L to L" parameterValue="0.5"/><parameter name="R to L" parameterValue="1"/><parameter name="L to R" parameterValue="0"/><parameter name="R to R" parameterValue="0.75"/></component>"#
        )));
        let s = &m.slot;
        assert_eq!(
            (val(s, "ll"), val(s, "lr"), val(s, "rl"), val(s, "rr")),
            (0.0, 100.0, 100.0, 50.0)
        );
        assert_eq!(val(s, "rl_inv"), 1.0, "0 is all of it, flipped");
        assert_eq!(val(s, "lr_inv"), 0.0);
        let (out, _) = to_component(s, &[], "g").unwrap();
        assert!(out.contains(r#"name="L to R" parameterValue="0""#), "{out}");
        assert!(
            out.contains(r#"name="R to R" parameterValue="0.75""#),
            "{out}"
        );
        assert!(out.contains(r#"name="5 to 5" parameterValue="1""#));

        let r = convert(&doc(&format!(
            r#"<component componentID="{STUDIO_REVERB}"><parameter name="Room Size" parameterValue="0.697"/><parameter name="Wet Output Level" parameterValue="0.25"/><parameter name="Decay" parameterValue="0"/></component>"#
        )));
        assert_eq!(r.slot.effect, "af.reverb");
        assert!(r.estimate);
        assert!((val(&r.slot, "size") - 69.7).abs() < 0.01);
        assert_eq!(val(&r.slot, "wet"), 25.0);
        assert_eq!(val(&r.slot, "decay"), 100.0);

        let c = convert(&doc(&format!(
            r#"<component componentID="{CHORUS}"><parameter name="Voices" parameterValue="4"/><parameter name="Delay Time" parameterValue="0.5"/></component>"#
        )));
        assert_eq!(c.slot.effect, "af.modulation");
        assert_eq!(val(&c.slot, "kind"), 0.0);
        assert_eq!(val(&c.slot, "voices"), 4.0);
        assert_eq!(val(&c.slot, "chorus_delay"), 22.5);
    }

    #[test]
    fn chunk_effects_convert_and_go_back() {
        let comp = stefan_bytes(
            1,
            6,
            &[2.0f32, -17.0, 3.0, 26.0, 220.0, 0.0].map(f32::to_le_bytes),
        );
        let s = compressor_from(&comp).unwrap();
        assert_eq!(s.effect, "compressor");
        assert_eq!(s.values, vec![-17.0, 3.0, 26.0, 220.0, 0.0, 2.0]);
        let (out, _) = to_component(&s, &[], "g").unwrap();
        let d = roxmltree::Document::parse(&out).unwrap();
        let back = compressor_from(&chunk(d.root_element()).unwrap()).unwrap();
        assert_eq!(back.values, s.values);

        let mut words = vec![1u32.to_le_bytes()];
        words.extend([100.0f32, 34.58, 450.0, 53.33, 30.0, 100.0, 0.0].map(f32::to_le_bytes));
        let e = analog_delay_from(&stefan_bytes(8, 7, &words)).unwrap();
        assert_eq!(e.effect, "echo");
        assert_eq!(e.values[0], 450.0);
        assert!((e.values[1] - 0.533).abs() < 1e-3);
        assert!((e.values[2] - 0.257).abs() < 1e-3);

        let mut w = vec![1u32.to_le_bytes()];
        w.extend([45.0f32, 100.0, 70.0, 73.0, 5.0, 100.0, 0.0].map(f32::to_le_bytes));
        w.push(17u32.to_le_bytes());
        w.push(8192u32.to_le_bytes());
        let mut b = stefan_bytes(6, 8, &w);
        b.extend("B:\\IR\\hall.wav".encode_utf16().flat_map(u16::to_le_bytes));
        b.extend([0u8; 8]);
        let cr = convolution_from(&b).unwrap();
        assert_eq!(cr.effect, "af.convolution_reverb");
        assert!(cr.data.as_deref().unwrap().contains("hall.wav"));
        assert!((val(&cr, "wet") - 20.0 * 0.45f64.log10()).abs() < 0.01);
        let (out, _) = to_component(&cr, &[], "g").unwrap();
        let d = roxmltree::Document::parse(&out).unwrap();
        let again = convolution_from(&chunk(d.root_element()).unwrap()).unwrap();
        for k in ["dry", "wet", "predelay", "width", "length"] {
            assert!((val(&again, k) - val(&cr, k)).abs() < 0.05, "{k}");
        }
        assert_eq!(again.data, cr.data);
        assert!(stefan(&[0u8; 20]).is_none());
    }

    #[test]
    fn phase_curve_points_go_back_as_the_phase_shifter() {
        let mut f = [0f32; 17];
        f[0] = 48_000.0;
        f[1] = 1.0;
        f[6] = 20.0 / 24_000.0;
        f[7] = 1.0;
        f[10] = 3.0;
        f[11..17].copy_from_slice(&[0.0, 0.5, 0.5, 1.0, 1.0, 0.25]);
        let bytes: Vec<u8> = f.iter().flat_map(|x| x.to_le_bytes()).collect();
        let s = super::super::phase_curve(&bytes).unwrap();
        let out = phase_shifter_bytes(&s).unwrap();
        let back = super::super::phase_curve(&out).unwrap();
        let (a, b) = (slot_vals(&s), slot_vals(&back));
        for k in [
            "points", "channels", "p1_freq", "p1_phase", "p2_freq", "p2_phase", "p3_freq",
            "p3_phase",
        ] {
            assert!((a[k] - b[k]).abs() < 0.5, "{k}: {} vs {}", a[k], b[k]);
        }
    }
}
