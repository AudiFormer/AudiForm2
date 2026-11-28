// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Closing the app with unsaved work: each unsaved audio file and session
//! in turn asks "Save changes to '...' before closing?" with Yes, Yes To
//! All, No, No To All and Cancel. Yes saves it (through Save As first when
//! it needs a name or a format) and waits until the save has finished;
//! the app closes once every one has been answered and saved.

use egui::{Align2, RichText};

use crate::state::{DocId, Mode, State};
use crate::theme::c;

/// Something with unsaved changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Doc(DocId),
    /// The session in front.
    Front,
    /// A session set aside, by its key.
    Parked(u64),
}

/// The close in progress.
pub struct QuitFlow {
    queue: Vec<Item>,
    /// Being saved: waiting for its Save As or its save to finish.
    waiting: Option<Item>,
    /// Yes To All was chosen.
    yes_all: bool,
}

/// How a frame of the close went.
pub enum Outcome {
    Asking,
    Cancelled,
    Done,
}

fn unsaved(st: &State) -> Vec<Item> {
    let mut v: Vec<Item> = st.docs.iter().filter(|d| d.dirty).map(|d| Item::Doc(d.id)).collect();
    if st.session_dirty && !st.front_is_blank() {
        v.push(Item::Front);
    }
    v.extend(st.sessions.parked.iter().filter(|p| p.dirty).map(|p| Item::Parked(p.key)));
    v
}

fn name(st: &State, it: Item) -> String {
    match it {
        Item::Doc(id) => st.docs.iter().find(|d| d.id == id).map_or("an audio file".into(), |d| d.name.clone()),
        Item::Front => st.session.name.clone(),
        Item::Parked(k) => st.sessions.parked.iter().find(|p| p.key == k).map_or("a session".into(), |p| p.session.name.clone()),
    }
}

/// Still unsaved?
fn still_unsaved(st: &State, it: Item) -> bool {
    match it {
        Item::Doc(id) => st.docs.iter().any(|d| d.id == id && d.dirty),
        Item::Front => st.session_dirty,
        Item::Parked(k) => st.sessions.parked.iter().any(|p| p.key == k && p.dirty),
    }
}

/// Starts saving `it`. True: saved already (nothing to wait for).
fn save(st: &mut State, it: Item) -> bool {
    match it {
        Item::Doc(id) => {
            st.current = Some(id);
            st.mode = Mode::Waveform;
            crate::export::save_in_place(st);
            false
        }
        Item::Front => {
            st.mode = Mode::Multitrack;
            st.save_front(false)
        }
        Item::Parked(k) => {
            // To the front, then as the front one.
            if let Some(i) = st.sessions.parked.iter().position(|p| p.key == k) {
                st.switch_session(i);
            }
            st.mode = Mode::Multitrack;
            st.save_front(false)
        }
    }
}

/// A save under way: a Save As open, or a save job running.
fn busy(st: &State) -> bool {
    st.export.open || st.session_save.is_some() || !st.jobs.is_empty()
}

impl QuitFlow {
    /// None when nothing is unsaved.
    pub fn start(st: &State) -> Option<QuitFlow> {
        let queue = unsaved(st);
        (!queue.is_empty()).then_some(QuitFlow { queue, waiting: None, yes_all: false })
    }
}

/// Runs the close each frame: asks about the next unsaved item, or waits
/// for the save of the last one.
pub fn run(ctx: &egui::Context, st: &mut State, flow: &mut QuitFlow) -> Outcome {
    if let Some(it) = flow.waiting {
        if busy(st) {
            return Outcome::Asking;
        }
        flow.waiting = None;
        if still_unsaved(st, it) {
            // Its Save As was cancelled, or the save failed: stay open.
            return Outcome::Cancelled;
        }
    }
    // Answered or saved elsewhere meanwhile: skip it.
    while flow.queue.first().is_some_and(|it| !still_unsaved(st, *it)) {
        flow.queue.remove(0);
    }
    let Some(&it) = flow.queue.first() else {
        return Outcome::Done;
    };
    let answer = if flow.yes_all { Some(Answer::Yes) } else { ask(ctx, st, it, flow.queue.len() > 1) };
    match answer {
        None => Outcome::Asking,
        Some(Answer::Cancel) => Outcome::Cancelled,
        Some(Answer::NoAll) => Outcome::Done,
        Some(Answer::No) => {
            flow.queue.remove(0);
            Outcome::Asking
        }
        Some(a @ (Answer::Yes | Answer::YesAll)) => {
            if a == Answer::YesAll {
                flow.yes_all = true;
            }
            flow.queue.remove(0);
            if !save(st, it) {
                flow.waiting = Some(it);
            } else if still_unsaved(st, it) {
                return Outcome::Cancelled;
            }
            Outcome::Asking
        }
    }
}

/// A dialog's main button (OK, Yes): filled with the theme's accent, its
/// text in the colour that reads on it.
pub fn main_button(th: &crate::theme::Theme, text: &str) -> egui::Button<'static> {
    let [r, g, b] = th.accent;
    let light = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32 > 140.0;
    let ink = if light { egui::Color32::from_gray(20) } else { egui::Color32::WHITE };
    egui::Button::new(RichText::new(text.to_string()).color(ink).strong())
        .fill(c(th.accent))
        .min_size(egui::vec2(86.0, 26.0))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    Yes,
    YesAll,
    No,
    NoAll,
    Cancel,
}

/// The question for one item.
fn ask(ctx: &egui::Context, st: &State, it: Item, more: bool) -> Option<Answer> {
    let th = st.theme.clone();
    let mut answer = None;
    egui::Area::new(egui::Id::new("af2-quit-dim"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let r = ctx.screen_rect();
            ui.painter().rect_filled(r, 0.0, egui::Color32::from_black_alpha(110));
            ui.allocate_rect(r, egui::Sense::click());
        });
    egui::Window::new("AudiForm 2")
        .id(egui::Id::new("af2-quit-ask"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("⚠").size(26.0).color(c(th.meter_mid)));
                ui.add_space(8.0);
                ui.label(format!("Save changes to '{}' before closing?", name(st, it)));
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let b = |ui: &mut egui::Ui, t: &str, main: bool| {
                    if main {
                        ui.add(main_button(&th, t)).clicked()
                    } else {
                        ui.add(egui::Button::new(t).min_size(egui::vec2(84.0, 26.0))).clicked()
                    }
                };
                if b(ui, "Yes", true) {
                    answer = Some(Answer::Yes);
                }
                if more && b(ui, "Yes To All", false) {
                    answer = Some(Answer::YesAll);
                }
                if b(ui, "No", false) {
                    answer = Some(Answer::No);
                }
                if more && b(ui, "No To All", false) {
                    answer = Some(Answer::NoAll);
                }
                if b(ui, "Cancel", false) {
                    answer = Some(Answer::Cancel);
                }
            });
            ui.add_space(4.0);
        });
    if answer.is_none() {
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Enter) {
                answer = Some(Answer::Yes);
            } else if i.key_pressed(egui::Key::Escape) {
                answer = Some(Answer::Cancel);
            }
        });
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unsaved_work_is_asked_about() {
        let mut st = State::new();
        assert!(QuitFlow::start(&st).is_none(), "nothing open: closes straight away");
        let id = st.add_doc("take.wav".into(), None, af2_core::AudioBuffer::new(48_000, 1, 48_000));
        assert!(QuitFlow::start(&st).is_none(), "an unchanged file is not asked about");
        st.docs.iter_mut().find(|d| d.id == id).unwrap().dirty = true;
        let flow = QuitFlow::start(&st).expect("an edited file is asked about");
        assert_eq!(flow.queue, vec![Item::Doc(id)]);
        assert_eq!(name(&st, Item::Doc(id)), "take.wav");
    }
}
