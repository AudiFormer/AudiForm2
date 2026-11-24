// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Favorite Effects: effects and VST3 plug-ins a click away, on the
//! Favorites bar (a tab, where they are added and removed) and in the
//! Favorites menu. Clicking one does what picking it from the Effects
//! menu does, and always brings its window up.

use crate::fx_menu::Item;
use crate::keymap::Cmd;
use crate::state::State;
use crate::ui::Tab;
use serde::{Deserialize, Serialize};

/// One favorite: what the Effects menu item is kept as, and its name
/// (a plug-in's, for when it is not installed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Favorite {
    pub key: String,
    pub name: String,
}

/// What a click on a favorite asks of the app.
#[derive(Default)]
pub struct Asked {
    pub cmd: Option<Cmd>,
    pub tab: Option<Tab>,
}

pub fn is_favorite(st: &State, key: &str) -> bool {
    st.prefs.favorites.iter().any(|f| f.key == key)
}

/// Adds `item` at the end, or takes it off when it is there.
pub fn toggle(st: &mut State, item: &Item) {
    toggle_in(&mut st.prefs.favorites, item);
    st.prefs.save();
}

fn toggle_in(list: &mut Vec<Favorite>, item: &Item) {
    let key = item.key();
    if let Some(i) = list.iter().position(|f| f.key == key) {
        list.remove(i);
    } else {
        list.push(Favorite { key, name: item.name().trim_end_matches("...").to_string() });
    }
}

/// Moves the favorite at `i` one place earlier (or later).
fn shift(st: &mut State, i: usize, earlier: bool) {
    if shift_in(&mut st.prefs.favorites, i, earlier) {
        st.prefs.save();
    }
}

fn shift_in(list: &mut [Favorite], i: usize, earlier: bool) -> bool {
    let j = if earlier { i.checked_sub(1) } else { (i + 1 < list.len()).then_some(i + 1) };
    if let Some(j) = j {
        list.swap(i, j);
    }
    j.is_some()
}

/// The favorites as items; a plug-in shows its saved name.
fn items(st: &State) -> Vec<(usize, Option<Item>, String)> {
    st.prefs
        .favorites
        .iter()
        .enumerate()
        .map(|(i, f)| (i, Item::from_key(&f.key, &f.name), f.name.clone()))
        .collect()
}

/// One favorite's button, with Remove and Move on a right-click.
fn button(ui: &mut egui::Ui, st: &mut State, at: usize, item: Option<Item>, name: &str, n: usize, asked: &mut Asked) {
    let r = match &item {
        Some(_) => ui.button(name),
        None => ui
            .add_enabled(false, egui::Button::new(name))
            .on_disabled_hover_text("No longer offered here; right-click to remove it"),
    };
    let r = r.on_hover_text("Right-click to remove or move");
    if r.clicked() {
        if let Some(item) = item.clone() {
            crate::ui::pick_effect(st, item, true, &mut asked.tab, &mut asked.cmd);
        }
    }
    r.context_menu(|ui| {
        if ui.button("Remove from Favorites").clicked() {
            st.prefs.favorites.remove(at);
            st.prefs.save();
            ui.close_menu();
        }
        if ui.add_enabled(at > 0, egui::Button::new("Move Earlier")).clicked() {
            shift(st, at, true);
            ui.close_menu();
        }
        if ui.add_enabled(at + 1 < n, egui::Button::new("Move Later")).clicked() {
            shift(st, at, false);
            ui.close_menu();
        }
    });
}

/// The top-level Favorites menu.
pub fn menu(ui: &mut egui::Ui, st: &mut State, asked: &mut Asked) {
    let list = items(st);
    if list.is_empty() {
        ui.add_enabled(false, egui::Button::new("No favorites yet"));
    }
    for (_, item, name) in list {
        let r = ui.add_enabled(item.is_some(), egui::Button::new(&name));
        if r.clicked() {
            if let Some(item) = item {
                crate::ui::pick_effect(st, item, true, &mut asked.tab, &mut asked.cmd);
            }
            ui.close_menu();
        }
    }
    ui.separator();
    if ui.button("Show Favorites Bar").on_hover_text("Add and remove favorites there").clicked() {
        asked.tab = Some(Tab::Favorites);
        ui.close_menu();
    }
}

/// The Favorites bar: a tab (with Levels to start with) holding one row of
/// buttons, the last of them + to add effects. What a click asks for is
/// left in `st.favorites_cmd` and `open_tab`.
pub fn panel(ui: &mut egui::Ui, st: &mut State, open_tab: &mut Option<Tab>) {
    let mut asked = Asked::default();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let list = items(st);
        let n = list.len();
        for (at, item, name) in list {
            button(ui, st, at, item, &name, n, &mut asked);
        }
        let add = if n == 0 { "+ Add an Effect" } else { "+" };
        ui.menu_button(add, |ui| add_menu(ui, st))
            .response
            .on_hover_text("Add effects and plug-ins to this bar (ticked ones are on it)");
    });
    if asked.tab.is_some() {
        *open_tab = asked.tab;
    }
    if asked.cmd.is_some() {
        st.favorites_cmd = asked.cmd;
    }
}

/// Every effect the Effects menu offers, and the VST3 plug-ins, each ticked
/// when it is on the bar; a click adds or takes it off.
fn add_menu(ui: &mut egui::Ui, st: &mut State) {
    let row = |ui: &mut egui::Ui, st: &mut State, item: Item| {
        let mut on = is_favorite(st, &item.key());
        if ui.checkbox(&mut on, item.name()).clicked() {
            toggle(st, &item);
        }
    };
    ui.menu_button("Basic", |ui| {
        for e in [af2_dsp::Effect::Invert, af2_dsp::Effect::Reverse, af2_dsp::Effect::Silence, af2_dsp::Effect::MatchLoudness] {
            row(ui, st, Item::Builtin(e));
        }
    });
    for (cat, list) in crate::fx_menu::categories() {
        if list.is_empty() {
            continue;
        }
        ui.menu_button(cat, |ui| {
            for item in list {
                row(ui, st, item);
            }
        });
    }
    ui.separator();
    ui.menu_button("VST 3", |ui| {
        if let Some(id) = crate::plugins::picker_items(ui) {
            let name = crate::plugins::name(&id);
            toggle(st, &Item::Plugin(id, name));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_twice_removes_and_the_order_can_change() {
        let mut list = Vec::new();
        let echo = Item::Fx(af2_fx::effects::echo::ID.into(), "Delay and Echo".into());
        let invert = Item::Builtin(af2_dsp::Effect::Invert);
        toggle_in(&mut list, &echo);
        toggle_in(&mut list, &invert);
        assert_eq!(list.len(), 2);
        assert!(shift_in(&mut list, 1, true));
        assert_eq!(list[0].key, invert.key());
        assert!(!shift_in(&mut list, 0, true));
        toggle_in(&mut list, &invert);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].key, echo.key());
    }
}
