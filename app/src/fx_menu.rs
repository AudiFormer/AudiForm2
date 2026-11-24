// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The Effects menu's category submenus, named as long-time editors name
//! them, and which of our effects goes in each.

use af2_dsp::Effect;

/// One entry in a category submenu.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// A built-in effect (`af2-dsp`).
    Builtin(Effect),
    /// A ported Audiform effect (`af2-fx`): (id, name).
    Fx(String, String),
    /// AI Stem Separation, in its own tab.
    Stems,
    /// Spatialize to Surround, in its own window.
    Spatialize,
    /// A Diagnostics scan, in the Diagnostics panel.
    Diagnose(crate::diagnostics::Kind),
    /// A VST3 plug-in: (id, name). Only among Favorite Effects; the menu
    /// lists plug-ins under VST 3.
    Plugin(String, String),
}

impl Item {
    pub fn name(&self) -> String {
        match self {
            Item::Builtin(e) => e.name().to_string(),
            Item::Fx(_, n) => n.clone(),
            Item::Stems => "AI Stem Separation...".to_string(),
            Item::Spatialize => "Spatialize to Surround...".to_string(),
            Item::Diagnose(k) => format!("{}...", k.name()),
            Item::Plugin(_, n) => n.clone(),
        }
    }

    /// What Favorite Effects keeps it as (with its name).
    pub fn key(&self) -> String {
        match self {
            Item::Builtin(e) => format!("dsp:{}", e.id()),
            Item::Fx(id, _) | Item::Plugin(id, _) => id.clone(),
            Item::Stems => "af2:stems".into(),
            Item::Spatialize => "af2:spatialize".into(),
            Item::Diagnose(k) => format!("diag:{}", k.name()),
        }
    }

    /// The item kept as `key`; None for one no longer offered.
    pub fn from_key(key: &str, name: &str) -> Option<Item> {
        if let Some(id) = key.strip_prefix("dsp:") {
            return Effect::from_id(id).map(Item::Builtin);
        }
        if let Some(k) = key.strip_prefix("diag:") {
            return crate::diagnostics::Kind::ALL.into_iter().find(|x| x.name() == k).map(Item::Diagnose);
        }
        if key.starts_with(af2_vst3::PREFIX) {
            return Some(Item::Plugin(key.to_string(), name.to_string()));
        }
        match key {
            "af2:stems" => Some(Item::Stems),
            "af2:spatialize" => Some(Item::Spatialize),
            _ => af2_fx::info(key).map(|i| Item::Fx(key.to_string(), i.name.to_string())),
        }
    }
}

pub const CATEGORIES: [&str; 10] = [
    "Amplitude and Compression",
    "Delay and Echo",
    "Diagnostics",
    "Filter and EQ",
    "Modulation",
    "Noise Reduction / Restoration",
    "Reverb",
    "Special",
    "Stereo Imagery",
    "Time and Pitch",
];

/// The effects with their own items at the top of the menu.
#[cfg(test)]
pub const TOP_LEVEL: [Effect; 4] = [
    Effect::Invert,
    Effect::Reverse,
    Effect::Silence,
    Effect::MatchLoudness,
];

/// Built-in effects kept for the sessions that use them but no longer
/// offered: the Reverb and Stereo Width, whose places are taken by
/// Audiform's Reverb (`af.reverb`) and Stereo Width (`af.stereo_width`); the Echo, whose place is taken by Delay and Echo (`af.echo`),
/// and the 3-Band EQ, whose place is taken by Band EQ (`af.band_eq`).
/// Compressor, Limiter, Noise Gate, De-Esser and Amplify give their places
/// to Audiform's Dynamics, Limiter, Breath Gate, De-Esser and Gain.
/// High-Pass, Low-Pass, Notch, Hum Remover and Remove DC Offset give
/// theirs to Audiform's Precision Filter, Notch Filter, Hum Cut and DC
/// Offset Fix.
pub const RETIRED: [Effect; 14] = [
    Effect::Echo,
    Effect::ParametricEq,
    Effect::Compressor,
    Effect::Limiter,
    Effect::NoiseGate,
    Effect::DeEsser,
    Effect::Amplify,
    Effect::Reverb,
    Effect::StereoWidth,
    Effect::HighPass,
    Effect::LowPass,
    Effect::Notch,
    Effect::HumRemover,
    Effect::DcOffset,
];

/// Audiform effects kept for the sessions that use them but not offered
/// in the menus or the rack: the Studio Reverb and the Convolution
/// Reverb (the Reverb's Studio and Impulse Response tabs), and Phase
/// Rotate and the Center Remover (retired in Audiform too: Phase Curve
/// and the Center Channel Extractor do what they did).
pub const HIDDEN_FX: &[&str] = &[
    "af.studio_reverb",
    "af.convolution_reverb",
    "af.phase_rotate",
    "af.center_remover",
];

/// Where a built-in effect goes.
fn builtin_category(e: Effect) -> Option<&'static str> {
    use Effect as E;
    if RETIRED.contains(&e) {
        return None;
    }
    Some(match e {
        E::Amplify
        | E::Normalize
        | E::FadeIn
        | E::FadeOut
        | E::Compressor
        | E::Limiter
        | E::NoiseGate
        | E::DeEsser => "Amplitude and Compression",
        E::Echo => "Delay and Echo",
        E::ParametricEq | E::HighPass | E::LowPass | E::Notch => "Filter and EQ",
        E::NoiseReduction | E::HumRemover | E::DcOffset => "Noise Reduction / Restoration",
        E::Reverb => "Reverb",
        E::StereoWidth => "Stereo Imagery",
        E::Speed => "Time and Pitch",
        E::Invert | E::Reverse | E::Silence | E::MatchLoudness => return None,
    })
}

/// Where an Audiform effect goes, by the category it was filed under
/// (Delay and Echo by name: it is filed with the reverbs).
fn fx_category(id: &str, category: &str) -> &'static str {
    if id == af2_fx::effects::echo::ID {
        return "Delay and Echo";
    }
    match category {
        "Amplitude" | "Dynamics" => "Amplitude and Compression",
        "Delay and Reverb" | "Reverb" => "Reverb",
        "Delay" | "Echo" => "Delay and Echo",
        "Modulation" => "Modulation",
        "Stereo" => "Stereo Imagery",
        "Filter" | "EQ" => "Filter and EQ",
        "Restoration" => "Noise Reduction / Restoration",
        "Time" | "Pitch" => "Time and Pitch",
        _ => "Special",
    }
}

/// Each category and what is in it, in menu order.
pub fn categories() -> Vec<(&'static str, Vec<Item>)> {
    let mut out: Vec<(&'static str, Vec<Item>)> =
        CATEGORIES.iter().map(|c| (*c, Vec::new())).collect();
    let mut put = |cat: &str, item: Item| {
        if let Some((_, v)) = out.iter_mut().find(|(c, _)| *c == cat) {
            v.push(item);
        }
    };
    for e in Effect::all() {
        if let Some(cat) = builtin_category(e) {
            put(cat, Item::Builtin(e));
        }
    }
    for info in af2_fx::registry() {
        if HIDDEN_FX.contains(&info.id) {
            continue;
        }
        put(
            fx_category(info.id, info.category),
            Item::Fx(info.id.to_string(), info.name.to_string()),
        );
    }
    put("Special", Item::Stems);
    put("Stereo Imagery", Item::Spatialize);
    for k in crate::diagnostics::Kind::ALL {
        put("Diagnostics", Item::Diagnose(k));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_comes_back_from_its_key() {
        let mut all: Vec<Item> = categories().into_iter().flat_map(|(_, v)| v).collect();
        all.extend(TOP_LEVEL.map(Item::Builtin));
        all.push(Item::Plugin(format!("{}0123", af2_vst3::PREFIX), "Some Plug-In".into()));
        for item in all {
            assert_eq!(Item::from_key(&item.key(), &item.name()), Some(item.clone()), "{}", item.name());
        }
        assert_eq!(Item::from_key("af.no_such_effect", "Gone"), None);
    }

    #[test]
    fn every_effect_has_a_place() {
        let cats = categories();
        let items: Vec<&Item> = cats.iter().flat_map(|(_, v)| v.iter()).collect();
        for e in Effect::all() {
            let offered = items.contains(&&Item::Builtin(e));
            assert!(
                TOP_LEVEL.contains(&e) || offered || RETIRED.contains(&e),
                "{} is in no menu",
                e.name()
            );
            assert!(!(offered && RETIRED.contains(&e)), "{}", e.name());
        }
        for info in af2_fx::registry() {
            let offered = items
                .iter()
                .any(|i| matches!(i, Item::Fx(id, _) if id == info.id));
            assert_eq!(offered, !HIDDEN_FX.contains(&info.id), "{}", info.id);
        }
        assert!(cats
            .iter()
            .any(|(c, v)| *c == "Special" && v.contains(&Item::Stems)));
        let (_, diag) = cats.iter().find(|(c, _)| *c == "Diagnostics").unwrap();
        assert_eq!(diag.len(), crate::diagnostics::Kind::ALL.len());
    }

    #[test]
    fn delay_and_echo_is_the_echo_offered() {
        let cats = categories();
        let (_, delays) = cats.iter().find(|(c, _)| *c == "Delay and Echo").unwrap();
        assert_eq!(
            delays,
            &vec![Item::Fx("af.echo".into(), "Delay and Echo".into())]
        );
    }

    #[test]
    fn the_rack_offers_the_menus_categories() {
        let groups: Vec<String> = crate::fx::catalog().into_iter().map(|(g, _)| g).collect();
        assert!(groups.iter().all(|g| CATEGORIES.contains(&g.as_str())), "{groups:?}");
        assert!(groups.iter().all(|g| !g.contains("(Audiform)")));
        let amp = crate::fx::catalog()
            .into_iter()
            .find(|(g, _)| g == "Amplitude and Compression")
            .unwrap()
            .1;
        assert!(amp.iter().any(|(id, _)| id.starts_with("af.")), "Audiform effects sit with the built-in ones");
    }
}
