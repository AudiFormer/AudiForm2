// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Workspaces: ready-made arrangements of the dockable panels.
//!
//! Music is the one the app starts with (there is no separate Default): a
//! left column running the full height (Files, then the effect rack, then
//! History), the Editor in the centre with the Mixer and Piano Roll behind
//! it, a right column of analysis panels, and the Levels meter along the
//! bottom with the selection and view times at its right end. The others
//! rearrange the same panels for a job. Every workspace can be changed by dragging; the
//! changes are kept until Window > Workspace > Reset.

use egui_dock::{DockState, NodeIndex};

use crate::ui::Tab;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workspace {
    Music,
    Mixing,
    FocusedEdit,
    Mastering,
    SpokenWord,
}

impl Workspace {
    pub const ALL: [Workspace; 5] = [
        Workspace::Music,
        Workspace::Mixing,
        Workspace::FocusedEdit,
        Workspace::Mastering,
        Workspace::SpokenWord,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Workspace::Music => "Music",
            Workspace::Mixing => "Mixing",
            Workspace::FocusedEdit => "Focused Edit",
            Workspace::Mastering => "Mastering",
            Workspace::SpokenWord => "Spoken Word",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Workspace::Music => "Editor, Mixer and Piano Roll in the middle; files, effects and History on the left; Phase Wheel, Visualizer and Spectrum together on the right, with 3D Space and AI Stems below; Levels along the bottom",
            Workspace::Mixing => "The Mixer up front with tall meters; files and markers on the left; the track Inspector on the right",
            Workspace::FocusedEdit => "As much room as possible for the timeline: files on the left, levels below",
            Workspace::Mastering => "Loudness statistics, spectrum and Phase Wheel on the right; the effect rack on the left",
            Workspace::SpokenWord => "For podcasts and voice work: files, markers and the effect rack on the left; Diagnostics and the live Loudness meter on the right",
        }
    }

    /// For the saved setting.
    pub fn key(self) -> &'static str {
        match self {
            Workspace::Music => "music",
            Workspace::Mixing => "mixing",
            Workspace::FocusedEdit => "focused-edit",
            Workspace::Mastering => "mastering",
            Workspace::SpokenWord => "spoken-word",
        }
    }

    /// Bumped when the built-in arrangement changes, so an arrangement
    /// saved from the old one is set aside for the new.
    pub fn version(self) -> u32 {
        match self {
            // 5: the Piano Roll sits with the Editor and Mixer.
            Workspace::Music => 5,
            _ => 3,
        }
    }

    /// The saved setting's workspace; the old "default" (and anything
    /// unknown) is Music now.
    pub fn from_key(k: &str) -> Workspace {
        Workspace::ALL
            .into_iter()
            .find(|w| w.key() == k.trim())
            .unwrap_or(Workspace::Music)
    }

    /// The arrangement, built as the window is: the left column first so
    /// it runs the full height, then the bottom row under the centre and
    /// right, then the right column. Fractions are the left or top part.
    pub fn build(self) -> DockState<Tab> {
        use Tab::*;
        match self {
            // The project lead's arrangement: files, effects and history
            // down the left; the scopes, then 3D Space and AI Stems down the
            // right; Levels and Range along the bottom.
            Workspace::Music => {
                let mut d = DockState::new(vec![Editor, Mixer, PianoRoll]);
                let s = d.main_surface_mut();
                let [centre, left] = s.split_left(NodeIndex::root(), 0.215, vec![Files, Markers]);
                let [_files, rack] = s.split_below(left, 0.40, vec![Effects, Properties]);
                let [_rack, _history] = s.split_below(rack, 0.69, vec![History, Video]);
                let [centre, bottom] = s.split_below(centre, 0.85, vec![Levels]);
                let [_levels, _range] = s.split_right(bottom, 0.76, vec![Selection]);
                let [_editor, right] =
                    s.split_right(centre, 0.77, vec![Phase, Visualizer, Spectrum, Statistics]);
                let [_scopes, _lower] = s.split_below(right, 0.48, vec![Space, AiSeparation]);
                d
            }
            Workspace::Mixing => {
                let mut d = DockState::new(vec![Mixer, Editor, PianoRoll]);
                let s = d.main_surface_mut();
                let [centre, left] = s.split_left(NodeIndex::root(), 0.15, vec![Files]);
                let [_files, _markers] = s.split_below(left, 0.55, vec![Markers, History, Video]);
                let [centre, bottom] = s.split_below(centre, 0.8, vec![Levels]);
                let [_levels, _range] = s.split_right(bottom, 0.8, vec![Selection]);
                let [_mixer, _right] = s.split_right(centre, 0.78, vec![Effects, Properties]);
                d
            }
            Workspace::FocusedEdit => {
                let mut d = DockState::new(vec![Editor, Mixer, PianoRoll]);
                let s = d.main_surface_mut();
                let [centre, _left] =
                    s.split_left(NodeIndex::root(), 0.13, vec![Files, Markers, History, Video]);
                let [_editor, bottom] = s.split_below(centre, 0.88, vec![Levels]);
                let [_levels, _range] = s.split_right(bottom, 0.8, vec![Selection]);
                d
            }
            Workspace::Mastering => {
                let mut d = DockState::new(vec![Editor, Mixer]);
                let s = d.main_surface_mut();
                let [centre, left] =
                    s.split_left(NodeIndex::root(), 0.18, vec![Effects, Properties]);
                let [_rack, _files] = s.split_below(left, 0.62, vec![Files, History, Video]);
                let [centre, bottom] = s.split_below(centre, 0.8, vec![Levels]);
                let [_levels, _range] = s.split_right(bottom, 0.76, vec![Selection]);
                let [_editor, right] = s.split_right(centre, 0.72, vec![Statistics]);
                let [_stats, lower] = s.split_below(right, 0.42, vec![Spectrum]);
                let [_spec, _phase] = s.split_below(lower, 0.5, vec![Phase, Visualizer]);
                d
            }
            Workspace::SpokenWord => {
                let mut d = DockState::new(vec![Editor, Mixer]);
                let s = d.main_surface_mut();
                let [centre, left] = s.split_left(NodeIndex::root(), 0.18, vec![Files, Markers]);
                let [_files, _rack] = s.split_below(left, 0.45, vec![Effects, History, Video]);
                let [centre, bottom] = s.split_below(centre, 0.85, vec![Levels]);
                let [_levels, _range] = s.split_right(bottom, 0.76, vec![Selection]);
                let [_editor, right] = s.split_right(centre, 0.74, vec![Diagnostics]);
                let [_diag, _loud] = s.split_below(right, 0.55, vec![Loudness, Statistics]);
                d
            }
        }
    }
}

/// Puts `tab` in the same panel as `beside`, behind it (not brought to
/// the front); in a window of its own when `beside` isn't there. Nothing
/// when `tab` is already somewhere.
pub fn add_behind(d: &mut DockState<Tab>, beside: Tab, tab: Tab) {
    if d.find_tab(&tab).is_some() {
        return;
    }
    let Some((s, n, _)) = d.find_tab(&beside) else {
        d.add_window(vec![tab]);
        return;
    };
    if let egui_dock::Node::Leaf { tabs, .. } = &mut d[s][n] {
        tabs.push(tab);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_workspace_builds_and_has_the_editor_and_levels() {
        for w in Workspace::ALL {
            let d = w.build();
            assert!(d.find_tab(&Tab::Editor).is_some(), "{}", w.name());
            assert!(d.find_tab(&Tab::Levels).is_some(), "{}", w.name());
            assert_eq!(Workspace::from_key(w.key()), w);
        }
        assert_eq!(Workspace::from_key("nope"), Workspace::Music);
        assert_eq!(Workspace::from_key("default"), Workspace::Music);
    }

    #[test]
    fn music_groups_the_scopes_and_gives_space_and_stems_their_own_panels() {
        let d = Workspace::Music.build();
        let node = |t: Tab| d.find_tab(&t).map(|(s, n, _)| (s, n)).unwrap();
        assert_eq!(node(Tab::Phase), node(Tab::Visualizer));
        assert_eq!(node(Tab::Phase), node(Tab::Spectrum));
        assert_ne!(node(Tab::Space), node(Tab::Phase));
        assert_ne!(node(Tab::AiSeparation), node(Tab::Phase));
    }

    #[test]
    fn video_sits_behind_history_in_every_workspace() {
        for w in Workspace::ALL {
            let d = w.build();
            let (vs, vn, _) = d.find_tab(&Tab::Video).expect("video");
            let (hs, hn, _) = d.find_tab(&Tab::History).expect("history");
            assert_eq!((vs, vn), (hs, hn), "{}", w.name());
            // Behind: never the tab in front.
            if let egui_dock::Node::Leaf { tabs, active, .. } = &d[hs][hn] {
                assert_ne!(tabs[active.0], Tab::Video, "{}", w.name());
                assert_eq!(tabs.last(), Some(&Tab::Video), "{}", w.name());
            }
        }
        // An arrangement saved before: added once, behind History.
        let mut old = DockState::new(vec![Tab::Editor]);
        let s = old.main_surface_mut();
        let _ = s.split_left(NodeIndex::root(), 0.2, vec![Tab::History]);
        add_behind(&mut old, Tab::History, Tab::Video);
        add_behind(&mut old, Tab::History, Tab::Video);
        let (s, n, _) = old.find_tab(&Tab::Video).unwrap();
        assert_eq!(Some((s, n)), old.find_tab(&Tab::History).map(|(s, n, _)| (s, n)));
    }

    #[test]
    fn the_spoken_word_tools_stay_out_of_the_other_layouts() {
        for w in Workspace::ALL {
            let d = w.build();
            let has = d.find_tab(&Tab::Diagnostics).is_some() && d.find_tab(&Tab::Loudness).is_some();
            assert_eq!(has, w == Workspace::SpokenWord, "{}", w.name());
        }
    }

    #[test]
    fn music_shows_every_main_panel_and_the_piano_roll_beside_the_editor() {
        let d = Workspace::Music.build();
        let node = |t: Tab| d.find_tab(&t).map(|(s, n, _)| (s, n)).unwrap();
        assert_eq!(node(Tab::PianoRoll), node(Tab::Editor));
        assert_eq!(node(Tab::Mixer), node(Tab::Editor));
        for t in [
            Tab::Files,
            Tab::Markers,
            Tab::Properties,
            Tab::Effects,
            Tab::AiSeparation,
            Tab::History,
            Tab::Mixer,
            Tab::Selection,
            Tab::Phase,
            Tab::Visualizer,
            Tab::Spectrum,
            Tab::Statistics,
            Tab::Space,
        ] {
            assert!(d.find_tab(&t).is_some(), "{t:?}");
        }
    }
}
