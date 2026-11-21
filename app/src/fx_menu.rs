// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The Effects menu's category submenus, named as long-time editors name
//! them, and which of our effects goes in each.

use af27_dsp::Effect;

/// One entry in a category submenu.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// A built-in effect (`af27-dsp`).
    Builtin(Effect),
    /// A ported Audiform effect (`af27-fx`): (id, name).
    Fx(String, String),
    /// AI Stem Separation, in its own tab.
    Stems,
}

impl Item {
    pub fn name(&self) -> String {
        match self {
            Item::Builtin(e) => e.name().to_string(),
            Item::Fx(_, n) => n.clone(),
            Item::Stems => "AI Stem Separation...".to_string(),
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
/// offered: the Echo, whose place is taken by Delay and Echo (`af.echo`).
pub const RETIRED: [Effect; 1] = [Effect::Echo];

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
    if id == af27_fx::effects::echo::ID {
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
    for info in af27_fx::registry() {
        put(
            fx_category(info.id, info.category),
            Item::Fx(info.id.to_string(), info.name.to_string()),
        );
    }
    put("Special", Item::Stems);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for info in af27_fx::registry() {
            assert!(items
                .iter()
                .any(|i| matches!(i, Item::Fx(id, _) if id == info.id)));
        }
        assert!(cats
            .iter()
            .any(|(c, v)| *c == "Special" && v.contains(&Item::Stems)));
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
