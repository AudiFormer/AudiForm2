// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The presets each effect comes with, listed under Default in the
//! effect window's Presets list. A preset is a name and the controls it
//! changes (by key); loading one puts every control back to its default
//! first, so a preset lists only what differs from the defaults.
//!
//! The settings are starting points written for AudiForm 2: common mixing
//! and repair moves (a low cut and presence lift for a voice, a slow
//! attack for drum punch, a plate for vocals), not copies of any other
//! product's presets. Gains that add up to a boost come with an output
//! trim so loading one does not jump in level.

pub use crate::effects::amp::FactoryPreset;
use crate::effects::{
    amp, band_eq, curve_eq, dynamics, echo, filters, mixer, modulation, parametric_eq, pitch,
    restore, speech, studio, tube, tube_master, verb, vocal, width,
};

/// The presets effect `id` comes with (empty for one that has none).
pub fn factory(id: &str) -> &'static [FactoryPreset] {
    match id {
        amp::AMP => amp::PRESETS,
        parametric_eq::ID => PARAMETRIC_EQ,
        band_eq::ID => BAND_EQ,
        curve_eq::ID => CURVE_EQ,
        filters::PRECISION => PRECISION_FILTER,
        filters::HUM => HUM_CUT,
        dynamics::DYNAMICS => DYNAMICS,
        dynamics::LIMITER => LIMITER,
        dynamics::DEESSER => DEESSER,
        tube::OPTO => OPTO,
        tube::PREAMP => PREAMP,
        tube_master::ID => TUBE_MASTER,
        vocal::VOCAL => VOX_STRIP,
        speech::LEVELER => SPEECH_LEVELER,
        speech::GATE => BREATH_GATE,
        restore::NOISE => NOISE_REDUCER,
        restore::HISS => HISS_REDUCER,
        restore::ROOM => ROOM_REDUCER,
        echo::ID => ECHO,
        verb::ID => REVERB,
        studio::ID => STUDIO_REVERB,
        modulation::ID => MODULATION,
        width::WIDTH => STEREO_WIDTH,
        mixer::ID => CHANNEL_MIXER,
        pitch::ID => PITCH,
        _ => &[],
    }
}

// ------------------------------------------------------------ EQ

/// Slope picks of the Parametric EQ's filters.
const S12: f64 = 0.0;
const S24: f64 = 1.0;
const S48: f64 = 2.0;

const PARAMETRIC_EQ: &[FactoryPreset] = &[
    (
        "Vocal: Clarity",
        &[
            ("hp_on", 1.0), ("hp_freq", 90.0), ("hp_slope", S12),
            ("b1_freq", 250.0), ("b1_gain", -2.5), ("b1_q", 1.2),
            ("b4_freq", 3500.0), ("b4_gain", 2.5), ("b4_q", 0.9),
            ("hs_freq", 10000.0), ("hs_gain", 3.0), ("hs_q", 0.7),
            ("gain", -1.5),
        ],
    ),
    (
        "Vocal: Warm",
        &[
            ("hp_on", 1.0), ("hp_freq", 70.0), ("hp_slope", S12),
            ("ls_freq", 150.0), ("ls_gain", 2.0), ("ls_q", 0.7),
            ("b4_freq", 3000.0), ("b4_gain", -1.5), ("b4_q", 1.5),
            ("hs_freq", 12000.0), ("hs_gain", -1.5), ("hs_q", 0.7),
            ("gain", -1.0),
        ],
    ),
    (
        "Speech / Podcast",
        &[
            ("hp_on", 1.0), ("hp_freq", 80.0), ("hp_slope", S24),
            ("b1_freq", 200.0), ("b1_gain", -2.0),
            ("b2_freq", 500.0), ("b2_gain", -1.5), ("b2_q", 1.4),
            ("b4_freq", 4000.0), ("b4_gain", 2.0),
            ("hs_freq", 12000.0), ("hs_gain", 1.5), ("hs_q", 0.7),
            ("gain", -1.0),
        ],
    ),
    (
        "Kick Drum",
        &[
            ("hp_on", 1.0), ("hp_freq", 30.0), ("hp_slope", S12),
            ("b1_freq", 60.0), ("b1_gain", 4.0), ("b1_q", 1.2),
            ("b2_freq", 350.0), ("b2_gain", -5.0), ("b2_q", 2.0),
            ("b4_freq", 3500.0), ("b4_gain", 3.0), ("b4_q", 1.5),
            ("gain", -3.0),
        ],
    ),
    (
        "Snare Drum",
        &[
            ("hp_on", 1.0), ("hp_freq", 80.0), ("hp_slope", S12),
            ("b1_freq", 200.0), ("b1_gain", 2.5), ("b1_q", 1.5),
            ("b2_freq", 900.0), ("b2_gain", -3.0), ("b2_q", 2.0),
            ("b4_freq", 5000.0), ("b4_gain", 2.5),
            ("hs_freq", 10000.0), ("hs_gain", 2.0), ("hs_q", 0.7),
            ("gain", -2.0),
        ],
    ),
    (
        "Bass Guitar",
        &[
            ("hp_on", 1.0), ("hp_freq", 35.0), ("hp_slope", S24),
            ("b1_freq", 80.0), ("b1_gain", 2.5),
            ("b2_freq", 250.0), ("b2_gain", -3.0), ("b2_q", 1.4),
            ("b3_freq", 800.0), ("b3_gain", 2.0),
            ("b4_freq", 2500.0), ("b4_gain", 1.5), ("b4_q", 1.2),
            ("gain", -2.0),
        ],
    ),
    (
        "Acoustic Guitar",
        &[
            ("hp_on", 1.0), ("hp_freq", 80.0), ("hp_slope", S12),
            ("b1_freq", 200.0), ("b1_gain", -3.0),
            ("b4_freq", 3000.0), ("b4_gain", 1.5),
            ("hs_freq", 10000.0), ("hs_gain", 3.0), ("hs_q", 0.7),
            ("gain", -1.5),
        ],
    ),
    (
        "Electric Guitar: Sit in Mix",
        &[
            ("hp_on", 1.0), ("hp_freq", 90.0), ("hp_slope", S12),
            ("b1_freq", 250.0), ("b1_gain", -2.0),
            ("b3_freq", 1500.0), ("b3_gain", 2.0),
            ("b5_freq", 7000.0), ("b5_gain", -2.0),
            ("lp_on", 1.0), ("lp_freq", 12000.0), ("lp_slope", S12),
        ],
    ),
    (
        "Piano",
        &[
            ("hp_on", 1.0), ("hp_freq", 40.0), ("hp_slope", S12),
            ("b1_freq", 300.0), ("b1_gain", -2.0),
            ("b4_freq", 3000.0), ("b4_gain", 1.5),
            ("hs_freq", 10000.0), ("hs_gain", 2.0), ("hs_q", 0.7),
            ("gain", -1.0),
        ],
    ),
    (
        "De-Mud (Low Mids)",
        &[
            ("b1_freq", 300.0), ("b1_gain", -4.0), ("b1_q", 1.4),
            ("b2_freq", 500.0), ("b2_gain", -2.0),
        ],
    ),
    (
        "Master: Gentle Smile",
        &[
            ("hp_on", 1.0), ("hp_freq", 25.0), ("hp_slope", S24),
            ("ls_freq", 80.0), ("ls_gain", 1.5), ("ls_q", 0.7),
            ("b3_freq", 1000.0), ("b3_gain", -1.0), ("b3_q", 0.5),
            ("hs_freq", 12000.0), ("hs_gain", 1.5), ("hs_q", 0.7),
            ("gain", -1.0),
        ],
    ),
    (
        "Rumble and Hiss Cut",
        &[
            ("hp_on", 1.0), ("hp_freq", 60.0), ("hp_slope", S24),
            ("lp_on", 1.0), ("lp_freq", 16000.0), ("lp_slope", S12),
        ],
    ),
    (
        "Telephone",
        &[
            ("hp_on", 1.0), ("hp_freq", 400.0), ("hp_slope", S48),
            ("lp_on", 1.0), ("lp_freq", 3400.0), ("lp_slope", S48),
            ("b3_freq", 1500.0), ("b3_gain", 4.0),
            ("gain", 3.0),
        ],
    ),
    (
        "AM Radio",
        &[
            ("hp_on", 1.0), ("hp_freq", 200.0), ("hp_slope", S24),
            ("lp_on", 1.0), ("lp_freq", 5000.0), ("lp_slope", S24),
            ("b3_freq", 1800.0), ("b3_gain", 3.0), ("b3_q", 0.8),
            ("gain", 1.0),
        ],
    ),
];

/// A 10-band (octave) Band EQ setting: the gains from 31.5 Hz to 16 kHz
/// and the output trim. Written out in full so the table reads as the
/// sliders do.
macro_rules! octaves {
    ($name:expr, [$a:expr, $b:expr, $c:expr, $d:expr, $e:expr, $f:expr, $g:expr, $h:expr, $i:expr, $j:expr], $out:expr) => {
        (
            $name,
            &[
                ("bands", 0.0),
                ("g01", $a), ("g02", $b), ("g03", $c), ("g04", $d), ("g05", $e),
                ("g06", $f), ("g07", $g), ("g08", $h), ("g09", $i), ("g10", $j),
                ("output", $out),
            ],
        )
    };
}

const BAND_EQ: &[FactoryPreset] = &[
    //                       31    63   125   250   500    1k    2k    4k    8k   16k
    octaves!("Bass Boost", [6.0, 5.0, 3.5, 1.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], -4.0),
    octaves!("Bass Cut", [-8.0, -6.0, -3.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 0.0),
    octaves!("Treble Boost", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 3.0, 5.0, 6.0], -4.0),
    octaves!("Treble Cut", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, -3.0, -6.0, -8.0], 0.0),
    octaves!("Loudness", [5.0, 4.0, 2.0, 0.0, -1.0, -1.0, 0.0, 2.0, 4.0, 5.0], -3.0),
    octaves!("Vocal Boost", [-2.0, -2.0, -1.0, 0.0, 1.5, 3.0, 3.5, 2.5, 1.0, 0.0], -2.5),
    octaves!("Spoken Word", [-6.0, -3.0, -1.0, 0.0, 1.0, 2.0, 3.0, 2.0, 0.0, -2.0], -1.5),
    octaves!("Rock", [4.0, 3.0, 1.0, -1.0, -2.0, -1.0, 1.0, 3.0, 4.0, 4.0], -3.0),
    octaves!("Pop", [-1.0, 1.0, 3.0, 3.5, 2.0, 0.0, -1.0, -1.0, 0.0, 1.0], -2.5),
    octaves!("Electronic", [5.0, 4.0, 1.0, 0.0, -1.5, -1.0, 0.0, 1.5, 3.0, 3.5], -3.5),
    octaves!("Hip-Hop", [5.0, 4.5, 2.0, 1.0, -1.0, -1.0, 0.5, 0.0, 1.0, 2.0], -3.5),
    octaves!("Jazz", [3.0, 2.0, 1.0, 1.5, -1.0, -1.0, 0.0, 1.5, 2.5, 3.0], -2.0),
    octaves!("Classical", [2.0, 1.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 1.5, 2.0], -1.5),
    octaves!("Acoustic", [2.5, 2.0, 1.0, 0.5, 1.0, 1.0, 2.0, 2.5, 2.0, 1.0], -2.0),
    octaves!("Small Speakers", [-6.0, -3.0, 0.0, 2.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0], -1.0),
];

const CURVE_EQ: &[FactoryPreset] = &[
    // The points keep their default frequencies (40, 100, 250, 600 Hz,
    // 1.5, 3.5, 8, 16 kHz) unless a preset moves them.
    (
        "Smile",
        &[
            ("p1_gain", 4.0), ("p2_gain", 3.0), ("p3_gain", 0.5), ("p4_gain", -1.0),
            ("p5_gain", -1.0), ("p6_gain", 0.5), ("p7_gain", 3.0), ("p8_gain", 4.0),
        ],
    ),
    (
        "Warm",
        &[
            ("p1_gain", 1.0), ("p2_gain", 2.0), ("p3_gain", 1.5), ("p4_gain", 0.5),
            ("p6_gain", -1.0), ("p7_gain", -2.0), ("p8_gain", -3.0),
        ],
    ),
    (
        "Bright and Airy",
        &[("p5_gain", 0.5), ("p6_gain", 1.5), ("p7_gain", 3.0), ("p8_gain", 5.0)],
    ),
    (
        "Bass Boost",
        &[("p1_gain", 6.0), ("p2_gain", 4.0), ("p3_gain", 1.0)],
    ),
    (
        "Speech Intelligibility",
        &[
            ("p1_gain", -12.0), ("p2_gain", -4.0), ("p3_gain", -1.0), ("p4_gain", -1.0),
            ("p6_gain", 3.0), ("p7_gain", 1.5),
        ],
    ),
    (
        "Dark Lo-Fi",
        &[("p1_gain", -6.0), ("p6_gain", -2.0), ("p7_gain", -12.0), ("p8_gain", -24.0)],
    ),
    (
        "Telephone",
        &[
            ("p1_freq", 150.0), ("p1_gain", -40.0),
            ("p2_freq", 350.0), ("p2_gain", 0.0),
            ("p3_freq", 800.0), ("p3_gain", 2.0),
            ("p4_freq", 1500.0), ("p4_gain", 3.0),
            ("p5_freq", 2500.0), ("p5_gain", 2.0),
            ("p6_freq", 3400.0), ("p6_gain", 0.0),
            ("p7_freq", 5000.0), ("p7_gain", -40.0),
            ("p8_freq", 16000.0), ("p8_gain", -40.0),
        ],
    ),
];

// Precision Filter picks.
const LOW_PASS: f64 = 0.0;
const HIGH_PASS: f64 = 1.0;
const BAND_PASS: f64 = 2.0;

const PRECISION_FILTER: &[FactoryPreset] = &[
    ("Rumble Cut (30 Hz)", &[("mode", HIGH_PASS), ("order", 1.0), ("freq", 30.0)]),
    ("Voice Low Cut (80 Hz)", &[("mode", HIGH_PASS), ("order", 1.0), ("freq", 80.0)]),
    (
        "Steep High Cut (16 kHz)",
        &[("design", 3.0), ("mode", LOW_PASS), ("order", 3.0), ("freq", 16000.0)],
    ),
    (
        "Sub Bass Only (below 120 Hz)",
        &[("mode", LOW_PASS), ("order", 3.0), ("freq", 120.0)],
    ),
    (
        "Telephone Band",
        &[("mode", BAND_PASS), ("order", 2.0), ("freq", 300.0), ("freq2", 3400.0)],
    ),
];

const HUM_CUT: &[FactoryPreset] = &[
    ("60 Hz (Americas)", &[("mains", 0.0)]),
    ("50 Hz (Europe, Asia, Africa)", &[("mains", 1.0)]),
    ("60 Hz Heavy Buzz", &[("mains", 0.0), ("harmonics", 8.0), ("q", 15.0)]),
    ("50 Hz Heavy Buzz", &[("mains", 1.0), ("harmonics", 8.0), ("q", 15.0)]),
];

// ------------------------------------------------------------ dynamics

const DYNAMICS: &[FactoryPreset] = &[
    (
        "Vocal: Gentle",
        &[("threshold", -18.0), ("ratio", 2.5), ("attack", 8.0), ("release", 120.0), ("makeup", 3.0)],
    ),
    (
        "Vocal: Upfront",
        &[("threshold", -24.0), ("ratio", 4.0), ("attack", 4.0), ("release", 80.0), ("knee", 4.0), ("makeup", 6.0)],
    ),
    (
        "Speech / Podcast",
        &[("threshold", -24.0), ("ratio", 3.5), ("attack", 5.0), ("release", 150.0), ("knee", 8.0), ("makeup", 6.0)],
    ),
    (
        "Drum Punch",
        &[("threshold", -20.0), ("ratio", 4.0), ("attack", 25.0), ("release", 80.0), ("knee", 2.0), ("makeup", 4.0)],
    ),
    (
        "Drum Bus Glue",
        &[("threshold", -16.0), ("ratio", 2.0), ("attack", 30.0), ("release", 150.0), ("makeup", 2.0)],
    ),
    (
        "Bass: Even Out",
        &[("threshold", -22.0), ("ratio", 4.0), ("attack", 15.0), ("release", 200.0), ("makeup", 4.0)],
    ),
    (
        "Acoustic Guitar",
        &[("threshold", -20.0), ("ratio", 3.0), ("attack", 15.0), ("release", 150.0), ("makeup", 3.0)],
    ),
    (
        "Master Bus: Gentle",
        &[("threshold", -12.0), ("ratio", 1.5), ("attack", 30.0), ("release", 300.0), ("knee", 12.0), ("makeup", 1.5)],
    ),
    (
        "Heavy Squash",
        &[("threshold", -35.0), ("ratio", 10.0), ("attack", 1.0), ("release", 60.0), ("knee", 0.0), ("makeup", 12.0)],
    ),
];

const LIMITER: &[FactoryPreset] = &[
    ("Master: -1 dB True Peak", &[("detect", 1.0), ("ceiling", -1.0)]),
    (
        "Master: Loud",
        &[("detect", 1.0), ("ceiling", -1.0), ("input_boost", 4.0), ("release", 60.0)],
    ),
    ("Broadcast: -2 dB True Peak", &[("detect", 1.0), ("ceiling", -2.0), ("release", 150.0)]),
    (
        "Safety Net (-0.3 dB)",
        &[("ceiling", -0.3), ("lookahead", 2.0), ("release", 50.0)],
    ),
];

const DEESSER: &[FactoryPreset] = &[
    ("Lower Voice", &[("freq", 5000.0), ("reduction", 6.0)]),
    ("Higher Voice", &[("freq", 7000.0), ("reduction", 6.0)]),
    ("Gentle", &[("threshold", -24.0), ("reduction", 4.0)]),
    ("Heavy", &[("freq", 6500.0), ("threshold", -40.0), ("reduction", 12.0)]),
];

const OPTO: &[FactoryPreset] = &[
    // Peak Reduction sets the threshold (0.6 dB lower per step), Gain the
    // makeup (half a dB per step above 40).
    ("Vocal Leveling", &[("peak", 45.0), ("gain", 48.0), ("emphasis", 20.0)]),
    ("Bass", &[("peak", 50.0), ("gain", 50.0), ("tube", 40.0)]),
    ("Gentle Bus", &[("peak", 30.0), ("gain", 44.0), ("tube", 20.0)]),
    ("Parallel Crush", &[("peak", 75.0), ("gain", 60.0), ("mix", 40.0)]),
    ("Limit", &[("mode", 1.0), ("peak", 60.0), ("gain", 54.0)]),
];

const PREAMP: &[FactoryPreset] = &[
    ("Warmth", &[("drive", 25.0), ("low", 1.5), ("high", -1.0)]),
    ("Bright Tube", &[("drive", 30.0), ("high", 2.0)]),
    ("Saturated", &[("drive", 65.0), ("output", -4.0)]),
    ("Parallel Grit", &[("drive", 80.0), ("mix", 35.0)]),
];

// Tube Master's picks are by position: Threshold -30 dB + 3 dB a step;
// Attack and Release slow (0) to fast (4); Ratio 1.5:1, 2:1, 3:1, 4:1,
// 6:1, 10:1, 20:1, Limit, Negative.
const TUBE_MASTER: &[FactoryPreset] = &[
    (
        "Mastering: Gentle",
        &[("thr", 5.0), ("ratio", 0.0), ("att", 1.0), ("rel", 2.0), ("sc", 1.0), ("detect", 1.0)],
    ),
    (
        "Mix Bus Glue",
        &[("thr", 4.0), ("ratio", 1.0), ("att", 2.0), ("rel", 3.0), ("sc", 1.0), ("detect", 1.0)],
    ),
    (
        "Punch",
        &[("thr", 4.0), ("ratio", 2.0), ("att", 0.0), ("rel", 4.0), ("detect", 1.0)],
    ),
    (
        "Parallel Crush",
        &[("thr", 2.0), ("ratio", 5.0), ("att", 4.0), ("rel", 3.0), ("mix", 40.0)],
    ),
    (
        "Warm Saturation",
        &[("thr", 8.0), ("ratio", 0.0), ("sat", 1.0), ("shape", 60.0)],
    ),
];

const VOX_STRIP: &[FactoryPreset] = &[
    (
        "Podcast Host",
        &[
            ("low_cut", 1.0), ("comp", 45.0), ("deess", 30.0),
            ("low", -1.0), ("high", 1.5), ("air", 20.0),
        ],
    ),
    (
        "Pop Vocal",
        &[
            ("low_cut", 1.0), ("comp", 55.0), ("deess", 35.0),
            ("tone_style", 2.0), ("high", 2.0), ("air", 35.0),
            ("fx_on", 1.0), ("fx_style", 1.0), ("fx", 20.0),
        ],
    ),
    (
        "Rap Vocal",
        &[
            ("low_cut", 1.0), ("comp", 65.0), ("deess", 30.0),
            ("tone_style", 2.0), ("mid", 1.5), ("high", 2.0), ("air", 25.0),
        ],
    ),
    (
        "Warm Vintage",
        &[
            ("dyn_style", 2.0), ("comp", 40.0), ("tone_style", 1.0),
            ("low", 1.5), ("high", -1.0),
            ("fx_on", 1.0), ("fx_style", 2.0), ("fx", 25.0),
        ],
    ),
    (
        "Radio Voice",
        &[("low_cut", 1.0), ("comp", 50.0), ("fx_on", 1.0), ("fx_style", 3.0), ("fx", 70.0)],
    ),
];

const SPEECH_LEVELER: &[FactoryPreset] = &[
    ("Podcast", &[("target", -18.0), ("max_gain", 12.0)]),
    ("Gentle", &[("speed", 3000.0), ("max_gain", 6.0), ("max_cut", 6.0)]),
    (
        "Interview (Fast)",
        &[("speed", 600.0), ("max_gain", 15.0), ("max_cut", 12.0), ("floor", -45.0)],
    ),
];

const BREATH_GATE: &[FactoryPreset] = &[
    (
        "Soften Breaths",
        &[("threshold", -45.0), ("hold", 80.0), ("release", 250.0), ("range", -12.0)],
    ),
    (
        "Podcast",
        &[("threshold", -42.0), ("hold", 100.0), ("release", 300.0), ("range", -20.0)],
    ),
    (
        "Tight",
        &[("threshold", -35.0), ("attack", 1.0), ("hold", 30.0), ("release", 120.0), ("range", -40.0)],
    ),
];

// ------------------------------------------------------------ repair

const NOISE_REDUCER: &[FactoryPreset] = &[
    ("Light", &[("reduction", 6.0), ("strength", 1.5)]),
    ("Strong", &[("reduction", 20.0), ("strength", 3.0), ("smoothing", 70.0)]),
];

const HISS_REDUCER: &[FactoryPreset] = &[
    ("Light", &[("reduction", 6.0), ("strength", 1.5)]),
    ("Tape Hiss", &[("reduction", 12.0), ("freq", 4000.0)]),
];

const ROOM_REDUCER: &[FactoryPreset] = &[
    ("Small Room", &[("amount", 30.0), ("decay", 0.4), ("reduction", 8.0)]),
    ("Large Room", &[("amount", 70.0), ("decay", 1.2), ("reduction", 18.0)]),
];

// ------------------------------------------------------------ time

// Delay and Echo's note picks.
const EIGHTH: f64 = 7.0;
const DOTTED_EIGHTH: f64 = 8.0;
const QUARTER: f64 = 10.0;
const DOTTED_QUARTER: f64 = 11.0;

const ECHO: &[FactoryPreset] = &[
    (
        "Slapback",
        &[("kind", 1.0), ("time", 110.0), ("feedback", 0.0), ("tone", 5000.0), ("low_cut", 100.0)],
    ),
    (
        "Quarter Note",
        &[("sync", 1.0), ("note", QUARTER), ("feedback", 30.0), ("wet", 25.0), ("low_cut", 150.0)],
    ),
    (
        "Dotted Eighth",
        &[
            ("sync", 1.0), ("note", DOTTED_EIGHTH), ("feedback", 40.0), ("wet", 25.0),
            ("low_cut", 200.0), ("tone", 5000.0),
        ],
    ),
    (
        "Ping-Pong Eighths",
        &[("sync", 1.0), ("note", EIGHTH), ("pingpong", 1.0), ("feedback", 45.0), ("wet", 25.0)],
    ),
    (
        "Tape Echo",
        &[
            ("kind", 1.0), ("time", 380.0), ("feedback", 50.0), ("depth", 20.0), ("rate", 0.5),
            ("drive", 25.0), ("tone", 3500.0), ("low_cut", 150.0),
        ],
    ),
    (
        "Analog Dub",
        &[
            ("kind", 2.0), ("sync", 1.0), ("note", DOTTED_QUARTER), ("feedback", 75.0),
            ("tone", 2500.0), ("low_cut", 200.0), ("drive", 30.0), ("wet", 35.0),
        ],
    ),
    (
        "Ambient Wash",
        &[
            ("time", 700.0), ("feedback", 60.0), ("depth", 25.0), ("rate", 0.3),
            ("tone", 4000.0), ("low_cut", 250.0), ("wet", 25.0),
        ],
    ),
    ("Send (100% Wet)", &[("dry", 0.0), ("wet", 100.0), ("low_cut", 150.0)]),
];

// Reverb types.
const ROOM: f64 = 0.0;
const HALL: f64 = 1.0;
const PLATE: f64 = 2.0;
const SPRING: f64 = 3.0;
const STUDIO: f64 = 5.0;

const REVERB: &[FactoryPreset] = &[
    (
        "Vocal Plate",
        &[
            ("kind", PLATE), ("decay", 1800.0), ("predelay", 30.0), ("damping", 40.0),
            ("diffusion", 70.0), ("low_cut", 200.0), ("high_cut", 9000.0), ("wet", 22.0),
        ],
    ),
    (
        "Snare Plate",
        &[
            ("kind", PLATE), ("size", 45.0), ("decay", 1300.0), ("predelay", 15.0),
            ("low_cut", 250.0), ("wet", 25.0),
        ],
    ),
    (
        "Small Room",
        &[
            ("kind", ROOM), ("size", 25.0), ("decay", 500.0), ("predelay", 5.0),
            ("early", 40.0), ("damping", 60.0), ("wet", 20.0),
        ],
    ),
    (
        "Drum Room",
        &[
            ("kind", ROOM), ("size", 40.0), ("decay", 800.0), ("predelay", 3.0),
            ("early", 50.0), ("damping", 45.0), ("low_cut", 120.0), ("wet", 25.0),
        ],
    ),
    (
        "Subtle Ambience",
        &[("kind", STUDIO), ("size", 30.0), ("decay", 600.0), ("early", 50.0), ("wet", 12.0)],
    ),
    (
        "Concert Hall",
        &[
            ("kind", HALL), ("size", 85.0), ("decay", 3200.0), ("predelay", 35.0), ("early", 25.0),
            ("damping", 40.0), ("diffusion", 75.0), ("low_cut", 80.0), ("wet", 28.0),
        ],
    ),
    (
        "Cathedral",
        &[
            ("kind", HALL), ("size", 100.0), ("decay", 7000.0), ("predelay", 60.0),
            ("damping", 30.0), ("modulation", 40.0), ("wet", 35.0),
        ],
    ),
    (
        "Spring Tank",
        &[
            ("kind", SPRING), ("size", 40.0), ("decay", 2000.0), ("low_cut", 300.0),
            ("high_cut", 6000.0), ("wet", 30.0),
        ],
    ),
    (
        "Huge Ambient",
        &[
            ("kind", HALL), ("size", 100.0), ("decay", 12000.0), ("predelay", 80.0),
            ("modulation", 60.0), ("high_cut", 7000.0), ("wet", 40.0),
        ],
    ),
    ("Send (100% Wet)", &[("dry", 0.0), ("wet", 100.0), ("low_cut", 150.0)]),
];

const STUDIO_REVERB: &[FactoryPreset] = &[
    (
        "Vocal Room",
        &[("size", 35.0), ("decay", 900.0), ("early", 30.0), ("low_cut", 200.0), ("high_cut", 10000.0), ("wet", 20.0)],
    ),
    (
        "Drum Room",
        &[("size", 40.0), ("decay", 700.0), ("early", 50.0), ("damping", 45.0), ("low_cut", 120.0), ("wet", 25.0)],
    ),
    (
        "Large Hall",
        &[("size", 90.0), ("decay", 3500.0), ("early", 15.0), ("damping", 35.0), ("diffusion", 75.0), ("wet", 28.0)],
    ),
    ("Send (100% Wet)", &[("dry", 0.0), ("wet", 100.0), ("low_cut", 150.0)]),
];

// ------------------------------------------------------------ modulation and stereo

// Modulation types.
const CHORUS: f64 = 0.0;
const FLANGER: f64 = 1.0;
const PHASER: f64 = 2.0;
const DOUBLE: f64 = 3.0;

const MODULATION: &[FactoryPreset] = &[
    (
        "Subtle Chorus",
        &[("kind", CHORUS), ("rate", 0.4), ("depth", 25.0), ("voices", 2.0), ("chorus_delay", 15.0), ("wet", 35.0)],
    ),
    (
        "Lush Chorus",
        &[("kind", CHORUS), ("rate", 0.8), ("depth", 60.0), ("voices", 4.0), ("chorus_delay", 22.0)],
    ),
    (
        "Clean Guitar Chorus",
        &[("kind", CHORUS), ("rate", 1.2), ("depth", 45.0), ("voices", 2.0), ("chorus_delay", 12.0)],
    ),
    (
        "Subtle Flanger",
        &[("kind", FLANGER), ("rate", 0.3), ("depth", 40.0), ("feedback", 30.0), ("wet", 40.0)],
    ),
    (
        "Jet Flanger",
        &[("kind", FLANGER), ("rate", 0.15), ("depth", 80.0), ("feedback", 70.0), ("flange_delay", 2.0)],
    ),
    (
        "Classic Phaser",
        &[("kind", PHASER), ("rate", 0.5), ("depth", 70.0), ("stages", 1.0), ("feedback", 40.0), ("center", 800.0)],
    ),
    (
        "Deep Phaser",
        &[("kind", PHASER), ("rate", 0.25), ("depth", 85.0), ("stages", 3.0), ("feedback", 60.0), ("center", 600.0)],
    ),
    (
        "Wide Double Track",
        &[("kind", DOUBLE), ("adt_offset", 15.0), ("src_pan", -70.0), ("adt_pan", 70.0)],
    ),
];

const STEREO_WIDTH: &[FactoryPreset] = &[
    ("Mono", &[("width", 0.0)]),
    ("Narrow", &[("width", 60.0)]),
    ("A Little Wider", &[("width", 130.0)]),
    ("Wide", &[("width", 170.0)]),
];

const CHANNEL_MIXER: &[FactoryPreset] = &[
    ("Swap Left and Right", &[("ll", 0.0), ("lr", 100.0), ("rl", 100.0), ("rr", 0.0)]),
    ("Mono (Sum)", &[("ll", 50.0), ("lr", 50.0), ("rl", 50.0), ("rr", 50.0)]),
    ("Left to Both", &[("rl", 100.0), ("rr", 0.0)]),
    ("Right to Both", &[("ll", 0.0), ("lr", 100.0)]),
    ("Flip Right Polarity", &[("rr_inv", 1.0)]),
];

const PITCH: &[FactoryPreset] = &[
    ("Octave Down", &[("semitones", -12.0)]),
    ("Fourth Down", &[("semitones", -5.0)]),
    ("Fifth Up", &[("semitones", 7.0)]),
    ("Octave Up", &[("semitones", 12.0)]),
    ("Slight Detune", &[("cents", 12.0)]),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_sine(f: f64, amp: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = (std::f64::consts::TAU * f * i as f64 / 48_000.0).sin() as f32 * amp;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn every_preset_sets_real_controls_in_range_and_plays() {
        let mut count = 0;
        for info in crate::registry() {
            let mut names = std::collections::HashSet::new();
            for (name, set) in factory(info.id) {
                assert!(names.insert(*name), "{}: {name} listed twice", info.id);
                for (k, v) in *set {
                    let p = info.params.iter().find(|p| p.key == *k);
                    let p = p.unwrap_or_else(|| panic!("{} {name}: no control {k}", info.id));
                    assert!(*v >= p.min && *v <= p.max, "{} {name}: {k} = {v}", info.id);
                }
                let saved: Vec<(String, f64)> =
                    set.iter().map(|(k, v)| (k.to_string(), *v)).collect();
                let params = crate::params_from(info.id, &saved).unwrap();
                let mut fx = crate::build(info.id, 48_000, &params).unwrap();
                let mut buf = stereo_sine(220.0, 0.25, 16_384);
                fx.process(&mut buf);
                assert!(buf.iter().all(|v| v.is_finite()), "{} {name}", info.id);
                count += 1;
            }
        }
        assert!(count > 100, "{count} presets");
    }

    #[test]
    fn curve_eq_presets_keep_points_in_order() {
        // Not needed by the effect (it sorts), but the graph reads better.
        let def = crate::info(curve_eq::ID).unwrap();
        for (name, set) in CURVE_EQ {
            let freq = |n: usize| {
                let k = format!("p{n}_freq");
                set.iter()
                    .find(|(key, _)| *key == k)
                    .map(|(_, v)| *v)
                    .unwrap_or_else(|| def.params.iter().find(|p| p.key == k).unwrap().default)
            };
            for n in 1..8 {
                assert!(freq(n) < freq(n + 1), "{name}: point {n}");
            }
        }
    }
}
