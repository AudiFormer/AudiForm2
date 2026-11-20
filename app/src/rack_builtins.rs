// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The rack presets that come with the app, listed under the saved ones in
//! the Effects rack's Presets list. The four echoes are the project lead's
//! own settings from Audiform; the rest are Audiform27's own, set by ear.

use af27_core::{EffectSlot, Rack};

/// One built-in rack preset.
pub struct Builtin {
    pub group: &'static str,
    pub name: &'static str,
    pub chain: Vec<EffectSlot>,
}

/// A slot of `id` with `sets` over its defaults. A key is the control's key
/// for Audiform effects and its name for the built-in effects.
fn s(id: &str, sets: &[(&str, f32)]) -> EffectSlot {
    let mut slot = crate::rack::new_slot(id);
    for (k, v) in sets {
        if let Some(i) = index(id, k) {
            if let Some(x) = slot.values.get_mut(i) {
                *x = *v;
            }
        }
    }
    slot
}

fn index(id: &str, key: &str) -> Option<usize> {
    if let Some(info) = af27_fx::info(id) {
        return info.params.iter().position(|p| p.key == key);
    }
    crate::fx::controls(id).iter().position(|c| c.name == key)
}

// ------------------------------------------------------------ building blocks

fn gate(threshold: f32, floor: f32) -> EffectSlot {
    s(
        "noise-gate",
        &[
            ("Threshold", threshold),
            ("Attack", 1.0),
            ("Hold", 60.0),
            ("Release", 180.0),
            ("Floor", floor),
        ],
    )
}

fn comp(threshold: f32, ratio: f32, attack: f32, release: f32, makeup: f32) -> EffectSlot {
    s(
        "compressor",
        &[
            ("Threshold", threshold),
            ("Ratio", ratio),
            ("Attack", attack),
            ("Release", release),
            ("Knee", 6.0),
            ("Makeup", makeup),
        ],
    )
}

fn deess(freq: f32, threshold: f32, reduction: f32) -> EffectSlot {
    s(
        "de-esser",
        &[
            ("Frequency", freq),
            ("Threshold", threshold),
            ("Reduction", reduction),
        ],
    )
}

fn limit(ceiling: f32) -> EffectSlot {
    s(
        "limiter",
        &[("Ceiling", ceiling), ("Look-ahead", 5.0), ("Release", 80.0)],
    )
}

fn leveler() -> EffectSlot {
    s("af.opto_leveler", &[("peak", 35.0), ("tube", 15.0)])
}

/// The Parametric EQ with only the bands named turned up or down.
fn eq(sets: &[(&str, f32)]) -> EffectSlot {
    s("af.parametric_eq", sets)
}

/// A spoken-voice clean-up curve: low cut, less mud, a little presence and air.
fn voice_eq() -> EffectSlot {
    eq(&[
        ("hp_on", 1.0),
        ("hp_freq", 85.0),
        ("hp_slope", 1.0),
        ("b2_freq", 300.0),
        ("b2_gain", -2.0),
        ("b2_q", 1.2),
        ("b4_freq", 3500.0),
        ("b4_gain", 2.0),
        ("b4_q", 0.9),
        ("hs_on", 1.0),
        ("hs_freq", 11000.0),
        ("hs_gain", 2.0),
    ])
}

fn reverb(kind: f32, size: f32, decay: f32, predelay: f32, wet: f32) -> EffectSlot {
    s(
        "af.reverb",
        &[
            ("kind", kind),
            ("size", size),
            ("decay", decay),
            ("predelay", predelay),
            ("wet", wet),
        ],
    )
}

fn chorus(rate: f32, depth: f32) -> EffectSlot {
    s(
        "af.modulation",
        &[("kind", 0.0), ("rate", rate), ("depth", depth)],
    )
}

fn drive(amount: f32) -> EffectSlot {
    s("af.tube_preamp", &[("drive", amount)])
}

/// The project lead's echo settings from Audiform (time ms, feedback %,
/// tone Hz, wet %). The Echo here has no ping-pong.
fn echo(time: f32, feedback: f32, tone: f32, wet: f32) -> EffectSlot {
    s(
        "echo",
        &[
            ("Delay", time),
            ("Feedback", feedback / 100.0),
            ("Tone", tone),
            ("Mix", wet / 100.0),
        ],
    )
}

fn b(group: &'static str, name: &'static str, chain: Vec<EffectSlot>) -> Builtin {
    Builtin { group, name, chain }
}

/// Every built-in preset, in list order.
pub fn all() -> Vec<Builtin> {
    const VOICE: &str = "Voice";
    const CHAR: &str = "Character voices";
    const BREATH: &str = "Breath control";
    const DYN: &str = "Dynamics";
    const ECHO: &str = "Echo";
    const REPAIR: &str = "Repair";
    const EQ: &str = "EQ and filters";
    vec![
        b(
            VOICE,
            "Podcast Vocal",
            vec![
                gate(-50.0, -15.0),
                voice_eq(),
                leveler(),
                comp(-20.0, 3.0, 8.0, 150.0, 3.0),
                deess(6800.0, -30.0, 6.0),
                limit(-6.0),
            ],
        ),
        b(
            VOICE,
            "Podcast Vocal - Firm",
            vec![
                gate(-45.0, -25.0),
                voice_eq(),
                leveler(),
                comp(-24.0, 4.5, 5.0, 120.0, 5.0),
                deess(6500.0, -32.0, 8.0),
                limit(-6.0),
            ],
        ),
        b(
            VOICE,
            "Podcast Vocal - Light",
            vec![comp(-18.0, 2.0, 15.0, 200.0, 2.0), voice_eq(), limit(-6.0)],
        ),
        b(
            VOICE,
            "Podcast Vocal - Light (-3 dB)",
            vec![comp(-18.0, 2.0, 15.0, 200.0, 2.0), voice_eq(), limit(-3.0)],
        ),
        b(
            VOICE,
            "Podcast Vocal - Dense (-3 dB)",
            vec![
                leveler(),
                comp(-26.0, 4.0, 4.0, 100.0, 6.0),
                voice_eq(),
                deess(6300.0, -32.0, 6.0),
                limit(-3.0),
            ],
        ),
        b(
            VOICE,
            "Level and Gate Voice",
            vec![leveler(), gate(-42.0, -40.0)],
        ),
        b(
            VOICE,
            "Clean and Level Voiceover",
            vec![
                eq(&[("hp_on", 1.0), ("hp_freq", 75.0), ("hp_slope", 1.0)]),
                gate(-48.0, -20.0),
                leveler(),
                deess(6800.0, -30.0, 6.0),
                limit(-3.0),
            ],
        ),
        b(
            VOICE,
            "Broadcast Announcer",
            vec![
                comp(-30.0, 7.0, 3.0, 90.0, 8.0),
                eq(&[
                    ("hp_on", 1.0),
                    ("hp_freq", 55.0),
                    ("ls_freq", 110.0),
                    ("ls_gain", 5.0),
                    ("b2_freq", 450.0),
                    ("b2_gain", -3.0),
                    ("b4_freq", 3800.0),
                    ("b4_gain", 3.0),
                    ("hs_on", 1.0),
                    ("hs_freq", 9500.0),
                    ("hs_gain", 2.5),
                ]),
                deess(6500.0, -30.0, 6.0),
                limit(-1.0),
            ],
        ),
        b(
            VOICE,
            "Voiceover Polish",
            vec![
                leveler(),
                comp(-22.0, 3.0, 10.0, 150.0, 3.0),
                voice_eq(),
                deess(6800.0, -30.0, 6.0),
                limit(-3.0),
            ],
        ),
        b(
            VOICE,
            "De-Ess and Limit - Low Voice",
            vec![deess(5200.0, -30.0, 7.0), limit(-3.0)],
        ),
        b(
            VOICE,
            "De-Ess and Limit - High Voice",
            vec![deess(7500.0, -30.0, 7.0), limit(-3.0)],
        ),
        b(
            VOICE,
            "Call-In Guest Cleanup",
            vec![
                gate(-36.0, -30.0),
                eq(&[
                    ("hp_on", 1.0),
                    ("hp_freq", 200.0),
                    ("hp_slope", 1.0),
                    ("b3_freq", 1500.0),
                    ("b3_gain", 2.0),
                    ("lp_on", 1.0),
                    ("lp_freq", 5500.0),
                ]),
                comp(-24.0, 4.0, 5.0, 120.0, 4.0),
                deess(6000.0, -30.0, 6.0),
                limit(-3.0),
            ],
        ),
        b(
            CHAR,
            "Divine Voice",
            vec![
                eq(&[
                    ("hp_on", 1.0),
                    ("hp_freq", 70.0),
                    ("ls_freq", 160.0),
                    ("ls_gain", 4.0),
                    ("b4_freq", 2800.0),
                    ("b4_gain", 2.0),
                ]),
                comp(-22.0, 3.0, 8.0, 150.0, 3.0),
                reverb(1.0, 85.0, 4500.0, 40.0, 22.0),
                limit(-3.0),
            ],
        ),
        b(
            CHAR,
            "Infernal Voice",
            vec![
                eq(&[
                    ("hp_on", 1.0),
                    ("hp_freq", 60.0),
                    ("ls_freq", 120.0),
                    ("ls_gain", 6.0),
                    ("b3_freq", 1000.0),
                    ("b3_gain", -4.0),
                    ("hs_on", 1.0),
                    ("hs_freq", 7000.0),
                    ("hs_gain", -4.0),
                ]),
                drive(60.0),
                comp(-26.0, 5.0, 4.0, 120.0, 4.0),
                reverb(0.0, 30.0, 900.0, 5.0, 15.0),
                limit(-3.0),
            ],
        ),
        b(
            CHAR,
            "Villain Voice",
            vec![
                eq(&[
                    ("hp_on", 1.0),
                    ("hp_freq", 70.0),
                    ("ls_freq", 140.0),
                    ("ls_gain", 3.0),
                    ("b4_freq", 3200.0),
                    ("b4_gain", 3.0),
                ]),
                drive(30.0),
                comp(-24.0, 4.0, 5.0, 120.0, 4.0),
                reverb(2.0, 35.0, 1200.0, 10.0, 12.0),
                limit(-3.0),
            ],
        ),
        b(
            CHAR,
            "Otherworldly",
            vec![
                eq(&[
                    ("hp_on", 1.0),
                    ("hp_freq", 140.0),
                    ("b2_freq", 450.0),
                    ("b2_gain", -3.0),
                    ("hs_on", 1.0),
                    ("hs_freq", 9500.0),
                    ("hs_gain", 5.0),
                ]),
                chorus(0.3, 40.0),
                reverb(1.0, 90.0, 6000.0, 60.0, 35.0),
                limit(-3.0),
            ],
        ),
        b(
            CHAR,
            "Heavenly Voice",
            vec![
                eq(&[
                    ("hp_on", 1.0),
                    ("hp_freq", 110.0),
                    ("b4_freq", 4800.0),
                    ("b4_gain", 3.0),
                    ("hs_on", 1.0),
                    ("hs_freq", 10500.0),
                    ("hs_gain", 4.0),
                ]),
                chorus(0.5, 25.0),
                reverb(2.0, 70.0, 3200.0, 25.0, 25.0),
                limit(-3.0),
            ],
        ),
        b(BREATH, "Breath Control", vec![gate(-42.0, -40.0)]),
        b(BREATH, "Breath Control - Light", vec![gate(-36.0, -12.0)]),
        b(BREATH, "Breath Control - Medium", vec![gate(-30.0, -20.0)]),
        b(
            DYN,
            "Brickwall -0.1 dB",
            vec![s(
                "limiter",
                &[("Ceiling", -0.1), ("Look-ahead", 5.0), ("Release", 50.0)],
            )],
        ),
        b(
            DYN,
            "Voiceover Squeeze",
            vec![s(
                "compressor",
                &[
                    ("Threshold", -26.0),
                    ("Ratio", 4.0),
                    ("Attack", 4.0),
                    ("Release", 120.0),
                    ("Knee", 4.0),
                    ("Makeup", 6.0),
                ],
            )],
        ),
        b(ECHO, "Slapback Echo", vec![echo(110.0, 8.0, 5000.0, 28.0)]),
        b(
            ECHO,
            "Podcast Room Echo",
            vec![echo(240.0, 25.0, 4500.0, 18.0)],
        ),
        b(
            ECHO,
            "Ping-Pong Echo",
            vec![echo(375.0, 40.0, 6000.0, 30.0)],
        ),
        b(
            ECHO,
            "Long Dark Echo",
            vec![echo(700.0, 55.0, 2800.0, 35.0)],
        ),
        b(
            REPAIR,
            "Vocal Cut (Center)",
            vec![s("af.center_extractor", &[("center", -48.0)])],
        ),
        b(
            REPAIR,
            "Hiss Tamer",
            vec![
                s("low-pass", &[("Cutoff", 10000.0), ("Slope", 2.0)]),
                gate(-55.0, -12.0),
            ],
        ),
        b(
            REPAIR,
            "Room Tail Tamer",
            vec![s(
                "noise-gate",
                &[
                    ("Threshold", -38.0),
                    ("Attack", 1.0),
                    ("Hold", 30.0),
                    ("Release", 90.0),
                    ("Floor", -14.0),
                ],
            )],
        ),
        b(REPAIR, "DC Offset Fix", vec![s("dc-offset", &[])]),
        b(
            EQ,
            "Mains Hum 60 Hz",
            vec![s(
                "hum-remover",
                &[("Base", 60.0), ("Harmonics", 5.0), ("Q", 35.0)],
            )],
        ),
        b(
            EQ,
            "Mains Hum 50 Hz",
            vec![s(
                "hum-remover",
                &[("Base", 50.0), ("Harmonics", 5.0), ("Q", 35.0)],
            )],
        ),
        b(
            EQ,
            "Low Rumble Filter",
            vec![s("high-pass", &[("Cutoff", 75.0), ("Slope", 3.0)])],
        ),
        b(
            EQ,
            "Infrasonic Filter",
            vec![s("high-pass", &[("Cutoff", 28.0), ("Slope", 4.0)])],
        ),
        b(
            EQ,
            "Phone Line",
            vec![
                eq(&[
                    ("hp_on", 1.0),
                    ("hp_freq", 320.0),
                    ("hp_slope", 2.0),
                    ("b3_freq", 1700.0),
                    ("b3_gain", 4.0),
                    ("b3_q", 1.2),
                    ("lp_on", 1.0),
                    ("lp_freq", 3200.0),
                    ("lp_slope", 2.0),
                ]),
                drive(25.0),
            ],
        ),
        b(
            EQ,
            "Answering Machine",
            vec![
                eq(&[
                    ("hp_on", 1.0),
                    ("hp_freq", 450.0),
                    ("hp_slope", 2.0),
                    ("b3_freq", 1400.0),
                    ("b3_gain", 6.0),
                    ("b3_q", 1.5),
                    ("lp_on", 1.0),
                    ("lp_freq", 2600.0),
                    ("lp_slope", 2.0),
                ]),
                drive(55.0),
                limit(-3.0),
            ],
        ),
    ]
}

/// The built-in preset called `name`, as a rack preset.
pub fn find(name: &str) -> Option<(Vec<EffectSlot>, Rack)> {
    all()
        .into_iter()
        .find(|p| p.name == name)
        .map(|p| (p.chain, Rack::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_names_a_real_control_inside_its_range() {
        crate::fx::register();
        let mut names = std::collections::HashSet::new();
        for p in all() {
            assert!(names.insert(p.name), "repeated name {}", p.name);
            assert!(!p.chain.is_empty() && p.chain.len() <= 16, "{}", p.name);
            for slot in &p.chain {
                let controls = crate::fx::controls(&slot.effect);
                assert!(
                    !controls.is_empty() || slot.effect == "dc-offset",
                    "{}: {}",
                    p.name,
                    slot.effect
                );
                assert!(
                    crate::fx::catalog()
                        .iter()
                        .any(|(_, l)| l.iter().any(|(id, _)| *id == slot.effect)),
                    "{}: {} is not a live effect",
                    p.name,
                    slot.effect
                );
                for (c, v) in controls.iter().zip(&slot.values) {
                    assert!(
                        *v >= c.min && *v <= c.max,
                        "{}: {} {} = {v} outside {}..{}",
                        p.name,
                        slot.effect,
                        c.name,
                        c.min,
                        c.max
                    );
                }
            }
        }
    }

    #[test]
    fn keys_used_by_the_presets_all_exist() {
        // `s` skips an unknown key quietly; catch typos here.
        let check = |id: &str, keys: &[&str]| {
            for k in keys {
                assert!(index(id, k).is_some(), "{id} has no control {k}");
            }
        };
        check(
            "noise-gate",
            &["Threshold", "Attack", "Hold", "Release", "Floor"],
        );
        check(
            "compressor",
            &["Threshold", "Ratio", "Attack", "Release", "Knee", "Makeup"],
        );
        check("de-esser", &["Frequency", "Threshold", "Reduction"]);
        check("limiter", &["Ceiling", "Look-ahead", "Release"]);
        check("echo", &["Delay", "Feedback", "Tone", "Mix"]);
        check("high-pass", &["Cutoff", "Slope"]);
        check("low-pass", &["Cutoff", "Slope"]);
        check("hum-remover", &["Base", "Harmonics", "Q"]);
        check("af.opto_leveler", &["peak", "tube"]);
        check("af.tube_preamp", &["drive"]);
        check("af.modulation", &["kind", "rate", "depth"]);
        check("af.reverb", &["kind", "size", "decay", "predelay", "wet"]);
        check("af.center_extractor", &["center"]);
        check(
            "af.parametric_eq",
            &[
                "hp_on", "hp_freq", "hp_slope", "ls_freq", "ls_gain", "b2_freq", "b2_gain", "b2_q",
                "b3_freq", "b3_gain", "b3_q", "b4_freq", "b4_gain", "b4_q", "hs_on", "hs_freq",
                "hs_gain", "lp_on", "lp_freq", "lp_slope",
            ],
        );
    }

    #[test]
    fn the_echoes_keep_the_project_leads_settings() {
        let (chain, _) = find("Slapback Echo").unwrap();
        let v = &chain[0].values;
        assert_eq!(chain[0].effect, "echo");
        assert_eq!(v[index("echo", "Delay").unwrap()], 110.0);
        assert!((v[index("echo", "Feedback").unwrap()] - 0.08).abs() < 1e-6);
        assert_eq!(v[index("echo", "Tone").unwrap()], 5000.0);
        assert!((v[index("echo", "Mix").unwrap()] - 0.28).abs() < 1e-6);
    }
}
