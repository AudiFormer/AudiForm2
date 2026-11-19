// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The keyboard shortcuts: every command a key can run, its default keys,
//! and the user's own map (saved as `keymap.json` in the data folder).
//! The window's key handling and the menus' shortcut text both read from
//! here, so a key rebound in Keyboard Shortcuts works everywhere at once.

use std::collections::BTreeMap;

use egui::{Key, Modifiers};
use serde::{Deserialize, Serialize};

use crate::i18n::tr;

/// A command a shortcut can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Cmd {
    NewFile,
    Open,
    Save,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Crop,
    SelectAll,
    Delete,
    AddMarker,
    ClearSelection,
    KeyboardShortcuts,
    Preferences,
    SplitAtPlayhead,
    Invert,
    Reverse,
    Silence,
    MatchLoudness,
    AutoHeal,
    PlayStop,
    Record,
    Loop,
    ToStart,
    ToEnd,
    WaveformView,
    MultitrackView,
    ZoomIn,
    ZoomOut,
    ZoomFull,
    MoveTool,
    RazorTool,
    TimeSelectTool,
}

impl Cmd {
    pub const ALL: &'static [Cmd] = &[
        Cmd::NewFile,
        Cmd::Open,
        Cmd::Save,
        Cmd::Undo,
        Cmd::Redo,
        Cmd::Cut,
        Cmd::Copy,
        Cmd::Paste,
        Cmd::Crop,
        Cmd::SelectAll,
        Cmd::Delete,
        Cmd::AddMarker,
        Cmd::ClearSelection,
        Cmd::KeyboardShortcuts,
        Cmd::Preferences,
        Cmd::SplitAtPlayhead,
        Cmd::Invert,
        Cmd::Reverse,
        Cmd::Silence,
        Cmd::MatchLoudness,
        Cmd::AutoHeal,
        Cmd::PlayStop,
        Cmd::Record,
        Cmd::Loop,
        Cmd::ToStart,
        Cmd::ToEnd,
        Cmd::WaveformView,
        Cmd::MultitrackView,
        Cmd::ZoomIn,
        Cmd::ZoomOut,
        Cmd::ZoomFull,
        Cmd::MoveTool,
        Cmd::RazorTool,
        Cmd::TimeSelectTool,
    ];

    /// The name shown, in the current language (the saved map uses the
    /// variant names instead).
    pub fn label(self) -> &'static str {
        match self {
            Cmd::NewFile => tr("New Audio File"),
            Cmd::Open => tr("Open..."),
            Cmd::Save => tr("Save"),
            Cmd::Undo => tr("Undo"),
            Cmd::Redo => tr("Redo"),
            Cmd::Cut => tr("Cut"),
            Cmd::Copy => tr("Copy"),
            Cmd::Paste => tr("Paste"),
            Cmd::Crop => tr("Crop"),
            Cmd::SelectAll => tr("Select All"),
            Cmd::Delete => tr("Delete"),
            Cmd::AddMarker => tr("Add Marker"),
            Cmd::ClearSelection => tr("Deselect"),
            Cmd::KeyboardShortcuts => tr("Keyboard Shortcuts..."),
            Cmd::Preferences => tr("Preferences..."),
            Cmd::SplitAtPlayhead => tr("Split at Playhead"),
            Cmd::Invert => tr("Invert"),
            Cmd::Reverse => tr("Reverse"),
            Cmd::Silence => tr("Silence"),
            Cmd::MatchLoudness => tr("Match Loudness"),
            Cmd::AutoHeal => tr("Auto Heal Selection"),
            Cmd::PlayStop => tr("Play / Stop"),
            Cmd::Record => tr("Record"),
            Cmd::Loop => tr("Loop Playback"),
            Cmd::ToStart => tr("Move Playhead to Start"),
            Cmd::ToEnd => tr("Move Playhead to End"),
            Cmd::WaveformView => tr("Waveform Editor"),
            Cmd::MultitrackView => tr("Multitrack Editor"),
            Cmd::ZoomIn => tr("Zoom In"),
            Cmd::ZoomOut => tr("Zoom Out"),
            Cmd::ZoomFull => tr("Zoom Out Full"),
            Cmd::MoveTool => tr("Move Tool"),
            Cmd::RazorTool => tr("Razor Tool"),
            Cmd::TimeSelectTool => tr("Time Selection Tool"),
        }
    }

    /// The heading shown above it, in the current language.
    pub fn group(self) -> &'static str {
        match self {
            Cmd::NewFile | Cmd::Open | Cmd::Save => tr("File"),
            Cmd::Undo
            | Cmd::Redo
            | Cmd::Cut
            | Cmd::Copy
            | Cmd::Paste
            | Cmd::Crop
            | Cmd::SelectAll
            | Cmd::Delete
            | Cmd::AddMarker
            | Cmd::ClearSelection
            | Cmd::KeyboardShortcuts
            | Cmd::Preferences => tr("Edit"),
            Cmd::SplitAtPlayhead => tr("Multitrack"),
            Cmd::Invert | Cmd::Reverse | Cmd::Silence | Cmd::MatchLoudness | Cmd::AutoHeal => {
                tr("Effects")
            }
            Cmd::PlayStop | Cmd::Record | Cmd::Loop | Cmd::ToStart | Cmd::ToEnd => tr("Transport"),
            Cmd::WaveformView
            | Cmd::MultitrackView
            | Cmd::ZoomIn
            | Cmd::ZoomOut
            | Cmd::ZoomFull => tr("View"),
            Cmd::MoveTool | Cmd::RazorTool | Cmd::TimeSelectTool => tr("Tools"),
        }
    }

    /// Runs even while a text box has the keyboard (Save, Undo...).
    pub fn global(self) -> bool {
        matches!(
            self,
            Cmd::NewFile | Cmd::Open | Cmd::Save | Cmd::Undo | Cmd::Redo
        )
    }

    pub fn defaults(self) -> Vec<Binding> {
        use Key as K;
        let c = |k| Binding::ctrl(k);
        let p = |k| Binding::plain(k);
        match self {
            Cmd::NewFile => vec![c(K::N)],
            Cmd::Open => vec![c(K::O)],
            Cmd::Save => vec![c(K::S)],
            Cmd::Undo => vec![c(K::Z)],
            Cmd::Redo => vec![
                Binding {
                    shift: true,
                    ..c(K::Z)
                },
                c(K::Y),
            ],
            Cmd::Cut => vec![c(K::X)],
            Cmd::Copy => vec![c(K::C)],
            Cmd::Paste => vec![c(K::V)],
            Cmd::Crop => vec![c(K::T)],
            Cmd::SelectAll => vec![c(K::A)],
            Cmd::Delete => vec![p(K::Delete), p(K::Backspace)],
            Cmd::AddMarker => vec![p(K::M)],
            Cmd::ClearSelection => vec![p(K::Escape)],
            Cmd::KeyboardShortcuts => vec![Binding {
                alt: true,
                ..p(K::K)
            }],
            Cmd::SplitAtPlayhead => vec![c(K::K)],
            Cmd::AutoHeal => vec![c(K::U)],
            Cmd::PlayStop => vec![p(K::Space)],
            Cmd::Record => vec![Binding {
                shift: true,
                ..p(K::Space)
            }],
            Cmd::Loop => vec![c(K::L)],
            Cmd::ToStart => vec![p(K::Home)],
            Cmd::ToEnd => vec![p(K::End)],
            Cmd::WaveformView => vec![p(K::Num9)],
            Cmd::MultitrackView => vec![p(K::Num0)],
            Cmd::ZoomIn => vec![p(K::Equals), p(K::Plus)],
            Cmd::ZoomOut => vec![p(K::Minus)],
            Cmd::MoveTool => vec![p(K::V)],
            Cmd::RazorTool => vec![p(K::R)],
            Cmd::TimeSelectTool => vec![p(K::T)],
            Cmd::Preferences
            | Cmd::Invert
            | Cmd::Reverse
            | Cmd::Silence
            | Cmd::MatchLoudness
            | Cmd::ZoomFull => Vec::new(),
        }
    }
}

/// One key combination. Ctrl is the command key (Cmd on a Mac).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Binding {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Key,
}

impl Binding {
    pub fn plain(key: Key) -> Binding {
        Binding {
            ctrl: false,
            shift: false,
            alt: false,
            key,
        }
    }

    pub fn ctrl(key: Key) -> Binding {
        Binding {
            ctrl: true,
            ..Binding::plain(key)
        }
    }

    pub fn from_event(key: Key, m: Modifiers) -> Binding {
        Binding {
            ctrl: m.command || m.ctrl,
            shift: m.shift,
            alt: m.alt,
            key,
        }
    }

    pub fn modifiers(self) -> Modifiers {
        let mut m = if self.ctrl {
            Modifiers::COMMAND
        } else {
            Modifiers::NONE
        };
        m.shift = self.shift;
        m.alt = self.alt;
        m
    }

    /// How many modifiers it needs: the more, the sooner it is tried, so
    /// Ctrl+Shift+Z is never taken for Ctrl+Z.
    fn weight(self) -> u8 {
        self.ctrl as u8 + self.shift as u8 + self.alt as u8
    }

    /// Reads "Ctrl+Shift+Z" and the like.
    pub fn parse(s: &str) -> Option<Binding> {
        let mut b = Binding::plain(Key::Space);
        let mut key = None;
        for part in s.split('+') {
            let part = part.trim();
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "cmd" | "command" => b.ctrl = true,
                "shift" => b.shift = true,
                "alt" | "option" => b.alt = true,
                "" => key = Some(Key::Plus),
                _ => key = Some(Key::from_name(part)?),
            }
        }
        b.key = key?;
        Some(b)
    }
}

impl std::fmt::Display for Binding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.ctrl {
            f.write_str(if cfg!(target_os = "macos") {
                "Cmd+"
            } else {
                "Ctrl+"
            })?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        f.write_str(self.key.name())
    }
}

/// Every command's keys.
#[derive(Debug, Clone, PartialEq)]
pub struct Keymap {
    binds: BTreeMap<Cmd, Vec<Binding>>,
}

impl Default for Keymap {
    fn default() -> Keymap {
        Keymap {
            binds: Cmd::ALL.iter().map(|&c| (c, c.defaults())).collect(),
        }
    }
}

/// The saved form: command name to its keys as text.
type Saved = BTreeMap<Cmd, Vec<String>>;

impl Keymap {
    pub fn path() -> std::path::PathBuf {
        crate::state::data_dir().join("keymap.json")
    }

    /// The saved map, or the defaults. Commands the file does not name
    /// keep their default keys.
    pub fn load() -> Keymap {
        std::fs::read_to_string(Keymap::path())
            .ok()
            .and_then(|s| Keymap::from_json(&s))
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let p = Keymap::path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, self.to_json());
    }

    pub fn to_json(&self) -> String {
        let saved: Saved = self
            .binds
            .iter()
            .map(|(c, v)| (*c, v.iter().map(|b| b.to_string()).collect()))
            .collect();
        serde_json::to_string_pretty(&saved).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Option<Keymap> {
        // Read loosely: an unknown command (from a later version) is
        // skipped rather than losing the whole file.
        let raw: BTreeMap<String, Vec<String>> = serde_json::from_str(s).ok()?;
        let mut km = Keymap::default();
        for (name, keys) in raw {
            let Ok(cmd) = serde_json::from_value::<Cmd>(serde_json::Value::String(name)) else {
                continue;
            };
            km.binds
                .insert(cmd, keys.iter().filter_map(|k| Binding::parse(k)).collect());
        }
        Some(km)
    }

    pub fn get(&self, cmd: Cmd) -> &[Binding] {
        self.binds.get(&cmd).map_or(&[], Vec::as_slice)
    }

    /// The first key, as menus show it.
    pub fn text(&self, cmd: Cmd) -> String {
        self.get(cmd)
            .first()
            .map(|b| b.to_string())
            .unwrap_or_default()
    }

    /// The command a key runs.
    #[cfg(test)]
    pub fn lookup(&self, b: Binding) -> Option<Cmd> {
        self.binds
            .iter()
            .find(|(_, v)| v.contains(&b))
            .map(|(c, _)| *c)
    }

    /// Other commands using the same key as `cmd`'s binding `b`.
    pub fn clashes(&self, cmd: Cmd, b: Binding) -> Vec<Cmd> {
        self.binds
            .iter()
            .filter(|(c, v)| **c != cmd && v.contains(&b))
            .map(|(c, _)| *c)
            .collect()
    }

    /// Every key used by more than one command.
    pub fn conflicts(&self) -> Vec<(Binding, Vec<Cmd>)> {
        let mut seen: Vec<(Binding, Vec<Cmd>)> = Vec::new();
        for (c, v) in &self.binds {
            for b in v {
                match seen.iter_mut().find(|(x, _)| x == b) {
                    Some((_, cmds)) => cmds.push(*c),
                    None => seen.push((*b, vec![*c])),
                }
            }
        }
        seen.retain(|(_, cmds)| cmds.len() > 1);
        seen
    }

    /// Sets `cmd`'s binding number `slot` (or adds one past the end).
    pub fn set(&mut self, cmd: Cmd, slot: usize, b: Binding) {
        let v = self.binds.entry(cmd).or_default();
        if slot < v.len() {
            v[slot] = b;
        } else if !v.contains(&b) {
            v.push(b);
        }
    }

    pub fn remove(&mut self, cmd: Cmd, slot: usize) {
        if let Some(v) = self.binds.get_mut(&cmd) {
            if slot < v.len() {
                v.remove(slot);
            }
        }
    }

    pub fn reset(&mut self, cmd: Cmd) {
        self.binds.insert(cmd, cmd.defaults());
    }

    /// Every (key, command), the keys needing the most modifiers first.
    pub fn dispatch_order(&self) -> Vec<(Binding, Cmd)> {
        let mut v: Vec<(Binding, Cmd)> = self
            .binds
            .iter()
            .flat_map(|(c, bs)| bs.iter().map(move |b| (*b, *c)))
            .collect();
        v.sort_by_key(|(b, _)| std::cmp::Reverse(b.weight()));
        v
    }

    /// The commands pressed this frame (their keys are consumed). With
    /// `typing`, only the global ones (Save, Undo...) are taken.
    pub fn pressed(&self, ctx: &egui::Context, typing: bool) -> Vec<Cmd> {
        let mut out = Vec::new();
        ctx.input_mut(|i| {
            for (b, c) in self.dispatch_order() {
                if typing && !c.global() {
                    continue;
                }
                if i.consume_key(b.modifiers(), b.key) {
                    out.push(c);
                }
            }
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_have_no_conflicts() {
        assert!(Keymap::default().conflicts().is_empty());
    }

    #[test]
    fn every_binding_reads_back_from_its_text() {
        for c in Cmd::ALL {
            for b in c.defaults() {
                assert_eq!(Binding::parse(&b.to_string()), Some(b), "{b}");
            }
        }
    }

    #[test]
    fn a_saved_map_loads_back_the_same() {
        let mut km = Keymap::default();
        km.set(Cmd::Invert, 0, Binding::ctrl(Key::I));
        km.remove(Cmd::Delete, 1);
        let back = Keymap::from_json(&km.to_json()).unwrap();
        assert_eq!(back, km);
    }

    #[test]
    fn unknown_commands_in_the_file_are_skipped() {
        let km = Keymap::from_json(r#"{"NoSuchThing":["Ctrl+Q"],"Save":["Ctrl+W"]}"#).unwrap();
        assert_eq!(km.get(Cmd::Save), &[Binding::ctrl(Key::W)]);
        assert_eq!(km.get(Cmd::Open), &[Binding::ctrl(Key::O)]);
    }

    #[test]
    fn rebinding_changes_what_a_key_runs() {
        let mut km = Keymap::default();
        assert_eq!(km.lookup(Binding::ctrl(Key::S)), Some(Cmd::Save));
        km.set(Cmd::Save, 0, Binding::ctrl(Key::W));
        assert_eq!(km.lookup(Binding::ctrl(Key::W)), Some(Cmd::Save));
        assert_eq!(km.lookup(Binding::ctrl(Key::S)), None);
        assert_eq!(km.text(Cmd::Save), Binding::ctrl(Key::W).to_string());
    }

    #[test]
    fn a_clash_is_reported() {
        let mut km = Keymap::default();
        km.set(Cmd::Invert, 0, Binding::ctrl(Key::S));
        assert_eq!(
            km.clashes(Cmd::Invert, Binding::ctrl(Key::S)),
            vec![Cmd::Save]
        );
        assert_eq!(km.conflicts().len(), 1);
        km.reset(Cmd::Invert);
        assert!(km.conflicts().is_empty());
    }

    #[test]
    fn more_specific_keys_are_tried_first() {
        let order = Keymap::default().dispatch_order();
        let redo = order
            .iter()
            .position(|(b, _)| b.ctrl && b.shift && b.key == Key::Z)
            .unwrap();
        let undo = order
            .iter()
            .position(|(b, _)| *b == Binding::ctrl(Key::Z))
            .unwrap();
        assert!(redo < undo);
    }
}
